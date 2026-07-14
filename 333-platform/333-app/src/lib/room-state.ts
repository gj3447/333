// KG: TASK_333_INT_MemFix, CONTRACT_SharedType_DataChannelSet
// KG: CONTRACT_SharedType_RoomState, SPAN_333_FE_PeerDiscovery
// KG: lesson-333-bft-keyring-exchange-2026-04-14 — BFT keyring 교환 작업 진입점 (createRoomState/createPeerConnection/setupChannel)
// KG: src-fe-createRoomState, src-fe-createPeerConnection, src-fe-setupChannel
// 4 pre-negotiated DataChannels: crdt, position, bft, chat

export type ConnectionStatus = 'disconnected' | 'connecting' | 'connected';

export interface PeerInfo {
  id: string;
  username: string;
  connectedAt: number;
}

// KG: CONTRACT_SharedType_DataChannelSet — 4-channel architecture
export const CH_CRDT = 'crdt';         // reliable, ordered
export const CH_POSITION = 'position'; // unreliable, unordered, 50ms
export const CH_BFT = 'bft';          // reliable, ordered, 5 retry
export const CH_CHAT = 'chat';        // reliable, ordered

/** DataChannel config per channel type */
// KG: CONTRACT_SharedType_DataChannelSet
function channelConfig(label: string): RTCDataChannelInit {
  switch (label) {
    case CH_CRDT:    return { ordered: true };
    case CH_POSITION: return { ordered: false, maxPacketLifeTime: 50 };
    case CH_BFT:     return { ordered: true }; // reliable, no cap — Taliban F5 fix (Rust와 통일)
    case CH_CHAT:    return { ordered: true };
    default:         return { ordered: true };
  }
}

const CHANNEL_LABELS = [CH_CRDT, CH_BFT, CH_CHAT, CH_POSITION];

export interface RoomState {
  roomId: string;
  myId: string;
  status: ConnectionStatus;
  peers: Map<string, PeerInfo>;
  /** Send on a specific channel (default: crdt) */
  sendOn: (channel: string, data: Uint8Array) => void;
  /** Send on crdt channel (backward compat) */
  send: (data: Uint8Array) => void;
  /** Register message handler (receives channel label + data) */
  onMessage: (handler: (from: string, channel: string, data: Uint8Array) => void) => void;
}

type MessageHandler = (from: string, channel: string, data: Uint8Array) => void;

// KG: TASK_333_INT_MemFix — 4-channel room state
export function createRoomState(roomId: string, myId: string, signalingUrl: string): RoomState {
  let ws: WebSocket | null = null;
  let status: ConnectionStatus = 'disconnected';
  const peers = new Map<string, PeerInfo>();
  const handlers: MessageHandler[] = [];
  const peerConnections = new Map<string, RTCPeerConnection>();
  // KG: CONTRACT_SharedType_DataChannelSet — per-peer, per-channel map
  const peerChannels = new Map<string, Map<string, RTCDataChannel>>();

  const state: RoomState = {
    roomId,
    myId,
    get status() { return status; },
    peers,
    // KG: plan-333-p2p-channel-readiness — send with channel-open check + warning
    sendOn: (channel: string, data: Uint8Array) => {
      let sent = false;
      for (const [peerId, channels] of peerChannels) {
        const ch = channels.get(channel);
        if (ch && ch.readyState === 'open') {
          // KG: lesson-333-ts57-uint8-bufferlike-2026-04-19
          ch.send(data as unknown as ArrayBuffer);
          sent = true;
        } else if (ch) {
          console.warn(`[333-rtc] sendOn(${channel}): channel not open for peer ${peerId.slice(0,8)}, state=${ch.readyState}`);
        }
      }
      if (!sent && peerChannels.size > 0) {
        console.warn(`[333-rtc] sendOn(${channel}): no open channel found (${peerChannels.size} peers)`);
      }
    },
    send: (data: Uint8Array) => {
      state.sendOn(CH_CRDT, data);
    },
    onMessage: (handler: MessageHandler) => { handlers.push(handler); }
  };

  function notifyHandlers(from: string, channel: string, data: Uint8Array) {
    for (const h of handlers) h(from, channel, data);
  }

  // KG: TASK_333_PRE_TURN, CONTRACT_333_PRE_TURN
  // Taliban F4: coturn 미배포. STUN-only 동작. coturn 배포 후 TURN 추가.
  // TODO: coturn 배포 후 iceServers에 TURN 추가 + ephemeral credentials
  function createPeerConnection(peerId: string, isInitiator: boolean) {
    const pc = new RTCPeerConnection({
      iceServers: [
        { urls: ['stun:stun.l.google.com:19302', 'stun:stun1.l.google.com:19302'] }
        // TURN: coturn 배포 후 활성화
        // { urls: ['turn:turn.metahumotonic.com:3478'], username: '...', credential: '...' }
      ]
    });
    peerConnections.set(peerId, pc);
    const channels = new Map<string, RTCDataChannel>();
    peerChannels.set(peerId, channels);

    pc.onicecandidate = (e) => {
      if (e.candidate && ws?.readyState === WebSocket.OPEN) {
        ws.send(JSON.stringify({ type: 'ice', to: peerId, candidate: e.candidate }));
      }
    };

    // KG: CONTRACT_SharedType_DataChannelSet — receive remote channels
    pc.ondatachannel = (e) => {
      console.log('[333-rtc] ondatachannel:', e.channel.label, 'from', peerId);
      setupChannel(peerId, e.channel);
    };
    pc.onconnectionstatechange = () => {
      console.log('[333-rtc] connection state:', pc.connectionState, 'peer:', peerId);
    };
    pc.oniceconnectionstatechange = () => {
      console.log('[333-rtc] ICE state:', pc.iceConnectionState, 'peer:', peerId);
    };

    if (isInitiator) {
      // KG: CONTRACT_SharedType_DataChannelSet — create 4 channels
      for (const label of CHANNEL_LABELS) {
        const ch = pc.createDataChannel(label, channelConfig(label));
        setupChannel(peerId, ch);
      }
      pc.createOffer().then(offer => {
        pc.setLocalDescription(offer);
        ws?.send(JSON.stringify({ type: 'offer', to: peerId, sdp: offer }));
      });
    }

    return pc;
  }

  // KG: VR_333_Integration_MemFix_Taliban — F4 채널 open 순서 race 수정
  const peerOpenChannels = new Map<string, Set<string>>();

  function setupChannel(peerId: string, channel: RTCDataChannel) {
    const channels = peerChannels.get(peerId);
    if (channels) channels.set(channel.label, channel);

    channel.binaryType = 'arraybuffer';

    channel.onopen = () => {
      // F4 Fix: track which channels are open per peer
      if (!peerOpenChannels.has(peerId)) peerOpenChannels.set(peerId, new Set());
      peerOpenChannels.get(peerId)!.add(channel.label);

      // KG: plan-333-p2p-channel-readiness — register peer when CRDT opens,
      // but only set 'connected' when ALL 4 channels are open (BFT needs reliable channel)
      if (channel.label === CH_CRDT) {
        peers.set(peerId, { id: peerId, username: peerId.slice(0, 8), connectedAt: Date.now() });
      }
      const openSet = peerOpenChannels.get(peerId)!;
      if (openSet.size >= CHANNEL_LABELS.length) {
        status = 'connected';
        console.log('[333-rtc] all 4 channels open for peer:', peerId);
      } else if (openSet.has(CH_CRDT)) {
        // At least CRDT is open — allow basic communication
        status = 'connected';
      }
    };

    channel.onclose = () => {
      const chs = peerChannels.get(peerId);
      if (chs) chs.delete(channel.label);
      // Only cleanup peer when crdt channel closes
      if (channel.label === CH_CRDT) {
        peers.delete(peerId);
        // Close remaining channels for this peer
        const remaining = peerChannels.get(peerId);
        if (remaining) {
          for (const [, ch] of remaining) ch.close();
          remaining.clear();
        }
        peerChannels.delete(peerId);
        peerConnections.get(peerId)?.close();
        peerConnections.delete(peerId);
        if (peers.size === 0 && ws?.readyState === WebSocket.OPEN) status = 'connecting';
      }
    };

    // KG: SPAN_333_INT_CrdtSync — message routing with channel label
    channel.onmessage = (e) => {
      const data = e.data instanceof ArrayBuffer
        ? new Uint8Array(e.data)
        : new TextEncoder().encode(e.data);
      notifyHandlers(peerId, channel.label, data);
    };
  }

  // Connect to signaling
  try {
    status = 'connecting';
    ws = new WebSocket(signalingUrl);

    ws.onopen = () => {
      ws!.send(JSON.stringify({ type: 'join', room: roomId, peerId: myId }));
    };

    ws.onmessage = async (e) => {
      const msg = JSON.parse(e.data);
      console.log('[333-sig]', msg.type, JSON.stringify(msg).slice(0, 120));

      if (msg.type === 'peers') {
        console.log('[333-sig] myId=' + myId + ', peers=' + msg.peers.join(','));
        for (const pid of msg.peers) {
          if (pid !== myId && !peerConnections.has(pid)) {
            console.log('[333-sig] connecting to peer:', pid);
            createPeerConnection(pid, true);
          }
        }
      } else if (msg.type === 'peer-joined' && msg.peerId !== myId) {
        // New peer joined — they will send us an offer (they are initiator via 'peers' msg)
        // We just prepare to receive their offer — do NOT initiate (avoids glare)
        console.log('[333-sig] peer-joined, waiting for their offer:', msg.peerId);
      } else if (msg.type === 'offer' && msg.from !== myId) {
        try {
          const pc = createPeerConnection(msg.from, false);
          await pc.setRemoteDescription(new RTCSessionDescription(msg.sdp));
          const answer = await pc.createAnswer();
          await pc.setLocalDescription(answer);
          ws!.send(JSON.stringify({ type: 'answer', to: msg.from, sdp: answer }));
          console.log('[333-sig] sent answer to', msg.from);
        } catch (e) { console.error('[333-sig] offer error:', e); }
      } else if (msg.type === 'answer' && msg.from !== myId) {
        try {
          await peerConnections.get(msg.from)?.setRemoteDescription(new RTCSessionDescription(msg.sdp));
          console.log('[333-sig] set answer from', msg.from);
        } catch (e) { console.error('[333-sig] answer error:', e); }
      } else if (msg.type === 'ice' && msg.from !== myId) {
        try {
          await peerConnections.get(msg.from)?.addIceCandidate(new RTCIceCandidate(msg.candidate));
        } catch (e) { console.error('[333-sig] ice error:', e); }
      }
    };

    ws.onclose = () => {
      if (peers.size === 0) status = 'disconnected';
    };
  } catch {
    status = 'disconnected';
  }

  return state;
}

export function generateRoomId(): string {
  const chars = 'abcdefghijklmnopqrstuvwxyz0123456789';
  let id = '';
  for (let i = 0; i < 6; i++) id += chars[Math.floor(Math.random() * chars.length)];
  return id;
}
