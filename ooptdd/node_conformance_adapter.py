"""ooptdd-loop in_process target — conformance for the REAL transfer333 multi-process node.

NOT a mock. ``run_node_probe`` spawns the actual ``node`` binary as N separate OS
processes (4 authorities + a submit client) that talk over real TCP, then reads their
live JSON stdout events and ships one trace event per genuinely-observed behaviour.
A broken node (no consensus / diverged ledgers / double-spend applied / skipped seq
accepted) => the corresponding bound gate goes RED.

Behaviours (one gate each in node_requirements.yaml):
    forged_owner_rejected       : wrong-key Alice is rejected by every authority
    forged_order_zero_votes     : the forged order gets no vote and changes no ledger
    forgery_did_not_poison_slot : valid Alice certifies at the same slot immediately after
    node_certifies_over_tcp     : an honest transfer certifies across 4 TCP authority processes
    authority_ledgers_converge  : all 4 independent authority ledgers apply the SAME balances
    double_spend_rejected       : a reused-seq spend gets no cert AND leaves ledgers unchanged
    out_of_order_rejected       : a skipped-seq spend gets no cert (ordering invariant holds)
    overspend_rejected          : a signed overspend gets no vote and consumes no sequence
    duplicate_authority_key_rejected : one physical signing key cannot inflate quorum aliases
    invalid_genesis_rejected    : overflowing premint config fails before node startup

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
_COMMITTEE = ",".join([
    "a0=3b6a27bcceb6a42d62a3a8d02a6f0d73653215771de243a63ac048a18b59da29",
    "a1=8a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5c",
    "a2=8139770ea87d175f56a35466c34c7ecccb8d8a91b4ee37a25df60f5b8fc9b394",
    "a3=ed4928c628d1c2c6eae90338905995612959273a5c63f93636c14614ac8737d1",
])
_OWNER_ROSTER = ",".join([
    "alice=197f6b23e16c8532c6abc838facd5ea789be0c76b2920334039bfa8b3d368d61",
    "bob=4508a07aa941707f3eb2db94c8897a80b2c1197476b6de213ac273df7d86c4ff",
])
# b0 (dev-seed 100): the member JOINING via epoch change in probe 10 — an
# observer until the new roster activates (committee-reconfiguration M3).
_B0_PUB = "2bc2800b3316e009209ffd757dab19ccf0ae84bc7ae90654e1e81712d270f653"
_NEXT_COMMITTEE = ",".join([
    "a0=3b6a27bcceb6a42d62a3a8d02a6f0d73653215771de243a63ac048a18b59da29",
    "a1=8a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5c",
    "a2=8139770ea87d175f56a35466c34c7ecccb8d8a91b4ee37a25df60f5b8fc9b394",
    f"b0={_B0_PUB}",
])
_NETWORK_ID = "transfer333-ooptdd-v2"
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
        return self.wait_matching(lambda event: event.get("event") == name, timeout)

    def wait_matching(self, predicate, timeout: float = 10.0) -> dict | None:
        deadline = time.time() + timeout
        while time.time() < deadline:
            for e in self.snapshot():
                if predicate(e):
                    return e
            if self.proc.poll() is not None:
                # process exited — one last look then give up
                for e in self.snapshot():
                    if predicate(e):
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


def _submit(
    client_port: int, auth_addrs: list[str], transfer: str, owner_seed: int,
    committee: str = _COMMITTEE,
) -> _Proc:
    argv = [
        str(_NODE_BIN), "submit",
        "--dev-seed", str(owner_seed),
        "--network-id", _NETWORK_ID,
        "--owner-roster", _OWNER_ROSTER,
        "--listen", f"127.0.0.1:{client_port}",
        "--peers", ",".join(auth_addrs),
        "--committee", committee,
        "--transfer", transfer,
        "--max-rounds", "300",
        "--pause-ms", "10",
    ]
    return _Proc("submit", argv)


def _cert_applied_balances(auth: _Proc) -> list[dict]:
    return [e for e in auth.snapshot() if e.get("event") == "cert_applied"]


# One emitter symbol per requirement. Longinus binds the exact behavior literal
# to the exact oracle that decides whether evidence is strong enough to ship it.
def _emit_forged_owner_rejected(backend, cid: str, ok: bool, **attrs) -> bool:  # KG: transfer333-req-forged-owner-rejected-20260715
    if ok:
        backend.ship([_ev(cid, "forged_owner_rejected", **attrs)])
    return ok


def _emit_forged_order_zero_votes(backend, cid: str, ok: bool, **attrs) -> bool:  # KG: transfer333-req-forged-order-zero-votes-20260715
    if ok:
        backend.ship([_ev(cid, "forged_order_zero_votes", **attrs)])
    return ok


def _emit_overspend_rejected(backend, cid: str, ok: bool, **attrs) -> bool:  # KG: transfer333-req-overspend-rejected-20260715
    if ok:
        backend.ship([_ev(cid, "overspend_rejected", **attrs)])
    return ok


def _emit_duplicate_authority_key_rejected(backend, cid: str, ok: bool, **attrs) -> bool:  # KG: transfer333-req-duplicate-authority-key-rejected-20260715
    if ok:
        backend.ship([_ev(cid, "duplicate_authority_key_rejected", **attrs)])
    return ok


def _emit_invalid_genesis_rejected(backend, cid: str, ok: bool, **attrs) -> bool:  # KG: transfer333-req-invalid-genesis-rejected-20260715
    if ok:
        backend.ship([_ev(cid, "invalid_genesis_rejected", **attrs)])
    return ok


def _emit_forgery_did_not_poison_slot(backend, cid: str, ok: bool, **attrs) -> bool:  # KG: transfer333-req-forgery-no-slot-poison-20260715
    if ok:
        backend.ship([_ev(cid, "forgery_did_not_poison_slot", **attrs)])
    return ok


def _emit_node_certifies_over_tcp(backend, cid: str, ok: bool, **attrs) -> bool:  # KG: transfer333-req-certify-over-tcp-20260715
    if ok:
        backend.ship([_ev(cid, "node_certifies_over_tcp", **attrs)])
    return ok


def _emit_authority_ledgers_converge(backend, cid: str, ok: bool, **attrs) -> bool:  # KG: transfer333-req-ledgers-converge-20260715
    if ok:
        backend.ship([_ev(cid, "authority_ledgers_converge", **attrs)])
    return ok


def _emit_double_spend_rejected(backend, cid: str, ok: bool, **attrs) -> bool:  # KG: transfer333-req-double-spend-rejected-20260715
    if ok:
        backend.ship([_ev(cid, "double_spend_rejected", **attrs)])
    return ok


def _emit_out_of_order_rejected(backend, cid: str, ok: bool, **attrs) -> bool:  # KG: transfer333-req-out-of-order-rejected-20260715
    if ok:
        backend.ship([_ev(cid, "out_of_order_rejected", **attrs)])
    return ok


def _emit_cert_anti_entropy_rebroadcast(backend, cid: str, ok: bool, **attrs) -> bool:  # KG: transfer333-req-cert-anti-entropy-rebroadcast-20260803
    if ok:
        backend.ship([_ev(cid, "cert_anti_entropy_rebroadcast", **attrs)])
    return ok


def _emit_client_retry_late_quorum(backend, cid: str, ok: bool, **attrs) -> bool:  # KG: transfer333-req-client-retry-late-quorum-20260803
    if ok:
        backend.ship([_ev(cid, "client_retry_late_quorum", **attrs)])
    return ok


def _emit_anti_entropy_convergence(backend, cid: str, ok: bool, **attrs) -> bool:  # KG: transfer333-req-anti-entropy-convergence-20260803
    if ok:
        backend.ship([_ev(cid, "anti_entropy_convergence", **attrs)])
    return ok


def _emit_epoch_change(backend, cid: str, ok: bool, **attrs) -> bool:  # KG: transfer333-req-epoch-change-20260803
    if ok:
        backend.ship([_ev(cid, "epoch_change", **attrs)])
    return ok


def _emit_epoch_safety(backend, cid: str, ok: bool, **attrs) -> bool:  # KG: transfer333-req-epoch-safety-20260803
    if ok:
        backend.ship([_ev(cid, "epoch_safety", **attrs)])
    return ok


def _emit_epoch_straggler(backend, cid: str, ok: bool, **attrs) -> bool:  # KG: transfer333-req-epoch-straggler-20260803
    if ok:
        backend.ship([_ev(cid, "epoch_straggler", **attrs)])
    return ok


def _authority_config_probe(*, committee: str, genesis: str) -> subprocess.CompletedProcess:
    """Run one authority config through the real CLI; expected failures occur pre-bind."""
    return subprocess.run(
        [
            str(_NODE_BIN), "authority",
            "--id", "a0", "--dev-seed", "0",
            "--network-id", _NETWORK_ID,
            "--owner-roster", _OWNER_ROSTER,
            "--listen", "127.0.0.1:0",
            "--peers", "127.0.0.1:1",
            "--committee", committee,
            "--genesis", genesis,
            "--rounds-idle-exit", "1",
        ],
        capture_output=True,
        text=True,
        timeout=5,
        check=False,
    )


def run_node_probe(backend, cid: str) -> dict:  # KG: transfer333-node-conformance-20260715
    """Loop entry point. Boots a real 4-authority TCP node network + submits transfers,
    shipping a trace event per genuinely-observed distributed-systems behaviour."""
    summary: dict = {"node_bin": str(_NODE_BIN)}
    # Always ask Cargo for the current binary. The build is incremental, and
    # existence alone is unsafe because target/debug/node may predate this CLI
    # and wire contract.
    subprocess.run(
        ["cargo", "build", "--bin", "node"],
        cwd=str(_ADAPTER_DIR.parent / "333-transfer"), check=True,
    )

    # Configuration falsifiers run through the real node CLI before
    # any TCP authority is admitted to the live committee.
    shared_key = _COMMITTEE.split(",", 1)[0].split("=", 1)[1]
    alias_inflated_committee = ",".join(
        f"a{i}={shared_key}" for i in range(_N_AUTH)
    )
    duplicate_key = _authority_config_probe(
        committee=alias_inflated_committee,
        genesis=_GENESIS,
    )
    duplicate_key_ok = (
        duplicate_key.returncode != 0
        and "empty or duplicate committee" in duplicate_key.stderr
    )
    summary["duplicate_authority_key"] = {
        "exit": duplicate_key.returncode,
        "rejected": duplicate_key_ok,
    }
    _emit_duplicate_authority_key_rejected(
        backend,
        cid,
        duplicate_key_ok,
        aliases=_N_AUTH,
        distinct_public_keys=1,
    )

    overflowing_genesis = (
        "alice=340282366920938463463374607431768211455,bob=1"
    )
    invalid_genesis = _authority_config_probe(
        committee=_COMMITTEE,
        genesis=overflowing_genesis,
    )
    invalid_genesis_ok = (
        invalid_genesis.returncode != 0
        and "TotalSupplyOverflow" in invalid_genesis.stderr
    )
    summary["invalid_genesis"] = {
        "exit": invalid_genesis.returncode,
        "rejected": invalid_genesis_ok,
    }
    _emit_invalid_genesis_rejected(
        backend,
        cid,
        invalid_genesis_ok,
        reason="total_supply_overflow",
    )

    # One distinct client port per sequential submit avoids TIME_WAIT/rebind
    # coupling between the five sequential submit probes; +1 for the b0
    # observer's listen address (probe 10 epoch change).
    _ports = _free_ports(_N_AUTH + 6)
    auth_ports = _ports[:_N_AUTH]
    (
        forged_client_port,
        overspend_client_port,
        valid_client_port,
        double_client_port,
        skip_client_port,
    ) = (
        _ports[_N_AUTH : _N_AUTH + 5]
    )
    b0_port = _ports[_N_AUTH + 5]
    auth_addrs = [f"127.0.0.1:{p}" for p in auth_ports]
    b0_addr = f"127.0.0.1:{b0_port}"
    summary["auth_addrs"] = auth_addrs

    authorities: list[_Proc] = []
    b0: _Proc | None = None
    try:
        for i in range(_N_AUTH):
            argv = [
                str(_NODE_BIN), "authority",
                "--id", f"a{i}", "--dev-seed", str(i),
                "--network-id", _NETWORK_ID,
                "--owner-roster", _OWNER_ROSTER,
                "--listen", auth_addrs[i],
                # b0's addr is in the peer list so the observer meshes from
                # boot (half-mesh rule: exactly one side dials per pair,
                # regardless of where its random port lands).
                "--peers", ",".join(auth_addrs + [b0_addr]),
                "--committee", _COMMITTEE,
                "--genesis", _GENESIS,
                "--rounds-idle-exit", "100000",
            ]
            authorities.append(_Proc(f"a{i}", argv))

        # b0 boots as an OBSERVER from the start (not in the epoch-0 roster):
        # it converges via old-epoch certificates and becomes a voter only
        # when the probe-10 epoch change activates the new roster.
        b0 = _Proc("b0", [
            str(_NODE_BIN), "authority",
            "--id", "b0", "--dev-seed", "100", "--observe",
            "--network-id", _NETWORK_ID,
            "--owner-roster", _OWNER_ROSTER,
            "--listen", b0_addr,
            "--peers", ",".join(auth_addrs),
            "--committee", _COMMITTEE,
            "--genesis", _GENESIS,
            "--rounds-idle-exit", "100000",
        ])

        # Wait until every authority meshed (committee_ready).
        ready = all(a.wait_event("committee_ready", timeout=15.0) for a in authorities)
        summary["committee_ready"] = ready
        time.sleep(0.3)  # settle accept threads

        # 1) Forge first. Seed 99 signs as Alice, whose roster key is seed 42.
        # Every authority must reject before sequence inspection/slot mutation.
        forged_transfer = "alice:0:bob:30"
        forged = _submit(forged_client_port, auth_addrs, forged_transfer, 99)
        forged_failed = forged.wait_event("cert_failed", timeout=25.0)
        forged_order_id = forged_failed.get("order_id") if forged_failed else None
        owner_rejections = [
            a.wait_matching(
                lambda event, order_id=forged_order_id: (
                    event.get("event") == "owner_auth_rejected"
                    and event.get("order_id") == order_id
                ),
                timeout=10.0,
            )
            for a in authorities
        ]
        forged.stop()
        time.sleep(0.3)

        rejected_by_all = (
            bool(forged_failed)
            and all(e is not None for e in owner_rejections)
            and all(
                e.get("transfer") == forged_transfer
                and e.get("reason") == "invalid_owner_signature"
                and e.get("order_id") == forged_order_id
                for e in owner_rejections
                if e is not None
            )
        )
        summary["forged_owner"] = {
            "cert_failed": bool(forged_failed),
            "rejected_each": [bool(e) for e in owner_rejections],
            "reasons": [e.get("reason") if e else None for e in owner_rejections],
        }
        _emit_forged_owner_rejected(
            backend,
            cid,
            ready and rejected_by_all,
            transfer=forged_transfer,
            order_id=forged_order_id,
            authorities=_N_AUTH,
        )

        forged_votes = [
            e
            for a in authorities
            for e in a.snapshot()
            if e.get("event") == "vote_cast"
            and e.get("order_id") == forged_order_id
        ]
        forged_applies = [
            e
            for a in authorities
            for e in a.snapshot()
            if e.get("event") == "cert_applied"
            and e.get("order_id") == forged_order_id
        ]
        zero_vote_no_apply = not forged_votes and not forged_applies
        summary["forged_order_zero_votes"] = {
            "votes": len(forged_votes),
            "cert_applies": len(forged_applies),
        }
        _emit_forged_order_zero_votes(
            backend,
            cid,
            ready and rejected_by_all and zero_vote_no_apply,
            transfer=forged_transfer,
            order_id=forged_order_id,
            votes=0,
            cert_applies=0,
        )

        # 2) A correctly signed overspend must be refused before slot mutation.
        overspend_transfer = "alice:0:bob:101"
        overspend = _submit(
            overspend_client_port, auth_addrs, overspend_transfer, 42
        )
        overspend_failed = overspend.wait_event("cert_failed", timeout=25.0)
        overspend_order_id = (
            overspend_failed.get("order_id") if overspend_failed else None
        )
        balance_rejections = [
            a.wait_matching(
                lambda event, order_id=overspend_order_id: (
                    event.get("event") == "state_rejected"
                    and event.get("order_id") == order_id
                    and event.get("reason") == "insufficient_balance"
                    and event.get("have") == 100
                    and event.get("need") == 101
                ),
                timeout=10.0,
            )
            for a in authorities
        ]
        overspend.stop()
        overspend_votes = [
            event
            for authority in authorities
            for event in authority.snapshot()
            if event.get("event") == "vote_cast"
            and event.get("order_id") == overspend_order_id
        ]
        overspend_applies = [
            event
            for authority in authorities
            for event in authority.snapshot()
            if event.get("event") == "cert_applied"
            and event.get("order_id") == overspend_order_id
        ]
        overspend_rejected = (
            bool(overspend_failed)
            and overspend_order_id is not None
            and all(event is not None for event in balance_rejections)
            and not overspend_votes
            and not overspend_applies
        )
        summary["overspend"] = {
            "cert_failed": bool(overspend_failed),
            "rejected_each": [bool(event) for event in balance_rejections],
            "votes": len(overspend_votes),
            "cert_applies": len(overspend_applies),
        }
        _emit_overspend_rejected(
            backend,
            cid,
            ready and overspend_rejected,
            transfer=overspend_transfer,
            order_id=overspend_order_id,
            authorities=_N_AUTH,
        )

        # 3) Legitimate Alice now submits the same slot after both rejections.
        s1 = _submit(valid_client_port, auth_addrs, forged_transfer, 42)
        certified = s1.wait_event("certified", timeout=25.0)
        summary["certified"] = bool(certified)
        valid_order_id = certified.get("order_id") if certified else None
        certified_ok = bool(certified and certified.get("status") == "Ok")
        _emit_node_certifies_over_tcp(
            backend,
            cid,
            ready and certified_ok,
            transfer=certified.get("transfer") if certified else None,
            order_id=valid_order_id,
        )
        s1.stop()

        # 4) Convergence for the exact legitimate order id.
        applied = [
            a.wait_matching(
                lambda event, order_id=valid_order_id: (
                    event.get("event") == "cert_applied"
                    and event.get("order_id") == order_id
                ),
                timeout=10.0,
            )
            for a in authorities
        ]
        summary["cert_applied_each"] = [bool(x) for x in applied]
        balances_all = [x.get("balances") if x else None for x in applied]
        summary["balances_all"] = balances_all
        want = {"alice": 70, "bob": 30}
        converged = (
            all(x is not None for x in applied)
            and all(b == want for b in balances_all)
            and all(x.get("total_supply") == 100 for x in applied if x is not None)
        )
        _emit_authority_ledgers_converge(
            backend, cid, ready and converged, balances=want, n=_N_AUTH,
            order_id=valid_order_id,
        )
        same_slot_recovered = bool(
            ready
            and rejected_by_all
            and zero_vote_no_apply
            and overspend_rejected
            and certified_ok
            and converged
            and len({forged_order_id, overspend_order_id, valid_order_id}) == 3
        )
        _emit_forgery_did_not_poison_slot(
            backend,
            cid,
            same_slot_recovered,
            transfer=forged_transfer,
            slot="alice:0",
            order_id=valid_order_id,
        )
        summary["forgery_did_not_poison_slot"] = same_slot_recovered

        # snapshot cert_applied counts BEFORE the adversarial submits
        pre_counts = [len(_cert_applied_balances(a)) for a in authorities]

        # 5) Double-spend: require exact typed rejections from all authorities.
        s2 = _submit(double_client_port, auth_addrs, "alice:0:bob:99", 42)
        failed2 = s2.wait_event("cert_failed", timeout=25.0)
        double_order_id = failed2.get("order_id") if failed2 else None
        double_rejections = [
            authority.wait_matching(
                lambda event, order_id=double_order_id: (
                    event.get("event") == "out_of_order"
                    and event.get("order_id") == order_id
                    and event.get("expected") == 1
                    and event.get("got") == 0
                    and event.get("balances") == want
                    and event.get("total_supply") == 100
                ),
                timeout=10.0,
            )
            for authority in authorities
        ]
        s2.stop()
        time.sleep(0.3)
        post_counts = [len(_cert_applied_balances(a)) for a in authorities]
        balances_unchanged = (
            post_counts == pre_counts
            and all(event is not None for event in double_rejections)
        )
        summary["double_spend"] = {"cert_failed": bool(failed2), "unchanged": balances_unchanged}
        double_rejected = bool(
            failed2
            and all(event is not None for event in double_rejections)
            and balances_unchanged
        )
        _emit_double_spend_rejected(
            backend,
            cid,
            double_rejected,
            reason=failed2.get("reason") if failed2 else None,
            order_id=double_order_id,
        )

        # 6) Out-of-order: require expected=1/got=2 from every authority.
        s3 = _submit(skip_client_port, auth_addrs, "alice:2:bob:5", 42)
        failed3 = s3.wait_event("cert_failed", timeout=25.0)
        skip_order_id = failed3.get("order_id") if failed3 else None
        skip_rejections = [
            authority.wait_matching(
                lambda event, order_id=skip_order_id: (
                    event.get("event") == "out_of_order"
                    and event.get("order_id") == order_id
                    and event.get("expected") == 1
                    and event.get("got") == 2
                    and event.get("balances") == want
                    and event.get("total_supply") == 100
                ),
                timeout=10.0,
            )
            for authority in authorities
        ]
        s3.stop()
        skip_rejected = bool(
            failed3 and all(event is not None for event in skip_rejections)
        )
        summary["out_of_order"] = {
            "cert_failed": bool(failed3),
            "rejected_each": [bool(event) for event in skip_rejections],
        }
        _emit_out_of_order_rejected(
            backend,
            cid,
            skip_rejected,
            reason=failed3.get("reason") if failed3 else None,
            order_id=skip_order_id,
        )

        # 7) Anti-entropy: an authority that applied the step-3 certificate
        # must re-present it during a quiet period. A peer that missed the
        # cert (late boot, lost frame) then converges level-triggered instead
        # of staying locked on an old sequence forever (audit 2026-07-15 P1).
        rebroadcasts = [
            a.wait_event("cert_rebroadcast", timeout=12.0) for a in authorities
        ]
        seen_rebroadcast = sum(1 for e in rebroadcasts if e)
        summary["cert_rebroadcast"] = {
            "authorities": [bool(e) for e in rebroadcasts],
        }
        _emit_cert_anti_entropy_rebroadcast(
            backend,
            cid,
            ready and seen_rebroadcast >= 1,
            authorities=_N_AUTH,
            seen=seen_rebroadcast,
        )

        # 8) Client retry liveness: with only 2 of 4 authorities up, the
        # submit client must keep (re-)dialing and re-broadcasting the order
        # until a late-booted third authority lets quorum form — never dying
        # on the first connect failure or a single lost broadcast.
        retry_ports = _free_ports(_N_AUTH + 1)
        retry_addrs = [f"127.0.0.1:{p}" for p in retry_ports[:3]]
        retry_client_port = retry_ports[3]
        early: list[_Proc] = []
        late: list[_Proc] = []

        def _retry_authority(i: int) -> _Proc:
            argv = [
                str(_NODE_BIN), "authority",
                "--id", f"a{i}", "--dev-seed", str(i),
                "--network-id", _NETWORK_ID,
                "--owner-roster", _OWNER_ROSTER,
                "--listen", retry_addrs[i],
                "--peers", ",".join(retry_addrs),
                "--committee", _COMMITTEE,
                "--genesis", _GENESIS,
                "--rounds-idle-exit", "100000",
            ]
            return _Proc(f"r{i}", argv)

        try:
            for i in (0, 1):
                early.append(_retry_authority(i))
            ready_early = all(
                a.wait_event("committee_ready", timeout=15.0) for a in early
            )
            # The retry authorities are FRESH processes at genesis — for them
            # alice's next sequence is 0 (the step-3 cert lives only on the
            # main committee's ledgers). Submitting seq 1 here would be a
            # self-inflicted out_of_order.
            s4 = _submit(retry_client_port, retry_addrs, "alice:0:bob:5", 42)
            time.sleep(2.0)
            late.append(_retry_authority(2))
            certified4 = s4.wait_event("certified", timeout=25.0)
            retry_ok = bool(
                ready_early
                and certified4
                and certified4.get("status") == "Ok"
            )
            summary["client_retry_late_quorum"] = {
                "early_ready": bool(ready_early),
                "certified": bool(certified4),
            }
            _emit_client_retry_late_quorum(
                backend,
                cid,
                retry_ok,
                transfer="alice:0:bob:5",
                order_id=certified4.get("order_id") if certified4 else None,
            )
            s4.stop()
        finally:
            for a in early + late:
                a.stop()

        # 9) Anti-entropy convergence: restart a3 with an empty (in-memory)
        # state — it has genesis only and missed the step-3 certificate.
        # The peers' quiet-period re-presentation must carry it to the same
        # balances (alice=70, bob=30): level-triggered reconciliation, not a
        # one-shot delivery (audit 2026-07-15 P1 acceptance criterion).
        a3 = authorities[3]
        a3.stop()
        time.sleep(0.5)
        argv = [
            str(_NODE_BIN), "authority",
            "--id", "a3", "--dev-seed", "3",
            "--network-id", _NETWORK_ID,
            "--owner-roster", _OWNER_ROSTER,
            "--listen", auth_addrs[3],
            "--peers", ",".join(auth_addrs),
            "--committee", _COMMITTEE,
            "--genesis", _GENESIS,
            "--rounds-idle-exit", "100000",
        ]
        a3_restarted = _Proc("a3r", argv)
        want = {"alice": 70, "bob": 30}
        converged = a3_restarted.wait_matching(
            lambda event: (
                event.get("event") == "cert_applied"
                and event.get("balances") == want
                and event.get("total_supply") == 100
            ),
            timeout=15.0,
        )
        summary["anti_entropy_convergence"] = {
            "restarted": bool(a3_restarted.wait_event("committee_ready", timeout=5.0)),
            "converged": bool(converged),
        }
        _emit_anti_entropy_convergence(
            backend,
            cid,
            ready and converged is not None,
            balances=want,
            order_id=converged.get("order_id") if converged else None,
        )
        # a3 stays UP (as the restarted instance) for probe 10: the retired
        # member must keep running — and keep trying to vote — to prove its
        # exclusion under the new roster.
        authorities[3] = a3_restarted

        # 10) Epoch change (committee-reconfiguration M3): live fence-then-
        # change reconfiguration 4 -> (a0,a1,a2,b0). b0 has observed since
        # boot and converged; a3 retires but keeps voting (and must be
        # excluded by the new-roster collector).
        b0_ready = b0.wait_event("committee_ready", timeout=5.0)
        b0_converged = b0.wait_matching(
            lambda event: (
                event.get("event") == "cert_applied"
                and event.get("balances") == want
            ),
            timeout=12.0,
        )
        summary["b0_observer"] = {"ready": bool(b0_ready), "converged": bool(b0_converged)}

        reconfig_port = _free_ports(1)[0]
        rc = subprocess.run(
            [
                str(_NODE_BIN), "reconfig",
                "--network-id", _NETWORK_ID,
                "--owner-roster", _OWNER_ROSTER,
                "--listen", f"127.0.0.1:{reconfig_port}",
                "--peers", ",".join(auth_addrs + [b0_addr]),
                "--committee", _COMMITTEE,
                "--next-committee", _NEXT_COMMITTEE,
                "--epoch", "1",
                "--max-rounds", "300",
                "--pause-ms", "10",
            ],
            capture_output=True,
            text=True,
            timeout=90,
            check=False,
        )
        reconfig_ok = rc.returncode == 0 and "epoch_cert_broadcast" in rc.stdout
        summary["reconfig"] = {
            "rc": rc.returncode,
            "stdout_tail": rc.stdout.strip().splitlines()[-3:] if rc.stdout else [],
            "stderr_tail": rc.stderr.strip().splitlines()[-3:] if rc.stderr else [],
        }

        installed = {
            name: proc.wait_event("epoch_installed", timeout=15.0) is not None
            for name, proc in [
                ("a0", authorities[0]),
                ("a1", authorities[1]),
                ("a2", authorities[2]),
                ("b0", b0),
            ]
        }
        summary["epoch_installed"] = installed

        # New-epoch transfer under the NEW roster. The retired a3 still
        # votes — and must be excluded by the collector (the cert can only
        # form from new-roster votes).
        s5 = _submit(
            _free_ports(1)[0],
            auth_addrs + [b0_addr],
            "alice:1:bob:5",
            42,
            committee=_NEXT_COMMITTEE,
        )
        certified5 = s5.wait_event("certified", timeout=25.0)
        a3_retired_vote = authorities[3].wait_matching(
            lambda event: (
                event.get("event") == "vote_cast"
                and event.get("epoch") == 1
                and event.get("transfer") == "alice:1:bob:5"
            ),
            timeout=12.0,
        )
        want_new = {"alice": 65, "bob": 35}
        new_members_applied = [
            p.wait_matching(
                lambda event: (
                    event.get("event") == "cert_applied"
                    and event.get("epoch") == 1
                    and event.get("balances") == want_new
                ),
                timeout=12.0,
            )
            for p in (authorities[0], authorities[1], authorities[2], b0)
        ]
        s5.stop()
        epoch_ok = bool(
            b0_ready
            and b0_converged
            and reconfig_ok
            and all(installed.values())
            and certified5
            and certified5.get("status") == "Ok"
            and a3_retired_vote is not None
            and all(new_members_applied)
        )
        summary["epoch_change"] = {
            "certified_new_roster": bool(certified5),
            "a3_retired_vote_seen": a3_retired_vote is not None,
            "new_members_applied": [bool(x) for x in new_members_applied],
        }
        _emit_epoch_change(
            backend,
            cid,
            epoch_ok,
            epoch=1,
            order_id=certified5.get("order_id") if certified5 else None,
        )

        # 11) Epoch safety (design §3/§7): the boundary must not leak a
        # double-spend, and the old trust root must be dead.
        #  (a) re-spending the pre-change alice:0 slot under the NEW roster
        #      -> out_of_order everywhere (the slot is spent), never a cert.
        s6 = _submit(
            _free_ports(1)[0],
            auth_addrs + [b0_addr],
            "alice:0:bob:99",
            42,
            committee=_NEXT_COMMITTEE,
        )
        failed6 = s6.wait_event("cert_failed", timeout=25.0)
        s6.stop()
        #  (b) a fresh order submitted under the OLD trust root: every live
        #      authority signs with the NEW committee_id now, so the old
        #      collector can never assemble a cert.
        s7 = _submit(
            _free_ports(1)[0],
            auth_addrs + [b0_addr],
            "alice:2:bob:1",
            42,
            committee=_COMMITTEE,
        )
        failed7 = s7.wait_event("cert_failed", timeout=25.0)
        s7.stop()
        # The gate is only meaningful POST-change: without epoch_installed
        # evidence, both cert_failed events fire for the wrong reasons
        # (trust-root mismatch / plain out_of_order) — so the precondition
        # is part of the oracle, keeping it RED on a pre-change binary.
        epoch_safety_ok = bool(failed6 and failed7 and all(installed.values()))
        summary["epoch_safety"] = {
            "respent_slot_rejected": bool(failed6),
            "old_trust_root_dead": bool(failed7),
            "post_change_precondition": all(installed.values()),
        }
        _emit_epoch_safety(backend, cid, epoch_safety_ok, epoch=1)

        # 12) Epoch straggler (design §7, M5 acceptance): a2 restarts at
        # genesis AFTER the change — it must receive the missed user certs
        # AND the epoch-1 cert via quiet-period re-presentation, install the
        # new epoch, and converge to the new roster's state.
        a2 = authorities[2]
        a2.stop()
        time.sleep(0.5)
        a2_restarted = _Proc("a2r", [
            str(_NODE_BIN), "authority",
            "--id", "a2", "--dev-seed", "2",
            "--network-id", _NETWORK_ID,
            "--owner-roster", _OWNER_ROSTER,
            "--listen", auth_addrs[2],
            "--peers", ",".join(auth_addrs + [b0_addr]),
            "--committee", _COMMITTEE,
            "--genesis", _GENESIS,
            "--rounds-idle-exit", "100000",
        ])
        authorities[2] = a2_restarted
        a2_epoch_installed = a2_restarted.wait_event("epoch_installed", timeout=20.0)
        a2_converged = a2_restarted.wait_matching(
            lambda event: (
                event.get("event") == "cert_applied"
                and event.get("epoch") == 1
                and event.get("balances") == {"alice": 65, "bob": 35}
            ),
            timeout=20.0,
        )
        straggler_ok = bool(a2_epoch_installed and a2_converged)
        summary["epoch_straggler"] = {
            "installed": bool(a2_epoch_installed),
            "converged_new_epoch": bool(a2_converged),
        }
        _emit_epoch_straggler(backend, cid, straggler_ok, epoch=1)

        return summary
    finally:
        for a in authorities:
            a.stop()
        if b0 is not None:
            b0.stop()
