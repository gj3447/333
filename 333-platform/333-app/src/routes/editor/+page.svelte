<!-- # KG: sprint5C-editor-wasm-ui-2026-04-15 -->
<!-- 333 Collaborative Editor — RGA CRDT-backed text editing with WASM.     -->
<!-- Sections: header bar / editor textarea / status sidebar / debug log    -->
<script lang="ts">
  // # KG: sprint5C-editor-wasm-ui-2026-04-15
  import { onMount, onDestroy } from 'svelte';
  import { EditorController, type EditorState, type EditorOp } from './editor_controller';
  import './+page.scss';

  // ---------------------------------------------------------------------------
  // Page data (injected by +page.ts load)
  // ---------------------------------------------------------------------------
  let { data } = $props();

  // ---------------------------------------------------------------------------
  // State
  // ---------------------------------------------------------------------------
  let editorState: EditorState | null = $state(null);
  let ctrl: EditorController | null = null;

  /** Mirror of textarea content — used for diff on each input event. */
  let prevText = $state('');

  // ---------------------------------------------------------------------------
  // Derived helpers
  // ---------------------------------------------------------------------------
  const charCount = $derived((editorState as EditorState | null)?.charCount ?? 0);
  const peerCount = $derived(((editorState as EditorState | null)?.remotePeers.length ?? 0) + 1); // +1 = self

  function formatTime(ts: number | null): string {
    if (!ts) return '—';
    const diff = Date.now() - ts;
    if (diff < 2000) return 'just now';
    if (diff < 60_000) return `${Math.floor(diff / 1000)}s ago`;
    return new Date(ts).toLocaleTimeString();
  }

  function opLabel(op: EditorOp): string {
    if (op.type === 'insert') return `+${JSON.stringify(op.text)} @${op.pos}`;
    return `-${op.len} @${op.pos}`;
  }

  // ---------------------------------------------------------------------------
  // Lifecycle
  // ---------------------------------------------------------------------------
  onMount(async () => {
    ctrl = new EditorController((s) => {
      editorState = s;
    });
    await ctrl.initEditor(data.peerId ?? 1);
  });

  onDestroy(() => {
    ctrl?.destroy();
  });

  // ---------------------------------------------------------------------------
  // Handlers
  // ---------------------------------------------------------------------------

  /** Called on every keystroke in the textarea. */
  function handleInput(e: Event) {
    const ta = e.target as HTMLTextAreaElement;
    const currentText = ta.value;
    ctrl?.applyLocalEdit(prevText, currentText);
    prevText = currentText;
  }

  /** Update cursor position in WASM on selectionchange. */
  function handleSelectionChange(e: Event) {
    const ta = e.target as HTMLTextAreaElement;
    ctrl?.updateCursor(ta.selectionStart ?? 0);
  }
</script>

<!-- ═══════════════════════════════════════════════════════════════════════════
     PAGE
════════════════════════════════════════════════════════════════════════════ -->
<div class="editor-page">

  <!-- ── Header bar ──────────────────────────────────────────────────────── -->
  <div class="editor-header">
    <h2 class="editor-title">333 Collaborative Editor</h2>

    <div class="header-meta">
      {#if editorState}
        <span class="peer-chip">Peer #{editorState.peerId}</span>
        <span class="sync-time">
          {editorState.lastSyncTime ? `sync ${formatTime(editorState.lastSyncTime)}` : 'not synced'}
        </span>
        <span class="wasm-badge {editorState.wasmActive ? 'active' : 'mock'}">
          <span class="dot"></span>
          {editorState.wasmActive ? 'WASM' : 'Mock'}
        </span>
      {/if}
    </div>
  </div>

  <!-- ── Main layout ─────────────────────────────────────────────────────── -->
  <div class="editor-layout">

    <!-- ── Editor area ─────────────────────────────────────────────────── -->
    <div class="editor-main">
      <div class="editor-wrap">
        <textarea
          class="code-editor"
          placeholder="Start typing… your edits are RGA CRDT-backed via WASM."
          spellcheck={false}
          autocomplete="off"
          autocapitalize="off"
          oninput={handleInput}
          onselect={handleSelectionChange}
          onkeyup={handleSelectionChange}
        ></textarea>
      </div>

      <div class="editor-footer">
        <span class="char-stat">{charCount} chars</span>
        <span>{peerCount} peer{peerCount !== 1 ? 's' : ''} connected</span>
      </div>
    </div>

    <!-- ── Sidebar ─────────────────────────────────────────────────────── -->
    <div class="editor-sidebar">

      <!-- Status card -->
      <div class="card">
        <p class="section-title">Status</p>
        {#if editorState}
          <div class="status-row">
            <span class="status-label">Peer ID</span>
            <span class="status-value">#{editorState.peerId}</span>
          </div>
          <div class="status-row">
            <span class="status-label">Chars</span>
            <span class="status-value">{editorState.charCount}</span>
          </div>
          <div class="status-row">
            <span class="status-label">Peers</span>
            <span class="status-value">{peerCount}</span>
          </div>
          <div class="status-row">
            <span class="status-label">Last sync</span>
            <span class="status-value">{formatTime(editorState.lastSyncTime)}</span>
          </div>
          <div class="status-row">
            <span class="status-label">Mode</span>
            <span class="status-value">{editorState.wasmActive ? 'WASM' : 'mock'}</span>
          </div>
        {:else}
          <div class="status-row">
            <span class="status-label">Loading…</span>
          </div>
        {/if}
      </div>

      <!-- Remote peers -->
      <div class="card">
        <p class="section-title">
          Remote Peers ({editorState?.remotePeers.length ?? 0})
        </p>
        <div class="peer-list">
          {#if editorState && editorState.remotePeers.length > 0}
            {#each editorState.remotePeers as peer (peer.peerId)}
              <div class="peer-row">
                <span class="peer-dot" style="background:{peer.color};"></span>
                <div class="peer-info">
                  <div class="peer-name">{peer.label}</div>
                  <div class="peer-cursor">cur:{peer.cursor}</div>
                </div>
              </div>
            {/each}
          {:else}
            <span style="font-size:.76rem; color:var(--text-muted,#888);">
              No remote peers (mock)
            </span>
          {/if}
        </div>
      </div>

    </div>
  </div>

  <!-- ── Debug log (recent ops + raw log) ─────────────────────────────────── -->
  {#if editorState && (editorState.recentOps.length > 0 || editorState.log.length > 1)}
    <details class="debug-log">
      <summary>
        Debug log — ops: {editorState.recentOps.length}, events: {editorState.log.length}
      </summary>
      <div class="log-inner">
        {#if editorState.recentOps.length > 0}
          <div style="margin-bottom:.4rem; font-weight:600; color:var(--text-muted,#888);">
            Last {editorState.recentOps.length} op(s):
          </div>
          {#each editorState.recentOps as op, i (i)}
            <div class="op-entry">
              <span class={op.type === 'insert' ? 'op-type-insert' : 'op-type-delete'}>
                {op.type.toUpperCase()}
              </span>
              {' '}{opLabel(op)}
            </div>
          {/each}
          <div style="margin-top:.4rem; border-top:1px solid rgba(255,255,255,.06); padding-top:.3rem;">
          </div>
        {/if}
        {editorState.log.slice(-20).join('\n')}
      </div>
    </details>
  {/if}

</div>
