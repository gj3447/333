// KG: sprint5B-om-wasm-ui-2026-04-15
// OM page load — passes URL params (peerId, mock) to page

import type { PageLoad } from './$types';

export const load: PageLoad = ({ url }) => {
  const peerId = parseInt(url.searchParams.get('peerId') ?? '1', 10);
  const mock = url.searchParams.get('mock') === '1';
  return { peerId, mock };
};
