# Repository Contract

Instructions for coding agents and new contributors. This file is the single
source; `CLAUDE.md` points here. For what 333 *is*, read `README.md`.

333 is built PROM-style: test-first, **no fake green**, adversarial self-check.
A gate that goes green because it was weakened is worse than a red one.

## The layout is deliberate — do not "fix" it

There is **no top-level Cargo workspace, on purpose.** Each `333-<name>/` is a
standalone crate linked to its siblings by relative path dependencies, so a
work-in-progress crate never blocks the others (`README.md` §Layout).

Do not add a root `Cargo.toml` workspace. Do not consolidate the crates. If a
change seems to require it, that is a proposal for the user, not a refactor.

`substrate/` is the exception: it *is* a self-contained Cargo workspace
(the absorbed `gj3447/p333` line, `p333-*` crates, merged via `git subtree` with
full history). Its `p333-` names deliberately do not collide with the top-level
`*333` crates, and the overlapping domains (`consensus`/`crdt`/`identity`) are
kept side-by-side by design — the `*333` versions are the broad P2P-OS line, the
`p333-*` versions the receipt-driven substrate line. Do not deduplicate them.

## Verified core vs. as-recovered

`README.md` states it and it is still true: **`transfer333` + `crdt333` are the
verified-green core. Every other top-level crate is as-recovered and its build
state is not exhaustively verified.**

Measured 2026-08-10 on cargo 1.97.0 / macOS:

| target | command | result |
|---|---|---|
| `333-transfer` | `cargo test --all-targets` | **159 passed**, 0 failed, 0 ignored |
| `333-transfer` | `cargo test --doc` | 0 tests (no doctests) |
| `333-crdt` | `cargo test --all-targets` | **55 passed**, 0 failed, 0 ignored |
| `substrate` | `cargo test --locked` | **92 passed**, 0 failed, 1 ignored (46 test binaries) |

`README.md` §Verified core still says `89 green` for the transfer crate. That
number is stale — it is 159 now. Do not cite the README figure.

The other 30 crates were **not** built or tested in that measurement. If you
touch one, establish its baseline first and say what you found.

## Commands

```
cd 333-transfer && cargo test --all-targets     # verified core
cd 333-crdt     && cargo test --all-targets     # verified core
cd substrate    && cargo test --locked          # p333 workspace
```

`--all-targets` does **not** run doctests. If a crate has any, run
`cargo test --doc` as a separate step.

### Node conformance (the real gate for transfer333)

```
cd /path/to/333 && <ooptdd-loop-venv>/bin/ooptdd-loop run ooptdd/node_requirements.yaml --json
```

This is **not a mock.** `run_node_probe` spawns the actual `node` binary as
separate OS processes (4 authorities + a submit client) talking over real TCP,
reads their live JSON stdout, and ships one trace event per genuinely-observed
behaviour. Ten gates, each Longinus-bound to an event literal:

```
forged_owner_rejected              forgery_did_not_poison_slot
forged_order_zero_votes            node_certifies_over_tcp
authority_ledgers_converge         double_spend_rejected
out_of_order_rejected              overspend_rejected
duplicate_authority_key_rejected   invalid_genesis_rejected
```

A regressed node — no consensus, diverged ledgers, an applied double-spend, an
accepted skipped sequence — turns the bound gate RED. There is no `ooptdd-loop`
on PATH here; it lives in the sibling `ooptdd-loop` repo's venv.

## Disk hazard on the Mac

Because there is no shared workspace, **every crate builds its own `target/`.**
`substrate/target` alone is 1.8 GB and `333-transfer/target` 316 MB. Building all
33 crates on the Mac can exhaust the disk, and an ENOSPC on this machine kills
the shell outright rather than failing cleanly.

Check `df -h /System/Volumes/Data` before a broad build. Build only the crates
you are actually touching, or move the build to a machine with headroom.

## Definition of Done

A task is complete only when:

1. Tests were added or updated for the behavior you changed.
2. The touched crate's `cargo test --all-targets` exits 0, from a baseline you
   recorded before editing.
3. For `transfer333` behavior, the ten node-conformance gates are green against
   real processes — not against a mock, and not by relaxing an event literal.
4. No test, gate, event literal, or Longinus binding was weakened.
5. The final diff was reviewed for unrelated changes.

Adding a crate, test, or gate needs no approval. **Removing a gate, relaxing an
expected event, or introducing a root workspace does.**

## Coding Rules

- Expected failures are values. A rejected transfer is a verdict, not a panic.
- Do not silently catch or discard errors.
- Keep `p333-*` and `*333` lineages separate. One canonical representation
  *within* a lineage, not across them.
- Prefer an existing pattern over a new abstraction.
- Native tests cannot validate the wasm targets (`333-transfer-wasm`,
  `substrate/crates/relay-client-wasm`). A green native suite says nothing about
  them; build and exercise the wasm target explicitly or say you did not.

## Licensing

AGPL-3.0-only. Operators who modify the program and expose it over a network
must provide Corresponding Source for **the exact deployed revision** — a link
to a newer revision is not a substitute. Provenance for adapted AGPL work
(Garage CRDT, Puter kernel) is recorded in `THIRD_PARTY_NOTICES.md` and
`333-crdt/NOTICE`. Keep those notices intact when you touch derived source.

## Workflow

1. Read the nearest tests and the closest existing crate first.
2. Record the baseline: run the touched crate's suite before editing.
3. State the intended behavior before writing code.
4. Make the smallest coherent change.
5. Run the crate suite; for transfer333 behavior, run the node-conformance gates.
6. Report which commands you ran and what they printed. Do not report a green
   you did not observe.
