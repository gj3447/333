# 333 — consensusless P2P OS (Rust)

A single-owner, consensus-number-1 asset-transfer substrate and its surrounding
P2P OS crates, built PROM-style (test-first, no fake green, adversarial self-check).
Lineage: FastPay (Baudet et al., Meta 2020), Zef (2022), Sui Lutris / Mysticeti
fast path (2024), Linera single-user microchains (2025), and the consensus-number-1
asset-transfer theorem (Guerraoui et al., PODC 2019).

> This repo is the `SERVER/07_PROJECTS/333-*` line — crates named `transfer333`,
> `crdt333`, `identity333`, … — 30 crates linked by relative path deps (kept as
> sibling directories; no forced top-level workspace so a WIP crate never blocks
> the others).
>
> **`substrate/` (absorbed 2026-07-21)** — the former `gj3447/p333` repo, the
> delltower-lineage *honest-333 v2 substrate*: a self-contained Cargo workspace
> (`p333-*` crates: `identity`/`discovery`/`ltdd`/`metering`/`crdt`/`replay`/
> `consensus`/`billing`/`relay`/`relay-billing`/`relay-client-wasm`/`wal`), each
> gated by an LTDD green receipt. Merged here via `git subtree` (full history
> preserved); `gj3447/p333` is archived and points here. It builds independently
> (`cd substrate && cargo test --locked`) and its `p333-` names do not collide
> with the top-level `*333` crates, so the two lineages coexist. The overlapping
> domains (`consensus`/`crdt`/`identity`) are kept side-by-side by design: the
> `*333` versions are the broad P2P-OS line, the `p333-*` versions the
> receipt-driven substrate line.

## Verified core

- **`333-transfer` (`transfer333`)** — consensusless single-owner asset transfer +
  FastPay-style authority quorum-certificate layer.
  - Safety core: lock-on-first-seen + next-seq ordering + `quorum(n)=n−f` uniqueness
    (any two quorums intersect in ≥1 honest authority ⇒ ≤1 transfer per (account,seq)
    can certify) + committee-bound verification + type-state `Verified` rail.
  - **Authenticity (2026-07-14):** authority `Vote`s carry **real Ed25519 (RFC 8032)**
    signatures over domain-separated, length-prefixed canonical transfer bytes;
    `Committee` binds `AuthorityId → VerifyingKey`; `Certificate::is_valid` verifies
    every vote per authority key. A vote is unforgeable outside the secret-key holder.
  - **Owner authorization (2026-07-15):** every certified-rail order is a
    policy-bound `SignedTransfer`. `PolicyId` commits to the deployment id and
    sorted owner roster; `CommitteeId` additionally commits to the sorted authority
    roster. Authorities verify the registered sender key **before** sequence
    inspection or slot locking, and certificates re-check both owner proof and
    authority quorum. Public committee/owner rosters are separated from private
    `--key-file` loading; debug-only `--dev-seed` is rejected by release builds.
  - **Authority FSM/state validity (2026-07-15):** each authority owns one canonical
    `(balance, next_seq, pending)` state. Admission checks owner proof, pending-slot
    conflict, sender existence, positive amount, exact sequence, and sufficient
    balance before voting. Certificate confirmation debits, credits, and advances
    sequence in one ledger transition; a failed confirmation changes none of them.
  - Transport is implemented behind one `AuthorityNet` boundary: deterministic
    in-memory mesh, real framed TCP, and Plumtree-style epidemic dissemination.
    Wire v2 rejects legacy unsigned orders, bounds decoded identity fields, and
    caps decoded certificate votes at the committee limit.
  - `cargo test --all-targets` → **89 green, 0 ignored**. OOPTDD executes **10/10**
    independently bound gates: duplicate authority-key and invalid-genesis config
    rejection plus live-TCP forge-first rejection/zero votes, signed overspend
    rejection, same-slot recovery, certification, four-ledger convergence,
    double-spend rejection, and skipped-sequence rejection.
  - Machine-readable boundary/FSM contract: `333-transfer/OWNER_AUTH_ENGINE_SPEC.json`.
    Longinus local hash/line baseline: `333-transfer/LONGINUS_REFERENCE_SITES.json`
    (`LOCAL_EXTRACTED_KG_UNVERIFIED` while Neo4j is unreachable).
  - Depends only on `333-crdt` (`crdt333`, standalone).
  - **Promotion boundary:** the kernel and multi-process harness are verified, but
    the node is not production-ready until authority state has WAL/snapshot restart
    recovery and TCP ingress has authenticated admission plus bounded peers/readers/
    inboxes. Human-readable aliases also still need self-certifying identities or an
    explicit registration/key-rotation protocol.

KG: `SA_333_Platform`, `consensus-prom16-333-no-blockchain-2026-07-12`,
`vp-prom16-333-coin-transferable-vs-local-credit-2026-07-12` (verdict: local-credit
first, gated flip to transferable once Ed25519 + transport land — both are now
implemented on the owner-authenticated certified rail),
`vp-prom16-333-coin-premint-vs-runtime-issuance-2026-07-12` (verdict: PREMINT),
`buildprogress-333-consensusless-transfer-2026-07-13`.

## Layout

Each `333-<name>/` is a standalone crate. Build/test one directly:

```
cd 333-transfer && cargo test
```

Other crates are as-recovered; their build state is not exhaustively verified here —
`transfer333` + `crdt333` are the verified-green core.
