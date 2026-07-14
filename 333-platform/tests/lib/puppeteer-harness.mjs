// KG: phase-post0-A-chrome-harness-2026-04-14, finding_d0_chrome_harness, guardrail-fired-load-27-2026-04-14
// Chrome 프로세스 누적 방지 — SIGINT/SIGTERM/exit handlers + close watchdog + per-context cleanup.
// 2026-04-14 load 27 CRITICAL 사건 (Chrome 39개 누적) 재발 방지.
import puppeteer from '../../333-app/node_modules/puppeteer-core/lib/esm/puppeteer/puppeteer-core.js';

const CHROME = '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';

// 전역 cleanup 등록대. process 종료 시 모두 호출.
const cleanups = new Set();
let handlersInstalled = false;

function installSignalHandlers() {
  if (handlersInstalled) return;
  handlersInstalled = true;
  const run = async (sig, code) => {
    console.error(`\n[harness] ${sig} received — ${cleanups.size} cleanups`);
    for (const fn of cleanups) {
      try { await withTimeout(fn(), 5000, sig + '-cleanup'); }
      catch (e) { console.error('  cleanup error:', e?.message?.slice(0, 80)); }
    }
    process.exit(code);
  };
  process.on('SIGINT',  () => run('SIGINT', 130));
  process.on('SIGTERM', () => run('SIGTERM', 143));
  process.on('uncaughtException', (e) => {
    console.error('[harness] uncaught:', e?.message?.slice(0, 100));
    run('uncaught', 1);
  });
  process.on('exit', () => {
    // 동기 처리만 가능 — 최후 방어선
    if (cleanups.size > 0) console.error(`[harness] exit: ${cleanups.size} cleanups pending (sync only)`);
  });
}

export function withTimeout(promise, ms, label = 'op') {
  return Promise.race([
    promise,
    new Promise((_, reject) => setTimeout(() => reject(new Error(`${label} timeout ${ms}ms`)), ms))
  ]);
}

/**
 * puppeteer.launch wrapper — Chrome 프로세스 자동 정리 보장.
 * 반환: [browser, cleanup()] — cleanup은 idempotent.
 */
export async function createBrowser(opts = {}) {
  installSignalHandlers();
  const browser = await puppeteer.launch({
    executablePath: CHROME,
    headless: opts.headless ?? false,
    handleSIGINT: false, // 우리가 직접 관리
    handleSIGTERM: false,
    handleSIGHUP: false,
    args: [
      '--no-sandbox',
      '--use-fake-ui-for-media-stream',
      '--use-fake-device-for-media-stream',
      '--allow-running-insecure-content',
      '--disable-web-security',
      '--disable-dev-shm-usage',
      ...(opts.args || [])
    ],
  });
  let closed = false;
  const cleanup = async () => {
    if (closed) return;
    closed = true;
    cleanups.delete(cleanup);
    try {
      await withTimeout(browser.close(), 30000, 'browser.close');
    } catch (e) {
      console.error('[harness] browser.close timeout — force kill proc');
      const proc = browser.process();
      if (proc && !proc.killed) proc.kill('SIGKILL');
    }
  };
  cleanups.add(cleanup);
  return [browser, cleanup];
}

/**
 * createBrowserContext wrapper — context 단위 cleanup 추가.
 */
export async function createContext(browser, label = 'ctx') {
  const ctx = await browser.createBrowserContext();
  const page = await ctx.newPage();
  const cleanup = async () => {
    try { await withTimeout(ctx.close(), 5000, label + '.ctx.close'); }
    catch (e) { /* 무시 — browser cleanup 처리 */ }
  };
  return { ctx, page, cleanup };
}

/**
 * run(fn, { timeout }) — 테스트 본문 실행 wrapper. 예외 시에도 cleanup 보장.
 */
export async function run(fn, opts = {}) {
  const timeout = opts.timeout ?? 600_000; // 10min max
  try {
    await withTimeout(fn(), timeout, 'test-body');
  } catch (e) {
    console.error('[harness] test failed:', e?.message?.slice(0, 120));
    process.exitCode = 1;
  } finally {
    // 등록된 모든 cleanup 실행 (browser close 등)
    for (const fn of [...cleanups]) {
      try { await fn(); } catch { /* 무시 */ }
    }
  }
}
