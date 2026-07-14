<!-- # KG: sprint5A-social-wasm-ui-2026-04-15 -->
<!-- Social page — 4 sections: Profile / Compose / Feed / Follow list         -->
<!-- WASM: SocialProfileWasm (OR-Set followers, LWW profile, posts, feed)     -->
<script lang="ts">
  // # KG: sprint5A-social-wasm-ui-2026-04-15
  import { onMount, onDestroy } from 'svelte';
  import { SocialController, type SocialState, type SocialPost } from './social_controller';
  import './+page.scss';

  // ---------------------------------------------------------------------------
  // Page data (injected by +page.ts load)
  // ---------------------------------------------------------------------------
  let { data } = $props();

  // ---------------------------------------------------------------------------
  // State
  // ---------------------------------------------------------------------------
  let socialState: SocialState | null = $state(null);
  let ctrl: SocialController | null = null;

  // Section 1 — Profile editing
  let editName = $state('');
  let editBio  = $state('');

  // Section 2 — Post composer
  let composeText = $state('');
  const MAX_CHARS = 280;

  // Section 4 — Follow input
  let followInput = $state('');

  // ---------------------------------------------------------------------------
  // Derived helpers
  // ---------------------------------------------------------------------------
  const charLeft = $derived(MAX_CHARS - composeText.length);

  /** All posts visible in feed: own posts + following feed, deduped, reverse-chrono. */
  const visibleFeed: SocialPost[] = $derived.by(() => {
    const s = socialState;
    if (!s) return [];
    const all: SocialPost[] = [...s.posts, ...s.feed];
    const seen = new Set<string>();
    const deduped: SocialPost[] = [];
    for (const p of all) {
      if (!seen.has(p.id)) { seen.add(p.id); deduped.push(p); }
    }
    return deduped.sort((a, b) => b.timestamp - a.timestamp);
  });

  /** Colour for peer avatar from peer_id. */
  function peerColor(pid: number): string {
    const palette = ['#a78bfa', '#34d399', '#f472b6', '#fbbf24', '#60a5fa', '#fb7185'];
    return palette[pid % palette.length];
  }

  /** Short display name for a peer. */
  function peerLabel(pid: number): string {
    return pid === socialState?.peerId ? 'You' : `Peer ${pid}`;
  }

  function relativeTime(ts: number): string {
    const diff = Date.now() - ts;
    if (diff < 60_000) return 'just now';
    if (diff < 3_600_000) return `${Math.floor(diff / 60_000)}m ago`;
    if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)}h ago`;
    return new Date(ts).toLocaleDateString();
  }

  // ---------------------------------------------------------------------------
  // Lifecycle
  // ---------------------------------------------------------------------------
  onMount(async () => {
    ctrl = new SocialController((s) => {
      socialState = s;
      // Sync profile editing fields from WASM on first load
      if (editName === '' && s.profile.name) editName = s.profile.name;
      if (editBio  === '' && s.profile.bio)  editBio  = s.profile.bio;
    });
    await ctrl.initSocial(data.peerId ?? 1);
  });

  onDestroy(() => {
    ctrl?.destroy();
  });

  // ---------------------------------------------------------------------------
  // Handlers
  // ---------------------------------------------------------------------------
  function handleProfileSave() {
    ctrl?.onProfileUpdate('name', editName.trim());
    ctrl?.onProfileUpdate('bio',  editBio.trim());
  }

  function handlePost() {
    const text = composeText.trim();
    if (!text || !ctrl) return;
    ctrl.onPost(text);
    composeText = '';
  }

  function handleFollow() {
    const pid = parseInt(followInput.trim(), 10);
    if (!isNaN(pid) && pid > 0) {
      ctrl?.onFollow(pid);
      followInput = '';
    }
  }

  function handleUnfollow(pid: number) {
    ctrl?.onUnfollow(pid);
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
      handlePost();
    }
  }
</script>

<!-- ═══════════════════════════════════════════════════════════════════════════
     PAGE
════════════════════════════════════════════════════════════════════════════ -->
<div class="social-page">

  <!-- ── Header ──────────────────────────────────────────────────────────── -->
  <div style="display:flex; align-items:center; justify-content:space-between; flex-wrap:wrap; gap:.5rem;">
    <h2 style="margin:0; font-size:1.15rem; font-weight:700;">Social</h2>
    {#if socialState}
      <span class="wasm-badge {socialState.wasmActive ? 'active' : 'mock'}">
        <span class="dot"></span>
        {socialState.wasmActive ? 'WASM' : 'Mock'}
      </span>
    {/if}
  </div>

  <!-- ═══════════════════════════════════════════════════════════════════════
       SECTION 1 — Profile
  ══════════════════════════════════════════════════════════════════════════ -->
  <div class="card profile-section">
    <p class="section-title">Profile</p>

    <div class="profile-row">
      <span class="profile-label">Name</span>
      <input
        class="social-input"
        type="text"
        placeholder="Your display name"
        bind:value={editName}
        maxlength={48}
      />
    </div>

    <div class="profile-row">
      <span class="profile-label">Bio</span>
      <input
        class="social-input"
        type="text"
        placeholder="Short bio"
        bind:value={editBio}
        maxlength={160}
      />
    </div>

    <div style="display:flex; justify-content:flex-end;">
      <button class="btn btn-ghost" onclick={handleProfileSave}>Save</button>
    </div>

    {#if socialState}
      <div class="profile-stats">
        <div class="stat">
          <span class="stat-value">{socialState.posts.length}</span>
          <span class="stat-label">Posts</span>
        </div>
        <div class="stat">
          <span class="stat-value">{socialState.followers.length}</span>
          <span class="stat-label">Followers</span>
        </div>
        <div class="stat">
          <span class="stat-value">{socialState.following.length}</span>
          <span class="stat-label">Following</span>
        </div>
        <div class="stat">
          <span class="stat-value" style="font-family:monospace; font-size:.85rem;">
            #{socialState.peerId}
          </span>
          <span class="stat-label">Peer ID</span>
        </div>
      </div>
    {/if}
  </div>

  <!-- ═══════════════════════════════════════════════════════════════════════
       SECTION 2 — Post composer
  ══════════════════════════════════════════════════════════════════════════ -->
  <div class="card compose-section">
    <p class="section-title">New Post</p>

    <textarea
      class="compose-textarea"
      placeholder="What's on your mind? (Ctrl+Enter to post)"
      bind:value={composeText}
      maxlength={MAX_CHARS}
      onkeydown={handleKeydown}
    ></textarea>

    <div class="compose-footer">
      <span class="char-count" style={charLeft < 20 ? 'color:#f87171;' : ''}>
        {charLeft}
      </span>
      <button
        class="btn btn-primary"
        onclick={handlePost}
        disabled={!composeText.trim() || composeText.length > MAX_CHARS}
      >
        Post
      </button>
    </div>
  </div>

  <!-- ═══════════════════════════════════════════════════════════════════════
       SECTION 3 — Feed
  ══════════════════════════════════════════════════════════════════════════ -->
  <div class="feed-section">
    <p class="section-title">
      Feed
      {#if socialState}
        <span style="font-weight:400; text-transform:none; letter-spacing:0;">
          ({visibleFeed.length} post{visibleFeed.length !== 1 ? 's' : ''})
        </span>
      {/if}
    </p>

    {#if !socialState || visibleFeed.length === 0}
      <div class="feed-empty">
        No posts yet — write something above or follow peers to see their posts.
      </div>
    {:else}
      {#each visibleFeed as post (post.id)}
        <div class="post-card">
          <div class="post-header">
            <div class="post-author">
              <div
                class="author-avatar"
                style="background:{peerColor(post.author)};"
              >
                {peerLabel(post.author).charAt(0).toUpperCase()}
              </div>
              <span class="author-name">{peerLabel(post.author)}</span>
            </div>
            <span class="post-time">{relativeTime(post.timestamp)}</span>
          </div>
          <div class="post-content">{post.content}</div>
          <div class="post-id">{post.id}</div>
        </div>
      {/each}
    {/if}
  </div>

  <!-- ═══════════════════════════════════════════════════════════════════════
       SECTION 4 — Follow list
  ══════════════════════════════════════════════════════════════════════════ -->
  <div class="card">
    <p class="section-title">Follow Graph</p>

    <div class="follow-section">
      <!-- Followers panel -->
      <div class="follow-panel">
        <p class="section-title" style="margin-bottom:.25rem;">
          Followers ({socialState?.followers.length ?? 0})
        </p>
        <div class="peer-list">
          {#if socialState && socialState.followers.length > 0}
            {#each socialState.followers as pid (pid)}
              <div class="peer-row">
                <span class="peer-id">#{pid} {pid === socialState.peerId ? '(you)' : ''}</span>
              </div>
            {/each}
          {:else}
            <span style="font-size:.8rem; color:var(--text-muted,#888);">No followers yet</span>
          {/if}
        </div>
      </div>

      <!-- Following panel -->
      <div class="follow-panel">
        <p class="section-title" style="margin-bottom:.25rem;">
          Following ({socialState?.following.length ?? 0})
        </p>
        <div class="peer-list">
          {#if socialState && socialState.following.length > 0}
            {#each socialState.following as pid (pid)}
              <div class="peer-row">
                <span class="peer-id">#{pid}</span>
                <button class="btn-unfollow" onclick={() => handleUnfollow(pid)}>
                  Unfollow
                </button>
              </div>
            {/each}
          {:else}
            <span style="font-size:.8rem; color:var(--text-muted,#888);">Not following anyone</span>
          {/if}
        </div>

        <!-- Add follow -->
        <div class="add-peer-row">
          <input
            class="peer-input"
            type="number"
            min="1"
            placeholder="Peer ID"
            bind:value={followInput}
            onkeydown={(e) => e.key === 'Enter' && handleFollow()}
          />
          <button
            class="btn btn-primary"
            onclick={handleFollow}
            disabled={!followInput.trim()}
          >
            Follow
          </button>
        </div>
      </div>
    </div>
  </div>

  <!-- ── Debug log (collapsed, dev-friendly) ─────────────────────────────── -->
  {#if socialState && socialState.log.length > 1}
    <details style="font-size:.72rem; color:var(--text-muted,#888);">
      <summary style="cursor:pointer;">Debug log ({socialState.log.length})</summary>
      <pre style="margin:.4rem 0 0; line-height:1.5; max-height:120px; overflow:auto;">
        {socialState.log.slice(-30).join('\n')}
      </pre>
    </details>
  {/if}

</div>
