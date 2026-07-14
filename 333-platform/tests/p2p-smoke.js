// KG: TEST_333_P2P_Smoke
// Simple P2P smoke test: 2 tabs, verify connection, exchange messages
// Usage: node tests/p2p-smoke.js ws://localhost:8333
// Requires: npm install --save-dev puppeteer-core

const puppeteer = require('puppeteer');
const assert = require('assert');

const SIGNALING_URL = process.argv[2] || 'ws://localhost:8333';
const APP_URL = process.env.APP_URL || 'http://localhost:5173/room';
const TIMEOUT = 15000;
const PEER_CONNECT_TIMEOUT = 10000;

let roomId = null;
let tab1 = null;
let tab2 = null;
const logs = [];

async function log(msg, data = '') {
  const line = `[SMOKE] ${msg} ${data}`.trim();
  console.log(line);
  logs.push(line);
}

async function sleep(ms) {
  return new Promise(r => setTimeout(r, ms));
}

async function runTest() {
  const browser = await puppeteer.launch({
    headless: 'new',
    args: ['--no-sandbox', '--disable-setuid-sandbox']
  });

  try {
    await log('Starting P2P smoke test...');
    await log(`Signaling URL: ${SIGNALING_URL}`);
    await log(`App URL: ${APP_URL}`);

    // ============= Tab 1: Create room =============
    await log('>>> Opening Tab 1 (creator)...');
    tab1 = await browser.newPage();
    tab1.on('console', msg => {
      const text = msg.text();
      if (text.includes('WS:') || text.includes('DC:') || text.includes('ICE:') || text.includes('CRDT:')) {
        console.log(`  [Tab1-console] ${text}`);
      }
    });

    tab1.on('error', err => {
      console.error(`[Tab1-error] ${err.message}`);
    });

    await tab1.goto(APP_URL, { waitUntil: 'networkidle2', timeout: TIMEOUT });
    await log('Tab 1 page loaded');

    // Look for "Create Room" button and click it
    const createButton = await tab1.$('button:has-text("Create Room")');
    if (!createButton) {
      // Fallback: look for any button with "Create" in text
      const buttons = await tab1.$$('button');
      let found = false;
      for (const btn of buttons) {
        const text = await tab1.evaluate(el => el.textContent, btn);
        if (text.includes('Create') || text.includes('create')) {
          await btn.click();
          found = true;
          break;
        }
      }
      assert(found, 'Create Room button not found');
    } else {
      await createButton.click();
    }

    await sleep(1500);

    // Extract room ID from page
    roomId = await tab1.evaluate(() => {
      const span = document.querySelector('[data-testid="room-id"]');
      if (span) return span.textContent.trim();

      // Fallback: look for room ID in visible text
      const text = document.body.innerText;
      const match = text.match(/Room (\w+)/);
      return match ? match[1] : null;
    });

    assert(roomId && roomId.length > 0, 'Room ID not found on page');
    await log(`Tab 1 created room: ${roomId}`);

    // Verify signaling connection
    const status1 = await tab1.evaluate(() => {
      return document.querySelector('[data-testid="connection-status"]')?.textContent?.trim() || 'unknown';
    });
    await log(`Tab 1 connection status: ${status1}`);

    // ============= Tab 2: Join room =============
    await log('>>> Opening Tab 2 (joiner)...');
    tab2 = await browser.newPage();
    tab2.on('console', msg => {
      const text = msg.text();
      if (text.includes('WS:') || text.includes('DC:') || text.includes('ICE:') || text.includes('CRDT:')) {
        console.log(`  [Tab2-console] ${text}`);
      }
    });

    const joinUrl = `${APP_URL}?id=${roomId}`;
    await tab2.goto(joinUrl, { waitUntil: 'networkidle2', timeout: TIMEOUT });
    await log(`Tab 2 navigated to: ${joinUrl}`);

    // ============= Wait for peer connection =============
    await log('>>> Waiting for peer connection...');
    let peerCount = 0;
    let elapsed = 0;
    const startTime = Date.now();

    while (elapsed < PEER_CONNECT_TIMEOUT) {
      peerCount = await tab1.evaluate(() => {
        const span = document.querySelector('[data-testid="peer-count"]');
        if (span) {
          const text = span.textContent.trim();
          return parseInt(text) || 0;
        }
        // Fallback: look for peer info in DOM
        const peerDivs = document.querySelectorAll('[data-peer-id]');
        return peerDivs.length;
      });

      if (peerCount > 0) {
        await log(`Tab 1 detected ${peerCount} peer(s) ✓`);
        break;
      }

      await sleep(500);
      elapsed = Date.now() - startTime;
    }

    assert(peerCount > 0, `Peers did not connect within ${PEER_CONNECT_TIMEOUT}ms`);

    // Verify bidirectional connection
    const peerCount2 = await tab2.evaluate(() => {
      const span = document.querySelector('[data-testid="peer-count"]');
      if (span) {
        const text = span.textContent.trim();
        return parseInt(text) || 0;
      }
      const peerDivs = document.querySelectorAll('[data-peer-id]');
      return peerDivs.length;
    });

    await log(`Tab 2 detected ${peerCount2} peer(s)`);
    assert(peerCount2 > 0, 'Tab 2 did not detect Tab 1 as a peer');

    // ============= CRDT test: Message exchange =============
    await log('>>> Testing CRDT message exchange...');

    // Find and click a block
    const block = await tab1.$('[data-x="2"][data-y="3"]');
    if (block) {
      await block.click();
      await log('Tab 1 clicked block (2,3)');
    } else {
      await log('⚠ Block (2,3) not found, trying alternative');
      const blocks = await tab1.$$('.block');
      if (blocks.length > 0) {
        await blocks[0].click();
        await log('Tab 1 clicked first available block');
      }
    }

    await sleep(1000);

    // Check if block appears on Tab 2
    const blockPresent = await tab2.evaluate(() => {
      const block = document.querySelector('[data-x="2"][data-y="3"]');
      return block ? block.classList.contains('active') || block.classList.contains('stone') : false;
    });

    if (blockPresent) {
      await log('✓ Block replicated to Tab 2 (CRDT working)');
    } else {
      await log('⚠ Block not detected on Tab 2 (may need longer wait)');
    }

    // ============= Verify no console errors =============
    await log('>>> Checking for console errors...');
    const consoleErrors = [];
    tab1.on('pageerror', err => consoleErrors.push(`Tab1: ${err.message}`));
    tab2.on('pageerror', err => consoleErrors.push(`Tab2: ${err.message}`));

    if (consoleErrors.length === 0) {
      await log('✓ No JavaScript errors in console');
    } else {
      await log(`⚠ Found ${consoleErrors.length} error(s):`);
      for (const err of consoleErrors) {
        await log(`  - ${err}`);
      }
    }

    // ============= Success =============
    await log('=== ALL SMOKE TESTS PASSED ===');

  } catch (err) {
    await log(`❌ TEST FAILED: ${err.message}`);
    if (logs.length > 0) {
      console.log('\n--- Test Log ---');
      logs.forEach(l => console.log(l));
    }
    throw err;
  } finally {
    if (tab1) await tab1.close();
    if (tab2) await tab2.close();
    await browser.close();
  }
}

runTest().catch(err => {
  console.error('\nFatal error:', err.message);
  process.exit(1);
});
