# 333 — consensusless P2P OS (Rust)

A single-owner, consensus-number-1 asset-transfer substrate and its surrounding
P2P OS crates, built PROM-style (test-first, no fake green, adversarial self-check).
Lineage: FastPay (Baudet et al., Meta 2020), Zef (2022), Sui Lutris / Mysticeti
fast path (2024), Linera single-user microchains (2025), and the consensus-number-1
asset-transfer theorem (Guerraoui et al., PODC 2019).

> Separate from `gj3447/p333` (the delltower-lineage `identity`/`crdt` crates). This
> repo is the `SERVER/07_PROJECTS/333-*` line — crates named `transfer333`,
> `crdt333`, `identity333`, … — 30 crates linked by relative path deps (kept as
> sibling directories; no forced top-level workspace so a WIP crate never blocks
> the others).

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
  - `cargo test` → **24 green** (21 lib incl. `authority` module + 3 e2e integration),
    incl. `forged_vote_with_wrong_key_is_rejected` and
    `vote_signature_does_not_transfer_to_a_different_transfer`.
  - Depends only on `333-crdt` (`crdt333`, standalone).
  - Follow-up: gossip/broadcast **transport** (this layer is in-memory, unit-testable).

KG: `SA_333_Platform`, `consensus-prom16-333-no-blockchain-2026-07-12`,
`vp-prom16-333-coin-transferable-vs-local-credit-2026-07-12` (verdict: local-credit
first, gated flip to transferable once Ed25519 + transport land — Ed25519 now done),
`vp-prom16-333-coin-premint-vs-runtime-issuance-2026-07-12` (verdict: PREMINT),
`buildprogress-333-consensusless-transfer-2026-07-13`.

## Layout

Each `333-<name>/` is a standalone crate. Build/test one directly:

```
cd 333-transfer && cargo test
```

Other crates are as-recovered; their build state is not exhaustively verified here —
`transfer333` + `crdt333` are the verified-green core.
