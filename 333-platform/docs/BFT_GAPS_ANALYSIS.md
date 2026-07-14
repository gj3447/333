# BFT Gap Analysis & Fix Report
# KG: sprint4A-gap-ABCD-fix-2026-04-15

Sprint 4A — HotStuff Gap A/B/C/D root fixes in `bft/state.rs`.
Date: 2026-04-15

---

## Summary

Four gaps were identified in the HotStuff BFT implementation where `bft_bridge.rs`
and `bft_multi_node.rs` applied manual workarounds instead of the state machine
handling them correctly.  All four gaps are now fixed in `src/bft/state.rs`
with workaround code removed from the bridge layer.

**Final test result: 314 lib + 4 integration + 4 e2e + 1 doc = 323 total, 0 failures.**

---

## Gap A — vote_tracker not cleared after commit

### Problem
`on_vote(Phase::Commit)` returned `ProcessResult::Committed` without clearing
`vote_tracker`.  On the next round, when the same leader re-collected votes for
new blocks, the equivocation detector found stale `(signer, phase) → block_hash`
entries and rejected legitimate votes as equivocations.

### Workaround (removed)
`bft_bridge.rs` and `bft_multi_node.rs` both called `state.vote_tracker.clear()`
manually after receiving `ProcessResult::Committed`.

### Fix (`src/bft/state.rs`)
`on_vote(Phase::Commit)` now calls `self.vote_tracker.clear()` before returning
`ProcessResult::Committed`.

### New test
`bft::state::tests::gap_a_vote_tracker_cleared_after_commit` — asserts
`leader.vote_tracker.is_empty()` immediately after commit.

---

## Gap B — locked_qc blocks Commit-phase proposal in subsequent rounds

### Problem
Two sub-issues combined to cause `on_proposal(Phase::Commit)` to reject the
Commit-phase proposal for the same block:

**B1 (leader path):** After `on_vote(Phase::PreCommit)` the leader sets
`locked_qc = precommit_qc` where `precommit_qc.block_hash = current_block`.
Then the Commit proposal is broadcast to ALL nodes (including the leader via
`broadcast_to_all`).  When the leader processes its own Commit proposal via
`on_proposal`, the safety check fires:
- `block.justify.view (old high_qc view) < locked_qc.view (current view)` = true
- `block.parent_hash (previous block) != locked_qc.block_hash (current block)` = true
→ REJECTED.

**B2 (high_qc stale on early-advancing nodes):** The Gap C fix causes validators
to advance their view before receiving the NewView message.  `on_new_view` only
updated `high_qc` when `view > self.view`.  If the node already advanced its
view via the Gap C mechanism, the NewView was a no-op and `high_qc` stayed stale.
The next time that node became leader it would propose on top of a stale chain.

### Workarounds (removed)
`bft_bridge.rs` pre-patched `locked_qc = block.justify` before pushing the
Commit proposal to the queue.  `bft_multi_node.rs` did the same via
`apply_commit_latch()`.

### Fix (`src/bft/state.rs`)
**B1:** Added condition (c) to the safety rule in `on_proposal`:
```
safe = justify.view >= locked_qc.view          // (a) classic liveness
    || parent_hash == locked_qc.block_hash     // (b) extends locked chain
    || block.hash == locked_qc.block_hash      // (c) IS the locked block (Commit)
```
Condition (c) allows a node to vote for the Commit phase of a block it already
locked on in PreCommit — exactly what the Commit phase is for.

**B2:** `on_new_view` now ALWAYS updates `high_qc` if the QC in the message is
newer (`qc.view > self.high_qc.view`), regardless of whether the view number
itself advances.  This ensures nodes that fast-forwarded their view still receive
the correct chain tip.

Additionally, `on_vote(Commit)` resets `self.locked_qc = self.high_qc` after
committing, so the leader's lock is aligned with the committed chain tip before
the next round.

**B3 (validator locked_qc):** Validators now update their `locked_qc` in
`on_proposal(Phase::PreCommit)`, mirroring the HotStuff spec.  This prevents
validators from using stale genesis locks across rounds.

### New test
`bft::state::tests::gap_b_locked_qc_reset_after_commit` — asserts
`locked_qc.view == high_qc.view` and `locked_qc.block_hash == high_qc.block_hash`
after a commit.

---

## Gap C — non-leader nodes don't advance view after commit

### Problem
`ProcessResult::Committed` was returned only from the leader's `on_vote(Phase::Commit)`.
Non-leader validators received the Commit-phase Proposal and voted, but their view
number was never advanced.  On the next round, `find_leader_idx()` (which calls
`is_leader()` = `leader_for_view(self.view, vs) == self.node_id`) could not find the
correct new leader because non-leader views were stale.

### Workaround (still needed, now secondary)
`bft_multi_node.rs::sync_views_to_leader()` broadcast a synthetic NewView to
non-leaders after each commit and drained their inboxes.

### Fix (`src/bft/state.rs`)
When a non-leader validator processes `on_proposal(Phase::Commit)` and returns a
vote, it also advances `self.view += 1`, resets `phase`, clears `pending_block`,
`votes`, and `vote_tracker`.  This is safe because a Commit-phase proposal can
only exist if the leader collected 2f+1 PreCommit votes.

`sync_views_to_leader()` is kept as a secondary mechanism to deliver the
`high_qc` update (via NewView) since the early view advance would otherwise
miss it (see Gap B2 above).

### New test
`bft::state::tests::gap_c_non_leader_view_advances_after_commit_vote` — asserts
all non-leader nodes advance from view 0 → 1 after processing a Commit proposal.

---

## Gap D — mpsc::try_send silently drops messages on full channel

### Problem
`deliver_to`, `broadcast_except`, `broadcast_to_all`, and `sync_views_to_leader`
in `bft_multi_node.rs` used `mpsc::SyncSender::try_send()`.  If the bounded
channel (size 256) was full, messages were silently discarded with `let _ = ...`.
In production this would cause silent vote loss → QuorumTimeout.

### Fix (`src/netcode/bft_multi_node.rs`)
All four delivery helpers and the `sync_views_to_leader` delivery loop now use
blocking `mpsc::SyncSender::send()` instead of `try_send()`.  A full channel
now causes the caller to block (backpressure) rather than dropping messages.
The channel bound of 256 is generous enough that blocking rarely occurs in
practice; if it does, it surfaces as a slowdown rather than a silent failure.

### New test
`netcode::bft_multi_node::tests::gap_d_backpressure_no_silent_drops_under_load`
— drives 8 consecutive checkpoints across 4 nodes (2 full leader-rotation cycles,
n=4, q=3) and asserts all succeed.

---

## Removed workaround code

| File | Removed code | Reason |
|---|---|---|
| `bft_bridge.rs` | `state.vote_tracker.clear()` after Committed | Gap A fixed in state.rs |
| `bft_bridge.rs` | `state.locked_qc = state.high_qc.clone()` after Committed | Gap B fixed in state.rs |
| `bft_bridge.rs` | `locked_qc = block.justify` latch before Commit proposal | Gap B fixed in state.rs |
| `bft_multi_node.rs` | `st.vote_tracker.clear()` after Committed | Gap A fixed in state.rs |
| `bft_multi_node.rs` | `st.locked_qc = st.high_qc.clone()` after Committed | Gap B fixed in state.rs |
| `bft_multi_node.rs` | `apply_commit_latch()` function | Gap B fixed in state.rs |

---

## New gaps discovered during fix (Gap E, F)

None. All four original gaps resolved without discovering new fundamental issues.
The interaction between Gap B2 (high_qc stale) and Gap C (early view advance) was
an emergent complexity but resolved within the same fix set.

---

## Files modified

- `src/bft/state.rs` — Gaps A, B (all sub-fixes), C, plus on_new_view improvement
- `src/netcode/bft_multi_node.rs` — Gap D (blocking send), bridge cleanup
- `src/netcode/bft_bridge.rs` — Gap A/B bridge workaround removal

## Test count delta

- Before: 47 lib tests (bft + netcode)
- After: 51 lib tests (+4 gap-specific tests)
- Workspace total: 323 pass, 0 fail
