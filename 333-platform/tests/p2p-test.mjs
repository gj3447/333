// KG: TASK_333_E2E_P2PTest + TASK_333_E2E_BftVerify
// 333 Full E2E: WASM + Signaling + P2P + CRDT + BFT
// Production build (vite preview :4173)
import puppeteer from '../333-app/node_modules/puppeteer-core/lib/esm/puppeteer/puppeteer-core.js';

const CHROME = '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
const BASE = 'http://localhost:4173/333';
const ROOM_ID = 'e2e' + Date.now().toString(36);

// Safe evaluate — retries on context destruction (SPA hydration)
async function safe(page, fn, retries = 3) {
  for (let i = 0; i < retries; i++) {
    try { return await page.evaluate(fn); }
    catch (e) {
      if (i < retries - 1) await new Promise(r => setTimeout(r, 2000));
      else return null;
    }
  }
}

// Wait for condition with timeout
async function waitFor(page, fn, timeout = 10000) {
  const start = Date.now();
  while (Date.now() - start < timeout) {
    const result = await safe(page, fn);
    if (result) return result;
    await new Promise(r => setTimeout(r, 500));
  }
  return null;
}

async function test() {
  console.log('=== 333 Full E2E Test ===');
  console.log(`Room: ${ROOM_ID}\n`);

  const browser = await puppeteer.launch({
    executablePath: CHROME,
    headless: false, // HEADFUL for WebRTC (headless blocks UDP)
    args: [
      '--no-sandbox',
      '--use-fake-ui-for-media-stream',
      '--use-fake-device-for-media-stream',
      '--allow-running-insecure-content',
      '--disable-web-security',
    ]
  });

  const results = { wasm: false, signaling: false, tabA: false, tabB: false, crdt: false, bft: false };

  try {
    // ===== 1. WASM + Signaling =====
    console.log('[1] WASM + Signaling...');
    const setupPage = await browser.newPage();
    await setupPage.evaluateOnNewDocument(() => { delete navigator.serviceWorker; });
    await setupPage.goto(BASE + '/', { waitUntil: 'networkidle2', timeout: 15000 });

    // Generate real identity (Ed25519 or P-256 fallback)
    const identity = await setupPage.evaluate(async () => {
      try {
        // Import and generate real identity
        const mod = await import('/333/wasm/triple_three.js');
        await mod.default();

        // Try Ed25519 first
        try {
          const kp = await crypto.subtle.generateKey({ name: 'Ed25519' }, true, ['sign', 'verify']);
          const pub = new Uint8Array(await crypto.subtle.exportKey('raw', kp.publicKey));
          const hex = Array.from(pub).map(b => b.toString(16).padStart(2, '0')).join('');
          const peerId = hex.slice(0, 16);
          localStorage.setItem('333_identity_meta', JSON.stringify({
            peerId, username: 'e2etester', publicKeyHex: hex, algorithm: 'Ed25519'
          }));
          return { peerId, algorithm: 'Ed25519' };
        } catch {
          // P-256 fallback
          const kp = await crypto.subtle.generateKey({ name: 'ECDSA', namedCurve: 'P-256' }, true, ['sign', 'verify']);
          const pub = new Uint8Array(await crypto.subtle.exportKey('raw', kp.publicKey));
          const hex = Array.from(pub).map(b => b.toString(16).padStart(2, '0')).join('');
          const peerId = hex.slice(0, 16);
          localStorage.setItem('333_identity_meta', JSON.stringify({
            peerId, username: 'e2etester', publicKeyHex: hex, algorithm: 'P-256'
          }));
          return { peerId, algorithm: 'P-256' };
        }
      } catch (e) {
        return { error: e.message };
      }
    });
    console.log(`  Identity: ${JSON.stringify(identity)}`);

    // WASM
    const health = await setupPage.evaluate(async () => {
      const m = await import('/333/wasm/triple_three.js');
      await m.default();
      return m.health();
    });
    results.wasm = health?.includes('ok');
    console.log(`  WASM: ${results.wasm ? '✅' : '❌'} ${health}`);

    // Signaling
    const ws = await setupPage.evaluate(() => new Promise(r => {
      try {
        const w = new WebSocket('ws://localhost:8333');
        w.onopen = () => { w.close(); r(true); };
        w.onerror = () => r(false);
        setTimeout(() => r(false), 3000);
      } catch { r(false); }
    }));
    results.signaling = ws;
    console.log(`  Signaling: ${ws ? '✅' : '❌'}`);
    await setupPage.close();

    // ===== 2. Tab A — Create Room (separate browser context) =====
    console.log('\n[2] Tab A — Room...');
    const ctxA = await browser.createBrowserContext();
    const pageA = await ctxA.newPage();
    pageA.on('pageerror', e => console.log('  [A err]', e.message.slice(0, 80)));
    pageA.on('console', m => { if (m.text().includes('333-')) console.log('  [A]', m.text().slice(0, 120)); });
    // KG: lesson-333-bft-keyring-exchange-2026-04-14 — Tab A is validator 1
    await pageA.evaluateOnNewDocument(() => {
      delete navigator.serviceWorker;
      window.__333_nodeId = 1;
      window.__333_validators = [1, 2];
    });
    // Set identity in this context too
    await pageA.goto(BASE + '/onboard', { waitUntil: 'load', timeout: 10000 });
    await new Promise(r => setTimeout(r, 2000)); // wait for SPA hydration
    // Generate real identity via the app's own onboard flow
    const idA = await safe(pageA, async () => {
      // Use the app's generateIdentity
      const { generateIdentity } = await import('/333/src/lib/identity.ts');
      const id = await generateIdentity('tabA');
      return id?.peerId;
    });
    // Fallback: set localStorage + IndexedDB manually
    if (!idA) {
      await pageA.evaluate(async () => {
        const meta = { peerId: 'tabA-' + Date.now().toString(36), username: 'tabA', publicKeyHex: '01'.repeat(32), algorithm: 'Ed25519' };
        localStorage.setItem('333_identity_meta', JSON.stringify(meta));
        // Create a minimal IndexedDB entry so loadIdentity() doesn't return null
        const db = await new Promise((resolve, reject) => {
          const req = indexedDB.open('333_identity_db', 1);
          req.onupgradeneeded = () => req.result.createObjectStore('333_keys');
          req.onsuccess = () => resolve(req.result);
          req.onerror = () => reject(req.error);
        });
        const kp = await crypto.subtle.generateKey({ name: 'ECDSA', namedCurve: 'P-256' }, false, ['sign', 'verify']);
        const tx = db.transaction('333_keys', 'readwrite');
        tx.objectStore('333_keys').put({
          publicKey: kp.publicKey, privateKey: kp.privateKey,
          ...meta, version: 4
        }, 'identity_v3');
        await new Promise(r => { tx.oncomplete = r; });
        db.close();
      });
    }
    console.log(`  Tab A identity: ${idA || 'fallback IDB'}`);
    await pageA.goto(BASE + '/room?id=' + ROOM_ID, { waitUntil: 'load', timeout: 15000 });

    // Wait for SPA hydration + WASM init
    console.log('  Waiting for WASM bridge...');
    const bridgeA = await waitFor(pageA, () => {
      if (window.wasmBridge?.ready) return { ready: true, world: window.wasmBridge.worldSize() };
      return null;
    }, 15000);

    results.tabA = !!bridgeA?.ready;
    console.log(`  Tab A bridge: ${results.tabA ? '✅' : '❌'} ${JSON.stringify(bridgeA)}`);

    // ===== 3. Tab B — Join Room (separate context) =====
    console.log('\n[3] Tab B — Join...');
    const ctxB = await browser.createBrowserContext();
    const pageB = await ctxB.newPage();
    pageB.on('pageerror', e => console.log('  [B err]', e.message.slice(0, 80)));
    pageB.on('console', m => { if (m.text().includes('333-')) console.log('  [B]', m.text().slice(0, 120)); });
    // KG: lesson-333-bft-keyring-exchange-2026-04-14 — Tab B is validator 2
    await pageB.evaluateOnNewDocument(() => {
      delete navigator.serviceWorker;
      window.__333_nodeId = 2;
      window.__333_validators = [1, 2];
    });
    await pageB.goto(BASE + '/onboard', { waitUntil: 'load', timeout: 10000 });
    await new Promise(r => setTimeout(r, 2000));
    await pageB.evaluate(async () => {
      const meta = { peerId: 'tabB-' + Date.now().toString(36), username: 'tabB', publicKeyHex: '02'.repeat(32), algorithm: 'P-256' };
      localStorage.setItem('333_identity_meta', JSON.stringify(meta));
      const db = await new Promise((resolve, reject) => {
        const req = indexedDB.open('333_identity_db', 1);
        req.onupgradeneeded = () => req.result.createObjectStore('333_keys');
        req.onsuccess = () => resolve(req.result);
        req.onerror = () => reject(req.error);
      });
      const kp = await crypto.subtle.generateKey({ name: 'ECDSA', namedCurve: 'P-256' }, false, ['sign', 'verify']);
      const tx = db.transaction('333_keys', 'readwrite');
      tx.objectStore('333_keys').put({ publicKey: kp.publicKey, privateKey: kp.privateKey, ...meta, version: 4 }, 'identity_v3');
      await new Promise(r => { tx.oncomplete = r; });
      db.close();
    });
    console.log('  Tab B identity set');
    await pageB.goto(BASE + '/room?id=' + ROOM_ID, { waitUntil: 'load', timeout: 15000 });

    const bridgeB = await waitFor(pageB, () => {
      if (window.wasmBridge?.ready) return { ready: true, world: window.wasmBridge.worldSize() };
      return null;
    }, 15000);

    results.tabB = !!bridgeB?.ready;
    console.log(`  Tab B bridge: ${results.tabB ? '✅' : '❌'} ${JSON.stringify(bridgeB)}`);

    // Debug: check both pages
    const debugA = await safe(pageA, () => ({ url: location.href, text: document.body.innerText.slice(0, 150) }));
    const debugB = await safe(pageB, () => ({ url: location.href, text: document.body.innerText.slice(0, 150) }));
    console.log(`  Tab A: ${JSON.stringify(debugA)}`);
    console.log(`  Tab B: ${JSON.stringify(debugB)}`);

    // Direct signaling probe: join room and check peers
    console.log('\n[3.5] Signaling probe...');
    const probe = await pageB.evaluate((rid) => new Promise(r => {
      const ws = new WebSocket('ws://localhost:8333');
      const msgs = [];
      ws.onopen = () => ws.send(JSON.stringify({ type: 'join', room: rid, peerId: 'probe-' + Date.now() }));
      ws.onmessage = (e) => { try { msgs.push(JSON.parse(e.data)); } catch {} if (msgs.length >= 2) { ws.close(); r(msgs); } };
      ws.onerror = (e) => r([{error: 'ws error'}]);
      setTimeout(() => { ws.close(); r(msgs); }, 3000);
    }), ROOM_ID).catch(() => null);
    console.log(`  Signaling probe: ${JSON.stringify(probe)}`);

    console.log('\n[4] P2P Connection...');

    // Check if tabs joined signaling
    const sigA = await safe(pageA, () => {
      const el = document.body.innerText;
      return {
        hasRoom: el.includes(ROOM_ID) || el.includes('Room'),
        msgLog: el.match(/peer.*\d+B/g)?.slice(0, 3) || [],
        status: el.match(/(connected|connecting|disconnected)/i)?.[0] || 'unknown',
        myId: el.match(/peer-\w+/)?.[0] || 'none'
      };
    });
    console.log(`  Tab A signaling: ${JSON.stringify(sigA)}`);

    // Wait more for WebRTC
    // Wait longer for ICE + DataChannel open
    console.log('  Waiting 10s for ICE negotiation + DataChannel...');
    await new Promise(r => setTimeout(r, 10000));

    const peersA = await safe(pageA, () => document.body.innerText.match(/(\d+)\s*PEERS/)?.[1] || '0');
    const peersB = await safe(pageB, () => document.body.innerText.match(/(\d+)\s*PEERS/)?.[1] || '0');
    const statusA = await safe(pageA, () => document.body.innerText.match(/(connected|connecting)/i)?.[0] || 'unknown');
    const statusB = await safe(pageB, () => document.body.innerText.match(/(connected|connecting)/i)?.[0] || 'unknown');
    console.log(`  Peers A: ${peersA}, B: ${peersB}`);
    console.log(`  Status A: ${statusA}, B: ${statusB}`);

    // ===== 4. CRDT Sync =====
    if (results.tabA && results.tabB) {
      console.log('\n[5] CRDT Sync...');
      await safe(pageA, () => window.wasmBridge?.placeBlock('0,0', 'stone'));
      console.log('  A placed stone at 0,0');

      // Wait for sync (poll_sync 200ms + WebRTC)
      await new Promise(r => setTimeout(r, 1500));

      const blockB = await safe(pageB, () => window.wasmBridge?.getBlock('0,0'));
      const worldA = await safe(pageA, () => window.wasmBridge?.worldSize());
      const worldB = await safe(pageB, () => window.wasmBridge?.worldSize());

      console.log(`  B sees: ${blockB}, worldA=${worldA}, worldB=${worldB}`);
      results.crdt = blockB === 'stone';
      console.log(`  CRDT: ${results.crdt ? '✅ SYNCED!' : '❌ not synced (P2P may not be connected)'}`);

      // ===== 5. BFT Consensus =====
      console.log('\n[6] BFT Consensus...');
      const before = await safe(pageA, () => window.wasmBridge?.roomState());
      console.log(`  Before: ${JSON.stringify(before)}`);

      await safe(pageA, () => window.wasmBridge?.submitTransfer(2, 100, 0));
      console.log('  Transfer submitted');

      // Poll up to 10s for commit
      let committedA = 0, committedB = 0;
      for (let i = 0; i < 20; i++) {
        await new Promise(r => setTimeout(r, 500));
        committedA = await safe(pageA, () => window.wasmBridge?.roomState()?.committedBlocks ?? 0) ?? 0;
        committedB = await safe(pageB, () => window.wasmBridge?.roomState()?.committedBlocks ?? 0) ?? 0;
        if (committedA > 0 || committedB > 0) break;
      }
      const stateA = await safe(pageA, () => window.wasmBridge?.roomState());
      const stateB = await safe(pageB, () => window.wasmBridge?.roomState());
      const bal1 = await safe(pageA, () => window.wasmBridge?.balance(1));
      const bal2 = await safe(pageA, () => window.wasmBridge?.balance(2));
      console.log(`  State A: ${JSON.stringify(stateA)}`);
      console.log(`  State B: ${JSON.stringify(stateB)}`);
      console.log(`  bal1=${bal1}, bal2=${bal2}`);

      results.bft = (committedA > 0) || (committedB > 0);
      console.log(`  BFT: ${results.bft ? '✅ COMMITTED!' : '❌ no commit (need 2+ validators voting)'}`);
    }

    // ===== Summary =====
    console.log('\n' + '='.repeat(40));
    console.log('  WASM:       ' + (results.wasm ? '✅' : '❌'));
    console.log('  Signaling:  ' + (results.signaling ? '✅' : '❌'));
    console.log('  Tab A WASM: ' + (results.tabA ? '✅' : '❌'));
    console.log('  Tab B WASM: ' + (results.tabB ? '✅' : '❌'));
    console.log('  CRDT Sync:  ' + (results.crdt ? '✅' : '❌'));
    console.log('  BFT:        ' + (results.bft ? '✅' : '❌'));
    console.log('='.repeat(40));

    const passed = Object.values(results).filter(Boolean).length;
    console.log(`\n${passed}/6 tests passed`);

  } catch (err) {
    console.error('\nFATAL:', err.message);
  } finally {
    await browser.close();
  }
}

test().catch(console.error);
