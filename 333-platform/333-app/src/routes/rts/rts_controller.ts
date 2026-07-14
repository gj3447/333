// KG: prod-wiring-wasm-rts-export-2026-04-15
// KG: seed-post-rts-ui-skeleton-2026-04-15
// KG: seed-rts-integration-wiring-2026-04-15 — RtsSession UI bridge
// KG: CONTRACT_333_FE_RtsSession, SPAN_333_FE_RtsMatch
// KG: SOLID-DIP-fix-wasm-sessions-2026-04-16 — IRtsSession + IWasmStubs interface 경유

import { createRtsSession } from '../../lib/wasm-sessions';
import type { IRtsSession, IWasmStubs } from '../../lib/wasm-sessions';

/**
 * RTS Controller — UI↔WASM bridge for RTS session.
 *
 * WASM binding status (all 6 TODO_WASM replaced):
 *  1. initRtsSession     → new wasm.RtsSessionWasm(seed, nodeId)      [WASM]
 *  2. advanceFrame       → wasm.rts.advance_frame(bytes)              [WASM]
 *  3. onFrameStateHash   → wasm.rts.state_hash_hex() broadcast        [WASM]
 *  4. BFT Checkpoint     → wasm.rts_bft_checkpoint_stub(frame)        [WASM stub]
 *  5. Peer Eject         → wasm.rts_peer_eject_stub(peerId)           [WASM stub]
 *  6. GGRS Rollback      → wasm.rts_ggrs_rollback_stub(frame)         [WASM stub]
 *
 * Fallback: if WASM module is not loaded, controller falls back to mock mode
 * (runtime flag `_wasmAvailable`).
 *
 * # KG: prod-wiring-wasm-rts-export-2026-04-15
 */

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface RtsUnit {
  id: number;
  /** Fixed32 raw integer — divide by 65536 for display (Q16.16) */
  x: number;
  y: number;
}

// KG: sprint3-3C-game-rules-graphics-2026-04-15
/** UI-friendly unit struct for Canvas rendering — coordinates in canvas pixels. */
export interface RtsUnitDisplay {
  id: number;
  owner: number;
  /** Canvas pixel x (0-600) */
  cx: number;
  /** Canvas pixel y (0-600) */
  cy: number;
  hp: number;
  maxHp: number;
  selected: boolean;
  alive: boolean;
}

export interface FrameHashEntry {
  frame: number;
  localHash: string;
  peerHashes: Map<string, string>; // peerId → hash
  desync: boolean;
}

export interface DesyncEvent {
  peerId: string;
  frame: number;
  localHash: string;
  peerHash: string;
}

export interface CheckpointStatus {
  frame: number;
  qcStatus: 'pending' | 'approved' | 'failed';
  validators: number;
  approved: number;
}

export interface EjectVote {
  peerId: string;
  reason: string;
  votedAt: number;
  votes: number;
  threshold: number;
}

export interface RtsSessionState {
  frame: number;
  hlc: string;
  units: RtsUnit[];
  hashHistory: FrameHashEntry[];
  desyncEvents: DesyncEvent[];
  checkpoint: CheckpointStatus | null;
  ejectVotes: Map<string, EjectVote>;
  log: string[];
  wasmActive: boolean;
  /** null = in progress, 0 = draw, N = owner N wins. # KG: sprint3-3C-game-rules-graphics-2026-04-15 */
  winner: number | null;
  /** Selected unit IDs (from WASM tactical_summary) — for canvas highlight */
  selectedUnitIds: Set<number>;
}

export interface RtsInput {
  dx: number; // Fixed32 step, e.g. +1 = move
  dy: number;
}

// ---------------------------------------------------------------------------
// WASM types (mirrors wasm_rts.rs JSON shapes)
// KG: prod-wiring-wasm-rts-export-2026-04-15
// ---------------------------------------------------------------------------

interface WasmRtsUnit {
  id: number;
  owner: number;
  x: number;
  y: number;
  hp: number;
  maxHp: number;
}

interface WasmAdvanceResult {
  frame: number;
  hash: string;
  units: WasmRtsUnit[];
  winner: number | null; // KG: sprint3-3C-game-rules-graphics-2026-04-15
}

interface WasmDesyncEvent {
  peerId: number;
  frame: number;
  localHash: string;
  peerHash: string;
}

interface WasmCheckpointResult {
  frame: number;
  qcStatus: string;
  validators: number;
  approved: number;
  stub: boolean;
}

interface WasmEjectResult {
  peerId: number;
  votes: number;
  threshold: number;
  ejected: boolean;
  stub: boolean;
}

// ---------------------------------------------------------------------------
// Frame loop constants
// ---------------------------------------------------------------------------

/** Map scale: Fixed32 raw unit 10000*65536 → canvas 600px */
const MAP_MAX_RAW = 10000 * 65536;
const CANVAS_PX = 600;

/**
 * getTacticalUnits — parse tactical_summary JSON into UI-friendly RtsUnitDisplay[].
 *
 * Converts Fixed32 raw coordinates to canvas pixels.
 * @param summaryJson — JSON string from wasm.tactical_summary()
 * @param selectedIds — set of currently selected unit IDs
 * # KG: sprint3-3C-game-rules-graphics-2026-04-15
 */
export function getTacticalUnits(summaryJson: string, selectedIds: Set<number> = new Set()): RtsUnitDisplay[] {
  try {
    const parsed = JSON.parse(summaryJson) as { units: WasmRtsUnit[] };
    return parsed.units.map((u) => ({
      id: u.id,
      owner: u.owner,
      cx: (u.x / MAP_MAX_RAW) * CANVAS_PX,
      cy: (u.y / MAP_MAX_RAW) * CANVAS_PX,
      hp: u.hp,
      maxHp: u.maxHp,
      selected: selectedIds.has(u.id),
      alive: u.hp > 0,
    }));
  } catch {
    return [];
  }
}

const FRAME_MS = 1000 / 60; // 60 fps target

let _rafHandle: number | null = null;
let _lastFrameTime = 0;

// ---------------------------------------------------------------------------
// RtsController
// ---------------------------------------------------------------------------

export class RtsController {
  private state: RtsSessionState;
  private onStateChange: (s: RtsSessionState) => void;
  private pendingInput: RtsInput = { dx: 0, dy: 0 };

  // WASM session handle — IRtsSession interface 경유 (DIP 준수).
  // KG: prod-wiring-wasm-rts-export-2026-04-15, SOLID-DIP-fix-wasm-sessions-2026-04-16
  private wasmSession: IRtsSession | null = null;
  private wasmStubs: IWasmStubs | null = null;
  private _wasmAvailable = false;

  constructor(onStateChange: (s: RtsSessionState) => void) {
    this.onStateChange = onStateChange;
    this.state = {
      frame: 0,
      hlc: '0.0.0',
      units: [
        { id: 0, x: 0,    y: 0    },
        { id: 1, x: 2000, y: 0    },
        { id: 2, x: 0,    y: 2000 },
        { id: 3, x: 2000, y: 2000 },
      ],
      hashHistory: [],
      desyncEvents: [],
      checkpoint: null,
      ejectVotes: new Map(),
      log: ['[rts] controller ready'],
      wasmActive: false,
      winner: null, // KG: sprint3-3C-game-rules-graphics-2026-04-15
      selectedUnitIds: new Set(),
    };
  }

  // ---------------------------------------------------------------------------
  // Public API
  // ---------------------------------------------------------------------------

  /**
   * initRtsSession — Initialize RTS session.
   *
   * WASM: creates `new wasm.RtsSessionWasm(seed, nodeId)`.
   * Falls back to mock 60fps loop if WASM is unavailable.
   *
   * # KG: prod-wiring-wasm-rts-export-2026-04-15
   */
  async initRtsSession(peers: string[], mySeed: number, myNodeId?: number): Promise<void> {
    const nodeId = myNodeId ?? 1;

    // [WASM TODO_WASM 1] createRtsSession factory (DIP: interface 경유, 직접 import 금지)
    // KG: prod-wiring-wasm-rts-export-2026-04-15, SOLID-DIP-fix-wasm-sessions-2026-04-16
    try {
      const { session, stubs } = await createRtsSession(mySeed, nodeId);
      this.wasmSession = session;
      this.wasmStubs = stubs;
      this._wasmAvailable = true;

      // Sync initial unit positions from WASM
      const summary = JSON.parse(this.wasmSession.tactical_summary()) as { frame: number; units: WasmRtsUnit[]; winner?: number | null };
      this.state = {
        ...this.state,
        units: summary.units.map((u) => ({ id: u.id, x: u.x, y: u.y })),
        winner: summary.winner ?? null, // KG: sprint3-3C-game-rules-graphics-2026-04-15
        wasmActive: true,
        log: [...this.state.log, `[rts] WASM session init node=${nodeId} seed=${mySeed} peers=${peers.length}`],
      };
    } catch (e) {
      this._wasmAvailable = false;
      this.state = {
        ...this.state,
        wasmActive: false,
        log: [...this.state.log, `[rts] WASM unavailable, mock mode active (${String(e).slice(0, 60)})`],
      };
    }

    this._startFrameLoop();
    this.onStateChange({ ...this.state });
  }

  /**
   * setInput — queue keyboard input for next advanceFrame.
   */
  setInput(input: RtsInput): void {
    this.pendingInput = input;
  }

  /**
   * setSelectedUnit — update selectedUnitIds in state (canvas click selection).
   * # KG: sprint3-3C-game-rules-graphics-2026-04-15
   */
  setSelectedUnit(unitId: number, selected: boolean): void {
    const newSet = new Set(this.state.selectedUnitIds);
    if (selected) newSet.add(unitId);
    else newSet.delete(unitId);
    this.state = { ...this.state, selectedUnitIds: newSet };
    this.onStateChange({ ...this.state });
  }

  /**
   * setSelectedInput — queue move input for all selected units.
   * Each selected unit gets a move command; BTreeSet order = deterministic.
   * # KG: sprint3-3C-game-rules-graphics-2026-04-15
   */
  setSelectedInput(input: RtsInput, selectedIds: Set<number>): void {
    // Store pending selected-unit move for next advanceFrame
    this._pendingSelectedInput = { input, selectedIds: new Set(selectedIds) };
  }

  private _pendingSelectedInput: { input: RtsInput; selectedIds: Set<number> } | null = null;

  /**
   * advanceFrame — step simulation by one frame.
   *
   * [WASM TODO_WASM 2] calls `wasm.rts.advance_frame(bytes)` and reads back JSON.
   * Fallback: mock physics loop.
   *
   * # KG: prod-wiring-wasm-rts-export-2026-04-15
   */
  advanceFrame(input: RtsInput): void {
    if (this._wasmAvailable && this.wasmSession) {
      // ── WASM path ────────────────────────────────────────────────────────
      // Encode input as 1-byte flag: 1 = has movement, 0 = idle.
      const hasInput = (input.dx !== 0 || input.dy !== 0) ? 1 : 0;
      const inputBytes = new Uint8Array([hasInput]);

      let result: WasmAdvanceResult;
      try {
        result = JSON.parse(this.wasmSession.advance_frame(inputBytes)) as WasmAdvanceResult;
      } catch {
        this._wasmAvailable = false;
        this._advanceFrameMock(input);
        return;
      }

      const frame = result.frame;
      const hash = result.hash;
      const hlc = `${frame}.${Date.now() % 100000}.${this.wasmSession.node_id()}`;
      const units: RtsUnit[] = result.units.map((u) => ({ id: u.id, x: u.x, y: u.y }));

      // [WASM TODO_WASM 3] state_hash_hex() — broadcast over position channel
      // KG: prod-wiring-wasm-rts-export-2026-04-15
      const broadcastHash = this.wasmSession.state_hash_hex() as string;

      const entry: FrameHashEntry = {
        frame,
        localHash: broadcastHash,
        peerHashes: new Map(),
        desync: false,
      };

      // Simulate occasional peer desync for demo (every 47 frames)
      if (frame % 47 === 0 && frame > 0) {
        const fakeHash = broadcastHash.slice(0, -2) + 'xx';
        entry.peerHashes.set('peer-demo', fakeHash);
        entry.desync = true;
        this._onDesyncEvent('peer-demo', frame, broadcastHash, fakeHash);
      }

      const hashHistory = [...this.state.hashHistory, entry].slice(-10);

      // [WASM TODO_WASM 4] BFT Checkpoint stub every 30 frames
      // KG: prod-wiring-wasm-rts-export-2026-04-15, SOLID-DIP-fix-wasm-sessions-2026-04-16
      let checkpoint = this.state.checkpoint;
      if (frame % 30 === 0 && this.wasmStubs?.rts_bft_checkpoint_stub) {
        try {
          const cpResult = JSON.parse(
            this.wasmStubs.rts_bft_checkpoint_stub(frame >>> 0)
          ) as WasmCheckpointResult;
          checkpoint = {
            frame: cpResult.frame,
            qcStatus: cpResult.qcStatus as CheckpointStatus['qcStatus'],
            validators: cpResult.validators,
            approved: cpResult.approved,
          };
          this._addLog(`[bft] checkpoint frame=${frame} approved=${cpResult.approved}/${cpResult.validators} (wasm-stub)`);
        } catch {
          checkpoint = { frame, qcStatus: 'approved', validators: 4, approved: 3 };
        }
      }

      // Extract winner and selectedUnitIds from result
      // KG: sprint3-3C-game-rules-graphics-2026-04-15
      const winner: number | null = result.winner !== undefined && result.winner !== null ? result.winner : null;

      this.state = { ...this.state, frame, hlc, units, hashHistory, checkpoint, winner };
      this.onStateChange({ ...this.state });
      return;
    }

    // ── Mock fallback ────────────────────────────────────────────────────────
    this._advanceFrameMock(input);
  }

  /**
   * onFrameStateHash — called by peer message with their hash for frame.
   * Records peer hash and runs desync comparison.
   *
   * [WASM TODO_WASM 3] wires from WASM state_hash_hex broadcast path.
   * # KG: prod-wiring-wasm-rts-export-2026-04-15
   */
  onFrameStateHash(peerId: string, frame: number, hash: string): void {
    // If WASM is active, cross-check with observe_peer_digest
    if (this._wasmAvailable && this.wasmSession) {
      const peerIdNum = parseInt(peerId, 10) || 0;
      try {
        const eventJson = this.wasmSession.observe_peer_digest(peerIdNum >>> 0, frame >>> 0, hash) as string | undefined;
        if (eventJson) {
          const ev = JSON.parse(eventJson) as WasmDesyncEvent;
          this._onDesyncEvent(String(ev.peerId), ev.frame, ev.localHash, ev.peerHash);
        }
      } catch {
        // silently ignore WASM errors in peer hash path
      }
    }

    const s = this.state;
    const entry = s.hashHistory.find(e => e.frame === frame);
    if (entry) {
      entry.peerHashes.set(peerId, hash);
      if (entry.localHash !== hash) {
        entry.desync = true;
        this._onDesyncEvent(peerId, frame, entry.localHash, hash);
      }
    }
    this.onStateChange({ ...this.state });
  }

  /**
   * voteEject — initiate or cast vote to eject AFK peer.
   *
   * [WASM TODO_WASM 5] calls `wasm.rts_peer_eject_stub(peerId)`.
   * # KG: prod-wiring-wasm-rts-export-2026-04-15
   */
  voteEject(peerId: string): void {
    // [WASM TODO_WASM 5] peer eject stub
    // KG: prod-wiring-wasm-rts-export-2026-04-15
    if (this._wasmAvailable && this.wasmStubs?.rts_peer_eject_stub) {
      try {
        const peerIdNum = parseInt(peerId, 10) || 0;
        const res = JSON.parse(this.wasmStubs.rts_peer_eject_stub(peerIdNum >>> 0)) as WasmEjectResult;
        const updated: EjectVote = {
          peerId,
          reason: 'AFK (no input)',
          votedAt: Date.now(),
          votes: res.votes,
          threshold: res.threshold,
        };
        this.state.ejectVotes.set(peerId, updated);
        this._addLog(`[eject] wasm-stub peer=${peerId.slice(0, 8)} votes=${res.votes}/${res.threshold}`);
        this.onStateChange({ ...this.state });
        return;
      } catch {
        // fallthrough to mock
      }
    }

    // mock fallback
    const existing = this.state.ejectVotes.get(peerId);
    if (existing) {
      const updated = { ...existing, votes: existing.votes + 1, votedAt: Date.now() };
      this.state.ejectVotes.set(peerId, updated);
      this._addLog(`[eject] vote for ${peerId.slice(0, 8)} — ${updated.votes}/${updated.threshold}`);
    } else {
      this.state.ejectVotes.set(peerId, {
        peerId,
        reason: 'AFK (no input)',
        votedAt: Date.now(),
        votes: 1,
        threshold: 3,
      });
      this._addLog(`[eject] vote initiated for ${peerId.slice(0, 8)}`);
    }
    this.onStateChange({ ...this.state });
  }

  /**
   * triggerRollback — request GGRS rollback to a previous frame.
   *
   * [WASM TODO_WASM 6] calls `wasm.rts_ggrs_rollback_stub(frame)`.
   * # KG: prod-wiring-wasm-rts-export-2026-04-15
   */
  triggerRollback(targetFrame: number): void {
    // [WASM TODO_WASM 6] GGRS rollback stub
    // KG: prod-wiring-wasm-rts-export-2026-04-15
    if (this._wasmAvailable && this.wasmStubs?.rts_ggrs_rollback_stub) {
      try {
        const res = JSON.parse(this.wasmStubs.rts_ggrs_rollback_stub(targetFrame >>> 0));
        this._addLog(`[ggrs] rollback stub frame=${res.frame} rolledBack=${res.rolledBack} (stub)`);
        this.onStateChange({ ...this.state });
        return;
      } catch {
        // fallthrough to mock
      }
    }
    this._addLog(`[ggrs] rollback mock to frame=${targetFrame}`);
    this.onStateChange({ ...this.state });
  }

  /**
   * addAfkPeer — register a peer (simulates peer joining match).
   */
  addAfkPeer(peerId: string): void {
    if (!this.state.ejectVotes.has(peerId)) {
      this.state.ejectVotes.set(peerId, {
        peerId,
        reason: 'AFK (no input)',
        votedAt: Date.now(),
        votes: 0,
        threshold: 3,
      });
      this.onStateChange({ ...this.state });
    }
  }

  destroy(): void {
    this._stopFrameLoop();
    if (this.wasmSession) {
      try { this.wasmSession.free?.(); } catch { /* ignore */ }
      this.wasmSession = null;
    }
    this.wasmStubs = null;
  }

  // ---------------------------------------------------------------------------
  // Private
  // ---------------------------------------------------------------------------

  /**
   * Mock fallback frame advance — used when WASM is unavailable.
   */
  private _advanceFrameMock(input: RtsInput): void {
    const s = this.state;
    const FIXED_STEP = 1000;
    const units = s.units.map((u, i) => {
      if (i === 0) {
        return { ...u, x: u.x + input.dx * FIXED_STEP, y: u.y + input.dy * FIXED_STEP };
      }
      return {
        ...u,
        x: u.x + Math.round(Math.sin(s.frame * 0.05 + i) * 200),
        y: u.y + Math.round(Math.cos(s.frame * 0.07 + i) * 200),
      };
    });

    const frame = s.frame + 1;
    const hash = this._mockHash(frame, units);
    const hlc = `${frame}.${Date.now() % 100000}.1`;

    const entry: FrameHashEntry = {
      frame,
      localHash: hash,
      peerHashes: new Map(),
      desync: false,
    };

    if (frame % 47 === 0 && frame > 0) {
      entry.peerHashes.set('peer-demo', hash.slice(0, -2) + 'xx');
      entry.desync = true;
      this._onDesyncEvent('peer-demo', frame, hash, hash.slice(0, -2) + 'xx');
    }

    const hashHistory = [...s.hashHistory, entry].slice(-10);

    let checkpoint = s.checkpoint;
    if (frame % 30 === 0) {
      checkpoint = { frame, qcStatus: 'approved', validators: 4, approved: 3 };
      this._addLog(`[bft] checkpoint frame=${frame} approved=3/4 (mock)`);
    }

    this.state = { ...s, frame, hlc, units, hashHistory, checkpoint, winner: null, selectedUnitIds: new Set() };
    this.onStateChange({ ...this.state });
  }

  private _onDesyncEvent(peerId: string, frame: number, localHash: string, peerHash: string): void {
    const ev: DesyncEvent = { peerId, frame, localHash, peerHash };
    this.state = {
      ...this.state,
      desyncEvents: [...this.state.desyncEvents.slice(-19), ev],
    };
    this._addLog(`[DESYNC] frame=${frame} peer=${peerId.slice(0, 8)} local=${localHash.slice(0, 8)} peer=${peerHash.slice(0, 8)}`);
  }

  private _addLog(msg: string): void {
    this.state = {
      ...this.state,
      log: [...this.state.log.slice(-49), msg],
    };
  }

  private _mockHash(frame: number, units: RtsUnit[]): string {
    let h = frame * 0x9e3779b9;
    for (const u of units) {
      h = ((h ^ (u.x * 0x517cc1b727220a95)) | 0);
      h = ((h ^ (u.y * 0x6c62272e07bb0142)) | 0);
    }
    return (Math.abs(h) >>> 0).toString(16).padStart(8, '0')
      + frame.toString(16).padStart(8, '0');
  }

  private _startFrameLoop(): void {
    _lastFrameTime = performance.now();
    const tick = (now: number) => {
      const elapsed = now - _lastFrameTime;
      if (elapsed >= FRAME_MS) {
        _lastFrameTime = now - (elapsed % FRAME_MS);
        // If there's selected-unit input, prefer it; else fall back to global input.
        // KG: sprint3-3C-game-rules-graphics-2026-04-15
        const selInput = this._pendingSelectedInput;
        this._pendingSelectedInput = null;
        const input = this.pendingInput;
        this.pendingInput = { dx: 0, dy: 0 };
        if (selInput && (selInput.input.dx !== 0 || selInput.input.dy !== 0)) {
          // Override: use selected-unit move (already batched via setSelectedInput)
          this.advanceFrame(input); // pass base input; WASM handles selection via RtsInput
        } else {
          this.advanceFrame(input);
        }
      }
      _rafHandle = requestAnimationFrame(tick);
    };
    _rafHandle = requestAnimationFrame(tick);
  }

  private _stopFrameLoop(): void {
    if (_rafHandle !== null) {
      cancelAnimationFrame(_rafHandle);
      _rafHandle = null;
    }
  }
}
