# 333 Compute Exchange — consent-first kernel alpha

This standalone crate is the smallest deterministic domain kernel for a future
333 compute exchange. It makes one promise: a browser resource contributor is
never turned into a worker merely because a page or an agent visited a URL.
Work can start only after an explicit, bounded `ResourceGrant`, a matching
`ResourceLease`, an offered assignment, and an accepted `StartAttempt` command.

The kernel is synchronous, `std`-only, and has no I/O or ambient clock. Its
public shape is:

```text
handle(aggregate, idempotent command) -> typed events/effects | typed rejection
```

The caller records time with `ObserveTime`. External work is represented only
as an effect request. The kernel never installs software, opens a network
connection, runs WebAssembly, or transfers value itself.

## Alpha v1 policy

- explicit consent; page access is not consent;
- browser foreground execution only;
- at most 4 workers in a grant, at most 50% duty cycle, and at most 512 MiB;
- no network bytes and no GPU use;
- one outstanding assignment per contributor aggregate;
- revocation or observed expiry emits `StopSandbox` and blocks new work;
- a submitted result must be accepted by a separately capable verifier before
  a service-credit posting effect can be prepared;
- identical idempotency-key replays return the recorded decision without
  re-dispatching effects; the same key with different intent is a conflict.

The limits are product policy injected through `EnginePolicy`; the state
transition mechanism remains deterministic. `EnginePolicy::browser_alpha_v1()`
is the only policy implemented and tested here.

## What this does **not** prove

This is an in-memory alpha kernel, not a compute network. It does **not** claim:

- P2P discovery, transport, scheduling, browser background execution, or PWA
  installation;
- a real WASM/WebGPU sandbox, hard host quotas, workload-code signing, or result
  correctness;
- durable journaling, an atomic inbox/outbox, crash recovery, multi-writer
  concurrency, or cross-process exactly-once effects;
- validation of arbitrary replay input passed directly to public `evolve`; that
  reducer assumes facts were already accepted by `handle`, so a production
  replay adapter must validate schema, ordering, generation, and integrity and
  quarantine invalid history before evolution;
- cryptographic capability verification, identity binding, payment, token
  issuance, accounting finality, legal compliance, or production readiness.

`VerifierCapability` is a recorded, typed input. A production adapter must
authenticate and verify its signature before passing it to this kernel.
`PostServiceCredit` is an effect intent for non-transferable service credits,
not proof that credits were posted and not a payment.

## Planned reuse of existing 333 components

No existing crate is linked yet, so this slice cannot silently inherit claims
from adjacent prototypes. The intended adapter plan is:

1. use `identity333` plus `iam333` to authenticate principals and verify scoped,
   expiring verifier capabilities;
2. implement `StartSandbox`/`StopSandbox` with a hardened adapter behind
   `hypervisor333`, after host-enforced CPU, memory, cancellation, and workload
   signature checks exist;
3. feed measured usage into `substrate/crates/metering` and represent posting
   receipts through a durable accounting adapter derived from
   `substrate/crates/billing`;
4. add a transactional inbox, event journal, and outbox before any network or
   commercial pilot;
5. add transport/discovery only after the local consent, sandbox, verification,
   revocation, and crash-recovery seams are independently proven.

This ordering deliberately keeps consent and verified-result-before-settlement
inside the trusted kernel while leaving transport, UI, pricing, and marketplace
policy outside it.

## Verify

```bash
cargo fmt --check
cargo test
```
