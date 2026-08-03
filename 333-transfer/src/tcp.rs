// KG: transport-plan Step 8 (2026-07-14)
//
// Real TCP I/O behind `AuthorityNet`, mirroring `333-wire`'s pattern:
// `std::net::{TcpListener, TcpStream}`, hand-rolled length-prefixed frames
// (u32 big-endian length + body), listener thread + per-connection reader
// threads, no tokio / no serde.
//
// Frame body = `wire::encode_authority_msg` (1-byte tag + domain-tagged body).
//
// Restart healing: every session carries an `alive` flag that its reader thread
// clears on EOF / fatal read error. The send path heals flagged sessions —
// dialed ones are redialled (once per send) at their retained listen address,
// accepted ones are pruned (their remote listen address is unknown; the dialer
// heals that pair by dialing us again). Write errors alone cannot be the
// trigger: a write into a freshly dead socket returns Ok until the RST lands.
//
// Peer state machine (P1, FSM audit 2026-07-15 §2-D): detection → demotion →
// recovery is an explicit per-peer FSM (`PeerState`) with etcd-raft discipline —
// a peer changes state only through named `become_*` transitions, direct enum
// assignment is forbidden, and an illegal transition panics.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::authority::{Certificate, Vote};
use crate::effect::EffectAttestation;
use crate::net::{AuthorityMsg, AuthorityNet, NetError};
use crate::wire::{decode_authority_msg, encode_authority_msg};
use crate::SignedTransfer;

const READ_TIMEOUT: Duration = Duration::from_millis(200);
const CONNECT_ATTEMPTS: u32 = 40;
const CONNECT_BACKOFF: Duration = Duration::from_millis(10);

/// Per-peer connection state (FSM audit 2026-07-15 §2-D / P1).
///
/// etcd-raft bar (`raft.go:891-971`): a peer changes state exclusively through
/// the named `become_*` transitions on [`PeerSession`]; assigning this enum
/// directly is forbidden and an illegal transition panics. Legality table:
///
/// - `Live -> Suspect` — death signal observed (reader EOF/fatal, write error)
/// - `Suspect -> Live` — recovery completed (redial succeeded)
/// - `Suspect -> Dead` — recovery attempted and failed
/// - `Dead -> Live` — tombstone redial finally succeeded
///
/// There is deliberately **no** `Live -> Dead`: a peer must be *observed*
/// failing before it may be declared dead. `Dead` is terminal for accepted
/// sessions (reaped at demotion); for dialed sessions it is a tombstone that
/// retains the peer's listen address and is retried on later sends.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeerState {
    /// Freshly dialed/accepted, no death signal observed.
    Live,
    /// Death signal observed; recovery (redial) has not completed.
    Suspect,
    /// Recovery attempted and failed. Dialed peers keep a tombstone (the
    /// redial address survives) and are retried on later sends.
    Dead,
}

impl PeerState {
    fn can_become(self, next: PeerState) -> bool {
        use PeerState::*;
        matches!(
            (self, next),
            (Live, Suspect) | (Suspect, Live) | (Suspect, Dead) | (Dead, Live)
        )
    }
}

/// One live (or corpse) TCP session on the write path.
///
/// `alive` is shared with the session's reader thread: the reader is the only
/// reliable observer of peer death (EOF / RST surface there first), so it flags
/// the corpse and the send path acts on the flag. `redial` retains the peer's
/// *listen* address for sessions we dialed — the only address that survives a
/// peer restart. Accepted sessions have no redialable address (the inbound
/// ephemeral port dies with the peer), so `redial` is `None` and a dead
/// accepted session is pruned instead of healed.
struct PeerSession {
    stream: TcpStream,
    redial: Option<SocketAddr>,
    alive: Arc<AtomicBool>,
    state: PeerState,
}

impl PeerSession {
    fn new(stream: TcpStream, redial: Option<SocketAddr>, alive: Arc<AtomicBool>) -> Self {
        Self {
            stream,
            redial,
            alive,
            state: PeerState::Live,
        }
    }

    /// The only state-change path. Illegal transitions are programming errors
    /// and panic, mirroring etcd-raft's `becomeXxx` discipline.
    fn transition(&mut self, next: PeerState) {
        assert!(
            self.state.can_become(next),
            "illegal peer transition {:?} -> {:?}",
            self.state,
            next
        );
        if next == PeerState::Suspect {
            // The cross-thread death signal and the FSM state are one fact;
            // keep them in lockstep so no later phase misreads a suspect
            // session as live.
            self.alive.store(false, Ordering::SeqCst);
        }
        self.state = next;
    }

    /// Death signal observed (reader EOF/fatal or write error).
    fn become_suspect(&mut self) {
        self.transition(PeerState::Suspect);
    }

    /// Recovery completed: this peer (identity = its listen address) is
    /// reachable again. Legal from `Suspect` and from a `Dead` tombstone.
    fn become_live(&mut self) {
        self.transition(PeerState::Live);
    }

    /// Recovery attempted and failed.
    fn become_dead(&mut self) {
        self.transition(PeerState::Dead);
    }

    /// Replace this session's transport with a freshly redialed one, keeping
    /// the peer identity (`redial` listen address), and record the recovery.
    fn replace_with(&mut self, fresh: PeerSession) {
        self.stream = fresh.stream;
        self.alive = fresh.alive;
        self.become_live();
    }
}

/// Shared state for one TCP authority peer (listener + outbound peers + inbox).
struct TcpInner {
    id: String,
    addr: SocketAddr,
    peers: Mutex<Vec<PeerSession>>,
    inbox: Mutex<VecDeque<AuthorityMsg>>,
    stop: AtomicBool,
    listener: Mutex<Option<JoinHandle<()>>>,
    readers: Mutex<Vec<JoinHandle<()>>>,
}

/// One TCP peer: binds `127.0.0.1:0`, accepts inbound framed messages, dials
/// outbound peers for broadcasts. Pair with [`TcpEndpoint`] for [`AuthorityNet`].
pub struct TcpAuthorityNet {
    inner: Arc<TcpInner>,
}

/// Per-peer handle implementing [`AuthorityNet`] (cheap clone via `Arc`).
#[derive(Clone)]
pub struct TcpEndpoint {
    inner: Arc<TcpInner>,
}

impl TcpAuthorityNet {
    /// Bind an ephemeral port on loopback and start the accept loop.
    pub fn bind(id: impl Into<String>) -> io::Result<Self> {
        Self::bind_at(id, "127.0.0.1:0")
    }

    /// Bind a specific listen address (e.g. `127.0.0.1:PORT` for multi-process nodes).
    pub fn bind_at(id: impl Into<String>, addr: impl std::net::ToSocketAddrs) -> io::Result<Self> {
        let id = id.into();
        let listener = TcpListener::bind(addr)?;
        let addr = listener.local_addr()?;
        // Short accept timeout so shutdown can join without hanging forever.
        let _ = listener.set_nonblocking(false);
        let inner = Arc::new(TcpInner {
            id,
            addr,
            peers: Mutex::new(Vec::new()),
            inbox: Mutex::new(VecDeque::new()),
            stop: AtomicBool::new(false),
            listener: Mutex::new(None),
            readers: Mutex::new(Vec::new()),
        });
        let handle = spawn_listener(Arc::clone(&inner), listener);
        *inner.listener.lock().expect("listener lock") = Some(handle);
        Ok(Self { inner })
    }

    pub fn id(&self) -> &str {
        &self.inner.id
    }

    pub fn addr(&self) -> SocketAddr {
        self.inner.addr
    }

    /// Observable peer FSM snapshot: `(peer listen address if dialed, state)`.
    /// Dead dialed peers appear as tombstones awaiting redial; accepted peers
    /// are reaped at demotion, so they never linger here as Dead.
    pub fn peer_states(&self) -> Vec<(Option<SocketAddr>, PeerState)> {
        self.inner
            .peers
            .lock()
            .expect("peers lock")
            .iter()
            .map(|p| (p.redial, p.state))
            .collect()
    }

    /// Cheap cloneable endpoint for the Steps 5–7 generic drivers.
    pub fn endpoint(&self) -> TcpEndpoint {
        TcpEndpoint {
            inner: Arc::clone(&self.inner),
        }
    }

    /// Dial one peer (bounded retries + small backoff for listener startup).
    pub fn connect_peer(&self, addr: SocketAddr) -> Result<(), NetError> {
        if addr == self.inner.addr {
            return Ok(());
        }
        let stream = connect_with_retry(addr)?;
        let session = dialed_session(&self.inner, addr, stream)?;
        self.inner
            .peers
            .lock()
            .expect("peers lock")
            .push(session);
        Ok(())
    }

    /// Half-mesh dial: only dial peers with strictly greater `SocketAddr` than
    /// self. Paired with accept-as-write + outbound readers, one TCP session per
    /// pair carries both directions without double delivery (full bidirectional
    /// dial + accept-as-write would fan out each broadcast twice).
    pub fn connect_all(&self, addrs: &[SocketAddr]) -> Result<(), NetError> {
        for &a in addrs {
            if a == self.inner.addr {
                continue;
            }
            if self.inner.addr < a {
                self.connect_peer(a)?;
            }
        }
        Ok(())
    }

    /// Dial every address except self (client → authorities). Use when the peer
    /// list is one-way (authorities do not dial the client back).
    pub fn connect_each(&self, addrs: &[SocketAddr]) -> Result<(), NetError> {
        for &a in addrs {
            if a == self.inner.addr {
                continue;
            }
            self.connect_peer(a)?;
        }
        Ok(())
    }

    /// Stop flag + join threads; ignore already-closed sockets.
    pub fn shutdown(&self) {
        self.inner.stop.store(true, Ordering::SeqCst);
        // Unblock accept by connecting to self (best-effort).
        let _ = TcpStream::connect(self.inner.addr);
        // Drop outbound peers so peer readers exit.
        self.inner.peers.lock().expect("peers lock").clear();
        if let Some(h) = self.inner.listener.lock().expect("listener lock").take() {
            let _ = h.join();
        }
        let readers = std::mem::take(&mut *self.inner.readers.lock().expect("readers lock"));
        for h in readers {
            let _ = h.join();
        }
    }
}

impl Drop for TcpAuthorityNet {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl TcpEndpoint {
    pub fn id(&self) -> &str {
        &self.inner.id
    }

    pub fn addr(&self) -> SocketAddr {
        self.inner.addr
    }
}

impl AuthorityNet for TcpEndpoint {
    fn broadcast_order(&self, order: SignedTransfer) -> Result<(), NetError> {
        self.write_all(&AuthorityMsg::Order(order))
    }

    fn broadcast_vote(&self, v: Vote) -> Result<(), NetError> {
        self.write_all(&AuthorityMsg::Vote(v))
    }

    fn broadcast_cert(&self, c: Certificate) -> Result<(), NetError> {
        self.write_all(&AuthorityMsg::Cert(c))
    }

    fn broadcast_attestation(&self, a: EffectAttestation) -> Result<(), NetError> {
        self.write_all(&AuthorityMsg::Attestation(a))
    }

    fn poll(&self) -> Vec<AuthorityMsg> {
        let mut g = self.inner.inbox.lock().expect("inbox lock");
        g.drain(..).collect()
    }
}

impl TcpEndpoint {
    fn write_all(&self, msg: &AuthorityMsg) -> Result<(), NetError> {
        if self.inner.stop.load(Ordering::SeqCst) {
            return Err(NetError::Closed);
        }
        let frame = encode_tcp_frame(msg);
        let mut peers = self.inner.peers.lock().expect("peers lock");
        if peers.is_empty() {
            return Ok(());
        }
        let mut ok = 0usize;
        let mut last_err: Option<String> = None;

        // Phase 1 — demote sessions whose reader flagged death (EOF / fatal
        // read), then attempt recovery. The flag, not a write error, is the
        // load-bearing trigger: a write into a corpse stream can return Ok
        // (the kernel buffers the bytes before the peer's RST arrives), so an
        // error-triggered-only reconnect would never fire and the corpse
        // would shadow the peer forever.
        let mut idx = 0;
        while idx < peers.len() {
            if !peers[idx].alive.load(Ordering::SeqCst) && peers[idx].state == PeerState::Live {
                peers[idx].become_suspect();
            }
            if peers[idx].state == PeerState::Live {
                idx += 1;
                continue;
            }
            match peers[idx].redial {
                // Dialed session: the retained listen address survives the peer
                // restart. One connect attempt per send — no retry/backoff loop
                // on the send path; the caller's next send is the retry.
                Some(addr) => match redial_once(&self.inner, addr) {
                    Ok(fresh) => {
                        peers[idx].replace_with(fresh);
                        idx += 1;
                    }
                    Err(e) => {
                        // Tombstone: keep the entry — it carries the redial
                        // address, and a later send retries the recovery.
                        if peers[idx].state == PeerState::Suspect {
                            peers[idx].become_dead();
                        }
                        last_err = Some(e);
                        idx += 1;
                    }
                },
                // Accepted session: no redialable address. Demote to Dead and
                // reap; a restarted dialer heals this pair by dialing us again
                // (the accept loop pushes the fresh Live session).
                None => {
                    peers[idx].become_dead();
                    peers.remove(idx);
                }
            }
        }
        if peers.is_empty() {
            // Every session was an unhealable corpse — same surface as "no
            // peers connected yet".
            return Ok(());
        }

        // Phase 2 — best-effort fan-out to live sessions. A surfaced write
        // error is the second death signal: demote Live -> Suspect and, for
        // dialed sessions, try one reconnect-and-resend within this send.
        for p in peers.iter_mut() {
            if !p.alive.load(Ordering::SeqCst) && p.state == PeerState::Live {
                p.become_suspect();
            }
            if p.state != PeerState::Live {
                continue; // recovery failed this round; next send retries
            }
            match p.stream.write_all(&frame).and_then(|_| p.stream.flush()) {
                Ok(()) => ok += 1,
                Err(e) => {
                    p.become_suspect();
                    last_err = Some(e.to_string());
                    if let Some(addr) = p.redial {
                        match redial_once(&self.inner, addr) {
                            Ok(fresh) => {
                                let written = (&fresh.stream)
                                    .write_all(&frame)
                                    .and_then(|_| (&fresh.stream).flush());
                                // The fresh transport replaces the suspect
                                // session even if its first write failed —
                                // phase 1 redials it again on the next send.
                                p.replace_with(fresh);
                                match written {
                                    Ok(()) => ok += 1,
                                    Err(e2) => {
                                        p.become_suspect();
                                        last_err = Some(e2.to_string());
                                    }
                                }
                            }
                            Err(e2) => {
                                p.become_dead();
                                last_err = Some(e2);
                            }
                        }
                    }
                }
            }
        }
        if ok == 0 {
            return Err(NetError::Io(
                last_err.unwrap_or_else(|| "all peer writes failed".into()),
            ));
        }
        // Best-effort full-mesh fan-out: succeed if at least one peer got the frame.
        Ok(())
    }
}

fn encode_tcp_frame(msg: &AuthorityMsg) -> Vec<u8> {
    let body = encode_authority_msg(msg);
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    out
}

fn read_tcp_frame<R: Read>(stream: &mut R) -> io::Result<AuthorityMsg> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let body_len = u32::from_be_bytes(len_buf) as usize;
    // Guard absurd lengths (DoS / corruption).
    if body_len > 16 * 1024 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    let mut body = vec![0u8; body_len];
    stream.read_exact(&mut body)?;
    decode_authority_msg(&body).map_err(|e| {
        io::Error::new(io::ErrorKind::InvalidData, format!("authority frame: {e:?}"))
    })
}

fn configure_stream(stream: &TcpStream) -> Result<(), NetError> {
    stream
        .set_read_timeout(Some(READ_TIMEOUT))
        .map_err(|e| NetError::Io(e.to_string()))?;
    stream
        .set_write_timeout(Some(READ_TIMEOUT))
        .map_err(|e| NetError::Io(e.to_string()))?;
    stream
        .set_nodelay(true)
        .map_err(|e| NetError::Io(e.to_string()))?;
    Ok(())
}

fn connect_with_retry(addr: SocketAddr) -> Result<TcpStream, NetError> {
    let mut last = None;
    for attempt in 0..CONNECT_ATTEMPTS {
        match TcpStream::connect(addr) {
            Ok(s) => return Ok(s),
            Err(e) => {
                last = Some(e);
                if attempt + 1 < CONNECT_ATTEMPTS {
                    thread::sleep(CONNECT_BACKOFF);
                }
            }
        }
    }
    Err(NetError::Io(format!(
        "connect {addr} failed after {CONNECT_ATTEMPTS} attempts: {:?}",
        last.map(|e| e.to_string())
    )))
}

/// Wrap an already-connected outbound stream into a [`PeerSession`]: configure
/// both halves, spawn the session reader sharing the `alive` flag, retain
/// `addr` (the peer's listen address) for restart redials.
fn dialed_session(
    inner: &Arc<TcpInner>,
    addr: SocketAddr,
    stream: TcpStream,
) -> Result<PeerSession, NetError> {
    configure_stream(&stream)?;
    let alive = Arc::new(AtomicBool::new(true));
    // Read replies on the same TCP session (peer may write back without dialing us).
    if let Ok(read_half) = stream.try_clone() {
        if configure_stream(&read_half).is_ok() {
            let handle = spawn_reader(Arc::clone(inner), read_half, Arc::clone(&alive));
            inner.readers.lock().expect("readers lock").push(handle);
        }
    }
    Ok(PeerSession::new(stream, Some(addr), alive))
}

/// One reconnect attempt to a dialed peer's listen address. Send-path helper:
/// exactly one `connect`, no retry/backoff loop — a down peer must not stall
/// every broadcast, and the caller's next send is the natural retry.
fn redial_once(inner: &Arc<TcpInner>, addr: SocketAddr) -> Result<PeerSession, String> {
    let stream = TcpStream::connect(addr).map_err(|e| format!("redial {addr}: {e}"))?;
    dialed_session(inner, addr, stream).map_err(|e| format!("redial {addr}: {e:?}"))
}

fn spawn_reader(inner: Arc<TcpInner>, stream: TcpStream, alive: Arc<AtomicBool>) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut stream = stream;
        loop {
            if inner.stop.load(Ordering::SeqCst) {
                break;
            }
            match read_tcp_frame(&mut stream) {
                Ok(msg) => {
                    inner
                        .inbox
                        .lock()
                        .expect("inbox lock")
                        .push_back(msg);
                }
                Err(e) => {
                    // Timeout: continue reading (allows stop check).
                    if e.kind() == io::ErrorKind::WouldBlock
                        || e.kind() == io::ErrorKind::TimedOut
                    {
                        continue;
                    }
                    // EOF or fatal error: the peer side of this session is
                    // gone. Flag the corpse for the send path — writes into it
                    // may keep returning Ok, so this flag is the only reliable
                    // death signal.
                    alive.store(false, Ordering::SeqCst);
                    break;
                }
            }
        }
    })
}

fn spawn_listener(inner: Arc<TcpInner>, listener: TcpListener) -> JoinHandle<()> {
    // Accept timeout: use set_nonblocking + sleep, or SO_RCVTIMEO via
    // read timeout on a dual-purpose approach. On Unix we can set_read_timeout
    // is not on TcpListener in std — unblock via self-connect on shutdown.
    thread::spawn(move || {
        for conn in listener.incoming() {
            if inner.stop.load(Ordering::SeqCst) {
                break;
            }
            let Ok(stream) = conn else { continue };
            if inner.stop.load(Ordering::SeqCst) {
                break;
            }
            let _ = configure_stream(&stream);
            // Accepted streams are also write peers so a dialing client (or peer)
            // receives our broadcasts without us knowing its listen addr in advance.
            // Full-mesh connect_all may double-deliver; collectors dedupe by authority.
            // `redial: None` — an accepted session cannot be healed from this
            // side (the remote listen addr is unknown), only pruned once dead.
            let alive = Arc::new(AtomicBool::new(true));
            if let Ok(write_half) = stream.try_clone() {
                if configure_stream(&write_half).is_ok() {
                    inner
                        .peers
                        .lock()
                        .expect("peers lock")
                        .push(PeerSession::new(write_half, None, Arc::clone(&alive)));
                }
            }
            let handle = spawn_reader(Arc::clone(&inner), stream, alive);
            inner
                .readers
                .lock()
                .expect("readers lock")
                .push(handle);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority::{Authority, Certificate, Committee};
    use crate::net::AuthorityNet;
    use crate::{Ledger, NetworkId, OwnerRegistry, Transfer, TransferPolicy};
    use ed25519_dalek::SigningKey;

    fn tx(from: &str, seq: u64, to: &str, amount: u128) -> Transfer {
        Transfer {
            from: from.into(),
            from_seq: seq,
            to: to.into(),
            amount,
        }
    }

    fn key(i: u8) -> SigningKey {
        SigningKey::from_bytes(&[i; 32])
    }

    fn policy() -> TransferPolicy {
        TransferPolicy::new(
            NetworkId::new("tcp-testnet").unwrap(),
            OwnerRegistry::new([
                ("alice", key(42).verifying_key()),
                ("bob", key(43).verifying_key()),
            ])
            .unwrap(),
        )
    }

    fn authority_genesis() -> Ledger {
        Ledger::genesis([
            ("alice".to_string(), 100),
            ("bob".to_string(), 0),
            ("carol".to_string(), 0),
            ("a".to_string(), 100),
            ("b".to_string(), 0),
        ])
    }

    fn signed_tx(policy: &TransferPolicy, seq: u64, amount: u128) -> SignedTransfer {
        SignedTransfer::sign(policy, tx("alice", seq, "bob", amount), &key(42))
    }

    #[test]
    fn tcp_order_round_trip_two_peers() {
        let policy = policy();
        let order = signed_tx(&policy, 0, 1);
        let a = TcpAuthorityNet::bind("a").unwrap();
        let b = TcpAuthorityNet::bind("b").unwrap();
        let addrs = [a.addr(), b.addr()];
        a.connect_all(&addrs).unwrap();
        b.connect_all(&addrs).unwrap();

        let ea = a.endpoint();
        let eb = b.endpoint();
        ea.broadcast_order(order.clone()).unwrap();

        // Poll with small retries for reader thread.
        let mut got = Vec::new();
        for _ in 0..50 {
            got.extend(eb.poll());
            if !got.is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        assert!(
            matches!(got.as_slice(), [AuthorityMsg::Order(received)] if received == &order),
            "got {got:?}"
        );
        a.shutdown();
        b.shutdown();
    }

    /// Lease a loopback port from the OS, then hand it back. The restart test
    /// needs one address to survive a bind/drop/bind cycle, which an ephemeral
    /// port cannot give: the whole defect is about dialling the *same* listen
    /// address twice.
    fn free_port() -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").expect("probe bind");
        l.local_addr().expect("probe addr").port()
    }

    fn poll_until(ep: &TcpEndpoint, tries: u32) -> Vec<AuthorityMsg> {
        let mut got = Vec::new();
        for _ in 0..tries {
            got.extend(ep.poll());
            if !got.is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        got
    }

    /// An authority restart is normal operation, not a fault. The committee is
    /// fixed and quorum names *specific* members, so a peer that stays
    /// unreachable for the rest of our process lifetime is a permanent quorum
    /// loss waiting to accumulate.
    #[test]
    fn a_restarted_peer_becomes_reachable_again() {
        let policy = policy();
        let port = free_port();
        let b_addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();

        let a = TcpAuthorityNet::bind("a").unwrap();
        let b1 = TcpAuthorityNet::bind_at("b", b_addr).unwrap();
        a.connect_peer(b_addr).unwrap();
        let ea = a.endpoint();

        ea.broadcast_order(signed_tx(&policy, 0, 1)).unwrap();
        assert_eq!(
            poll_until(&b1.endpoint(), 50).len(),
            1,
            "baseline: a live peer receives the broadcast"
        );

        b1.shutdown();
        drop(b1);
        // A write into a just-dead session can still return Ok — the kernel
        // buffers it before the RST lands. Ok is not evidence of delivery.
        let _ = ea.broadcast_order(signed_tx(&policy, 1, 1));

        let b2 = TcpAuthorityNet::bind_at("b", b_addr).unwrap();
        let eb2 = b2.endpoint();
        let mut got = Vec::new();
        for _ in 0..60 {
            let _ = ea.broadcast_order(signed_tx(&policy, 2, 1));
            got.extend(eb2.poll());
            if !got.is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !got.is_empty(),
            "restarted peer is permanently unreachable: the corpse stream is never replaced"
        );

        a.shutdown();
        b2.shutdown();
    }

    #[test]
    fn tcp_frame_preserves_valid_certificate_bytes() {
        // Encode path used on the wire must still yield is_valid after decode.
        let policy = policy();
        let order = signed_tx(&policy, 0, 10);
        let committee = Committee::new(
            (0..4u8).map(|i| (format!("a{i}"), key(i).verifying_key())),
            policy.clone(),
        )
        .unwrap();
        let auth: Vec<Authority> = (0..4u8)
            .map(|i| {
                Authority::new(
                    format!("a{i}"),
                    key(i),
                    policy.clone(),
                    committee.id(),
                    authority_genesis(),
                )
            })
            .collect();
        let mut auth = auth;
        let votes: Vec<_> = auth
            .iter_mut()
            .take(3)
            .map(|a| a.handle(&order).unwrap())
            .collect();
        let cert = Certificate::assemble(order, votes, &committee).unwrap();
        let frame = encode_tcp_frame(&AuthorityMsg::Cert(cert.clone()));
        // Strip length prefix and decode body as wire does on the reader.
        let body = &frame[4..];
        let back = decode_authority_msg(body).unwrap();
        match back {
            AuthorityMsg::Cert(c) => assert!(c.is_valid(&committee)),
            _ => panic!("cert"),
        }
    }

    fn stream_pair() -> (TcpStream, TcpStream) {
        let l = TcpListener::bind("127.0.0.1:0").expect("pair bind");
        let a = l.local_addr().expect("pair addr");
        let c = TcpStream::connect(a).expect("pair connect");
        let (s, _) = l.accept().expect("pair accept");
        (c, s)
    }

    /// The audit bar (§2-D/P1): detection -> demotion -> recovery must be an
    /// explicit, named-transition FSM, not an implicit boolean dance.
    #[test]
    fn peer_fsm_named_transitions_cover_detect_demote_recover() {
        let (c, _s) = stream_pair();
        let alive = Arc::new(AtomicBool::new(true));
        let mut p = PeerSession::new(c, Some("127.0.0.1:9".parse().unwrap()), alive);
        assert_eq!(p.state, PeerState::Live);
        p.become_suspect(); // detect
        assert_eq!(p.state, PeerState::Suspect);
        assert!(
            !p.alive.load(Ordering::SeqCst),
            "suspect keeps the cross-thread death signal in lockstep"
        );
        p.become_live(); // recover
        assert_eq!(p.state, PeerState::Live);
        p.become_suspect();
        p.become_dead(); // demote after a failed recovery
        assert_eq!(p.state, PeerState::Dead);
        p.become_live(); // tombstone redial finally succeeds
        assert_eq!(p.state, PeerState::Live);
    }

    #[test]
    #[should_panic(expected = "illegal peer transition")]
    fn peer_fsm_live_to_dead_panics() {
        let (c, _s) = stream_pair();
        let mut p = PeerSession::new(c, None, Arc::new(AtomicBool::new(true)));
        p.become_dead(); // a peer must be *observed* failing (Suspect) first
    }

    #[test]
    #[should_panic(expected = "illegal peer transition")]
    fn peer_fsm_dead_to_dead_panics() {
        let (c, _s) = stream_pair();
        let mut p = PeerSession::new(c, None, Arc::new(AtomicBool::new(true)));
        p.become_suspect();
        p.become_dead();
        p.become_dead(); // no silent self-transition
    }

    /// Same restart scenario as `a_restarted_peer_becomes_reachable_again`,
    /// but asserting the observable FSM path: Live -> Suspect -> Dead
    /// (tombstone retained) -> Live (recovery completed).
    #[test]
    fn restarted_peer_fsm_transitions_are_observable() {
        let policy = policy();
        let port = free_port();
        let b_addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();

        let a = TcpAuthorityNet::bind("a").unwrap();
        let b1 = TcpAuthorityNet::bind_at("b", b_addr).unwrap();
        a.connect_peer(b_addr).unwrap();
        let ea = a.endpoint();
        ea.broadcast_order(signed_tx(&policy, 0, 1)).unwrap();
        assert_eq!(poll_until(&b1.endpoint(), 50).len(), 1, "baseline live delivery");
        assert_eq!(a.peer_states(), vec![(Some(b_addr), PeerState::Live)]);

        b1.shutdown();
        drop(b1);
        for i in 0..30 {
            let _ = ea.broadcast_order(signed_tx(&policy, 1 + i, 1));
            if a.peer_states().first().map(|p| p.1) == Some(PeerState::Dead) {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            a.peer_states(),
            vec![(Some(b_addr), PeerState::Dead)],
            "a dead dialed peer must surface as a Dead tombstone, not a silent corpse"
        );

        let b2 = TcpAuthorityNet::bind_at("b", b_addr).unwrap();
        let eb2 = b2.endpoint();
        let mut got = Vec::new();
        for i in 0..60 {
            let _ = ea.broadcast_order(signed_tx(&policy, 100 + i, 1));
            got.extend(eb2.poll());
            if !got.is_empty() && a.peer_states().first().map(|p| p.1) == Some(PeerState::Live) {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(!got.is_empty(), "restarted peer must receive again");
        assert_eq!(
            a.peer_states(),
            vec![(Some(b_addr), PeerState::Live)],
            "recovery must complete Dead -> Live"
        );

        a.shutdown();
        b2.shutdown();
    }

    /// An accepted session has no redialable address: on death it must be
    /// demoted and reaped, never linger as a corpse in the peer set.
    #[test]
    fn accepted_corpse_is_demoted_and_reaped() {
        let policy = policy();
        let a = TcpAuthorityNet::bind("a").unwrap();
        let b = TcpAuthorityNet::bind("b").unwrap();
        b.connect_peer(a.addr()).unwrap(); // b dials a; a accepts (redial: None)
        for _ in 0..50 {
            if a.peer_states().len() == 1 {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(a.peer_states(), vec![(None, PeerState::Live)]);

        b.shutdown();
        drop(b);
        let ea = a.endpoint();
        for i in 0..30 {
            let _ = ea.broadcast_order(signed_tx(&policy, i, 1));
            if a.peer_states().is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(
            a.peer_states().is_empty(),
            "accepted corpse must be demoted and reaped, got {:?}",
            a.peer_states()
        );
        a.shutdown();
    }
}
