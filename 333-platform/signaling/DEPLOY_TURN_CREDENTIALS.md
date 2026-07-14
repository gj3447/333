# TURN Credentials — Deployment Guide

> KG: seed-post-rts-turn-credentials-2026-04-15

## 배경

signaling-333 서버가 TURN REST API credential endpoint를 제공한다.  
coturn의 `use-auth-secret` 방식과 호환되며, client는 이 endpoint에서 ephemeral 자격증명을 받아 TURN allocate한다.

```
GET /turn-credentials?clientId=<id>
→ { username, password, ttl, uris }
```

---

## 1. Secret 확인 (coturn-auth)

coturn은 `infra/coturn-auth` Secret에 `TURN_SHARED_SECRET`을 보유한다.  
signaling-333은 **같은 secret**을 사용해 HMAC-SHA1 password를 생성해야 한다.

```bash
# secret 값 확인
kubectl -n infra get secret coturn-auth -o jsonpath='{.data.TURN_SHARED_SECRET}' | base64 -d
```

---

## 2. signaling-333 Deployment에 env 주입

### 방법 A — kubectl set env (빠른 패치)

```bash
SECRET_VALUE=$(kubectl -n infra get secret coturn-auth \
  -o jsonpath='{.data.TURN_SHARED_SECRET}' | base64 -d)

kubectl -n apps set env deployment/signaling-333 \
  TURN_SHARED_SECRET="$SECRET_VALUE"
```

### 방법 B — Deployment YAML 수정 (권장, GitOps)

signaling-333 Deployment manifest에 아래 블록을 추가한다:

```yaml
# infra/k8s/apps/signaling-333/deployment.yaml
spec:
  template:
    spec:
      containers:
        - name: signaling-333
          # ... 기존 설정 유지 ...
          env:
            - name: TURN_SHARED_SECRET
              valueFrom:
                secretKeyRef:
                  name: coturn-auth     # infra namespace의 Secret
                  key: TURN_SHARED_SECRET
                  optional: false
```

> **주의:** coturn-auth는 `infra` namespace에 있고 signaling-333은 `apps` namespace에 있다.  
> Cross-namespace Secret 참조는 기본 지원되지 않으므로, 아래 중 하나를 선택:
>
> 1. `apps` namespace에 Secret을 복사한다:
>    ```bash
>    kubectl -n infra get secret coturn-auth -o json \
>      | jq 'del(.metadata.namespace,.metadata.resourceVersion,.metadata.uid,.metadata.creationTimestamp)' \
>      | kubectl -n apps apply -f -
>    ```
> 2. External Secrets Operator로 동기화한다 (이미 설치된 경우).
> 3. Vault / Sealed Secrets 등 시크릿 관리 도구를 사용한다.

---

## 3. 배포 확인

```bash
# Pod 재시작 확인
kubectl -n apps rollout status deployment/signaling-333

# env 주입 확인
kubectl -n apps exec deploy/signaling-333 -- \
  sh -c 'echo $TURN_SHARED_SECRET | wc -c'   # 0이면 주입 실패

# endpoint 외부 테스트
curl -s "https://metahumotonic.com/ws333/turn-credentials?clientId=test-peer" | jq .
# 예상 응답:
# {
#   "username": "1745000000:test-peer",
#   "password": "AbCdEfGhIjKlMnOpQrSt==",
#   "ttl": 3600,
#   "uris": [
#     "turn:turn.metahumotonic.com:3478?transport=udp",
#     "turn:turn.metahumotonic.com:3478?transport=tcp",
#     "turns:turn.metahumotonic.com:5349?transport=tcp"
#   ]
# }
```

---

## 4. Traefik 라우팅 확인

signaling-333은 `/ws333/` PathPrefix로 라우팅된다.  
HTTP endpoint `GET /turn-credentials` 도 동일 prefix 하위에 노출된다.

클라이언트 URL:
```
https://metahumotonic.com/ws333/turn-credentials?clientId=<id>
```

Traefik StripPrefix 미들웨어가 활성화된 경우 서버 내부 경로는 `/turn-credentials`로 도달한다. 현재 구현은 경로 매칭을 `/turn-credentials`로 수행하므로 StripPrefix 적용 상태가 정상이다.

---

## 5. CORS / Rate-limit 설정

`server.mjs` 내 두 상수로 조절한다:

| 상수 | 위치 | 기본값 |
|---|---|---|
| `ALLOWED_ORIGINS` | `server.mjs` L14–L20 | metahumotonic.com, 333.metahumotonic.com, localhost:3000/5173/8080 |
| `RATE_LIMIT` | `server.mjs` L29 | 10 req/min per IP |
| `RATE_WINDOW_MS` | `server.mjs` L30 | 60,000 ms |

origin 추가가 필요하면 `ALLOWED_ORIGINS` Set에 URL 문자열을 추가한다.

---

## 6. 테스트

```bash
cd /Users/lagyeongjun/CD/SERVER/07_PROJECTS/333-platform/signaling
TURN_SHARED_SECRET=test_secret_12345 node --test test_turn_credentials.mjs
```

기대 출력:
```
▶ credential structure: username format and password is base64
  ✔ credential structure: username format and password is base64 (Xms)
▶ reproducibility: same inputs yield same username/password
  ✔ reproducibility: same inputs yield same username/password (Xms)
▶ missing TURN_SHARED_SECRET throws Error
  ✔ missing TURN_SHARED_SECRET throws Error (Xms)
ℹ tests 3
ℹ pass 3
ℹ fail 0
```
