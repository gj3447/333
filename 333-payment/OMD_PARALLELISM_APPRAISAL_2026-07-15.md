# OMD parallel-agent optimization appraisal

Revision reviewed: `b124bfa306fd5626b336dde5d0f7a05e1c272962`  
Verdict: **SAFETY-PROGRESSIVE, THROUGHPUT-UNPROVEN**

## Highest-severity problem

The default agent lease expired during a legitimate long-running OOPTDD run.
OMD retired the agent and force-removed a dirty worktree, destroying uncommitted
changes. Expiry is therefore not merely a scheduling event: it can become data
loss. A safe coordinator must quarantine, checkpoint or preserve a dirty
worktree and expose a recovery task. It must never delete it silently.

This was observed directly in task `prom4-payment-ooptdd-omd`, not inferred from
documentation. The task was reconstructed under `agent_ttl=None` and later
merged with fence `1`.

## Is parallel agenting optimized?

Not yet. OMD has strong conflict-aware admission, but not a proven optimizing
scheduler.

- `next_task` selects by stored priority/FIFO while rejecting write-set overlap.
  It has no task-duration estimate, critical-path score, agent capability model,
  expected merge cost or online feedback objective.
- SQLite `BEGIN IMMEDIATE` intentionally permits one mutation writer at a time.
  Reads can overlap, but coordinator mutations serialize.
- A repo-wide merge token intentionally serializes every `connect`; parallel
  editing therefore converges into a single integration queue.
- shared/hot-file lanes allow concurrent edits, but same-hunk conflicts still
  require rebase/retry and can erase the predicted gain.
- glob overlap is conservative, so false-positive conflicts trade parallelism
  for safety.
- no committed throughput/latency/scale benchmark was found. There is no measured
  speedup curve for N=1,2,4,8,16,32 agents, hot-file ratios, or connect latency.

The accurate claim is: **OMD safely exposes parallelism that is obvious from
declared disjoint write-sets. It does not currently maximize parallel throughput.**

## Additional operational findings from this run

1. `start()` produced a new orbit from the coordinator repo's detached HEAD,
   behind the declared integration branch tip. A clean explicit rebase was
   required. Orbit creation must resolve and pin `integration_branch^{commit}`.
2. An already checked-out integration branch caused the temporary integration
   worktree creation to fail, but OMD classified the Git branch-in-use error as a
   merge conflict. It should have a dedicated configuration/recovery result.
3. coordinator heartbeat and agent heartbeat are separate. A live coordinator
   does not keep a long-running agent lease alive automatically.
4. a task with two valid write orbits received two different fences, but
   commit/finish/connect accept one fence argument. Either value made the other
   orbit stale. The run had to release both and claim the union atomically under
   one fence, but released historical orbits still poisoned validation as stale.
   Aggregate multi-orbit fencing and its recovery path are therefore incomplete.

## Required optimization programme

Priority order:

1. **P0 durability:** preserve/checkpoint dirty worktrees on expiry; add a
   destructive-reclaim OOPTDD regression test.
2. **P0 base correctness:** atomically resolve the declared integration tip when
   starting an orbit; support an integration branch already checked out.
3. **P1 liveness:** automatic agent heartbeat or renewable long-operation lease,
   bounded periodic sweep and explicit recovery queue.
4. **P1 measurement:** benchmark task throughput, scheduling wait, p50/p95/p99
   connect latency, conflict/retry rate and speedup efficiency across agent counts
   and hot-file ratios.
5. **P2 optimization:** critical-path/cost-aware scheduling and safe merge
   batching. These must retain write-set fencing and cannot bypass the single
   integration truth.
