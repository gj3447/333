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

### Hard core
- Identity is **Ed25519 (RFC 8032)**, never secp256k1/BIP-340. The curve-trap guard
  lives at the dependency-feature level: `libp2p-identity` is built with `ed25519`
  only (secp256k1 feature deliberately not enabled).
- A 333 DID is the canonical libp2p **PeerId** (base58btc; Ed25519 → `12D3KooW…`).

## Done / next (PROM step 0)
- [x] receipt #1 — Ed25519 RFC-8032 conformance + DID==PeerId derivation + round-trip
- [x] receipt #2 — PKARR publish/resolve over a local DHT testnet (cross-client put+get)
- [ ] `did:key` (W3C) interop decision (currently DID := PeerId base58)
- [ ] step 1 — browser ephemeral-client reachability via Circuit-Relay-v2 / TURN
- [ ] step 2 — coturn relay metering → Credit bucket gate
