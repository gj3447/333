// KG: TASK_333_PRE_Ed25519, CONTRACT_333_PRE_Ed25519
// KG: SOLID-ISP-fix-identity-crypto-2026-04-16
// Ed25519 primary + ECDSA P-256 fallback for older browsers
// Ed25519: Chrome 137+, Firefox 129+, Safari 17+
// P-256 fallback: all browsers
//
// ISP: 이 파일은 신원 생성/로드/확인만 담당.
// 고급 암호 연산(서명 검증, 키 바이트 변환, 계정 삭제)은 identity-crypto.ts 를 import.

const IDB_STORE = '333_keys';
const IDB_KEY = 'identity_v3';
const LS_META_KEY = '333_identity_meta';

const ED25519_ALG = { name: 'Ed25519' };
const P256_ALG = { name: 'ECDSA', namedCurve: 'P-256' };

export interface PeerIdentity {
  peerId: string;
  username: string;
  publicKeyHex: string;
  algorithm: 'Ed25519' | 'P-256';
  sign: (msg: Uint8Array) => Promise<Uint8Array>;
}

function toHex(buf: Uint8Array): string {
  return Array.from(buf).map(b => b.toString(16).padStart(2, '0')).join('');
}

function fromHex(hex: string): Uint8Array {
  const bytes = new Uint8Array(hex.length / 2);
  for (let i = 0; i < hex.length; i += 2) {
    bytes[i / 2] = parseInt(hex.slice(i, i + 2), 16);
  }
  return bytes;
}

// --- IndexedDB helpers ---
function openKeyDB(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const req = indexedDB.open('333_identity_db', 1);
    req.onupgradeneeded = () => { req.result.createObjectStore(IDB_STORE); };
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}

async function storeKeyPair(data: any): Promise<void> {
  const db = await openKeyDB();
  return new Promise((resolve, reject) => {
    const tx = db.transaction(IDB_STORE, 'readwrite');
    tx.objectStore(IDB_STORE).put(data, IDB_KEY);
    tx.oncomplete = () => { db.close(); resolve(); };
    tx.onerror = () => { db.close(); reject(tx.error); };
  });
}

async function loadKeyPair(): Promise<any | null> {
  try {
    const db = await openKeyDB();
    return new Promise((resolve, reject) => {
      const tx = db.transaction(IDB_STORE, 'readonly');
      const req = tx.objectStore(IDB_STORE).get(IDB_KEY);
      req.onsuccess = () => { db.close(); resolve(req.result ?? null); };
      req.onerror = () => { db.close(); reject(req.error); };
    });
  } catch { return null; }
}

// --- Feature detection ---
async function isEd25519Supported(): Promise<boolean> {
  try {
    await crypto.subtle.generateKey(ED25519_ALG, false, ['sign', 'verify']);
    return true;
  } catch { return false; }
}

// --- Public API ---

export async function generateIdentity(username: string): Promise<PeerIdentity> {
  const useEd25519 = await isEd25519Supported();

  if (useEd25519) {
    return generateEd25519Identity(username);
  } else {
    console.warn('Ed25519 not supported, using ECDSA P-256 fallback');
    return generateP256Identity(username);
  }
}

async function generateEd25519Identity(username: string): Promise<PeerIdentity> {
  const keyPair = await crypto.subtle.generateKey(ED25519_ALG, true, ['sign', 'verify']) as CryptoKeyPair;
  const pubRaw = new Uint8Array(await crypto.subtle.exportKey('raw', keyPair.publicKey));
  const peerId = toHex(pubRaw.slice(0, 8));
  const publicKeyHex = toHex(pubRaw);

  // Re-import private key as non-extractable
  const privJwk = await crypto.subtle.exportKey('jwk', keyPair.privateKey);
  const securePrivKey = await crypto.subtle.importKey('jwk', privJwk, ED25519_ALG, false, ['sign']);

  await storeKeyPair({ publicKey: keyPair.publicKey, privateKey: securePrivKey, peerId, username, publicKeyHex, algorithm: 'Ed25519', version: 4 });
  localStorage.setItem(LS_META_KEY, JSON.stringify({ peerId, username, publicKeyHex, algorithm: 'Ed25519' }));

  return {
    peerId, username, publicKeyHex, algorithm: 'Ed25519',
    // KG: lesson-333-ts57-uint8-bufferlike-2026-04-19
    sign: async (msg) => new Uint8Array(await crypto.subtle.sign(ED25519_ALG, securePrivKey, msg as BufferSource)),
  };
}

async function generateP256Identity(username: string): Promise<PeerIdentity> {
  const keyPair = await crypto.subtle.generateKey(P256_ALG, true, ['sign', 'verify']) as CryptoKeyPair;
  const pubRaw = new Uint8Array(await crypto.subtle.exportKey('raw', keyPair.publicKey));
  const peerId = toHex(pubRaw.slice(0, 8));
  const publicKeyHex = toHex(pubRaw);

  const privJwk = await crypto.subtle.exportKey('jwk', keyPair.privateKey);
  const securePrivKey = await crypto.subtle.importKey('jwk', privJwk, P256_ALG, false, ['sign']);

  await storeKeyPair({ publicKey: keyPair.publicKey, privateKey: securePrivKey, peerId, username, publicKeyHex, algorithm: 'P-256', version: 4 });
  localStorage.setItem(LS_META_KEY, JSON.stringify({ peerId, username, publicKeyHex, algorithm: 'P-256' }));

  return {
    peerId, username, publicKeyHex, algorithm: 'P-256',
    // KG: lesson-333-ts57-uint8-bufferlike-2026-04-19
    sign: async (msg) => new Uint8Array(await crypto.subtle.sign({ name: 'ECDSA', hash: 'SHA-256' }, securePrivKey, msg as BufferSource)),
  };
}

export async function loadIdentity(): Promise<PeerIdentity | null> {
  const stored = await loadKeyPair();
  if (!stored || stored.version !== 4) return null;

  const alg = stored.algorithm === 'Ed25519' ? ED25519_ALG : { name: 'ECDSA', hash: 'SHA-256' };
  const signAlg = stored.algorithm === 'Ed25519' ? ED25519_ALG : { name: 'ECDSA', hash: 'SHA-256' };

  return {
    peerId: stored.peerId,
    username: stored.username,
    publicKeyHex: stored.publicKeyHex,
    algorithm: stored.algorithm,
    // KG: lesson-333-ts57-uint8-bufferlike-2026-04-19
    sign: async (msg) => new Uint8Array(await crypto.subtle.sign(signAlg, stored.privateKey, msg as BufferSource)),
  };
}

export function hasIdentity(): boolean {
  return localStorage.getItem(LS_META_KEY) !== null;
}

// 고급 암호 연산은 identity-crypto.ts 에서 import:
// import { verifySignature, getPublicKeyBytes, clearIdentity } from './identity-crypto';
