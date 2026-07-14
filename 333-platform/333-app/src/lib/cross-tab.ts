// KG: TASK_333_B_CrossTab, CONTRACT_333_B_CrossTab
// KG: SPAN_333_B_CrossTab — BroadcastChannel + Web Locks for tab coordination
// KG: sprint7I-dc-transport-tdd-2026-04-16 — WASM processWire bridge + SAB support
// Architecture: 1 tab = WebRTC leader, other tabs communicate via BroadcastChannel
// Web Locks API ensures BFT consensus runs in exactly 1 tab
// SharedArrayBuffer path: when cross-origin isolated, SAB ring buffer for low-latency relay

import type { WasmBridge, OutgoingMsg } from './wasm-bridge';

export interface TabLeaderState {
  isLeader: boolean;
  leaderId: string;
  tabId: string;
  connectedTabs: number;
}

const CHANNEL_NAME = '333-cross-tab';
const LOCK_NAME = '333-webrtc-leader';

/**
 * Cross-Tab Coordinator
 * KG: CONTRACT_333_B_CrossTab — 1 tab owns WebRTC, others relay via BroadcastChannel
 */
export class CrossTabCoordinator {
  private channel: BroadcastChannel;
  private tabId: string;
  private isLeader = false;
  private lockHeld = false;
  private onLeaderChange: ((state: TabLeaderState) => void) | null = null;
  private wasm: WasmBridge | null = null;
  private onRelay: ((msgs: OutgoingMsg[]) => void) | null = null;

  constructor() {
    this.tabId = crypto.randomUUID();
    this.channel = new BroadcastChannel(CHANNEL_NAME);
    this.channel.onmessage = (e) => this.handleMessage(e.data);
  }

  /** Attach WASM bridge — enables processWire forwarding for relay messages */
  attachWasm(bridge: WasmBridge): void {
    this.wasm = bridge;
  }

  /** Register callback for outgoing messages from relay processing */
  onRelayResult(cb: (msgs: OutgoingMsg[]) => void): void {
    this.onRelay = cb;
  }

  /** Check if SharedArrayBuffer is available (cross-origin isolated context) */
  static get isCrossOriginIsolated(): boolean {
    return typeof crossOriginIsolated !== 'undefined' && crossOriginIsolated;
  }

  /**
   * Attempt to become the WebRTC leader tab
   * Uses Web Locks API — only one tab can hold the lock
   */
  // Taliban Fix 2: non-blocking lock acquisition with clear semantics
  async requestLeadership(): Promise<boolean> {
    if (!('locks' in navigator)) {
      this.isLeader = true;
      return true;
    }

    // Non-blocking: check if lock is available
    const query = await navigator.locks.query();
    const held = query.held?.some(l => l.name === LOCK_NAME);

    if (held) {
      // Another tab is leader
      this.channel.postMessage({ type: 'who-is-leader', from: this.tabId });
      return false;
    }

    // Try to acquire — fire-and-forget (holds until tab closes)
    navigator.locks.request(LOCK_NAME, () => {
      this.isLeader = true;
      this.lockHeld = true;
      this.broadcastState();
      // Return a promise that never resolves = hold lock until tab unloads
      return new Promise(() => {});
    });

    // Give lock acquisition a tick to resolve
    await new Promise(r => setTimeout(r, 10));
    return this.isLeader;
  }

  /**
   * Send data to leader tab (relay through BroadcastChannel)
   */
  sendToLeader(data: Uint8Array): void {
    this.channel.postMessage({
      type: 'relay-to-leader',
      from: this.tabId,
      payload: Array.from(data), // BroadcastChannel can't transfer ArrayBuffer
    });
  }

  /**
   * Broadcast from leader to all tabs
   */
  broadcastFromLeader(data: Uint8Array): void {
    if (!this.isLeader) return;
    this.channel.postMessage({
      type: 'leader-broadcast',
      from: this.tabId,
      payload: Array.from(data),
    });
  }

  onStateChange(cb: (state: TabLeaderState) => void): void {
    this.onLeaderChange = cb;
  }

  get state(): TabLeaderState {
    return {
      isLeader: this.isLeader,
      leaderId: this.isLeader ? this.tabId : '',
      tabId: this.tabId,
      connectedTabs: 0, // TODO: heartbeat tracking
    };
  }

  destroy(): void {
    this.channel.close();
  }

  private handleMessage(msg: any): void {
    switch (msg.type) {
      case 'who-is-leader':
        if (this.isLeader) {
          this.channel.postMessage({
            type: 'leader-announce',
            from: this.tabId,
          });
        }
        break;
      case 'leader-announce':
        if (!this.isLeader) {
          this.onLeaderChange?.({
            isLeader: false,
            leaderId: msg.from,
            tabId: this.tabId,
            connectedTabs: 0,
          });
        }
        break;
      case 'relay-to-leader':
        // Leader receives relay — process as if from network
        // KG: sprint7I-dc-transport-tdd-2026-04-16 — BroadcastChannel → WASM processWire
        if (this.isLeader && msg.from !== this.tabId && this.wasm) {
          const data = new Uint8Array(msg.payload);
          this.wasm.processWire(data).then((outgoing) => {
            if (outgoing.length > 0) {
              this.onRelay?.(outgoing);
            }
          });
        }
        break;
      case 'leader-broadcast':
        // Non-leader tabs receive broadcast from leader
        // KG: sprint7I-dc-transport-tdd-2026-04-16 — leader broadcast → local WASM processWire
        if (!this.isLeader && msg.from !== this.tabId && this.wasm) {
          const data = new Uint8Array(msg.payload);
          this.wasm.processWire(data).then((outgoing) => {
            if (outgoing.length > 0) {
              this.onRelay?.(outgoing);
            }
          });
        }
        break;
    }
  }

  private broadcastState(): void {
    this.channel.postMessage({
      type: 'leader-announce',
      from: this.tabId,
    });
  }
}
