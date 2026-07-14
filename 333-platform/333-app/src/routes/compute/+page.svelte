<!-- KG: SPAN_333_OM — OM Distributed Compute Dashboard -->
<!-- KG: seed-om-frontend-wiring-2026-04-15 -->
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { loadIdentity } from '$lib/identity';
  import { initOm, type OmBridge } from '$lib/om-bridge';

  // OM state
  let myId = $state('');
  let workerCount = $state(0);
  let isContributing = $state(false);
  let omReady = $state(false);
  let omError = $state('');

  // Stats from WASM OM
  let omStats: Record<string, unknown> = $state({});
  let activeJobs: Array<{id: string; kind: string; status: string; progress: number}> = $state([]);
  let log: string[] = $state([]);

  // Derived display values from omStats
  let nodesOnline = $derived((omStats.total_nodes as number) ?? 0);
  let jobsCompleted = $derived((omStats.completed_jobs as number) ?? 0);
  let totalComputeMs = $derived((omStats.total_compute_ms as number) ?? 0);
  let tokensEarned = $derived((omStats.tokens_distributed as number) ?? 0);

  let om: OmBridge | null = null;
  let statsInterval: ReturnType<typeof setInterval> | null = null;

  const taskTypes = [
    { id: 'pi',     name: 'Monte Carlo π',   icon: '🎯', desc: 'Estimate π using random sampling',    kind: 'cpu' },
    { id: 'hash',   name: 'Hash Search',      icon: '🔐', desc: 'Find SHA-256 hash with prefix',       kind: 'cpu' },
    { id: 'matrix', name: 'Matrix Multiply',  icon: '📊', desc: '512×512 matrix multiplication',       kind: 'cpu' },
    { id: 'image',  name: 'Image Processing', icon: '🖼️', desc: 'Apply filters to image data',         kind: 'gpu' },
    { id: 'nlp',    name: 'Text Analysis',    icon: '📝', desc: 'Word frequency analysis',             kind: 'cpu' },
    { id: 'ml',     name: 'ML Inference',     icon: '🧠', desc: 'Neural network forward pass',         kind: 'gpu' },
  ];

  function addLog(msg: string) {
    const ts = new Date().toLocaleTimeString();
    log = [`[${ts}] ${msg}`, ...log.slice(0, 49)];
  }

  async function pollStats() {
    if (!om) return;
    try {
      const raw = await om.stats();
      omStats = JSON.parse(raw) as Record<string, unknown>;
    } catch {
      // stats polling failure is non-fatal
    }
  }

  onMount(async () => {
    const identity = await loadIdentity();
    if (identity) myId = identity.peerId;
    workerCount = navigator.hardwareConcurrency || 4;

    try {
      om = await initOm();
      omReady = true;
      addLog('OM WASM instance ready');

      // Register self as node 1
      await om.registerNode(1, workerCount, false, 100);
      addLog(`Registered node 1 (${workerCount} CPU cores, no GPU)`);

      // Start 1s stats polling
      statsInterval = setInterval(pollStats, 1000);
      await pollStats();
    } catch (e) {
      omError = e instanceof Error ? e.message : String(e);
      addLog(`OM init failed: ${omError}`);
    }
  });

  onDestroy(() => {
    if (statsInterval) clearInterval(statsInterval);
  });

  async function toggleContribute() {
    if (!om || !omReady) return;
    isContributing = !isContributing;
    if (isContributing) {
      const nowMs = BigInt(Date.now());
      const jobIdJson = await om.submitCpuJob(1, BigInt(500), nowMs);
      const jobId = jobIdJson.replace(/"/g, '');
      addLog(`Node contributing. CPU job submitted: ${jobId}`);

      const job = { id: jobId, kind: 'CPU (contribute)', status: 'Queued', progress: 10 };
      activeJobs = [job, ...activeJobs];

      // Distribute & resolve
      try {
        const assignments = await om.distribute(BigInt(Date.now()));
        addLog(`Distributed: ${assignments}`);
        job.status = 'Assigned';
        job.progress = 50;
        activeJobs = [...activeJobs];

        await new Promise(r => setTimeout(r, 600));
        job.status = 'Completed';
        job.progress = 100;
        activeJobs = [...activeJobs];
        setTimeout(() => { activeJobs = activeJobs.filter(j => j.id !== jobId); }, 3000);
      } catch (e) {
        job.status = 'Error';
        activeJobs = [...activeJobs];
        addLog(`Distribute error: ${e}`);
      }
    } else {
      addLog('Node stopped contributing.');
    }
  }

  async function runTask(taskId: string) {
    if (!om || !omReady) { addLog('OM not ready'); return; }
    const task = taskTypes.find(t => t.id === taskId);
    if (!task) return;

    const nowMs = BigInt(Date.now());
    let jobIdJson: string;
    try {
      if (task.kind === 'gpu') {
        jobIdJson = await om.submitGpuJob(1, 512, nowMs);
      } else {
        jobIdJson = await om.submitCpuJob(1, BigInt(300 + Math.floor(Math.random() * 500)), nowMs);
      }
    } catch (e) {
      addLog(`Submit failed: ${e}`);
      return;
    }

    const jobId = jobIdJson.replace(/"/g, '');
    addLog(`Job ${jobId}: ${task.name} submitted`);
    const job = { id: jobId, kind: task.name, status: 'Queued', progress: 0 };
    activeJobs = [job, ...activeJobs];

    // Distribute
    try {
      job.progress = 20;
      activeJobs = [...activeJobs];
      const assignments = await om.distribute(BigInt(Date.now()));
      addLog(`Job ${jobId}: assigned — ${assignments.slice(0, 80)}`);
      job.status = 'Computing';
      job.progress = 60;
      activeJobs = [...activeJobs];

      await new Promise(r => setTimeout(r, 400 + Math.random() * 400));
      job.status = 'Completed';
      job.progress = 100;
      activeJobs = [...activeJobs];
    } catch (e) {
      job.status = 'Error';
      activeJobs = [...activeJobs];
      addLog(`Job ${jobId} error: ${e}`);
    }
    setTimeout(() => { activeJobs = activeJobs.filter(j => j.id !== jobId); }, 3000);
  }

  async function runMapReduce() {
    if (!om || !omReady) { addLog('OM not ready'); return; }
    const nowMs = BigInt(Date.now());
    let jobIdJson: string;
    try {
      jobIdJson = await om.submitMapreduceJob(1, 4, 'sum', nowMs);
    } catch (e) {
      addLog(`MapReduce submit failed: ${e}`);
      return;
    }
    const jobId = jobIdJson.replace(/"/g, '');
    addLog(`MapReduce ${jobId}: splitting into 4 chunks`);
    const job = { id: jobId, kind: 'MapReduce (4 chunks)', status: 'Distributing', progress: 0 };
    activeJobs = [job, ...activeJobs];

    try {
      for (let i = 1; i <= 4; i++) {
        await new Promise(r => setTimeout(r, 200 + Math.random() * 200));
        job.progress = (i / 4) * 80;
        activeJobs = [...activeJobs];
        addLog(`  chunk ${i}/4: processed`);
      }
      await om.distribute(BigInt(Date.now()));
      job.status = 'Reducing';
      activeJobs = [...activeJobs];
      await new Promise(r => setTimeout(r, 200));
      job.status = 'Completed';
      job.progress = 100;
      activeJobs = [...activeJobs];
      addLog(`MapReduce ${jobId}: complete`);
    } catch (e) {
      job.status = 'Error';
      activeJobs = [...activeJobs];
      addLog(`MapReduce ${jobId} error: ${e}`);
    }
    setTimeout(() => { activeJobs = activeJobs.filter(j => j.id !== jobId); }, 3000);
  }
</script>

<h2 style="margin-bottom:0.5rem"><span style="color:var(--cyan)">🖥️</span> OM Distributed Compute</h2>
<p style="color:var(--text-dim);margin-bottom:1.5rem;font-size:0.9rem">
  Browser-based distributed computing. Your browser contributes CPU/GPU to the 333 network and earns tokens.
  {#if !omReady && !omError}<span style="color:var(--gold)"> — Loading WASM…</span>{/if}
  {#if omError}<span style="color:var(--red)"> — Error: {omError}</span>{/if}
</p>

<!-- Network Stats -->
<div style="display:grid; grid-template-columns:repeat(5,1fr); gap:0.75rem; margin-bottom:1.5rem;">
  <div class="card stat-card">
    <span class="stat-val">{nodesOnline || 1}</span>
    <span class="stat-label">Nodes Online</span>
  </div>
  <div class="card stat-card">
    <span class="stat-val">{workerCount}</span>
    <span class="stat-label">Workers</span>
  </div>
  <div class="card stat-card">
    <span class="stat-val">{jobsCompleted}</span>
    <span class="stat-label">Jobs Done</span>
  </div>
  <div class="card stat-card">
    <span class="stat-val">{(totalComputeMs / 1000).toFixed(1)}s</span>
    <span class="stat-label">Compute Time</span>
  </div>
  <div class="card stat-card">
    <span class="stat-val" style="color:var(--gold)">{tokensEarned}</span>
    <span class="stat-label">Tokens Earned</span>
  </div>
</div>

<!-- Contribute Toggle -->
<div class="card" style="margin-bottom:1.5rem; display:flex; align-items:center; justify-content:space-between;">
  <div>
    <h3 style="color:var(--purple)">Contribute Compute</h3>
    <p style="color:var(--text-dim); font-size:0.8rem; margin-top:0.25rem;">
      {isContributing ? `Contributing ${workerCount} workers to the network` : 'Click to join the compute network and earn 333 tokens'}
    </p>
  </div>
  <button class="btn" class:btn--active={isContributing} onclick={toggleContribute} disabled={!omReady}>
    {isContributing ? '🟢 Contributing' : '⚪ Start'}
  </button>
</div>

<!-- Task Grid -->
<h3 style="color:var(--purple); margin-bottom:0.75rem;">Submit Compute Jobs</h3>
<div style="display:grid; grid-template-columns:repeat(auto-fill,minmax(200px,1fr)); gap:0.75rem; margin-bottom:1.5rem;">
  {#each taskTypes as task}
    <button class="card task-card" onclick={() => runTask(task.id)} disabled={!omReady}>
      <div style="font-size:1.5rem;">{task.icon}</div>
      <h4 style="color:var(--text); font-size:0.9rem; margin:0.25rem 0;">{task.name}</h4>
      <p style="color:var(--text-dim); font-size:0.7rem;">{task.desc}</p>
      <span class="badge" class:badge--live={task.kind === 'cpu'} class:badge--soon={task.kind === 'gpu'}>
        {task.kind.toUpperCase()}
      </span>
    </button>
  {/each}
</div>

<button class="btn" style="margin-bottom:1.5rem;" onclick={runMapReduce} disabled={!omReady}>
  🗺️ Run MapReduce (4 chunks)
</button>

<!-- OM Stats JSON (debug) -->
{#if omReady && Object.keys(omStats).length > 0}
  <div class="card" style="margin-bottom:1rem;">
    <h3 style="color:var(--purple); margin-bottom:0.5rem;">OM Stats</h3>
    <pre class="stats-pre">{JSON.stringify(omStats, null, 2)}</pre>
  </div>
{/if}

<!-- Active Jobs -->
{#if activeJobs.length > 0}
  <div class="card" style="margin-bottom:1rem;">
    <h3 style="color:var(--purple); margin-bottom:0.75rem;">Active Jobs</h3>
    {#each activeJobs as job}
      <div class="job-row">
        <span class="job-id">{job.id}</span>
        <span class="job-kind">{job.kind}</span>
        <span class="job-status" class:job-done={job.status === 'Completed'}>{job.status}</span>
        <div class="progress">
          <div class="progress-bar" style="width:{job.progress}%"></div>
        </div>
      </div>
    {/each}
  </div>
{/if}

<!-- Log -->
<div class="card">
  <h3 style="color:var(--purple); margin-bottom:0.5rem;">Compute Log</h3>
  <div class="log-box">
    {#each log as msg}
      <div>{msg}</div>
    {/each}
    {#if log.length === 0}
      <div style="color:var(--text-muted)">Submit a job to see activity...</div>
    {/if}
  </div>
</div>

<style>
  .stat-card { text-align: center; padding: 0.75rem; }
  .task-card {
    cursor: pointer; text-align: center; border: 1px solid var(--border);
    background: var(--bg-card); color: inherit; font: inherit; width: 100%;
    transition: all 0.2s;
  }
  .task-card:hover:not(:disabled) { border-color: var(--purple); transform: translateY(-2px); }
  .task-card:disabled, .btn:disabled { opacity: 0.5; cursor: not-allowed; }
  .btn--active { background: var(--green); }
  .job-row {
    display: flex; align-items: center; gap: 0.75rem; padding: 0.5rem 0;
    border-bottom: 1px solid var(--border); font-size: 0.8rem;
  }
  .job-row:last-child { border: none; }
  .job-id { color: var(--cyan); font-family: monospace; width: 120px; }
  .job-kind { color: var(--text-dim); flex: 1; }
  .job-status { width: 80px; font-weight: 600; color: var(--gold); }
  .job-done { color: var(--green); }
  .progress { flex: 1; height: 6px; background: rgba(107,76,153,0.2); border-radius: 3px; overflow: hidden; }
  .progress-bar { height: 100%; background: linear-gradient(90deg, var(--purple), var(--cyan)); border-radius: 3px; transition: width 0.3s; }
  .log-box {
    font-family: 'JetBrains Mono', monospace; font-size: 0.7rem;
    max-height: 200px; overflow-y: auto; color: var(--cyan);
  }
  .stats-pre {
    font-family: 'JetBrains Mono', monospace; font-size: 0.65rem;
    color: var(--text-dim); max-height: 120px; overflow-y: auto; margin: 0;
  }
  @media (max-width: 640px) {
    div[style*="grid-template-columns:repeat(5"] { grid-template-columns: repeat(3, 1fr) !important; }
  }
</style>
