# PROM 4 — 333/ORRR payment safety on a Lakatos tree

Date: 2026-07-15 (Asia/Seoul)  
Cycle: `prom4-333-payment-safety-2026-07-15`  
Tree: `LakatosTree_333PaymentSafety_20260715`  
Branch: `codex/prom-333-safety`  
Scope: logical, cryptographic and distributed-systems defects only. Regulation
and product policy are explicitly outside this cycle.

## Question and local counterexamples

The reviewed design had four coupled defects:

1. `transfer333::Transfer` named a string sender but carried no owner public key
   or owner signature. A quorum certified authority observations, not the
   account owner's authorization.
2. authority `locked` and `next_expected` maps lived only in memory. Restart
   erased the signing lock and allowed the same authority key to vote for a
   conflicting `(account, sequence)` value.
3. signed bytes omitted protocol version, network, asset, genesis, committee id,
   committee epoch and expiry. The same order/certificate could cross a trust
   domain or survive an undefined committee rotation.
4. `transfer333::Ledger` and `token333::InMemoryLedger` were independently
   writable monetary states. ORRR escrow, bids, disputes and reward epochs are
   multi-writer state; routing them through a bare FastPay account sequence is
   unsound, while routing every routine payout through BFT destroys the intended
   concurrency.

## External evidence axes

### A. Owner authentication and authority account state

FastPay defines an account address as a cryptographic hash of its public
verification key. Its transfer order includes the sender address, recipient,
amount, sequence and a sender signature. Authorities persist the public key,
balance, next sequence and pending signed order, and check the owner signature,
positive amount, exact sequence and sufficient balance before signing.

Primary source: Baudet, Danezis, Sonnino, *FastPay: High-Performance Byzantine
Fault Tolerant Settlement*, §§3.2–4.1.
<https://sonnino.com/papers/fastpay.pdf>

### B. Crash safety

FastPay explicitly says authority/account information is persisted. Tendermint's
production consensus package exposes a write-and-sync WAL and replay support;
DiemBFT isolates signing in a safety module that protects monotonically
increasing vote/QC state. The common rule is: a safety decision must become
durable before its signature is released.

Primary sources:

- <https://pkg.go.dev/github.com/tendermint/tendermint@v0.35.9/internal/consensus>
- <https://developers.diem.com/papers/diem-consensus-state-machine-replication-in-the-diem-blockchain/2021-08-17.pdf>

### C. Replay domain and committee context

Cosmos `SignDoc` binds `chain_id`, account number and sequence to the signed
transaction; its documentation identifies chain id as cross-chain replay
protection. For 333 the analogous domain must be stronger because a single wire
format may serve multiple networks and assets: protocol version + network id +
asset id + genesis hash + committee epoch + roster-derived committee id +
per-account sequence + control-height expiry.

Primary source:
<https://docs.cosmos.network/sdk/latest/learn/concepts/encoding>

### D. Marketplace state is not a routine payout

Akash's compute-marketplace lifecycle orders deployments, bids, leases and
escrow transitions; its escrow module locks tenant funds, pays providers and
refunds the remainder. This is shared state whose transitions need one agreed
order. It is categorically different from a single owner's independent payout.

Primary sources:

- <https://akash.network/docs/node-operators/architecture/application-layer/>
- <https://akash.network/docs/learn/core-concepts/deployments/>

## Lakatos research programme

### Hard core (protected)

`HC-333-PAYMENT-1`: value has one conservation invariant. Routine debits are
authorized by exactly one public-key-bound owner and need only per-owner order;
multi-writer marketplace transitions require Byzantine total order. No unsigned
or caller-asserted proof may cross either mutation boundary.

Changing this statement would create a different programme. Performance,
serialization format, storage backend, committee size and marketplace operation
set belong to the protective belt.

### Protective-belt moves

| Belt move | Defect closed | Mechanism |
|---|---:|---|
| `B1 OwnerSignedOrder` | 1 | SHA-256 account binding + Ed25519 owner signature |
| `B2 DurableSafety` | 2 | exclusive writer lock + checksum + atomic rename + file and directory `fsync` before vote |
| `B3 PaymentContext` | 3 | version/network/asset/genesis/epoch/roster id and expiry in both owner and authority sign bytes |
| `B4 RotationDrain` | 3 | successor epoch must be exactly `e+1`; authority refuses rotation with pending transfer locks; old epoch cannot authorize a new fast apply |
| `B5 CertifiedDepositVoucherBridge` | 4 | fast certificate deposits into keyless escrow; BFT job consumes it once; reserve-backed voucher redeems once |
| `B6 SplitCommitment` | 4 | BFT root excludes commuting fast balances but the durable ledger and total supply remain single |

`B6` is essential. Hashing all FastPay balances into every BFT parent would make
unrelated fast transfers invalidate proposals and silently recreate the global
ordering bottleneck the programme is meant to avoid.

### Pre-registered predictions and falsifiers

| Prediction | Test/falsifier | Result |
|---|---|---|
| P1 owner authenticity | modified amount or mismatched sender/key obtains an authority vote | FALSIFIER NOT OBSERVED |
| P2 restart safety | after reopening the same key/state file, a conflicting transfer or control block at the locked slot obtains a vote | FALSIFIER NOT OBSERVED |
| P3 domain/epoch isolation | certificate from another network or retired epoch mutates the current fast ledger | FALSIFIER NOT OBSERVED |
| P4 rail separation | a zero-vote control block, duplicate deposit, duplicate reward epoch or duplicate voucher changes balances/state | FALSIFIER NOT OBSERVED |
| P5 conservation | any tested interleaving of deposit, dispute, payout, refund, reward and routine transfer changes total supply | FALSIFIER NOT OBSERVED |
| P6 rotation liveness boundary | settled pre-rotation escrow becomes unusable after rotation | FALSIFIER NOT OBSERVED; historical applied proof remains consumable |

Verification command on DGX:

```text
cargo test --manifest-path /Users/lagyeongjun/CD/worktrees/333-prom-safety/333-payment/Cargo.toml
7 passed; 0 failed
```

Regression gates on the same DGX worktree also passed: `transfer333` 65 passed
(1 multi-process smoke ignored), `token333` 17 passed, and `incentive333` 16
passed. The payment suite also passed in optimized `--release` mode.

Relevant executable evidence:

- `333-payment/tests/payment_safety.rs`
- `333-payment/src/types.rs`
- `333-payment/src/authority.rs`
- `333-payment/src/control.rs`
- `333-payment/src/ledger.rs`
- `333-payment/src/storage.rs`

## OOPTDD and OMD operationalization

The six PROM predictions refine into seven OOPTDD requirements (restart signing
locks and durable ledger reopen are separate). `OOPTDD_methodology_v1` enforces
structured positive evidence, correlation IDs, Longinus bindings, separate
message/state contracts and real integration backstops. GREEN events require an
exact registered Rust test with zero exit status; test text is not an oracle.

Implementation work uses OMD revision `b124bfa306fd5626b336dde5d0f7a05e1c272962`,
an explicit HELD write-set lease, `strict_writeset=true`, a dedicated worktree and
the coordinator's connect fence.

Evidence:

- `ooptdd/payment_requirements.yaml`
- `ooptdd/payment_conformance_adapter.py`
- `333-payment/OOPTDD_RECEIPT_PROM4_PAYMENT_2026-07-15.json`
- `333-payment/OMD_WORK_UNIT_PROM4_PAYMENT_METHODS.yaml`
- `333-payment/OMD_EXECUTION_RECEIPT_PROM4_PAYMENT_2026-07-15.json`
- `333-payment/OOPTDD_OMD_APPLICATION_2026-07-15.md`

## Appraisal

Verdict: **PROGRESSIVE_CORE**.

The four reviewed defects now have an executable countermeasure and a direct
falsifier. The programme gained excess content beyond patching the original
bugs: the split control commitment predicts and prevents a new global-bottleneck
failure; the applied-certificate binding prevents a lagging replica from
redeeming one job's voucher out of another job's aggregate escrow balance; and
rotation drains pending signatures without stranding already-settled deposits.

## Honest residual boundary

The crate implements the safety core and proof-verification boundary of the BFT
control plane: deterministic execution, sequential height/parent/result roots,
durable one-value-per-height authority locks, `n-f` signed finality certificate,
and committee rotation. It intentionally does **not** implement a network
pacemaker/view-change protocol. A Byzantine proposer can split honest locks and
halt a height; it cannot create two committed values. Production liveness must
connect this boundary to a reviewed HotStuff/Tendermint/Malachite-style engine
without weakening `ValidatedControlBlock` or the durable signing rule.

TLS/peer transport, key custody/HSM, backup/restore orchestration, performance
benchmarks and cross-language canonical test vectors are separate engineering
cycles. They do not reopen defects 1–4, but they remain release gates.
