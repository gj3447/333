// KG: SPAN_333_Signaling, SPAN_333_Infra
// KG: seed-post-rts-turn-credentials-2026-04-15
// KG: sprint6C-observability-2026-04-15
// 333 Signaling Server — WebSocket relay + TURN credential HTTP endpoint
// Usage: node server.mjs [port]

import { createServer } from 'node:http';
import { WebSocketServer } from 'ws';
import { generateTurnCredentials } from './turn_credentials.mjs';

// ---------------------------------------------------------------------------
// In-memory metrics counters + gauges
// KG: sprint6C-observability-2026-04-15
// ---------------------------------------------------------------------------
const metrics = {
  // Gauges (current state)
  signaling_ws_connections: 0,
  signaling_rooms_active: 0,
  // Counters (monotonically increasing)
  signaling_turn_credentials_issued_total: 0,
  signaling_rate_limited_total: 0,
};

/**
 * Render current metrics in Prometheus text exposition format.
 * Endpoint: GET /metrics  (not publicly routed; protected by Traefik BasicAuth at /ws333/metrics)
 * # KG: sprint6C-observability-2026-04-15
 */
function renderPrometheusMetrics() {
  const lines = [
    '# HELP signaling_ws_connections Current number of active WebSocket connections',
    '# TYPE signaling_ws_connections gauge',
    `signaling_ws_connections ${metrics.signaling_ws_connections}`,
    '',
    '# HELP signaling_rooms_active Current number of active rooms (rooms with ≥1 peer)',
    '# TYPE signaling_rooms_active gauge',
    `signaling_rooms_active ${metrics.signaling_rooms_active}`,
    '',
    '# HELP signaling_turn_credentials_issued_total Total TURN credential sets issued',
    '# TYPE signaling_turn_credentials_issued_total counter',
    `signaling_turn_credentials_issued_total ${metrics.signaling_turn_credentials_issued_total}`,
    '',
    '# HELP signaling_rate_limited_total Total requests rejected by the rate limiter',
    '# TYPE signaling_rate_limited_total counter',
    `signaling_rate_limited_total ${metrics.signaling_rate_limited_total}`,
    '',
  ];
  return lines.join('\n');
}

const PORT = parseInt(process.argv[2] || '8333');

// ---------------------------------------------------------------------------
// CORS origins
// ---------------------------------------------------------------------------
const ALLOWED_ORIGINS = new Set([
  'https://metahumotonic.com',
  'https://333.metahumotonic.com',
  'http://localhost:3000',
  'http://localhost:5173',
  'http://localhost:8080',
]);

function isCorsAllowed(origin) {
  if (!origin) return true; // same-origin / no-origin (non-browser)
  return ALLOWED_ORIGINS.has(origin);
}

// ---------------------------------------------------------------------------
// In-memory rate limiter — 10 req/min per IP
// ---------------------------------------------------------------------------
const rateLimitMap = new Map(); // ip → { count, resetAt }
const RATE_LIMIT = 10;
const RATE_WINDOW_MS = 60_000;

function checkRateLimit(ip) {
  const now = Date.now();
  let entry = rateLimitMap.get(ip);
  if (!entry || now >= entry.resetAt) {
    entry = { count: 0, resetAt: now + RATE_WINDOW_MS };
    rateLimitMap.set(ip, entry);
  }
  entry.count += 1;
  return entry.count <= RATE_LIMIT;
}

// Prune stale entries periodically (every 5 min)
setInterval(() => {
  const now = Date.now();
  for (const [ip, entry] of rateLimitMap) {
    if (now >= entry.resetAt) rateLimitMap.delete(ip);
  }
}, 5 * 60_000).unref();

// ---------------------------------------------------------------------------
// HTTP server (shared with WebSocket upgrade)
// ---------------------------------------------------------------------------
const server = createServer((req, res) => {
  const url = new URL(req.url, `http://localhost`);
  const origin = req.headers['origin'] || '';

  // CORS preflight
  if (req.method === 'OPTIONS') {
    if (isCorsAllowed(origin)) {
      res.writeHead(204, {
        'Access-Control-Allow-Origin': origin || '*',
        'Access-Control-Allow-Methods': 'GET, OPTIONS',
        'Access-Control-Allow-Headers': 'Content-Type',
        'Access-Control-Max-Age': '86400',
      });
    } else {
      res.writeHead(403);
    }
    res.end();
    return;
  }

  // GET /turn-credentials?clientId=<id>
  if (req.method === 'GET' && url.pathname === '/turn-credentials') {
    // CORS check
    if (!isCorsAllowed(origin)) {
      res.writeHead(403, { 'Content-Type': 'application/json' });
      res.end(JSON.stringify({ error: 'CORS: origin not allowed' }));
      return;
    }

    // Rate limit
    const ip = req.socket.remoteAddress || 'unknown';
    if (!checkRateLimit(ip)) {
      // KG: sprint6C-observability-2026-04-15
      metrics.signaling_rate_limited_total += 1;
      res.writeHead(429, {
        'Content-Type': 'application/json',
        'Retry-After': '60',
        ...(origin ? { 'Access-Control-Allow-Origin': origin } : {}),
      });
      res.end(JSON.stringify({ error: 'Too many requests. Retry after 60s.' }));
      return;
    }

    const clientId = url.searchParams.get('clientId') || `anon-${Date.now()}`;

    let creds;
    try {
      creds = generateTurnCredentials(clientId);
    } catch (err) {
      res.writeHead(503, { 'Content-Type': 'application/json' });
      res.end(JSON.stringify({ error: 'TURN service unavailable', detail: err.message }));
      return;
    }

    // KG: sprint6C-observability-2026-04-15
    metrics.signaling_turn_credentials_issued_total += 1;
    res.writeHead(200, {
      'Content-Type': 'application/json',
      'Cache-Control': 'no-store',
      ...(origin ? { 'Access-Control-Allow-Origin': origin } : {}),
    });
    res.end(JSON.stringify(creds));
    return;
  }

  // GET /metrics — Prometheus text format (NOT public; protected upstream by BasicAuth)
  // KG: sprint6C-observability-2026-04-15
  if (req.method === 'GET' && url.pathname === '/metrics') {
    res.writeHead(200, { 'Content-Type': 'text/plain; version=0.0.4; charset=utf-8' });
    res.end(renderPrometheusMetrics());
    return;
  }

  // Health check
  if (req.method === 'GET' && url.pathname === '/health') {
    res.writeHead(200, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify({ status: 'ok' }));
    return;
  }

  res.writeHead(404, { 'Content-Type': 'application/json' });
  res.end(JSON.stringify({ error: 'Not found' }));
});

// ---------------------------------------------------------------------------
// WebSocket server — attached to the same HTTP server
// ---------------------------------------------------------------------------
const wss = new WebSocketServer({ server });

// room → Map<peerId, ws>
const rooms = new Map();

wss.on('connection', (ws) => {
  // KG: sprint6C-observability-2026-04-15
  metrics.signaling_ws_connections += 1;
  let peerId = null;
  let roomId = null;

  ws.on('message', (raw) => {
    let msg;
    try { msg = JSON.parse(raw); } catch { return; }

    switch (msg.type) {
      case 'join': {
        peerId = msg.peerId || `peer-${Date.now()}`;
        roomId = msg.room || 'default';

        if (!rooms.has(roomId)) rooms.set(roomId, new Map());
        const room = rooms.get(roomId);
        room.set(peerId, ws);
        // KG: sprint6C-observability-2026-04-15
        metrics.signaling_rooms_active = rooms.size;

        // Tell this peer about existing peers
        const peers = [...room.keys()].filter(id => id !== peerId);
        ws.send(JSON.stringify({ type: 'peers', peers, you: peerId }));

        // Tell existing peers about new peer
        for (const [id, sock] of room) {
          if (id !== peerId && sock.readyState === 1) {
            sock.send(JSON.stringify({ type: 'peer-joined', peerId }));
          }
        }
        console.log(`[${roomId}] ${peerId} joined (${room.size} peers)`);
        break;
      }

      case 'offer':
      case 'answer':
      case 'ice': {
        // Relay to target peer
        const room = rooms.get(roomId);
        if (!room) break;
        const target = room.get(msg.to);
        if (target && target.readyState === 1) {
          target.send(JSON.stringify({ ...msg, from: peerId }));
        }
        break;
      }

      case 'broadcast': {
        // Relay to all peers in room (for CRDT delta sync)
        const room = rooms.get(roomId);
        if (!room) break;
        for (const [id, sock] of room) {
          if (id !== peerId && sock.readyState === 1) {
            sock.send(JSON.stringify({ ...msg, from: peerId }));
          }
        }
        break;
      }
    }
  });

  ws.on('close', () => {
    // KG: sprint6C-observability-2026-04-15
    metrics.signaling_ws_connections = Math.max(0, metrics.signaling_ws_connections - 1);
    if (roomId && peerId) {
      const room = rooms.get(roomId);
      if (room) {
        room.delete(peerId);
        // Notify others
        for (const [id, sock] of room) {
          if (sock.readyState === 1) {
            sock.send(JSON.stringify({ type: 'peer-left', peerId }));
          }
        }
        console.log(`[${roomId}] ${peerId} left (${room.size} peers)`);
        if (room.size === 0) rooms.delete(roomId);
        // KG: sprint6C-observability-2026-04-15
        metrics.signaling_rooms_active = rooms.size;
      }
    }
  });
});

server.listen(PORT, () => {
  console.log(`333 Signaling Server on http://localhost:${PORT}`);
  console.log(`  WS  ws://localhost:${PORT}`);
  console.log(`  GET http://localhost:${PORT}/turn-credentials?clientId=<id>`);
  console.log(`  GET http://localhost:${PORT}/health`);
  // KG: sprint6C-observability-2026-04-15
  console.log(`  GET http://localhost:${PORT}/metrics  (Prometheus; internal only)`);
});
