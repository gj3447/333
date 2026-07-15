#!/usr/bin/env node
// WASM smoke test — catches breakage that native `cargo test` structurally cannot.
//
// Why this exists: native builds have a working `std::time::Instant`, so a
// `Instant::now()` call compiles and passes every native test while panicking
// ("time not implemented on this platform") on wasm32-unknown-unknown. That is
// exactly what happened: `ViewChangeTracker::new` used `Instant::now()`, so
// `Platform333::new` — the constructor the browser calls — aborted on load, and
// 440/440 native tests stayed green throughout.
//
// Run:
//   cd 333-platform
//   wasm-pack build --target nodejs --dev --out-dir wasm-smoke/pkg-node
//   node wasm-smoke/wasm_smoke.cjs
//
// # KG: lesson-333-viewchange-instant-panics-on-wasm-2026-07-15
// # KG: fix-333-pacemaker-unwired-2026-07-15

const path = require('path');
const w = require(path.join(__dirname, 'pkg-node', 'triple_three.js'));

let failed = 0;
const ok = (name) => console.log('  ok   - ' + name);
const bad = (name, err) => { failed++; console.log('  FAIL - ' + name + ': ' + err); };

function check(name, fn) {
  try { fn(); ok(name); } catch (e) { bad(name, (e && e.message) || String(e)); }
}

console.log('# wasm smoke');

// 1. The constructor must not panic. This is the regression that killed the
//    entire browser platform.
let p;
check('Platform333 constructor does not panic on wasm', () => {
  p = new w.Platform333(1, new Uint32Array([1, 2, 3, 4]));
  if (p.node_id() !== 1) throw new Error('node_id mismatch: ' + p.node_id());
});
if (!p) { console.log('\n1..' + 1 + '\nnot ok — constructor dead, aborting'); process.exit(1); }

// 2. Every method the 200ms browser poll loop calls must survive.
for (const m of ['poll_sync', 'try_propose', 'bft_tick', 'room_state_json']) {
  check('poll-loop method ' + m + '() does not panic', () => {
    if (typeof p[m] !== 'function') throw new Error('export missing');
    p[m]();
  });
}

// 3. The pacemaker must be inert while the leader is healthy...
check('bft_tick is silent before the view timeout', () => {
  const out = JSON.parse(p.bft_tick());
  if (out.length !== 0) throw new Error('spurious ViewChange: ' + JSON.stringify(out));
});

// 4. ...and must actually fire once a non-leader's view times out.
//    Uses the real 3s default timeout, so this test costs ~3.2s.
check('bft_tick emits a signed ViewChange after the leader stalls', () => {
  let nonLeader = null;
  for (const id of [1, 2, 3, 4]) {
    const n = new w.Platform333(id, new Uint32Array([1, 2, 3, 4]));
    if (!n.is_leader()) { nonLeader = n; break; }
  }
  if (!nonLeader) throw new Error('no non-leader among 4 validators');
  const t0 = Date.now();
  while (Date.now() - t0 < 3200) { /* the view timer is wall-clock; busy-wait */ }
  const out = JSON.parse(nonLeader.bft_tick());
  if (out.length === 0) throw new Error('pacemaker did not fire — leader stall goes unnoticed');
  if (out[0].channel !== 'bft') throw new Error('wrong channel: ' + out[0].channel);
});

// 5. The kernel tick must survive being driven — this is the supported path.
//    Note the contract it does NOT have: catch_unwind in worker.rs is inert here,
//    because wasm32-unknown-unknown's target spec forces "panic-strategy": "abort".
//    A job that panics traps the whole instance; a job must report failure as Err.
//    Documented so nobody re-reads worker.rs's comment as a browser guarantee.
//    # KG: lesson-333-catch-unwind-inert-on-wasm-2026-07-15
check('tick_kernel runs without trapping the instance', () => {
  p.tick_kernel(4);
  if (typeof p.kernel_healthy() !== 'boolean') throw new Error('kernel unreadable after tick');
});

console.log(failed === 0 ? '\nall passed' : '\n' + failed + ' FAILED');
process.exit(failed === 0 ? 0 : 1);
