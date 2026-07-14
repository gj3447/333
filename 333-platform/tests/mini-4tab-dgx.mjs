// KG: phase-post0-D-dgx-job-2026-04-14
// DGX k8s Job variant — Mac Chrome 대신 dgx-worker(arm64)에서 puppeteer-core 실행.
// zenika/alpine-chrome:with-puppeteer (multi-arch amd64+arm64).
// Mac host에 vite preview --host 0.0.0.0:4173 + socat 8333 0.0.0.0 open 필요.
import puppeteer from 'puppeteer-core';
const CHROME_BIN = process.env.CHROME_BIN || '/usr/bin/chromium-browser';

const BASE = process.env.BASE_URL || 'http://192.168.0.101:4173/333';
const ROOM = 'dgx' + Date.now().toString(36);
const V = [1, 2, 3, 4];

async function bootTab(browser, label, nid) {
  const ctx = await browser.createBrowserContext();
  const page = await ctx.newPage();
  page.on('pageerror', e => console.log(`  [${label} err] ${e.message.slice(0, 100)}`));
  await page.evaluateOnNewDocument((id, v, sig) => {
    delete navigator.serviceWorker;
    window.__333_nodeId = id;
    window.__333_validators = v;
    window.__333_signaling = sig; // getSignalingUrl override
  }, nid, V, process.env.SIGNALING_URL || 'ws://192.168.0.101:8333');
  await page.goto(BASE + '/onboard', { waitUntil: 'load', timeout: 20000 });
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
  await page.goto(BASE + '/room?id=' + ROOM, { waitUntil: 'load', timeout: 20000 });
  for (let i = 0; i < 30; i++) {
    await new Promise(r => setTimeout(r, 500));
    if (await page.evaluate(() => window.wasmBridge?.ready)) break;
  }
  return { page, label, nodeId: nid };
}

const safeEval = async (page, fn, fallback = null) => {
  try { return await page.evaluate(fn); } catch (e) { return fallback; }
};

(async () => {
  console.log(`=== mini-4tab-dgx: room=${ROOM} base=${BASE} ===`);
  // HTTP non-localhost에서 WebCrypto.subtle 활성화 — --unsafely-treat-insecure-origin-as-secure
  // KG: lesson-333-dgx-chromium-http-crypto-2026-04-14
  const origin = new URL(BASE).origin;
  const browser = await puppeteer.launch({
    executablePath: CHROME_BIN,
    headless: 'new',
    args: [
      '--no-sandbox', '--disable-setuid-sandbox',
      '--use-fake-ui-for-media-stream',
      '--use-fake-device-for-media-stream',
      '--disable-dev-shm-usage',
      '--disable-gpu',
      '--disable-web-security',
      `--unsafely-treat-insecure-origin-as-secure=${origin}`,
      `--user-data-dir=/tmp/chrome-profile-${process.pid}`,
    ],
  });
  const tabs = [];
  const L = ['A', 'B', 'C', 'D'];
  try {
    for (let i = 0; i < 4; i++) {
      console.log(`  boot ${L[i]} (node ${V[i]})...`);
      const t = await bootTab(browser, L[i], V[i]);
      tabs.push(t);
      await new Promise(r => setTimeout(r, 1500));
    }
    console.log(`  ✅ ${tabs.length}/4 tabs booted`);

    console.log('[1] mesh + handshake settle 20s...');
    await new Promise(r => setTimeout(r, 20000));

    const observer = tabs[0];
    const s0 = await safeEval(observer.page, () => window.wasmBridge?.roomState());
    console.log(`  ${observer.label} initial state:`, s0 ? JSON.stringify(s0) : 'null');

    console.log('[2] submitTransfer 2,77,0 (leader)...');
    const sub = await safeEval(observer.page, async () => {
      try { await window.wasmBridge.submitTransfer(2, 77, 0); return 'ok'; }
      catch (e) { return 'err:' + e.message.slice(0, 80); }
    });
    console.log(`  submit: ${sub}`);

    console.log('[3] single-observer poll 30s for commit...');
    let commitAt = null;
    const startAt = Date.now();
    for (let i = 0; i < 60; i++) {
      await new Promise(r => setTimeout(r, 500));
      const s = await safeEval(observer.page, () => window.wasmBridge?.roomState());
      if (s?.committedBlocks > 0) {
        commitAt = Date.now() - startAt;
        console.log(`  ✅ COMMIT at t=${commitAt}ms: ${JSON.stringify(s)}`);
        break;
      }
      if ((i + 1) % 10 === 0) {
        console.log(`  t=${(i + 1) * 500}ms v=${s?.view} c=${s?.committedBlocks} leader=${s?.isLeader}`);
      }
    }

    console.log('[4] final states (sequential, no Promise.all)...');
    for (const t of tabs) {
      const s = await safeEval(t.page, () => window.wasmBridge?.roomState());
      console.log(`  ${t.label}: ${JSON.stringify(s)}`);
    }
    const stats = await safeEval(observer.page, () => window.__wasmStats?.());
    console.log(`  observer stats: runs=${stats?.totalRuns} errs=${stats?.errorCount} lastErr=${stats?.lastError || '-'}`);

    console.log('\n' + '='.repeat(40));
    console.log(`  Boots:      ✅ 4/4`);
    console.log(`  BFT commit: ${commitAt !== null ? '✅ t=' + commitAt + 'ms' : '❌ no commit in 30s'}`);
    console.log('='.repeat(40));
    if (commitAt === null) process.exitCode = 1;
  } finally {
    await browser.close();
  }
})().catch(e => { console.error('FATAL:', e?.message); process.exit(1); });
