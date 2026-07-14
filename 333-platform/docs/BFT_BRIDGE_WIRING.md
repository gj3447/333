# BFT Bridge Wiring

<!-- # KG: seed-post-rts-bft-bridge-2026-04-15 -->

## Overview

`src/netcode/bft_bridge.rs` wires the `BftCheckpointProvider` trait (defined in
`two_layer.rs`) to the real `HotStuffState` in `src/bft/state.rs`.

```
SlowBftCheckpoint
  └── BftCheckpointProvider::propose_checkpoint(handle)
       └── HotStuffCheckpointProvider
            ├── encode handle → OrderedTx::RankedAction (payload = frame_n || tactical_hash)
            ├── HotStuffState::submit_tx(tx)
            ├── drive_to_commit() — message-queue simulation of 3-phase HotStuff
            └── return high_qc (QuorumCert) on success
```

## Connection Points

| Bridge method | BFT API called | File |
|---|---|---|
| `propose_checkpoint` | `HotStuffState::submit_tx` | `src/bft/state.rs:108` |
| `propose_checkpoint` | `HotStuffState::propose` | `src/bft/state.rs:117` |
| `propose_checkpoint` | `HotStuffState::process` (loop) | `src/bft/state.rs:149` |
| `verify_checkpoint_qc` | `crypto::verify(sig, block_hash, keyring)` | `src/bft/crypto.rs:153` |

## Consensus Encoding

Each `CheckpointHandle` is serialized as a `RankedAction` transaction:

```
action_type = 0xC0DE_1000   (checkpoint magic)
payload     = frame_n (8 bytes LE) || tactical_hash (32 bytes)
```

`RankedAction` does not modify token balances (see `executor.rs`) so it is
safe to inject into any view.

## Current Limitations (single-node simulation)

The `drive_to_commit` method simulates a complete 3-phase HotStuff pipeline
using a local FIFO message queue.  This works correctly for **n=1** (single
super-peer, quorum=1) and covers all unit tests.  For multi-node deployments,
remote `Vote` messages from other validators must be fed into the queue before
`drive_to_commit` can advance past Prepare phase.

## API Mismatch Findings

Two gaps were discovered in `HotStuffState` post-commit cleanup that are not
observable in multi-node tests (because a real `NewView` round resets state):

**Gap A — `vote_tracker` not cleared after Commit:**
`on_vote(Phase::Commit)` does not call `vote_tracker.clear()`, but
`on_view_change` and `on_new_view` do.  Without a real `NewView` round the
equivocation tracker retains stale `(signer, phase) → block_hash` entries,
blocking votes for the next block.

*Bridge workaround:* `state.vote_tracker.clear()` called in bridge after
receiving `ProcessResult::Committed`.

*Recommended fix in state.rs:* add `self.vote_tracker.clear()` at the end of
the `Phase::Commit` branch in `on_vote`.

**Gap B — `locked_qc.block_hash` set to current block during PreCommit:**
`on_vote(Phase::PreCommit)` sets `locked_qc = qc` where `qc.block_hash =
current_block_hash`.  When the same block is later proposed at `Phase::Commit`,
`on_proposal` checks `block.parent_hash != locked_qc.block_hash` and rejects
it (parent is the previous committed block, locked_qc points to the current
block).

*Bridge workaround:* when a `Broadcast(Proposal{Commit})` is dequeued, the
bridge resets `state.locked_qc = block.justify` so the safety check
`justify.view < locked_qc.view` evaluates to false.

*Recommended fix in state.rs:* the safety check should compare
`block.justify.view < locked_qc.view && !block_extends_locked(block, locked_qc)`
where `block_extends_locked` traverses the block's ancestor chain.

## TODO List

1. `// TODO: wire transport::broadcast(proposal_msg) to remote validators`
   — `drive_to_commit` currently self-delivers all messages; real multi-node
   requires sending `Proposal` to all validator peers via the signaling
   transport layer.

2. `// TODO: wire transport::send_to_leader(vote) for multi-peer routing`
   — `Vote` messages must be delivered to the current leader's peer connection.

3. `// TODO: feed remote Vote messages into pending_msgs before calling drive`
   — In async context, votes arrive via WebRTC data channel; they must be
   collected and injected into the message queue.

4. `// TODO: wire register_peer_key() for each remote validator during handshake`
   — `verify_checkpoint_qc` calls `crypto::verify(sig, hash, keyring)` which
   requires remote validators' public keys to be registered.  Key exchange
   should happen during the peer handshake in `wasm.rs`.

5. `// TODO: verify quorum count against ValidatorSet (has_quorum)`
   — `verify_checkpoint_qc` currently verifies each individual signature but
   does not check that the number of unique signers meets `2f+1`.  Add:
   `qc.has_quorum(state.validators.n())`.

6. `// TODO: implement view-change timeout for multi-peer`
   — `drive_to_commit` handles `ProcessResult::ViewChange` by advancing view
   and re-proposing, but a real timeout timer (per `viewchange.rs`) must be
   wired for production.

7. `// TODO: wire HotStuffState::submit_tx quota`
   — `submit_tx` returns `bool` (false when pool is full at 10K).  The bridge
   propagates this as `Err("tx pool full")`.  Consider pre-draining the pool
   before submitting a checkpoint tx.

8. Fix **Gap A** and **Gap B** in `state.rs` (see above) so the bridge does
   not need post-commit state surgery.
