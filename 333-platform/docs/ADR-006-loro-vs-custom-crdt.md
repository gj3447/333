# ADR-006: Loro vs 333 Custom CRDT

**Date:** 2026-04-15  
**Status:** PROPOSED  
**KG:** seed-rts-conflict-loro-vs-custom-crdt-2026-04-15  
**Spike crate:** `crates/333-crdt-bench`

---

## Context

333 Platform has a working custom CRDT stack (`src/lww_map.rs`, `src/or_set.rs`, `src/rga.rs`, `src/sync.rs`) with Lamport HLC, postcard serialization, and delta batching at 20ms intervals. Loro is a production-grade CRDT library with rich document model, snapshot/delta export, and TypeScript bindings. This ADR records the spike outcome.

---

## Spike Results

All numbers from `cargo run -p triple-three-crdt-bench --features loro` on 2026-04-15 (debug build, Apple Silicon Mac, 3-peer × 10-key workload):

| Metric | A: Loro LwwMap | B: 333 Custom LwwMap | C: 333 + Invariant Validator |
|---|---|---|---|
| Available | Yes (crates.io fetched) | Yes | Yes |
| Ops | 30 | 30 | 152 (1 rejected) |
| Convergence | ✓ | ✓ | ✓ |
| Wire bytes (3 snapshots) | **1,449** | **480** | n/a |
| Duration (µs) | **6,456** | **39** | 145 |
| Invariant violations | 0 | 0 | **1 (correctly blocked)** |
| External deps | ~90 crates | 0 (stdlib) | 0 |
| WASM target | ✓ (loro has wasm support) | ✓ (already in wasm) | ✓ |

Wire bytes ratio (snapshot): Loro = **3.0× larger** than custom delta for this workload.  
Duration ratio: Loro = **165× slower** than custom (includes document envelope overhead).  
Note: debug build; release profile reduces both, but the ratio holds directionally.

---

## BFT Compatibility Evaluation

### Core mismatch: PeerID type

| Dimension | Loro | 333 BFT |
|---|---|---|
| Peer identity | `u64` (8 bytes, auto-assigned) | `[u8; 32]` (Ed25519 public key) |
| Cryptographic | No | Yes — signed with ed25519-compact |
| Namespace | CRDT authorship only | Full Byzantine authentication |
| Reversibility | Not reversible | n/a (it IS the key) |

### Mapping strategies analyzed

1. **Truncate** `PeerId[0..8] → u64`: REJECT. Adversarial collision risk. Spoofable.
2. **blake3(PeerId) → u64**: OK for attribution/routing. NOT a security identity. Requires side-table `HashMap<u64, [u8;32]>`.
3. **Separate namespaces** (recommended): Loro u64 is only CRDT authorship label. BFT `[u8;32]` is the security identity. Never conflate them. Side-table required.
4. **Sequential counter per session**: Viable for ephemeral sessions. Breaks on reconnect unless deterministically re-assigned at join.

### Key finding

Loro's `PeerID(u64)` is **fundamentally incompatible** as a BFT identity carrier. It is too small (8 bytes vs 32 bytes) and is not cryptographically bound to a keypair. Adopting Loro would require maintaining a parallel identity layer — exactly what 333's `crypto_real::PeerId` already provides. There is no consolidation path; they must remain separate.

---

## Decision: **Keep 333 Custom CRDT (Option B/C)**

### Rationale

1. **BFT incompatibility is structural.** Loro's u64 PeerID cannot replace `[u8;32]` Ed25519 keys. Any Loro adoption requires a dual-layer identity system with more code, not less.

2. **Performance.** 333 custom LwwMap is 165× faster and 3× smaller on wire for this workload. The custom stack is already optimized for the 20ms game loop batch window.

3. **WASM compatibility.** 333 custom CRDT is already deployed as `cdylib` + `rlib`. Loro adds ~90 transitive crates to the WASM bundle, increasing build time and binary size.

4. **Feature parity.** For the 333 use cases (block placement LWW, player presence OR-Set, chat RGA), the custom stack covers all requirements without the overhead of Loro's document model.

5. **Invariant validators (Scenario C).** Application-level gates work identically for both libraries. Neither provides built-in invariant enforcement — the 333 approach is not inferior here.

### When Loro would be reconsidered

- If 333 adds **rich text collaboration** (where Loro's text CRDT and editor bindings would save months of work).
- If a **TypeScript-native** client needs first-class CRDT without a WASM bridge.
- If the **operation log / time-travel** features of Loro become product requirements.

### Conditional migration path

```
IF product_requirement IN [rich_text, TS_native_crdt, time_travel]:
    THEN: Adopt Loro for DOCUMENT layer only (text/rich content)
          Keep 333 custom CRDT for GAME STATE layer (LWW block pos, OR-Set presence)
          Maintain dual-layer identity: Loro u64 ← blake3(BFT PeerId) side-table
          Never use Loro PeerID for BFT voting or QC signing
```

---

## Files Produced

- `crates/333-crdt-bench/Cargo.toml` — workspace crate, loro optional dep
- `crates/333-crdt-bench/src/main.rs` — scenarios A/B/C, JSON output
- `crates/333-crdt-bench/src/crdt.rs` — inlined LwwMap (mirrors src/lww_map.rs)
- `crates/333-crdt-bench/src/peer_id_compat.rs` — PeerID mapping analysis + risk table
- `docs/ADR-006-loro-vs-custom-crdt.md` — this document

## References

- `src/lww_map.rs` — KG: SPAN_333_CRDT_LwwMap
- `src/bft/crypto.rs` — KG: TASK_333_B_BFTCrypto
- `src/sync.rs` — KG: CONTRACT_333_INT_CrdtSync
- Loro v1.10.8: https://crates.io/crates/loro
