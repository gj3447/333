"""ooptdd-loop in_process target — conformance for the REAL transfer333 multi-process node.

NOT a mock. ``run_node_probe`` spawns the actual ``node`` binary as N separate OS
processes (4 authorities + a submit client) that talk over real TCP, then reads their
live JSON stdout events and ships one trace event per genuinely-observed behaviour.
A broken node (no consensus / diverged ledgers / double-spend applied / skipped seq
accepted) => the corresponding bound gate goes RED.

Behaviours (one gate each in node_requirements.yaml):
    node_certifies_over_tcp    : an honest transfer certifies across 4 TCP authority processes
    authority_ledgers_converge : all 4 independent authority ledgers apply the SAME balances
    double_spend_rejected      : a reused-seq spend gets no cert AND leaves ledgers unchanged
    out_of_order_rejected      : a skipped-seq spend gets no cert (ordering invariant holds)

# KG: transport-plan Step 8 / node-binary (2026-07-14)
"""
from __future__ import annotations

import json
import socket
import subprocess
import threading
import time
from pathlib import Path

_ADAPTER_DIR = Path(__file__).resolve().parent
_NODE_BIN = _ADAPTER_DIR.parent / "333-transfer" / "target" / "debug" / "node"
_COMMITTEE = "a0=0,a1=1,a2=2,a3=3"
_N_AUTH = 4
_GENESIS = "alice=100,bob=0"


def _ev(cid: str, event: str, **attrs) -> dict:
    return {
        "cid": cid,
        "correlation_id": cid,
        "cycle_id": cid,
        "service": "transfer333-node",
        "event": event,
        **attrs,
    }


def _free_ports(n: int) -> list[int]:
    # Bind ALL sockets first, THEN read ports + close — so the OS never hands the
    # same just-freed port back on the next bind (a sequential picker can collide
    # the client port onto an authority port -> submit bind fails -> no cert).
    socks = [socket.socket(socket.AF_INET, socket.SOCK_STREAM) for _ in range(n)]
    for s in socks:
        s.bind(("127.0.0.1", 0))
    ports = [s.getsockname()[1] for s in socks]
    for s in socks:
        s.close()
    return ports


class _Proc:
    """A node child process whose JSON stdout lines are drained by a reader thread.

    stderr is ALSO drained on its own thread — an undrained PIPE fills at ~64 KB and
    the child then blocks on its next stderr write (the classic subprocess deadlock).
    """

    def __init__(self, name: str, argv: list[str]):
        self.name = name
        self.events: list[dict] = []
        self.stderr_lines: list[str] = []
        self._lock = threading.Lock()
        # stdin=PIPE kept OPEN for the process's lifetime: the node treats stdin EOF
        # as a shutdown signal, so an inherited/closed stdin makes authorities exit
        # right after committee_ready (before submit connects).
        self.proc = subprocess.Popen(
            argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, text=True, bufsize=1
        )
        self._t = threading.Thread(target=self._drain, daemon=True)
        self._t.start()
        self._te = threading.Thread(target=self._drain_err, daemon=True)
        self._te.start()

    def _drain(self) -> None:
        assert self.proc.stdout is not None
        for line in self.proc.stdout:
            line = line.strip()
            if not line.startswith("{"):
                continue
            try:
                obj = json.loads(line)
            except json.JSONDecodeError:
                continue
            with self._lock:
                self.events.append(obj)

    def _drain_err(self) -> None:
        assert self.proc.stderr is not None
        for line in self.proc.stderr:
            with self._lock:
                self.stderr_lines.append(line.rstrip())

    def snapshot(self) -> list[dict]:
        with self._lock:
            return list(self.events)

    def wait_event(self, name: str, timeout: float = 10.0) -> dict | None:
        deadline = time.time() + timeout
        while time.time() < deadline:
            for e in self.snapshot():
                if e.get("event") == name:
                    return e
            if self.proc.poll() is not None:
                # process exited — one last look then give up
                for e in self.snapshot():
                    if e.get("event") == name:
                        return e
                return None
            time.sleep(0.02)
        return None

    def stop(self) -> None:
        if self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=3)
            except subprocess.TimeoutExpired:
                self.proc.kill()


def _submit(client_port: int, auth_addrs: list[str], transfer: str) -> _Proc:
    argv = [
        str(_NODE_BIN), "submit",
        "--seed", "99",
        "--listen", f"127.0.0.1:{client_port}",
        "--peers", ",".join(auth_addrs),
        "--committee", _COMMITTEE,
        "--transfer", transfer,
        "--max-rounds", "300",
        "--pause-ms", "10",
    ]
    return _Proc("submit", argv)


def _cert_applied_balances(auth: _Proc) -> list[dict]:
    return [e for e in auth.snapshot() if e.get("event") == "cert_applied"]


def run_node_probe(backend, cid: str) -> dict:
    """Loop entry point. Boots a real 4-authority TCP node network + submits transfers,
    shipping a trace event per genuinely-observed distributed-systems behaviour."""
    summary: dict = {"node_bin": str(_NODE_BIN)}
    if not _NODE_BIN.exists():
        subprocess.run(
            ["cargo", "build", "--bin", "node"],
            cwd=str(_ADAPTER_DIR.parent / "333-transfer"), check=True,
        )

    _ports = _free_ports(_N_AUTH + 1)
    auth_ports = _ports[:_N_AUTH]
    client_port = _ports[_N_AUTH]
    auth_addrs = [f"127.0.0.1:{p}" for p in auth_ports]
    summary["auth_addrs"] = auth_addrs

    authorities: list[_Proc] = []
    try:
        for i in range(_N_AUTH):
            argv = [
                str(_NODE_BIN), "authority",
                "--id", f"a{i}", "--seed", str(i),
                "--listen", auth_addrs[i],
                "--peers", ",".join(auth_addrs),
                "--committee", _COMMITTEE,
                "--genesis", _GENESIS,
                "--rounds-idle-exit", "100000",
            ]
            authorities.append(_Proc(f"a{i}", argv))

        # Wait until every authority meshed (committee_ready).
        ready = all(a.wait_event("committee_ready", timeout=15.0) for a in authorities)
        summary["committee_ready"] = ready
        time.sleep(0.3)  # settle accept threads

        # 1) Honest transfer certifies over 4 TCP authority processes.
        s1 = _submit(client_port, auth_addrs, "alice:0:bob:30")
        certified = s1.wait_event("certified", timeout=25.0)
        summary["certified"] = bool(certified)
        if certified and certified.get("status") == "Ok":
            backend.ship([_ev(cid, "node_certifies_over_tcp", transfer=certified.get("transfer"))])
        s1.stop()

        # 2) Convergence: every authority applied the SAME balances {alice:70, bob:30}.
        applied = [a.wait_event("cert_applied", timeout=10.0) for a in authorities]
        summary["cert_applied_each"] = [bool(x) for x in applied]
        balances_all = [x.get("balances") if x else None for x in applied]
        summary["balances_all"] = balances_all
        want = {"alice": 70, "bob": 30}
        converged = (
            all(x is not None for x in applied)
            and all(b == want for b in balances_all)
        )
        if converged:
            backend.ship([_ev(cid, "authority_ledgers_converge", balances=want, n=_N_AUTH)])

        # snapshot cert_applied counts BEFORE the adversarial submits
        pre_counts = [len(_cert_applied_balances(a)) for a in authorities]

        # 3) Double-spend: reuse seq 0 for a different payout → no cert, ledgers unchanged.
        s2 = _submit(client_port, auth_addrs, "alice:0:bob:99")
        failed2 = s2.wait_event("cert_failed", timeout=25.0)
        s2.stop()
        time.sleep(0.3)
        post_counts = [len(_cert_applied_balances(a)) for a in authorities]
        balances_unchanged = post_counts == pre_counts  # no new cert applied anywhere
        summary["double_spend"] = {"cert_failed": bool(failed2), "unchanged": balances_unchanged}
        if failed2 and balances_unchanged:
            backend.ship([_ev(cid, "double_spend_rejected", reason=failed2.get("reason"))])

        # 4) Out-of-order: skip seq 1, submit seq 2 → no cert.
        s3 = _submit(client_port, auth_addrs, "alice:2:bob:5")
        failed3 = s3.wait_event("cert_failed", timeout=25.0)
        s3.stop()
        summary["out_of_order"] = {"cert_failed": bool(failed3)}
        if failed3:
            backend.ship([_ev(cid, "out_of_order_rejected", reason=failed3.get("reason"))])

        return summary
    finally:
        for a in authorities:
            a.stop()
