"""OOPTDD adapter for the real payment333 Rust integration boundary.

Each positive event requires an exact libtest registration and a zero subprocess
exit. stdout is diagnostic, never the oracle.

KG: LakatosTree_333PaymentSafety_20260715 / OOPTDD_methodology_v1
"""
from __future__ import annotations

import os
import shlex
import subprocess
from pathlib import Path


_ADAPTER_DIR = Path(__file__).resolve().parent
_REPO_ROOT = _ADAPTER_DIR.parent
_TEST_HOST = os.environ.get("PAYMENT333_TEST_HOST", "")
_TEST_ROOT = Path(os.environ.get("PAYMENT333_TEST_ROOT", str(_REPO_ROOT)))
_MANIFEST = _TEST_ROOT / "333-payment" / "Cargo.toml"


def _execute(command: list[str], *, timeout: int = 600) -> subprocess.CompletedProcess:
    if _TEST_HOST:
        remote = f"cd {shlex.quote(str(_TEST_ROOT))} && {shlex.join(command)}"
        argv, cwd = ["ssh", _TEST_HOST, remote], _REPO_ROOT
    else:
        argv, cwd = command, _TEST_ROOT
    return subprocess.run(
        argv, cwd=cwd, capture_output=True, text=True, timeout=timeout, check=False
    )


def _execution_revision() -> str | None:
    proc = _execute(["git", "-C", str(_TEST_ROOT), "rev-parse", "HEAD"], timeout=30)
    return proc.stdout.strip() if proc.returncode == 0 else None


def _registered_tests() -> tuple[set[str], dict]:
    command = [
        "cargo", "test", "--release", "--manifest-path", str(_MANIFEST),
        "--test", "payment_safety", "--", "--list",
    ]
    proc = _execute(command)
    tests = {
        line.rsplit(": test", 1)[0].strip()
        for line in proc.stdout.splitlines()
        if line.rstrip().endswith(": test")
    }
    return tests, {
        "command": command,
        "returncode": proc.returncode,
        "registered": sorted(tests),
        "execution_host": _TEST_HOST or "local",
        "execution_root": str(_TEST_ROOT),
        "execution_revision": _execution_revision(),
        "stderr_tail": proc.stderr[-2000:],
    }


def _run_exact(test_name: str) -> dict:
    command = [
        "cargo", "test", "--release", "--manifest-path", str(_MANIFEST),
        "--test", "payment_safety", test_name, "--", "--exact",
    ]
    proc = _execute(command)
    return {
        "command": command,
        "returncode": proc.returncode,
        "stdout_tail": proc.stdout[-2000:],
        "stderr_tail": proc.stderr[-2000:],
    }


def run_payment_probe(backend, cid: str) -> dict:
    """Execute seven exact real-Rust tests and ship one structured event per pass."""
    cases = (
        ("owner_signature_and_pubkey_bound_account_are_mandatory", "payment_owner_auth_verified"),
        ("restart_preserves_transfer_and_control_signing_locks", "payment_restart_safety_verified"),
        ("durable_ledger_reopens_without_replay_window", "payment_durable_reopen_verified"),
        ("network_asset_genesis_and_committee_epoch_are_signature_bound", "payment_context_epoch_binding_verified"),
        ("rotation_cannot_cross_an_unsettled_fastpay_lock", "payment_rotation_lock_fence_verified"),
        ("settled_escrow_deposit_survives_committee_rotation", "payment_rotation_escrow_continuity_verified"),
        ("unified_fast_and_bft_lanes_preserve_supply_and_are_idempotent", "payment_unified_lanes_supply_verified"),
    )
    registered, registry = _registered_tests()
    summary = {"registry": registry, "tests": {}}
    if registry["returncode"] != 0:
        return summary
    for test_name, event_name in cases:
        if test_name not in registered:
            summary["tests"][test_name] = {"registered": False, "returncode": None}
            continue
        result = _run_exact(test_name)
        result["registered"] = True
        summary["tests"][test_name] = result
        if result["returncode"] == 0:
            backend.ship([{
                "cid": cid,
                "correlation_id": cid,
                "cycle_id": cid,
                "service": "payment333",
                "event": event_name,
                "oracle": "subprocess_exit_and_exact_test_registry",
                "test": test_name,
                "returncode": 0,
            }])
    return summary
