// KG: assess-333-engine-fsm-2026-07-13 (action 9), prom16-333-consensusless-frontier Q1
//
// End-to-end integration test of the Byzantine-safe certified rail through the
// PUBLIC API only (the assessment noted no such e2e test existed): an owner's
// transfer is certified by an independent authority committee and applied to the
// ledger ONLY via a `Verified` minted by quorum-certificate verification.

use transfer333::{certify, Authority, Certified, Committee, Ledger, SigningKey, Transfer};

fn t(from: &str, seq: u64, to: &str, amount: u128) -> Transfer {
    Transfer { from: from.into(), from_seq: seq, to: to.into(), amount }
}

fn setup() -> (Committee, Vec<Authority>) {
    // n=4, quorum=3. Each authority holds a seeded Ed25519 secret key; the
    // committee binds each id to the matching public key.
    let authorities: Vec<Authority> = (0..4u8)
        .map(|i| Authority::new(format!("a{i}"), SigningKey::from_bytes(&[i; 32])))
        .collect();
    let committee =
        Committee::new(authorities.iter().map(|a| (a.id().clone(), a.verifying_key()))).unwrap();
    (committee, authorities)
}

#[test]
fn end_to_end_certified_transfer_applies_only_via_verified() {
    let (committee, mut authorities) = setup();
    let mut ledger = Ledger::genesis([("alice".to_string(), 100u128), ("bob".to_string(), 0u128)]);

    let (verified, status) = certify(&t("alice", 0, "bob", 30), &mut authorities, &committee);
    assert_eq!(status, Certified::Ok);
    let verified = verified.expect("quorum reached -> a Verified is minted");

    ledger.apply_verified(&verified).unwrap();
    assert_eq!(ledger.balance(&"bob".to_string()), 30);
    assert_eq!(ledger.balance(&"alice".to_string()), 70);
    assert_eq!(ledger.total_supply(), 100); // conservation
}

#[test]
fn end_to_end_byzantine_equivocation_cannot_double_apply() {
    let (committee, mut authorities) = setup();
    let mut ledger = Ledger::genesis([
        ("alice".to_string(), 100u128),
        ("bob".to_string(), 0u128),
        ("carol".to_string(), 0u128),
    ]);

    // Owner alice's honest seq-0 transfer certifies (all 4 lock it, then confirm).
    let (v1, s1) = certify(&t("alice", 0, "bob", 100), &mut authorities, &committee);
    assert_eq!(s1, Certified::Ok);
    ledger.apply_verified(&v1.unwrap()).unwrap();

    // A Byzantine attempt to reuse seq 0 for a different recipient: authorities
    // have advanced next-expected past 0, so every one refuses (OutOfOrder) and
    // no certificate — hence no Verified — can ever be minted. carol gets nothing.
    let (v2, s2) = certify(&t("alice", 0, "carol", 100), &mut authorities, &committee);
    assert!(v2.is_none(), "no Verified can be minted for the double-spend");
    assert!(matches!(s2, Certified::Failed { .. }));

    assert_eq!(ledger.balance(&"bob".to_string()), 100);
    assert_eq!(ledger.balance(&"carol".to_string()), 0);
    assert_eq!(ledger.total_supply(), 100); // alice spent her 100 exactly once
}

#[test]
fn sequential_transfers_certify_in_order_only() {
    let (committee, mut authorities) = setup();
    let mut ledger = Ledger::genesis([("alice".to_string(), 100u128), ("bob".to_string(), 0u128)]);

    // seq 0 then seq 1 both certify and apply.
    for (seq, amt) in [(0u64, 40u128), (1, 25)] {
        let (v, s) = certify(&t("alice", seq, "bob", amt), &mut authorities, &committee);
        assert_eq!(s, Certified::Ok, "seq {seq}");
        ledger.apply_verified(&v.unwrap()).unwrap();
    }
    assert_eq!(ledger.balance(&"bob".to_string()), 65);

    // Skipping to seq 3 (seq 2 not yet certified) is refused by every authority.
    let (v3, s3) = certify(&t("alice", 3, "bob", 5), &mut authorities, &committee);
    assert!(v3.is_none());
    assert!(matches!(s3, Certified::Failed { contested: false, .. }));
}
