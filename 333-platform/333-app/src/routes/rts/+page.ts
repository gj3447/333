// KG: seed-post-rts-ui-skeleton-2026-04-15
// KG: seed-rts-integration-wiring-2026-04-15 — RTS page load
// KG: SPAN_333_FE_RtsMatch

/**
 * RTS page load function.
 *
 * Reads URL params to pre-configure room/nodeId so the harness test can
 * inject them without UI interaction — mirrors the room/+page.svelte pattern.
 *
 * TODO_WASM: Once RtsSession is exported from the WASM module, call
 *   initWasm(nodeId, validators) here and pass the bridge to the page via data.
 */

import type { PageLoad } from './$types';

export const load: PageLoad = ({ url }) => {
  const roomId = url.searchParams.get('room') ?? '';
  const nodeId = parseInt(url.searchParams.get('nodeId') ?? '1', 10);
  const validators = (url.searchParams.get('validators') ?? '1,2,3,4')
    .split(',')
    .map(Number)
    .filter(n => !isNaN(n));

  return { roomId, nodeId, validators };
};
