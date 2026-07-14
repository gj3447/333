// KG: seed-post-rts-turn-credentials-2026-04-15
// TURN REST API credential generator
// RFC draft: https://datatracker.ietf.org/doc/html/draft-uberti-behave-turn-rest-00
// username = "<unix_ts_exp>:<clientId>"
// password = base64(HMAC_SHA1(username, sharedSecret))

import { createHmac } from 'node:crypto';

const URIS = [
  'turn:turn.metahumotonic.com:3478?transport=udp',
  'turn:turn.metahumotonic.com:3478?transport=tcp',
  'turns:turn.metahumotonic.com:5349?transport=tcp',
];

/**
 * Generate TURN REST API credentials.
 * @param {string} clientId  — peer/client identifier (e.g. peer UUID)
 * @param {number} [ttl=3600] — credential lifetime in seconds
 * @returns {{ username: string, password: string, ttl: number, uris: string[] }}
 */
export function generateTurnCredentials(clientId, ttl = 3600) {
  const secret = process.env.TURN_SHARED_SECRET;
  if (!secret) {
    throw new Error('TURN_SHARED_SECRET env var is required');
  }

  const expiry = Math.floor(Date.now() / 1000) + ttl;
  const username = `${expiry}:${clientId}`;
  const password = createHmac('sha1', secret)
    .update(username)
    .digest('base64');

  return { username, password, ttl, uris: URIS };
}
