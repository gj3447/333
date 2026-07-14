<!-- KG: CONTRACT_333_FE_AppLauncher, SPAN_333_FE_AppLauncher -->
<script lang="ts">
  // KG: CONTRACT_333_FE_AppLauncher — App registry, search filter, WASM launch
  import { initWasm } from '$lib/wasm-bridge';

  let activeApp: string | null = $state(null);
  let searchQuery = $state('');

  interface AppInfo {
    id: string;
    name: string;
    icon: string;
    desc: string;
    tags: string[];
    status: 'live' | 'coming';
    url?: string;
  }

  const apps: AppInfo[] = [
    {
      id: 'blockworld', name: 'Block World', icon: '#',
      desc: 'P2P block placement with CRDT LWW-Map sync. Real-time across browsers.',
      tags: ['crdt', 'game', 'p2p'], status: 'live', url: '/wasm/p2p-demo.html'
    },
    {
      id: 'editor', name: 'CollabEditor', icon: '=',
      desc: 'P2P text co-editing with RGA CRDT. No server sync needed.',
      tags: ['crdt', 'editor', 'collab'], status: 'live', url: '/wasm/editor.html'
    },
    {
      id: 'compute', name: 'Compute', icon: '>',
      desc: 'Browser distributed computing. Image processing, hash search, matrix ops.',
      tags: ['compute', 'wasm', 'distributed'], status: 'live', url: '/wasm/compute.html'
    },
    {
      id: 'social', name: 'Social Network', icon: '@',
      desc: 'P2P social: OR-Set followers, LWW-Map profiles, DHT post storage.',
      tags: ['social', 'crdt', 'p2p'], status: 'coming'
    },
    {
      id: 'starcraft', name: 'StarCraft RTS', icon: 'x',
      desc: 'P2P real-time strategy with lockstep + CRDT hybrid sync.',
      tags: ['game', 'rts', 'p2p'], status: 'coming'
    },
  ];

  let filteredApps = $derived(
    searchQuery.trim()
      ? apps.filter(a => {
          const q = searchQuery.toLowerCase();
          return a.name.toLowerCase().includes(q)
            || a.desc.toLowerCase().includes(q)
            || a.tags.some(t => t.includes(q));
        })
      : apps
  );

  async function launchApp(app: AppInfo) {
    if (app.status !== 'live' || !app.url) return;
    // Ensure WASM is initialized before launch
    await initWasm();
    activeApp = app.id;
  }

  function closeApp() {
    activeApp = null;
  }

  $effect(() => {
    if (typeof document !== 'undefined') {
      document.body.style.overflow = activeApp ? 'hidden' : '';
    }
  });
</script>

<h2 class="page-title"><span class="accent">#</span> Apps</h2>
<p class="page-desc">
  333 apps run entirely in your browser. No downloads. No servers. WASM + P2P.
</p>

<!-- Search filter -->
<div class="search-bar">
  <input
    bind:value={searchQuery}
    placeholder="Search apps..."
    class="search-input"
    type="search"
  />
  {#if searchQuery}
    <span class="search-count">{filteredApps.length} result{filteredApps.length !== 1 ? 's' : ''}</span>
  {/if}
</div>

<div class="app-grid">
  {#each filteredApps as app}
    <button
      class="card app-card"
      class:app-card--disabled={app.status === 'coming'}
      onclick={() => launchApp(app)}
      disabled={app.status === 'coming'}
    >
      <div class="app-card__top">
        <span class="app-icon">{app.icon}</span>
        <span class="badge" class:badge--live={app.status === 'live'} class:badge--soon={app.status === 'coming'}>
          {app.status === 'live' ? 'LIVE' : 'SOON'}
        </span>
      </div>
      <h3 class="app-name">{app.name}</h3>
      <p class="app-desc">{app.desc}</p>
      <div class="app-tags">
        {#each app.tags as tag}
          <span class="app-tag">{tag}</span>
        {/each}
      </div>
      {#if app.status === 'live'}
        <span class="app-launch">Launch WASM</span>
      {/if}
    </button>
  {/each}
</div>

{#if filteredApps.length === 0}
  <div class="no-results">
    <p>No apps match "{searchQuery}"</p>
  </div>
{/if}

<!-- App viewer overlay -->
{#if activeApp}
  {@const app = apps.find(a => a.id === activeApp)}
  {#if app?.url}
    <div class="overlay">
      <div class="overlay__header">
        <span class="overlay__title"><span class="accent">{app.icon}</span> {app.name}</span>
        <button class="btn btn--danger" onclick={closeApp}>Close</button>
      </div>
      <iframe src={app.url} class="overlay__frame" title={app.name}></iframe>
    </div>
  {/if}
{/if}

<style>
  /* KG: CONTRACT_333_FE_AppLauncher — Styles */
  .page-title { margin-bottom: 0.5rem; }
  .page-desc { color: var(--text-dim); margin-bottom: 1.25rem; }
  .accent { color: var(--cyan); font-family: 'JetBrains Mono', monospace; font-weight: 700; }

  .search-bar {
    display: flex; align-items: center; gap: 0.75rem; margin-bottom: 1.25rem;
  }
  .search-input {
    flex: 1; padding: 0.6rem 1rem;
    background: rgba(0,0,0,0.3); border: 1px solid var(--border);
    border-radius: 8px; color: var(--text); font-size: 0.9rem; outline: none;
  }
  .search-input:focus { border-color: var(--purple); }
  .search-input::placeholder { color: var(--text-muted); }
  .search-count { color: var(--text-muted); font-size: 0.8rem; white-space: nowrap; }

  .app-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(250px, 1fr)); gap: 1rem; }
  .app-card {
    text-align: left; cursor: pointer; border: 1px solid var(--border);
    background: var(--bg-card); color: inherit; font: inherit;
    transition: all 0.3s; width: 100%; position: relative;
  }
  .app-card:hover:not(.app-card--disabled) {
    transform: translateY(-3px); border-color: var(--purple);
    box-shadow: 0 10px 30px rgba(107,76,153,0.15);
  }
  .app-card--disabled { opacity: 0.5; cursor: not-allowed; }

  .app-card__top { display: flex; justify-content: space-between; align-items: center; margin-bottom: 0.5rem; }
  .app-icon {
    font-size: 1.6rem; font-weight: 900;
    font-family: 'JetBrains Mono', monospace; color: var(--pink);
  }
  .app-name { color: var(--text); font-size: 1rem; margin-bottom: 0.3rem; }
  .app-desc { color: var(--text-dim); font-size: 0.8rem; line-height: 1.4; margin-bottom: 0.5rem; }

  .app-tags { display: flex; gap: 0.3rem; flex-wrap: wrap; }
  .app-tag {
    font-size: 0.65rem; color: var(--text-muted);
    background: rgba(107,76,153,0.15); padding: 0.1rem 0.4rem;
    border-radius: 4px;
  }

  .app-launch {
    display: block; margin-top: 0.75rem; text-align: center;
    font-size: 0.75rem; font-weight: 600; color: var(--purple);
    text-transform: uppercase; letter-spacing: 0.05em;
  }

  .no-results { text-align: center; padding: 2rem; color: var(--text-muted); }

  .overlay {
    position: fixed; inset: 0; z-index: 200;
    background: var(--bg); display: flex; flex-direction: column;
  }
  .overlay__header {
    display: flex; align-items: center; justify-content: space-between;
    padding: 0.75rem 1.5rem; border-bottom: 1px solid var(--border);
    background: rgba(5,8,16,0.95);
  }
  .overlay__title { font-size: 1.1rem; font-weight: 700; }
  .overlay__frame { flex: 1; border: none; width: 100%; background: var(--bg); }
  .btn--danger { background: var(--red); }
  .btn--danger:hover { background: #dc2626; }
</style>
