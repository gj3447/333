// KG: sprint5C-editor-wasm-ui-2026-04-15
// KG: SOLID-DIP-fix-wasm-sessions-2026-04-16 — IEditorSession interface 경유
// Editor Controller — UI↔WASM bridge for CollabDocument.

import { createEditorSession } from '../../lib/wasm-sessions';
import type { IEditorSession } from '../../lib/wasm-sessions';
//
// WASM binding status:
//  1. initEditor       → new wasm.CollabEditorWasm(peerId)            [WASM]
//  2. applyLocalEdit   → diff → insert_text / delete_range            [WASM]
//  3. getText          → wasm.editor.get_text()                        [WASM]
//  4. poll remote ops  → mock (real signaling = TODO_SIGNALING)
//  5. cursors          → wasm.editor.cursors_json()                    [WASM]
//
// Mock mode: if WASM fails to load, local string state is used.
// Real signaling integration is marked TODO_SIGNALING throughout.

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface EditorOp {
  type: 'insert' | 'delete';
  pos: number;
  text?: string;   // for insert
  len?: number;    // for delete
  timestamp: number;
}

export interface RemotePeer {
  peerId: number;
  cursor: number;
  color: string;
  label: string;
}

export interface EditorState {
  peerId: number;
  text: string;
  charCount: number;
  wasmActive: boolean;
  remotePeers: RemotePeer[];
  recentOps: EditorOp[];     // last 10 ops for debug log
  lastSyncTime: number | null;
  log: string[];
}

// ---------------------------------------------------------------------------
// Diff helpers
// ---------------------------------------------------------------------------

/**
 * Compute the first divergence point between two strings.
 * Returns { pos, deletedLen, insertedText }.
 */
function diffStrings(
  before: string,
  after: string
): { pos: number; deletedLen: number; insertedText: string } {
  let start = 0;
  while (start < before.length && start < after.length && before[start] === after[start]) {
    start++;
  }

  let endBefore = before.length;
  let endAfter = after.length;
  while (
    endBefore > start &&
    endAfter > start &&
    before[endBefore - 1] === after[endAfter - 1]
  ) {
    endBefore--;
    endAfter--;
  }

  return {
    pos: start,
    deletedLen: endBefore - start,
    insertedText: after.slice(start, endAfter),
  };
}

// ---------------------------------------------------------------------------
// Mock peers (skeleton — real signaling = TODO_SIGNALING)
// ---------------------------------------------------------------------------

const MOCK_PEERS: RemotePeer[] = [
  { peerId: 42, cursor: 0, color: '#a78bfa', label: 'Peer 42' },
  { peerId: 99, cursor: 0, color: '#34d399', label: 'Peer 99' },
];

// ---------------------------------------------------------------------------
// EditorController
// ---------------------------------------------------------------------------

export class EditorController {
  private state: EditorState;
  private onStateChange: (s: EditorState) => void;

  // WASM handle — IEditorSession interface 경유 (DIP 준수).
  // KG: sprint5C-editor-wasm-ui-2026-04-15, SOLID-DIP-fix-wasm-sessions-2026-04-16
  private wasmEditor: IEditorSession | null = null;
  private _wasmAvailable = false;

  private _pollTimer: ReturnType<typeof setInterval> | null = null;
  private _lastOpCount = 0;

  constructor(onStateChange: (s: EditorState) => void) {
    this.onStateChange = onStateChange;
    this.state = {
      peerId: 1,
      text: '',
      charCount: 0,
      wasmActive: false,
      remotePeers: [],
      recentOps: [],
      lastSyncTime: null,
      log: ['[editor] controller ready'],
    };
  }

  // ---------------------------------------------------------------------------
  // Public API
  // ---------------------------------------------------------------------------

  /**
   * initEditor — load WASM module and create CollabEditorWasm(peerId).
   * Falls back to mock mode if WASM is unavailable.
   *
   * # KG: sprint5C-editor-wasm-ui-2026-04-15
   */
  async initEditor(peerId: number): Promise<void> {
    this.state = { ...this.state, peerId };

    try {
      // createEditorSession factory (DIP: interface 경유, 직접 import 금지)
      // KG: SOLID-DIP-fix-wasm-sessions-2026-04-16
      this.wasmEditor = await createEditorSession(peerId);
      this._wasmAvailable = true;
      this._addLog(`[editor] WASM init peerId=${peerId}`);
      this.state = { ...this.state, wasmActive: true };
    } catch (e) {
      this._wasmAvailable = false;
      this._addLog(`[editor] WASM unavailable, mock mode (${String(e).slice(0, 60)})`);
    }

    this._startPoll();
    this._syncState();
    this._pushState();
  }

  /**
   * applyLocalEdit — diff text_before vs text_after, call WASM insert/delete.
   *
   * # KG: sprint5C-editor-wasm-ui-2026-04-15
   */
  applyLocalEdit(textBefore: string, textAfter: string): void {
    const { pos, deletedLen, insertedText } = diffStrings(textBefore, textAfter);

    const now = Date.now();
    const ops: EditorOp[] = [];

    if (this._wasmAvailable && this.wasmEditor) {
      try {
        if (deletedLen > 0) {
          this.wasmEditor.delete_range(pos >>> 0, deletedLen >>> 0);
          ops.push({ type: 'delete', pos, len: deletedLen, timestamp: now });
        }
        if (insertedText.length > 0) {
          this.wasmEditor.insert_text(pos >>> 0, insertedText);
          ops.push({ type: 'insert', pos, text: insertedText, timestamp: now });
        }
        this._syncState();
      } catch (e) {
        this._addLog(`[edit] WASM error: ${String(e).slice(0, 60)}`);
        // Keep text from the fallthrough below
        this.state = { ...this.state, text: textAfter, charCount: textAfter.length };
      }
    } else {
      // Mock: just store the string directly
      this.state = { ...this.state, text: textAfter, charCount: textAfter.length };
      if (deletedLen > 0) ops.push({ type: 'delete', pos, len: deletedLen, timestamp: now });
      if (insertedText.length > 0) ops.push({ type: 'insert', pos, text: insertedText, timestamp: now });
    }

    if (ops.length > 0) {
      const recent = [...this.state.recentOps, ...ops].slice(-10);
      this.state = { ...this.state, recentOps: recent };
    }

    this._pushState();
  }

  /**
   * updateCursor — notify WASM of cursor position.
   *
   * # KG: sprint5C-editor-wasm-ui-2026-04-15
   */
  updateCursor(position: number): void {
    if (this._wasmAvailable && this.wasmEditor) {
      try {
        this.wasmEditor.update_cursor(position >>> 0, '#a78bfa');
      } catch { /* ignore */ }
    }
  }

  destroy(): void {
    this._stopPoll();
    if (this.wasmEditor) {
      try { this.wasmEditor.free?.(); } catch { /* ignore */ }
      this.wasmEditor = null;
    }
  }

  // ---------------------------------------------------------------------------
  // Private
  // ---------------------------------------------------------------------------

  /** Sync text + charCount from WASM. */
  private _syncState(): void {
    if (!this._wasmAvailable || !this.wasmEditor) return;
    try {
      const text: string = this.wasmEditor.get_text() as string;
      const charCount: number = this.wasmEditor.char_count() as number;
      this.state = {
        ...this.state,
        text,
        charCount,
        lastSyncTime: Date.now(),
      };
    } catch (e) {
      this._addLog(`[sync] WASM read error: ${String(e).slice(0, 60)}`);
    }
  }

  private _startPoll(): void {
    // Poll every 500 ms — merge mock remote ops (real = TODO_SIGNALING).
    // KG: sprint5C-editor-wasm-ui-2026-04-15
    this._pollTimer = setInterval(() => {
      this._pollRemoteMock();
      if (this._wasmAvailable) {
        this._syncState();
        this._pushState();
      }
    }, 500);
  }

  private _stopPoll(): void {
    if (this._pollTimer !== null) {
      clearInterval(this._pollTimer);
      this._pollTimer = null;
    }
  }

  /**
   * Mock remote peer simulation — drifts cursor positions.
   * TODO_SIGNALING: replace with real WebRTC/signaling messages.
   */
  private _pollRemoteMock(): void {
    const updated = MOCK_PEERS.map((p) => ({
      ...p,
      cursor: Math.floor(Math.random() * Math.max(1, this.state.charCount)),
    }));
    this.state = { ...this.state, remotePeers: updated };
  }

  private _addLog(msg: string): void {
    this.state = { ...this.state, log: [...this.state.log.slice(-49), msg] };
  }

  private _pushState(): void {
    this.onStateChange({ ...this.state });
  }
}
