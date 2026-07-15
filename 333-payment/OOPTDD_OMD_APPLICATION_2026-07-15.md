# OOPTDD + OMD application — payment333 PROM 4

This increment applies `OOPTDD_methodology_v1` and the real OMD coordinator to
`LakatosTree_333PaymentSafety_20260715`.

- OOPTDD decides whether payment behavior is GREEN, source-bound and
  methodology-valid.
- OMD controls the permitted write-set and the fenced path to integration.

## OOPTDD role/protocol boundary

| Role | Responsibility | Protocol/message |
|---|---|---|
| `PaymentOwner` | authorize one account-sequenced debit | `OwnerSignedTransferOrder` |
| `PaymentAuthority` | validate and durably lock one vote slot | `DurableAuthorityVote` |
| `PaymentCommittee` | form an epoch/context-bound quorum proof | `EpochBoundCertificate` |
| `FastPaymentLane` | settle independent owner-authorized transfers | applied transfer certificate |
| `ControlPaymentLane` | order escrow, dispute, reward and rotation state | control certificate |
| `ORRREscrowBridge` | consume one deposit and create one-shot vouchers | `AppliedEscrowFundingProof` |

Message contracts and state invariants are separate in
`ooptdd/payment_requirements.yaml`. Seven real Rust integration tests back the
requirements. A positive event requires exact libtest registration and a zero
subprocess exit; stdout is diagnostic only. Every requirement binds through
Longinus to `payment_conformance_adapter.py::run_payment_probe`, and all 14
canonical OOPTDD rules are enforced.

The first local run was correctly RED because local Cargo could not read lockfile
format v4. The final run delegates only the Rust subprocess to a verified remote
worktree at the same source commit; OOPTDD's memory store, CID, rules and
Longinus verification remain in the OMD worktree. Host/root/revision are in the
receipt.

## OMD execution boundary and observed defect

OMD source revision: `b124bfa306fd5626b336dde5d0f7a05e1c272962`.
Task: `prom4-payment-ooptdd-omd`; integration branch:
`codex/prom-333-safety`; `strict_writeset=true`.

Required transition:

```text
PENDING -> READY -> CLAIMED -> IN_ORBIT -> COMMITTED -> DONE -> MERGED
```

Two fail-loud observations were retained:

1. `PENDING -> IN_ORBIT` was rejected until `next_task` produced `READY`.
2. the default 90-second agent TTL expired during a legitimate long-running test;
   OMD retired the agent and force-removed its dirty worktree, losing uncommitted
   files. The task had to be reconstructed under a non-expiring agent lease.

The second behavior is OMD's current highest-severity operational defect: lease
expiry must quarantine or checkpoint a dirty worktree, never silently destroy
it. OMD also remains safety-oriented rather than throughput-optimal: task choice
is priority/FIFO plus overlap avoidance, SQLite has one writer, and the repo-wide
merge token serializes all connect operations. No committed scale benchmark
currently proves linear multi-agent speedup.
