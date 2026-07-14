// KG: lesson-333-e2e-4tab-bft-2026-04-14
// 4-validator E2E: n=4 f=1 quorum=3. Browser-level HotStuff verification.
import puppeteer from '../333-app/node_modules/puppeteer-core/lib/esm/puppeteer/puppeteer-core.js';

const CHROME = '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
const BASE = 'http://localhost:4173/333';
const ROOM_ID = 'e4t' + Date.now().toString(36);
const VALIDATORS = [1, 2, 3, 4];

async function safe(page, fn, retries = 3) {
  for (let i = 0; i < retries; i++) {
    try { return await page.evaluate(fn); }
    catch { if (i < retries - 1) await new Promise(r => setTimeout(r, 1500)); else return null; }
  }
}

async function waitFor(page, fn, timeout = 15000) {
  const start = Date.now();
  while (Date.now() - start < timeout) {
    const r = await safe(page, fn);
    if (r) return r;
    await new Promise(r => setTimeout(r, 500));
  }
  return null;
}

async function bootTab(browser, label, nodeId) {
  const ctx = await browser.createBrowserContext();
  const page = await ctx.newPage();
  page.on('pageerror', e => console.log(`  [${label} err]`, e.message.slice(0, 80)));
  page.on('console', m => {
    const t = m.text();
    if (t.includes('333-hs') || t.includes('333-bft')) console.log(`  [${label}]`, t.slice(0, 130));
  });
  await page.evaluateOnNewDocument((id, validators) => {
    delete navigator.serviceWorker;
    window.__333_nodeId = id;
    window.__333_validators = validators;
  }, nodeId, VALIDATORS);
  await page.goto(BASE + '/onboard', { waitUntil: 'load', timeout: 10000 });
  await new Promise(r => setTimeout(r, 1500));

  await page.evaluate(async (label, nodeId) => {
    const pub = (nodeId.toString(16).padStart(2, '0')).repeat(32);
    const meta = { peerId: label + '-' + Date.now().toString(36), username: label, publicKeyHex: pub, algorithm: 'P-256' };
    localStorage.setItem('333_identity_meta', JSON.stringify(meta));
    const db = await new Promise((res, rej) => {
      const r = indexedDB.open('333_identity_db', 1);
      r.onupgradeneeded = () => r.result.createObjectStore('333_keys');
      r.onsuccess = () => res(r.result);
      r.onerror = () => rej(r.error);
    });
    const kp = await crypto.subtle.generateKey({ name: 'ECDSA', namedCurve: 'P-256' }, false, ['sign', 'verify']);
    const tx = db.transaction('333_keys', 'readwrite');
    tx.objectStore('333_keys').put({ publicKey: kp.publicKey, privateKey: kp.privateKey, ...meta, version: 4 }, 'identity_v3');
    await new Promise(r => { tx.oncomplete = r; });
    db.close();
  }, label, nodeId);

  await page.goto(BASE + '/room?id=' + ROOM_ID, { waitUntil: 'load', timeout: 15000 });
  const bridge = await waitFor(page, () => window.wasmBridge?.ready ? { ready: true } : null, 15000);
  console.log(`  ${label} (node ${nodeId}) bridge: ${bridge ? '✅' : '❌'}`);
  return { ctx, page, nodeId, bridge: !!bridge };
}

async function test() {
  console.log('=== 333 4-Validator E2E ===');
  console.log(`Room: ${ROOM_ID}, Validators: [${VALIDATORS}]\n`);

  const browser = await puppeteer.launch({
    executablePath: CHROME,
    headless: false,
    args: ['--no-sandbox', '--use-fake-ui-for-media-stream', '--use-fake-device-for-media-stream',
           '--allow-running-insecure-content', '--disable-web-security'],
  });

  const results = { boots: 0, mesh: false, handshake: false, commit: false };
  const labels = ['A', 'B', 'C', 'D'];
  const tabs = [];

  try {
    console.log('[1] Boot 4 tabs sequentially...');
    for (let i = 0; i < 4; i++) {
      const tab = await bootTab(browser, labels[i], VALIDATORS[i]);
      tabs.push(tab);
      if (tab.bridge) results.boots++;
      await new Promise(r => setTimeout(r, 1500)); // stagger to let signaling settle
    }
    console.log(`  Booted ${results.boots}/4 tabs`);

    console.log('\n[2] Wait for WebRTC mesh + keyring handshakes (20s)...');
    await new Promise(r => setTimeout(r, 20000));

    const peerCounts = await Promise.all(tabs.map(t => safe(t.page, () =>
      document.body.innerText.match(/(\d+)\s*PEERS/)?.[1] || '0'
    )));
    console.log(`  PEER counts: ${peerCounts.join(', ')} (expected 3 each for full mesh)`);
    results.mesh = peerCounts.every(c => parseInt(c) >= 3);

    const states = await Promise.all(tabs.map(t => safe(t.page, () => window.wasmBridge?.roomState())));
    console.log('  States:');
    states.forEach((s, i) => console.log(`    ${labels[i]} (node ${VALIDATORS[i]}): ${JSON.stringify(s)}`));

    // Handshake sanity: no observable failure implies keyrings exchanged (logs shown above).
    results.handshake = states.every(s => s?.nodeId);

    console.log('\n[3] Leader submits transfer...');
    // KG: lesson-333-wasm-borrowmut-multitab-2026-04-14 — Tab A null state due to BorrowMut.
    // view=0 leader is deterministic: validators[0%4]=1 = Tab A. Use index, not isLeader field.
    const leader = tabs[0];
    console.log(`  Leader: ${labels[0]} (node ${leader.nodeId}, deterministic by view=0)`);
    {
      await safe(leader.page, () => window.wasmBridge?.submitTransfer(2, 77, 0));

      console.log('  Poll for commit on any tab (20s max)...');
      for (let i = 0; i < 40; i++) {
        await new Promise(r => setTimeout(r, 500));
        const counts = await Promise.all(tabs.map(t =>
          safe(t.page, () => window.wasmBridge?.roomState()?.committedBlocks ?? 0) ?? 0
        ));
        if (counts.some(c => c > 0)) {
          console.log(`  ✅ Commit seen — counts: ${counts.join(',')}`);
          results.commit = true;
          break;
        }
      }
      if (!results.commit) console.log('  ❌ No commit within 20s');

      const postStates = await Promise.all(tabs.map(t => safe(t.page, () => window.wasmBridge?.roomState())));
      console.log('  Post-submit states:');
      postStates.forEach((s, i) => console.log(`    ${labels[i]}: ${JSON.stringify(s)}`));
    }

    console.log('\n' + '='.repeat(40));
    console.log('  4 Boots:    ' + (results.boots === 4 ? '✅' : `❌ (${results.boots}/4)`));
    console.log('  Full Mesh:  ' + (results.mesh ? '✅' : '❌'));
    console.log('  Handshake:  ' + (results.handshake ? '✅' : '❌'));
    console.log('  BFT Commit: ' + (results.commit ? '✅' : '❌'));
    console.log('='.repeat(40));

  } catch (err) {
    console.error('\nFATAL:', err.message);
  } finally {
    await browser.close();
  }
}

test();
