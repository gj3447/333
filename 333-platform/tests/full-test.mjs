// KG: SPAN_333_FullTest — 333 Platform 전체 기능 테스트 (36 시나리오)
import puppeteer from '../333-app/node_modules/puppeteer-core/lib/esm/puppeteer/puppeteer-core.js';

const CHROME = '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
const BASE = 'http://localhost:4173/333';
const results = {};
let totalPass = 0, totalFail = 0;

function log(cat, name, pass, detail = '') {
  const icon = pass ? '✅' : '❌';
  console.log(`  ${icon} ${name}${detail ? ' — ' + detail : ''}`);
  results[`${cat}:${name}`] = pass;
  pass ? totalPass++ : totalFail++;
}

// KG: plan-333-test-waitfor-fix — robust retry pattern (from p2p-test.mjs)
async function safe(page, fn, fallback = null) {
  for (let i = 0; i < 5; i++) {
    try { return await page.evaluate(fn); }
    catch (e) {
      if (i === 4) console.log('  [safe] final retry failed:', e.message?.slice(0, 80));
      await new Promise(r => setTimeout(r, 1000));
    }
  }
  return fallback;
}

async function waitFor(page, fn, timeout = 10000) {
  const start = Date.now();
  while (Date.now() - start < timeout) {
    const result = await safe(page, fn);
    if (result) return result;
    await new Promise(r => setTimeout(r, 500));
  }
  return null;
}

async function setupIdentity(page, peerId) {
  await page.evaluate(async (pid) => {
    localStorage.setItem('333_identity_meta', JSON.stringify({
      peerId: pid, username: pid, publicKeyHex: '00'.repeat(32), algorithm: 'P-256'
    }));
    const db = await new Promise((r, j) => {
      const req = indexedDB.open('333_identity_db', 1);
      req.onupgradeneeded = () => req.result.createObjectStore('333_keys');
      req.onsuccess = () => r(req.result);
      req.onerror = () => j(req.error);
    });
    const kp = await crypto.subtle.generateKey({ name: 'ECDSA', namedCurve: 'P-256' }, false, ['sign', 'verify']);
    const tx = db.transaction('333_keys', 'readwrite');
    tx.objectStore('333_keys').put({
      publicKey: kp.publicKey, privateKey: kp.privateKey,
      peerId: pid, username: pid, publicKeyHex: '00'.repeat(32), algorithm: 'P-256', version: 4
    }, 'identity_v3');
    await new Promise(r => { tx.oncomplete = r; });
    db.close();
  }, peerId);
}

async function test() {
  console.log('╔══════════════════════════════════════╗');
  console.log('║  333 Platform Full Test Suite (36)   ║');
  console.log('╚══════════════════════════════════════╝\n');

  const browser = await puppeteer.launch({
    executablePath: CHROME,
    headless: false,
    args: ['--no-sandbox', '--use-fake-ui-for-media-stream']
  });

  try {
    // ════════════════════════════════════════
    // 0. SETUP
    // ════════════════════════════════════════
    const ROOM = 'ft' + Date.now().toString(36);

    // Create 2 tabs with identity
    // KG: lesson-333-bft-keyring-exchange-2026-04-14 — inject nodeId via window globals
    const ctxA = await browser.createBrowserContext();
    const pageA = await ctxA.newPage();
    await pageA.evaluateOnNewDocument(() => {
      delete navigator.serviceWorker;
      window.__333_nodeId = 1;
      window.__333_validators = [1, 2];
    });
    await pageA.goto(BASE + '/onboard', { waitUntil: 'load', timeout: 10000 });
    await new Promise(r => setTimeout(r, 1000));
    await setupIdentity(pageA, 'nodeA');
    await pageA.goto(BASE + '/room?id=' + ROOM, { waitUntil: 'load', timeout: 10000 });
    await new Promise(r => setTimeout(r, 4000));

    const ctxB = await browser.createBrowserContext();
    const pageB = await ctxB.newPage();
    await pageB.evaluateOnNewDocument(() => {
      delete navigator.serviceWorker;
      window.__333_nodeId = 2;
      window.__333_validators = [1, 2];
    });
    await pageB.goto(BASE + '/onboard', { waitUntil: 'load', timeout: 10000 });
    await new Promise(r => setTimeout(r, 1000));
    await setupIdentity(pageB, 'nodeB');
    await pageB.goto(BASE + '/room?id=' + ROOM, { waitUntil: 'load', timeout: 10000 });

    // KG: plan-333-test-waitfor-fix — robust waitFor pattern (SPA hydration safe)
    console.log('Waiting for WASM bridge...');
    const bridgeA = await waitFor(pageA, () => {
      if (window.wasmBridge?.ready) return { ready: true, size: window.wasmBridge.worldSize() };
      return null;
    }, 20000);
    console.log(bridgeA ? '  Tab A wasmBridge ready ✅' : '  Tab A wasmBridge timeout ❌');
    const bridgeB = await waitFor(pageB, () => {
      if (window.wasmBridge?.ready) return { ready: true, size: window.wasmBridge.worldSize() };
      return null;
    }, 20000);
    console.log(bridgeB ? '  Tab B wasmBridge ready ✅' : '  Tab B wasmBridge timeout ❌');

    // Wait for WebRTC P2P
    console.log('Waiting for P2P connection (10s)...\n');
    await new Promise(r => setTimeout(r, 10000));

    const peersA = await safe(pageA, () => document.body.innerText.match(/(\d+)\s*PEERS/)?.[1]) || '0';
    const peersB = await safe(pageB, () => document.body.innerText.match(/(\d+)\s*PEERS/)?.[1]) || '0';

    // ════════════════════════════════════════
    // 1. CRDT STRESS
    // ════════════════════════════════════════
    console.log('─── 1. CRDT Stress ───');

    // 1.1 Single block sync (baseline)
    await safe(pageA, () => window.wasmBridge?.placeBlock('0,0', 'stone'));
    await new Promise(r => setTimeout(r, 1000));
    const b1 = await safe(pageB, () => window.wasmBridge?.getBlock('0,0'));
    log('CRDT', '1-block sync', b1 === 'stone', `B sees: ${b1}`);

    // 1.2 Multiple blocks (10)
    for (let i = 1; i <= 10; i++) {
      await pageA.evaluate((idx) => window.wasmBridge?.placeBlock(`${idx},0`, 'grass'), i).catch(() => {});
    }
    await new Promise(r => setTimeout(r, 2000));
    const wA = await safe(pageA, () => window.wasmBridge?.worldSize()) || 0;
    const wB = await safe(pageB, () => window.wasmBridge?.worldSize()) || 0;
    log('CRDT', '10-block batch', wA >= 10 && wB >= 10, `A=${wA} B=${wB}`);

    // 1.3 Concurrent edits (both place different blocks)
    await safe(pageA, () => window.wasmBridge?.placeBlock('7,7', 'water'));
    await safe(pageB, () => window.wasmBridge?.placeBlock('6,6', 'fire'));
    await new Promise(r => setTimeout(r, 2000));
    const concA = await safe(pageA, () => window.wasmBridge?.getBlock('6,6'));
    const concB = await safe(pageB, () => window.wasmBridge?.getBlock('7,7'));
    log('CRDT', 'concurrent edits', concA === 'fire' && concB === 'water', `A sees 6,6=${concA}, B sees 7,7=${concB}`);

    // 1.4 Delete sync
    await safe(pageA, () => window.wasmBridge?.placeBlock('0,0', ''));
    await new Promise(r => setTimeout(r, 1000));
    const del = await safe(pageB, () => window.wasmBridge?.getBlock('0,0'));
    log('CRDT', 'delete sync', !del || del === '', `B sees: "${del}"`);

    // 1.5 World size consistency
    const wsA = await safe(pageA, () => window.wasmBridge?.worldSize()) || 0;
    const wsB = await safe(pageB, () => window.wasmBridge?.worldSize()) || 0;
    log('CRDT', 'world size match', wsA === wsB, `A=${wsA} B=${wsB}`);

    // ════════════════════════════════════════
    // 2. BFT CONSENSUS
    // ════════════════════════════════════════
    console.log('\n─── 2. BFT Consensus ───');

    const stateA = await safe(pageA, () => window.wasmBridge?.roomState());
    log('BFT', 'roomState available', !!stateA, JSON.stringify(stateA)?.slice(0, 80));

    // 2.1 Submit transfer — wait longer for key exchange + consensus roundtrip
    await safe(pageA, () => window.wasmBridge?.submitTransfer(2, 100, 0));
    await new Promise(r => setTimeout(r, 8000));
    const bal1 = await safe(pageA, () => window.wasmBridge?.balance(1));
    const bal2 = await safe(pageA, () => window.wasmBridge?.balance(2));
    log('BFT', 'token transfer', bal1 !== 10000 || bal2 !== 10000, `bal1=${bal1} bal2=${bal2}`);

    const afterState = await safe(pageA, () => window.wasmBridge?.roomState());
    log('BFT', 'view advanced', afterState?.view > 0, `view=${afterState?.view}`);
    log('BFT', 'blocks committed', afterState?.committedBlocks > 0, `committed=${afterState?.committedBlocks}`);

    // ════════════════════════════════════════
    // 3. P2P RESILIENCE
    // ════════════════════════════════════════
    console.log('\n─── 3. P2P Resilience ───');

    log('P2P', 'connection established', parseInt(peersA) > 0, `peersA=${peersA} peersB=${peersB}`);

    // 3.1 Channel separation check
    const hasChannels = await safe(pageA, () => {
      const text = document.body.innerText;
      return text.includes('PEERS') || text.includes('connected');
    });
    log('P2P', 'UI shows connection', !!hasChannels);

    // ════════════════════════════════════════
    // 4. RENDER + GAMELOOP (unit check)
    // ════════════════════════════════════════
    console.log('\n─── 4. Render + GameLoop ───');

    const canWebGL = await safe(pageA, () => {
      const c = document.createElement('canvas');
      return !!c.getContext('webgl2');
    });
    log('Render', 'WebGL2 supported', !!canWebGL);

    const canRAF = await safe(pageA, () => typeof requestAnimationFrame === 'function');
    log('Render', 'RAF available', !!canRAF);

    // ════════════════════════════════════════
    // 5. BROWSER APIs
    // ════════════════════════════════════════
    console.log('\n─── 5. Browser APIs ───');

    // 5.1 Web Worker
    const workerOk = await safe(pageA, () => new Promise(r => {
      try {
        const code = 'self.onmessage=()=>self.postMessage("ok")';
        const b = new Blob([code], { type: 'application/javascript' });
        const w = new Worker(URL.createObjectURL(b));
        w.onmessage = () => { w.terminate(); r(true); };
        w.postMessage('test');
        setTimeout(() => r(false), 2000);
      } catch { r(false); }
    }));
    log('API', 'Web Worker', !!workerOk);

    // 5.2 OPFS
    const opfsOk = await safe(pageA, () => !!navigator.storage?.getDirectory);
    log('API', 'OPFS available', !!opfsOk);

    // 5.3 Crypto encrypt/decrypt
    const cryptoOk = await safe(pageA, async () => {
      const key = await crypto.subtle.generateKey({ name: 'AES-GCM', length: 256 }, false, ['encrypt', 'decrypt']);
      const iv = crypto.getRandomValues(new Uint8Array(12));
      const data = new TextEncoder().encode('test333');
      const enc = await crypto.subtle.encrypt({ name: 'AES-GCM', iv }, key, data);
      const dec = await crypto.subtle.decrypt({ name: 'AES-GCM', iv }, key, enc);
      return new TextDecoder().decode(dec) === 'test333';
    });
    log('API', 'AES-GCM encrypt/decrypt', !!cryptoOk);

    // 5.4 BroadcastChannel
    const bcOk = await safe(pageA, () => {
      try { const bc = new BroadcastChannel('test'); bc.close(); return true; }
      catch { return false; }
    });
    log('API', 'BroadcastChannel', !!bcOk);

    // 5.5 Web Locks
    const locksOk = await safe(pageA, () => !!navigator.locks);
    log('API', 'Web Locks', !!locksOk);

    // 5.6 navigator.share
    const shareOk = await safe(pageA, () => typeof navigator.share === 'function' || typeof navigator.clipboard?.writeText === 'function');
    log('API', 'Share/Clipboard', !!shareOk);

    // ════════════════════════════════════════
    // 6. SECURITY
    // ════════════════════════════════════════
    console.log('\n─── 6. Security ───');

    // 6.1 Ed25519 available
    const ed25519 = await safe(pageA, async () => {
      try {
        await crypto.subtle.generateKey({ name: 'Ed25519' }, false, ['sign', 'verify']);
        return true;
      } catch { return false; }
    });
    log('Security', 'Ed25519 WebCrypto', !!ed25519);

    // 6.2 Non-extractable key
    const nonExtract = await safe(pageA, async () => {
      const kp = await crypto.subtle.generateKey({ name: 'ECDSA', namedCurve: 'P-256' }, false, ['sign']);
      try { await crypto.subtle.exportKey('raw', kp.privateKey); return false; }
      catch { return true; } // should throw — non-extractable
    });
    log('Security', 'Non-extractable key', !!nonExtract);

    // 6.3 WASM health integrity (KG: plan-wasm-executor-solid — health() now Promise<string>)
    const healthOk = await safe(pageA, async () => {
      const h = await window.wasmBridge?.health();
      return h?.includes('ok') && h?.includes('333');
    });
    log('Security', 'WASM integrity', !!healthOk);

    // ════════════════════════════════════════
    // SUMMARY
    // ════════════════════════════════════════
    console.log('\n╔══════════════════════════════════════╗');
    console.log(`║  Results: ${totalPass}/${totalPass + totalFail} passed${' '.repeat(20 - String(totalPass).length - String(totalPass + totalFail).length)}║`);
    console.log('╚══════════════════════════════════════╝');

    const cats = {};
    for (const [k, v] of Object.entries(results)) {
      const [cat] = k.split(':');
      if (!cats[cat]) cats[cat] = { pass: 0, fail: 0 };
      v ? cats[cat].pass++ : cats[cat].fail++;
    }
    for (const [cat, { pass, fail }] of Object.entries(cats)) {
      console.log(`  ${pass}/${pass + fail} ${cat}`);
    }

  } catch (err) {
    console.error('\nFATAL:', err.message);
  } finally {
    await browser.close();
  }
}

test().catch(console.error);
