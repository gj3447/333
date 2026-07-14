<!-- # KG: seed-post-rts-ui-skeleton-2026-04-15 -->
<!-- # KG: sprint3-3C-game-rules-graphics-2026-04-15 — Canvas 2D rendering + click select + winner overlay -->
<!-- KG: seed-rts-integration-wiring-2026-04-15 — RTS lockstep+CRDT hybrid UI -->
<!-- KG: CONTRACT_333_FE_RtsSession, SPAN_333_FE_RtsMatch -->
<script lang="ts">
  // KG: seed-post-rts-ui-skeleton-2026-04-15
  // # KG: sprint3-3C-game-rules-graphics-2026-04-15
  import { onMount, onDestroy } from 'svelte';
  import { page } from '$app/state';
  import { base } from '$app/paths';
  import { createRoomState, generateRoomId, type RoomState, type ConnectionStatus } from '$lib/room-state';
  import { loadIdentity } from '$lib/identity';
  import { RtsController, type RtsSessionState, getTacticalUnits } from './rts_controller';

  // ---------------------------------------------------------------------------
  // Page data (injected by +page.ts load)
  // ---------------------------------------------------------------------------
  let { data } = $props();

  // ---------------------------------------------------------------------------
  // State — Lobby
  // ---------------------------------------------------------------------------
  let myId = $state('');
  let roomId = $state<string>('');
  let joinId = $state('');
  let signalingStatus: ConnectionStatus = $state('disconnected');
  let peerList: Array<{ id: string; username: string }> = $state([]);
  let room: RoomState | null = $state(null);
  let sigPollTimer: ReturnType<typeof setInterval> | null = null;

  // ---------------------------------------------------------------------------
  // State — RTS session
  // ---------------------------------------------------------------------------
  let sessionActive = $state(false);
  let rtsState: RtsSessionState | null = $state(null);
  let ctrl: RtsController | null = null;

  // ---------------------------------------------------------------------------
  // Canvas rendering
  // # KG: sprint3-3C-game-rules-graphics-2026-04-15
  // ---------------------------------------------------------------------------
  const CANVAS_SIZE = 600;
  const MAP_MAX_RAW = 10000 * 65536; // Fixed32 raw for 10000 world units
  const UNIT_RADIUS_PX = 8; // visual radius in canvas pixels
  const HP_BAR_W = 18;
  const HP_BAR_H = 3;
  // 4 owner colors
  const OWNER_COLORS = ['#a78bfa', '#34d399', '#f472b6', '#fbbf24'];

  let canvasEl: HTMLCanvasElement | null = $state(null);
  let rafId: number | null = null;

  function worldToCanvas(raw: number): number {
    return (raw / MAP_MAX_RAW) * CANVAS_SIZE;
  }

  function renderCanvas() {
    if (!canvasEl || !rtsState) return;
    const ctx = canvasEl.getContext('2d');
    if (!ctx) return;

    // Clear
    ctx.clearRect(0, 0, CANVAS_SIZE, CANVAS_SIZE);

    // Map background
    ctx.fillStyle = 'rgba(10,10,18,0.95)';
    ctx.fillRect(0, 0, CANVAS_SIZE, CANVAS_SIZE);

    // Grid (subtle)
    ctx.strokeStyle = 'rgba(255,255,255,0.04)';
    ctx.lineWidth = 1;
    for (let i = 0; i <= 10; i++) {
      const pos = (i / 10) * CANVAS_SIZE;
      ctx.beginPath(); ctx.moveTo(pos, 0); ctx.lineTo(pos, CANVAS_SIZE); ctx.stroke();
      ctx.beginPath(); ctx.moveTo(0, pos); ctx.lineTo(CANVAS_SIZE, pos); ctx.stroke();
    }

    // Get display units from WASM tactical_summary if available, else use rtsState.units
    const summary = (ctrl as any)?.wasmSession
      ? (() => { try { return (ctrl as any).wasmSession.tactical_summary(); } catch { return null; } })()
      : null;

    const displayUnits = summary
      ? getTacticalUnits(summary, rtsState.selectedUnitIds)
      : rtsState.units.map((u, i) => ({
          id: u.id,
          owner: i % 4,
          cx: worldToCanvas(u.x),
          cy: worldToCanvas(u.y),
          hp: 100, maxHp: 100,
          selected: rtsState!.selectedUnitIds.has(u.id),
          alive: true,
        }));

    for (const u of displayUnits) {
      const color = OWNER_COLORS[u.owner % OWNER_COLORS.length] || '#ffffff';
      const cx = u.cx;
      const cy = u.cy;

      // Unit circle (half-transparent fill for radius visualization)
      ctx.beginPath();
      ctx.arc(cx, cy, UNIT_RADIUS_PX, 0, Math.PI * 2);
      ctx.fillStyle = color + '33'; // 20% alpha
      ctx.fill();

      // Solid circle
      ctx.beginPath();
      ctx.arc(cx, cy, UNIT_RADIUS_PX * 0.65, 0, Math.PI * 2);
      ctx.fillStyle = color;
      ctx.fill();

      // Selected: yellow outline
      if (u.selected) {
        ctx.beginPath();
        ctx.arc(cx, cy, UNIT_RADIUS_PX + 2, 0, Math.PI * 2);
        ctx.strokeStyle = '#fef08a';
        ctx.lineWidth = 2;
        ctx.stroke();
      }

      // HP bar
      const hpRatio = u.maxHp > 0 ? u.hp / u.maxHp : 0;
      const barX = cx - HP_BAR_W / 2;
      const barY = cy - UNIT_RADIUS_PX - 7;
      ctx.fillStyle = 'rgba(0,0,0,0.5)';
      ctx.fillRect(barX, barY, HP_BAR_W, HP_BAR_H);
      ctx.fillStyle = hpRatio > 0.5 ? '#34d399' : hpRatio > 0.25 ? '#fbbf24' : '#ef4444';
      ctx.fillRect(barX, barY, HP_BAR_W * hpRatio, HP_BAR_H);

      // Unit ID label
      ctx.fillStyle = '#ffffff';
      ctx.font = '8px monospace';
      ctx.textAlign = 'center';
      ctx.fillText(`U${u.id}`, cx, cy + UNIT_RADIUS_PX + 10);
    }

    // Winner overlay
    if (rtsState.winner !== null) {
      ctx.fillStyle = 'rgba(0,0,0,0.65)';
      ctx.fillRect(0, 0, CANVAS_SIZE, CANVAS_SIZE);
      ctx.fillStyle = '#fef08a';
      ctx.font = 'bold 28px monospace';
      ctx.textAlign = 'center';
      const winText = rtsState.winner === 0
        ? 'Draw!'
        : `Winner: Player ${rtsState.winner}`;
      ctx.fillText(winText, CANVAS_SIZE / 2, CANVAS_SIZE / 2);
      ctx.fillStyle = '#a78bfa';
      ctx.font = '14px monospace';
      ctx.fillText('End Match to restart', CANVAS_SIZE / 2, CANVAS_SIZE / 2 + 32);
    }
  }

  function startRenderLoop() {
    const tick = () => {
      renderCanvas();
      rafId = requestAnimationFrame(tick);
    };
    rafId = requestAnimationFrame(tick);
  }

  function stopRenderLoop() {
    if (rafId !== null) { cancelAnimationFrame(rafId); rafId = null; }
  }

  // Canvas click → select nearest unit
  function handleCanvasClick(e: MouseEvent) {
    if (!canvasEl || !ctrl || !rtsState) return;
    const rect = canvasEl.getBoundingClientRect();
    const mx = (e.clientX - rect.left) * (CANVAS_SIZE / rect.width);
    const my = (e.clientY - rect.top) * (CANVAS_SIZE / rect.height);

    // Find closest unit within click radius
    let closestId = -1;
    let closestDist = UNIT_RADIUS_PX * 2 + 4;
    for (const u of rtsState.units) {
      const cx = worldToCanvas(u.x);
      const cy = worldToCanvas(u.y);
      const d = Math.hypot(cx - mx, cy - my);
      if (d < closestDist) { closestDist = d; closestId = u.id; }
    }

    if (closestId >= 0) {
      // Toggle selection: if already selected, deselect; else select
      if (rtsState.selectedUnitIds.has(closestId)) {
        ctrl?.setSelectedUnit(closestId, false);
      } else {
        ctrl?.setSelectedUnit(closestId, true);
      }
    }
  }

  // ---------------------------------------------------------------------------
  // WASD input tracking
  // ---------------------------------------------------------------------------
  const STEP = 1000; // Fixed32 1.0 unit
  const keysDown = new Set<string>();

  function getInput(): { dx: number; dy: number } {
    let dx = 0;
    let dy = 0;
    if (keysDown.has('a') || keysDown.has('ArrowLeft'))  dx -= STEP;
    if (keysDown.has('d') || keysDown.has('ArrowRight')) dx += STEP;
    if (keysDown.has('w') || keysDown.has('ArrowUp'))    dy -= STEP;
    if (keysDown.has('s') || keysDown.has('ArrowDown'))  dy += STEP;
    return { dx, dy };
  }

  function handleKeyDown(e: KeyboardEvent) {
    if (['w','a','s','d','ArrowUp','ArrowDown','ArrowLeft','ArrowRight'].includes(e.key)) {
      e.preventDefault();
      keysDown.add(e.key);
      ctrl?.setInput(getInput());
      // Apply WASD to selected units via select-aware input
      // # KG: sprint3-3C-game-rules-graphics-2026-04-15
      ctrl?.setSelectedInput(getInput(), rtsState?.selectedUnitIds ?? new Set());
    }
  }

  function handleKeyUp(e: KeyboardEvent) {
    keysDown.delete(e.key);
    ctrl?.setInput(getInput());
    ctrl?.setSelectedInput(getInput(), rtsState?.selectedUnitIds ?? new Set());
  }

  // ---------------------------------------------------------------------------
  // Signaling helpers — mirror room/+page.svelte pattern
  // ---------------------------------------------------------------------------
  function getSignalingUrl(): string {
    if (typeof window === 'undefined') return 'ws://localhost:8333';
    const override = (window as any).__333_signaling;
    if (typeof override === 'string' && override.length > 0) return override;
    return location.hostname === 'localhost'
      ? 'ws://localhost:8333'
      : 'wss://' + location.host + '/ws333/';
  }

  function peerIdToNodeId(peerId: string): number {
    let h = 0;
    for (let i = 0; i < peerId.length; i++) h = ((h << 5) - h + peerId.charCodeAt(i)) | 0;
    return (Math.abs(h) % 100) + 1;
  }

  // ---------------------------------------------------------------------------
  // Lifecycle
  // ---------------------------------------------------------------------------
  // # KG: sprint4E-remaining-wasm-autostart-2026-04-15 — ?auto=1 auto-start support
  onMount(async () => {
    const identity = await loadIdentity();
    myId = identity?.peerId || ('peer-' + Math.random().toString(36).slice(2, 10));

    // Initialize from page load data (URL params via +page.ts)
    const loadData = data as { roomId?: string; nodeId?: number; validators?: number[] };
    if (loadData.roomId) { roomId = loadData.roomId; joinId = loadData.roomId; joinRoom(); }

    const urlRoom = page.url.searchParams.get('room');
    if (!loadData.roomId && urlRoom) { joinId = urlRoom; joinRoom(); }

    window.addEventListener('keydown', handleKeyDown);
    window.addEventListener('keyup', handleKeyUp);

    // ?auto=1: automatic Create Room + Start Match (for harness / headless testing)
    // Preserves existing ?room= join logic: if room= given, we joined above; if not, create new.
    // ?force=1: allows startMatch() even if peerList is empty (single-node testing).
    const urlParams = page.url.searchParams;
    const autoMode = urlParams.get('auto') === '1';
    if (autoMode) {
      // Step 1: ensure we are in a room
      if (!roomId) {
        const autoRoom = urlParams.get('room') || generateRoomId();
        joinId = autoRoom;
        roomId = autoRoom;
        connectToRoom(autoRoom);
      }
      // Step 2: wait 1s for signaling to connect (best-effort), then startMatch
      await new Promise(r => setTimeout(r, 1000));
      if (!sessionActive) {
        startMatchForce();
      }
    }
  });

  onDestroy(() => {
    stopRenderLoop(); // # KG: sprint3-3C-game-rules-graphics-2026-04-15
    if (sigPollTimer) { clearInterval(sigPollTimer); sigPollTimer = null; }
    ctrl?.destroy();
    window.removeEventListener('keydown', handleKeyDown);
    window.removeEventListener('keyup', handleKeyUp);
  });

  // ---------------------------------------------------------------------------
  // Lobby actions
  // ---------------------------------------------------------------------------
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
    room = createRoomState(id, myId, sigUrl);
    signalingStatus = 'connecting';

    sigPollTimer = setInterval(() => {
      if (!room) { clearInterval(sigPollTimer!); sigPollTimer = null; return; }
      signalingStatus = room.status;
      peerList = Array.from(room.peers.values());
    }, 500);
  }

  function leaveRoom() {
    if (sigPollTimer) { clearInterval(sigPollTimer); sigPollTimer = null; }
    endMatch();
    room = null;
    signalingStatus = 'disconnected';
    peerList = [];
    roomId = '';
  }

  // ---------------------------------------------------------------------------
  // Match actions
  // ---------------------------------------------------------------------------
  function startMatch() {
    if (sessionActive) return;
    sessionActive = true;

    const peers = peerList.map(p => p.id);
    const seed = peerIdToNodeId(myId);

    ctrl = new RtsController((s) => { rtsState = s; });
    ctrl.initRtsSession(peers, seed);
    // Start canvas render loop after session is initialized
    // # KG: sprint3-3C-game-rules-graphics-2026-04-15
    startRenderLoop();

    // TODO_WASM: Add demo AFK peer for eject panel demo
    setTimeout(() => {
      ctrl?.addAfkPeer('peer-afk-demo1234');
    }, 2000);
  }

  // # KG: sprint4E-remaining-wasm-autostart-2026-04-15
  // startMatchForce — like startMatch but bypasses signalingStatus check.
  // Used by ?auto=1 mode (harness / headless testing) so a single-node tab
  // can enter match state without waiting for peers.
  function startMatchForce() {
    if (sessionActive) return;
    sessionActive = true;

    // In auto/force mode, use URL seed param if available for determinism
    const urlSeed = parseInt(page.url.searchParams.get('seed') || '0', 10);
    const peers = peerList.map(p => p.id);
    const seed = urlSeed > 0 ? urlSeed : peerIdToNodeId(myId);

    ctrl = new RtsController((s) => { rtsState = s; });
    ctrl.initRtsSession(peers, seed);
    startRenderLoop();
  }

  function endMatch() {
    stopRenderLoop(); // # KG: sprint3-3C-game-rules-graphics-2026-04-15
    ctrl?.destroy();
    ctrl = null;
    sessionActive = false;
    rtsState = null;
  }

  // ---------------------------------------------------------------------------
  // Eject vote
  // ---------------------------------------------------------------------------
  function voteEject(peerId: string) {
    ctrl?.voteEject(peerId);
  }

  // ---------------------------------------------------------------------------
  // Derived helpers
  // ---------------------------------------------------------------------------
  function fmtFixed(v: number): string {
    const sign = v < 0 ? '-' : '';
    const abs = Math.abs(v);
    return `${sign}${Math.floor(abs / 1000)}.${String(abs % 1000).padStart(3, '0')}`;
  }

  function logLineClass(line: string): string {
    if (line.startsWith('[DESYNC]') || line.includes('ERROR')) return 'log-error';
    if (line.startsWith('[bft]') || line.startsWith('[eject]')) return 'log-warn';
    return 'log-info';
  }
</script>

<div class="rts-page">
  <h2 class="page-title"><span class="accent">~</span> RTS — lockstep + CRDT hybrid</h2>

  <!-- ======================================================================
       SECTION 1: Lobby
  ====================================================================== -->
  <div class="card">
    <p class="section-title">1 · Lobby</p>

    {#if !roomId}
      <div class="lobby-form">
        <button class="btn" onclick={createRoom}>Create Room</button>
        <input
          class="rts-input"
          bind:value={joinId}
          placeholder="Room ID..."
          onkeydown={(e) => e.key === 'Enter' && joinRoom()}
        />
        <button class="btn btn--outline" onclick={joinRoom}>Join</button>
      </div>
    {:else}
      <div class="lobby-form">
        <span class="dot"
          class:dot--green={signalingStatus === 'connected'}
          class:dot--yellow={signalingStatus === 'connecting'}
          class:dot--red={signalingStatus === 'disconnected'}
        ></span>
        <strong style="color:var(--gold)">{roomId}</strong>
        <span style="color:var(--text-muted);font-size:0.8rem">{signalingStatus}</span>
        <div style="margin-left:auto;display:flex;gap:0.5rem">
          {#if !sessionActive}
            <button class="btn" onclick={startMatch} disabled={signalingStatus !== 'connected' && peerList.length === 0}>
              Start Match
            </button>
          {:else}
            <button class="btn btn--outline" onclick={endMatch}>End Match</button>
          {/if}
          <button class="btn btn--outline" style="--purple:#ef4444;color:var(--red);border-color:var(--red)" onclick={leaveRoom}>
            Leave
          </button>
        </div>
      </div>

      {#if peerList.length > 0}
        <div class="peer-list">
          {#each peerList as peer}
            <span class="peer-chip">
              <span class="dot dot--green" style="width:6px;height:6px"></span>
              {peer.username}
            </span>
          {/each}
        </div>
      {:else}
        <p class="empty-hint" style="margin-top:0.5rem">Waiting for peers...</p>
      {/if}
    {/if}
  </div>

  {#if sessionActive && rtsState}
    <!-- ====================================================================
         SECTION 2: Match — frame, HLC, units, WASD input
    ==================================================================== -->
    <div class="card">
      <p class="section-title">2 · Match</p>

      <div class="stat-row">
        <div class="stat-item">
          <span class="stat-key">Frame</span>
          <span class="stat-num">{rtsState.frame}</span>
        </div>
        <div class="stat-item">
          <span class="stat-key">HLC</span>
          <span class="stat-num" style="font-size:0.85rem;color:var(--cyan)">{rtsState.hlc}</span>
        </div>
        <div class="stat-item">
          <span class="stat-key">Units</span>
          <span class="stat-num">{rtsState.units.length}</span>
        </div>
      </div>

      <table class="unit-table">
        <thead>
          <tr>
            <th>ID</th>
            <th>X (Fixed32)</th>
            <th>Y (Fixed32)</th>
          </tr>
        </thead>
        <tbody>
          {#each rtsState.units as unit}
            <tr class:unit-highlight={unit.id === 0}>
              <td>U{unit.id}{unit.id === 0 ? ' *' : ''}</td>
              <td>{fmtFixed(unit.x)}</td>
              <td>{fmtFixed(unit.y)}</td>
            </tr>
          {/each}
        </tbody>
      </table>

      <p class="wasd-hint">
        Move selected units: <kbd>W</kbd><kbd>A</kbd><kbd>S</kbd><kbd>D</kbd> or arrow keys.
        Click unit on canvas to select/deselect.
        {#if rtsState.selectedUnitIds.size > 0}
          <span style="color:var(--gold)">Selected: {[...rtsState.selectedUnitIds].map(id => `U${id}`).join(', ')}</span>
        {:else}
          <span style="color:var(--pink)">No units selected — click canvas to select</span>
        {/if}
      </p>

      <!-- Canvas 2D map — # KG: sprint3-3C-game-rules-graphics-2026-04-15 -->
      <div class="canvas-wrap">
        <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
        <canvas
          bind:this={canvasEl}
          width={600}
          height={600}
          class="rts-canvas"
          onclick={handleCanvasClick}
        ></canvas>
        {#if rtsState.winner !== null}
          <div class="winner-badge">
            {rtsState.winner === 0 ? 'Draw!' : `Winner: Player ${rtsState.winner}`}
          </div>
        {/if}
      </div>
    </div>

    <!-- ====================================================================
         SECTION 3: Desync monitor — last 10 frame hashes, peer comparison
    ==================================================================== -->
    <div class="card">
      <p class="section-title">3 · Desync Monitor</p>

      {#if rtsState.hashHistory.length > 0}
        <table class="hash-table">
          <thead>
            <tr>
              <th>Frame</th>
              <th>Local Hash</th>
              <th>Peers</th>
              <th>Status</th>
            </tr>
          </thead>
          <tbody>
            {#each [...rtsState.hashHistory].reverse() as entry}
              <tr>
                <td>{entry.frame}</td>
                <td class={entry.desync ? 'hash-bad' : 'hash-ok'}>{entry.localHash.slice(0, 10)}</td>
                <td>
                  {#each [...entry.peerHashes.entries()] as [pid, h]}
                    <span class={entry.localHash === h ? 'hash-ok' : 'hash-bad'}>
                      {pid.slice(0, 6)}:{h.slice(0, 6)}
                    </span>
                  {/each}
                  {#if entry.peerHashes.size === 0}
                    <span style="color:var(--text-muted)">—</span>
                  {/if}
                </td>
                <td>
                  {#if entry.desync}
                    <span class="hash-bad">DESYNC</span>
                  {:else}
                    <span class="hash-ok">OK</span>
                  {/if}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      {:else}
        <p class="empty-hint">No frames yet...</p>
      {/if}

      {#if rtsState.desyncEvents.length > 0}
        {#each rtsState.desyncEvents.slice(-3) as ev}
          <div class="desync-alert">
            DESYNC frame={ev.frame} peer={ev.peerId.slice(0, 8)}
            local={ev.localHash.slice(0, 8)} peer={ev.peerHash.slice(0, 8)}
          </div>
        {/each}
      {/if}
    </div>

    <!-- ====================================================================
         SECTION 4: BFT Checkpoint
    ==================================================================== -->
    <div class="card">
      <p class="section-title">4 · BFT Checkpoint</p>

      {#if rtsState.checkpoint}
        <div class="qc-row">
          <div class="stat-item">
            <span class="stat-key">Last Checkpoint Frame</span>
            <span class="stat-num">{rtsState.checkpoint.frame}</span>
          </div>
          <div class="stat-item">
            <span class="stat-key">QC Status</span>
            <span class="qc-badge qc-badge--{rtsState.checkpoint.qcStatus}">
              {rtsState.checkpoint.qcStatus}
            </span>
          </div>
          <div class="stat-item">
            <span class="stat-key">Validators</span>
            <span class="stat-num" style="font-size:0.9rem">
              {rtsState.checkpoint.approved}/{rtsState.checkpoint.validators}
            </span>
          </div>
        </div>
      {:else}
        <p class="empty-hint">No checkpoint yet — fires every 30 frames.</p>
      {/if}

      <p style="margin-top:0.5rem;font-size:0.72rem;color:var(--text-muted)">
        <!-- TODO_WASM: wire to BftGgrsSession checkpoint + QC aggregation from rts_session.rs -->
        TODO_WASM: connect to RtsSession.ggrs save_state + BFT quorum verification.
      </p>
    </div>

    <!-- ====================================================================
         SECTION 5: Peer eject (AFK voting)
    ==================================================================== -->
    <div class="card">
      <p class="section-title">5 · Peer Eject</p>

      {#if rtsState.ejectVotes.size > 0}
        <div class="eject-list">
          {#each [...rtsState.ejectVotes.values()] as vote}
            <div class="eject-row">
              <span class="eject-peer">{vote.peerId.slice(0, 16)}</span>
              <span style="font-size:0.7rem;color:var(--text-muted)">{vote.reason}</span>
              <div class="vote-bar">
                <div
                  class="vote-fill"
                  style="width:{Math.min(100, (vote.votes / vote.threshold) * 100)}%"
                ></div>
              </div>
              <span style="font-size:0.72rem;color:var(--red)">{vote.votes}/{vote.threshold}</span>
              <button class="btn btn--outline" style="padding:0.25rem 0.6rem;font-size:0.72rem"
                onclick={() => voteEject(vote.peerId)}>
                Vote
              </button>
            </div>
          {/each}
        </div>
      {:else}
        <p class="empty-hint">No AFK peers detected.</p>
      {/if}

      <p style="margin-top:0.5rem;font-size:0.72rem;color:var(--text-muted)">
        <!-- TODO_WASM: wire to RtsSession peer liveness tracking + BFT-signed eject proposal -->
        TODO_WASM: connect to liveness heartbeat and BFT-signed eject vote broadcast.
      </p>
    </div>

    <!-- ====================================================================
         SECTION 6: Debug log
    ==================================================================== -->
    <div class="card">
      <p class="section-title">6 · Debug Log</p>
      <div class="debug-log">
        {#each rtsState.log.slice(-50).reverse() as line}
          <div class="log-line {logLineClass(line)}">{line}</div>
        {/each}
        {#if rtsState.log.length === 0}
          <div class="log-line">waiting for events...</div>
        {/if}
      </div>
    </div>
  {/if}

  {#if !sessionActive && roomId}
    <div class="card" style="text-align:center;color:var(--text-muted);font-size:0.85rem">
      Connect to signaling and press <strong style="color:var(--purple)">Start Match</strong> to begin the RTS session.
    </div>
  {/if}
</div>

<style>
  /* KG: seed-post-rts-ui-skeleton-2026-04-15 — scoped styles (import SCSS separately if build supports) */
  .rts-page { display: flex; flex-direction: column; gap: 1rem; }

  .page-title { margin-bottom: 0.25rem; }
  .accent { color: var(--pink); font-family: 'JetBrains Mono', monospace; }

  .section-title {
    font-size: 0.68rem; font-weight: 700; letter-spacing: 0.1em;
    text-transform: uppercase; color: var(--text-muted); margin-bottom: 0.6rem;
  }

  /* Lobby */
  .lobby-form { display: flex; gap: 0.5rem; flex-wrap: wrap; align-items: center; }
  .rts-input {
    flex: 1; min-width: 140px; padding: 0.55rem 0.8rem;
    background: rgba(0,0,0,0.35); border: 1px solid var(--border);
    border-radius: 8px; color: var(--text); font-size: 0.88rem; outline: none;
  }
  .rts-input:focus { border-color: var(--purple); }

  .peer-list { display: flex; gap: 0.4rem; flex-wrap: wrap; margin-top: 0.6rem; }
  .peer-chip {
    display: inline-flex; align-items: center; gap: 0.3rem;
    padding: 0.2rem 0.6rem;
    background: rgba(167,139,250,0.12); border: 1px solid rgba(167,139,250,0.25);
    border-radius: 20px; font-size: 0.75rem; color: var(--purple);
  }

  /* Stats */
  .stat-row { display: flex; gap: 1.5rem; flex-wrap: wrap; margin-bottom: 0.75rem; }
  .stat-item { display: flex; flex-direction: column; gap: 0.1rem; }
  .stat-key { font-size: 0.65rem; text-transform: uppercase; color: var(--text-muted); letter-spacing: 0.08em; }
  .stat-num { font-size: 1.1rem; font-weight: 800; color: var(--gold); font-family: 'JetBrains Mono', monospace; }

  /* Unit table */
  .unit-table {
    width: 100%; border-collapse: collapse;
    font-size: 0.78rem; font-family: 'JetBrains Mono', monospace;
  }
  .unit-table th {
    text-align: left; padding: 0.3rem 0.5rem; color: var(--text-muted);
    font-weight: 600; border-bottom: 1px solid var(--border);
    font-size: 0.68rem; text-transform: uppercase;
  }
  .unit-table td { padding: 0.35rem 0.5rem; color: var(--cyan); }
  .unit-highlight td { color: var(--pink) !important; font-weight: 700; }
  .wasd-hint { margin-top: 0.5rem; font-size: 0.72rem; color: var(--text-muted); }
  .wasd-hint kbd {
    display: inline-block; padding: 0.1rem 0.35rem;
    background: rgba(255,255,255,0.08); border: 1px solid var(--border);
    border-radius: 4px; font-family: 'JetBrains Mono', monospace; font-size: 0.68rem;
  }

  /* Hash table */
  .hash-table {
    width: 100%; border-collapse: collapse;
    font-size: 0.72rem; font-family: 'JetBrains Mono', monospace;
  }
  .hash-table th {
    text-align: left; padding: 0.25rem 0.4rem; color: var(--text-muted);
    font-size: 0.65rem; text-transform: uppercase; border-bottom: 1px solid var(--border);
  }
  .hash-table td { padding: 0.3rem 0.4rem; color: var(--cyan); }
  .hash-ok { color: var(--green) !important; }
  .hash-bad { color: var(--red) !important; font-weight: 700; }
  .desync-alert {
    margin-top: 0.5rem; padding: 0.4rem 0.75rem;
    background: rgba(239,68,68,0.1); border: 1px solid rgba(239,68,68,0.35);
    border-radius: 6px; font-size: 0.75rem; color: var(--red);
    font-family: 'JetBrains Mono', monospace;
  }

  /* BFT */
  .qc-row { display: flex; align-items: center; gap: 1rem; flex-wrap: wrap; }
  .qc-badge {
    display: inline-block; padding: 0.25rem 0.65rem; border-radius: 6px;
    font-size: 0.72rem; font-weight: 700; text-transform: uppercase;
  }
  .qc-badge--approved { background: rgba(52,211,153,0.15); color: var(--green); }
  .qc-badge--pending { background: rgba(251,191,36,0.15); color: var(--gold); }
  .qc-badge--failed { background: rgba(239,68,68,0.15); color: var(--red); }

  /* Eject */
  .eject-list { display: flex; flex-direction: column; gap: 0.5rem; }
  .eject-row {
    display: flex; align-items: center; gap: 0.75rem;
    padding: 0.4rem 0.6rem;
    background: rgba(0,0,0,0.2); border: 1px solid var(--border);
    border-radius: 8px; font-size: 0.78rem;
  }
  .eject-peer { color: var(--text-dim); flex: 1; font-family: 'JetBrains Mono', monospace; }
  .vote-bar {
    height: 6px; border-radius: 3px;
    background: rgba(255,255,255,0.08); width: 80px; overflow: hidden;
  }
  .vote-fill { height: 100%; border-radius: 3px; background: var(--red); }

  /* Debug log */
  .debug-log {
    font-family: 'JetBrains Mono', monospace; font-size: 0.72rem;
    max-height: 200px; overflow-y: auto; line-height: 1.6;
  }
  .log-line { color: var(--text-dim); padding: 0.05rem 0; }
  .log-line.log-error { color: var(--red); }
  .log-line.log-warn { color: var(--gold); }
  .log-line.log-info { color: var(--cyan); }

  .empty-hint { color: var(--text-muted); font-size: 0.78rem; }

  /* Canvas — # KG: sprint3-3C-game-rules-graphics-2026-04-15 */
  .canvas-wrap {
    position: relative;
    display: inline-block;
    margin-top: 0.75rem;
    border-radius: 8px;
    overflow: hidden;
    border: 1px solid var(--border);
  }
  .rts-canvas {
    display: block;
    width: 600px;
    height: 600px;
    cursor: crosshair;
    background: #0a0a12;
  }
  .winner-badge {
    position: absolute;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    background: rgba(0,0,0,0.82);
    border: 2px solid var(--gold);
    border-radius: 12px;
    padding: 1rem 2rem;
    color: var(--gold);
    font-size: 1.4rem;
    font-weight: 800;
    font-family: 'JetBrains Mono', monospace;
    text-align: center;
    pointer-events: none;
  }
</style>
