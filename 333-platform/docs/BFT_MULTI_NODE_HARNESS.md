# BFT Multi-Node Harness

<!-- # KG: prod-wiring-bft-multi-node-2026-04-15 -->

## Overview

`src/netcode/bft_multi_node.rs` provides `MultiNodeBftHarness` — an in-process
N-node HotStuff simulation that drives real `HotStuffState` machines with
`std::sync::mpsc` channels as the mock transport layer.

This is the next layer above `bft_bridge.rs` (which covers single-node n=1).

## Architecture

```
MultiNodeBftHarness
  ├─ slots: Vec<NodeSlot>           one slot per validator
  │    ├─ state: Arc<RwLock<HotStuffState>>
  │    ├─ tx: mpsc::SyncSender<HotStuffMsg>  → inbox for this node
  │    ├─ rx: mpsc::Receiver<HotStuffMsg>    ← outbox processed by drive loop
  │    └─ byzantine: bool                    true = silently drop all msgs
  └─ drive_checkpoint(frame, hash)
       1. find_leader_idx() — query is_leader() on all states
       2. leader.submit_tx(RankedAction{frame_n || tactical_hash})
       3. leader.propose() → initial Proposal
       4. broadcast Proposal to all non-leader inboxes + leader self-inbox
       5. message pump loop:
            for each node: drain inbox → state.process(msg)
              Committed   → sync_views_to_leader() → return high_qc
              SendToLeader→ route vote to leader inbox
              Broadcast   → apply locked_qc latch → deliver to all inboxes
       6. Err(QuorumTimeout) if max_steps exceeded
```

## Transport Abstraction

The harness uses two internal helpers that map 1:1 to real network operations:

| Helper | Mock (in-process) | Real network hook |
|---|---|---|
| `broadcast_to_all(msg)` | `mpsc::try_send` to all slots | `transport::broadcast(msg, peers)` |
| `deliver_to(idx, msg)` | `mpsc::try_send` to slot[idx] | `transport::send_to(peer_id, msg)` |

To wire real networking: replace these helpers with WebRTC data-channel sends
(see `src/p2p/super_peer.rs`) or QUIC streams.

## Keyring Exchange

`register_all_peer_identities()` calls `HotStuffState::register_peer_identity()`
for every other node.  This is the in-process simulation of the handshake.

**In production** the equivalent is:

```rust
// During WebRTC offer/answer handshake:
state.register_peer_key(remote_node_id, remote_peer.peer_id());
// OR during headscale VPN join:
state.register_peer_identity(remote_node_id, exchanged_identity);
```

`ValidatorKeyring` distinguishes local keys (can sign) from remote peer-ids
(can verify only).  Production should use `register_peer_key` (public key only),
not `register_peer_identity` (which stores the full private key).

## Leader Rotation

Leader is determined by `leader_for_view(view, &validators)` — simple
round-robin: `validators[view % n]`.  `find_leader_idx()` queries `is_leader()`
on each `HotStuffState` to find the current leader rather than tracking view
externally.  `sync_views_to_leader()` broadcasts a `NewView` after each commit
so all non-leader nodes advance their view for the next round.

## Byzantine Fault Model

`set_byzantine(idx)` marks a node as faulty: it receives messages but silently
drops them (does not vote).  This models a **crash-fault** or **silent
Byzantine** node.  The harness does not model equivocation (voting for two
different blocks in the same phase) — that path is tested at the `HotStuffState`
level via `equivocation_detection_4validator` in `bft/state.rs` tests.

## API Gap Findings (new, not in BFT_BRIDGE_WIRING.md)

**Gap C — Non-leader nodes never see `Committed` result:**
`on_vote(Commit)` returns `ProcessResult::Committed` only on the leader node
that aggregated the quorum.  Non-leader nodes process the Commit `Proposal`
and return `SendToLeader(Vote)`, never `Committed`.  Their view only advances
when they receive a `NewView` message.  Without `sync_views_to_leader()`, the
second checkpoint would be driven by the wrong leader (wrong view on non-leader
nodes means `is_leader()` returns false for the correct leader).

*Harness workaround:* `sync_views_to_leader()` after every `Committed` result.

*Recommended fix in state.rs:* when a non-leader node receives the Commit-phase
QC (e.g. via `NewView`), it should advance view automatically.

**Gap D — `mpsc::SyncSender` can silently drop messages if channel is full:**
The harness uses `try_send` (non-blocking) with a 256-message buffer.  Under
real workload with many rapid checkpoints, slow nodes could fall behind and lose
messages.  In production, use backpressure (`send` with timeout) or unbounded
channels with explicit flow control.

## Tests

| Test | Scenario | Expected |
|---|---|---|
| `four_nodes_quorum_three_checkpoint_succeeds` | n=4, q=3, all honest | Ok(QC) |
| `one_node_byzantine_still_reaches_quorum` | n=4, q=3, idx=3 Byzantine | Ok(QC) |
| `two_nodes_byzantine_fails_quorum` | n=4, q=3, idx=2,3 Byzantine | Err(QuorumTimeout) |
| `keyring_exchange_registers_all_peers` | n=5, check keyring counts | each node has n entries |
| `consecutive_checkpoints_with_leader_rotation` | n=4, 3 consecutive checkpoints | all Ok, round advances |

## Files

| File | Role |
|---|---|
| `src/netcode/bft_multi_node.rs` | Harness implementation + tests |
| `src/netcode/bft_bridge.rs` | Single-node bridge (unchanged) |
| `src/bft/state.rs` | `HotStuffState` core (not modified) |
| `src/bft/crypto.rs` | `ValidatorKeyring`, `register_peer_identity` (not modified) |

## KG References

- `# KG: prod-wiring-bft-multi-node-2026-04-15` — this module
- `# KG: seed-post-rts-bft-bridge-2026-04-15` — single-node bridge (Gap A, B)
- `# KG: lesson-333-bft-keyring-exchange-2026-04-14` — keyring exchange design
- `# KG: lesson-333-bft-multivalidator-stress-2026-04-14` — 4-validator stress tests
