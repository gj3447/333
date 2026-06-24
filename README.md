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
| `crates/replay` | Lane C deterministic replay (rollback netcode), LTDD-verified: the **determinism law** — same inputs replay to the same state; a run-dependent step diverges and is caught (`absent` forbids `replay_diverged`). Production adopts **ggrs** | **receipt #6 green** |
| `crates/consensus` | Lane A owned-object consistency (Sui-Lutris / FastPay **fast path**, single-writer — *not* the shared-object/consensus path), LTDD-verified: the **no-double-spend** safety law — an object version finalizes *exactly once* (`spend_finalized == 1`); an equivocating spend is rejected, and a double-finalize turns the count check RED. Production adopts a Sui-Lutris/FastPay engine | **receipt #7 green** |
| `crates/billing` | **credits as owned objects** — composes Lane A + metering: a relay debit is a finalized owned-object spend (replay-safe, no double-debit) *and* equals the metered cost (conservation). The combined `ooptdd` gate (conservation `invariant` + finalized + no-replay) is GREEN at `queryable_causal`; a free-riding forward or a replayed debit turns it RED | **receipt #8 green** |

## Verification (LTDD)

The PROM receipts already are LTDD: e.g. the discovery receipt asserts a **real DHT put+get**
across two independent clients (`publisher ≠ resolver`), "not a serialize-then-read" — the
side-effect actually happened, not a return value. `crates/ltdd` makes that a reusable
primitive: a `Store` you `ship` events to and `query` back, and `verify_present(...) ->
Present | Absent | Inconclusive` (`Inconclusive` = store unreachable, **never** a hard fail).

It also **bridges out of Rust**. `Event::to_ooptdd_json` emits the exact envelope the
[`ooptdd`](../ooptdd) (Python) reference verifier reads, so a trace this Rust substrate emitted
can be judged by a gate in a *different language and process* (generator ≠ verifier).

Two distinct verdicts, honestly: the **cargo receipts** gate the build with Rust-native
`verify_present()` / value-conservation asserts (stricter — exact `==` counts and balances). The
**`ooptdd` gates** (`verify/*.yaml`) are the *independent cross-language* check, and they are
**asserted, not just demonstrated**: `verify/run_gates.sh` runs every gate over a real Rust-emitted
trace (must be GREEN, exit 0) **and** over an injected adversary (must be RED, exit 1), so the
forbid/`invariant` gates are proven to fire — not merely shipped against their own green input.

```sh
sh verify/run_gates.sh   # emits every trace (Docker) → judges with ooptdd → asserts GREEN green + RED red
```

Honest scope: `run_gates.sh` is the cross-language gate suite (needs Docker + the `ooptdd`
package); it is not wired into `cargo test` (that container has no Python). The conservation/
correspondence invariants also guard against a future refactor that decouples the emits — on the
current happy path a correct call can't violate them, which is exactly why the RED suite (a
free-riding forward, a double-finalize, a desynced replica) is what proves the gate discriminates.

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
- [x] receipt #6 — Lane C deterministic replay: determinism law, LTDD-verified (run-dependent step caught)
- [x] receipt #7 — Lane A owned-object: no-double-spend safety, LTDD-verified (`spend_finalized == 1`)
- [x] receipt #8 — credits-as-owned-objects billing: composes Lane A + metering (conservation + replay-safe debit)
- [x] `did:key` (W3C) interop decision — **DID stays PeerId-canonical; `did:key:z6Mk…` is the export/interop form** (same Ed25519 key, round-trip KAT in `crates/identity`); a secp256k1 `did:key` is rejected (curve-trap)
- [x] cross-language gate suite — `verify/run_gates.sh` asserts every `ooptdd` gate GREEN over a real trace **and** RED over an injected adversary (the forbid/`invariant` gates are proven to fire)
- [x] Lane B — **yrs** (Yjs/Rust) adopted behind the `crates/crdt` convergence receipt; the receipt holds against the real CRDT (concurrent edits converge byte-identically, judged by the same `ooptdd` gate)
- [ ] step 1 — browser ephemeral-client reachability via Circuit-Relay-v2 / TURN
- [~] step 2 — metering is now wired behind a `Transport` trait + an in-process `Loopback` (end-to-end byte→credit, conservation holds); the real **libp2p Circuit-Relay-v2** transport + OpenMeter/Stripe wiring remain (the relay needs a public reservation address, out of the sandbox)
