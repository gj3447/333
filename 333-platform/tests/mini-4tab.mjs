// KG: phase-post0-D-mini-4tab-verify-2026-04-14, finding_d3_min_4tab_verify
// Phase 0 lol_alloc 후 4-validator BFT commit 최초 확증 테스트.
// 설계: single-observer 폴링 (tabs[0]만 roomState 조회). Promise.all(tabs.map) 제거.
// 이유: 4-way 동시 puppeteer evaluate가 WASM borrow 경쟁 유발 의심 (D3).
// harness: SIGINT/SIGTERM + close watchdog으로 Chrome 누적 방지 (load 27 사건 재발 금지).
import { createBrowser, createContext, run } from './lib/puppeteer-harness.mjs';

const BASE = 'http://localhost:4173/333';
const ROOM = 'mini' + Date.now().toString(36);
const V = [1, 2, 3, 4];

async function bootTab(browser, label, nid) {
  const { page } = await createContext(browser, label);
  page.on('pageerror', e => console.log(`  [${label} err] ${e.message.slice(0, 100)}`));
  page.on('console', m => {
    const t = m.text();
    if (t.includes('[333-bft]') || t.includes('[333-hs]')) {
      // 로그 너무 많으면 생략. critical만 보고 싶으면 여기 필터.
    }
  });
  await page.evaluateOnNewDocument((id, v) => {
    delete navigator.serviceWorker;
    window.__333_nodeId = id;
    window.__333_validators = v;
  }, nid, V);
  await page.goto(BASE + '/onboard', { waitUntil: 'load', timeout: 15000 });
  await new Promise(r => setTimeout(r, 1200));
  await page.evaluate(async (lbl) => {
    const meta = {
      peerId: lbl + '-' + Date.now().toString(36),
      username: lbl, publicKeyHex: '00'.repeat(32), algorithm: 'P-256'
    };
    localStorage.setItem('333_identity_meta', JSON.stringify(meta));
    const db = await new Promise((r, j) => {
      const q = indexedDB.open('333_identity_db', 1);
      q.onupgradeneeded = () => q.result.createObjectStore('333_keys');
      q.onsuccess = () => r(q.result);
      q.onerror = () => j(q.error);
    });
    const kp = await crypto.subtle.generateKey({ name: 'ECDSA', namedCurve: 'P-256' }, false, ['sign', 'verify']);
    const tx = db.transaction('333_keys', 'readwrite');
    tx.objectStore('333_keys').put({ publicKey: kp.publicKey, privateKey: kp.privateKey, ...meta, version: 4 }, 'identity_v3');
    await new Promise(r => { tx.oncomplete = r; });
    db.close();
  }, label);
  await page.goto(BASE + '/room?id=' + ROOM, { waitUntil: 'load', timeout: 15000 });
  for (let i = 0; i < 30; i++) {
    await new Promise(r => setTimeout(r, 500));
    if (await page.evaluate(() => window.wasmBridge?.ready)) break;
  }
  return { page, label, nodeId: nid };
}

const safeEval = async (page, fn, fallback = null) => {
  try { return await page.evaluate(fn); } catch (e) { return fallback; }
};

run(async () => {
  console.log(`=== mini-4tab: room=${ROOM} validators=[${V.join(',')}] ===`);
  const [browser] = await createBrowser({ headless: false });

  const tabs = [];
  const L = ['A', 'B', 'C', 'D'];
  for (let i = 0; i < 4; i++) {
    const t = await bootTab(browser, L[i], V[i]);
    console.log(`  ${L[i]} (node ${V[i]}) booted`);
    tabs.push(t);
    await new Promise(r => setTimeout(r, 1500));
  }

  console.log('[1] mesh + handshake settle 20s...');
  await new Promise(r => setTimeout(r, 20000));

  // single-observer: tabs[0]만 polling (Promise.all 제거 — D3 insight)
  const observer = tabs[0];
  const s0 = await safeEval(observer.page, () => window.wasmBridge?.roomState());
  console.log(`  observer ${observer.label} state:`, s0 ? JSON.stringify(s0) : 'null');

  console.log('[2] leader submitTransfer 2,77,0...');
  await safeEval(observer.page, async () => {
    try { await window.wasmBridge.submitTransfer(2, 77, 0); return 'ok'; }
    catch (e) { return 'err:' + e.message.slice(0, 80); }
  });

  console.log('[3] single-observer poll 30s for commit...');
  let commitAt = null;
  const startAt = Date.now();
  for (let i = 0; i < 60; i++) {
    await new Promise(r => setTimeout(r, 500));
    const s = await safeEval(observer.page, () => window.wasmBridge?.roomState());
    if (s?.committedBlocks > 0) {
      commitAt = Date.now() - startAt;
      console.log(`  ✅ COMMIT seen at t=${commitAt}ms: ${JSON.stringify(s)}`);
      break;
    }
    if ((i + 1) % 10 === 0) {
      console.log(`  t=${(i + 1) * 500}ms: view=${s?.view} committed=${s?.committedBlocks} leader=${s?.isLeader}`);
    }
  }

  console.log('[4] post-state (snapshot 1 tab at a time — no Promise.all)...');
  for (const t of tabs) {
    const s = await safeEval(t.page, () => window.wasmBridge?.roomState());
    console.log(`  ${t.label}: ${JSON.stringify(s)}`);
  }

  // wasmStats
  const stats0 = await safeEval(observer.page, () => window.__wasmStats?.());
  console.log(`  observer wasmStats: runs=${stats0?.totalRuns} errors=${stats0?.errorCount} lastErr=${stats0?.lastError || '-'}`);

  console.log('\n' + '='.repeat(40));
  console.log(`  Boots:       ✅ 4/4`);
  console.log(`  BFT commit:  ${commitAt !== null ? '✅ t=' + commitAt + 'ms' : '❌ no commit in 30s'}`);
  console.log('='.repeat(40));
  if (commitAt === null) process.exitCode = 1;
}, { timeout: 180_000 });
