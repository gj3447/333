// KG: SPAN_333_OM — OM Distributed Compute WASM Bridge
// KG: seed-om-frontend-wiring-2026-04-15
// KG: lesson-333-wasm-borrowmut-needs-global-queue-2026-04-14
//
// SOLID:
//  S(SRP): OmCompute WASM method 직렬화만 책임.
//  O(OCP): 새 method 추가 시 run<T> 한 줄만 추가.
//  L(LSP): 모든 method Promise<T> 반환 (uniform contract).
//  I(ISP): 외부엔 OmBridge interface + initOm()만 노출.
//  D(DIP): UI는 OmBridge에 의존, Bridge는 wasmExecutor에 의존.

import { wasmExecutor } from './wasm-executor';

export interface OmBridge {
  /** 노드 등록 (node_id, cpu_cores, gpu, battery_pct) */
  registerNode: (nodeId: number, cpuCores: number, gpu: boolean, battery: number) => Promise<void>;
  /** CPU 집약 작업 제출 → job_id JSON 반환 */
  submitCpuJob: (submitter: number, estimatedMs: bigint, nowMs: bigint) => Promise<string>;
  /** GPU 작업 제출 → job_id JSON 반환 */
  submitGpuJob: (submitter: number, modelSizeMb: number, nowMs: bigint) => Promise<string>;
  /** MapReduce 작업 제출 → job_id JSON 반환 */
  submitMapreduceJob: (submitter: number, chunks: number, reduceFn: string, nowMs: bigint) => Promise<string>;
  /** 태스크 분배 실행 → assignment JSON 배열 */
  distribute: (nowMs: bigint) => Promise<string>;
  /** 결과 제출 → verdict 문자열 */
  submitResult: (taskId: bigint, workerId: number, output: Uint8Array, computeMs: bigint, outputHash: string, nowMs: bigint) => Promise<string>;
  /** OM 전체 통계 JSON */
  stats: () => Promise<string>;
  /** 특정 job 상태 JSON */
  jobStatus: (jobId: string) => Promise<string>;
}

let instance: OmBridge | null = null;

function createOmBridge(mod: any): OmBridge {
  // OmCompute는 &mut self 이므로 wasmExecutor 직렬화 필수
  // KG: lesson-333-wasm-borrowmut-needs-global-queue-2026-04-14
  const om = new mod.OmCompute();
  const exec = wasmExecutor;

  return {
    registerNode: (nodeId, cpuCores, gpu, battery) =>
      exec.run(() => om.register_node(nodeId, cpuCores, gpu, battery), 'om.registerNode'),

    // u64 인자 → BigInt 변환 (wasm-bindgen u64 = JS BigInt)
    submitCpuJob: (submitter, estimatedMs, nowMs) =>
      exec.run(() => om.submit_cpu_job(submitter, estimatedMs, nowMs), 'om.submitCpuJob'),

    submitGpuJob: (submitter, modelSizeMb, nowMs) =>
      exec.run(() => om.submit_gpu_job(submitter, modelSizeMb, nowMs), 'om.submitGpuJob'),

    submitMapreduceJob: (submitter, chunks, reduceFn, nowMs) =>
      exec.run(() => om.submit_mapreduce_job(submitter, chunks, reduceFn, nowMs), 'om.submitMapreduceJob'),

    distribute: (nowMs) =>
      exec.run(() => om.distribute(nowMs), 'om.distribute'),

    submitResult: (taskId, workerId, output, computeMs, outputHash, nowMs) =>
      exec.run(() => om.submit_result(taskId, workerId, output, computeMs, outputHash, nowMs), 'om.submitResult'),

    stats: () =>
      exec.run(() => om.stats(), 'om.stats'),

    jobStatus: (jobId) =>
      exec.run(() => om.job_status(jobId), 'om.jobStatus'),
  };
}

export async function initOm(
  wasmUrl = '/333/wasm/triple_three.js'
): Promise<OmBridge> {
  if (instance) return instance;

  try {
    const mod = await import(/* @vite-ignore */ wasmUrl);
    if (mod.default) await mod.default();
    instance = createOmBridge(mod);
    if (typeof window !== 'undefined') {
      (window as any).omBridge = instance;
    }
    console.log('[OM] OmCompute instance created');
  } catch (e) {
    console.error('[OM] WASM init failed:', e);
    throw e;
  }
  return instance;
}

export function getOm(): OmBridge | null {
  return instance;
}
