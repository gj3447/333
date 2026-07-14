# APT Progress: 333 Platform — 234 Scope

## Anchor: 333_Platform (기존 재사용)
## Domain: web3_platform
## Status: active
## Created: 2026-03-26
## Last Updated: 2026-04-06
## Context Budget: total=300K, per_span=8K

---

## 기존 완료 Spans (Phase 1+2, 152 tests)
- SPAN_333_CRDT: HLC, Lamport, LwwMap, ORSet, RGA → SCW 완료
- SPAN_333_Consensus: BFT StateMachine, Crypto, Executor, Transport, Types, ViewChange → SCW 완료
- SPAN_333_P2P: DataChannel, MeshRoom, WireProtocol, Signaling → SCW 완료
- SPAN_333_Storage: IndexedDB, DHT(mock) → SCW 완료
- SPAN_333_AppSDK: SchemaRegistry, Events → SCW 완료
- SPAN_333_Identity: Ed25519 crypto_real → SCW 완료
- SPAN_333_Runtime: Platform core → SCW 완료

## 234 신규 Spans (SA 완료, SP 대기)

### 1. SPAN_333_Frontend [PRIORITY: NOW]
333.app 웹 프론트엔드
- A: Room 생성/참가 UI
- B: 풀 웹사이트 (온보딩, 토큰 지갑, Room 관리)
- C: metahumotonic.com 333 데모 페이지

### 2. SPAN_333_Infra [PRIORITY: NEXT]
- Cloudflare Workers 시그널링 배포
- Super Peer 모드 (항상 켜진 인프라 노드, DHT anchor, TURN relay)
- Protocol Bridge (Browser SCTP ↔ Tauri QUIC)
- Kademlia DHT 실제 구현 (현재 MemoryStore mock)

### 3. SPAN_333_OM [PRIORITY: NEXT]
OM 브라우저 분산 컴퓨팅 통합
- AI Inference Edge (ONNX Runtime Web + WebGPU)
- P2P Game Server (이미 CRDT+BFT로 기반 있음)
- Content Pipeline (FFmpeg.wasm)
- Sensor/Verification/Financial Networks

### 4. SPAN_333_KillerApps [PRIORITY: FUTURE]
- CollabEditor: RGA 기반 P2P 동시편집 (기존 editor.html 확장)
- SocialNetwork: P2P SNS (프로필, 피드, DM)
- StarCraft RTS: 실시간 전략 P2P (lockstep + CRDT hybrid)

### 5. SPAN_333_Security [PRIORITY: FUTURE]
- Anti-cheat Referee Node
- BFT 진짜 Ed25519 전체 통합 (현재 HMAC 바인딩)
- 콘텐츠 모더레이션 (커뮤니티 기반)
- 토큰 지갑 암호화

### 6. SPAN_333_Tauri [PRIORITY: FUTURE]
- Tauri v2 데스크탑 빌드
- libp2p QUIC/TCP 네이티브 P2P
- Protocol Bridge 연동

## Blocked
- metahumotonic.com 랜딩페이지 재배포 (구버전 배포 중, 최신 빌드 적용 필요)

## KG Stats
- SemanticAnchor: 333_Platform
- L1 Spans: 13 (기존 7 + 신규 6)
- INFORMED_BY links: 15+
- Source Bindings: 60+
- Research Findings: 7

## Next Steps
1. **Taliban Gate**: SA 검증 (이 문서 기준)
2. **SP Phase**: SPAN_333_Frontend 분해 시작
3. metahumotonic.com 최신 랜딩페이지 재배포

## Session Log
- [2026-04-06] SA Phase: 333_Platform 재사용, 234 scope 6개 L1 브랜치 추가
  - Frontend, OM, KillerApps, Infra, Tauri, Security
  - INFORMED_BY: SvelteKit, Tauri_v2, research-333-browser-compute-2026
  - RELATED_TO: MetaHumotonic_WebPlatform
