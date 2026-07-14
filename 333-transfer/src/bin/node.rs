// KG: transfer333 multi-process node binary (TCP authorities + submit client)
//
// Runnable process that reuses the library Authority / Committee / Ledger /
// TcpAuthorityNet / collect-until-quorum path. Hand-rolled CLI + JSON lines
// (no clap, no serde). Driven by an external harness that reads stdout events.

use std::collections::BTreeMap;
use std::io::{self, BufRead, Write};
use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use transfer333::{
    Authority, AuthorityError, AuthorityMsg, AuthorityNet, Certified, Committee, Ledger,
    SigningKey, TcpAuthorityNet, Transfer, VoteCollector,
};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    // args[0] = binary path
    match args.get(1).map(String::as_str) {
        Some("authority") => match run_authority(&args[2..]) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("authority error: {e}");
                ExitCode::from(1)
            }
        },
        Some("submit") => match run_submit(&args[2..]) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("submit error: {e}");
                ExitCode::from(1)
            }
        },
        _ => {
            usage();
            ExitCode::from(1)
        }
    }
}

fn usage() {
    eprintln!(
        "usage:
  node authority --id <a0> --seed <0> --listen <127.0.0.1:PORT> --peers <addr,addr,...> --committee a0=0,a1=1,... [--genesis alice=100,bob=0] [--rounds-idle-exit <n>]
  node submit --seed <99> --listen <127.0.0.1:PORT> --peers <authority_addrs,...> --committee a0=0,... --transfer <alice:0:bob:30> [--max-rounds 200] [--pause-ms 10]"
    );
}

// --- hand-rolled flag parsing ------------------------------------------------

struct Flags {
    map: BTreeMap<String, String>,
}

impl Flags {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut map = BTreeMap::new();
        let mut i = 0;
        while i < args.len() {
            let a = &args[i];
            if !a.starts_with("--") {
                return Err(format!("unexpected argument: {a}"));
            }
            let key = a[2..].to_string();
            if key.is_empty() {
                return Err("empty flag".into());
            }
            i += 1;
            if i >= args.len() {
                return Err(format!("flag --{key} requires a value"));
            }
            map.insert(key, args[i].clone());
            i += 1;
        }
        Ok(Self { map })
    }

    fn require(&self, key: &str) -> Result<&str, String> {
        self.map
            .get(key)
            .map(String::as_str)
            .ok_or_else(|| format!("missing required --{key}"))
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.map.get(key).map(String::as_str)
    }
}

// --- JSON emit (no serde) ----------------------------------------------------

fn escape_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn transfer_str(t: &Transfer) -> String {
    format!("{}:{}:{}:{}", t.from, t.from_seq, t.to, t.amount)
}

fn balances_json(ledger: &Ledger) -> String {
    let mut parts = Vec::new();
    for (id, bal) in ledger.balances() {
        parts.push(format!("\"{}\":{}", escape_json(&id), bal));
    }
    format!("{{{}}}", parts.join(","))
}

/// Print one compact JSON object on its own line and flush immediately.
fn emit(obj: &str) {
    let mut out = io::stdout();
    let _ = writeln!(out, "{obj}");
    let _ = out.flush();
}

// --- shared parsers ----------------------------------------------------------

fn parse_seed(s: &str) -> Result<u8, String> {
    s.parse::<u8>()
        .map_err(|_| format!("invalid seed (need u8 0..255): {s}"))
}

fn signing_key_from_seed(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

/// `a0=0,a1=1,a2=2,a3=3` → Committee with deterministic keys.
fn parse_committee(s: &str) -> Result<Committee, String> {
    let mut members = Vec::new();
    for part in s.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (id, seed_s) = part
            .split_once('=')
            .ok_or_else(|| format!("bad committee entry (want id=seed): {part}"))?;
        let seed = parse_seed(seed_s.trim())?;
        let sk = signing_key_from_seed(seed);
        members.push((id.trim().to_string(), sk.verifying_key()));
    }
    Committee::new(members).ok_or_else(|| "empty committee".into())
}

/// `alice=100,bob=0`
fn parse_genesis(s: &str) -> Result<Ledger, String> {
    let mut alloc = Vec::new();
    for part in s.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (id, bal_s) = part
            .split_once('=')
            .ok_or_else(|| format!("bad genesis entry (want name=amount): {part}"))?;
        let bal: u128 = bal_s
            .trim()
            .parse()
            .map_err(|_| format!("bad genesis amount: {bal_s}"))?;
        alloc.push((id.trim().to_string(), bal));
    }
    if alloc.is_empty() {
        return Err("empty genesis".into());
    }
    Ok(Ledger::genesis(alloc))
}

/// `alice:0:bob:30`
fn parse_transfer(s: &str) -> Result<Transfer, String> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 4 {
        return Err(format!(
            "bad transfer (want from:seq:to:amount): {s}"
        ));
    }
    let from_seq: u64 = parts[1]
        .parse()
        .map_err(|_| format!("bad transfer seq: {}", parts[1]))?;
    let amount: u128 = parts[3]
        .parse()
        .map_err(|_| format!("bad transfer amount: {}", parts[3]))?;
    Ok(Transfer {
        from: parts[0].to_string(),
        from_seq,
        to: parts[2].to_string(),
        amount,
    })
}

fn parse_peers(s: &str) -> Result<Vec<SocketAddr>, String> {
    let mut out = Vec::new();
    for part in s.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let addr: SocketAddr = part
            .parse()
            .map_err(|_| format!("bad peer address: {part}"))?;
        out.push(addr);
    }
    Ok(out)
}

fn parse_listen(s: &str) -> Result<SocketAddr, String> {
    s.parse()
        .map_err(|_| format!("bad listen address: {s}"))
}

// --- authority subcommand ----------------------------------------------------

fn run_authority(args: &[String]) -> Result<(), String> {
    let flags = Flags::parse(args).map_err(|e| {
        usage();
        e
    })?;
    let id = flags.require("id")?.to_string();
    let seed = parse_seed(flags.require("seed")?)?;
    let listen = parse_listen(flags.require("listen")?)?;
    let peers = parse_peers(flags.require("peers")?)?;
    let committee = parse_committee(flags.require("committee")?)?;
    let genesis_s = flags.get("genesis").unwrap_or("alice=100,bob=0");
    let mut ledger = parse_genesis(genesis_s)?;
    let idle_exit: Option<usize> = match flags.get("rounds-idle-exit") {
        Some(s) => Some(
            s.parse()
                .map_err(|_| format!("bad --rounds-idle-exit: {s}"))?,
        ),
        None => None,
    };

    let mut auth = Authority::new(id.clone(), signing_key_from_seed(seed));
    // Public key must match committee entry for this id.
    if let Some(expected) = committee.key_of(&id) {
        if auth.verifying_key() != *expected {
            return Err(format!(
                "authority --id {id} --seed {seed} does not match --committee key"
            ));
        }
    } else {
        return Err(format!("authority id {id} not in --committee"));
    }

    let net = TcpAuthorityNet::bind_at(id.clone(), listen)
        .map_err(|e| format!("bind {listen}: {e}"))?;
    emit(&format!(
        "{{\"event\":\"listening\",\"id\":\"{}\",\"addr\":\"{}\"}}",
        escape_json(net.id()),
        escape_json(&net.addr().to_string())
    ));

    net.connect_all(&peers)
        .map_err(|e| format!("connect_all: {e:?}"))?;
    // Brief settle so accept threads register peer streams.
    thread::sleep(Duration::from_millis(20));
    emit(&format!(
        "{{\"event\":\"committee_ready\",\"n\":{}}}",
        committee.size()
    ));

    let stop = Arc::new(AtomicBool::new(false));
    let stop_stdin = Arc::clone(&stop);
    thread::spawn(move || {
        let stdin = io::stdin();
        let mut line = String::new();
        // Any line or EOF → clean shutdown.
        let _ = stdin.lock().read_line(&mut line);
        stop_stdin.store(true, Ordering::SeqCst);
    });

    let endpoint = net.endpoint();
    let mut idle_rounds = 0usize;
    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }

        let msgs = endpoint.poll();
        if msgs.is_empty() {
            idle_rounds += 1;
            if let Some(n) = idle_exit {
                if idle_rounds >= n {
                    break;
                }
            }
            thread::sleep(Duration::from_millis(5));
            continue;
        }
        idle_rounds = 0;

        for msg in msgs {
            match msg {
                AuthorityMsg::Order(t) => match auth.handle(&t) {
                    Ok(vote) => {
                        let _ = endpoint.broadcast_vote(vote);
                        emit(&format!(
                            "{{\"event\":\"vote_cast\",\"transfer\":\"{}\",\"authority\":\"{}\"}}",
                            escape_json(&transfer_str(&t)),
                            escape_json(&id)
                        ));
                    }
                    Err(AuthorityError::Equivocation { account, seq }) => {
                        emit(&format!(
                            "{{\"event\":\"equivocation_rejected\",\"transfer\":\"{}\",\"authority\":\"{}\",\"account\":\"{}\",\"seq\":{}}}",
                            escape_json(&transfer_str(&t)),
                            escape_json(&id),
                            escape_json(&account),
                            seq
                        ));
                    }
                    Err(AuthorityError::OutOfOrder {
                        account,
                        expected,
                        got,
                    }) => {
                        emit(&format!(
                            "{{\"event\":\"out_of_order\",\"transfer\":\"{}\",\"authority\":\"{}\",\"account\":\"{}\",\"expected\":{},\"got\":{}}}",
                            escape_json(&transfer_str(&t)),
                            escape_json(&id),
                            escape_json(&account),
                            expected,
                            got
                        ));
                    }
                },
                AuthorityMsg::Cert(c) => {
                    if let Some(v) = c.verify(&committee) {
                        // Double delivery is possible under full-mesh + accept-as-write;
                        // only emit on successful apply. confirm is monotonic/idempotent.
                        match ledger.apply_verified(&v) {
                            Ok(()) => {
                                auth.confirm(&v);
                                emit(&format!(
                                    "{{\"event\":\"cert_applied\",\"authority\":\"{}\",\"balances\":{},\"total_supply\":{}}}",
                                    escape_json(&id),
                                    balances_json(&ledger),
                                    ledger.total_supply()
                                ));
                            }
                            Err(_) => {
                                // Already applied (or reject); still confirm for seq advance.
                                auth.confirm(&v);
                            }
                        }
                    }
                }
                AuthorityMsg::Vote(_) => {
                    // Authorities ignore peer votes; the submit client assembles.
                }
            }
        }
        thread::sleep(Duration::from_millis(2));
    }

    net.shutdown();
    Ok(())
}

// --- submit subcommand -------------------------------------------------------

fn run_submit(args: &[String]) -> Result<ExitCode, String> {
    let flags = Flags::parse(args).map_err(|e| {
        usage();
        e
    })?;
    let seed = parse_seed(flags.require("seed")?)?;
    let listen = parse_listen(flags.require("listen")?)?;
    let peers = parse_peers(flags.require("peers")?)?;
    let committee = parse_committee(flags.require("committee")?)?;
    let transfer = parse_transfer(flags.require("transfer")?)?;
    let max_rounds: usize = flags
        .get("max-rounds")
        .unwrap_or("200")
        .parse()
        .map_err(|e| format!("bad --max-rounds: {e}"))?;
    let pause_ms: u64 = flags
        .get("pause-ms")
        .unwrap_or("10")
        .parse()
        .map_err(|e| format!("bad --pause-ms: {e}"))?;

    // Client identity is only used for the TCP peer id; seed is reserved for
    // future client signing and keeps the CLI symmetric with authority.
    let client_id = format!("client-{seed}");
    let net = TcpAuthorityNet::bind_at(client_id, listen)
        .map_err(|e| format!("bind {listen}: {e}"))?;
    emit(&format!(
        "{{\"event\":\"listening\",\"id\":\"{}\",\"addr\":\"{}\"}}",
        escape_json(net.id()),
        escape_json(&net.addr().to_string())
    ));

    // Client dials every authority (one-way); authorities do not list the client.
    net.connect_each(&peers)
        .map_err(|e| format!("connect_each: {e:?}"))?;
    thread::sleep(Duration::from_millis(30));
    emit(&format!(
        "{{\"event\":\"committee_ready\",\"n\":{}}}",
        committee.size()
    ));

    let endpoint = net.endpoint();
    endpoint
        .broadcast_order(transfer.clone())
        .map_err(|e| format!("broadcast_order: {e:?}"))?;

    let mut coll = VoteCollector::new(transfer.clone());
    let (cert, status) = coll.collect_until_quorum_with_pause(
        &endpoint,
        &committee,
        max_rounds,
        Duration::from_millis(pause_ms),
    );

    let code = match status {
        Certified::Ok => {
            let cert = cert.expect("Certified::Ok implies assembled certificate");
            endpoint
                .broadcast_cert(cert)
                .map_err(|e| format!("broadcast_cert: {e:?}"))?;
            // Give authorities a moment to receive/apply before we tear down the
            // client connections (accepted write peers die with our process).
            thread::sleep(Duration::from_millis(50));
            emit(&format!(
                "{{\"event\":\"certified\",\"transfer\":\"{}\",\"status\":\"Ok\"}}",
                escape_json(&transfer_str(&transfer))
            ));
            ExitCode::SUCCESS
        }
        failed => {
            // Rejected double-spend / out-of-order / sub-quorum is a correct
            // protocol outcome — exit 0 so the harness can assert on the event.
            let reason = format!("{failed:?}");
            emit(&format!(
                "{{\"event\":\"cert_failed\",\"transfer\":\"{}\",\"reason\":\"{}\"}}",
                escape_json(&transfer_str(&transfer)),
                escape_json(&reason)
            ));
            ExitCode::SUCCESS
        }
    };

    net.shutdown();
    Ok(code)
}
