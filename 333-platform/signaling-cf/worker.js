// KG: SPAN_333_Infra — Cloudflare Workers Signaling Server
// Durable Objects for room state, WebSocket relay for SDP/ICE
// Deploy: wrangler deploy

export default {
  async fetch(request, env) {
    const url = new URL(request.url);

    if (url.pathname === '/health') {
      return new Response(JSON.stringify({ status: 'ok', service: '333-signaling', runtime: 'cloudflare-workers' }), {
        headers: { 'content-type': 'application/json', 'access-control-allow-origin': '*' }
      });
    }

    // WebSocket upgrade
    if (request.headers.get('Upgrade') === 'websocket') {
      const roomId = url.searchParams.get('room') || 'default';
      const id = env.ROOM.idFromName(roomId);
      const room = env.ROOM.get(id);
      return room.fetch(request);
    }

    return new Response('333 Signaling Server — connect via WebSocket', { status: 200 });
  }
};

// Durable Object: one per room
export class SignalingRoom {
  constructor(state, env) {
    this.state = state;
    this.peers = new Map(); // peerId → WebSocket
  }

  async fetch(request) {
    const pair = new WebSocketPair();
    const [client, server] = Object.values(pair);

    this.state.acceptWebSocket(server);

    let peerId = null;

    server.addEventListener('message', (event) => {
      let msg;
      try { msg = JSON.parse(event.data); } catch { return; }

      switch (msg.type) {
        case 'join': {
          peerId = msg.peerId || `peer-${Date.now().toString(36)}`;
          this.peers.set(peerId, server);

          // Send existing peers to new peer
          const peers = [...this.peers.keys()].filter(id => id !== peerId);
          server.send(JSON.stringify({ type: 'peers', peers, you: peerId }));

          // Notify others
          this.broadcast({ type: 'peer-joined', peerId }, peerId);
          break;
        }

        case 'offer':
        case 'answer':
        case 'ice': {
          const target = this.peers.get(msg.to);
          if (target && target.readyState === 1) {
            target.send(JSON.stringify({ ...msg, from: peerId }));
          }
          break;
        }

        case 'broadcast': {
          this.broadcast({ ...msg, from: peerId }, peerId);
          break;
        }
      }
    });

    server.addEventListener('close', () => {
      if (peerId) {
        this.peers.delete(peerId);
        this.broadcast({ type: 'peer-left', peerId }, peerId);
      }
    });

    return new Response(null, { status: 101, webSocket: client });
  }

  broadcast(msg, excludeId) {
    const data = JSON.stringify(msg);
    for (const [id, ws] of this.peers) {
      if (id !== excludeId && ws.readyState === 1) {
        ws.send(data);
      }
    }
  }
}
