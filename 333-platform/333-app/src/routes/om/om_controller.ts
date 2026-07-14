// KG: sprint5B-om-wasm-ui-2026-04-15
// KG: SOLID-DIP-fix-wasm-sessions-2026-04-16 — IOmSession interface 경유
// OM Controller — UI↔WASM bridge for OmOrchestrator.

import { createOmSession } from '../../lib/wasm-sessions';
import type { IOmSession } from '../../lib/wasm-sessions';
//
// WASM binding status:
//  1. initOmSession    → new wasm.OmSessionWasm(peerId)           [WASM]
//  2. submitJob        → wasm.om.submit_job(kindJson, nowMs)      [WASM]
//  3. distribute       → wasm.om.distribute(nowMs)                [WASM]
//  4. submitResult     → wasm.om.submit_result(resultJson, nowMs) [WASM]
//  5. pollStats        → wasm.om.stats_json()                     [WASM]
//  6. jobStatus        → wasm.om.job_status_json(jobId)           [WASM]
//
// Mock mode: if WASM fails to load, local in-memory state is used.

// ---------------------------------------------------------------------------
// Types
// KG: sprint5B-om-wasm-ui-2026-04-15
// ---------------------------------------------------------------------------

export interface OmNodeInfo {
  node_id: number;
  cpu_cores: number;
  memory_mb: number;
  gpu_available: boolean;
  battery_pct: number | null;
  active_workers: number;
  max_workers: number;
}

export interface OmStats {
  nodes_online: number;
  nodes_available: number;
  gpu_nodes: number;
  total_capacity: number;
  total_jobs: number;
  active_jobs: number;
  completed_jobs: number;
  total_compute_ms: number;
  total_tokens_distributed: number;
  pending_tasks: number;
}

export interface OmJob {
  job_id: string;
  kind: string;
  submitter: number;
  reward_per_task: number;
  status: string;
  created_at: number;
  completed_at: number | null;
  task_count: number;
  result_count: number;
}

export interface OmAssignment {
  task_id: number;
  worker_id: number;
  assigned_at: number;
  timeout_ms: number;
  reward: number;
}

export type OmJobKind = 'cpu' | 'gpu' | 'data' | 'mapreduce';

export interface OmState {
  peerId: number;
  stats: OmStats;
  jobs: OmJob[];
  recentAssignments: OmAssignment[];
  lastVerifyJson: string;
  lastResultTs: number | null;
  wasmActive: boolean;
  log: string[];
}

// ---------------------------------------------------------------------------
// Mock state helpers
// KG: sprint5B-om-wasm-ui-2026-04-15
// ---------------------------------------------------------------------------

let _mockJobSeq = 0;

function mockJobId(): string {
  return `om-mock-${++_mockJobSeq}`;
}

function emptyStats(): OmStats {
  return {
    nodes_online: 0,
    nodes_available: 0,
    gpu_nodes: 0,
    total_capacity: 0,
    total_jobs: 0,
    active_jobs: 0,
    completed_jobs: 0,
    total_compute_ms: 0,
    total_tokens_distributed: 0,
    pending_tasks: 0,
  };
}

// ---------------------------------------------------------------------------
// OmController
// KG: sprint5B-om-wasm-ui-2026-04-15
// ---------------------------------------------------------------------------

export class OmController {
  private state: OmState;
  private onStateChange: (s: OmState) => void;

  // WASM handle — IOmSession interface 경유 (DIP 준수).
  // KG: sprint5B-om-wasm-ui-2026-04-15, SOLID-DIP-fix-wasm-sessions-2026-04-16
  private wasmOm: IOmSession | null = null;
  private _wasmAvailable = false;

  private _pollTimer: ReturnType<typeof setInterval> | null = null;

  // Mock job list for fallback mode
  private _mockJobs: OmJob[] = [];

  constructor(onStateChange: (s: OmState) => void) {
    this.onStateChange = onStateChange;
    this.state = {
      peerId: 1,
      stats: emptyStats(),
      jobs: [],
      recentAssignments: [],
      lastVerifyJson: '',
      lastResultTs: null,
      wasmActive: false,
      log: ['[om] controller ready'],
    };
  }

  // ---------------------------------------------------------------------------
  // Public API
  // ---------------------------------------------------------------------------

  /**
   * initOmSession — load WASM module and create OmSessionWasm(peerId).
   * Falls back to mock mode if WASM is unavailable.
   *
   * # KG: sprint5B-om-wasm-ui-2026-04-15
   */
  async initOmSession(peerId: number): Promise<void> {
    this.state = { ...this.state, peerId };

    try {
      // createOmSession factory (DIP: interface 경유, 직접 import 금지)
      // KG: SOLID-DIP-fix-wasm-sessions-2026-04-16
      this.wasmOm = await createOmSession(peerId);
      this._wasmAvailable = true;
      this._addLog(`[om] WASM init peerId=${peerId}`);
      this.state = { ...this.state, wasmActive: true };
    } catch (e) {
      this._wasmAvailable = false;
      this._addLog(`[om] WASM unavailable, mock mode (${String(e).slice(0, 60)})`);
    }

    this._startPoll();
    this._syncStats();
    this._pushState();
  }

  /**
   * submitJob — submit a compute job.
   * Returns the job_id string.
   *
   * # KG: sprint5B-om-wasm-ui-2026-04-15
   */
  submitJob(kind: OmJobKind, params: Record<string, number | string> = {}): string {
    const kindJson = JSON.stringify({ kind, ...params });
    const nowMs = Date.now();

    if (this._wasmAvailable && this.wasmOm) {
      try {
        const jobId = this.wasmOm.submit_job(kindJson, nowMs) as string;
        this._addLog(`[submit] ${jobId} kind=${kind}`);
        // Trigger immediate distribute
        this._distribute(nowMs);
        this._syncStats();
        return jobId;
      } catch (e) {
        this._addLog(`[submit] WASM error: ${String(e).slice(0, 60)}`);
      }
    }

    // Mock fallback
    const jid = mockJobId();
    const mockJob: OmJob = {
      job_id: jid,
      kind,
      submitter: this.state.peerId,
      reward_per_task: 1,
      status: 'Submitted',
      created_at: nowMs,
      completed_at: null,
      task_count: kind === 'mapreduce' ? (params.chunk_count as number ?? 4) : 1,
      result_count: 0,
    };
    this._mockJobs = [mockJob, ...this._mockJobs];
    this._updateMockStats();
    this._addLog(`[submit-mock] ${jid} kind=${kind}`);
    this._pushState();
    return jid;
  }

  /**
   * submitResult — submit a task result (for demo/testing).
   * In real usage this would come from worker execution.
   *
   * # KG: sprint5B-om-wasm-ui-2026-04-15
   */
  submitResult(taskId: number, workerId: number, outputHash: string, computeMs: number): void {
    const nowMs = Date.now();
    const resultJson = JSON.stringify({
      task_id: taskId,
      worker_id: workerId,
      output: [],
      compute_ms: computeMs,
      output_hash: outputHash,
    });

    if (this._wasmAvailable && this.wasmOm) {
      try {
        const verifyJson = this.wasmOm.submit_result(resultJson, nowMs) as string;
        this._addLog(`[result] task=${taskId} verify=${verifyJson}`);
        this.state = { ...this.state, lastVerifyJson: verifyJson, lastResultTs: nowMs };
        this._syncStats();
        return;
      } catch (e) {
        this._addLog(`[result] WASM error: ${String(e).slice(0, 60)}`);
      }
    }

    // Mock fallback — mark first Submitted job as Completed
    const idx = this._mockJobs.findIndex(j => j.status !== 'Completed');
    if (idx >= 0) {
      this._mockJobs[idx] = {
        ...this._mockJobs[idx],
        status: 'Completed',
        completed_at: nowMs,
        result_count: this._mockJobs[idx].task_count,
      };
    }
    this._updateMockStats();
    this.state = { ...this.state, lastVerifyJson: '{"verdict":"ok","kind":"no_verification"}', lastResultTs: nowMs };
    this._addLog(`[result-mock] task=${taskId}`);
    this._pushState();
  }

  /**
   * jobStatusJson — get full job JSON by ID.
   *
   * # KG: sprint5B-om-wasm-ui-2026-04-15
   */
  jobStatusJson(jobId: string): string {
    if (this._wasmAvailable && this.wasmOm) {
      try {
        return this.wasmOm.job_status_json(jobId) as string;
      } catch { /* fallthrough */ }
    }
    const job = this._mockJobs.find(j => j.job_id === jobId);
    return job ? JSON.stringify(job) : 'null';
  }

  destroy(): void {
    this._stopPoll();
    if (this.wasmOm) {
      try { this.wasmOm.free?.(); } catch { /* ignore */ }
      this.wasmOm = null;
    }
  }

  // ---------------------------------------------------------------------------
  // Private
  // ---------------------------------------------------------------------------

  /** Trigger WASM distribute and capture assignments. */
  private _distribute(nowMs: number): void {
    if (!this._wasmAvailable || !this.wasmOm) return;
    try {
      const assignJson = this.wasmOm.distribute(nowMs) as string;
      const assignments: OmAssignment[] = JSON.parse(assignJson);
      if (assignments.length > 0) {
        this.state = {
          ...this.state,
          recentAssignments: [...assignments, ...this.state.recentAssignments].slice(0, 20),
        };
        this._addLog(`[distribute] +${assignments.length} assignments`);
      }
    } catch (e) {
      this._addLog(`[distribute] error: ${String(e).slice(0, 60)}`);
    }
  }

  /** Sync stats from WASM. */
  private _syncStats(): void {
    if (this._wasmAvailable && this.wasmOm) {
      try {
        const stats: OmStats = JSON.parse(this.wasmOm.stats_json() as string);
        this.state = { ...this.state, stats };
      } catch { /* ignore */ }
    } else {
      this._updateMockStats();
    }
    this._pushState();
  }

  /** Update stats from mock job list. */
  private _updateMockStats(): void {
    const jobs = this._mockJobs;
    const active = jobs.filter(j => j.status !== 'Completed' && j.status !== 'Failed').length;
    const done = jobs.filter(j => j.status === 'Completed').length;
    const totalMs = jobs.filter(j => j.completed_at !== null)
      .reduce((acc, j) => acc + (j.completed_at! - j.created_at), 0);
    this.state = {
      ...this.state,
      jobs: [...this._mockJobs],
      stats: {
        nodes_online: 1,
        nodes_available: 1,
        gpu_nodes: 0,
        total_capacity: 4,
        total_jobs: jobs.length,
        active_jobs: active,
        completed_jobs: done,
        total_compute_ms: totalMs,
        total_tokens_distributed: done,
        pending_tasks: active,
      },
    };
  }

  private _startPoll(): void {
    // Poll every 2 s for stats + pending assignments
    // KG: sprint5B-om-wasm-ui-2026-04-15
    this._pollTimer = setInterval(() => {
      if (this._wasmAvailable) {
        this._distribute(Date.now());
        this._syncStats();
      }
    }, 2000);
  }

  private _stopPoll(): void {
    if (this._pollTimer !== null) {
      clearInterval(this._pollTimer);
      this._pollTimer = null;
    }
  }

  private _addLog(msg: string): void {
    this.state = { ...this.state, log: [...this.state.log.slice(-49), msg] };
  }

  private _pushState(): void {
    this.onStateChange({ ...this.state });
  }
}
