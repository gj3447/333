# Committee Reconfiguration (Epoch Changes) — Design

> Status: DESIGN v1.1 (2026-08-03), M0–M3 implemented and live-gated.
> Closes: `LakatosTree_333_Cryptocurrency_20260721` question `committee-reconfiguration-epoch`.
> Audit basis: `SYMPOSIUM/FINDINGS/audit-333-fsm-vs-borg-k8s-2026-07-15/FSM_AUDIT_REPORT.md` (P1: "Committee 정적 — wire에 재구성 메시지 없음").
> Acceptance (from the tree question): wire epoch/committee-change messages + 2-phase reconfiguration FSM + quorum-intersection safety oracle tests.
>
> **v1.1 revisions (discovered during M3 implementation):**
> 1. **`EpochVote` carries the full `frontier`**, not just its digest — a
>    reconfig collector has no request/response channel, so the certificate
>    could never be assembled from digests alone. `frontier_digest` stays in
>    the signed preimage; consumers check the pair is self-consistent.
>    (Epoch-wire v1 was unreleased — no producer existed before M3 — so the
>    layout was revised in place rather than versioned.)
> 2. **Joining members boot as observers.** A not-yet-member cannot pass the
>    boot-time membership check nor `confirm()`'s self-key binding. The
>    `--observe` daemon mode boots anyway, follows the log via
>    `confirm_as_observer` (identical validation minus exactly the self-key
>    check), installs the epoch cert, and votes only once it IS a member.
>    Its pre-membership votes are rejected network-wide by every collector.
> 3. **Joiners must be in the mesh peer list.** The half-mesh dial rule
>    (dial strictly greater addresses) isolates any node whose address isn't
>    in the others' `--peers`; observers are added to `--peers` from boot.

## 0. Problem

`Committee` is a static CLI flag (`bin/node.rs --committee`). Membership cannot
change without a full stop-and-restart of every authority at once, which is not
an operation a live network has. We need to add/remove/replace authorities
while preserving:

- **Safety**: never two conflicting certificates for the same slot — including
  across the reconfiguration boundary itself.
- **Liveness**: reconfiguration completes under the same partial-synchrony
  assumptions the network already makes, and retired members can actually stop.

## 1. What the code already gives us (verified 2026-08-03)

| Fact | Location | Consequence |
|---|---|---|
| `Vote` binds `committee_id`, `policy_id`, `network_id`, `round` | `authority.rs:206-219` | Votes from different committees can never mix into one certificate. The epoch-boundary anchor already exists. |
| `Verified` retains `committee_id`; `confirm()` rejects `WrongCommittee` | `authority.rs:787-798, 900` | A certificate from a foreign committee is refused at apply time. |
| `quorum(n) = n - floor((n-1)/3)` preserves honest intersection of any two quorums | `authority.rs:105-115`, property test `:1157` | Within-epoch uniqueness is proven, incl. non-3f+1 sizes. |
| Wire rejects unknown tags fail-closed | `wire.rs:61-62` (`UnknownTag`) | Old binaries refuse new message types instead of misreading them — clean version skew behavior. |
| Quiet-period certificate re-presentation (anti-entropy) | `bin/node.rs` (`applied_certs`, `cert_rebroadcast`) | Stragglers converge level-triggered; epoch-change certs ride the same channel. |
| `Committee::new` fails closed on empty/oversized/duplicate-alias/duplicate-key rosters | `authority.rs:148-179` | Roster validation for proposed committees is already written. |

Nothing here is thrown away. Reconfiguration is an **extension**, not a fork:
epoch 0 is the CLI committee, and every later epoch is justified by a
certificate from the previous one.

## 2. Model

- An **epoch** `e >= 0` is a committee generation with roster `C_e`
  (`CommitteeId_e = digest(policy_id, roster_e)`, existing scheme).
- Epoch 0 = the static CLI committee (today's behavior, unchanged).
- An **epoch change** `e -> e+1` is authorized by a certificate of `C_e` over
  an `EpochChange` value: `(next_roster, frontier)`. No outside trust root is
  introduced — the old committee signs its own succession.
- **`round` (owner re-vote slot bump, 2026-07-23) is orthogonal to epoch.**
  Round = owner's recovery from a split vote within one epoch. Epoch =
  committee generation. A slot remains `(account, seq, round)`; votes
  additionally resolve against the epoch's committee (they already bind
  `committee_id`, so this costs nothing).

## 3. The race that forces two phases

Naive one-shot reconfiguration is unsafe:

1. Quorum of `C_e` signs `EpochChange(roster', F)` committing to frontier `F`.
2. Concurrently, a user order `X` is collecting votes under `C_e`.
3. If `X`'s certificate completes *after* `F` was committed, the new epoch
   starts from a frontier that excludes `X` — and `X`'s slot can be signed
   again under `C_{e+1}`. **Double spend across the boundary.**

So reconfiguration must **fence first, agree second** (the same shape as
Sui's epoch boundary and etcd-raft's joint consensus, adapted to
consensusless quorum certs):

- **Phase 1 — Fence.** Upon accepting a valid `EpochProposal(epoch = e+1)`,
  an authority **stops voting on new user orders under `C_e`** (the fence).
  Any user order needs a quorum of `C_e`; the fence quorum intersects it in at
  least one honest member who now refuses — so no *new* user certificate can
  form under `C_e` once a quorum has fenced. Certificates whose votes
  completed *before* the fence still exist and are still applied (only
  **voting** is fenced, never `confirm()`).
- **Phase 2 — Change.** In-flight pre-fence certificates are carried to
  everyone by quiet-period re-presentation (existing anti-entropy). When an
  authority's frontier has been **stable for the re-presentation interval**
  (no new `cert_applied` across a full quiet window), it signs
  `EpochVote(e+1, roster', frontier_digest)`. An `EpochCert` is valid only
  when quorum of `C_e` signs the **same** `(roster', frontier_digest)`.
- **Install.** On a verified `EpochCert`: drop all epoch-e locks (Sui
  semantics — a lock is a promise to a committee that no longer exists;
  unconfirmed orders are re-submitted by their owners under the new epoch),
  install `C_{e+1}`, keep `confirmed`/ledger state, refuse to vote until the
  local frontier **covers** the committed frontier (per-account `next_seq`
  comparison; the roster of accounts is bounded by `OwnerRegistry`, so the
  frontier map is small). Covering happens mechanically via re-presentation.

Cross-epoch slot safety then reduces to the within-epoch proof: any
certificate under `C_{e+1}` for slot `(a, s)` requires the local frontier to
have `(a, s)` unspent; the committed frontier records every `(a, s)` that can
ever be certified under `C_e` (Phase 1 shows no later ones exist); so
`C_{e+1}` never re-signs a spent slot.

## 4. Wire additions (`wire.rs`)

New tags and domains — never a reused tag (project convention):

| Tag | Message | Domain |
|---|---|---|
| 4 | `EpochProposal { network_id, policy_id, epoch, next_roster }` (operator → authorities) | `transfer333/wire-epoch-proposal/v1` |
| 5 | `EpochVote { authority, committee_id, epoch, next_roster_digest, frontier_digest, frontier, signature }` | `transfer333/wire-epoch-vote/v1` |
| 6 | `EpochCert { epoch, next_roster, frontier, votes }` | `transfer333/wire-epoch-cert/v1` |

`frontier` = `Vec<(AccountId, u64)>` (next-seq per account), bounded by the
owner roster. `next_roster_digest` mirrors the `CommitteeId` scheme so an
`EpochCert` is self-authenticating exactly like a `Certificate`.

## 5. Authority epoch FSM

```
Active(e)  --valid EpochProposal(e+1)-->  Fencing(e, proposal)
Fencing    --frontier stable over quiet window-->  signs EpochVote
Fencing    --valid EpochCert(e+1)-->  Installing(e+1)
Installing --local frontier covers committed frontier-->  Active(e+1)
```

Named transitions only, illegal transitions panic (the `tcp.rs`/`webrtc.rs`
discipline). Notable rules:

- `Active(e)` receiving `EpochProposal` for `epoch != e+1` → ignore (logged),
  never fence.
- `Fencing`: user `handle()` returns `EpochFencing` error (a new, typed
  rejection — distinct from equivocation; clients retry after the change).
- `Fencing` never refuses `confirm()` — pre-fence certs must still apply.
- Duplicate/conflicting `EpochCert` for the same epoch with different
  `(roster, frontier)` = evidence of ≥f+1 Byzantine members: fail-stop and
  emit (same poison class as a durability fault).

## 6. Operator path

`node reconfig --epoch <e+1> --next-committee a0=<hex>,...,b0=<hex> --peers ...`:
broadcasts `EpochProposal`, collects `EpochVote`s like `VoteCollector`
(reuse the collection pattern, distinct types), assembles + broadcasts
`EpochCert`. v1 is deliberately operator-driven (no autonomous reconfig
trigger); the operator is expected to quiet user traffic first — the fence
makes this safe rather than merely polite.

## 7. Oracle tests (ooptdd, to be added with implementation)

- `REQ-EPOCH-CHANGE`: reconfigure 4 → 4 (one member swapped). A transfer then
  certifies under the new committee; the retired member's votes are rejected.
- `REQ-EPOCH-SAFETY`: adversarial — the owner signs two conflicting orders
  for the same slot, one submitted pre-fence, one submitted post-change. At
  most one certificate ever exists; all converged ledgers agree.
- `REQ-EPOCH-STRAGGLER`: one authority is down through the whole change,
  boots late, and converges to the new epoch + frontier via re-presentation
  (epoch certs join the `applied_certs` re-broadcast set).
- Negative oracle: all three RED against a pre-change binary, mirroring the
  anti-entropy gates' proof method.

## 8. Milestones

- **M0** scaffolding: `Committee::with_epoch`, `Authority::epoch()` accessor,
  current-epoch in events. No behavior change. **DONE** (`a3e4908`).
- **M1** wire types + codec + domains (roundtrip/unknown-tag tests).
  **DONE** (`8a86f73`; v1.1 revised `EpochVote` to carry the frontier).
- **M2** authority epoch FSM (fence/vote/install) + frontier digest + unit
  tests (illegal transitions panic; fence blocks votes, never confirms).
  **DONE** (`2dc1e4a` — 9 integration tests, fully durable via journal).
- **M3** `node reconfig` operator path (proposal → vote collection → cert)
  + daemon epoch-message handling + quiet-window vote signing + `--observe`
  join path + `REQ-EPOCH-CHANGE` live gate (14/14, negative oracle RED
  against pre-M3 binary). **DONE** (this change).
- **M4** remaining oracle gates: `REQ-EPOCH-SAFETY` (boundary double-spend
  attempt: old-committee submit post-change must fail, conflicting order
  re-signed under new epoch must never certify), `REQ-EPOCH-STRAGGLER`
  (authority down through the whole change converges via re-presentation).
- **M5** hardening: epoch certs in the anti-entropy set (done in M3),
  recovered-authority epoch-cert re-presentation (journal →
  `applied_epoch_certs` refill), observe-mode vote suppression (currently
  observers broadcast ignored votes — harmless noise, not a safety issue).

## 9. Non-goals / explicit risks

- **No autonomous reconfiguration** in v1 (no on-chain governance, no DKG).
- **Quiet-window starvation**: under *continuous* load the stability window
  may keep slipping; v1 accepts this (the operator coordinates a traffic
  pause). A fence-certificate phase that removes the heuristics is v2.
- **Frontier size** is bounded by the owner roster; if the roster ever becomes
  permissionless this needs a digest-only commitment instead.
- **Fresh member joining AFTER a completed change** (never observed the old
  epoch): its trust-root assertion and the foreignness of old-epoch certs
  need an explicit state-transfer path (balances are not in the frontier) —
  v2. The M3 observe-join covers members who observe *before* the switch.
- Removed members learn nothing new; members removed *and* re-added later
  rejoin as fresh (state sync via re-presentation, as with any straggler).
