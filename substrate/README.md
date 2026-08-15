# p333 — honest-333 v2 substrate

A test-first (PROM) build of the **honest** 333 v2 platform: one shared P2P
substrate carrying per-data-class consistency lanes. The original "browser IS the
server / serverless decentralized OS / DIY decentralized coin" framing was killed
by adversarial analysis; this builds the version that actually closes
(see KG `research-333-oss-survey-promv2`, `plan-333-treatment`).

## Method (PROM)
Test-first, **no fake green**, adversarial self-check, atomic conventional commits.
Every block lands as: write the failing receipt → make it green → adversarially
probe → commit. Receipts are real `cargo test` runs.

## Toolchain
Native Rust builds are supported when a C linker is available. The reproducible
receipt route uses the committed lockfile:

```sh
cargo test --locked
```

On a host without the native toolchain, run the same locked build in Docker:

```sh
docker run --rm \
  -v "$PWD":/work -v p333_cargo:/usr/local/cargo/registry -v p333_target:/work/target \
  -w /work rust:1-slim cargo test --locked
```

## Crates / build blocks
| crate | block | status |
|-------|-------|--------|
| `crates/identity` | substrate identity — Ed25519 (RFC 8032) DID == libp2p PeerId | **receipt #1 green** |
| `crates/discovery` | substrate discovery — PKARR announce/resolve Super-Peer location by DID | **receipt #2 green** |
| `crates/ltdd` | substrate **verification** — Rust-native LTDD: assert the trace the substrate *emitted* (read back from a store), not a return value; 3-valued (Present/Absent/Inconclusive); transparent JSONL diagnostics | **receipt #3 green** |
| `crates/metering` | relay metering → credit-bucket (step 2), LTDD-verified: metered bytes are *actually* debited and the bucket gates when empty; the **credit conservation invariant** catches a free-riding relay | **receipt #4 green** |
| `crates/crdt` | consistency lane (Lane B), LTDD-verified: replicas that exchange state must **converge**; a replica that missed a sync is surfaced as `replica_diverged` (the `absent`/forbid check turns RED). The same receipt runs against a minimal G-Counter **and the real `yrs` CRDT** (concurrent edits converge byte-identically) | **receipt #5 green** |
| `crates/replay` | Lane C deterministic replay (rollback netcode), LTDD-verified: the **determinism law** — same inputs replay to the same state; a run-dependent step diverges and is caught (`absent` forbids `replay_diverged`). Production adopts **ggrs** | **receipt #6 green** |
| `crates/consensus` | Lane A owned-object consistency (Sui-Lutris / FastPay **fast path**, single-writer — *not* the shared-object/consensus path), LTDD-verified: the **no-double-spend** safety law — an object version finalizes *exactly once* (`spend_finalized == 1`); an equivocating spend is rejected, and a double-finalize turns the count check RED. Production adopts a Sui-Lutris/FastPay engine | **receipt #7 green** |
| `crates/billing` | **credits as owned objects** — composes Lane A + metering: a relay debit is a finalized owned-object spend (replay-safe, no double-debit) *and* equals the metered cost (conservation). Native tests inject free-riding and replay attempts and assert the measured state directly | **receipt #8 green** |
| `crates/relay-billing` | **real-relay byte metering** — forwards a request-response payload over an *actual* Circuit-Relay-v2 connection (server = `crates/relay`) and meters + bills it end-to-end; the relayed delivery is proven by the ack, then the bytes become a finalized owned-object debit. Closes the metering↔relay wiring on the **real relay**, not the in-process `Loopback` | **green** |
| `crates/wal` | **durability substrate (P0-1)** — write-ahead log absorbed as *design invariants* from etcd `server/storage/wal` @6006f405 (rolling crc32c chain, torn-tail zero-sector repair, tmpdir-rename create, crc-handoff cuts, poisoned-handle fsyncgate). `sync()` → `DurableReceipt` is the ack boundary: nothing externalizes before the receipt. LTDD-verified: abort-crash recovery keeps the receipted prefix intact; a byte flip in the synced region turns reopen RED (`CrcMismatch`/`CorruptFrame`, never auto-repaired) | **green** |

## Verification (LTDD)

The PROM receipts already are LTDD: e.g. the discovery receipt asserts a **real DHT put+get**
across two independent clients (`publisher ≠ resolver`), "not a serialize-then-read" — the
side-effect actually happened, not a return value. `crates/ltdd` makes that a reusable
primitive: a `Store` you `ship` events to and `query` back, and `verify_present(...) ->
Present | Absent | Inconclusive` (`Inconclusive` = store unreachable, **never** a hard fail).

`Event::to_trace_json` and `to_trace_jsonl` expose transparent diagnostics, but
they do not delegate authority to another rule engine. `cargo test --locked`
executes the native arrival, conservation, replay, convergence, durability, and
fault-injection assertions. A failed or unreachable producer therefore remains
a test failure or an explicit inconclusive state at the code boundary.

This is the path for the distributed/metering blocks (per the OSS survey: relay metering →
OpenMeter → a credit-bucket gate): "did the relay *actually* meter N bytes and did the bucket
*actually* debit?" is an arrival question. Production swaps the JSONL file for OTLP → a store;
the gate is unchanged. **Crypto correctness stays a log-free zone** — Ed25519/RFC-8032 and
DID==PeerId are asserted directly against test vectors (receipts #1/#2), never via trace arrival.

### Hard core
- Identity is **Ed25519 (RFC 8032)**, never secp256k1/BIP-340. The curve-trap guard
  lives at the dependency-feature level: `libp2p-identity` is built with `ed25519`
  only (secp256k1 feature deliberately not enabled).
- A 333 DID is the canonical libp2p **PeerId** (base58btc; Ed25519 → `12D3KooW…`).

## Done / next (PROM step 0)
- [x] receipt #1 — Ed25519 RFC-8032 conformance + DID==PeerId derivation + round-trip
- [x] receipt #2 — PKARR publish/resolve over a local DHT testnet (cross-client put+get)
- [x] receipt #3 — Rust-native LTDD verification primitive + transparent JSONL diagnostics
- [x] receipt #4 — credit-bucket accounting + gate, LTDD-verified (conservation invariant catches free-riding)
- [x] receipt #5 — Lane B consistency: CRDT convergence law, LTDD-verified (`absent` forbids `replica_diverged`)
- [x] receipt #6 — Lane C deterministic replay: determinism law, LTDD-verified (run-dependent step caught)
- [x] receipt #7 — Lane A owned-object: no-double-spend safety, LTDD-verified (`spend_finalized == 1`)
- [x] receipt #8 — credits-as-owned-objects billing: composes Lane A + metering (conservation + replay-safe debit)
- [x] `did:key` (W3C) interop decision — **DID stays PeerId-canonical; `did:key:z6Mk…` is the export/interop form** (same Ed25519 key, round-trip KAT in `crates/identity`); a secp256k1 `did:key` is rejected (curve-trap)
- [x] Lane B — **yrs** (Yjs/Rust) adopted behind the `crates/crdt` convergence tests; concurrent edits converge byte-identically under direct measurement
- [ ] step 1 — browser ephemeral-client reachability via Circuit-Relay-v2 / TURN
- [x] step 2 (substrate side) — metering→credit billing wired to the **real Circuit-Relay-v2**: `crates/relay-billing` forwards a payload over an actual relayed connection and meters + bills it end-to-end, LTDD-verified (the in-process `Loopback` stays as the deterministic fixture). Remaining: the OpenMeter/Stripe billing backend (out of repo)
