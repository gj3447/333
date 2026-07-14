// KG: SOLID-DIP-fix-wasm-sessions-2026-04-16
// KG: CONTRACT_333_FE_WasmSessions, SPAN_333_FE_WasmBoundary
//
// WASM session typed interfaces + factory functions.
//
// DIP 적용: Controller(고수준) → Interface → factory → WASM concrete(저수준).
// Controllers는 이 파일의 interface만 알면 됨. mod.XxxWasm() 직접 참조 금지.
//
// ISP 적용: 각 interface는 해당 controller가 실제 사용하는 메서드만 포함.
// wasmMod: any 에서 typed interface로 — `any` 타입 경계를 이 파일 내부에 격리.

const WASM_URL = '/333/wasm/triple_three.js';

// 모듈-레벨 캐시 — 4개 Controller가 동일 WASM 모듈을 공유 (중복 로드 방지).
let _modCache: Promise<any> | null = null;

async function loadWasmMod(): Promise<any> {
  if (!_modCache) {
    _modCache = (async () => {
      const mod = await import(/* @vite-ignore */ WASM_URL);
      if (mod.default) await mod.default();
      return mod;
    })();
  }
  return _modCache;
}

// ---------------------------------------------------------------------------
// IRtsSession — RTS 게임 세션 계약
// KG: CONTRACT_333_FE_RtsSession, prod-wiring-wasm-rts-export-2026-04-15
// ---------------------------------------------------------------------------

export interface IRtsSession {
  tactical_summary(): string;
  advance_frame(bytes: Uint8Array): string;
  state_hash_hex(): string;
  node_id(): number;
  observe_peer_digest(peerId: number, frame: number, hash: string): string | undefined;
  free?(): void;
}

/** WASM 모듈 레벨 stub 함수 (인스턴스 메서드 아님) — KG: sprint3-3C-game-rules-graphics */
export interface IWasmStubs {
  rts_bft_checkpoint_stub?: (frame: number) => string;
  rts_peer_eject_stub?: (peerId: number) => string;
  rts_ggrs_rollback_stub?: (frame: number) => string;
}

export async function createRtsSession(
  seed: number,
  nodeId: number
): Promise<{ session: IRtsSession; stubs: IWasmStubs }> {
  const mod = await loadWasmMod();
  return {
    session: new mod.RtsSessionWasm(seed >>> 0, nodeId >>> 0) as IRtsSession,
    stubs: {
      rts_bft_checkpoint_stub: mod.rts_bft_checkpoint_stub,
      rts_peer_eject_stub: mod.rts_peer_eject_stub,
      rts_ggrs_rollback_stub: mod.rts_ggrs_rollback_stub,
    },
  };
}

// ---------------------------------------------------------------------------
// IOmSession — Orchestration Manager 세션 계약
// KG: sprint5B-om-wasm-ui-2026-04-15
// ---------------------------------------------------------------------------

export interface IOmSession {
  submit_job(kindJson: string, nowMs: number): string;
  distribute(nowMs: number): string;
  submit_result(resultJson: string, nowMs: number): string;
  stats_json(): string;
  job_status_json(jobId: string): string;
  free?(): void;
}

export async function createOmSession(peerId: number): Promise<IOmSession> {
  const mod = await loadWasmMod();
  return new mod.OmSessionWasm(peerId >>> 0) as IOmSession;
}

// ---------------------------------------------------------------------------
// ISocialSession — Social 프로파일 세션 계약
// KG: sprint5A-social-wasm-ui-2026-04-15
// ---------------------------------------------------------------------------

export interface ISocialSession {
  post_content(text: string): string;
  set_profile_field(key: string, value: string): void;
  follow_peer(peerId: number): void;
  unfollow_peer(peerId: number): void;
  is_following(peerId: number): boolean;
  ingest_remote_post(author: number, postJson: string): void;
  my_posts_json(): string;
  feed_json(): string;
  followers_list(): string;
  following_list(): string;
  free?(): void;
}

export async function createSocialSession(peerId: number): Promise<ISocialSession> {
  const mod = await loadWasmMod();
  return new mod.SocialProfileWasm(peerId >>> 0) as ISocialSession;
}

// ---------------------------------------------------------------------------
// IEditorSession — 협동 에디터 세션 계약
// KG: sprint5C-editor-wasm-ui-2026-04-15
// ---------------------------------------------------------------------------

export interface IEditorSession {
  delete_range(pos: number, len: number): void;
  insert_text(pos: number, text: string): void;
  get_text(): string;
  char_count(): number;
  update_cursor(position: number, color: string): void;
  free?(): void;
}

export async function createEditorSession(peerId: number): Promise<IEditorSession> {
  const mod = await loadWasmMod();
  return new mod.CollabEditorWasm(peerId >>> 0) as IEditorSession;
}
