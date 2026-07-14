<!-- KG: seed-phase-B-sdk-test-page-2026-04-15 — SDK diagnostics test page -->
<!-- KG: src-fe-sdk-test-page (NEW) -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/state';
  import { initWasm, type Diagnostics } from '$lib/wasm-bridge';
  import { createRoomState } from '$lib/room-state';

  let diag: Diagnostics | null = $state(null);
  let log: string[] = $state([]);

  onMount(async () => {
    const p = page.url.searchParams;
    const nodeId = parseInt(p.get('nodeId') || '1');
    const validators = (p.get('validators') || '1,2,3,4').split(',').map(Number);
    const sig = p.get('sig') || 'ws://localhost:8333';

    // KG: CONTRACT_333_FE_PeerDiscovery — window.__333_signaling override (room/+page.svelte 패턴 호환)
    (window as any).__333_signaling = sig;

    try {
      const wasm = await initWasm(nodeId, validators);
      log = [...log, `initWasm ok nodeId=${nodeId} validators=[${validators.join(',')}]`];

      const roomId = 'sdk-' + Math.random().toString(36).slice(2, 8);
      const peerId = 'peer-' + nodeId;
      createRoomState(roomId, peerId, sig);
      log = [...log, `createRoomState ok roomId=${roomId} peerId=${peerId} sig=${sig}`];

      setInterval(async () => {
        try {
          diag = await wasm.diagnostics();
        } catch (e) {
          log = [...log, 'err: ' + (e as Error).message];
        }
      }, 1000);
    } catch (e) {
      log = [...log, 'init error: ' + (e as Error).message];
    }
  });
</script>

<main style="font-family: monospace; padding: 1rem;">
  <h2>/sdk-test</h2>
  <p style="color: #888; font-size: 0.85rem;">
    URL params: <code>?nodeId=1&amp;validators=1,2,3,4&amp;sig=ws://localhost:8333</code>
  </p>

  {#if diag}
    <section>
      <h3>diagnostics</h3>
      <pre style="background: #1a1a2e; color: #a8d8ea; padding: 1rem; border-radius: 6px;">{JSON.stringify(diag, null, 2)}</pre>
    </section>
  {:else}
    <p style="color: #888;">waiting for diagnostics...</p>
  {/if}

  <section>
    <h3>log (last 10)</h3>
    <ul style="list-style: none; padding: 0;">
      {#each log.slice(-10) as l}
        <li style="padding: 2px 0; color: #ccc;">&gt; {l}</li>
      {/each}
    </ul>
  </section>
</main>
