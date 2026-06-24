#!/usr/bin/env sh
# Cross-language LTDD gate suite. Runs every p333 ooptdd gate over a REAL Rust-emitted trace and
# asserts GREEN (exit 0), then over an INJECTED adversarial trace and asserts RED (exit 1). This is
# the independent verifier (different language + process) the Rust cargo receipts complement —
# the forbid/invariant gates are proven to fire, not merely shipped against their own green input.
#
# Requires Docker (to emit traces) + the `ooptdd` Python package (../ooptdd). Run from anywhere:
#   sh verify/run_gates.sh
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OOPTDD=/data/kjra/PROJECT/PI/ooptdd
CACHE=/data/kjra/PROJECT/PI/.uv-cache
V="$ROOT/verify"
T="$(mktemp -d)"
fail=0

emit() { # emit <crate> <example>  -> stdout
  docker run --rm -v "$ROOT":/work -v p333_cargo:/usr/local/cargo/registry -v p333_target:/work/target \
    -w /work rust:1-slim sh -c "cargo run -q -p $1 --example $2 2>/dev/null"
}
gate() { # gate <trace> <gate.yaml> <want_exit> <label>
  ( cd "$OOPTDD" && UV_CACHE_DIR="$CACHE" uv run --extra dev python "$V/ooptdd_verify.py" "$1" "$2" ) >/dev/null 2>&1
  rc=$?
  if [ "$rc" -eq "$3" ]; then echo "  PASS  $4 (exit $rc)"; else echo "  FAIL  $4 (exit $rc, want $3)"; fail=1; fi
}

echo "== emitting Rust traces (Docker) =="
emit p333-ltdd      emit_trace           > "$T/superpeer.jsonl"
emit p333-metering  emit_metering        > "$T/relay_credit.jsonl"
emit p333-metering  emit_session         > "$T/session.jsonl"
emit p333-crdt      emit_convergence     > "$T/crdt.jsonl"
emit p333-crdt      emit_yrs_convergence > "$T/yrs.jsonl"
emit p333-replay    emit_replay          > "$T/replay.jsonl"
emit p333-consensus emit_spend           > "$T/spend.jsonl"
emit p333-billing   emit_billing         > "$T/billing.jsonl"

echo "== GREEN: each gate over its real trace (want exit 0) =="
gate "$T/superpeer.jsonl"    "$V/superpeer.yaml"        0 "discovery/superpeer"
gate "$T/relay_credit.jsonl" "$V/relay_credit.yaml"     0 "metering conservation"
gate "$T/session.jsonl"      "$V/relay_credit.yaml"     0 "metering transport session"
gate "$T/crdt.jsonl"         "$V/crdt_convergence.yaml" 0 "crdt convergence (G-Counter)"
gate "$T/yrs.jsonl"          "$V/crdt_convergence.yaml" 0 "crdt convergence (real yrs)"
gate "$T/replay.jsonl"       "$V/determinism.yaml"      0 "replay determinism"
gate "$T/spend.jsonl"        "$V/owned_safety.yaml"     0 "owned-object safety"
gate "$T/billing.jsonl"      "$V/billing.yaml"          0 "billing (combined)"

echo "== RED: each forbid/invariant gate over an injected adversary (want exit 1) =="
cp "$T/crdt.jsonl" "$T/crdt_red.jsonl"
echo '{"cid":"doc-demo","cycle_id":"doc-demo","event":"replica_diverged","replica":"z","state":"X"}' >> "$T/crdt_red.jsonl"
gate "$T/crdt_red.jsonl" "$V/crdt_convergence.yaml" 1 "crdt: injected replica_diverged"

cp "$T/replay.jsonl" "$T/replay_red.jsonl"
echo '{"cid":"match-demo","cycle_id":"match-demo","event":"replay_diverged","run":9,"hash":1}' >> "$T/replay_red.jsonl"
gate "$T/replay_red.jsonl" "$V/determinism.yaml" 1 "replay: injected replay_diverged"

cp "$T/relay_credit.jsonl" "$T/relay_red.jsonl"
echo '{"cid":"relay-sess-demo","cycle_id":"relay-sess-demo","event":"relay_forwarded","bytes":9000,"cost":9}' >> "$T/relay_red.jsonl"
gate "$T/relay_red.jsonl" "$V/relay_credit.yaml" 1 "metering: free-riding forward"

cp "$T/spend.jsonl" "$T/spend_red.jsonl"
echo '{"cid":"spend-demo","cycle_id":"spend-demo","event":"spend_finalized","object":"coin-A","version":0,"txn":"x"}' >> "$T/spend_red.jsonl"
gate "$T/spend_red.jsonl" "$V/owned_safety.yaml" 1 "consensus: double-finalize"

cp "$T/billing.jsonl" "$T/billing_red.jsonl"
echo '{"cid":"billing-demo","cycle_id":"billing-demo","event":"equivocation_rejected","object":"alice-credits","version":0,"txn":"replay"}' >> "$T/billing_red.jsonl"
gate "$T/billing_red.jsonl" "$V/billing.yaml" 1 "billing: replayed-debit makes the forbid fire"

rm -rf "$T"
if [ "$fail" -eq 0 ]; then echo "== ALL GATES PASS: GREEN green, RED red =="; else echo "== SOME GATES FAILED =="; fi
exit "$fail"
