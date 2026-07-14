#!/usr/bin/env bash
# Build 333-platform Rust core → WASM → 333-app/static/wasm/
# Usage: ./scripts/build-wasm.sh [--dev | --release]
# # KG: sprint3-3D-wasm-build-script-2026-04-15
set -euo pipefail

MODE="${1:---release}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$ROOT/333-app/static/wasm"

cd "$ROOT"
echo "[wasm-pack] mode=$MODE out=$OUT"
wasm-pack build --target web --out-dir "$OUT" "$MODE"

echo "[verify] checking exports…"
# KG: sprint5A-social-wasm-ui-2026-04-15 — SocialProfileWasm added
# KG: sprint5C-editor-wasm-ui-2026-04-15 — CollabEditorWasm added
# KG: sprint5B-om-wasm-ui-2026-04-15 — OmSessionWasm added
for sym in RtsSessionWasm rts_bft_checkpoint_stub rts_peer_eject_stub rts_ggrs_rollback_stub SocialProfileWasm CollabEditorWasm OmSessionWasm; do
  grep -q "$sym" "$OUT/triple_three.js" || { echo "  MISSING: $sym"; exit 1; }
  echo "  ✓ $sym"
done

SIZE=$(wc -c < "$OUT/triple_three_bg.wasm" | tr -d ' ')
echo "[size] $SIZE bytes"
[ "$SIZE" -lt 1000000 ] || echo "  WARN: WASM > 1MB, consider wasm-opt tuning"

echo "DONE"
