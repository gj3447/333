<!-- KG: CONTRACT_333_FE_WalletUI, SPAN_333_FE_WalletUI -->
<script lang="ts">
  // KG: CONTRACT_333_FE_WalletUI — Balance, Send, Stake, Tx History
  import { onMount } from 'svelte';
  import { loadIdentity, type PeerIdentity } from '$lib/identity';
  import { getWasm } from '$lib/wasm-bridge';

  let identity: PeerIdentity | null = $state(null);
  let balance = $state(1000);
  let staked = $state(0);
  let pending = $state(0);
  let recipient = $state('');
  let amount = $state('');
  let stakeAmount = $state('');
  let txResult = $state('');
  let txResultType: 'ok' | 'err' = $state('ok');
  let txHistory: Array<{ from: string; to: string; amount: number; time: number }> = $state([]);

  let available = $derived(balance - staked - pending);

  onMount(async () => {
    identity = await loadIdentity();
    refreshBalance();
  });

  async function refreshBalance() {
    const wasm = getWasm();
    if (wasm && identity) {
      balance = await wasm.balance(1); // TODO: map peerId to nodeId
    }
  }

  function showResult(msg: string, type: 'ok' | 'err') {
    txResult = msg;
    txResultType = type;
    setTimeout(() => { txResult = ''; }, 4000);
  }

  function sendTokens() {
    if (!identity) return;
    const amt = parseInt(amount);
    if (!recipient.trim()) { showResult('Recipient required', 'err'); return; }
    if (isNaN(amt) || amt <= 0) { showResult('Enter a valid amount', 'err'); return; }
    if (amt > available) { showResult('Insufficient available balance', 'err'); return; }

    const wasm = getWasm();
    if (wasm) {
      wasm.submitTransfer(parseInt(recipient.trim()) || 2, amt, 0)
        .then(() => {
          showResult(`Sent ${amt} to ${recipient.slice(0,8)}...`, 'ok');
          refreshBalance();
          recipient = ''; amount = '';
        })
        .catch((e: Error) => showResult(e.message, 'err'));
    }
  }

  function stakeTokens() {
    const amt = parseInt(stakeAmount);
    if (isNaN(amt) || amt <= 0) { showResult('Enter a valid stake amount', 'err'); return; }
    if (amt > available) { showResult('Exceeds available balance', 'err'); return; }
    staked += amt;
    showResult(`Staked ${amt} tokens`, 'ok');
    stakeAmount = '';
  }

  function unstakeTokens() {
    if (staked <= 0) return;
    const amt = staked;
    staked = 0;
    showResult(`Unstaked ${amt} tokens`, 'ok');
  }

  function formatTime(ts: number): string {
    return new Date(ts).toLocaleTimeString();
  }
</script>

<h2 class="page-title"><span class="accent">$</span> Wallet</h2>

<!-- Balance display -->
<div class="grid-3 balance-row">
  <div class="card balance-card">
    <span class="bal-label">Available</span>
    <span class="bal-val bal-val--gold">{available.toLocaleString()}</span>
  </div>
  <div class="card balance-card">
    <span class="bal-label">Staked</span>
    <span class="bal-val bal-val--purple">{staked.toLocaleString()}</span>
  </div>
  <div class="card balance-card">
    <span class="bal-label">Pending</span>
    <span class="bal-val bal-val--cyan">{pending.toLocaleString()}</span>
  </div>
</div>

{#if identity}
  <div class="peer-info">
    Peer: <code>{identity.peerId}</code> | Total: <strong>{balance.toLocaleString()}</strong> 333
  </div>
{/if}

<!-- Result banner -->
{#if txResult}
  <div class="result" class:result--ok={txResultType === 'ok'} class:result--err={txResultType === 'err'}>
    {txResult}
  </div>
{/if}

<div class="grid-2" style="margin-top:1rem">
  <!-- Send form with validation -->
  <div class="card">
    <h3 class="card-heading">Send Tokens</h3>
    <div class="form">
      <input bind:value={recipient} placeholder="Recipient Peer ID..." class="input" />
      <input bind:value={amount} placeholder="Amount" type="number" min="1" class="input" />
      <button class="btn" onclick={sendTokens} disabled={!identity}>Send</button>
    </div>
  </div>

  <!-- Stake/unstake controls -->
  <div class="card">
    <h3 class="card-heading">Staking</h3>
    <div class="form">
      <input bind:value={stakeAmount} placeholder="Stake amount..." type="number" min="1" class="input" />
      <div class="btn-row">
        <button class="btn" onclick={stakeTokens}>Stake</button>
        <button class="btn btn--outline" onclick={unstakeTokens} disabled={staked <= 0}>Unstake All</button>
      </div>
    </div>
  </div>
</div>

<!-- Tx history list -->
<div class="card" style="margin-top:1rem">
  <h3 class="card-heading">Transaction History</h3>
  {#if txHistory.length === 0}
    <p class="empty-text">No transactions yet.</p>
  {:else}
    <div class="tx-list">
      {#each txHistory.toReversed() as tx}
        <div class="tx-row">
          <span class="tx-dir" class:tx-out={tx.from === identity?.peerId} class:tx-in={tx.to === identity?.peerId}>
            {tx.from === identity?.peerId ? 'SENT' : 'RECV'}
          </span>
          <span class="tx-amount">{tx.amount}</span>
          <span class="tx-peer">{tx.from === identity?.peerId ? tx.to.slice(0,8) : tx.from.slice(0,8)}</span>
          <span class="tx-time">{formatTime(tx.time)}</span>
        </div>
      {/each}
    </div>
  {/if}
</div>

<style>
  /* KG: CONTRACT_333_FE_WalletUI — Styles */
  .page-title { margin-bottom: 1rem; }
  .accent { color: var(--gold); font-family: 'JetBrains Mono', monospace; }
  .card-heading { color: var(--purple); margin-bottom: 0.75rem; }

  .balance-row { margin-bottom: 0.5rem; }
  .balance-card { text-align: center; padding: 1rem; }
  .bal-label { display: block; color: var(--text-muted); font-size: 0.7rem; text-transform: uppercase; letter-spacing: 0.05em; }
  .bal-val { display: block; font-size: 1.8rem; font-weight: 900; margin-top: 0.25rem; }
  .bal-val--gold { color: var(--gold); }
  .bal-val--purple { color: var(--purple); }
  .bal-val--cyan { color: var(--cyan); }

  .peer-info {
    text-align: center; font-size: 0.75rem; color: var(--text-muted);
    margin-bottom: 0.5rem;
  }
  .peer-info code { color: var(--text-dim); }

  .result {
    text-align: center; padding: 0.5rem 1rem; border-radius: 8px;
    font-size: 0.85rem; margin-top: 0.5rem;
    animation: fadeIn 0.2s ease;
  }
  .result--ok { background: rgba(52,211,153,0.1); color: var(--green); border: 1px solid rgba(52,211,153,0.3); }
  .result--err { background: rgba(239,68,68,0.1); color: var(--red); border: 1px solid rgba(239,68,68,0.3); }

  .form { display: flex; flex-direction: column; gap: 0.5rem; }
  .input {
    width: 100%; padding: 0.6rem 0.8rem;
    background: rgba(0,0,0,0.3); border: 1px solid var(--border);
    border-radius: 8px; color: var(--text); font-size: 0.9rem; outline: none;
  }
  .input:focus { border-color: var(--purple); }
  .btn-row { display: flex; gap: 0.5rem; }
  .empty-text { color: var(--text-muted); font-size: 0.85rem; }

  .tx-list { max-height: 250px; overflow-y: auto; }
  .tx-row {
    display: flex; align-items: center; gap: 1rem; padding: 0.5rem 0;
    border-bottom: 1px solid var(--border); font-size: 0.8rem;
  }
  .tx-row:last-child { border: none; }
  .tx-dir { font-weight: 700; font-size: 0.7rem; width: 50px; }
  .tx-out { color: var(--red); }
  .tx-in { color: var(--green); }
  .tx-amount { color: var(--gold); font-weight: 700; font-family: monospace; }
  .tx-peer { color: var(--text-muted); font-family: monospace; flex: 1; }
  .tx-time { color: var(--text-muted); font-size: 0.7rem; }

  @keyframes fadeIn { from { opacity: 0; } to { opacity: 1; } }
</style>
