// KG: TASK_333_INT_E2E, CONTRACT_SharedType_ProcessWireResult
// KG: CONTRACT_SharedType_WasmBridge, SPAN_333_FE_Shell
// KG: lesson-333-bft-keyring-exchange-2026-04-14 — registerPeerIdentity(nodeId,pubKey) 진입
// KG: lesson-333-wasm-borrowmut-needs-global-queue-2026-04-14 — 모든 method가 wasmExecutor.run() wrap (SOLID)
// KG: src-fe-wasm-bridge, SOLID-ISP-fix-wasm-bridge-2026-04-16
// Taliban F1: WASM process_wire + poll_sync exposed to TS

import { wasmExecutor } from './wasm-executor';

// ---------------------------------------------------------------------------
// ISP sub-interfaces — 호출자가 필요한 부분만 받을 수 있도록.
// KG: SOLID-ISP-fix-wasm-bridge-2026-04-16
// ---------------------------------------------------------------------------

/** P2P 메시지 처리 + 합의 흐름 — room/+page.svelte 주요 사용 */
export interface IWasmConsensus {
  processWire: (data: Uint8Array) => Promise<OutgoingMsg[]>;
  pollSync: () => Promise<OutgoingMsg[]>;
  roomState: () => Promise<RoomConsensusState>;
  tryPropose: () => Promise<OutgoingMsg[]>;
  bftTick: () => Promise<OutgoingMsg[]>;
  getPublicKey: () => Promise<string>;
  registerPeerKey: (nodeId: number, pubKeyHex: string) => Promise<boolean>;
}

/** CRDT 블록 스토리지 — 미래 사용 예정 API */
export interface IWasmStorage {
  placeBlock: (key: string, value: string) => Promise<string>;
  getBlock: (key: string) => Promise<string | undefined>;
  worldSize: () => Promise<number>;
  createSnapshot: () => Promise<OutgoingMsg | null>;
}

/** 토큰 경제 — balance, transfer */
export interface IWasmEconomy {
  submitTransfer: (to: number, amount: number, nonce: number) => Promise<void>;
  balance: (nodeId: number) => Promise<number>;
}

/** 진단 + 헬스체크 */
export interface IWasmDiagnostics {
  health: () => Promise<string>;
  diagnostics: () => Promise<Diagnostics>;
}

/** 전체 Bridge — 위 4개 sub-interface의 합집합. 직접 사용보다 sub-interface 권장. */
export interface WasmBridge extends IWasmConsensus, IWasmStorage, IWasmEconomy, IWasmDiagnostics {
  ready: boolean;
  // KG: SOLID L(LSP) — 모든 method가 Promise<T> 반환 (uniform contract)
}

// KG: seed-phase-B-sdk-test-page-2026-04-15 — diagnostics_json return shape
export interface Diagnostics {
  nodeId: number;
  view: number;
  isLeader: boolean;
  committedBlocks: number;
  worldSize: number;
  syncPending: number;
  publicKeyHex: string;
  registeredPeers: number;
}

// KG: CONTRACT_SharedType_ProcessWireResult
export interface OutgoingMsg {
  msg_type: number;
  channel: string;
  payload: string; // hex-encoded
}

export interface RoomConsensusState {
  nodeId: number;
  view: number;
  isLeader: boolean;
  committedBlocks: number;
  worldSize: number;
  syncPending: number;
}

let instance: WasmBridge | null = null;
let wasmPlatform: any = null;

function createBridge(mod: any, nodeId: number, validatorIds: number[]): WasmBridge {
  // Platform333 인스턴스 생성도 직렬화 — 이후 모든 호출과 같은 큐
  const platform = new mod.Platform333(nodeId, new Uint32Array(validatorIds));
  wasmPlatform = platform;
  const exec = wasmExecutor;

  // 모든 method = exec.run(() => platform.X(...)) 패턴
  // SOLID O(OCP): 새 method 추가는 아래 한 줄 추가만으로 직렬화 보장
  return {
    ready: true,

    health: () => exec.run(() => mod.health?.() ?? 'unknown', 'health'),

    placeBlock: (key, value) =>
      exec.run(() => platform.place_block(key, value), 'placeBlock'),

    getBlock: (key) =>
      exec.run(() => platform.get_block(key) ?? undefined, 'getBlock'),

    worldSize: () => exec.run(() => platform.world_size(), 'worldSize'),

    processWire: (data) => exec.run(() => {
      const json = platform.process_wire(data);
      try { return JSON.parse(json) as OutgoingMsg[]; } catch { return []; }
    }, 'processWire'),

    pollSync: () => exec.run(() => {
      const json = platform.poll_sync();
      try { return JSON.parse(json) as OutgoingMsg[]; } catch { return []; }
    }, 'pollSync'),

    createSnapshot: () => exec.run(() => {
      const json = platform.create_snapshot_for_peer();
      try { return JSON.parse(json) as OutgoingMsg; } catch { return null; }
    }, 'createSnapshot'),

    roomState: () => exec.run(() => {
      const json = platform.room_state_json();
      try { return JSON.parse(json) as RoomConsensusState; }
      catch {
        return { nodeId: 0, view: 0, isLeader: false, committedBlocks: 0, worldSize: 0, syncPending: 0 };
      }
    }, 'roomState'),

    // KG: plan-333-bft-try-propose — u64 → BigInt
    submitTransfer: (to, amount, nonce) => exec.run(() => {
      platform.submit_transfer(to, BigInt(amount), BigInt(nonce));
    }, 'submitTransfer'),

    balance: (nid) => exec.run(() => platform.balance(nid), 'balance'),

    tryPropose: () => exec.run(() => {
      const json = platform.try_propose();
      try { return JSON.parse(json) as OutgoingMsg[]; } catch { return []; }
    }, 'tryPropose'),

    // KG: fix-333-pacemaker-unwired-2026-07-15 — drives the view-change timer.
    // Must be called from the poll loop or a stalled leader is never replaced.
    bftTick: () => exec.run(() => {
      const json = platform.bft_tick();
      try { return JSON.parse(json) as OutgoingMsg[]; } catch { return []; }
    }, 'bftTick'),

    getPublicKey: () => exec.run(() => platform.get_public_key(), 'getPublicKey'),

    registerPeerKey: (nid, hex) =>
      exec.run(() => platform.register_peer_key(nid, hex), 'registerPeerKey'),

    diagnostics: () => exec.run(() => {
      const json = platform.diagnostics_json();
      try { return JSON.parse(json) as Diagnostics; }
      catch {
        return { nodeId: 0, view: 0, isLeader: false, committedBlocks: 0, worldSize: 0, syncPending: 0, publicKeyHex: '', registeredPeers: 0 };
      }
    }, 'diagnostics'),
  };
}

// KG: TASK_333_INT_E2E — initialization with node identity
export async function initWasm(
  nodeId: number = 1,
  validatorIds: number[] = [1, 2, 3, 4],
  wasmUrl = '/333/wasm/triple_three.js'
): Promise<WasmBridge> {
  if (instance && wasmPlatform?.node_id?.() === nodeId) return instance;
  instance = null; wasmPlatform = null;

  try {
    const mod = await import(/* @vite-ignore */ wasmUrl);
    if (mod.default) await mod.default();
    instance = createBridge(mod, nodeId, validatorIds);
    if (typeof window !== 'undefined') {
      (window as any).wasmBridge = instance;
    }
  } catch (e) {
    console.error('WASM init failed:', e);
    throw e;
  }
  return instance;
}

export function getWasm(): WasmBridge | null {
  return instance;
}

export function getWasmPlatform(): any {
  return wasmPlatform;
}
