#!/usr/bin/env node
// KG: SPAN_333_MCP_Server, SPAN_333_MCP_Tools, SPAN_333_MCP_Resources
// KG: CONTRACT_333_MCP_Server, CONTRACT_333_MCP_Tools, CONTRACT_333_MCP_Resources
// KG: TASK_333_MCP_Server, TASK_333_MCP_Tools, TASK_333_MCP_Resources
//
// 333 Platform MCP Server — stdio JSON-RPC
//
// Exposes signaling status, TURN credentials, and platform metadata as MCP
// primitives so AI clients can introspect the running 333 Platform.

import { Server } from '@modelcontextprotocol/sdk/server/index.js';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import {
  CallToolRequestSchema,
  ListToolsRequestSchema,
  ListResourcesRequestSchema,
  ReadResourceRequestSchema,
} from '@modelcontextprotocol/sdk/types.js';

const SIGNALING_URL = process.env.SIGNALING_URL || 'http://localhost:8333';
const PLATFORM_VERSION = '0.1.0';
const TESTS_TOTAL = 1340;

// ── Signaling metrics fetch (Prometheus text) ──────────────────────────────
async function fetchSignalingMetrics() {
  try {
    const r = await fetch(`${SIGNALING_URL}/metrics`, { signal: AbortSignal.timeout(3000) });
    if (!r.ok) return { reachable: false, http_status: r.status };
    const text = await r.text();
    const out = { reachable: true };
    for (const line of text.split('\n')) {
      const m = line.match(/^signaling_(\w+)\s+(\d+)/);
      if (m) out[m[1]] = Number(m[2]);
    }
    return out;
  } catch (e) {
    return { reachable: false, error: String(e.message || e) };
  }
}

async function fetchTurnCredentials(peerId) {
  const url = new URL('/turn-credentials', SIGNALING_URL);
  if (peerId) url.searchParams.set('peerId', peerId);
  try {
    const r = await fetch(url, { signal: AbortSignal.timeout(3000) });
    if (!r.ok) return { ok: false, http_status: r.status };
    return { ok: true, ...(await r.json()) };
  } catch (e) {
    return { ok: false, error: String(e.message || e) };
  }
}

function platformInfo() {
  return {
    name: '333 Platform',
    internal_name: 'triple-three',
    version: PLATFORM_VERSION,
    kind: 'browser-native P2P application platform',
    stack: ['Rust', 'WASM', 'WebRTC', 'CRDT', 'HotStuff-BFT'],
    layers: {
      L1_Core_WASM: 'HotStuff BFT + CRDT delta + Ed25519 + token 333M cap',
      L2_Network_WebRTC: 'DataChannel mesh, 20-peer limit, 4B binary header',
      L3_Compute_Workers: 'CRDT merge, sig batch verify, token burn',
      L4_Storage: 'OPFS + IndexedDB + Cache API',
      L5_Runtime: 'Service Worker + PWA',
    },
    tests_total_passing: TESTS_TOTAL,
    signaling_url: SIGNALING_URL,
    kg_anchor: '333_Platform',
    kg_root_span: 'SPAN_333_ROOT',
  };
}

// ── MCP Server ─────────────────────────────────────────────────────────────
const server = new Server(
  { name: 'triple-three', version: PLATFORM_VERSION },
  { capabilities: { tools: {}, resources: {} } }
);

// KG: SPAN_333_MCP_Tools — 3 tools
server.setRequestHandler(ListToolsRequestSchema, async () => ({
  tools: [
    {
      name: 'signaling_status',
      description: 'Fetch live metrics from the 333 signaling server (WS connections, active rooms, TURN creds issued, rate-limited count).',
      inputSchema: { type: 'object', properties: {}, additionalProperties: false },
    },
    {
      name: 'turn_credentials',
      description: 'Request short-lived TURN/STUN credentials from the 333 signaling server for a given peerId.',
      inputSchema: {
        type: 'object',
        properties: { peerId: { type: 'string', description: 'Peer identifier requesting credentials' } },
        required: ['peerId'],
        additionalProperties: false,
      },
    },
    {
      name: 'platform_info',
      description: 'Return static 333 Platform metadata — layers, stack, KG anchors, test counts.',
      inputSchema: { type: 'object', properties: {}, additionalProperties: false },
    },
  ],
}));

server.setRequestHandler(CallToolRequestSchema, async (req) => {
  const { name, arguments: args = {} } = req.params;
  let payload;
  switch (name) {
    case 'signaling_status':
      payload = await fetchSignalingMetrics();
      break;
    case 'turn_credentials':
      payload = await fetchTurnCredentials(args.peerId);
      break;
    case 'platform_info':
      payload = platformInfo();
      break;
    default:
      return { content: [{ type: 'text', text: `Unknown tool: ${name}` }], isError: true };
  }
  return { content: [{ type: 'text', text: JSON.stringify(payload, null, 2) }] };
});

// KG: SPAN_333_MCP_Resources — 2 resources
server.setRequestHandler(ListResourcesRequestSchema, async () => ({
  resources: [
    {
      uri: '333://platform/info',
      name: '333 Platform info',
      description: 'Static metadata about the 333 Platform.',
      mimeType: 'application/json',
    },
    {
      uri: '333://signaling/metrics',
      name: 'Signaling live metrics',
      description: 'Live snapshot from signaling server /metrics endpoint (re-fetched per read).',
      mimeType: 'application/json',
    },
  ],
}));

server.setRequestHandler(ReadResourceRequestSchema, async (req) => {
  const { uri } = req.params;
  let body;
  if (uri === '333://platform/info') body = platformInfo();
  else if (uri === '333://signaling/metrics') body = await fetchSignalingMetrics();
  else return { contents: [], isError: true };
  return {
    contents: [{ uri, mimeType: 'application/json', text: JSON.stringify(body, null, 2) }],
  };
});

// ── main ────────────────────────────────────────────────────────────────────
const transport = new StdioServerTransport();
await server.connect(transport);
