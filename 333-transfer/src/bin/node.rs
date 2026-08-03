// KG: transfer333 multi-process node binary (TCP authorities + submit client)
//
// Runnable process that reuses the library Authority / Committee / Ledger /
// TcpAuthorityNet / collect-until-quorum path. Hand-rolled CLI + JSON lines
// (no clap, no serde). Driven by an external harness that reads stdout events.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use transfer333::{
    Authority, AuthorityError, AuthorityMsg, AuthorityNet, Certificate, Certified, Committee,
    ConfirmError, ConfirmOutcome, FileJournal, Ledger, NetworkId, OwnerAuthError, OwnerRegistry,
    SignedTransfer, SigningKey,
    TcpAuthorityNet, Transfer, TransferPolicy, VerifyingKey, VoteCollector,
};
use zeroize::Zeroizing;

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
  node authority --id <a0> (--key-file <hex-key-file> | --dev-seed <0>) --network-id <deployment> --owner-roster alice=<pubhex>,bob=<pubhex> --listen <127.0.0.1:PORT> --peers <addr,addr,...> --committee a0=<pubhex>,a1=<pubhex>,... [--genesis alice=100,bob=0] [--journal <path>] [--rounds-idle-exit <n>]
  node submit (--key-file <hex-key-file> | --dev-seed <42>) --network-id <deployment> --owner-roster alice=<pubhex>,bob=<pubhex> --listen <127.0.0.1:PORT> --peers <authority_addrs,...> --committee a0=<pubhex>,... --transfer <alice:0:bob:30> [--max-rounds 200] [--pause-ms 10]

`--dev-seed` is deterministic debug-build scaffolding only and is rejected by
release builds. Production rosters contain public keys; private authority/owner
keys are loaded separately."
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

fn order_id_hex(order: &SignedTransfer) -> String {
    order
        .order_id()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
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

#[cfg(debug_assertions)]
fn parse_dev_seed(s: &str) -> Result<u8, String> {
    s.parse::<u8>()
        .map_err(|_| format!("invalid seed (need u8 0..255): {s}"))
}

#[cfg(debug_assertions)]
fn signing_key_from_dev_seed(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn decode_hex_32(s: &str, label: &str) -> Result<[u8; 32], String> {
    if s.len() != 64 || !s.is_ascii() {
        return Err(format!("{label} must be exactly 64 hexadecimal characters"));
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16)
            .map_err(|_| format!("{label} contains non-hexadecimal characters"))?;
    }
    Ok(out)
}

fn parse_verifying_key(s: &str, label: &str) -> Result<VerifyingKey, String> {
    VerifyingKey::from_bytes(&decode_hex_32(s, label)?)
        .map_err(|_| format!("{label} is not a valid Ed25519 public key"))
}

fn load_signing_key(flags: &Flags) -> Result<SigningKey, String> {
    match (flags.get("key-file"), flags.get("dev-seed")) {
        (Some(_), Some(_)) => Err("use exactly one of --key-file or --dev-seed".into()),
        (None, None) => Err("missing --key-file (or test-only --dev-seed)".into()),
        (Some(path), None) => {
            let encoded = Zeroizing::new(
                std::fs::read_to_string(path)
                    .map_err(|e| format!("cannot read --key-file: {e}"))?,
            );
            let bytes = Zeroizing::new(decode_hex_32(
                encoded.trim(),
                "private key file",
            )?);
            Ok(SigningKey::from_bytes(&bytes))
        }
        (None, Some(seed)) => {
            #[cfg(debug_assertions)]
            {
                Ok(signing_key_from_dev_seed(parse_dev_seed(seed)?))
            }
            #[cfg(not(debug_assertions))]
            {
                let _ = seed;
                Err("--dev-seed is disabled in release builds; use --key-file".into())
            }
        }
    }
}

/// Public-key-only authority roster. Private seeds never appear in this value.
fn parse_committee(s: &str, policy: TransferPolicy) -> Result<Committee, String> {
    let mut members = Vec::new();
    for part in s.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (id, public_hex) = part
            .split_once('=')
            .ok_or_else(|| format!("bad committee entry (want id=public-key-hex): {part}"))?;
        let id = id.trim().to_string();
        let key = parse_verifying_key(public_hex.trim(), "committee public key")?;
        members.push((id, key));
    }
    Committee::new(members, policy).ok_or_else(|| "empty or duplicate committee".into())
}

/// `alice=<pubhex>,bob=<pubhex>` → immutable verifier-owned alias bindings.
fn parse_owner_roster(s: &str) -> Result<OwnerRegistry, String> {
    let mut owners = Vec::new();
    for part in s.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (account, public_hex) = part
            .split_once('=')
            .ok_or_else(|| format!("bad owner entry (want account=public-key-hex): {part}"))?;
        let account = account.trim().to_string();
        let key = parse_verifying_key(public_hex.trim(), "owner public key")?;
        owners.push((account, key));
    }
    OwnerRegistry::new(owners).map_err(|e| format!("invalid owner roster: {e:?}"))
}

fn parse_network_id(s: &str) -> Result<NetworkId, String> {
    NetworkId::new(s).map_err(|e| format!("invalid --network-id: {e:?}"))
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
    Ledger::try_genesis(alloc).map_err(|e| format!("invalid genesis: {e:?}"))
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
    let signing_key = load_signing_key(&flags)?;
    let network_id = parse_network_id(flags.require("network-id")?)?;
    let owners = parse_owner_roster(flags.require("owner-roster")?)?;
    let policy = TransferPolicy::new(network_id, owners);
    let listen = parse_listen(flags.require("listen")?)?;
    let peers = parse_peers(flags.require("peers")?)?;
    let committee = parse_committee(flags.require("committee")?, policy.clone())?;
    let genesis_s = flags.get("genesis").unwrap_or("alice=100,bob=0");
    let ledger = parse_genesis(genesis_s)?;
    let idle_exit: Option<usize> = match flags.get("rounds-idle-exit") {
        Some(s) => Some(
            s.parse()
                .map_err(|_| format!("bad --rounds-idle-exit: {s}"))?,
        ),
        None => None,
    };

    // Durability is opt-in at the process boundary. With `--journal` the
    // authority always goes through `recover`: a fresh file replays an empty log,
    // which is exactly a first boot, so one code path covers both cases.
    //
    // Without it the node keeps the historical in-memory behaviour, in which a
    // restart forgets every lock and an honest crash spends one unit of the
    // Byzantine budget (audit-333-fsm-vs-borg-k8s-2026-07-15). That is a
    // debug/ephemeral configuration, and the node says so on stderr rather than
    // letting an operator assume otherwise.
    let mut auth = match flags.get("journal") {
        Some(path) => {
            let journal = FileJournal::open(path)
                .map_err(|e| format!("--journal {path}: {e}"))?;
            Authority::recover(id.clone(), signing_key, policy, committee.id(), ledger, journal)
                .map_err(|e| format!("journal recovery failed: {e}"))?
        }
        None => {
            eprintln!(
                "warning: no --journal; locks are in-memory only. A restart will \
                 forget them and this authority may sign a conflicting order for a \
                 slot it already voted on. Do not use for a durable deployment."
            );
            Authority::new(id.clone(), signing_key, policy, committee.id(), ledger)
        }
    };
    // Public key must match committee entry for this id.
    if let Some(expected) = committee.key_of(&id) {
        if auth.verifying_key() != *expected {
            return Err(format!(
                "authority --id {id} private key does not match --committee public key"
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

    // Best-effort initial mesh (anti-entropy, audit 2026-07-15 P1): some
    // peers may still be booting. A hard failure here used to kill the
    // process; now a missing peer is (re-)dialed level-triggered in the main
    // loop below, so a partially-up committee no longer bricks startup.
    for &a in &peers {
        if a != net.addr() && net.addr() < a {
            let _ = net.connect_peer(a);
        }
    }
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
    // Anti-entropy, receiver-independent half (audit 2026-07-15 P1): a
    // certificate is public, self-authenticating evidence — re-presenting it
    // is always safe (`confirm` is idempotent: AlreadyApplied). The daemon
    // retains every certificate it applied and re-broadcasts the set during
    // quiet periods, so an authority that missed one (late boot, lost frame)
    // converges level-triggered instead of staying locked on an old seq
    // forever. No request/response protocol: just periodic re-presentation.
    let mut applied_certs: HashMap<(String, u64), Certificate> = HashMap::new();
    // ~100 idle rounds ≈ 0.7s at this loop's sleep pace; quiet-period-only
    // rebroadcast self-throttles under load.
    const REBROADCAST_IDLE_ROUNDS: usize = 100;
    let mut since_rebroadcast = 0usize;
    // Level-triggered (re-)dial of missing mesh peers, ~50 rounds ≈ 0.35s.
    const REDIAL_INTERVAL_ROUNDS: usize = 50;
    let mut since_redial = 0usize;
    let mut idle_rounds = 0usize;
    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }

        since_redial += 1;
        if since_redial >= REDIAL_INTERVAL_ROUNDS {
            since_redial = 0;
            for &a in &peers {
                // Half-mesh invariant: we only ever dial strictly greater
                // addresses; lesser ones heal by dialing us.
                if a == net.addr() || net.addr() > a {
                    continue;
                }
                let known = net.peer_states().iter().any(|(ra, _)| *ra == Some(a));
                if !known {
                    let _ = net.connect_peer(a);
                }
            }
        }

        let msgs = endpoint.poll();
        if msgs.is_empty() {
            idle_rounds += 1;
            since_rebroadcast += 1;
            if since_rebroadcast >= REBROADCAST_IDLE_ROUNDS && !applied_certs.is_empty() {
                since_rebroadcast = 0;
                let mut order_ids = String::new();
                for cert in applied_certs.values() {
                    let _ = endpoint.broadcast_cert(cert.clone());
                    if !order_ids.is_empty() {
                        order_ids.push(',');
                    }
                    order_ids.push_str(&order_id_hex(&cert.order));
                }
                emit(&format!(
                    "{{\"event\":\"cert_rebroadcast\",\"authority\":\"{}\",\"certs\":{},\"order_ids\":\"{}\"}}",
                    escape_json(&id),
                    applied_certs.len(),
                    escape_json(&order_ids)
                ));
            }
            if let Some(n) = idle_exit {
                if idle_rounds >= n {
                    break;
                }
            }
            thread::sleep(Duration::from_millis(5));
            continue;
        }
        idle_rounds = 0;
        since_rebroadcast = 0;

        for msg in msgs {
            match msg {
                AuthorityMsg::Order(t) => match auth.handle(&t) {
                    Ok(vote) => {
                        let _ = endpoint.broadcast_vote(vote);
                        emit(&format!(
                            "{{\"event\":\"vote_cast\",\"order_id\":\"{}\",\"transfer\":\"{}\",\"authority\":\"{}\"}}",
                            order_id_hex(&t),
                            escape_json(&transfer_str(&t.transfer)),
                            escape_json(&id)
                        ));
                    }
                    Err(AuthorityError::Equivocation { account, seq }) => {
                        emit(&format!(
                            "{{\"event\":\"equivocation_rejected\",\"order_id\":\"{}\",\"transfer\":\"{}\",\"authority\":\"{}\",\"account\":\"{}\",\"seq\":{}}}",
                            order_id_hex(&t),
                            escape_json(&transfer_str(&t.transfer)),
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
                            "{{\"event\":\"out_of_order\",\"order_id\":\"{}\",\"transfer\":\"{}\",\"authority\":\"{}\",\"account\":\"{}\",\"expected\":{},\"got\":{},\"balances\":{},\"total_supply\":{}}}",
                            order_id_hex(&t),
                            escape_json(&transfer_str(&t.transfer)),
                            escape_json(&id),
                            escape_json(&account),
                            expected,
                            got,
                            balances_json(auth.ledger()),
                            auth.ledger().total_supply()
                        ));
                    }
                    Err(AuthorityError::OwnerAuth(error)) => {
                        let reason = match error {
                            OwnerAuthError::WrongNetwork { .. } => "wrong_network",
                            OwnerAuthError::WrongPolicy { .. } => "wrong_policy",
                            OwnerAuthError::UnknownSender { .. } => "unknown_sender",
                            OwnerAuthError::UnknownRecipient { .. } => "unknown_recipient",
                            OwnerAuthError::InvalidOwnerSignature { .. } => {
                                "invalid_owner_signature"
                            }
                        };
                        emit(&format!(
                            "{{\"event\":\"owner_auth_rejected\",\"order_id\":\"{}\",\"transfer\":\"{}\",\"authority\":\"{}\",\"reason\":\"{}\"}}",
                            order_id_hex(&t),
                            escape_json(&transfer_str(&t.transfer)),
                            escape_json(&id),
                            reason
                        ));
                    }
                    Err(AuthorityError::UnknownSender { account }) => {
                        emit(&format!(
                            "{{\"event\":\"state_rejected\",\"order_id\":\"{}\",\"transfer\":\"{}\",\"authority\":\"{}\",\"reason\":\"unknown_sender\",\"account\":\"{}\"}}",
                            order_id_hex(&t),
                            escape_json(&transfer_str(&t.transfer)),
                            escape_json(&id),
                            escape_json(&account)
                        ));
                    }
                    // Durability fault: this authority can no longer prove it has
                    // not already signed a conflicting order for some slot, so it
                    // must stop rather than keep voting. Fail-stop is the whole
                    // point of the journal (see journal.rs).
                    Err(AuthorityError::JournalFailed { reason }) => {
                        emit(&format!(
                            "{{\"event\":\"durability_failed\",\"order_id\":\"{}\",\"transfer\":\"{}\",\"authority\":\"{}\",\"reason\":\"{}\"}}",
                            order_id_hex(&t),
                            escape_json(&transfer_str(&t.transfer)),
                            escape_json(&id),
                            escape_json(&reason)
                        ));
                        return Err(format!("durability failure, fail-stop: {reason}"));
                    }
                    Err(AuthorityError::Poisoned) => {
                        emit(&format!(
                            "{{\"event\":\"poisoned\",\"order_id\":\"{}\",\"authority\":\"{}\"}}",
                            order_id_hex(&t),
                            escape_json(&id)
                        ));
                        return Err("authority poisoned by an earlier durability failure".to_string());
                    }
                    Err(AuthorityError::ZeroAmount) => {
                        emit(&format!(
                            "{{\"event\":\"state_rejected\",\"order_id\":\"{}\",\"transfer\":\"{}\",\"authority\":\"{}\",\"reason\":\"zero_amount\"}}",
                            order_id_hex(&t),
                            escape_json(&transfer_str(&t.transfer)),
                            escape_json(&id)
                        ));
                    }
                    Err(AuthorityError::InsufficientBalance { account, have, need }) => {
                        emit(&format!(
                            "{{\"event\":\"state_rejected\",\"order_id\":\"{}\",\"transfer\":\"{}\",\"authority\":\"{}\",\"reason\":\"insufficient_balance\",\"account\":\"{}\",\"have\":{},\"need\":{}}}",
                            order_id_hex(&t),
                            escape_json(&transfer_str(&t.transfer)),
                            escape_json(&id),
                            escape_json(&account),
                            have,
                            need
                        ));
                    }
                    Err(AuthorityError::SequenceExhausted { account }) => {
                        emit(&format!(
                            "{{\"event\":\"state_rejected\",\"order_id\":\"{}\",\"transfer\":\"{}\",\"authority\":\"{}\",\"reason\":\"sequence_exhausted\",\"account\":\"{}\"}}",
                            order_id_hex(&t),
                            escape_json(&transfer_str(&t.transfer)),
                            escape_json(&id),
                            escape_json(&account)
                        ));
                    }
                    Err(AuthorityError::BalanceOverflow {
                        account,
                        have,
                        credit,
                    }) => {
                        emit(&format!(
                            "{{\"event\":\"state_rejected\",\"order_id\":\"{}\",\"transfer\":\"{}\",\"authority\":\"{}\",\"reason\":\"balance_overflow\",\"account\":\"{}\",\"have\":{},\"credit\":{}}}",
                            order_id_hex(&t),
                            escape_json(&transfer_str(&t.transfer)),
                            escape_json(&id),
                            escape_json(&account),
                            have,
                            credit
                        ));
                    }
                },
                AuthorityMsg::Cert(c) => {
                    if let Some(v) = c.verify(&committee) {
                        match auth.confirm(&v, &committee) {
                            Ok(ConfirmOutcome::Applied) => {
                                emit(&format!(
                                    "{{\"event\":\"cert_applied\",\"order_id\":\"{}\",\"transfer\":\"{}\",\"authority\":\"{}\",\"balances\":{},\"total_supply\":{}}}",
                                    order_id_hex(v.order()),
                                    escape_json(&transfer_str(v.transfer())),
                                    escape_json(&id),
                                    balances_json(auth.ledger()),
                                    auth.ledger().total_supply()
                                ));
                                // Attest-iff-applied: only here, after the
                                // committed apply, does the attestation exist.
                                let account = v.transfer().from.clone();
                                let seq = v.transfer().from_seq;
                                // Retain for quiet-period anti-entropy
                                // re-presentation (see applied_certs above).
                                applied_certs.insert((account.clone(), seq), c.clone());
                                if let Some(attestation) = auth.attestation_for(&account, seq) {
                                    let _ = endpoint.broadcast_attestation(attestation);
                                }
                            }
                            Ok(ConfirmOutcome::AlreadyApplied) => {
                                // A re-presented cert may be our only copy
                                // after a journal-recovered restart — retain
                                // it too so we can re-present it onward.
                                let account = v.transfer().from.clone();
                                let seq = v.transfer().from_seq;
                                applied_certs.insert((account, seq), c.clone());
                            }
                            Err(error) => {
                                let reason = match error {
                                    ConfirmError::WrongCommittee { .. } => "wrong_committee",
                                    ConfirmError::WrongPolicy { .. } => "wrong_policy",
                                    ConfirmError::OwnerAuth(_) => "owner_auth",
                                    ConfirmError::State(_) => "state",
                                    ConfirmError::Journal(_) => "durability",
                                    ConfirmError::Poisoned => "poisoned",
                                };
                                emit(&format!(
                                    "{{\"event\":\"cert_state_rejected\",\"order_id\":\"{}\",\"transfer\":\"{}\",\"authority\":\"{}\",\"reason\":\"{}\"}}",
                                    order_id_hex(v.order()),
                                    escape_json(&transfer_str(v.transfer())),
                                    escape_json(&id),
                                    reason
                                ));
                            }
                        }
                    }
                }
                AuthorityMsg::Vote(_) => {
                    // Authorities ignore peer votes; the submit client assembles.
                }
                AuthorityMsg::Attestation(_) => {
                    // Authorities ignore peer attestations; clients collect
                    // them for EffectCert finality.
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
    let signing_key = load_signing_key(&flags)?;
    let network_id = parse_network_id(flags.require("network-id")?)?;
    let owners = parse_owner_roster(flags.require("owner-roster")?)?;
    let policy = TransferPolicy::new(network_id.clone(), owners);
    let listen = parse_listen(flags.require("listen")?)?;
    let peers = parse_peers(flags.require("peers")?)?;
    let committee = parse_committee(flags.require("committee")?, policy.clone())?;
    let transfer = parse_transfer(flags.require("transfer")?)?;
    let order = SignedTransfer::sign(&policy, transfer, &signing_key);
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

    let client_id = format!("client-{}", std::process::id());
    let net = TcpAuthorityNet::bind_at(client_id, listen)
        .map_err(|e| format!("bind {listen}: {e}"))?;
    emit(&format!(
        "{{\"event\":\"listening\",\"id\":\"{}\",\"addr\":\"{}\"}}",
        escape_json(net.id()),
        escape_json(&net.addr().to_string())
    ));

    // Client dials every authority (one-way); authorities do not list the
    // client. Best-effort (anti-entropy, audit 2026-07-15 P1): a late-booting
    // authority used to kill the submit at connect time — now missing peers
    // are (re-)dialed in the collection loop below.
    for &a in &peers {
        let _ = net.connect_peer(a);
    }
    thread::sleep(Duration::from_millis(30));
    emit(&format!(
        "{{\"event\":\"committee_ready\",\"n\":{}}}",
        committee.size()
    ));

    let endpoint = net.endpoint();

    // Level-triggered order broadcast: a lost frame or a late authority no
    // longer means certain cert_failed. The order is re-broadcast every
    // collection window while the vote budget lasts. Authorities re-vote
    // idempotently on duplicates, and the collector accumulates across
    // windows, so retrying changes liveness, never safety. Total wait budget
    // (--max-rounds) is unchanged.
    const RETRY_WINDOW: usize = 25;
    let mut coll = VoteCollector::new(order.clone());
    let mut remaining = max_rounds.max(1);
    let (cert, status) = loop {
        for &a in &peers {
            let known = net.peer_states().iter().any(|(ra, _)| *ra == Some(a));
            if !known {
                let _ = net.connect_peer(a);
            }
        }
        endpoint
            .broadcast_order(order.clone())
            .map_err(|e| format!("broadcast_order: {e:?}"))?;
        let window = remaining.min(RETRY_WINDOW);
        let (c, s) = coll.collect_until_quorum_with_pause(
            &endpoint,
            &committee,
            window,
            Duration::from_millis(pause_ms),
        );
        let certified = matches!(s, Certified::Ok);
        remaining = remaining.saturating_sub(window);
        if certified || remaining == 0 {
            break (c, s);
        }
    };

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
                "{{\"event\":\"certified\",\"order_id\":\"{}\",\"transfer\":\"{}\",\"status\":\"Ok\"}}",
                order_id_hex(&order),
                escape_json(&transfer_str(&order.transfer))
            ));
            ExitCode::SUCCESS
        }
        failed => {
            let reason = format!("{failed:?}");
            emit(&format!(
                "{{\"event\":\"cert_failed\",\"order_id\":\"{}\",\"transfer\":\"{}\",\"reason\":\"{}\"}}",
                order_id_hex(&order),
                escape_json(&transfer_str(&order.transfer)),
                escape_json(&reason)
            ));
            ExitCode::from(2)
        }
    };

    net.shutdown();
    Ok(code)
}
