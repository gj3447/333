// KG: seed-post-rts-turn-credentials-2026-04-15
// Node native test suite for turn_credentials.mjs
// Run: node --test test_turn_credentials.mjs
//   or: TURN_SHARED_SECRET=test_secret node --test test_turn_credentials.mjs

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createHmac } from 'node:crypto';

// Inject a test secret before importing the module
process.env.TURN_SHARED_SECRET = 'test_secret_12345';

const { generateTurnCredentials } = await import('./turn_credentials.mjs');

// ---------------------------------------------------------------------------
// Test 1: credential structure — username format & password base64 length
// ---------------------------------------------------------------------------
test('credential structure: username format and password is base64', () => {
  const clientId = 'peer-abc-123';
  const ttl = 3600;
  const creds = generateTurnCredentials(clientId, ttl);

  // username = "<expiry>:<clientId>"
  const parts = creds.username.split(':');
  assert.equal(parts.length, 2, 'username must have exactly one colon separator');

  const expiry = parseInt(parts[0], 10);
  assert.ok(!isNaN(expiry), 'first part of username must be numeric unix timestamp');

  const now = Math.floor(Date.now() / 1000);
  assert.ok(expiry > now, 'expiry must be in the future');
  assert.ok(expiry <= now + ttl + 2, 'expiry must not exceed ttl (+2s clock slack)');
  assert.equal(parts[1], clientId, 'second part must be clientId');

  // HMAC-SHA1 → 20 bytes → base64 = 28 chars (with padding)
  assert.equal(
    Buffer.from(creds.password, 'base64').length,
    20,
    'HMAC-SHA1 decoded from base64 must be 20 bytes',
  );

  // uris array
  assert.ok(Array.isArray(creds.uris), 'uris must be an array');
  assert.equal(creds.uris.length, 3, 'must have 3 TURN URIs');
  assert.ok(creds.uris[0].startsWith('turn:'), 'first URI must be turn:');
  assert.ok(creds.uris[2].startsWith('turns:'), 'third URI must be turns:');

  // ttl field
  assert.equal(creds.ttl, ttl, 'ttl field must match input');
});

// ---------------------------------------------------------------------------
// Test 2: reproducibility — same clientId + frozen time → same password
// ---------------------------------------------------------------------------
test('reproducibility: same inputs yield same username/password', () => {
  // We fix the clock by computing the expected expiry ourselves
  const clientId = 'deterministic-peer';
  const ttl = 7200;

  const c1 = generateTurnCredentials(clientId, ttl);
  const c2 = generateTurnCredentials(clientId, ttl);

  // username expiry may differ by at most 1 second between calls
  const exp1 = parseInt(c1.username.split(':')[0], 10);
  const exp2 = parseInt(c2.username.split(':')[0], 10);
  assert.ok(Math.abs(exp2 - exp1) <= 1, 'expiry should match within 1 second');

  // If expiry is identical, password must be byte-for-byte equal
  if (exp1 === exp2) {
    assert.equal(c1.password, c2.password, 'identical username → identical HMAC');
  }

  // Verify HMAC independently using Node crypto
  const expectedPassword = createHmac('sha1', process.env.TURN_SHARED_SECRET)
    .update(c1.username)
    .digest('base64');
  assert.equal(c1.password, expectedPassword, 'password must equal manual HMAC-SHA1');
});

// ---------------------------------------------------------------------------
// Test 3: missing secret → throw
// ---------------------------------------------------------------------------
test('missing TURN_SHARED_SECRET throws Error', () => {
  const original = process.env.TURN_SHARED_SECRET;
  delete process.env.TURN_SHARED_SECRET;

  try {
    assert.throws(
      () => generateTurnCredentials('some-peer'),
      (err) => {
        assert.ok(err instanceof Error, 'must throw an Error instance');
        assert.match(err.message, /TURN_SHARED_SECRET/, 'error must mention TURN_SHARED_SECRET');
        return true;
      },
    );
  } finally {
    process.env.TURN_SHARED_SECRET = original; // restore for any later tests
  }
});
