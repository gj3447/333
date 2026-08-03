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
    ConfirmError, ConfirmOutcome, EpochCert, EpochError, EpochProposal, EpochVote, FenceOutcome,
    FileJournal, InstallOutcome, Ledger, NetworkId, OwnerAuthError, OwnerRegistry, SignedTransfer,
    SigningKey,
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
        Some("reconfig") => match run_reconfig(&args[2..]) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("reconfig error: {e}");
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
  node reconfig --network-id <deployment> --owner-roster alice=<pubhex>,... --listen <127.0.0.1:PORT> --peers <authority_addrs,...> --committee <current roster> --next-committee <new roster> --epoch <n> [--max-rounds 300] [--pause-ms 10]
  node authority ... [--observe]  (observer boot for a not-yet-member joining via epoch change)

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
    /// Boolean switches that take no value (e.g. `--observe`). Everything
    /// else still requires one — a missing value must stay a loud error.
    const BOOLEAN: &'static [&'static str] = &["observe"];

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
            if Self::BOOLEAN.contains(&key.as_str())
                && (i >= args.len() || args[i].starts_with("--"))
            {
                map.insert(key, "true".to_string());
                continue;
            }
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
fn parse_roster(s: &str) -> Result<Vec<(String, VerifyingKey)>, String> {
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
    Ok(members)
}

/// Public-key-only authority roster. Private seeds never appear in this value.
fn parse_committee(s: &str, policy: TransferPolicy) -> Result<Committee, String> {
    let members = parse_roster(s)?;
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
            Authority::recover(id.clone(), signing_key, policy.clone(), committee.id(), ledger, journal)
                .map_err(|e| format!("journal recovery failed: {e}"))?
        }
        None => {
            eprintln!(
                "warning: no --journal; locks are in-memory only. A restart will \
                 forget them and this authority may sign a conflicting order for a \
                 slot it already voted on. Do not use for a durable deployment."
            );
            Authority::new(id.clone(), signing_key, policy.clone(), committee.id(), ledger)
        }
    };
    // Public key must match committee entry for this id. With `--observe` a
    // non-member may boot as a passive observer (committee-reconfiguration
    // M3 join path: it converges via old-epoch certificates, installs the
    // epoch change, and starts voting only once it IS a member). Without the
    // flag, a missing membership is a config mistake and a hard error.
    let observe = flags.get("observe").is_some();
    if let Some(expected) = committee.key_of(&id) {
        if auth.verifying_key() != *expected {
            return Err(format!(
                "authority --id {id} private key does not match --committee public key"
            ));
        }
    } else if observe {
        eprintln!(
            "warning: authority id {id} not in --committee; booting as OBSERVER \
             (votes ignored until an epoch change makes it a member)"
        );
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
    // The committee the daemon validates against. Swapped to the next roster
    // the moment an installed epoch change completes (committee-reconfig M3).
    let mut committee = committee;
    // Anti-entropy, receiver-independent half (audit 2026-07-15 P1): a
    // certificate is public, self-authenticating evidence — re-presenting it
    // is always safe (`confirm` is idempotent: AlreadyApplied). The daemon
    // retains every certificate it applied and re-broadcasts the set during
    // quiet periods, so an authority that missed one (late boot, lost frame)
    // converges level-triggered instead of staying locked on an old seq
    // forever. No request/response protocol: just periodic re-presentation.
    let mut applied_certs: HashMap<(String, u64), Certificate> = HashMap::new();
    // Epoch certificates get the same re-presentation (M3/M5): a straggler
    // that missed the change converges to the new epoch the same way.
    let mut applied_epoch_certs: HashMap<u64, EpochCert> = HashMap::new();
    // A journal-recovered process re-joins the epoch-cert re-presentation set
    // from its durable install record (M5).
    if let Some(ec) = auth.last_epoch_cert() {
        applied_epoch_certs.insert(ec.epoch, ec.clone());
    }
    // ~100 idle rounds ≈ 0.7s at this loop's sleep pace; quiet-period-only
    // rebroadcast self-throttles under load.
    const REBROADCAST_IDLE_ROUNDS: usize = 100;
    let mut since_rebroadcast = 0usize;
    // Level-triggered (re-)dial of missing mesh peers, ~50 rounds ≈ 0.35s.
    const REDIAL_INTERVAL_ROUNDS: usize = 50;
    let mut since_redial = 0usize;
    // Epoch-change daemon state (M3): the quiet window for signing an epoch
    // vote is the same cadence as the re-presentation above — the frontier
    // must be stable (no new cert_applied) for the whole window.
    const QUIET_ROUNDS_FOR_EPOCH_VOTE: u64 = 100;
    let mut loop_tick: u64 = 0;
    let mut frontier_changed_at: u64 = 0;
    let mut epoch_vote_sent = false;
    // An epoch cert parked in Installing (frontier catch-up); the committee
    // swaps when coverage completes.
    let mut pending_epoch_cert: Option<EpochCert> = None;
    let mut idle_rounds = 0usize;
    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        loop_tick += 1;

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
                for ec in applied_epoch_certs.values() {
                    let _ = endpoint.broadcast_epoch_cert(ec.clone());
                }
                emit(&format!(
                    "{{\"event\":\"cert_rebroadcast\",\"authority\":\"{}\",\"certs\":{},\"epoch_certs\":{},\"order_ids\":\"{}\"}}",
                    escape_json(&id),
                    applied_certs.len(),
                    applied_epoch_certs.len(),
                    escape_json(&order_ids)
                ));
            }
            // Quiet window (design §5): sign the epoch vote only after the
            // frontier has been stable for the whole window — the signature
            // binds exactly that stable state.
            if !epoch_vote_sent
                && loop_tick.saturating_sub(frontier_changed_at) >= QUIET_ROUNDS_FOR_EPOCH_VOTE
            {
                if let Ok(vote) = auth.sign_epoch_vote() {
                    emit(&format!(
                        "{{\"event\":\"epoch_vote_cast\",\"authority\":\"{}\",\"epoch\":{}}}",
                        escape_json(&id),
                        vote.epoch
                    ));
                    let _ = endpoint.broadcast_epoch_vote(vote);
                    epoch_vote_sent = true;
                }
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
                        // Observers withhold votes while they are not members
                        // of the CURRENT committee (collectors would reject
                        // them anyway); once an epoch change makes them a
                        // member, their votes flow normally.
                        if !(observe && !committee.contains(&id)) {
                            let _ = endpoint.broadcast_vote(vote);
                        }
                        emit(&format!(
                            "{{\"event\":\"vote_cast\",\"order_id\":\"{}\",\"transfer\":\"{}\",\"authority\":\"{}\",\"epoch\":{}}}",
                            order_id_hex(&t),
                            escape_json(&transfer_str(&t.transfer)),
                            escape_json(&id),
                            committee.epoch()
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
                    // Epoch gate (design §5): fencing/catching-up withholds
                    // votes but is not a failure — the order's owner retries
                    // after the change completes.
                    Err(AuthorityError::EpochFencing { epoch }) => {
                        emit(&format!(
                            "{{\"event\":\"epoch_fencing\",\"order_id\":\"{}\",\"transfer\":\"{}\",\"authority\":\"{}\",\"epoch\":{}}}",
                            order_id_hex(&t),
                            escape_json(&transfer_str(&t.transfer)),
                            escape_json(&id),
                            epoch
                        ));
                    }
                    Err(AuthorityError::EpochCatchingUp { epoch }) => {
                        emit(&format!(
                            "{{\"event\":\"epoch_catching_up\",\"order_id\":\"{}\",\"transfer\":\"{}\",\"authority\":\"{}\",\"epoch\":{}}}",
                            order_id_hex(&t),
                            escape_json(&transfer_str(&t.transfer)),
                            escape_json(&id),
                            epoch
                        ));
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
                        // Observers follow the log without the self-membership
                        // binding (join path); members take the strict check.
                        let outcome = if observe {
                            auth.confirm_as_observer(&v, &committee)
                        } else {
                            auth.confirm(&v, &committee)
                        };
                        match outcome {
                            Ok(ConfirmOutcome::Applied) => {
                                emit(&format!(
                                    "{{\"event\":\"cert_applied\",\"order_id\":\"{}\",\"transfer\":\"{}\",\"authority\":\"{}\",\"epoch\":{},\"balances\":{},\"total_supply\":{}}}",
                                    order_id_hex(v.order()),
                                    escape_json(&transfer_str(v.transfer())),
                                    escape_json(&id),
                                    committee.epoch(),
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
                                frontier_changed_at = loop_tick;
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
                AuthorityMsg::EpochProposal(p) => {
                    match auth.fence(p.clone()) {
                        Ok(FenceOutcome::Fenced { next_committee_id }) => {
                            epoch_vote_sent = false;
                            frontier_changed_at = loop_tick; // fence restarts the quiet window
                            emit(&format!(
                                "{{\"event\":\"epoch_fenced\",\"authority\":\"{}\",\"epoch\":{},\"next_committee\":\"{}\"}}",
                                escape_json(&id),
                                p.epoch,
                                escape_json(&next_committee_id.to_string())
                            ));
                        }
                        Ok(other) => {
                            emit(&format!(
                                "{{\"event\":\"epoch_fence_ignored\",\"authority\":\"{}\",\"epoch\":{},\"outcome\":\"{}\"}}",
                                escape_json(&id),
                                p.epoch,
                                escape_json(&format!("{other:?}"))
                            ));
                        }
                        Err(e) => {
                            emit(&format!(
                                "{{\"event\":\"epoch_fence_rejected\",\"authority\":\"{}\",\"epoch\":{},\"error\":\"{}\"}}",
                                escape_json(&id),
                                p.epoch,
                                escape_json(&format!("{e:?}"))
                            ));
                        }
                    }
                }
                AuthorityMsg::EpochVote(_) => {
                    // Authorities ignore peer epoch votes; the reconfig client
                    // collects them (mirrors user Vote handling).
                }
                AuthorityMsg::EpochCert(c) => {
                    match auth.install_epoch_cert(&c, &committee) {
                        Ok(InstallOutcome::Installed) => {
                            emit(&format!(
                                "{{\"event\":\"epoch_installed\",\"authority\":\"{}\",\"epoch\":{}}}",
                                escape_json(&id),
                                c.epoch
                            ));
                            committee = Committee::with_epoch(
                                c.next_roster.clone(),
                                policy.clone(),
                                c.epoch,
                            )
                            .expect("roster validated at install");
                            epoch_vote_sent = true;
                            applied_epoch_certs.insert(c.epoch, c.clone());
                        }
                        Ok(InstallOutcome::Installing) => {
                            emit(&format!(
                                "{{\"event\":\"epoch_installing\",\"authority\":\"{}\",\"epoch\":{}}}",
                                escape_json(&id),
                                c.epoch
                            ));
                            pending_epoch_cert = Some(c.clone());
                            applied_epoch_certs.insert(c.epoch, c.clone());
                        }
                        Ok(other) => {
                            emit(&format!(
                                "{{\"event\":\"epoch_install_ignored\",\"authority\":\"{}\",\"epoch\":{},\"outcome\":\"{}\"}}",
                                escape_json(&id),
                                c.epoch,
                                escape_json(&format!("{other:?}"))
                            ));
                        }
                        Err(EpochError::ConflictingCert) => {
                            emit(&format!(
                                "{{\"event\":\"epoch_conflicting\",\"authority\":\"{}\",\"epoch\":{}}}",
                                escape_json(&id),
                                c.epoch
                            ));
                            return Err(
                                "conflicting valid epoch certs: old committee's Byzantine budget exceeded"
                                    .to_string(),
                            );
                        }
                        Err(e) => {
                            emit(&format!(
                                "{{\"event\":\"epoch_install_rejected\",\"authority\":\"{}\",\"epoch\":{},\"error\":\"{}\"}}",
                                escape_json(&id),
                                c.epoch,
                                escape_json(&format!("{e:?}"))
                            ));
                        }
                    }
                }
            }
        }
        // Coverage completion (design §5): an epoch cert parked in Installing
        // activates the moment the local frontier covers the committed one —
        // the committee swap happens exactly then, never earlier.
        if let Some(pc) = &pending_epoch_cert {
            if auth.epoch() == pc.epoch {
                emit(&format!(
                    "{{\"event\":\"epoch_installed\",\"authority\":\"{}\",\"epoch\":{},\"via\":\"coverage\"}}",
                    escape_json(&id),
                    pc.epoch
                ));
                committee = Committee::with_epoch(
                    pc.next_roster.clone(),
                    policy.clone(),
                    pc.epoch,
                )
                .expect("roster validated at install");
                epoch_vote_sent = true;
                pending_epoch_cert = None;
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

// --- reconfig subcommand (committee-reconfiguration M3) ----------------------

/// Operator path for an epoch change: broadcast the proposal, collect the
/// fenced authorities' epoch votes until one frontier digest has quorum, then
/// assemble and broadcast the epoch certificate. Level-triggered like submit:
/// the proposal is re-broadcast every window, so a late or missed authority
/// delays the change instead of killing it.
fn run_reconfig(args: &[String]) -> Result<ExitCode, String> {
    let flags = Flags::parse(args).map_err(|e| {
        usage();
        e
    })?;
    let network_id = parse_network_id(flags.require("network-id")?)?;
    let owners = parse_owner_roster(flags.require("owner-roster")?)?;
    let policy = TransferPolicy::new(network_id.clone(), owners);
    let listen = parse_listen(flags.require("listen")?)?;
    let peers = parse_peers(flags.require("peers")?)?;
    let committee = parse_committee(flags.require("committee")?, policy.clone())?;
    let next_roster = parse_roster(flags.require("next-committee")?)?;
    let epoch: u64 = flags
        .require("epoch")?
        .parse()
        .map_err(|_| "bad --epoch".to_string())?;
    if epoch != committee.epoch() + 1 {
        return Err(format!(
            "--epoch {epoch} is not current+1 ({})",
            committee.epoch() + 1
        ));
    }
    let next_committee = Committee::with_epoch(next_roster.clone(), policy.clone(), epoch)
        .ok_or("invalid --next-committee roster")?;
    let max_rounds: usize = flags
        .get("max-rounds")
        .unwrap_or("300")
        .parse()
        .map_err(|e| format!("bad --max-rounds: {e}"))?;
    let pause_ms: u64 = flags
        .get("pause-ms")
        .unwrap_or("10")
        .parse()
        .map_err(|e| format!("bad --pause-ms: {e}"))?;

    let proposal = EpochProposal {
        network_id,
        policy_id: policy.id(),
        epoch,
        next_roster,
    };
    let client_id = format!("reconfig-{}", std::process::id());
    let net = TcpAuthorityNet::bind_at(client_id, listen)
        .map_err(|e| format!("bind {listen}: {e}"))?;
    // Best-effort connect; missing peers are retried in the collection loop.
    for &a in &peers {
        let _ = net.connect_peer(a);
    }
    thread::sleep(Duration::from_millis(30));
    emit(&format!(
        "{{\"event\":\"epoch_proposed\",\"epoch\":{},\"next_committee\":\"{}\"}}",
        epoch,
        escape_json(&next_committee.id().to_string())
    ));

    const RETRY_WINDOW: usize = 25;
    let endpoint = net.endpoint();
    let mut votes: Vec<EpochVote> = Vec::new();
    let mut remaining = max_rounds.max(1);
    let assembled: Option<EpochCert> = loop {
        for &a in &peers {
            let known = net.peer_states().iter().any(|(ra, _)| *ra == Some(a));
            if !known {
                let _ = net.connect_peer(a);
            }
        }
        endpoint
            .broadcast_epoch_proposal(proposal.clone())
            .map_err(|e| format!("broadcast_epoch_proposal: {e:?}"))?;
        let window = remaining.min(RETRY_WINDOW);
        let mut done = None;
        for _ in 0..window {
            for msg in endpoint.poll() {
                let AuthorityMsg::EpochVote(v) = msg else {
                    continue;
                };
                // Foreign change, foreign trust root, or foreign roster:
                // never mixed into this certificate.
                if v.epoch != epoch
                    || v.committee_id != committee.id()
                    || v.next_committee_id != next_committee.id()
                {
                    continue;
                }
                let Some(key) = committee.key_of(&v.authority) else {
                    continue;
                };
                if v.verify_signature(key).is_err() {
                    continue;
                }
                if !votes.iter().any(|seen| seen.authority == v.authority) {
                    votes.push(v);
                }
            }
            // One frontier digest must hold quorum — a split frontier means
            // the fence quorum has not converged yet, so keep waiting.
            let mut by_digest: HashMap<[u8; 32], Vec<EpochVote>> = HashMap::new();
            for v in &votes {
                by_digest.entry(v.frontier_digest).or_default().push(v.clone());
            }
            if let Some((_, group)) = by_digest
                .into_iter()
                .find(|(_, group)| group.len() >= committee.quorum())
            {
                done = Some(EpochCert {
                    epoch,
                    next_roster: proposal.next_roster.clone(),
                    frontier: group[0].frontier.clone(),
                    votes: group[..committee.quorum()].to_vec(),
                });
                break;
            }
            if pause_ms > 0 {
                thread::sleep(Duration::from_millis(pause_ms));
            }
        }
        if done.is_some() {
            break done;
        }
        remaining = remaining.saturating_sub(window);
        if remaining == 0 {
            break None;
        }
    };

    let code = match assembled {
        Some(cert) => {
            endpoint
                .broadcast_epoch_cert(cert)
                .map_err(|e| format!("broadcast_epoch_cert: {e:?}"))?;
            // Give authorities a moment to receive/install before teardown.
            thread::sleep(Duration::from_millis(100));
            emit(&format!(
                "{{\"event\":\"epoch_cert_broadcast\",\"epoch\":{},\"votes\":{}}}",
                epoch,
                committee.quorum()
            ));
            ExitCode::SUCCESS
        }
        None => {
            emit(&format!(
                "{{\"event\":\"epoch_reconfig_failed\",\"epoch\":{},\"reason\":\"no quorum-agreed frontier within budget\"}}",
                epoch
            ));
            ExitCode::from(2)
        }
    };

    net.shutdown();
    Ok(code)
}
