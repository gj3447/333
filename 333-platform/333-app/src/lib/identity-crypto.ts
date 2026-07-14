// KG: SOLID-ISP-fix-identity-crypto-2026-04-16
// KG: CONTRACT_333_PRE_Ed25519, TASK_333_PRE_Ed25519
//
// 고급 암호화 유틸리티 — identity.ts 에서 ISP 분리.
// 호출자가 서명 검증/바이트 변환/계정 삭제가 필요한 경우에만 이 파일을 import.
//
// ISP 원칙: 기본 신원 관리(generateIdentity/loadIdentity/hasIdentity)는
// identity.ts 에서 오고, 암호 연산은 이 파일에서만 온다.

import type { PeerIdentity } from './identity';

const P256_ALG = { name: 'ECDSA', namedCurve: 'P-256' };
const ED25519_ALG = { name: 'Ed25519' };
const IDB_STORE = '333_keys';
const IDB_KEY = 'identity_v3';

function fromHex(hex: string): Uint8Array {
  const bytes = new Uint8Array(hex.length / 2);
  for (let i = 0; i < hex.length; i += 2) {
    bytes[i / 2] = parseInt(hex.slice(i, i + 2), 16);
  }
  return bytes;
}

/**
 * verifySignature — 원격 피어의 서명 검증.
 * 사용: 미래 signaling 연결 시 피어 인증.
 */
export async function verifySignature(
  publicKeyHex: string,
  message: Uint8Array,
  signature: Uint8Array,
  algorithm: 'Ed25519' | 'P-256' = 'Ed25519'
): Promise<boolean> {
  try {
    const pubKeyBytes = fromHex(publicKeyHex);
    const alg = algorithm === 'Ed25519' ? ED25519_ALG : P256_ALG;
    const verifyAlg = algorithm === 'Ed25519' ? ED25519_ALG : { name: 'ECDSA', hash: 'SHA-256' };
    // KG: lesson-333-ts57-uint8-bufferlike-2026-04-19
    const pubKey = await crypto.subtle.importKey('raw', pubKeyBytes as BufferSource, alg, false, ['verify']);
    return await crypto.subtle.verify(verifyAlg, pubKey, signature as BufferSource, message as BufferSource);
  } catch { return false; }
}

/**
 * getPublicKeyBytes — PeerIdentity.publicKeyHex → Uint8Array.
 * 사용: WASM registerPeerKey 에 바이트 직접 전달 시.
 */
export function getPublicKeyBytes(identity: PeerIdentity): Uint8Array {
  return fromHex(identity.publicKeyHex);
}

/**
 * clearIdentity — 저장된 키페어 + 로컬 스토리지 완전 삭제 (로그아웃).
 * 사용: 계정 초기화 UI.
 */
export async function clearIdentity(): Promise<void> {
  localStorage.removeItem('333_identity_meta');
  localStorage.removeItem('333_identity');
  localStorage.removeItem('333_identity_v2');
  try {
    const db = await new Promise<IDBDatabase>((resolve, reject) => {
      const req = indexedDB.open('333_identity_db', 1);
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => reject(req.error);
    });
    const tx = db.transaction(IDB_STORE, 'readwrite');
    tx.objectStore(IDB_STORE).delete(IDB_KEY);
    await new Promise<void>((r) => { tx.oncomplete = () => { db.close(); r(); }; });
  } catch {}
}
