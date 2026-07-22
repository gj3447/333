// SPDX-License-Identifier: AGPL-3.0-only
// 333 modification notice (2026): tick-driven Rust worker queue inspired by
// Puter's worker scheduling pattern. See ../../../THIRD_PARTY_NOTICES.md.
// KG: puter-tpa-P8-worker-queue-2026-04-16, SPAN_333_Kernel
// P8: Worker job queue — lightweight async task queue (WASM-compatible, no tokio)
// Inspired by Puter Worker job scheduling pattern.
// Jobs are polled synchronously (tick-driven) — fits the 333 game loop model.

use std::collections::{VecDeque, HashMap};
use std::fmt;

/// Unique job identifier
pub type JobId = u64;

/// Job priority (higher = processed first)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum JobPriority {
    Low    = 0,
    Normal = 1,
    High   = 2,
    /// Reserved for platform-internal urgent work
    Critical = 3,
}

/// Job state
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobState {
    Pending,
    Running,
    Completed,
    Failed(String),
    Cancelled,
}

impl fmt::Display for JobState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JobState::Pending    => write!(f, "Pending"),
            JobState::Running    => write!(f, "Running"),
            JobState::Completed  => write!(f, "Completed"),
            JobState::Failed(e)  => write!(f, "Failed({})", e),
            JobState::Cancelled  => write!(f, "Cancelled"),
        }
    }
}

/// A unit of deferred work. The closure takes no args — captures its context.
pub struct Job {
    pub id:       JobId,
    pub name:     String,
    pub priority: JobPriority,
    pub state:    JobState,
    work:         Box<dyn FnOnce() -> Result<(), String> + Send>,
}

impl fmt::Debug for Job {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Job({}, {:?}, {:?})", self.id, self.name, self.state)
    }
}

/// Worker queue — tick-driven job dispatcher (no async runtime needed)
///
/// Design:
/// - Jobs are enqueued with a priority.
/// - `tick(n)` runs up to n jobs per frame, highest priority first.
/// - Completed/failed jobs stay in `done` for inspection (bounded ring).
/// - Cancel is O(n) scan — acceptable for small queues.
pub struct WorkerQueue {
    queues:   [VecDeque<Job>; 4], // indexed by JobPriority as usize
    done:     VecDeque<(JobId, JobState)>,
    in_flight: HashMap<JobId, String>, // id → name (currently Running)
    next_id:  JobId,
    max_done: usize,
    total_submitted: u64,
    total_completed: u64,
    total_failed:    u64,
}

impl WorkerQueue {
    pub fn new() -> Self {
        Self {
            queues: [
                VecDeque::new(),
                VecDeque::new(),
                VecDeque::new(),
                VecDeque::new(),
            ],
            done:     VecDeque::new(),
            in_flight: HashMap::new(),
            next_id:  1,
            max_done: 256,
            total_submitted: 0,
            total_completed: 0,
            total_failed:    0,
        }
    }

    /// Enqueue a job. Returns the assigned JobId.
    pub fn enqueue<F>(
        &mut self,
        name: impl Into<String>,
        priority: JobPriority,
        work: F,
    ) -> JobId
    where
        F: FnOnce() -> Result<(), String> + Send + 'static,
    {
        let id = self.next_id;
        self.next_id += 1;
        let job = Job {
            id,
            name: name.into(),
            priority,
            state: JobState::Pending,
            work: Box::new(work),
        };
        self.queues[priority as usize].push_back(job);
        self.total_submitted += 1;
        id
    }

    /// Process up to `max_jobs` jobs this tick. Returns count executed.
    pub fn tick(&mut self, max_jobs: usize) -> usize {
        let mut executed = 0;
        for pri in (0..4).rev() { // Critical first, then High, Normal, Low
            while executed < max_jobs {
                let Some(mut job) = self.queues[pri].pop_front() else { break };
                job.state = JobState::Running;
                self.in_flight.insert(job.id, job.name.clone());

                // Panic isolation (assessment action 7): a panicking job must NOT
                // unwind the whole tick loop or leak its in_flight entry. Catch the
                // unwind, clean up, and map the panic to a Failed outcome so the
                // kernel and the other queued jobs survive.
                //
                // NATIVE ONLY. On wasm32-unknown-unknown this catches nothing:
                // the target spec hard-codes `"panic-strategy": "abort"`, so a
                // panic traps the instance instead of unwinding and control never
                // returns here. `-C panic=unwind` does not opt out — it fails to
                // build ("the crate `panic_unwind` does not have the panic strategy
                // `unwind`"), and the `panic = "abort"` line in [profile.release] is
                // not the cause; removing it changes nothing but binary size.
                // Measured 2026-07-15: a panicking job traps the wasm instance and
                // every later call on it fails, in BOTH dev and release profiles.
                //
                // So in the browser the real contract is the job signature, not this
                // guard: a job MUST report failure as `Err`, never by panicking.
                // Genuine wasm panic isolation needs a process boundary (run the
                // platform in a Web Worker and respawn it), which is architectural
                // and deliberately out of scope here.
                // # KG: lesson-333-catch-unwind-inert-on-wasm-2026-07-15
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (job.work)()));
                self.in_flight.remove(&job.id);

                match result {
                    Ok(Ok(())) => {
                        job.state = JobState::Completed;
                        self.total_completed += 1;
                    }
                    Ok(Err(e)) => {
                        job.state = JobState::Failed(e);
                        self.total_failed += 1;
                    }
                    Err(_panic) => {
                        job.state = JobState::Failed("job panicked".into());
                        self.total_failed += 1;
                    }
                }
                // Store outcome in done ring
                self.done.push_back((job.id, job.state));
                if self.done.len() > self.max_done {
                    self.done.pop_front();
                }
                executed += 1;
            }
        }
        executed
    }

    /// Cancel a pending job. Returns true if found and cancelled.
    pub fn cancel(&mut self, id: JobId) -> bool {
        for queue in &mut self.queues {
            if let Some(pos) = queue.iter().position(|j| j.id == id) {
                let mut job = queue.remove(pos).unwrap();
                job.state = JobState::Cancelled;
                self.done.push_back((id, JobState::Cancelled));
                return true;
            }
        }
        false
    }

    /// Total jobs pending across all priority queues
    pub fn pending_count(&self) -> usize {
        self.queues.iter().map(|q| q.len()).sum()
    }

    /// How many jobs currently executing (should be 0 outside tick() in single-threaded)
    pub fn in_flight_count(&self) -> usize { self.in_flight.len() }

    /// Outcome of a completed job (if in done ring)
    pub fn outcome(&self, id: JobId) -> Option<&JobState> {
        self.done.iter().find(|(jid, _)| *jid == id).map(|(_, s)| s)
    }

    pub fn stats(&self) -> WorkerStats {
        WorkerStats {
            pending:   self.pending_count(),
            submitted: self.total_submitted,
            completed: self.total_completed,
            failed:    self.total_failed,
        }
    }
}

impl Default for WorkerQueue {
    fn default() -> Self { Self::new() }
}

#[derive(Debug, Clone)]
pub struct WorkerStats {
    pub pending:   usize,
    pub submitted: u64,
    pub completed: u64,
    pub failed:    u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn enqueue_and_tick() {
        let mut wq = WorkerQueue::new();
        let result = Arc::new(Mutex::new(0u32));
        let r = Arc::clone(&result);

        let id = wq.enqueue("add_one", JobPriority::Normal, move || {
            *r.lock().unwrap() += 1;
            Ok(())
        });
        assert_eq!(wq.pending_count(), 1);

        let ran = wq.tick(10);
        assert_eq!(ran, 1);
        assert_eq!(*result.lock().unwrap(), 1);
        assert_eq!(wq.pending_count(), 0);
        assert!(matches!(wq.outcome(id), Some(JobState::Completed)));
    }

    #[test]
    fn priority_order() {
        let mut wq = WorkerQueue::new();
        let order = Arc::new(Mutex::new(Vec::<&'static str>::new()));

        let o1 = Arc::clone(&order);
        wq.enqueue("low",      JobPriority::Low,      move || { o1.lock().unwrap().push("low");      Ok(()) });
        let o2 = Arc::clone(&order);
        wq.enqueue("normal",   JobPriority::Normal,   move || { o2.lock().unwrap().push("normal");   Ok(()) });
        let o3 = Arc::clone(&order);
        wq.enqueue("high",     JobPriority::High,     move || { o3.lock().unwrap().push("high");     Ok(()) });
        let o4 = Arc::clone(&order);
        wq.enqueue("critical", JobPriority::Critical, move || { o4.lock().unwrap().push("critical"); Ok(()) });

        wq.tick(4);
        let executed = order.lock().unwrap().clone();
        assert_eq!(executed, vec!["critical", "high", "normal", "low"]);
    }

    #[test]
    fn failed_job_recorded() {
        let mut wq = WorkerQueue::new();
        let id = wq.enqueue("bomb", JobPriority::Normal, || Err("kaboom".into()));
        wq.tick(1);
        assert!(matches!(wq.outcome(id), Some(JobState::Failed(_))));
        assert_eq!(wq.stats().failed, 1);
    }

    #[test]
    fn cancel_pending_job() {
        let mut wq = WorkerQueue::new();
        let ran = Arc::new(Mutex::new(false));
        let r = Arc::clone(&ran);
        let id = wq.enqueue("cancel_me", JobPriority::Normal, move || {
            *r.lock().unwrap() = true;
            Ok(())
        });
        assert!(wq.cancel(id));
        wq.tick(10);
        assert!(!*ran.lock().unwrap()); // never ran
        assert!(matches!(wq.outcome(id), Some(JobState::Cancelled)));
    }

    #[test]
    fn tick_respects_max_jobs() {
        let mut wq = WorkerQueue::new();
        for i in 0..10 {
            wq.enqueue(format!("j{}", i), JobPriority::Normal, || Ok(()));
        }
        let ran = wq.tick(3);
        assert_eq!(ran, 3);
        assert_eq!(wq.pending_count(), 7);
    }

    #[test]
    fn stats_accurate() {
        let mut wq = WorkerQueue::new();
        wq.enqueue("ok",   JobPriority::Normal, || Ok(()));
        wq.enqueue("fail", JobPriority::Normal, || Err("e".into()));
        wq.tick(2);
        let s = wq.stats();
        assert_eq!(s.submitted, 2);
        assert_eq!(s.completed, 1);
        assert_eq!(s.failed, 1);
        assert_eq!(s.pending, 0);
    }

    #[test]
    fn panicking_job_is_isolated_and_others_survive() {
        // Assessment action 7: a panicking job must not abort the tick loop or
        // leak its in_flight entry; it fails, and the other jobs still run.
        let mut wq = WorkerQueue::new();
        wq.enqueue("boom", JobPriority::Normal, || panic!("job blew up"));
        wq.enqueue("ok", JobPriority::Normal, || Ok(()));
        let ran = wq.tick(10);
        assert_eq!(ran, 2, "panicking job does not abort the tick loop");
        assert_eq!(wq.in_flight_count(), 0, "in_flight cleaned up despite panic");
        let s = wq.stats();
        assert_eq!(s.completed, 1); // the ok job still ran
        assert_eq!(s.failed, 1); // the panicking job counted as failed
    }
}
