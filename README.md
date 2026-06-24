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
This box has no local C linker, so receipts run in Docker:

```sh
docker run --rm \
  -v "$PWD":/work -v p333_cargo:/usr/local/cargo/registry -v p333_target:/work/target \
  -w /work rust:1-slim cargo test
```

## Crates / build blocks
| crate | block | status |
|-------|-------|--------|
| `crates/identity` | substrate identity — Ed25519 (RFC 8032) DID == libp2p PeerId | **receipt #1 green** |
| `crates/discovery` | substrate discovery — PKARR announce/resolve Super-Peer location by DID | **receipt #2 green** |
| `crates/ltdd` | substrate **verification** — Rust-native LTDD: assert the trace the substrate *emitted* (read back from a store), not a return value; 3-valued (Present/Absent/Inconclusive); JSONL bridge to `ooptdd` | **receipt #3 green** |
| `crates/metering` | relay metering → credit-bucket (step 2), LTDD-verified: metered bytes are *actually* debited and the bucket gates when empty; the **credit conservation invariant** catches a free-riding relay | **receipt #4 green** |
| `crates/crdt` | consistency lane (Lane B), LTDD-verified: replicas that exchange state must **converge**; a replica that missed a sync is surfaced as `replica_diverged` (the `absent`/forbid check turns RED). The same receipt runs against a minimal G-Counter **and the real `yrs` CRDT** (concurrent edits converge byte-identically) | **receipt #5 green** |

## Verification (LTDD)

The PROM receipts already are LTDD: e.g. the discovery receipt asserts a **real DHT put+get**
across two independent clients (`publisher ≠ resolver`), "not a serialize-then-read" — the
side-effect actually happened, not a return value. `crates/ltdd` makes that a reusable
primitive: a `Store` you `ship` events to and `query` back, and `verify_present(...) ->
Present | Absent | Inconclusive` (`Inconclusive` = store unreachable, **never** a hard fail).

It also **bridges out of Rust**. `Event::to_ooptdd_json` emits the exact envelope the
[`ooptdd`](../ooptdd) (Python) reference verifier reads, so a trace this Rust substrate emitted
can be judged by a gate in a *different language and process* — the strongest
generator-≠-verifier separation a verdict can have:

```sh
cargo run -p p333-ltdd --example emit_trace > verify/trace.jsonl   # Rust substrate emits
python verify/ooptdd_verify.py verify/trace.jsonl verify/superpeer.yaml  # Python ooptdd judges (GREEN/RED)
```

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
- [x] receipt #3 — Rust-native LTDD verification primitive + cross-language `ooptdd` bridge
- [x] receipt #4 — credit-bucket accounting + gate, LTDD-verified (conservation invariant catches free-riding)
- [x] receipt #5 — Lane B consistency: CRDT convergence law, LTDD-verified (`absent` forbids `replica_diverged`)
- [ ] `did:key` (W3C) interop decision (currently DID := PeerId base58)
- [x] Lane B — **yrs** (Yjs/Rust) adopted behind the `crates/crdt` convergence receipt; the receipt holds against the real CRDT (concurrent edits converge byte-identically, judged by the same `ooptdd` gate)
- [ ] step 1 — browser ephemeral-client reachability via Circuit-Relay-v2 / TURN
- [ ] step 2 — coturn relay *transport* + OpenMeter/Stripe wiring (the metering→credit **accounting + gate** is done in `crates/metering`/receipt #4; transport integration remains)
