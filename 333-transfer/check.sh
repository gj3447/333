#!/usr/bin/env bash
# Local verification gate for transfer333.
#
# Not a GitHub Actions workflow on purpose: Actions is billing-blocked on this
# account, so a workflow file would only produce red X's that say nothing about
# the code. This script is the thing that actually runs.
#
# Usage:  ./check.sh
set -euo pipefail
cd "$(dirname "$0")"

fail=0
step() { printf '\n=== %s ===\n' "$1"; }

step "native build + full suite"
# --no-fail-fast matters: without it cargo stops at the first failing test
# binary and the remaining ones silently never run, which reads as a lower
# total rather than as a failure.
cargo test --no-fail-fast

step "wasm32 library builds"
# 333's identity is a P2P *browser* base, so the library — not the node binary —
# must compile for the browser target. Guards against reintroducing a native-only
# dependency into the library path (the last one was getrandom needing its "js"
# feature: a build gate, not a runtime one).
if ! rustup target list --installed | grep -q wasm32-unknown-unknown; then
    echo "wasm32-unknown-unknown target not installed; run:"
    echo "    rustup target add wasm32-unknown-unknown"
    fail=1
else
    cargo check --lib --target wasm32-unknown-unknown
fi

step "native still requires Send + Sync on AuthorityNet"
# The wasm build relaxes that bound (JS handles are !Send). This asserts the
# relaxation did not leak into native, where authorities really are shared across
# reader/listener threads.
probe=examples/_sendsync_bound_probe.rs
mkdir -p examples
cat > "$probe" <<'RS'
use transfer333::{AuthorityMsg, AuthorityNet, Certificate, NetError, SignedTransfer, Vote};
struct NotSend(*const u8);
impl AuthorityNet for NotSend {
    fn broadcast_order(&self, _: SignedTransfer) -> Result<(), NetError> { Ok(()) }
    fn broadcast_vote(&self, _: Vote) -> Result<(), NetError> { Ok(()) }
    fn broadcast_cert(&self, _: Certificate) -> Result<(), NetError> { Ok(()) }
    fn poll(&self) -> Vec<AuthorityMsg> { vec![] }
}
fn main() {}
RS
if cargo check --example _sendsync_bound_probe >/dev/null 2>&1; then
    echo "FAIL: a !Send type implemented AuthorityNet on native — the bound leaked"
    fail=1
else
    echo "ok: !Send impl correctly rejected"
fi
rm -f "$probe"; rmdir examples 2>/dev/null || true

if [ "$fail" -ne 0 ]; then
    printf '\ncheck.sh: FAILED\n'; exit 1
fi
printf '\ncheck.sh: all gates passed\n'
