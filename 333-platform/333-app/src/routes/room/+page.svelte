<!-- KG: CONTRACT_333_FE_PeerDiscovery, SPAN_333_FE_PeerDiscovery -->
<!-- KG: lesson-333-bft-keyring-exchange-2026-04-14 — BFT keyring 교환 UI 훅 지점 -->
<!-- KG: src-fe-room-page, src-fe-peerIdToNodeId -->
<script lang="ts">
  // KG: CONTRACT_333_FE_PeerDiscovery — P2P Room create/join
  import { onMount, onDestroy } from 'svelte';
  import { page } from '$app/state';
  import { base } from '$app/paths';
  import { createRoomState, generateRoomId, type RoomState, type ConnectionStatus } from '$lib/room-state';
  import { loadIdentity } from '$lib/identity';
  import { getWasm, initWasm, type WasmBridge, type RoomConsensusState, type OutgoingMsg } from '$lib/wasm-bridge';
  import { CH_CRDT, CH_BFT } from '$lib/room-state';

  // KG: CONTRACT_333_FE_PeerDiscovery — Dynamic signaling URL
  // KG: phase-post0-D-dgx-job-2026-04-14 — window.__333_signaling override (test injection)
  function getSignalingUrl(): string {
    if (typeof window === 'undefined') return 'ws://localhost:8333';
    const override = (window as any).__333_signaling;
    if (typeof override === 'string' && override.length > 0) return override;
    return location.hostname === 'localhost'
      ? 'ws://localhost:8333'
      : 'wss://' + location.host + '/ws333/';
  }

  let room: RoomState | null = $state(null);
  let status: ConnectionStatus = $state('disconnected');
  let peerList: Array<{ id: string; username: string }> = $state([]);
  let roomId = $state('');
  let joinId = $state('');
  let messageLog: string[] = $state([]);
  let myId = $state('');
  let copied = $state(false);
  let wasm: WasmBridge | null = $state(null);
  let consensusState: RoomConsensusState = $state({
    nodeId: 0, view: 0, isLeader: false, committedBlocks: 0, worldSize: 0, syncPending: 0
  });
  let syncPollTimer: ReturnType<typeof setInterval> | null = null;
  // KG: lesson-333-bft-keyring-exchange-2026-04-14 — shared across onMount / connectToRoom closures
  let myNodeId = $state(0);

  // Block world state
  const GRID = 8;
  let blocks: Map<string, string> = $state(new Map());
  let selectedBlock = $state('stone');
  const blockTypes = ['stone', 'grass', 'water', 'fire'];
  const blockColors: Record<string, string> = {
    stone: '#6b7280', grass: '#34d399', water: '#67e8f9',
    fire: '#f472b6', empty: 'rgba(107,76,153,0.1)'
  };

  // KG: plan-333-bft-try-propose — derive unique nodeId from peerId for BFT quorum
  function peerIdToNodeId(peerId: string): number {
    let h = 0;
    for (let i = 0; i < peerId.length; i++) h = ((h << 5) - h + peerId.charCodeAt(i)) | 0;
    return (Math.abs(h) % 100) + 1; // 1~100 range, avoid 0
  }

  onMount(async () => {
    const identity = await loadIdentity();
    // Use identity peerId or generate random one for testing
    myId = identity?.peerId || ('peer-' + Math.random().toString(36).slice(2, 10));

    // nodeId: window.__333_nodeId (injected by test) or URL param or derived from peerId
    // validators: window.__333_validators or URL param or default [self]
    const winNodeId = typeof window !== 'undefined' && (window as any).__333_nodeId;
    const winValidators = typeof window !== 'undefined' && (window as any).__333_validators;
    const urlNodeId = page.url.searchParams.get('nodeId');
    const urlValidators = page.url.searchParams.get('validators');
    myNodeId = winNodeId || (urlNodeId ? parseInt(urlNodeId) : peerIdToNodeId(myId));
    const validators = winValidators
      || (urlValidators ? urlValidators.split(',').map(Number) : [myNodeId]);

    try {
      wasm = await initWasm(myNodeId, validators);
      const h = await wasm.health();
      log(`WASM initialized: nodeId=${myNodeId} validators=[${validators}] ${h}`);
    } catch (e) {
      log('WASM init failed: ' + (e as Error).message);
    }

    const urlRoom = page.url.searchParams.get('id');
    if (urlRoom) { joinId = urlRoom; joinRoom(); }
  });

  function createRoom() {
    roomId = generateRoomId();
    connectToRoom(roomId);
  }

  function joinRoom() {
    if (!joinId.trim()) return;
    roomId = joinId.trim();
    connectToRoom(roomId);
  }

  function connectToRoom(id: string) {
    const sigUrl = getSignalingUrl();
    log(`Connecting: room=${id}, myId=${myId}, sig=${sigUrl}`);
    room = createRoomState(id, myId, sigUrl);
    status = 'connecting';

    const interval = setInterval(() => {
      if (!room) { clearInterval(interval); return; }
      status = room.status;
      peerList = Array.from(room.peers.values());
    }, 500);

    // KG: plan-wasm-executor-solid-2026-04-14 phase-3-ui-cleanup — 이중 큐 제거.
    // wasm-bridge.ts의 wasmExecutor가 모든 platform 호출 직렬화. UI는 await만.

    // KG: TASK_333_INT_E2E — WASM process_wire bridge (executor-serialized)
    room.onMessage((from, channel, data) => {
      if (!wasm) return;
      // async IIFE — bridge가 직렬화하므로 race 안전
      (async () => {
        try {
          // KG: lesson-333-bft-keyring-exchange — handshake
          // KG: phase-post0-E-textdecoder-2026-04-14, finding_d4_textdecoder_race
          // chat 채널 wire format: [type_byte][payload]
          //   0x01 = handshake JSON. 다른 type은 무시 (future-proof).
          // WebRTC DataChannel이 UTF-8 multi-byte 경계에서 분할되는 race 방지 +
          // TextDecoder fatal:false로 invalid sequence 놓침 없이 U+FFFD 대체.
          if (channel === 'chat') {
            if (data.length >= 1 && data[0] === 0x01) {
              try {
                const text = new TextDecoder('utf-8', { fatal: false }).decode(data.slice(1));
                const msg = JSON.parse(text);
                if (msg.type === 'handshake' && msg.pubKey && msg.nodeId) {
                  const ok = await wasm!.registerPeerKey(msg.nodeId, msg.pubKey);
                  console.warn('[333-hs] RECV handshake from node' + msg.nodeId + ' ok=' + ok);
                  log(`BFT key registered: node${msg.nodeId} ${msg.pubKey.slice(0,8)}...`);
                  return;
                }
              } catch (e) {
                console.warn('[333-hs] handshake parse failed:', (e as Error).message);
              }
            }
            return; // chat 채널은 handshake 전용. 다른 payload 무시.
          }

          if (channel === 'bft') {
            console.warn('[333-bft] RECV bft ' + data.length + 'B from ' + from.slice(0,8));
          }
          const outgoing = await wasm!.processWire(data);
          if (channel === 'bft' && outgoing.length > 0) {
            console.warn('[333-bft] processWire emitted ' + outgoing.length + ' reply, ch=' + outgoing[0].channel);
          }
          sendOutgoing(outgoing);
          consensusState = await wasm!.roomState();
          blocks = new Map();
          messageLog = [...messageLog.slice(-19), `${from.slice(0,6)}[${channel}]: ${data.length}B`];
        } catch (e) {
          console.error('onMessage error:', e);
        }
      })();
    });

    // KG: lesson-333-bft-keyring-exchange — send handshake
    let handshakeSent = 0;
    const handshakeInterval = setInterval(() => {
      if (!wasm || !room || status !== 'connected') return;
      if (handshakeSent >= 10) { clearInterval(handshakeInterval); return; }
      (async () => {
        try {
          const myPubKey = await wasm!.getPublicKey();
          const handshake = JSON.stringify({ type: 'handshake', nodeId: myNodeId, pubKey: myPubKey });
          // KG: phase-post0-E-textdecoder-2026-04-14 — type byte prefix 0x01
          const payload = new TextEncoder().encode(handshake);
          const prefixed = new Uint8Array(payload.length + 1);
          prefixed[0] = 0x01; // MSG_TYPE_HANDSHAKE
          prefixed.set(payload, 1);
          console.warn('[333-hs] SEND handshake nodeId=' + myNodeId + ' attempt=' + handshakeSent + ' len=' + prefixed.length);
          room!.sendOn('chat', prefixed);
          handshakeSent++;
        } catch (e) { console.error('handshake error:', e); }
      })();
    }, 1000);

    // KG: TASK_333_E2E_HmrFix, plan-333-bft-try-propose — sync + BFT poll
    syncPollTimer = setInterval(() => {
      if (!wasm || !room || status !== 'connected') return;
      (async () => {
        try {
          const crdtOut = await wasm!.pollSync();
          const bftOut  = await wasm!.tryPropose();
          // KG: fix-333-pacemaker-unwired-2026-07-15 — without this the
          // view-change timer never fires and a dead leader stalls the room.
          const tickOut = await wasm!.bftTick();
          const state   = await wasm!.roomState();
          const allOut  = [...crdtOut, ...bftOut, ...tickOut];
          if (bftOut.length > 0) {
            console.warn('[333-bft] tryPropose emitted ' + bftOut.length + ' msg, channel=' + bftOut[0].channel + ' len=' + bftOut[0].payload.length);
          }
          if (tickOut.length > 0) {
            console.warn('[333-bft] pacemaker fired — leader stalled, broadcasting ViewChange');
          }
          if (allOut.length > 0) sendOutgoing(allOut);
          consensusState = state;
        } catch (e) { console.error('poll error:', e); }
      })();
    }, 200);

    // Removed: history.replaceState conflicted with SvelteKit router + puppeteer
    log(`Room ${id} | Signaling: ${sigUrl}`);
  }

  // KG: TASK_333_INT_E2E — send outgoing wire messages on correct channels
  function sendOutgoing(msgs: OutgoingMsg[]) {
    if (!room) return;
    for (const msg of msgs) {
      // Decode hex payload to binary
      const bytes = new Uint8Array(msg.payload.length / 2);
      for (let i = 0; i < msg.payload.length; i += 2) {
        bytes[i / 2] = parseInt(msg.payload.slice(i, i + 2), 16);
      }
      room.sendOn(msg.channel, bytes);
    }
  }

  // KG: TASK_333_INT_CrdtSync — place_block through WASM SyncManager
  function placeBlock(x: number, y: number) {
    if (!room) return;
    const key = `${x},${y}`;
    const current = blocks.get(key);
    const newBlock = current === selectedBlock ? '' : selectedBlock;
    if (newBlock) blocks.set(key, newBlock); else blocks.delete(key);
    blocks = new Map(blocks);

    if (wasm) {
      // WASM path: place_block → SyncManager queues delta → poll_sync sends
      // executor가 직렬화하므로 fire-and-forget OK. 에러는 console로.
      wasm.placeBlock(key, newBlock || '').catch(e => console.error('placeBlock:', e));
    } else {
      // Fallback: raw JSON broadcast
      const msg = JSON.stringify({ type: 'block', x, y, block: newBlock });
      room.send(new TextEncoder().encode(msg));
    }
  }

  function copyRoomLink() {
    const url = `${window.location.origin}${base}/room?id=${roomId}`;
    navigator.clipboard.writeText(url);
    copied = true;
    log('Link copied to clipboard');
    setTimeout(() => { copied = false; }, 2000);
  }

  function log(msg: string) {
    messageLog = [...messageLog.slice(-19), msg];
  }

  // KG: TASK_333_E2E_HmrFix — cleanup on destroy (prevents HMR leak)
  onDestroy(() => {
    if (syncPollTimer) { clearInterval(syncPollTimer); syncPollTimer = null; }
  });

  function leaveRoom() {
    if (syncPollTimer) { clearInterval(syncPollTimer); syncPollTimer = null; }
    room = null; status = 'disconnected'; peerList = []; roomId = '';
    blocks = new Map();
    // Removed: history.replaceState conflicted with SvelteKit router
  }
</script>

<h2 class="page-title"><span class="accent">~</span> P2P Room</h2>

{#if !roomId}
  <div class="grid-2">
    <div class="card">
      <h3 class="card-heading">Create Room</h3>
      <p class="card-desc">
        Start a new P2P room. Share the link with others to connect directly.
      </p>
      <button class="btn" onclick={createRoom}>Create New Room</button>
    </div>
    <div class="card">
      <h3 class="card-heading">Join Room</h3>
      <p class="card-desc">
        Enter a room ID to join an existing session.
      </p>
      <div class="join-row">
        <input
          bind:value={joinId}
          placeholder="Room ID..."
          class="room-input"
          onkeydown={(e) => e.key === 'Enter' && joinRoom()}
        />
        <button class="btn" onclick={joinRoom}>Join</button>
      </div>
    </div>
  </div>
{:else}
  <!-- Room header -->
  <div class="card room-header">
    <div class="room-header__row">
      <div class="room-header__status">
        <span class="dot"
          class:dot--green={status === 'connected'}
          class:dot--yellow={status === 'connecting'}
          class:dot--red={status === 'disconnected'}
        ></span>
        <strong>{status}</strong>
      </div>
      <div class="stat-box">
        <span class="stat-val">{roomId}</span>
        <span class="stat-label">Room</span>
      </div>
      <div class="stat-box">
        <span class="stat-val">{peerList.length}</span>
        <span class="stat-label">Peers</span>
      </div>
      <div class="room-header__actions">
        <button class="btn btn--outline" onclick={copyRoomLink}>
          {copied ? 'Copied!' : 'Share URL'}
        </button>
        <button class="btn btn--danger" onclick={leaveRoom}>Leave</button>
      </div>
    </div>

    <!-- Peer list with status dots -->
    {#if peerList.length > 0}
      <div class="peer-list">
        {#each peerList as peer}
          <span class="peer-badge">
            <span class="dot dot--green dot--sm"></span>
            {peer.username}
          </span>
        {/each}
      </div>
    {:else}
      <p class="peer-empty">Waiting for peers to connect...</p>
    {/if}
  </div>

  <!-- Block world -->
  <div class="card" style="margin-top:1rem">
    <h3 class="card-heading">Block World -- CRDT Sync</h3>
    <div class="block-toolbar">
      {#each blockTypes as bt}
        <button
          class="block-select"
          class:block-select--active={selectedBlock === bt}
          style="background:{blockColors[bt]}"
          onclick={() => selectedBlock = bt}
        >{bt}</button>
      {/each}
    </div>
    <div class="block-grid">
      {#each Array(GRID) as _, y}
        {#each Array(GRID) as _, x}
          {@const key = `${x},${y}`}
          {@const block = blocks.get(key)}
          <button
            class="block"
            style="background:{block ? blockColors[block] : blockColors.empty}"
            onclick={() => placeBlock(x, y)}
            aria-label="Block {x},{y}"
          ></button>
        {/each}
      {/each}
    </div>
  </div>

  <!-- Network log -->
  <div class="card" style="margin-top:1rem">
    <h3 class="card-heading">Network Log</h3>
    <div class="log">
      {#each messageLog as msg}
        <div>{msg}</div>
      {/each}
      {#if messageLog.length === 0}
        <div class="log--empty">Waiting for activity...</div>
      {/if}
    </div>
  </div>

  <!-- KG: TASK_333_INT_E2E — Debug UI Panel (6 indicators) -->
  {#if wasm}
  <dl class="debug-panel">
    <div><dt>Node ID</dt><dd>{consensusState.nodeId}</dd></div>
    <div><dt>View</dt><dd>{consensusState.view}</dd></div>
    <div><dt>Leader</dt><dd>{consensusState.isLeader ? 'YES' : 'no'}</dd></div>
    <div><dt>Committed</dt><dd>{consensusState.committedBlocks}</dd></div>
    <div><dt>World Size</dt><dd>{consensusState.worldSize}</dd></div>
    <div><dt>Sync Queue</dt><dd>{consensusState.syncPending}</dd></div>
  </dl>
  {/if}
{/if}

<style>
  /* KG: CONTRACT_333_FE_PeerDiscovery — Styles */
  .page-title { margin-bottom: 1rem; }
  .accent { color: var(--pink); font-family: 'JetBrains Mono', monospace; }
  .card-heading { color: var(--purple); margin-bottom: 0.75rem; }
  .card-desc { color: var(--text-dim); font-size: 0.85rem; margin-bottom: 1rem; }
  .join-row { display: flex; gap: 0.5rem; }
  .room-input {
    flex: 1; padding: 0.6rem 0.8rem;
    background: rgba(0,0,0,0.3); border: 1px solid var(--border);
    border-radius: 8px; color: var(--text); font-size: 0.9rem; outline: none;
  }
  .room-input:focus { border-color: var(--purple); }

  .room-header { margin-bottom: 0; }
  .room-header__row {
    display: flex; align-items: center; gap: 1rem; flex-wrap: wrap;
  }
  .room-header__status { display: flex; align-items: center; gap: 0.4rem; }
  .room-header__actions { margin-left: auto; display: flex; gap: 0.5rem; }
  .btn--danger { background: var(--red); }
  .btn--danger:hover { background: #dc2626; }

  .peer-list {
    display: flex; gap: 0.5rem; margin-top: 0.75rem; flex-wrap: wrap;
  }
  .peer-badge {
    display: inline-flex; align-items: center; gap: 0.3rem;
    background: rgba(167,139,250,0.15); border: 1px solid rgba(167,139,250,0.3);
    padding: 0.2rem 0.6rem; border-radius: 20px; font-size: 0.8rem;
  }
  .peer-empty { color: var(--text-muted); font-size: 0.8rem; margin-top: 0.5rem; }
  .dot--sm { width: 6px; height: 6px; }

  .stat-box { text-align: center; }

  .block-toolbar { display: flex; gap: 0.5rem; margin-bottom: 0.75rem; }
  .block-grid { display: grid; grid-template-columns: repeat(8, 36px); gap: 2px; }
  .block {
    width: 36px; height: 36px; border-radius: 4px; border: none; cursor: pointer;
    transition: transform 0.1s;
  }
  .block:hover { transform: scale(1.1); }
  .block-select {
    padding: 0.3rem 0.7rem; border: 2px solid transparent; border-radius: 6px;
    color: var(--bg); font-size: 0.75rem; font-weight: 700; cursor: pointer;
    text-transform: uppercase;
  }
  .block-select--active { border-color: var(--gold); box-shadow: 0 0 10px rgba(251,191,36,0.4); }

  .log {
    font-family: 'JetBrains Mono', monospace; font-size: 0.75rem;
    max-height: 150px; overflow-y: auto; color: var(--cyan);
  }
  .log--empty { color: var(--text-muted); }
  .debug-panel {
    margin-top: 1rem; padding: 0.75rem; border: 1px solid var(--border);
    border-radius: 8px; font-size: 0.75rem; font-family: monospace;
    display: grid; grid-template-columns: repeat(3, 1fr); gap: 0.5rem;
  }
  .debug-panel dt { color: var(--text-muted); }
  .debug-panel dd { color: var(--cyan); margin: 0; }
</style>
