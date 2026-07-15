#!/usr/bin/env python3
"""Run payment333 through the canonical vendored OOPTDD loop."""
from __future__ import annotations

import argparse
import dataclasses
import datetime as dt
import json
import os
import subprocess
import sys
from pathlib import Path


def _git(path: Path, *args: str) -> str | None:
    proc = subprocess.run(
        ["git", "-C", str(path), *args], capture_output=True, text=True, check=False
    )
    return proc.stdout.strip() if proc.returncode == 0 else None


def _receipt(run, runtime_root: Path, spec_path: Path) -> dict:
    requirements = []
    for result in run.results:
        requirements.append({
            "id": result.id,
            "done": result.done,
            "gate_ok": result.gate_ok,
            "bound": result.bound,
            "mutation_ok": result.mutation_ok,
            "checks": result.checks,
            "binding": dataclasses.asdict(result.binding) if result.binding else None,
            "rca": result.rca,
        })
    return {
        "schema": "payment333-ooptdd-receipt-v1",
        "generated_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "cid": run.cid,
        "spec": str(spec_path),
        "runtime_root": str(runtime_root),
        "runtime_revision": _git(runtime_root, "rev-parse", "HEAD"),
        "source_revision": _git(Path.cwd(), "rev-parse", "HEAD"),
        "execution": {
            "host": os.environ.get("PAYMENT333_TEST_HOST") or "local",
            "root": os.environ.get("PAYMENT333_TEST_ROOT") or str(Path.cwd()),
        },
        "backend": run.backend,
        "complete": run.complete,
        "done": run.n_done,
        "total": len(run.results),
        "methodology_ok": run.methodology_ok,
        "methodology_checks": [dataclasses.asdict(c) for c in run.methodology_checks],
        "requirements": requirements,
    }


def main() -> int:
    here = Path(__file__).resolve().parent
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--runtime-root", type=Path,
        default=Path(os.environ.get("OOPTDD_RUNTIME_ROOT", "")),
        help="directory containing canonical ooptdd/ and ooptdd_loop/ packages",
    )
    parser.add_argument("--spec", type=Path, default=here / "payment_requirements.yaml")
    parser.add_argument("--receipt", type=Path)
    parser.add_argument("--cid", default="payment333-prom4-ooptdd-20260715")
    args = parser.parse_args()
    runtime_root = args.runtime_root.resolve()
    if not (runtime_root / "ooptdd").is_dir() or not (runtime_root / "ooptdd_loop").is_dir():
        parser.error("--runtime-root must contain ooptdd/ and ooptdd_loop/")
    sys.path.insert(0, str(runtime_root))
    from ooptdd_loop.runner import run_until_complete
    from ooptdd_loop.spec import load_spec

    os.environ["OOPTDD_REQUIRE_BINDING"] = "1"
    run = run_until_complete(load_spec(str(args.spec)), cid=args.cid, max_passes=1)
    receipt = _receipt(run, runtime_root, args.spec)
    rendered = json.dumps(receipt, ensure_ascii=False, indent=2, sort_keys=True)
    print(rendered)
    if args.receipt:
        args.receipt.parent.mkdir(parents=True, exist_ok=True)
        args.receipt.write_text(rendered + "\n", encoding="utf-8")
    return 0 if run.complete else 1


if __name__ == "__main__":
    raise SystemExit(main())
