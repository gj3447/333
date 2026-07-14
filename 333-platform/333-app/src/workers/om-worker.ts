// KG: TASK_333_B_Workers, CONTRACT_333_B_Workers
// KG: SPAN_333_B_Workers — OM compute runs in Web Worker (not main thread)
// KG: lesson-333-ts57-uint8-bufferlike-2026-04-19 — webworker lib ref for DedicatedWorkerGlobalScope
/// <reference lib="webworker" />
// This worker handles CPU-intensive tasks: hash search, matrix ops, MapReduce
// Communication: postMessage + Transferable ArrayBuffer

/// Message types from main thread → worker
interface WorkerTask {
  id: number;
  type: 'hash' | 'mapreduce' | 'cpu';
  payload: ArrayBuffer;
}

/// Message types from worker → main thread
interface WorkerResult {
  id: number;
  success: boolean;
  output: ArrayBuffer;
  compute_ms: number;
}

// Worker context
const ctx = self as unknown as DedicatedWorkerGlobalScope;

ctx.onmessage = async (e: MessageEvent<WorkerTask>) => {
  const { id, type, payload } = e.data;
  const start = performance.now();

  try {
    let output: ArrayBuffer;

    switch (type) {
      case 'hash': {
        // SHA-256 hash computation (CPU intensive)
        const data = new Uint8Array(payload);
        const hash = await crypto.subtle.digest('SHA-256', data);
        output = hash;
        break;
      }
      case 'mapreduce': {
        // Simple MapReduce: count byte frequencies
        const data = new Uint8Array(payload);
        const freq = new Uint32Array(256);
        for (let i = 0; i < data.length; i++) {
          freq[data[i]]++;
        }
        output = freq.buffer;
        break;
      }
      case 'cpu': {
        // Generic CPU work: sort + hash
        const data = new Uint8Array(payload);
        const sorted = data.slice().sort();
        const hash = await crypto.subtle.digest('SHA-256', sorted);
        output = hash;
        break;
      }
      default:
        output = new ArrayBuffer(0);
    }

    const compute_ms = performance.now() - start;
    const result: WorkerResult = { id, success: true, output, compute_ms };
    // Transfer ownership of output buffer (zero-copy)
    ctx.postMessage(result, [output]);

  } catch (err) {
    const compute_ms = performance.now() - start;
    const result: WorkerResult = { id, success: false, output: new ArrayBuffer(0), compute_ms };
    ctx.postMessage(result);
  }
};
