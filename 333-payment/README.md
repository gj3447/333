# payment333

`payment333` is the canonical 333/ORRR payment boundary. It keeps ordinary
single-owner transfers off the global consensus path while forcing multi-writer
marketplace state through a sequential Byzantine quorum certificate.

## Two proof-typed lanes, one supply

1. **Fast lane:** an Ed25519 owner signs a context-bound transfer order. Each
   authority verifies owner/account binding, positive amount, expiry, sequence
   and its local durable balance, then persists its per-account lock before
   returning a vote. An `n-f` quorum forms a `TransferCertificate`.
2. **Control lane:** every validator locally executes an escrow/bid/dispute/
   reward/rotation block. Only `PaymentLedger::validate_control_block` can mint
   the type accepted by `Authority::vote_control`. Authorities persist one
   digest per control height before voting; an `n-f` quorum forms a
   `ControlCertificate` and the ledger enforces the parent/result state roots.
3. **Bridge:** a job consumes exactly one already-applied FastPay certificate
   whose recipient is the protocol escrow vault. Resolution creates payout
   vouchers tied to that deposit. Each voucher moves value from a protocol-owned
   account exactly once. Rewards use a pre-funded reserve rather than minting.

The fast and control state roots are deliberately separate: unrelated FastPay
traffic cannot invalidate a control proposal. They still share one balance map,
one durable ledger file and one total-supply invariant.

## Durable mode

Use `PaymentLedger::create/open` and `Authority::create/open`. State files are:

- protected by a lifetime exclusive writer lock;
- serialized deterministically with a schema version;
- checksum-verified on load;
- written to a same-directory temporary file, `fsync`ed, atomically renamed,
  then followed by a parent-directory `fsync`.

An authority writes a transfer/control lock before producing the signature.
After a crash, the same order is idempotently re-signed and a conflicting order
is rejected.

## Verification

```sh
cargo test --manifest-path 333-payment/Cargo.toml
```

The adversarial suite is in `tests/payment_safety.rs`. See
`PROM_4_PAYMENT_SAFETY_2026-07-15.md` for the research programme, falsifiers and
scope boundary.
