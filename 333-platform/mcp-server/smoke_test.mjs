#!/usr/bin/env node
// KG: SPAN_333_MCP_Server — smoke test
//
// Spawns the MCP server as a child process, speaks JSON-RPC over stdio,
// and verifies: initialize → tools/list → tools/call × 3 → resources/list → resources/read.

import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const server = join(here, 'index.mjs');

const child = spawn('node', [server], {
  stdio: ['pipe', 'pipe', 'inherit'],
  env: { ...process.env, SIGNALING_URL: process.env.SIGNALING_URL || 'http://127.0.0.1:1' },
});

let buf = '';
const pending = new Map();
let seq = 0;

child.stdout.on('data', (chunk) => {
  buf += chunk.toString('utf8');
  while (true) {
    const nl = buf.indexOf('\n');
    if (nl < 0) break;
    const line = buf.slice(0, nl);
    buf = buf.slice(nl + 1);
    if (!line.trim()) continue;
    try {
      const msg = JSON.parse(line);
      if (msg.id != null && pending.has(msg.id)) {
        const { resolve, reject } = pending.get(msg.id);
        pending.delete(msg.id);
        msg.error ? reject(msg.error) : resolve(msg.result);
      }
    } catch {}
  }
});

function rpc(method, params = {}) {
  const id = ++seq;
  const req = { jsonrpc: '2.0', id, method, params };
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject });
    child.stdin.write(JSON.stringify(req) + '\n');
    setTimeout(() => {
      if (pending.has(id)) {
        pending.delete(id);
        reject(new Error(`RPC timeout: ${method}`));
      }
    }, 5000);
  });
}

const results = [];
async function expect(label, fn) {
  try {
    const r = await fn();
    results.push({ label, ok: true });
    return r;
  } catch (e) {
    results.push({ label, ok: false, error: String(e.message || e) });
    return null;
  }
}

try {
  await expect('initialize', () =>
    rpc('initialize', {
      protocolVersion: '2024-11-05',
      capabilities: {},
      clientInfo: { name: 'smoke', version: '0.1.0' },
    })
  );
  child.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n');

  const tools = await expect('tools/list', () => rpc('tools/list'));
  if (tools) {
    const names = (tools.tools || []).map((t) => t.name).sort();
    const expected = ['platform_info', 'signaling_status', 'turn_credentials'];
    if (JSON.stringify(names) !== JSON.stringify(expected)) {
      results[results.length - 1].ok = false;
      results[results.length - 1].error = `tool list mismatch: ${JSON.stringify(names)}`;
    }
  }

  await expect('tools/call platform_info', async () => {
    const r = await rpc('tools/call', { name: 'platform_info', arguments: {} });
    const text = r?.content?.[0]?.text ?? '';
    const j = JSON.parse(text);
    if (j.internal_name !== 'triple-three') throw new Error('platform_info missing internal_name=triple-three');
    return j;
  });

  await expect('tools/call signaling_status (unreachable → reachable=false)', async () => {
    const r = await rpc('tools/call', { name: 'signaling_status', arguments: {} });
    const text = r?.content?.[0]?.text ?? '';
    const j = JSON.parse(text);
    if (j.reachable !== false) throw new Error('expected reachable=false for fake URL');
    return j;
  });

  await expect('tools/call turn_credentials', async () => {
    const r = await rpc('tools/call', {
      name: 'turn_credentials',
      arguments: { peerId: 'smoke-peer-1' },
    });
    const text = r?.content?.[0]?.text ?? '';
    const j = JSON.parse(text);
    if (j.ok !== false) throw new Error('expected ok=false for fake URL');
    return j;
  });

  const resources = await expect('resources/list', () => rpc('resources/list'));
  if (resources) {
    const uris = (resources.resources || []).map((r) => r.uri).sort();
    const expected = ['333://platform/info', '333://signaling/metrics'];
    if (JSON.stringify(uris) !== JSON.stringify(expected)) {
      results[results.length - 1].ok = false;
      results[results.length - 1].error = `resource list mismatch: ${JSON.stringify(uris)}`;
    }
  }

  await expect('resources/read 333://platform/info', async () => {
    const r = await rpc('resources/read', { uri: '333://platform/info' });
    const text = r?.contents?.[0]?.text ?? '';
    const j = JSON.parse(text);
    if (j.kg_anchor !== '333_Platform') throw new Error('bad kg_anchor');
    return j;
  });
} finally {
  child.kill();
}

const ok = results.filter((r) => r.ok).length;
const fail = results.length - ok;
console.log('\n=== 333 MCP smoke test ===');
for (const r of results) {
  console.log(`${r.ok ? 'ok  ' : 'FAIL'}  ${r.label}${r.error ? '  — ' + r.error : ''}`);
}
console.log(`\n${ok}/${results.length} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
