// KG: transport-plan Steps 2–3, 5–8 (2026-07-14); Step 4 hop metadata (2026-07-14)
//
// Narrow authority-net trait + deterministic in-memory full-mesh mailboxes.
// Each peer owns its own mailbox; broadcasts fan out `AuthorityMsg` values.
// Step 8: real TCP behind the same trait (`tcp` module).
// Step 4: Plumtree epidemic lives in `epidemic` (reuses gossip333); hop-aware
// `deliver_to_from` / `drain_with_hops` let epidemic drive targeted delivery.
// Does NOT reinvent Certificate aggregation — collectors call the existing
// `Certificate::assemble` / `verify`.
//
// Steps 5–7 (on top of 1–3):
//   5. Decoupled multi-round vote collection (`VoteCollector` / `collect_until_quorum`)
//   6. Certificate dissemination to independent `MeshLedger` replicas
//   7. Remote `Authority::confirm` from polled Cert messages
//
// Steps 5–7 drivers are generic over `N: AuthorityNet` so one code path serves
// both `MeshEndpoint` and `TcpEndpoint` (and Step 4 `EpidemicEndpoint`).
//
// Local mailbox map (not transport333::InMemoryMesh): that crate is unicast +
// identity333::NodeId point-to-point. A transfer-local AuthorityId mesh keeps
// the path self-contained without pulling parallel stacks for Steps 1–3/5–8.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::authority::{
    Authority, AuthorityError, Certificate, Certified, Committee, Verified, Vote,
};
use crate::effect::EffectAttestation;
use crate::epoch::{EpochCert, EpochProposal, EpochVote};
use crate::{AccountId, Ledger, Reject, SignedTransfer};

pub use crate::wire::AuthorityMsg;

/// Transport / mesh failure for the authority network (in-memory or TCP).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetError {
    /// Peer id is not registered on the mesh.
    UnknownPeer(String),
    /// Mesh has been marked closed / down.
    Closed,
    /// TCP / I/O failure (Step 8).
    Io(String),
}

/// Object-safe broadcast + poll surface for committee authorities (and clients).
///
/// - `broadcast_order`       — client → authorities (transfer intent)
/// - `broadcast_vote`        — authority → mesh (signed vote)
/// - `broadcast_cert`        — optional cert fan-out after assembly
/// - `broadcast_attestation` — authority → mesh (effect attestation after a
///   genuinely applied confirm; clients collect these for EffectCert finality)
/// - `poll`                  — drain this endpoint's inbox
/// `Send + Sync` on native, no bound on wasm32.
///
/// A browser endpoint holds JS handles (`RtcDataChannel` and friends) which are
/// neither `Send` nor `Sync`, and wasm32 is single-threaded anyway. Requiring the
/// bound there would make a `WasmEndpoint` impossible to write while buying
/// nothing. Native keeps the bound unchanged: authorities are shared across
/// reader/listener threads there, so dropping it would be a real regression.
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSendSync: Send + Sync {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Send + Sync + ?Sized> MaybeSendSync for T {}
#[cfg(target_arch = "wasm32")]
pub trait MaybeSendSync {}
#[cfg(target_arch = "wasm32")]
impl<T: ?Sized> MaybeSendSync for T {}

pub trait AuthorityNet: MaybeSendSync {
    fn broadcast_order(&self, order: SignedTransfer) -> Result<(), NetError>;
    fn broadcast_vote(&self, v: Vote) -> Result<(), NetError>;
    fn broadcast_cert(&self, c: Certificate) -> Result<(), NetError>;
    fn broadcast_attestation(&self, a: EffectAttestation) -> Result<(), NetError>;
    /// Epoch-change messages (committee-reconfiguration M3): operator proposal,
    /// authority assent, and the quorum certificate itself.
    fn broadcast_epoch_proposal(&self, p: EpochProposal) -> Result<(), NetError>;
    fn broadcast_epoch_vote(&self, v: EpochVote) -> Result<(), NetError>;
    fn broadcast_epoch_cert(&self, c: EpochCert) -> Result<(), NetError>;
    fn poll(&self) -> Vec<AuthorityMsg>;
}

// --- in-memory full mesh -----------------------------------------------------

/// One mailbox entry: optional previous-hop peer id (Step 4 epidemic) + payload.
#[derive(Debug, Clone)]
struct MailEntry {
    from: Option<String>,
    msg: AuthorityMsg,
}

#[derive(Default)]
struct MeshState {
    /// peer id → FIFO mailbox
    mailboxes: HashMap<String, Vec<MailEntry>>,
    closed: bool,
}

/// Shared in-process authority mesh. Every joined peer has an isolated mailbox;
/// broadcasts deliver a clone of the message into **every** mailbox.
#[derive(Default)]
pub struct InMemoryAuthorityMesh {
    inner: Mutex<MeshState>,
}

impl InMemoryAuthorityMesh {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Register `peer` and return a handle bound to that id.
    pub fn join(self: &Arc<Self>, peer: impl Into<String>) -> MeshEndpoint {
        let id = peer.into();
        {
            let mut g = self.inner.lock().expect("mesh lock");
            g.mailboxes.entry(id.clone()).or_default();
        }
        MeshEndpoint {
            me: id,
            mesh: Arc::clone(self),
        }
    }

    pub fn participants(&self) -> Vec<String> {
        self.inner
            .lock()
            .expect("mesh lock")
            .mailboxes
            .keys()
            .cloned()
            .collect()
    }

    /// Fault injection: reject further broadcasts / treat as down.
    pub fn set_closed(&self, closed: bool) {
        self.inner.lock().expect("mesh lock").closed = closed;
    }

    /// Directed inject into a single peer's mailbox (adversarial / partition tests).
    /// Full-mesh honest path uses [`AuthorityNet::broadcast_order`] instead.
    pub fn deliver_to(&self, peer: &str, msg: AuthorityMsg) -> Result<(), NetError> {
        self.deliver_to_from(peer, None, msg)
    }

    /// Targeted delivery with an explicit previous-hop id (Plumtree eager/control).
    /// `from = None` matches plain [`Self::deliver_to`].
    pub fn deliver_to_from(
        &self,
        peer: &str,
        from: Option<&str>,
        msg: AuthorityMsg,
    ) -> Result<(), NetError> {
        let mut g = self.inner.lock().expect("mesh lock");
        if g.closed {
            return Err(NetError::Closed);
        }
        let mb = g
            .mailboxes
            .get_mut(peer)
            .ok_or_else(|| NetError::UnknownPeer(peer.to_owned()))?;
        mb.push(MailEntry {
            from: from.map(|s| s.to_owned()),
            msg,
        });
        Ok(())
    }

    fn fanout(&self, msg: AuthorityMsg) -> Result<(), NetError> {
        let mut g = self.inner.lock().expect("mesh lock");
        if g.closed {
            return Err(NetError::Closed);
        }
        if g.mailboxes.is_empty() {
            return Ok(());
        }
        for mb in g.mailboxes.values_mut() {
            mb.push(MailEntry {
                from: None,
                msg: msg.clone(),
            });
        }
        Ok(())
    }

    fn drain(&self, me: &str) -> Vec<AuthorityMsg> {
        self.drain_with_hops(me)
            .into_iter()
            .map(|(_, m)| m)
            .collect()
    }

    /// Drain mailbox preserving optional previous-hop ids (Step 4 epidemic).
    pub fn drain_with_hops(&self, me: &str) -> Vec<(Option<String>, AuthorityMsg)> {
        let mut g = self.inner.lock().expect("mesh lock");
        g.mailboxes
            .get_mut(me)
            .map(std::mem::take)
            .unwrap_or_default()
            .into_iter()
            .map(|e| (e.from, e.msg))
            .collect()
    }
}

/// Per-peer handle implementing [`AuthorityNet`].
#[derive(Clone)]
pub struct MeshEndpoint {
    me: String,
    mesh: Arc<InMemoryAuthorityMesh>,
}

impl MeshEndpoint {
    pub fn id(&self) -> &str {
        &self.me
    }
}

impl AuthorityNet for MeshEndpoint {
    fn broadcast_order(&self, order: SignedTransfer) -> Result<(), NetError> {
        self.mesh.fanout(AuthorityMsg::Order(order))
    }

    fn broadcast_vote(&self, v: Vote) -> Result<(), NetError> {
        self.mesh.fanout(AuthorityMsg::Vote(v))
    }

    fn broadcast_cert(&self, c: Certificate) -> Result<(), NetError> {
        self.mesh.fanout(AuthorityMsg::Cert(c))
    }

    fn broadcast_attestation(&self, a: EffectAttestation) -> Result<(), NetError> {
        self.mesh.fanout(AuthorityMsg::Attestation(a))
    }

    fn broadcast_epoch_proposal(&self, p: EpochProposal) -> Result<(), NetError> {
        self.mesh.fanout(AuthorityMsg::EpochProposal(p))
    }

    fn broadcast_epoch_vote(&self, v: EpochVote) -> Result<(), NetError> {
        self.mesh.fanout(AuthorityMsg::EpochVote(v))
    }

    fn broadcast_epoch_cert(&self, c: EpochCert) -> Result<(), NetError> {
        self.mesh.fanout(AuthorityMsg::EpochCert(c))
    }

    fn poll(&self) -> Vec<AuthorityMsg> {
        self.mesh.drain(&self.me)
    }
}

// --- mesh certification (Step 3) ---------------------------------------------

/// One certification round over isolated mailboxes (no shared `certify` loop).
///
/// Flow:
/// 1. `client` broadcasts the order into every peer mailbox.
/// 2. Each authority drains **only its own** mailbox, runs existing `handle`,
///    and broadcasts a `Vote` on success.
/// 3. `client` (collector) drains its mailbox for votes and calls the existing
///    `Certificate::assemble` once enough distinct votes arrive.
/// 4. On success, cert is broadcast and each authority `confirm`s after polling.
///
/// Authorities are mutated only via messages that appeared in their mailbox;
/// there is no `certify(&mut [Authority])` shared-slice pass.
pub fn certify_via_mesh(
    order: &SignedTransfer,
    authorities: &mut [Authority],
    auth_endpoints: &[MeshEndpoint],
    client: &MeshEndpoint,
    committee: &Committee,
) -> (Option<Verified>, Certified) {
    assert_eq!(
        authorities.len(),
        auth_endpoints.len(),
        "one endpoint per authority"
    );

    // 1. Client publishes the order to the mesh.
    if client.broadcast_order(order.clone()).is_err() {
        return (
            None,
            Certified::Failed {
                votes: 0,
                refusals: 0,
                contested: false,
            },
        );
    }

    // 2. Each authority processes its own mailbox only.
    let mut refusals = 0usize;
    let mut contested = false;
    for (auth, ep) in authorities.iter_mut().zip(auth_endpoints.iter()) {
        for msg in ep.poll() {
            if let AuthorityMsg::Order(order) = msg {
                match auth.handle(&order) {
                    Ok(vote) => {
                        // Fan-out the signed vote; ignore mesh-down here (collector
                        // will simply see fewer votes).
                        let _ = ep.broadcast_vote(vote);
                    }
                    Err(AuthorityError::Equivocation { .. }) => {
                        refusals += 1;
                        contested = true;
                    }
                    // Durability fault => no vote. Kept out of `contested` for the
                    // same reason as in `certify`: a disk failure is not contention.
                    Err(
                        AuthorityError::OutOfOrder { .. }
                        | AuthorityError::OwnerAuth(_)
                        | AuthorityError::UnknownSender { .. }
                        | AuthorityError::ZeroAmount
                        | AuthorityError::InsufficientBalance { .. }
                        | AuthorityError::SequenceExhausted { .. }
                        | AuthorityError::BalanceOverflow { .. }
                        | AuthorityError::JournalFailed { .. }
                        | AuthorityError::Poisoned
                        | AuthorityError::EpochFencing { .. }
                        | AuthorityError::EpochCatchingUp { .. },
                    ) => {
                        refusals += 1;
                    }
                }
            }
        }
    }

    // 3. Collector drains votes from its mailbox (true transport path).
    let mut votes: Vec<Vote> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for msg in client.poll() {
        if let AuthorityMsg::Vote(v) = msg {
            if v.validate_for(order, committee).is_ok() && seen.insert(v.authority.clone()) {
                votes.push(v);
            }
        }
    }

    match Certificate::assemble(order.clone(), votes.clone(), committee) {
        Some(cert) => {
            let verified = cert
                .verify(committee)
                .expect("assembled certificate verifies");
            // 4. Disseminate cert; authorities confirm from their mailboxes and
            // broadcast their effect attestations (same path as Step 7).
            let _ = client.broadcast_cert(cert);
            confirm_from_mesh(authorities, auth_endpoints, committee);
            (Some(verified), Certified::Ok)
        }
        None => (
            None,
            Certified::Failed {
                votes: votes.len(),
                refusals,
                contested,
            },
        ),
    }
}

// --- Step 5: decoupled multi-round collection --------------------------------

/// Accumulates DISTINCT votes for one transfer from a collector mailbox.
///
/// A **round** is one drain of currently-available `Vote` messages (deterministic
/// in-memory clock = the round counter). No wall-clock, no async runtime.
/// Callers that stagger vote delivery between rounds use [`Self::poll_round`];
/// [`Self::collect_until_quorum`] / free [`collect_until_quorum`] loop that drain.
///
/// Refusals / contested flags are recorded via [`Self::note_refusal`] when the
/// caller observes `Authority::handle` outcomes (collection alone only sees votes).
#[derive(Debug, Clone)]
pub struct VoteCollector {
    order: SignedTransfer,
    votes: Vec<Vote>,
    seen: HashSet<String>,
    refusals: usize,
    contested: bool,
}

impl VoteCollector {
    pub fn new(order: SignedTransfer) -> Self {
        Self {
            order,
            votes: Vec::new(),
            seen: HashSet::new(),
            refusals: 0,
            contested: false,
        }
    }

    pub fn transfer(&self) -> &crate::Transfer {
        &self.order.transfer
    }

    pub fn order(&self) -> &SignedTransfer {
        &self.order
    }

    pub fn votes(&self) -> &[Vote] {
        &self.votes
    }

    pub fn vote_count(&self) -> usize {
        self.votes.len()
    }

    pub fn refusals(&self) -> usize {
        self.refusals
    }

    pub fn contested(&self) -> bool {
        self.contested
    }

    /// Record an authority `handle` refusal (Equivocation → contested).
    pub fn note_refusal(&mut self, err: &AuthorityError) {
        self.refusals += 1;
        if matches!(err, AuthorityError::Equivocation { .. }) {
            self.contested = true;
        }
    }

    /// One round: drain currently-available Vote messages matching this transfer
    /// (dedupe by authority id). Works for any [`AuthorityNet`] (in-mem or TCP).
    pub fn poll_round<N: AuthorityNet + ?Sized>(
        &mut self,
        collector: &N,
        committee: &Committee,
    ) {
        for msg in collector.poll() {
            if let AuthorityMsg::Vote(v) = msg {
                // Validate before dedupe: an invalid vote claiming a real id may
                // not occupy that id and suppress the later valid vote.
                if v.validate_for(&self.order, committee).is_ok()
                    && self.seen.insert(v.authority.clone())
                {
                    self.votes.push(v);
                }
            }
        }
    }

    /// Try `Certificate::assemble` with votes collected so far.
    pub fn try_assemble(&self, committee: &Committee) -> Option<Certificate> {
        Certificate::assemble(self.order.clone(), self.votes.clone(), committee)
    }

    /// Failure snapshot matching `certify` / `certify_via_mesh` counters.
    pub fn failed_status(&self) -> Certified {
        Certified::Failed {
            votes: self.votes.len(),
            refusals: self.refusals,
            contested: self.contested,
        }
    }

    /// Poll up to `max_rounds` drains; assemble+verify on quorum, else Failed.
    /// `pause` is `Duration::ZERO` for deterministic in-memory tests; TCP tests
    /// may pass a short sleep to absorb socket/thread latency.
    pub fn collect_until_quorum_with_pause<N: AuthorityNet + ?Sized>(
        &mut self,
        collector: &N,
        committee: &Committee,
        max_rounds: usize,
        pause: Duration,
    ) -> (Option<Certificate>, Certified) {
        for r in 0..max_rounds {
            self.poll_round(collector, committee);
            if let Some(cert) = self.try_assemble(committee) {
                // assemble already ran is_valid; verify is the type-state mint path.
                let _ = cert
                    .verify(committee)
                    .expect("assembled certificate verifies");
                return (Some(cert), Certified::Ok);
            }
            if r + 1 < max_rounds && !pause.is_zero() {
                std::thread::sleep(pause);
            }
        }
        (None, self.failed_status())
    }

    /// Poll up to `max_rounds` drains (no wall-clock pause).
    pub fn collect_until_quorum<N: AuthorityNet + ?Sized>(
        &mut self,
        collector: &N,
        committee: &Committee,
        max_rounds: usize,
    ) -> (Option<Certificate>, Certified) {
        self.collect_until_quorum_with_pause(collector, committee, max_rounds, Duration::ZERO)
    }
}

/// Free-function multi-round collection from `collector`'s mailbox.
///
/// Polls across up to `max_rounds` rounds (one drain each). Accumulates DISTINCT
/// valid votes matching `transfer` until `committee.quorum()`, then
/// `Certificate::assemble` + `verify`. Sub-quorum after `max_rounds` → Failed.
///
/// Does not observe authority refusals (no wire refuse message); for contested
/// splits use [`VoteCollector::note_refusal`] then [`VoteCollector::collect_until_quorum`].
pub fn collect_until_quorum<N: AuthorityNet + ?Sized>(
    collector: &N,
    order: &SignedTransfer,
    committee: &Committee,
    max_rounds: usize,
) -> (Option<Verified>, Certified) {
    let mut coll = VoteCollector::new(order.clone());
    match coll.collect_until_quorum(collector, committee, max_rounds) {
        (Some(cert), Certified::Ok) => {
            let verified = cert
                .verify(committee)
                .expect("assembled certificate verifies");
            (Some(verified), Certified::Ok)
        }
        (_, status) => (None, status),
    }
}

/// One authority-side round: each authority drains **its own** mailbox, runs
/// existing `handle` on Orders, broadcasts Votes on success, and records
/// refusals on the collector (for Failed counters / contested).
pub fn authority_handle_round<N: AuthorityNet>(
    authorities: &mut [Authority],
    auth_endpoints: &[N],
    collector: &mut VoteCollector,
) {
    assert_eq!(
        authorities.len(),
        auth_endpoints.len(),
        "one endpoint per authority"
    );
    for (auth, ep) in authorities.iter_mut().zip(auth_endpoints.iter()) {
        for msg in ep.poll() {
            if let AuthorityMsg::Order(order) = msg {
                match auth.handle(&order) {
                    Ok(vote) => {
                        let _ = ep.broadcast_vote(vote);
                    }
                    Err(err) => collector.note_refusal(&err),
                }
            }
        }
    }
}

// --- Step 6: certificate dissemination → independent ledgers -----------------

/// A ledger replica with its own mesh mailbox and local balance state.
/// Committee is supplied at verify/apply time (verifier-owned, never caller size).
/// Generic over the endpoint type so TCP ledgers reuse the same path.
pub struct MeshLedger<N: AuthorityNet = MeshEndpoint> {
    endpoint: N,
    ledger: Ledger,
}

impl<N: AuthorityNet> MeshLedger<N> {
    pub fn new(endpoint: N, ledger: Ledger) -> Self {
        Self { endpoint, ledger }
    }

    pub fn endpoint(&self) -> &N {
        &self.endpoint
    }

    pub fn ledger(&self) -> &Ledger {
        &self.ledger
    }

    pub fn ledger_mut(&mut self) -> &mut Ledger {
        &mut self.ledger
    }

    /// Poll this replica's mailbox; for each Cert, verify against the **local**
    /// `committee` and `apply_verified` on success.
    pub fn poll_and_apply(&mut self, committee: &Committee) -> Vec<Result<(), Reject>> {
        let mut outcomes = Vec::new();
        for msg in self.endpoint.poll() {
            if let AuthorityMsg::Cert(c) = msg {
                match c.verify(committee) {
                    Some(v) => outcomes.push(self.ledger.apply_verified(&v, committee)),
                    None => outcomes.push(Err(Reject::NoCertificate)),
                }
            }
        }
        outcomes
    }
}

impl MeshLedger<MeshEndpoint> {
    pub fn id(&self) -> &str {
        self.endpoint.id()
    }
}

/// Broadcast an assembled certificate and have each independent replica
/// poll → verify (local committee) → `apply_verified` (local ledger).
pub fn disseminate_certificate<B: AuthorityNet + ?Sized, L: AuthorityNet>(
    broadcaster: &B,
    cert: &Certificate,
    committee: &Committee,
    replicas: &mut [MeshLedger<L>],
) -> Result<Vec<Vec<Result<(), Reject>>>, NetError> {
    broadcaster.broadcast_cert(cert.clone())?;
    let mut all = Vec::with_capacity(replicas.len());
    for r in replicas.iter_mut() {
        all.push(r.poll_and_apply(committee));
    }
    Ok(all)
}

/// Like [`disseminate_certificate`], but retries replica polls with an optional
/// pause (TCP latency). In-memory tests use the non-pausing path.
pub fn disseminate_certificate_with_pause<B: AuthorityNet + ?Sized, L: AuthorityNet>(
    broadcaster: &B,
    cert: &Certificate,
    committee: &Committee,
    replicas: &mut [MeshLedger<L>],
    max_polls: usize,
    pause: Duration,
) -> Result<Vec<Vec<Result<(), Reject>>>, NetError> {
    broadcaster.broadcast_cert(cert.clone())?;
    let mut all = vec![Vec::new(); replicas.len()];
    for p in 0..max_polls {
        for (i, r) in replicas.iter_mut().enumerate() {
            let batch = r.poll_and_apply(committee);
            all[i].extend(batch);
        }
        if all.iter().all(|v| !v.is_empty()) {
            break;
        }
        if p + 1 < max_polls && !pause.is_zero() {
            std::thread::sleep(pause);
        }
    }
    Ok(all)
}

// --- Step 7: remote confirm over the mesh ------------------------------------

/// Remote authorities drain their mailboxes for `Cert` messages, verify against
/// the local committee, and call existing `Authority::confirm` (advances
/// `next_expected`) — no shared in-process confirm loop.
///
/// After a successful confirm each authority broadcasts its
/// [`EffectAttestation`] (attest-iff-applied: `Authority::attestation_for`
/// yields one only when the apply genuinely committed), so a client polling
/// the mesh can collect an EffectCert. A failed confirm broadcasts nothing.
pub fn confirm_from_mesh<N: AuthorityNet>(
    authorities: &mut [Authority],
    auth_endpoints: &[N],
    committee: &Committee,
) {
    assert_eq!(
        authorities.len(),
        auth_endpoints.len(),
        "one endpoint per authority"
    );
    for (auth, ep) in authorities.iter_mut().zip(auth_endpoints.iter()) {
        for msg in ep.poll() {
            if let AuthorityMsg::Cert(c) = msg {
                if let Some(v) = c.verify(committee) {
                    if auth.confirm(&v, committee).is_ok() {
                        let account = v.transfer().from.clone();
                        let seq = v.transfer().from_seq;
                        if let Some(attestation) = auth.attestation_for(&account, seq) {
                            // Fan-out the attestation; ignore mesh-down here
                            // (collectors will simply see fewer attestations).
                            let _ = ep.broadcast_attestation(attestation);
                        }
                    }
                }
            }
        }
    }
}

/// Client-side EffectCert collection: drain `collector`'s mailbox for
/// `Attestation` messages and keep the DISTINCT, committee-valid ones bound to
/// exactly `(account, seq, order_id)`. Feed the result to
/// [`crate::is_final_effectcert`] / [`crate::EffectCert::assemble`]; a bare
/// certificate is only provisional until this yields a quorum.
pub fn collect_effect_attestations<N: AuthorityNet + ?Sized>(
    collector: &N,
    account: &AccountId,
    seq: u64,
    order_id: [u8; 32],
    committee: &Committee,
) -> Vec<EffectAttestation> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for msg in collector.poll() {
        if let AuthorityMsg::Attestation(a) = msg {
            // Validate before dedupe for the same reason as VoteCollector: an
            // invalid attestation claiming a real id may not occupy that id.
            if &a.account == account
                && a.seq == seq
                && a.order_id == order_id
                && a.is_valid(committee)
                && seen.insert(a.authority.clone())
            {
                out.push(a);
            }
        }
    }
    out
}

/// Multi-round certification path (Steps 5–7 composed):
/// broadcast order → authority handle + collect rounds → on Ok, broadcast cert
/// + remote confirm. Generic over any [`AuthorityNet`].
///
/// `pause` is `Duration::ZERO` for deterministic in-memory tests; TCP tests
/// pass a short sleep so orders/votes/certs can land in peer inboxes.
pub fn certify_via_mesh_rounds_with_pause<N: AuthorityNet>(
    order: &SignedTransfer,
    authorities: &mut [Authority],
    auth_endpoints: &[N],
    client: &N,
    committee: &Committee,
    max_rounds: usize,
    pause: Duration,
) -> (Option<Verified>, Option<Certificate>, Certified) {
    assert_eq!(
        authorities.len(),
        auth_endpoints.len(),
        "one endpoint per authority"
    );

    if client.broadcast_order(order.clone()).is_err() {
        return (
            None,
            None,
            Certified::Failed {
                votes: 0,
                refusals: 0,
                contested: false,
            },
        );
    }

    let mut coll = VoteCollector::new(order.clone());
    // Interleave authority handle + collector poll so TCP latency on Order
    // delivery still reaches handle before we give up (in-mem: first round
    // still sees the order immediately after broadcast).
    for r in 0..max_rounds {
        authority_handle_round(authorities, auth_endpoints, &mut coll);
        coll.poll_round(client, committee);
        if let Some(cert) = coll.try_assemble(committee) {
            let verified = cert
                .verify(committee)
                .expect("assembled certificate verifies");
            let _ = client.broadcast_cert(cert.clone());
            // Confirm may need a few polls under TCP; empty drains are no-ops
            // for in-memory once the cert is already consumed.
            for c in 0..max_rounds {
                confirm_from_mesh(authorities, auth_endpoints, committee);
                if c + 1 < max_rounds && !pause.is_zero() {
                    std::thread::sleep(pause);
                }
            }
            return (Some(verified), Some(cert), Certified::Ok);
        }
        if r + 1 < max_rounds && !pause.is_zero() {
            std::thread::sleep(pause);
        }
    }
    (None, None, coll.failed_status())
}

/// Multi-round certification path (Steps 5–7, no wall-clock pause).
/// Separates collection from single-pass [`certify_via_mesh`].
pub fn certify_via_mesh_rounds<N: AuthorityNet>(
    order: &SignedTransfer,
    authorities: &mut [Authority],
    auth_endpoints: &[N],
    client: &N,
    committee: &Committee,
    max_rounds: usize,
) -> (Option<Verified>, Option<Certificate>, Certified) {
    certify_via_mesh_rounds_with_pause(
        order,
        authorities,
        auth_endpoints,
        client,
        committee,
        max_rounds,
        Duration::ZERO,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority::{authority_signing_message, Authority, Committee};
    use crate::{NetworkId, OwnerRegistry, Transfer, TransferPolicy};
    use ed25519_dalek::{Signer, SigningKey};

    fn raw_tx(from: &str, seq: u64, to: &str, amount: u128) -> Transfer {
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

    fn owner_key(account: &str) -> SigningKey {
        match account {
            "alice" => key(42),
            "bob" => key(43),
            "carol" => key(44),
            "a" => key(45),
            "b" => key(46),
            other => panic!("no test owner key for {other}"),
        }
    }

    fn policy() -> TransferPolicy {
        TransferPolicy::new(
            NetworkId::new("net-testnet").unwrap(),
            OwnerRegistry::new(
                ["alice", "bob", "carol", "a", "b"]
                    .map(|account| (account, owner_key(account).verifying_key())),
            )
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

    fn committee(n: u8, policy: &TransferPolicy) -> Committee {
        Committee::new(
            (0..n).map(|i| (format!("a{i}"), key(i).verifying_key())),
            policy.clone(),
        )
        .unwrap()
    }

    fn tx(from: &str, seq: u64, to: &str, amount: u128) -> SignedTransfer {
        let policy = policy();
        SignedTransfer::sign(&policy, raw_tx(from, seq, to, amount), &owner_key(from))
    }

    fn setup_mesh(n: u8) -> (Committee, Vec<Authority>, Arc<InMemoryAuthorityMesh>, Vec<MeshEndpoint>, MeshEndpoint) {
        let policy = policy();
        let committee = committee(n, &policy);
        let committee_id = committee.id();
        let authorities: Vec<Authority> = (0..n)
            .map(|i| {
                Authority::new(
                    format!("a{i}"),
                    key(i),
                    policy.clone(),
                    committee_id,
                    authority_genesis(),
                )
            })
            .collect();
        let mesh = InMemoryAuthorityMesh::new();
        let endpoints: Vec<MeshEndpoint> = authorities
            .iter()
            .map(|a| mesh.join(a.id().clone()))
            .collect();
        let client = mesh.join("client");
        (committee, authorities, mesh, endpoints, client)
    }

    #[test]
    fn mock_mailbox_delivers_order_vote_cert() {
        let mesh = InMemoryAuthorityMesh::new();
        let a0 = mesh.join("a0");
        let a1 = mesh.join("a1");
        let t = tx("alice", 0, "bob", 1);

        a0.broadcast_order(t.clone()).unwrap();
        let m0 = a0.poll();
        let m1 = a1.poll();
        assert!(matches!(m0.as_slice(), [AuthorityMsg::Order(_)]));
        assert!(matches!(m1.as_slice(), [AuthorityMsg::Order(_)]));
        // Second poll is empty (drain).
        assert!(a0.poll().is_empty());

        // Vote fan-out after a synthetic vote.
        let policy = policy();
        let committee = committee(1, &policy);
        let mut auth = Authority::new(
            "a0",
            key(0),
            policy,
            committee.id(),
            authority_genesis(),
        );
        let v = auth.handle(&t).unwrap();
        a0.broadcast_vote(v).unwrap();
        assert!(matches!(a1.poll().as_slice(), [AuthorityMsg::Vote(_)]));
    }

    #[test]
    fn trait_object_safe_broadcast() {
        let mesh = InMemoryAuthorityMesh::new();
        let ep = mesh.join("x");
        let net: &dyn AuthorityNet = &ep;
        net.broadcast_order(tx("a", 0, "b", 1)).unwrap();
        assert_eq!(net.poll().len(), 1);
    }

    #[test]
    fn invalid_vote_cannot_poison_later_valid_vote_from_same_authority() {
        let (committee, mut authorities, _mesh, endpoints, client) = setup_mesh(4);
        let order = tx("alice", 0, "bob", 1);
        let forged_authority = "a0".to_string();
        let forged = Vote {
            authority: forged_authority.clone(),
            committee_id: committee.id(),
            policy_id: order.policy_id(),
            network_id: order.network_id.clone(),
            transfer: order.transfer.clone(),
            round: order.round,
            signature: key(99).sign(&authority_signing_message(
                &forged_authority,
                &committee.id(),
                &order.policy_id(),
                &order.network_id,
                &order.transfer,
                order.round,
            )),
        };
        let valid = authorities[0].handle(&order).unwrap();

        client.broadcast_vote(forged).unwrap();
        endpoints[0].broadcast_vote(valid.clone()).unwrap();
        let mut collector = VoteCollector::new(order);
        collector.poll_round(&client, &committee);

        assert_eq!(collector.vote_count(), 1);
        assert_eq!(collector.votes()[0], valid);
    }

    #[test]
    fn honest_transfer_certifies_over_mesh_without_shared_certify() {
        let (committee, mut authorities, _mesh, endpoints, client) = setup_mesh(4);
        assert_eq!(committee.quorum(), 3);

        let t = tx("alice", 0, "bob", 30);
        let (verified, status) =
            certify_via_mesh(&t, &mut authorities, &endpoints, &client, &committee);
        assert_eq!(status, Certified::Ok);
        let verified = verified.expect("quorum over mailboxes");

        let mut ledger = Ledger::genesis([("alice".to_string(), 100u128), ("bob".to_string(), 0u128)]);
        ledger.apply_verified(&verified, &committee).unwrap();
        assert_eq!(ledger.balance(&"bob".to_string()), 30);
        assert_eq!(ledger.balance(&"alice".to_string()), 70);
        assert_eq!(ledger.total_supply(), 100);
    }

    #[test]
    fn mesh_equivocation_barred_after_confirm() {
        // Port of tests/certified_transfer.rs Byzantine path onto mailboxes.
        let (committee, mut authorities, _mesh, endpoints, client) = setup_mesh(4);
        let mut ledger = Ledger::genesis([
            ("alice".to_string(), 100u128),
            ("bob".to_string(), 0u128),
            ("carol".to_string(), 0u128),
        ]);

        let (v1, s1) = certify_via_mesh(
            &tx("alice", 0, "bob", 100),
            &mut authorities,
            &endpoints,
            &client,
            &committee,
        );
        assert_eq!(s1, Certified::Ok);
        ledger.apply_verified(&v1.unwrap(), &committee).unwrap();

        // Reuse seq 0 for a different recipient: every authority is past seq 0.
        let (v2, s2) = certify_via_mesh(
            &tx("alice", 0, "carol", 100),
            &mut authorities,
            &endpoints,
            &client,
            &committee,
        );
        assert!(v2.is_none());
        assert!(matches!(s2, Certified::Failed { .. }));
        assert_eq!(ledger.balance(&"bob".to_string()), 100);
        assert_eq!(ledger.balance(&"carol".to_string()), 0);
        assert_eq!(ledger.total_supply(), 100);
    }

    #[test]
    fn mesh_sequential_transfers_in_order_only() {
        let (committee, mut authorities, _mesh, endpoints, client) = setup_mesh(4);
        let mut ledger =
            Ledger::genesis([("alice".to_string(), 100u128), ("bob".to_string(), 0u128)]);

        for (seq, amt) in [(0u64, 40u128), (1, 25)] {
            let (v, s) = certify_via_mesh(
                &tx("alice", seq, "bob", amt),
                &mut authorities,
                &endpoints,
                &client,
                &committee,
            );
            assert_eq!(s, Certified::Ok, "seq {seq}");
            ledger.apply_verified(&v.unwrap(), &committee).unwrap();
        }
        assert_eq!(ledger.balance(&"bob".to_string()), 65);

        let (v3, s3) = certify_via_mesh(
            &tx("alice", 3, "bob", 5),
            &mut authorities,
            &endpoints,
            &client,
            &committee,
        );
        assert!(v3.is_none());
        assert!(matches!(
            s3,
            Certified::Failed {
                contested: false,
                ..
            }
        ));
    }

    #[test]
    fn mesh_equivocation_split_before_cert_is_contested() {
        // Half the committee locks T1, half locks T2 via *directed* mailbox
        // injection (not full-mesh order) to reproduce the in-process contested
        // split; then a full-mesh certify of T1 fails as contested.
        let (committee, mut authorities, mesh, endpoints, client) = setup_mesh(4);
        let t1 = tx("alice", 0, "bob", 100);
        let t2 = tx("alice", 0, "carol", 100);

        // Direct-inject distinct orders into individual authority mailboxes.
        mesh.deliver_to("a0", AuthorityMsg::Order(t1.clone())).unwrap();
        mesh.deliver_to("a1", AuthorityMsg::Order(t1.clone())).unwrap();
        mesh.deliver_to("a2", AuthorityMsg::Order(t2.clone())).unwrap();
        mesh.deliver_to("a3", AuthorityMsg::Order(t2.clone())).unwrap();
        for (auth, ep) in authorities.iter_mut().zip(endpoints.iter()) {
            for msg in ep.poll() {
                if let AuthorityMsg::Order(order) = msg {
                    let _ = auth.handle(&order);
                }
            }
        }

        // Full-mesh certify T1: a0/a1 re-vote, a2/a3 Equivocation → < quorum.
        let (v, r) = certify_via_mesh(&t1, &mut authorities, &endpoints, &client, &committee);
        assert!(v.is_none());
        assert_eq!(
            r,
            Certified::Failed {
                votes: 2,
                refusals: 2,
                contested: true
            }
        );
    }

    #[test]
    fn closed_mesh_rejects_broadcast() {
        let mesh = InMemoryAuthorityMesh::new();
        let ep = mesh.join("a0");
        mesh.set_closed(true);
        assert_eq!(
            ep.broadcast_order(tx("a", 0, "b", 1)),
            Err(NetError::Closed)
        );
    }

    // M3 transport path: after a mesh certification the authorities' real
    // confirms broadcast their effect attestations, and the client collects a
    // quorum from its own mailbox -> EffectCert finality over the wire, with no
    // in-process hand-off of attestation sets.
    #[test]
    fn mesh_confirm_broadcasts_attestations_client_assembles_effectcert() {
        let (committee, mut authorities, _mesh, endpoints, client) = setup_mesh(4);
        let t = tx("alice", 0, "bob", 30);
        let oid = t.order_id();
        let (verified, status) =
            certify_via_mesh(&t, &mut authorities, &endpoints, &client, &committee);
        assert_eq!(status, Certified::Ok);
        assert!(verified.is_some());

        let attestations =
            collect_effect_attestations(&client, &"alice".to_string(), 0, oid, &committee);
        assert_eq!(
            attestations.len(),
            4,
            "every applied authority's attestation reached the client mailbox"
        );
        assert!(crate::is_final_effectcert("alice", 0, oid, &attestations, &committee));

        // The same drain yields nothing for a slot nothing applied at.
        assert!(collect_effect_attestations(&client, &"alice".to_string(), 1, oid, &committee)
            .is_empty());
    }
}
