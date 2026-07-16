#!/usr/bin/env sh
# Cross-language LTDD gate suite. Runs every p333 ooptdd gate over a REAL Rust-emitted trace and
# asserts GREEN (exit 0), then over an INJECTED adversarial trace and asserts RED (exit 1). This is
# the independent verifier (different language + process) the Rust cargo receipts complement —
# the forbid/invariant gates are proven to fire, not merely shipped against their own green input.
#
# Requires a container runtime (to emit traces) + an `ooptdd` source checkout. By default the
# checkout is resolved at ../ooptdd; set OOPTDD_PATH when the repositories are not siblings.
# Run from anywhere:
#   sh verify/run_gates.sh
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OOPTDD="${OOPTDD_PATH:-$ROOT/../ooptdd}"
CACHE="${OOPTDD_CACHE_DIR:-$OOPTDD/.uv-cache}"
CONTAINER_RUNTIME="${P333_CONTAINER_RUNTIME:-docker}"
RUST_IMAGE="${P333_RUST_IMAGE:-rust:1-slim}"
CARGO_VOLUME="${P333_CARGO_VOLUME:-p333_cargo}"
TARGET_VOLUME="${P333_TARGET_VOLUME:-p333_target}"
CRDT_CID="${P333_CRDT_CID:-p333-crdt-$(date -u '+%Y%m%dT%H%M%SZ')-$$}"
V="$ROOT/verify"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT HUP INT TERM
fail=0
gate_index=0

die() {
  echo "ERROR: $*" >&2
  exit 1
}

command -v "$CONTAINER_RUNTIME" >/dev/null 2>&1 ||
  die "container runtime '$CONTAINER_RUNTIME' not found; set P333_CONTAINER_RUNTIME or run the declared container route on a Docker host"
command -v uv >/dev/null 2>&1 || die "uv not found"
[ -f "$OOPTDD/pyproject.toml" ] ||
  die "ooptdd checkout not found at '$OOPTDD'; set OOPTDD_PATH to its source checkout"
case "$CRDT_CID" in
  ""|*[!A-Za-z0-9._:-]*) die "P333_CRDT_CID must use only A-Z, a-z, 0-9, dot, underscore, colon, or hyphen" ;;
esac

emit() { # emit <crate> <example> [cid] -> stdout
  "$CONTAINER_RUNTIME" run --rm \
    -e "P333_CID=${3:-}" \
    -v "$ROOT":/work \
    -v "$CARGO_VOLUME":/usr/local/cargo/registry \
    -v "$TARGET_VOLUME":/work/target \
    -w /work "$RUST_IMAGE" cargo run --locked -q -p "$1" --example "$2"
}
emit_to() { # emit_to <crate> <example> <trace> [cid]
  crate=$1
  example=$2
  trace=$3
  emit "$crate" "$example" "${4:-}" > "$trace" || die "Rust trace producer failed: $crate/$example"
  grep -q '[^[:space:]]' "$trace" || die "Rust trace producer emitted an empty trace: $crate/$example"
}
verify() {
  ( cd "$OOPTDD" && UV_CACHE_DIR="$CACHE" uv run --frozen --extra dev \
      python "$V/ooptdd_verify.py" "$1" "$2" )
}
gate() { # gate <trace> <gate.yaml> <want_exit> <label>
  gate_index=$((gate_index + 1))
  out="$T/gate-$gate_index.out"
  verify "$1" "$2" >"$out" 2>&1
  rc=$?
  expected_ok=False
  [ "$3" -eq 0 ] && expected_ok=True
  if [ "$rc" -eq "$3" ] &&
     grep -Fq "ooptdd verdict: ok=$expected_ok" "$out" &&
     grep -Fq "reachable=True" "$out"; then
    echo "  PASS  $4 (exit $rc, ok=$expected_ok, reachable=True)"
  else
    echo "  FAIL  $4 (exit $rc, want $3 with ok=$expected_ok and reachable=True)"
    sed 's/^/        /' "$out"
    fail=1
  fi
}

echo "== emitting Rust traces (container: $CONTAINER_RUNTIME, image: $RUST_IMAGE) =="
emit_to p333-ltdd      emit_trace           "$T/superpeer.jsonl"
emit_to p333-metering  emit_metering        "$T/relay_credit.jsonl"
emit_to p333-metering  emit_session         "$T/session.jsonl"
emit_to p333-crdt      emit_convergence     "$T/crdt.jsonl" "$CRDT_CID"
emit_to p333-crdt      emit_yrs_convergence "$T/yrs.jsonl"
emit_to p333-replay    emit_replay          "$T/replay.jsonl"
emit_to p333-consensus emit_spend           "$T/spend.jsonl"
emit_to p333-billing   emit_billing         "$T/billing.jsonl"

echo "== GREEN: each gate over its real trace (want exit 0) =="
echo "  focal correlation: $CRDT_CID"
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
printf '{"cid":"%s","cycle_id":"%s","event":"replica_diverged","replica":"z","state":"X"}\n' \
  "$CRDT_CID" "$CRDT_CID" >> "$T/crdt_red.jsonl"
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

echo "== RESTORED: focal real trace remains GREEN after fault injection =="
gate "$T/crdt.jsonl" "$V/crdt_convergence.yaml" 0 "crdt convergence restored"

if [ "$fail" -eq 0 ]; then echo "== ALL GATES PASS: GREEN green, RED red =="; else echo "== SOME GATES FAILED =="; fi
exit "$fail"
