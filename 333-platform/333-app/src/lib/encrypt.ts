// KG: TASK_333_C_Encrypt, SPAN_333_C_Encrypt
// Client-side AES-GCM encryption for stored data (IndexedDB/OPFS)
// PBKDF2 key derivation from user passphrase
// Non-extractable CryptoKey for security

/**
 * Encrypted blob structure stored in IndexedDB/OPFS
 */
export interface EncryptedBlob {
  salt: Uint8Array;    // 16 bytes, unique per encryption
  iv: Uint8Array;      // 12 bytes, unique per encryption
  ciphertext: Uint8Array;
}

/**
 * Derive AES-GCM key from passphrase using PBKDF2
 * KG: CONTRACT_333_C_Encrypt — non-extractable key
 */
// Taliban Fix 6: 100K iterations (mobile-friendly, <200ms on phone)
// 600K was causing 1-3s UI blocking. For P2P game, 100K is sufficient.
// OWASP minimum is 600K but that assumes server-side, not mobile browser.
const PBKDF2_ITERATIONS = 100_000;

async function deriveKey(passphrase: string, salt: Uint8Array): Promise<CryptoKey> {
  const encoder = new TextEncoder();
  const keyMaterial = await crypto.subtle.importKey(
    'raw', encoder.encode(passphrase), 'PBKDF2', false, ['deriveKey']
  );
  return crypto.subtle.deriveKey(
    // KG: lesson-333-ts57-uint8-bufferlike-2026-04-19 — Uint8Array<ArrayBufferLike> → BufferSource
    { name: 'PBKDF2', salt: salt as BufferSource, iterations: PBKDF2_ITERATIONS, hash: 'SHA-256' },
    keyMaterial,
    { name: 'AES-GCM', length: 256 },
    false,
    ['encrypt', 'decrypt']
  );
}

/**
 * Encrypt data with a passphrase
 * KG: CONTRACT_333_C_Encrypt — AES-GCM authenticated encryption
 */
export async function encrypt(data: Uint8Array, passphrase: string): Promise<EncryptedBlob> {
  const salt = crypto.getRandomValues(new Uint8Array(16));
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const key = await deriveKey(passphrase, salt);

  const ciphertext = await crypto.subtle.encrypt(
    // KG: lesson-333-ts57-uint8-bufferlike-2026-04-19
    { name: 'AES-GCM', iv: iv as BufferSource },
    key,
    data as BufferSource
  );

  return { salt, iv, ciphertext: new Uint8Array(ciphertext) };
}

/**
 * Decrypt data with a passphrase
 */
export async function decrypt(blob: EncryptedBlob, passphrase: string): Promise<Uint8Array> {
  const key = await deriveKey(passphrase, blob.salt);

  const plaintext = await crypto.subtle.decrypt(
    // KG: lesson-333-ts57-uint8-bufferlike-2026-04-19
    { name: 'AES-GCM', iv: blob.iv as BufferSource },
    key,
    blob.ciphertext as BufferSource
  );

  return new Uint8Array(plaintext);
}

/**
 * Encrypt a string (convenience)
 */
export async function encryptString(text: string, passphrase: string): Promise<EncryptedBlob> {
  return encrypt(new TextEncoder().encode(text), passphrase);
}

/**
 * Decrypt to string (convenience)
 */
export async function decryptString(blob: EncryptedBlob, passphrase: string): Promise<string> {
  const bytes = await decrypt(blob, passphrase);
  return new TextDecoder().decode(bytes);
}
