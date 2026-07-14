// 333 Platform — Browser E2E Test via puppeteer-core
// Tests: page load, identity creation, room creation, WASM health
import puppeteer from '../333-app/node_modules/puppeteer-core/lib/esm/puppeteer/puppeteer-core.js';

const CHROME = '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
const BASE = 'http://localhost:5173/333';

async function test() {
  console.log('=== 333 Browser E2E Test ===\n');

  const browser = await puppeteer.launch({
    executablePath: CHROME,
    headless: 'shell',
    args: ['--no-sandbox', '--disable-setuid-sandbox']
  });

  try {
    // TEST 1: Main page loads
    console.log('TEST 1: Main page load...');
    const page = await browser.newPage();
    page.on('console', msg => console.log('  [browser]', msg.text()));
    page.on('pageerror', err => console.log('  [ERROR]', err.message));

    await page.goto(`${BASE}/`, { waitUntil: 'networkidle0', timeout: 15000 });
    const title = await page.title();
    console.log(`  Title: ${title}`);
    console.log('  ✅ Main page loaded\n');

    // TEST 2: WASM loads
    console.log('TEST 2: WASM health check...');
    const health = await page.evaluate(async () => {
      try {
        const mod = await import('/333/wasm/triple_three.js');
        await mod.default();
        return mod.health();
      } catch (e) {
        return `ERROR: ${e.message}`;
      }
    });
    console.log(`  Health: ${health}`);
    if (health.includes('ok')) {
      console.log('  ✅ WASM loaded\n');
    } else {
      console.log('  ❌ WASM failed\n');
    }

    // TEST 3: Identity creation (Ed25519 or P-256 fallback)
    console.log('TEST 3: Identity creation...');
    const identity = await page.evaluate(async () => {
      try {
        // Clear old identity
        localStorage.removeItem('333_identity_meta');
        localStorage.removeItem('333_identity');

        const { generateIdentity } = await import('/333/@fs/Users/lagyeongjun/CD/SERVER/07_PROJECTS/333-platform/333-app/src/lib/identity.ts');
        const id = await generateIdentity('testuser');
        return { peerId: id.peerId, algorithm: id.algorithm, pubKeyLen: id.publicKeyHex.length };
      } catch (e) {
        return { error: e.message };
      }
    });
    console.log(`  Identity: ${JSON.stringify(identity)}`);
    if (identity.peerId) {
      console.log(`  ✅ Identity created (${identity.algorithm})\n`);
    } else {
      console.log(`  ❌ Identity failed: ${identity.error}\n`);
    }

    // TEST 4: Onboard page loads
    console.log('TEST 4: Onboard page...');
    await page.goto(`${BASE}/onboard`, { waitUntil: 'networkidle0', timeout: 15000 });
    const onboardContent = await page.evaluate(() => document.body.innerText.slice(0, 200));
    console.log(`  Content: ${onboardContent.slice(0, 100)}...`);
    console.log('  ✅ Onboard page loaded\n');

    // TEST 5: Room page loads
    console.log('TEST 5: Room page...');
    await page.goto(`${BASE}/room`, { waitUntil: 'networkidle0', timeout: 15000 });
    const roomContent = await page.evaluate(() => document.body.innerText.slice(0, 200));
    console.log(`  Content: ${roomContent.slice(0, 100)}...`);
    console.log('  ✅ Room page loaded\n');

    // TEST 6: WebSocket signaling server reachable
    console.log('TEST 6: Signaling server...');
    const wsTest = await page.evaluate(async () => {
      return new Promise((resolve) => {
        try {
          const ws = new WebSocket('ws://localhost:8333');
          ws.onopen = () => { ws.close(); resolve('connected'); };
          ws.onerror = () => resolve('error');
          setTimeout(() => resolve('timeout'), 3000);
        } catch (e) {
          resolve(`error: ${e.message}`);
        }
      });
    });
    console.log(`  Signaling: ${wsTest}`);
    if (wsTest === 'connected') {
      console.log('  ✅ Signaling OK\n');
    } else {
      console.log('  ❌ Signaling failed\n');
    }

    // TEST 7: Platform333 constructor
    console.log('TEST 7: Platform333 constructor...');
    const platform = await page.evaluate(async () => {
      try {
        const mod = await import('/333/wasm/triple_three.js');
        await mod.default();
        const keys = Object.keys(mod).filter(k => !k.startsWith('__'));
        if (!mod.Platform333) return { error: 'Platform333 not in mod', keys };
        const p = new mod.Platform333(1, new Uint32Array([1, 2, 3, 4]));
        return { nodeId: p.node_id(), worldSize: p.world_size(), view: p.view(), keys };
      } catch (e) {
        return { error: String(e), stack: String(e.stack || '').slice(0, 300) };
      }
    });
    console.log(`  Platform333: ${JSON.stringify(platform)}`);
    if (platform.nodeId) {
      console.log('  ✅ Platform333 created\n');
    } else {
      console.log('  ❌ Platform333 failed\n');
    }

    console.log('=== Test Summary ===');
    console.log('Pages: ✅ Main, Onboard, Room');
    console.log(`WASM: ${health.includes('ok') ? '✅' : '❌'}`);
    console.log(`Identity: ${identity.peerId ? '✅ ' + identity.algorithm : '❌'}`);
    console.log(`Signaling: ${wsTest === 'connected' ? '✅' : '❌'}`);

  } catch (err) {
    console.error('TEST FAILED:', err.message);
  } finally {
    await browser.close();
  }
}

test().catch(console.error);
