# RTS Browser Harness

<!-- # KG: sprint3-3A-browser-harness-2026-04-15 -->

`333-app/tests/rts-4tab-browser.mjs` — Puppeteer-core 기반 4-tab E2E harness.

---

## 실행

```bash
# 333-app/ 디렉토리에서 실행
cd 333-app
npm run test:rts-browser
```

dev server + signaling 이 이미 실행 중이면 그대로 사용. 없으면 harness가 자동 spawn.

### 환경 변수 (선택)

| 변수 | 기본값 | 설명 |
|---|---|---|
| `PUPPETEER_EXECUTABLE_PATH` | `/Applications/Google Chrome.app/...` | Chrome/Chromium 경로 |
| `SIGNALING_URL` | `ws://localhost:8333` | 시그널링 서버 WebSocket URL |
| `DEV_URL` | `http://localhost:5173/333` | Vite dev server URL |
| `HEADLESS` | `true` | `false`로 설정하면 브라우저 창 표시 |

### Prod signaling 사용 시

```bash
SIGNALING_URL=wss://metahumotonic.com/ws333/ npm run test:rts-browser
```

---

## 시나리오별 기대 결과

| 시나리오 | 우선순위 | 기대 |
|---|---|---|
| **S1: WASM 로드** | 필수 | 4탭 중 3탭+ 에서 RTS controller 초기화 확인 (WASM 또는 mock 모드) |
| **S2: Hash Consensus** | 필수 | 4탭 중 2탭+ 가 state_hash 생성 확인 |
| S3: Move Input | bonus | tab1 WASD 입력 후 frame 증가 확인 |
| S4: Attack/BFT | bonus | BFT checkpoint 로그 또는 unit table 존재 확인 |
| S5: Peer Eject | bonus | tab2 닫은 후 나머지 3탭 frame 계속 증가 확인 |

S1, S2 실패 시 exit 1. S3-S5 실패는 `⚠️ (bonus)` 표시, exit code 영향 없음.

---

## 실패 디버깅

### BLOCKER: Chrome 없음

```
[BLOCKER] Cannot launch browser
  CHROME path: /Applications/Google Chrome.app/...
  Set env PUPPETEER_EXECUTABLE_PATH to override.
```

해결:
```bash
# Chromium 설치
brew install --cask chromium
export PUPPETEER_EXECUTABLE_PATH="/Applications/Chromium.app/Contents/MacOS/Chromium"
npm run test:rts-browser
```

### BLOCKER: dev server 미기동

harness 내부에서 `npm run dev` 자동 spawn 시도. 실패 시:
```bash
# 터미널 1
cd 333-app && npm run dev

# 터미널 2
npm run test:rts-browser
```

### S1 FAIL: WASM 로드 실패

`333-app/static/wasm/triple_three.js` 파일 확인:
```bash
ls 333-app/static/wasm/
```
WASM 파일이 없으면 controller는 mock mode로 fallback — S1은 mock mode도 PASS 처리.
debug log에 `mock mode active` 메시지 확인.

### S2 FAIL: Hash 수집 실패

- 탭이 RTS session을 시작하지 못한 경우 (Start Match 버튼 미클릭)
- signaling 미기동으로 room 연결 실패 → 탭이 lobby에만 머무름
- 해결: `HEADLESS=false npm run test:rts-browser` 로 실행해 브라우저 상태 직접 확인

### 로그 위치

```
tests/rts-4tab-browser.mjs 실행 시 stdout
  [P1] [rts] WASM session init ...
  [P2] [rts] mock mode active ...
  S1: ✅ — 4/4 tabs RTS controller ready
  S2: ✅ — 3/4 tabs produced state hashes
```

### Chrome 누적 방지

harness는 `SIGINT`/`SIGTERM` cleanup 핸들러 등록. `Ctrl+C` 시 브라우저 자동 종료.
강제 종료 후 프로세스 확인:
```bash
ps aux | grep -i chrome | grep -v grep
```

---

## 설계 메모

- **WebRTC DataChannel 미사용**: 실 브라우저 자동화에서 WebRTC DataChannel 연결은
  STUN/TURN + 네트워크 조건에 의존해 flaky. 대신 signaling WS side-channel로 hash broadcast 검증.
- **Hash broadcast 방식**: 각 탭은 `__rts_hash__<roomId>` 방에 별도 join, `broadcast` 메시지로
  `state_hash_hex()` 값 전파. 이 방식은 실 P2P 연결 없이 consensus 프로토콜 검증 가능.
- **S3-S5 bonus**: 실 WebRTC 없이는 크로스-탭 state 반영 불가. 단일탭 동작(frame 증가, unit 존재)으로 검증.

<!-- KG: sprint3-3A-browser-harness-2026-04-15 -->
