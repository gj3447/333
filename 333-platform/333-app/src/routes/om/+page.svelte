<!-- # KG: sprint5B-om-wasm-ui-2026-04-15 -->
<!-- OM page — 4 sections: Stats / Submit Job / Jobs Table / Assignments log -->
<!-- WASM: OmSessionWasm (P2P distributed compute orchestrator)               -->
<script lang="ts">
  // # KG: sprint5B-om-wasm-ui-2026-04-15
  import { onMount, onDestroy } from 'svelte';
  import { OmController, type OmState, type OmJobKind } from './om_controller';
  import './+page.scss';

  // ---------------------------------------------------------------------------
  // Page data (injected by +page.ts load)
  // ---------------------------------------------------------------------------
  let { data } = $props();

  // ---------------------------------------------------------------------------
  // State
  // ---------------------------------------------------------------------------
  let omState: OmState | null = $state(null);
  let ctrl: OmController | null = null;

  // Job submission form
  let selectedKind: OmJobKind = $state('cpu');
  let estimatedMs = $state(500);
  let chunkCount = $state(4);
  let modelSizeMb = $state(256);
  let inputSizeKb = $state(64);

  // Result submission form
  let resultTaskId = $state(1);
  let resultHash = $state('abc123');
  let resultComputeMs = $state(100);

  // ---------------------------------------------------------------------------
  // Derived helpers
  // ---------------------------------------------------------------------------
  const kindParams = $derived.by((): Record<string, number | string> => {
    switch (selectedKind) {
      case 'cpu':       return { estimated_ms: estimatedMs };
      case 'gpu':       return { model_size_mb: modelSizeMb };
      case 'data':      return { input_size_kb: inputSizeKb };
      case 'mapreduce': return { chunk_count: chunkCount, reduce_fn: 'sum' };
      default:          return {};
    }
  });

  function statusColor(status: string): string {
    switch (status) {
      case 'Completed':    return '#34d399';
      case 'Computing':
      case 'Distributing': return '#60a5fa';
      case 'Failed':       return '#f87171';
      default:             return '#fbbf24';
    }
  }

  function fmtMs(ms: number): string {
    if (ms < 1000) return `${ms}ms`;
    return `${(ms / 1000).toFixed(1)}s`;
  }

  function relativeTime(ts: number | null): string {
    if (ts === null) return '—';
    const diff = Date.now() - ts;
    if (diff < 60_000) return 'just now';
    if (diff < 3_600_000) return `${Math.floor(diff / 60_000)}m ago`;
    return `${Math.floor(diff / 3_600_000)}h ago`;
  }

  // ---------------------------------------------------------------------------
  // Lifecycle
  // ---------------------------------------------------------------------------
  onMount(async () => {
    ctrl = new OmController((s) => { omState = s; });
    await ctrl.initOmSession(data.peerId ?? 1);
  });

  onDestroy(() => {
    ctrl?.destroy();
  });

  // ---------------------------------------------------------------------------
  // Handlers
  // ---------------------------------------------------------------------------
  function handleSubmitJob() {
    ctrl?.submitJob(selectedKind, kindParams);
  }

  function handleSubmitResult() {
    ctrl?.submitResult(resultTaskId, omState?.peerId ?? 1, resultHash.trim(), resultComputeMs);
  }
</script>

<!-- ═══════════════════════════════════════════════════════════════════════════
     PAGE
════════════════════════════════════════════════════════════════════════════ -->
<div class="om-page">

  <!-- ── Header ──────────────────────────────────────────────────────────── -->
  <div style="display:flex; align-items:center; justify-content:space-between; flex-wrap:wrap; gap:.5rem;">
    <h2 style="margin:0; font-size:1.15rem; font-weight:700;">Operations Manager</h2>
    <div style="display:flex; gap:.5rem; align-items:center; flex-wrap:wrap;">
      {#if omState}
        <span class="peer-chip">Peer #{omState.peerId}</span>
        <span class="wasm-badge {omState.wasmActive ? 'active' : 'mock'}">
          <span class="dot"></span>
          {omState.wasmActive ? 'WASM' : 'Mock'}
        </span>
      {/if}
    </div>
  </div>

  <!-- ═══════════════════════════════════════════════════════════════════════
       SECTION 1 — Network Stats
  ══════════════════════════════════════════════════════════════════════════ -->
  <div class="card stats-section">
    <p class="section-title">Network Stats</p>
    {#if omState}
      {@const s = omState.stats}
      <div class="stats-grid">
        <div class="stat-tile">
          <span class="stat-val">{s.nodes_online}</span>
          <span class="stat-lbl">Nodes Online</span>
        </div>
        <div class="stat-tile">
          <span class="stat-val">{s.nodes_available}</span>
          <span class="stat-lbl">Available</span>
        </div>
        <div class="stat-tile">
          <span class="stat-val">{s.gpu_nodes}</span>
          <span class="stat-lbl">GPU Nodes</span>
        </div>
        <div class="stat-tile">
          <span class="stat-val">{s.total_capacity.toFixed(1)}</span>
          <span class="stat-lbl">Capacity</span>
        </div>
        <div class="stat-tile">
          <span class="stat-val">{s.total_jobs}</span>
          <span class="stat-lbl">Total Jobs</span>
        </div>
        <div class="stat-tile">
          <span class="stat-val" style="color:#60a5fa;">{s.active_jobs}</span>
          <span class="stat-lbl">Active</span>
        </div>
        <div class="stat-tile">
          <span class="stat-val" style="color:#34d399;">{s.completed_jobs}</span>
          <span class="stat-lbl">Completed</span>
        </div>
        <div class="stat-tile">
          <span class="stat-val">{fmtMs(s.total_compute_ms)}</span>
          <span class="stat-lbl">Compute Time</span>
        </div>
        <div class="stat-tile">
          <span class="stat-val" style="color:#a78bfa;">{s.total_tokens_distributed}</span>
          <span class="stat-lbl">Tokens Paid</span>
        </div>
        <div class="stat-tile">
          <span class="stat-val">{s.pending_tasks}</span>
          <span class="stat-lbl">Pending Tasks</span>
        </div>
      </div>
    {:else}
      <p style="font-size:.82rem; color:var(--text-muted,#888); margin:.5rem 0 0;">Loading…</p>
    {/if}
  </div>

  <!-- ═══════════════════════════════════════════════════════════════════════
       SECTION 2 — Submit Job
  ══════════════════════════════════════════════════════════════════════════ -->
  <div class="card">
    <p class="section-title">Submit Job</p>

    <div class="form-row">
      <span class="form-label">Kind</span>
      <select class="om-select" bind:value={selectedKind}>
        <option value="cpu">CPU Intensive</option>
        <option value="gpu">GPU Compute</option>
        <option value="data">Data Pipeline</option>
        <option value="mapreduce">MapReduce</option>
      </select>
    </div>

    {#if selectedKind === 'cpu'}
      <div class="form-row">
        <span class="form-label">Est. ms</span>
        <input class="om-input" type="number" min="10" max="60000" bind:value={estimatedMs} />
      </div>
    {:else if selectedKind === 'gpu'}
      <div class="form-row">
        <span class="form-label">Model MB</span>
        <input class="om-input" type="number" min="1" bind:value={modelSizeMb} />
      </div>
    {:else if selectedKind === 'data'}
      <div class="form-row">
        <span class="form-label">Input KB</span>
        <input class="om-input" type="number" min="1" bind:value={inputSizeKb} />
      </div>
    {:else if selectedKind === 'mapreduce'}
      <div class="form-row">
        <span class="form-label">Chunks</span>
        <input class="om-input" type="number" min="1" max="64" bind:value={chunkCount} />
      </div>
    {/if}

    <div style="display:flex; justify-content:flex-end; margin-top:.6rem;">
      <button class="btn btn-primary" onclick={handleSubmitJob}>Submit Job</button>
    </div>
  </div>

  <!-- ═══════════════════════════════════════════════════════════════════════
       SECTION 3 — Jobs Table
  ══════════════════════════════════════════════════════════════════════════ -->
  <div class="card">
    <p class="section-title">
      Jobs
      {#if omState}
        <span style="font-weight:400; text-transform:none; letter-spacing:0;">
          ({omState.stats.total_jobs})
        </span>
      {/if}
    </p>

    {#if !omState || omState.jobs.length === 0}
      <div class="empty-state">No jobs yet — submit a job above.</div>
    {:else}
      <div class="jobs-table-wrap">
        <table class="jobs-table">
          <thead>
            <tr>
              <th>Job ID</th>
              <th>Kind</th>
              <th>Tasks</th>
              <th>Status</th>
              <th>Created</th>
            </tr>
          </thead>
          <tbody>
            {#each omState.jobs as job (job.job_id)}
              <tr>
                <td class="mono">{job.job_id}</td>
                <td><span class="kind-chip">{job.kind}</span></td>
                <td>{job.result_count}/{job.task_count}</td>
                <td>
                  <span class="status-dot" style="background:{statusColor(job.status)};"></span>
                  {job.status}
                </td>
                <td class="ts">{relativeTime(job.created_at)}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  </div>

  <!-- ═══════════════════════════════════════════════════════════════════════
       SECTION 4 — Submit Result (demo)
  ══════════════════════════════════════════════════════════════════════════ -->
  <div class="card">
    <p class="section-title">Submit Task Result <span style="font-weight:400;text-transform:none;letter-spacing:0;">(demo)</span></p>

    <div class="form-row">
      <span class="form-label">Task ID</span>
      <input class="om-input" type="number" min="1" bind:value={resultTaskId} />
    </div>
    <div class="form-row">
      <span class="form-label">Output Hash</span>
      <input class="om-input" type="text" placeholder="sha256..." bind:value={resultHash} />
    </div>
    <div class="form-row">
      <span class="form-label">Compute ms</span>
      <input class="om-input" type="number" min="0" bind:value={resultComputeMs} />
    </div>

    <div style="display:flex; justify-content:space-between; align-items:center; margin-top:.6rem; flex-wrap:wrap; gap:.5rem;">
      {#if omState?.lastVerifyJson}
        <span class="verify-badge {omState.lastVerifyJson.includes('"ok"') ? 'ok' : omState.lastVerifyJson.includes('"pending"') ? 'pending' : 'fail'}">
          {JSON.parse(omState.lastVerifyJson).kind ?? 'unknown'}
          &middot;
          {JSON.parse(omState.lastVerifyJson).verdict ?? '?'}
        </span>
      {:else}
        <span></span>
      {/if}
      <button class="btn btn-primary" onclick={handleSubmitResult}>Submit Result</button>
    </div>

    {#if omState?.lastResultTs}
      <p class="ts" style="margin:.35rem 0 0; text-align:right;">
        Last result: {relativeTime(omState.lastResultTs)}
      </p>
    {/if}
  </div>

  <!-- ── Debug log (collapsed, dev-friendly) ─────────────────────────────── -->
  {#if omState && omState.log.length > 1}
    <details style="font-size:.72rem; color:var(--text-muted,#888);">
      <summary style="cursor:pointer;">Debug log ({omState.log.length})</summary>
      <pre style="margin:.4rem 0 0; line-height:1.5; max-height:120px; overflow:auto;">
        {omState.log.slice(-30).join('\n')}
      </pre>
    </details>
  {/if}

</div>
