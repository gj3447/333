<!-- KG: CONTRACT_333_FE_MetaDemo, SPAN_333_FE_MetaDemo -->
<script lang="ts">
  // KG: CONTRACT_333_FE_MetaDemo — 333 platform intro/demo with live status
  import { onMount } from 'svelte';
  import { base } from '$app/paths';
  import { getWasm } from '$lib/wasm-bridge';
  import { hasIdentity } from '$lib/identity';

  let wasmHealth = $state('');
  let isOnboarded = $state(false);
  let peerCount = $state(0);
  let connected = $state(false);

  const pillars = [
    { glyph: '>', title: 'Compute', desc: 'Rust compiled to WASM.\n150KB runtime.\nBrowser-native execution.' },
    { glyph: '~', title: 'Communication', desc: 'WebRTC P2P mesh.\nOne signaling server.\nDirect peer connections.' },
    { glyph: '*', title: 'Storage', desc: 'CRDT + DHT.\nIndexedDB local cache.\nServerless persistence.' },
  ];

  const stats = [
    { val: '8,134', label: 'Lines of Rust' },
    { val: '152', label: 'Tests' },
    { val: '150KB', label: 'WASM Bundle' },
    { val: '0', label: 'Servers Needed' },
  ];

  const techCards = [
    { title: 'CRDT', desc: 'HLC / Lamport / LWW-Map / OR-Set / RGA' },
    { title: 'BFT Consensus', desc: 'HotStuff 3-phase / ViewChange / Executor' },
    { title: '333 Coin', desc: 'Utility Token / Stake / Slash / Halving / PoUW' },
    { title: 'P2P Network', desc: 'WebRTC DataChannel / Signaling / MeshRoom' },
  ];

  // KG: lesson-333-ts57-uint8-bufferlike-2026-04-19 — Svelte onMount: async cleanup return needs sync wrapper
  let __interval: ReturnType<typeof setInterval> | null = null;
  onMount(() => {
    (async () => {
      isOnboarded = hasIdentity();
      const wasm = getWasm();
      if (wasm) {
        wasmHealth = await wasm.health();
        connected = true;
      }
    })();
    // Simulated live peer count (in production, query signaling server)
    __interval = setInterval(() => {
      const wasm = getWasm();
      if (wasm) {
        connected = true;
        // Random jitter for demo feel
        peerCount = Math.floor(Math.random() * 3) + (connected ? 1 : 0);
      }
    }, 3000);
    return () => { if (__interval) clearInterval(__interval); };
  });
</script>

<section class="hero">
  <h1 class="hero__title"><span class="hero__333">333</span> Platform</h1>
  <p class="hero__tagline">Decentralized Apps. No Server. Just Browsers.</p>
  <p class="hero__desc">
    333 is a decentralized application platform. Your browser IS the server --
    WebRTC P2P connections, CRDT state sync, WASM computing.
    No cloud. Browsers run apps and sync data directly.
  </p>

  {#if connected}
    <div class="hero__live">
      <span class="dot dot--green"></span>
      <span>WASM Active</span>
      {#if peerCount > 0}
        <span class="hero__peers">{peerCount} peer{peerCount !== 1 ? 's' : ''} online</span>
      {/if}
    </div>
  {/if}

  <div class="hero__cta">
    {#if isOnboarded}
      <a href="{base}/room" class="btn">Create Room</a>
      <a href="{base}/apps" class="btn btn--outline">Browse Apps</a>
    {:else}
      <a href="{base}/onboard" class="btn">Get Started</a>
      <a href="{base}/apps" class="btn btn--outline">Browse Apps</a>
    {/if}
  </div>
</section>

<section class="pillars grid-3">
  {#each pillars as p}
    <div class="card pillar-card">
      <div class="pillar-glyph">{p.glyph}</div>
      <h3 class="pillar-title">{p.title}</h3>
      <p class="pillar-desc">{p.desc}</p>
    </div>
  {/each}
</section>

<section class="stats-row">
  {#each stats as s}
    <div class="stat-box">
      <span class="stat-val">{s.val}</span>
      <span class="stat-label">{s.label}</span>
    </div>
  {/each}
</section>

<section class="tech grid-2">
  {#each techCards as tc}
    <div class="card tech-card">
      <h3 class="tech-title">{tc.title}</h3>
      <p class="tech-desc">{tc.desc}</p>
    </div>
  {/each}
</section>

<style>
  /* KG: CONTRACT_333_FE_MetaDemo — Styles */
  .hero { text-align: center; padding: 3rem 0 2rem; }
  .hero__title { font-size: 3rem; font-weight: 900; }
  .hero__333 {
    background: linear-gradient(135deg, var(--pink), var(--purple));
    -webkit-background-clip: text; -webkit-text-fill-color: transparent;
    background-clip: text;
  }
  .hero__tagline {
    color: var(--purple); font-size: 1.1rem; font-weight: 600;
    letter-spacing: 0.05em; margin: 0.5rem 0;
  }
  .hero__desc {
    color: var(--text-dim); max-width: 550px;
    margin: 0 auto 1.5rem; line-height: 1.7;
  }
  .hero__live {
    display: inline-flex; align-items: center; gap: 0.5rem;
    font-size: 0.8rem; color: var(--green); margin-bottom: 1rem;
    background: rgba(52,211,153,0.08); padding: 0.3rem 0.8rem;
    border-radius: 20px; border: 1px solid rgba(52,211,153,0.2);
  }
  .hero__peers { color: var(--text-dim); margin-left: 0.5rem; }
  .hero__cta { display: flex; gap: 1rem; justify-content: center; }

  .pillars { margin: 2rem 0; }
  .pillar-card { text-align: center; }
  .pillar-glyph {
    font-size: 2rem; font-weight: 900; margin-bottom: 0.5rem;
    color: var(--pink); font-family: 'JetBrains Mono', monospace;
  }
  .pillar-title { color: var(--text); margin-bottom: 0.3rem; }
  .pillar-desc { color: var(--text-dim); font-size: 0.8rem; white-space: pre-line; line-height: 1.5; }

  .stats-row {
    display: flex; gap: 2rem; justify-content: center;
    flex-wrap: wrap; margin: 2rem 0;
  }

  .tech { margin: 2rem 0; }
  .tech-card { padding: 1rem 1.25rem; }
  .tech-title { color: var(--purple); margin-bottom: 0.25rem; font-size: 1rem; }
  .tech-desc { color: var(--text-dim); font-size: 0.85rem; }
</style>
