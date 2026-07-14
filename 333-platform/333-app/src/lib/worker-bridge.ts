// KG: TASK_333_B_Workers, CONTRACT_333_B_Workers
// KG: SPAN_333_B_Workers — Worker pool bridge for OM distributed compute
// Uses native Web Workers (not Comlink — keeping deps minimal)
// Transferable ArrayBuffer for zero-copy data transfer

export interface ComputeTask {
  type: 'hash' | 'mapreduce' | 'cpu';
  data: Uint8Array;
}

export interface ComputeResult {
  success: boolean;
  output: Uint8Array;
  compute_ms: number;
}

interface PendingTask {
  resolve: (result: ComputeResult) => void;
  reject: (error: Error) => void;
}

/**
 * Worker Pool — manages N workers for CPU offload
 * KG: CONTRACT_333_B_Workers — UI 블로킹 0, hardwareConcurrency 기반
 */
export class WorkerPool {
  private workers: Worker[] = [];
  private pending = new Map<number, PendingTask>();
  private nextId = 0;
  private roundRobin = 0;
  readonly size: number;

  // Taliban Fix 1: Use inline worker blob URL (no external file dependency)
  constructor() {
    this.size = Math.min(navigator.hardwareConcurrency || 4, 8);

    // Inline worker code as blob — guaranteed to exist at runtime
    const workerCode = `
      self.onmessage = async (e) => {
        const { id, type, payload } = e.data;
        const start = performance.now();
        try {
          let output;
          switch (type) {
            case 'hash': output = await crypto.subtle.digest('SHA-256', payload); break;
            case 'mapreduce': {
              const data = new Uint8Array(payload);
              const freq = new Uint32Array(256);
              for (let i = 0; i < data.length; i++) freq[data[i]]++;
              output = freq.buffer; break;
            }
            case 'cpu': {
              const sorted = new Uint8Array(payload).slice().sort();
              output = await crypto.subtle.digest('SHA-256', sorted); break;
            }
            default: output = new ArrayBuffer(0);
          }
          self.postMessage({ id, success: true, output, compute_ms: performance.now()-start }, [output]);
        } catch(err) {
          self.postMessage({ id, success: false, output: new ArrayBuffer(0), compute_ms: performance.now()-start });
        }
      };
    `;
    const blob = new Blob([workerCode], { type: 'application/javascript' });
    const url = URL.createObjectURL(blob);

    for (let i = 0; i < this.size; i++) {
      const worker = new Worker(url);
      worker.onmessage = (e) => this.onResult(e.data);
      worker.onerror = (e) => {
        console.error(`Worker ${i} error:`, e);
        // Reject all pending for this worker
      };
      this.workers.push(worker);
    }
  }

  /**
   * Submit a compute task — returns Promise resolved when worker completes
   * KG: CONTRACT_333_B_Workers — Transferable ArrayBuffer, zero-copy
   */
  submit(task: ComputeTask): Promise<ComputeResult> {
    return new Promise((resolve, reject) => {
      const id = this.nextId++;
      this.pending.set(id, { resolve, reject });

      // Round-robin worker selection
      const worker = this.workers[this.roundRobin % this.size];
      this.roundRobin++;

      // Transfer ownership of data buffer to worker (zero-copy)
      const buffer = task.data.buffer.slice(0);
      worker.postMessage(
        { id, type: task.type, payload: buffer },
        [buffer] // Transferable
      );
    });
  }

  /**
   * Submit SHA-256 hash computation
   */
  async hash(data: Uint8Array): Promise<Uint8Array> {
    const result = await this.submit({ type: 'hash', data });
    return result.output;
  }

  /**
   * Submit MapReduce (byte frequency count)
   */
  async mapReduce(data: Uint8Array): Promise<Uint32Array> {
    const result = await this.submit({ type: 'mapreduce', data });
    return new Uint32Array(result.output.buffer);
  }

  /**
   * Submit generic CPU work
   */
  async cpuWork(data: Uint8Array): Promise<Uint8Array> {
    const result = await this.submit({ type: 'cpu', data });
    return result.output;
  }

  /** Number of pending tasks */
  get pendingCount(): number {
    return this.pending.size;
  }

  /** Terminate all workers */
  terminate(): void {
    for (const w of this.workers) w.terminate();
    this.workers = [];
    this.pending.clear();
  }

  private onResult(data: { id: number; success: boolean; output: ArrayBuffer; compute_ms: number }) {
    const task = this.pending.get(data.id);
    if (!task) return;
    this.pending.delete(data.id);

    if (data.success) {
      task.resolve({
        success: true,
        output: new Uint8Array(data.output),
        compute_ms: data.compute_ms,
      });
    } else {
      task.reject(new Error('Worker task failed'));
    }
  }
}

// Singleton pool instance
let pool: WorkerPool | null = null;

export function getWorkerPool(): WorkerPool {
  if (!pool) {
    pool = new WorkerPool();
  }
  return pool;
}

export function terminateWorkerPool(): void {
  pool?.terminate();
  pool = null;
}
