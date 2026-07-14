# RTS 3-Tier State Classification Matrix

> # KG: seed-rts-3tier-state-classification-2026-04-15
> Design phase document — integration follows in subsequent work.

---

## Tier Overview

| Tier | Name | Channel | Sync Period | Max Tolerated Latency | Consistency Model |
|------|------|---------|-------------|----------------------|-------------------|
| 1 | **Tactical** | `CH_POSITION` (lockstep) | every turn (~50 ms) | 50 ms | Deterministic lockstep (all peers agree) |
| 2 | **Persistent** | `CH_CRDT` (delta) | 20 ms batch flush | 200 ms | CRDT LwwMap (eventual, last-write-wins) |
| 3 | **Critical** | `CH_BFT` (ordered) | per event | < 1 s (BFT commit) | HotStuff BFT total order |

---

## Tier 1 — Tactical State (Lockstep CH_POSITION, 50 ms)

High-frequency, short-lived data that drives the game simulation. Every peer must hold an identical copy each turn; divergence = desync.

| Field / Event | Type | Sync Trigger | Notes |
|---|---|---|---|
| Unit position `(x, y)` | `f32, f32` | Every `Turn` | Deterministic float arithmetic |
| Unit HP | `u32` | Every `Turn` | Derived from damage events in `Turn.commands` |
| Unit alive/dead status | implicit | `Turn` post-process | Dead units removed after `process_turn` |
| Animation frame index | `u8` | Every `Turn` | Client-side interpolation between turns |
| Active ability cooldown | `u16` (ticks remaining) | Every `Turn` | Decremented deterministically |
| Projectile position | `(f32, f32)` | Every `Turn` | Spawned and removed within lockstep |
| Command queue (pending) | `Vec<Command>` | Sent in `Turn` | Cleared after execution |
| Minerals gathered (delta) | `u32` | In `Turn` via Gather | Accumulated per-turn |
| Terrain visibility mask | `u64` bitmask | Every `Turn` | Fog-of-war derived from unit positions |

**Desync detection**: `Turn.hash` = `RtsGame::state_hash()` over all Tier-1 fields. Mismatch → desync alarm → BFT-escalate or replay.

---

## Tier 2 — Persistent State (CRDT LwwMap CH_CRDT, 20 ms)

Durable, convergent data that outlives individual turns. Conflicts resolved via LWW timestamp; no determinism requirement.

| Field / Event | CRDT Key Pattern | Value Type | Conflict Resolution | Notes |
|---|---|---|---|---|
| Player inventory slot | `inv:{player_id}:{slot}` | JSON item blob | LWW (last write wins) | Items picked up between sessions |
| Alliance status | `alliance:{p1}:{p2}` | `"allied"` / `"neutral"` / `"war"` | LWW | Negotiated out-of-band |
| Chat message | `chat:{hlc_hex}` | message text | Append-only via unique HLC key | HLC key guarantees uniqueness |
| Resource node state | `resource:{x}:{y}` | `"full"` / `"depleted"` | LWW | Updated on `Gather` command |
| Player display name | `player:{id}:name` | `String` | LWW | Set once, rarely changes |
| Building construction progress | `build:{unit_id}:progress` | `u8` percentage | LWW | Visible to all players |
| Custom map annotation | `annotation:{hlc_hex}` | GeoJSON point | LWW | Ping / mark on map |
| Player profile badge | `badge:{player_id}:{badge_id}` | `bool` / timestamp | LWW | Earned achievements |

**Sync path**: `LwwMap::set()` → `LwwDelta` → `SyncManager::on_local_delta()` → `poll_outgoing()` every 20 ms → `CH_CRDT` wire → remote `process_incoming()` → `merge_delta()`.

---

## Tier 3 — Critical Events (BFT HotStuff CH_BFT, ordered)

Irreversible, globally-ordered events. Require f+1 honest quorum agreement. Cannot be LWW-resolved; ordering matters.

| Event | `OrderedTx` Variant | Why BFT Required | Expected Frequency |
|---|---|---|---|
| Victory / defeat declaration | `RankedAction { action_type: 0x01 }` | No two peers should see different outcomes | Once per game |
| Token transfer (reward) | `Transfer { from, to, amount, nonce }` | Double-spend prevention | Per match end |
| Cheat report + adjudication | `RankedAction { action_type: 0x02, payload: desync_proof }` | Tamper-evident log | Rare |
| Replay anchor commit | `RankedAction { action_type: 0x03, payload: turn_hash_chain }` | Canonical game record | Every N turns (configurable) |
| Auction bid finalization | `AuctionBid { bidder, item_id, amount }` | "Who bid first?" race condition | In-game economy |
| Player ban (consensus kick) | `RankedAction { action_type: 0x04, payload: player_id_bytes }` | Requires quorum to prevent griefing | Rare |
| Match result attestation | `RankedAction { action_type: 0x05, payload: final_state_hash }` | On-chain record anchor | Once per game |

**Commit path**: event → `HotStuffMsg::Proposal` → 3-phase (Prepare / PreCommit / Commit) → `ProcessResult::Committed(Vec<OrderedTx>)` → apply.

---

## Tier Boundary Rules

| Situation | Rule |
|---|---|
| Lockstep desync detected | Tier 1 → escalate to Tier 3 via `RankedAction { action_type: 0x02 }` |
| CRDT conflict within same wall_ms | HLC counter + node_id tiebreak (LwwEntry timestamp) |
| BFT committed event contradicts Tier 1 state | BFT is authoritative; Tier 1 must rewind to last valid anchor |
| New peer joins mid-game | Tier 1: replay turn buffer; Tier 2: `FullState` snapshot; Tier 3: replay committed blocks |
| Offline peer reconnects | Tier 2: state vector diff sync; Tier 3: fetch missing blocks from any honest peer |

---

## Channel Summary

```
CH_POSITION  →  lockstep Turn packets, 50 ms cadence, broadcast to all peers
CH_CRDT      →  postcard-encoded SyncPayload (Delta / FullState), 20 ms batch flush
CH_BFT       →  HotStuffMsg (Proposal / Vote / NewView / ViewChange), event-driven
```
