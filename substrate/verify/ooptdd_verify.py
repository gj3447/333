"""Cross-language LTDD bridge — a Python `ooptdd` gate judges a trace the Rust p333 substrate
emitted (as ooptdd-envelope JSONL).

    cargo run -p p333-ltdd --example emit_trace > verify/trace.jsonl
    python verify/ooptdd_verify.py verify/trace.jsonl verify/superpeer.yaml

The substrate GENERATES the trace (Rust); `ooptdd` VERIFIES it (Python) — different language and
process, so the judge does not descend from the same authority as the emit (this is the strongest
form of ooptdd's generator-≠-verifier principle, and the one thing that pushes against its own
single-authority blind spot). Production swaps the JSONL file for OTLP → a store (the OSS-survey
plan: relay metering → OpenMeter); the gate and this verifier are unchanged.

Requires `ooptdd` (e.g. `pip install ooptdd`); exits 0 GREEN / 1 RED / 2 inconclusive.
"""
import json
import os
import sys

from ooptdd.backends import MemoryBackend, memory_reset
from ooptdd.gate import evaluate, evidence_tier, load_gate

if len(sys.argv) != 3:
    sys.exit("usage: ooptdd_verify.py <trace.jsonl> <gate.yaml>")
trace_path, gate_path = sys.argv[1], sys.argv[2]

events = [json.loads(line) for line in open(trace_path, encoding="utf-8") if line.strip()]
if not events:
    sys.exit("no events in trace")

memory_reset()
backend = MemoryBackend()
backend.ship(events)  # the store the substrate "emitted to" — now ooptdd reads it back
os.environ.setdefault("OOPTDD_CID", events[0].get("cid") or events[0].get("cycle_id"))

res = evaluate(backend, load_gate(gate_path))
print(f"ooptdd verdict: ok={res['ok']}  evidence_tier={evidence_tier(res)!r}  "
      f"reachable={res['reachable']}")
_KINDS = ("invariant", "metamorphic", "present", "absent", "must_order",
          "external", "ratioMetric", "conforms", "heartbeat")
for c in res["checks"]:
    label = c.get("event") or next((k for k in _KINDS if k in c), "check")
    print(f"  {label:20} got={c.get('got')}  passed={c['passed']}")

if not res["reachable"]:
    sys.exit(2)
sys.exit(0 if res["ok"] else 1)
