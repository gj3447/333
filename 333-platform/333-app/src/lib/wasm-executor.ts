// KG: lesson-333-wasm-borrowmut-needs-global-queue-2026-04-14
// KG: plan-wasm-executor-solid-2026-04-14, phase-1-wasm-executor-design-2026-04-14
// KG: src-fe-wasm-executor (NEW)
//
// SOLID:
//  S(SRP): 직렬화만 책임. wasm method 호출 자체는 caller closure.
//  O(OCP): 새 method 추가 시 본 클래스 변경 불필요 (run<T> generic).
//  L(LSP): 모든 wasm method가 동일 contract — () => T|Promise<T>.
//  I(ISP): 외부엔 run + readonly stats만. 큐 내부 비공개.
//  D(DIP): UI는 WasmBridge에 의존, Bridge는 Executor에 의존, Executor는 Promise primitives에만 의존.

export interface WasmExecutorStats {
  readonly queueDepth: number;     // 현재 대기 중 작업 수
  readonly maxConcurrent: number;  // 관측된 최대 동시 대기
  readonly totalRuns: number;      // 누적 실행 수
  readonly avgWaitMs: number;      // 평균 큐 대기 시간
  readonly errorCount: number;     // 누적 에러
  readonly lastError?: string;     // 마지막 에러 메시지 (디버깅용)
}

export class WasmExecutor {
  private chain: Promise<unknown> = Promise.resolve();
  private _stats = {
    queueDepth: 0, maxConcurrent: 0, totalRuns: 0,
    avgWaitMs: 0, errorCount: 0,
    waitMsAccum: 0, lastError: undefined as string | undefined,
  };

  constructor(private readonly trace: boolean = false, public readonly label = 'wasm') {
    // 디버깅: puppeteer evaluate로 window.__wasmStats() 호출 시 실시간 값 반환
    // NOTE: 초기 버그 — `= this.stats` 는 생성자 호출 시점 snapshot 저장으로 이후 업데이트 반영 안 됨.
    //       getter function으로 노출해서 매 호출마다 live 값 반환.
    if (typeof window !== 'undefined') {
      (window as any).__wasmStats = () => this.stats;
      (window as any).__wasmStatsLive = this._stats;  // mutable 직접 참조 (읽기전용 캐스트용)
    }
  }

  /** 외부 노출 stats — 새 객체로 매번 반환 (불변성 보장) */
  get stats(): WasmExecutorStats {
    const s = this._stats;
    return {
      queueDepth: s.queueDepth, maxConcurrent: s.maxConcurrent,
      totalRuns: s.totalRuns, avgWaitMs: s.avgWaitMs,
      errorCount: s.errorCount, lastError: s.lastError,
    };
  }

  /** 직렬화 실행. fn은 sync 또는 async. label은 디버그 trace용. */
  run<T>(fn: () => T | Promise<T>, opLabel?: string): Promise<T> {
    this._stats.queueDepth++;
    if (this._stats.queueDepth > this._stats.maxConcurrent) {
      this._stats.maxConcurrent = this._stats.queueDepth;
    }
    const enqueueTime = performance.now();

    const next = this.chain.then(async () => {
      const startTime = performance.now();
      const waitMs = startTime - enqueueTime;
      this._stats.waitMsAccum += waitMs;
      this._stats.totalRuns++;
      this._stats.avgWaitMs = this._stats.waitMsAccum / this._stats.totalRuns;

      if (this.trace) {
        console.debug(`[${this.label}] ▶ ${opLabel ?? '?'} wait=${waitMs.toFixed(1)}ms depth=${this._stats.queueDepth}`);
      }
      try {
        const result = await fn();
        if (this.trace) {
          console.debug(`[${this.label}] ✓ ${opLabel ?? '?'}`);
        }
        return result;
      } catch (e) {
        this._stats.errorCount++;
        this._stats.lastError = `${opLabel ?? '?'}: ${e instanceof Error ? e.message : String(e)}`;
        if (this.trace) console.error(`[${this.label}] ✗ ${opLabel}:`, e);
        throw e;
      } finally {
        this._stats.queueDepth--;
      }
    });

    // chain은 항상 resolve로 진행 (caller가 reject 받지만 큐는 안 끊김)
    this.chain = next.catch(() => undefined);
    return next as Promise<T>;
  }

  /** 디버깅: 현재 큐가 빌 때까지 대기 */
  drain(): Promise<void> {
    return this.chain.then(() => undefined);
  }
}

// 글로벌 single instance — Platform333 인스턴스가 single-borrow이므로
// 전역 직렬화가 정확함. 새 인스턴스 만들면 격리 큐가 됨 (테스트용).
const trace = (typeof import.meta !== 'undefined' && (import.meta as any).env?.VITE_WASM_TRACE === '1');
export const wasmExecutor = new WasmExecutor(trace, 'wasm');
