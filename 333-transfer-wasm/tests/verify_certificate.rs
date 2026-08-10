// KG: browser-gap-shallow-3-blockers-2026-07-15
//
// Native tests of the browser verification logic. The wasm-bindgen surface is a
// thin wrapper over `transfer333`; these exercise the wrapper against the real
// library on native (fast, no headless browser) and, critically, the negative
// cases — a verifier that accepts a forged certificate is worse than none.
//
// The wasm32 build itself is covered by `cargo build --target wasm32-unknown-unknown`
// in the crate's own CI step; behaviour is identical because the code path is.

use ed25519_dalek::SigningKey;
use transfer333::{
    encode_certificate, Authority, Committee, Ledger, NetworkId, OwnerRegistry, SignedTransfer,
    Transfer, TransferPolicy,
};
use transfer333_wasm::{
    browser_owner_pubkey_hex, config_fingerprint_core, verify_certificate_core,
};

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

const NET: &str = "rt2-testnet";
const AUTH_IDS: [&str; 3] = ["a0", "a1", "a2"];

/// The honest world: a payer, three authorities, and a browser payee whose key is
/// generated in-page.
struct World {
    policy: TransferPolicy,
    committee: Committee,
    authorities: Vec<Authority>,
    config_json: String,
}

fn setup() -> World {
    // Browser owner pubkey, derived exactly as the wasm BrowserOwner would.
    let browser_pub = browser_owner_pubkey_hex(&[7u8; 32]);
    let browser_key = {
        let raw: Vec<u8> = (0..browser_pub.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&browser_pub[i..i + 2], 16).unwrap())
            .collect();
        ed25519_dalek::VerifyingKey::from_bytes(&raw.try_into().unwrap()).unwrap()
    };
    let owners = vec![
        ("treasury".to_string(), key(42).verifying_key()),
        ("browser-payee".to_string(), browser_key),
    ];
    let policy = TransferPolicy::new(
        NetworkId::new(NET).unwrap(),
        OwnerRegistry::new(owners.clone()).unwrap(),
    );
    let committee = Committee::new(
        AUTH_IDS.iter().map(|id| (id.to_string(), key_for(id))),
        policy.clone(),
    )
    .unwrap();
    let authorities = AUTH_IDS
        .iter()
        .map(|id| {
            Authority::new(
                id.to_string(),
                signer_for(id),
                policy.clone(),
                committee.id(),
                Ledger::genesis([
                    ("treasury".to_string(), 333u128),
                    ("browser-payee".to_string(), 0),
                ]),
            )
        })
        .collect();

    let owners_json = owners
        .iter()
        .map(|(a, k)| format!(r#""{a}":"{}""#, hex(k.as_bytes())))
        .collect::<Vec<_>>()
        .join(",");
    let auth_json = AUTH_IDS
        .iter()
        .map(|id| format!(r#""{id}":"{}""#, hex(signer_for(id).verifying_key().as_bytes())))
        .collect::<Vec<_>>()
        .join(",");
    let config_json = format!(
        r#"{{"network_id":"{NET}","owners":{{{owners_json}}},"authorities":{{{auth_json}}}}}"#
    );

    World {
        policy,
        committee,
        authorities,
        config_json,
    }
}

fn signer_for(id: &str) -> SigningKey {
    match id {
        "a0" => key(1),
        "a1" => key(2),
        "a2" => key(3),
        _ => unreachable!(),
    }
}
fn key_for(id: &str) -> ed25519_dalek::VerifyingKey {
    signer_for(id).verifying_key()
}

/// Treasury pays the browser; produce the quorum certificate bytes the browser
/// would receive.
fn pay_browser(w: &mut World) -> Vec<u8> {
    let order = SignedTransfer::sign(
        &w.policy,
        Transfer {
            from: "treasury".into(),
            from_seq: 0,
            to: "browser-payee".into(),
            amount: 1,
        },
        &key(42),
    );
    // Encode the quorum certificate the payer would ship to the browser.
    encode_cert_for(&mut w.authorities, &w.committee, &order)
}

/// Build and encode the certificate the payer would ship.
fn encode_cert_for(authorities: &mut [Authority], committee: &Committee, order: &SignedTransfer) -> Vec<u8> {
    let mut votes = Vec::new();
    for a in authorities.iter_mut() {
        if let Ok(v) = a.handle(order) {
            votes.push(v);
        }
    }
    let cert = transfer333::Certificate::assemble(order.clone(), votes, committee)
        .expect("assemble");
    encode_certificate(&cert)
}

#[test]
fn browser_verifies_an_honest_certificate() {
    let mut w = setup();
    let cert = pay_browser(&mut w);
    let json = verify_certificate_core(&w.config_json, &cert).expect("verify");
    assert!(json.contains(r#""verified":true"#));
    assert!(json.contains(r#""to":"browser-payee""#));
    assert!(json.contains(r#""amount":"1""#));
    assert!(json.contains(r#""quorum":3"#));
}

/// The attack the whole thing exists to stop: a certificate signed by a committee
/// the browser did NOT pin must be rejected, even though it is internally valid.
#[test]
fn browser_rejects_a_certificate_from_an_unpinned_committee() {
    let mut w = setup();
    // A rogue coordinator builds its own committee of keys it controls and signs
    // an internally-valid payment (amount within treasury). It is a real quorum
    // certificate — just not from the committee the browser pinned.
    let rogue_ids = ["r0", "r1", "r2"];
    let rogue_signers = |id: &str| match id {
        "r0" => key(90),
        "r1" => key(91),
        "r2" => key(92),
        _ => unreachable!(),
    };
    let rogue_committee = Committee::new(
        rogue_ids.iter().map(|id| (id.to_string(), rogue_signers(id).verifying_key())),
        w.policy.clone(),
    )
    .unwrap();
    let mut rogue_auths: Vec<Authority> = rogue_ids
        .iter()
        .map(|id| {
            Authority::new(
                id.to_string(),
                rogue_signers(id),
                w.policy.clone(),
                rogue_committee.id(),
                Ledger::genesis([("treasury".to_string(), 333u128), ("browser-payee".to_string(), 0)]),
            )
        })
        .collect();
    let order = SignedTransfer::sign(
        &w.policy,
        Transfer { from: "treasury".into(), from_seq: 0, to: "browser-payee".into(), amount: 1 },
        &key(42),
    );
    let rogue_cert = encode_cert_for(&mut rogue_auths, &rogue_committee, &order);

    // The browser verifies against the committee it PINNED, not the rogue one.
    let result = verify_certificate_core(&w.config_json, &rogue_cert);
    assert!(
        result.is_err(),
        "a certificate from an unpinned committee must be rejected: {result:?}"
    );
}

#[test]
fn browser_rejects_a_tampered_certificate() {
    let mut w = setup();
    let mut cert = pay_browser(&mut w);
    // Flip a byte in the cert body.
    let n = cert.len();
    cert[n / 2] ^= 0xFF;
    assert!(
        verify_certificate_core(&w.config_json, &cert).is_err(),
        "a tampered certificate must not verify"
    );
}

#[test]
fn config_fingerprint_is_stable_and_specific() {
    let w = setup();
    let fp1 = config_fingerprint_core(&w.config_json).unwrap();
    let fp2 = config_fingerprint_core(&w.config_json).unwrap();
    assert_eq!(fp1, fp2, "same config -> same fingerprint");
    assert_eq!(fp1.len(), 64, "sha-256 hex");

    // A different authority set must fingerprint differently.
    let tweaked = w.config_json.replace(
        &hex(signer_for("a0").verifying_key().as_bytes()),
        &hex(key(200).verifying_key().as_bytes()),
    );
    assert_ne!(
        config_fingerprint_core(&tweaked).unwrap(),
        fp1,
        "a different roster must fingerprint differently"
    );
}

#[test]
fn browser_owner_key_is_deterministic_from_seed() {
    let k1 = browser_owner_pubkey_hex(&[9u8; 32]);
    let k2 = browser_owner_pubkey_hex(&[9u8; 32]);
    assert_eq!(k1, k2, "seed determines key");
    assert_eq!(k1.len(), 64);
    let k3 = browser_owner_pubkey_hex(&[10u8; 32]);
    assert_ne!(k1, k3, "different seed -> different key");
    // The private key never has a getter on BrowserOwner — a compile-time
    // property, so there is nothing to assert at runtime beyond the pubkey.
}

#[test]
fn malformed_config_is_rejected_not_defaulted() {
    let cert = {
        let mut w = setup();
        pay_browser(&mut w)
    };
    assert!(verify_certificate_core("not json", &cert).is_err());
    assert!(verify_certificate_core(r#"{"network_id":"x"}"#, &cert).is_err(), "missing owners");
    assert!(
        verify_certificate_core(r#"{"network_id":"x","owners":{},"authorities":{}}"#, &cert).is_err(),
        "empty rosters must be rejected, not treated as a vacuous pass"
    );
}
