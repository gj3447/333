<!-- KG: CONTRACT_333_FE_Shell, SPAN_333_FE_Shell -->
<script lang="ts">
  import '../app.css';
  import { page } from '$app/state';
  import { base } from '$app/paths';
  import { onMount } from 'svelte';
  import { initWasm } from '$lib/wasm-bridge';
  import { loadIdentity, type PeerIdentity } from '$lib/identity';

  let { children } = $props();

  let identity: PeerIdentity | null = $state(null);
  let wasmReady = $state(false);
  let menuOpen = $state(false);

  const navItems = [
    { href: `${base}/`, label: 'Home', icon: '333' },
    { href: `${base}/room`, label: 'Room', icon: '~' },
    { href: `${base}/wallet`, label: 'Wallet', icon: '$' },
    { href: `${base}/apps`, label: 'Apps', icon: '#' },
    { href: `${base}/onboard`, label: 'Onboard', icon: '+' },
  ];

  onMount(async () => {
    const [wasm, id] = await Promise.all([initWasm(), loadIdentity()]);
    wasmReady = wasm.ready;
    identity = id;
    if (!id && !page.url.pathname.endsWith('/onboard')) {
      window.location.href = `${base}/onboard`;
    }
  });

  function isActive(href: string): boolean {
    const path = page.url.pathname;
    if (href === `${base}/`) return path === `${base}/` || path === `${base}`;
    return path.startsWith(href);
  }

  function toggleMenu() {
    menuOpen = !menuOpen;
  }

  function closeMenu() {
    menuOpen = false;
  }
</script>

<svelte:head>
  <title>333 Platform</title>
  <meta name="description" content="Decentralized Apps. No Server. Just Browsers." />
</svelte:head>

<nav class="nav">
  <div class="nav__inner">
    <a href="{base}/" class="nav__brand" onclick={closeMenu}>
      <span class="nav__logo">333</span>
      <span class="nav__status">
        <span class="dot" class:dot--green={wasmReady} class:dot--red={!wasmReady}></span>
        {wasmReady ? 'WASM' : '...'}
      </span>
    </a>

    <button class="nav__hamburger" onclick={toggleMenu} aria-label="Toggle menu">
      <span class="nav__bar" class:nav__bar--open={menuOpen}></span>
      <span class="nav__bar" class:nav__bar--open={menuOpen}></span>
      <span class="nav__bar" class:nav__bar--open={menuOpen}></span>
    </button>

    <div class="nav__links" class:nav__links--open={menuOpen}>
      {#each navItems as item}
        <a
          href={item.href}
          class="nav__link"
          class:nav__link--active={isActive(item.href)}
          onclick={closeMenu}
        >
          <span class="nav__icon">{item.icon}</span>
          <span class="nav__label">{item.label}</span>
        </a>
      {/each}
    </div>

    <div class="nav__user">
      {#if identity}
        <span class="nav__peer">{identity.username}</span>
        <span class="nav__id">{identity.peerId.slice(0, 8)}</span>
      {:else}
        <a href="{base}/onboard" class="btn btn--outline btn--sm">Start</a>
      {/if}
    </div>
  </div>
</nav>

<main class="container">
  {@render children()}
</main>

<footer class="footer">
  <p><span class="footer__brand">333</span> Platform -- <a href="https://metahumotonic.com">MetaHumotonic</a></p>
  <p class="footer__sub">Decentralized Apps. No Server. Just Browsers.</p>
</footer>

<!-- Overlay to close mobile menu -->
{#if menuOpen}
  <button class="nav__overlay" onclick={closeMenu} aria-label="Close menu"></button>
{/if}

<style>
  /* KG: CONTRACT_333_FE_Shell — Nav */
  .nav {
    position: sticky; top: 0; z-index: 100;
    background: rgba(5, 8, 16, 0.92);
    backdrop-filter: blur(14px);
    border-bottom: 1px solid var(--border);
  }
  .nav__inner {
    max-width: 960px; margin: 0 auto;
    display: flex; align-items: center; gap: 1rem;
    padding: 0.75rem 1.5rem;
  }
  .nav__brand {
    display: flex; align-items: center; gap: 0.5rem;
    text-decoration: none; flex-shrink: 0;
  }
  .nav__logo {
    font-size: 1.4rem; font-weight: 900;
    background: linear-gradient(135deg, var(--pink), var(--purple));
    -webkit-background-clip: text; -webkit-text-fill-color: transparent;
    background-clip: text;
  }
  .nav__status {
    font-size: 0.7rem; color: var(--text-muted);
    display: flex; align-items: center; gap: 4px;
  }

  /* Links */
  .nav__links {
    display: flex; gap: 0.25rem; margin-left: auto;
  }
  .nav__link {
    display: flex; align-items: center; gap: 0.3rem;
    padding: 0.4rem 0.75rem; border-radius: 8px;
    color: var(--text-dim); text-decoration: none;
    font-size: 0.85rem; font-weight: 500; transition: all 0.2s;
  }
  .nav__link:hover { color: var(--text); background: rgba(167, 139, 250, 0.1); text-decoration: none; }
  .nav__link--active { color: var(--purple); background: rgba(167, 139, 250, 0.15); }
  .nav__icon {
    font-family: 'JetBrains Mono', monospace;
    font-weight: 700; font-size: 0.8rem; color: var(--pink);
  }

  /* User */
  .nav__user { margin-left: 1rem; display: flex; align-items: center; gap: 0.5rem; flex-shrink: 0; }
  .nav__peer { color: var(--text); font-weight: 600; font-size: 0.85rem; }
  .nav__id { color: var(--text-muted); font-size: 0.7rem; font-family: monospace; }
  .btn--sm { padding: 0.35rem 0.8rem; font-size: 0.8rem; }

  /* Hamburger */
  .nav__hamburger {
    display: none;
    flex-direction: column; gap: 4px;
    background: none; border: none; cursor: pointer;
    padding: 4px; margin-left: auto;
  }
  .nav__bar {
    display: block; width: 20px; height: 2px;
    background: var(--text-dim); border-radius: 2px;
    transition: transform 0.3s, opacity 0.3s;
  }
  .nav__bar--open:nth-child(1) { transform: rotate(45deg) translate(4px, 4px); }
  .nav__bar--open:nth-child(2) { opacity: 0; }
  .nav__bar--open:nth-child(3) { transform: rotate(-45deg) translate(4px, -4px); }

  .nav__overlay {
    display: none; position: fixed; inset: 0; z-index: 90;
    background: rgba(0,0,0,0.5); border: none; cursor: pointer;
  }

  /* Footer */
  .footer {
    text-align: center; padding: 2rem 1.5rem;
    border-top: 1px solid var(--border);
    color: var(--text-muted); font-size: 0.8rem;
    margin-top: 3rem;
  }
  .footer__brand { color: var(--pink); font-weight: 700; }
  .footer__sub { margin-top: 0.3rem; }

  /* Responsive hamburger at <768px */
  @media (max-width: 768px) {
    .nav__hamburger { display: flex; }
    .nav__links {
      display: none; position: absolute;
      top: 100%; left: 0; right: 0;
      flex-direction: column; gap: 0;
      background: rgba(5, 8, 16, 0.97);
      border-bottom: 1px solid var(--border);
      padding: 0.5rem 0;
    }
    .nav__links--open { display: flex; }
    .nav__link {
      padding: 0.75rem 1.5rem; border-radius: 0;
      font-size: 0.95rem;
    }
    .nav__overlay { display: block; }
    .nav__user { display: none; }
    .nav__label { display: inline; }
  }

  @media (max-width: 480px) {
    .nav__peer { display: none; }
  }
</style>
