# APT Progress: 333 Platform
# KG: 333_Platform (SemanticAnchor)

## 🔥 ACTIVE BRANCH (2026-04-14): SPAN_333_SOLID_Refactor
- **앵커**: 333_Platform (기존 재사용, 2-C 라우팅)
- **목표**: wasm.rs DIP 위반 해결 — 8 concrete → PlatformCore trait 의존성 주입
- **소스**: lesson-333-solid-audit-2026-04-13, VR_333_SOLID_Audit (S=C/O=C/L=B/I=B/D=D)
- **목표 스코어**: S=A/O=A/L=A/I=A/D=A
- **불변식**:
  1. 6/6 E2E PASS 유지 (WASM/Signaling/TabA/TabB/CRDT/BFT)
  2. Rust 223 단위테스트 ALL PASS
  3. WASM binary +10% 이내
  4. ValidatorKeyring handshake 불변
- **Context Budget**: 50K (depth=1)
- **Phase**: SA ✅ → Taliban Gate → SP
- **v21 Reflection**: 약점 — PlatformCore가 실제로 모든 concrete 타입을 대체할 수 있는지 미검증. ST 단계에서 각 concrete → trait method 매핑 계약 정밀 검증 필요.

---



## 333이란
> 브라우저가 곧 서버. 중앙 서버 없이 브라우저끼리 직접 통신·저장·연산하는 탈중앙 플랫폼.
> 3₁=탈중앙 연산(WASM), 3₂=탈중앙 통신(WebRTC P2P), 3₃=탈중앙 저장(CRDT+DHT)

## 프로젝트 통계 (2026-04-13)
- **11,290줄** (Rust 9,461 + Svelte 1,829)
- **209 tests** (205 unit + 4 integration), 0 failed
- **WASM**: 153KB (wasm-opt release)
- **롱기누스**: 50/50 파일 KG ref (100%)
- **배포**: kubeadm signaling-333 Running

## Phase 현황 — 모듈 구현 완료, Integration 진행 중

| # | Phase | 상태 | 비고 |
|:-:|-------|:----:|------|
| 1 | **Core** (CRDT, BFT, Token, Wire) | ✅ 모듈 완료 | 단위테스트 통과 |
| 2 | **Frontend** (SvelteKit 7 route) | ✅ UI 완료 | WASM 바인딩 동작 |
| 3 | **Compute** (OM Orchestrator) | ✅ 모듈 완료 | 단위테스트 통과 |
| 4 | **Apps** (RTS/Editor/Social) | ⚠️ 스텁 | 타입 정의만, 실동작 X |
| 5 | **Infra** (SuperPeer/CF Workers) | ⚠️ 스텁 | 설계만, 코드 없음 |
| 6 | **Security** (Ed25519/AntiCheat) | ⚠️ 미통합 | P2P 플로우에 연결 안 됨 |
| 7 | **Tauri** | ✅ 래퍼 완료 | 빌드 가능 |
| 8 | **🔴 Integration** | 🚧 SA 완료 | **← 현재 Phase** |

## ⚠️ 정직한 감사 (2026-04-13)
> **모듈은 전부 있지만 연결이 없다.**
> CRDT↔WebRTC↔BFT↔Token 각각 단위테스트만 통과.
> 브라우저 간 실제 합의/동기화 한 번도 안 해봄.
> KG: lesson-333-modules-not-integrated (CRITICAL)

## 🎯 Performance Budget Research (2026-04-13)
> **Status**: Complete. See `PERFORMANCE_BUDGET_P2P_GAME.md` for full analysis.
> **KG**: PERF_BUDGET_P2P_GAME
> **Key findings**:
> - Frame budget: WASM 5ms + Rendering 8ms + Network 2ms + GC 1ms = 16ms @ 60fps
> - Input latency: 31ms (input→render) with 69ms headroom for P2P
> - P2P sync: 24-35ms point-to-point; 105ms gossip @ 6-8 peers (within 200ms)
> - CRDT merge: 1.5ms per 50-delta batch; 1.2 merges/frame typical
> - BFT overhead: 0.8ms @ 8 validators; defer to Web Worker (consensus 1/5-10sec)
> - Memory total: <225MB budget (WASM 50MB, JS 100MB, WebGL 50MB)
> - Validation: measure 95th frame latency, P99 RTT, memory drift <1MB/min
> - **Bottom line**: 96% utilization; 4% jitter headroom. Optimize nearest 1-2% on any miss.

## Integration Phase 범위 (SPAN_333_Integration)

### 목표
두 브라우저에서: room 생성 → WebRTC 연결 → CRDT 실시간 동기화 → BFT 합의 → 토큰 전송

### 와이어링 필요 항목
```
[Browser A]                         [Browser B]
Platform333                         Platform333
    ↕ CRDT delta (1.5ms merge)          ↕
    ↕ BFT HotStuff msg (0.8ms @ 8v)    ↕
    ↕ Token tx                          ↕
    └──→ wire::encode ──→ WebRTC DC ──→ wire::decode ──┘
         (perf target: <2ms)    (2-10ms RTT)    (perf target: <2ms)
```
**Integration phase must respect frame budget: CRDT + BFT I/O cannot exceed 7ms/frame combined.**

### SP 분해 완료 (4 AtomicSpan, Crystallization Frontier 도달)

| # | AtomicSpan | 예상 줄수 | 대상 파일 | INFORMED_BY |
|:-:|-----------|:---------:|----------|:-----------:|
| 1 | `INT_MemFix` | 150 | webrtc.rs | 5 |
| 2 | `INT_CrdtSync` | 250 | wasm.rs, platform.rs, room-state.ts | 5 |
| 3 | `INT_ConsensusNet` | 300 | wasm.rs, platform.rs, bft/transport.rs | 6 |
| 4 | `INT_E2E` | 200 | tests/, 333-app/ | 5 |

**구현 순서** (SEQUENCED_WITH):
1. MemFix → 기반 안정화 (CONTRACT_333_INT_MemFix)
2. CrdtSync → 실시간 동기화 (CONTRACT_333_INT_CrdtSync)
3. ConsensusNet → BFT + 토큰 (CONTRACT_333_INT_ConsensusNet)
4. E2E → 통합 검증 (CONTRACT_333_INT_E2E)

### ST 완료 — Contract 6개 (개별 4 + SharedType 2)

| Contract | 타입 | target_file | NFR |
|----------|------|-------------|-----|
| `CONTRACT_333_INT_MemFix` | 개별 | src/p2p/webrtc.rs | 힙<600KB@50peer |
| `CONTRACT_333_INT_CrdtSync` | 개별 | src/sync.rs (신규) | p99<50ms |
| `CONTRACT_333_INT_ConsensusNet` | 개별 | src/bft/transport.rs | p99<1500ms |
| `CONTRACT_333_INT_E2E` | 개별 | tests/e2e/ | 전체<3s |
| `CONTRACT_SharedType_ProcessWireResult` | **공유** | src/wasm.rs | — |
| `CONTRACT_SharedType_DataChannelSet` | **공유** | room-state.ts | — |

### 롱기누스 바인딩 (소스코드 ↔ KG)
```
src/p2p/webrtc.rs    → KG: CONTRACT_333_INT_MemFix, TASK_333_INT_MemFix
src/sync.rs          → KG: CONTRACT_333_INT_CrdtSync, TASK_333_INT_CrdtSync
src/bft/transport.rs → KG: CONTRACT_333_INT_ConsensusNet, TASK_333_INT_ConsensusNet
src/wasm.rs          → KG: CONTRACT_SharedType_ProcessWireResult
src/dispatch.rs      → KG: CONTRACT_333_INT_E2E
room-state.ts        → KG: CONTRACT_SharedType_DataChannelSet
tests/e2e/           → KG: TASK_333_INT_E2E
```

## 아키텍처 결정 (프로메테우스 16-agent 리서치 해소)
- **합의**: HotStuff BFT 유지 + Chained 파이프라이닝
- **CRDT GC**: Epoch-based compaction + delta-state (Yjs 모델)
- **토큰**: Filecoin-style baseline+burn, 333M cap, 15tok/epoch
- **투표**: sqrt(stake) × conviction × reputation 하이브리드
- **서명**: Ed25519 유지 (BLS 불필요, N<50), crypto-agility 추상화
- **WebRTC**: 브라우저 네이티브 API 유지, Pure Rust WebRTC 불필요
- **wire**: 현재 4-byte header binary 최적, bincode 직렬화 추가

## 아키텍처 결정 (2026-04-13)
- **P2P 푸시 알림**: Web Push API 대신 DataChannel in-app + Notification API 선택
  - 이유: Web Push는 중앙 서버 필수 (VAPID 키 관리), "no server" 철학 위배
  - 결정: DataChannel (온라인) + IndexedDB 오프라인 큐 + 선택적 Notification API
  - 문서: P2P_NOTIFICATIONS_RESEARCH.md, P2P_NOTIFICATIONS_TECHNICAL_REFERENCE.md
  - 참고: SimpleX, Briar, Jami 등 P2P앱도 동일 패턴 사용

## Pre-Sprint 완료 (Taliban C2+C3 해결)
- **TURN**: coturn config 추가 (turn.metahumotonic.com:3478). webrtc.rs + room-state.ts 반영.
- **Ed25519**: identity.ts P-256→Ed25519 WebCrypto 전환 완료. RFC 8032 호환.
- **호환성**: Chrome137+, Firefox129+, Safari17+. iOS<17 제외. SharedArrayBuffer 선택적.
- **정책**: Ed25519 전용. 미지원 브라우저 graceful error.

## 미해결
- **외부 접근**: 라우터 80→10080, 443→10443 포워딩 변경 필요
- **coturn 실배포**: kubeadm apps namespace에 coturn Pod 배포 (인프라 작업)

## 접근 URL
- `/333/` — SvelteKit 메인 (8라우트)
- `/333/wasm/p2p-demo.html` — P2P 데모
- `/333/wasm/editor.html` — 에디터 데모
- `/333/wasm/compute.html` — 분산컴퓨팅 데모
- `/ws333/` — 시그널링 서버 (WebSocket)

## 재개
```
"333 이어서 해줘" → 이 파일 읽기
"P2P 테스트 해줘" → p2p-demo.html 브라우저 열기
"라우터 고쳐줘" → 80→10080, 443→10443
```
