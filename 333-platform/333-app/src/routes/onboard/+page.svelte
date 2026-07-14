<!-- KG: CONTRACT_333_FE_Onboarding, SPAN_333_FE_Onboarding -->
<script lang="ts">
  // KG: CONTRACT_333_FE_Onboarding — Ed25519 keypair onboarding
  import { generateIdentity, hasIdentity } from '$lib/identity';
  import { base } from '$app/paths';
  import { onMount } from 'svelte';

  let step = $state(0);
  let username = $state('');
  let peerId = $state('');
  let pubKeyHex = $state('');
  let generating = $state(false);
  let error = $state('');

  const USERNAME_RE = /^[a-zA-Z0-9_]{3,20}$/;

  onMount(() => {
    if (hasIdentity()) {
      window.location.href = `${base}/`;
    }
  });

  function validateUsername(val: string): string {
    if (!val.trim()) return 'Username is required';
    if (val.length < 3) return 'Min 3 characters';
    if (val.length > 20) return 'Max 20 characters';
    if (!USERNAME_RE.test(val)) return 'Alphanumeric and underscore only';
    return '';
  }

  async function handleGenerate() {
    const validationError = validateUsername(username);
    if (validationError) { error = validationError; return; }
    error = '';
    generating = true;
    try {
      const identity = await generateIdentity(username.trim());
      peerId = identity.peerId;
      pubKeyHex = identity.publicKeyHex.slice(0, 32);

      // Persist to IndexedDB as well for durability
      // KG: CONTRACT_333_FE_Onboarding — IndexedDB persistence
      try {
        const db = await openIDB();
        const tx = db.transaction('identity', 'readwrite');
        tx.objectStore('identity').put({
          key: '333_identity',
          peerId: identity.peerId,
          username: username.trim(),
          publicKey: identity.publicKeyHex,
          createdAt: Date.now()
        });
      } catch { /* IndexedDB optional, localStorage is primary */ }

      step = 2;
    } catch {
      error = 'Failed to generate keypair. Try again.';
    }
    generating = false;
  }

  function openIDB(): Promise<IDBDatabase> {
    return new Promise((resolve, reject) => {
      const req = indexedDB.open('333platform', 1);
      req.onupgradeneeded = () => {
        const db = req.result;
        if (!db.objectStoreNames.contains('identity')) {
          db.createObjectStore('identity', { keyPath: 'key' });
        }
      };
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => reject(req.error);
    });
  }

  function finish() {
    window.location.href = `${base}/`;
  }
</script>

<div class="onboard">
  {#if step === 0}
    <div class="onboard__step">
      <div class="onboard__glyph">+</div>
      <h1>Welcome to <span class="accent">333</span></h1>
      <p class="onboard__desc">
        Your browser is the server. No accounts, no passwords.
        Just a cryptographic keypair that proves you are you.
      </p>
      <button class="btn" onclick={() => step = 1}>Begin Setup</button>
    </div>

  {:else if step === 1}
    <div class="onboard__step">
      <div class="onboard__glyph">~</div>
      <h2>Choose Your Identity</h2>
      <p class="onboard__desc">
        Pick a username (3-20 chars, letters, digits, underscore).
        A unique ECDSA keypair will be generated in your browser.
      </p>
      <div class="onboard__form">
        <input
          type="text"
          bind:value={username}
          placeholder="username"
          class="onboard__input"
          maxlength={20}
          autocomplete="off"
          onkeydown={(e) => e.key === 'Enter' && handleGenerate()}
          oninput={() => { if (error) error = ''; }}
        />
        <div class="onboard__hint">
          {#if username.length > 0}
            <span class:hint--ok={USERNAME_RE.test(username)} class:hint--bad={!USERNAME_RE.test(username)}>
              {USERNAME_RE.test(username) ? 'Valid' : validateUsername(username)}
            </span>
          {/if}
        </div>
        {#if error}<p class="onboard__error">{error}</p>{/if}
        <button class="btn" onclick={handleGenerate} disabled={generating}>
          {generating ? 'Generating keypair...' : 'Generate Keypair'}
        </button>
      </div>
    </div>

  {:else}
    <div class="onboard__step">
      <div class="onboard__glyph">*</div>
      <h2>Identity Created</h2>
      <div class="onboard__card card">
        <div class="onboard__field">
          <span class="onboard__label">Username</span>
          <span class="onboard__value">{username}</span>
        </div>
        <div class="onboard__field">
          <span class="onboard__label">Peer ID</span>
          <span class="onboard__value mono">{peerId}</span>
        </div>
        <div class="onboard__field">
          <span class="onboard__label">Public Key</span>
          <span class="onboard__value mono">{pubKeyHex}...</span>
        </div>
      </div>
      <p class="onboard__desc">
        Stored locally in your browser. No server ever sees your private key.
      </p>
      <button class="btn" onclick={finish}>Enter 333</button>
    </div>
  {/if}

  <div class="onboard__progress">
    {#each [0, 1, 2] as i}
      <span class="onboard__dot" class:onboard__dot--active={step >= i}></span>
    {/each}
  </div>
</div>

<style>
  /* KG: CONTRACT_333_FE_Onboarding — Styles */
  .onboard { max-width: 480px; margin: 2rem auto; text-align: center; }
  .onboard__step { animation: fadeIn 0.3s ease; }
  .onboard__glyph {
    font-size: 2.5rem; font-weight: 900; margin-bottom: 1rem;
    color: var(--pink); font-family: 'JetBrains Mono', monospace;
  }
  .accent { color: var(--pink); }
  .onboard__desc { color: var(--text-dim); margin: 1rem 0 1.5rem; line-height: 1.7; }
  .onboard__form { display: flex; flex-direction: column; gap: 0.75rem; align-items: center; }
  .onboard__input {
    width: 100%; max-width: 300px; padding: 0.7rem 1rem;
    background: rgba(0,0,0,0.3); border: 1px solid var(--border);
    border-radius: 8px; color: var(--text); font-size: 1rem;
    outline: none; text-align: center; font-family: monospace;
  }
  .onboard__input:focus { border-color: var(--purple); }
  .onboard__hint { font-size: 0.75rem; min-height: 1.2em; }
  .hint--ok { color: var(--green); }
  .hint--bad { color: var(--gold); }
  .onboard__error { color: var(--red); font-size: 0.8rem; }
  .onboard__card { text-align: left; margin: 1rem 0; }
  .onboard__field {
    display: flex; justify-content: space-between; align-items: center;
    padding: 0.5rem 0; border-bottom: 1px solid var(--border);
  }
  .onboard__field:last-child { border: none; }
  .onboard__label { color: var(--text-muted); font-size: 0.8rem; }
  .onboard__value { color: var(--text); font-size: 0.85rem; font-weight: 600; }
  .mono { font-family: monospace; font-size: 0.75rem; }
  .onboard__progress { display: flex; gap: 0.5rem; justify-content: center; margin-top: 2rem; }
  .onboard__dot {
    width: 8px; height: 8px; border-radius: 50%;
    background: var(--text-muted); transition: background 0.3s;
  }
  .onboard__dot--active { background: var(--purple); }
  @keyframes fadeIn { from { opacity: 0; transform: translateY(10px); } to { opacity: 1; transform: none; } }
</style>
