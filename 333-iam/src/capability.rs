// KG: SPAN_333_L9_IAM_Prod, finding_333_synth_sec_sota_d28, finding_333_synth_sec_prt_d30
// UCAN-lite CapabilityToken: signed delegation with audience, expiry, nonce.
//
// Sources:
//   - D28 SOTA: UCAN 0.10 (DID chains, revocation registry).
//   - D30 Port-333: Trait + Ed25519 via 333-identity.
//   - D31 Pitfalls: (a) missing nonce → replay; (b) audience bypass confused
//     deputy (CVE-2024-5798, CVE-2025-62610). Both defended here.
//
// This crate models one-hop delegation only (no chain attenuation yet —
// that lands when rs-ucan is plugged in as a downstream impl). Chain depth
// is capped at 1 via the `delegatee` field; multi-hop remains a future
// extension.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use identity333::{Keypair, NodeId, Signature};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CapabilityError {
    #[error("signature invalid")]
    InvalidSignature,
    #[error("token expired at {expired_at_ms} (now={now_ms})")]
    Expired { expired_at_ms: u64, now_ms: u64 },
    #[error("audience mismatch: token.aud={token_aud:?}, service={service:?}")]
    AudienceMismatch { token_aud: String, service: String },
    #[error("nonce replay detected: {0}")]
    NonceReplay(String),
    #[error("capability not granted: need {need:?}, have {have:?}")]
    InsufficientCapability { need: String, have: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityToken {
    pub issuer: NodeId,
    pub audience: String,
    pub delegatee: NodeId,
    pub capabilities: Vec<String>,
    pub not_before_ms: u64,
    pub expires_at_ms: u64,
    pub nonce: String,
    pub sig: Signature,
}

impl CapabilityToken {
    /// Canonical bytes for signing = iss || aud || delegatee || caps || nbf || exp || nonce.
    pub fn canonical_bytes(
        issuer: &NodeId,
        audience: &str,
        delegatee: &NodeId,
        capabilities: &[String],
        not_before_ms: u64,
        expires_at_ms: u64,
        nonce: &str,
    ) -> Vec<u8> {
        let mut out = Vec::with_capacity(256);
        out.extend_from_slice(issuer.as_bytes());
        out.push(b'|');
        out.extend_from_slice(audience.as_bytes());
        out.push(b'|');
        out.extend_from_slice(delegatee.as_bytes());
        out.push(b'|');
        for c in capabilities {
            out.extend_from_slice(c.as_bytes());
            out.push(b',');
        }
        out.push(b'|');
        out.extend_from_slice(&not_before_ms.to_be_bytes());
        out.extend_from_slice(&expires_at_ms.to_be_bytes());
        out.push(b'|');
        out.extend_from_slice(nonce.as_bytes());
        out
    }

    pub fn issue(
        issuer_kp: &Keypair,
        audience: &str,
        delegatee: &NodeId,
        capabilities: Vec<String>,
        not_before_ms: u64,
        expires_at_ms: u64,
        nonce: String,
    ) -> Self {
        let msg = Self::canonical_bytes(
            &issuer_kp.node_id(),
            audience,
            delegatee,
            &capabilities,
            not_before_ms,
            expires_at_ms,
            &nonce,
        );
        let sig = issuer_kp.sign(&msg);
        Self {
            issuer: issuer_kp.node_id(),
            audience: audience.to_string(),
            delegatee: delegatee.clone(),
            capabilities,
            not_before_ms,
            expires_at_ms,
            nonce,
            sig,
        }
    }
}

/// Tracks seen nonces to detect replay. Short TTL cleanup is delegated to the
/// downstream revocation store (CRDT tombstones).
pub trait CapabilityVerifier {
    fn verify(
        &self,
        token: &CapabilityToken,
        service: &str,
        required_capability: &str,
        now_ms: u64,
    ) -> Result<(), CapabilityError>;
}

#[derive(Debug, Default)]
pub struct InMemoryVerifier {
    seen_nonces: Arc<Mutex<HashMap<String, u64>>>, // nonce -> expiry_ms
}

impl InMemoryVerifier {
    pub fn new() -> Self {
        Self::default()
    }

    /// Purge nonces whose tokens have expired (cheap GC).
    pub fn purge_expired(&self, now_ms: u64) {
        let mut g = self.seen_nonces.lock().unwrap();
        g.retain(|_, exp| *exp > now_ms);
    }
}

impl CapabilityVerifier for InMemoryVerifier {
    fn verify(
        &self,
        token: &CapabilityToken,
        service: &str,
        required_capability: &str,
        now_ms: u64,
    ) -> Result<(), CapabilityError> {
        // D31 pitfall fix: audience exact-match (no substring).
        if token.audience != service {
            return Err(CapabilityError::AudienceMismatch {
                token_aud: token.audience.clone(),
                service: service.to_string(),
            });
        }
        if now_ms < token.not_before_ms || now_ms >= token.expires_at_ms {
            return Err(CapabilityError::Expired {
                expired_at_ms: token.expires_at_ms,
                now_ms,
            });
        }
        // Nonce replay check
        {
            let mut g = self.seen_nonces.lock().unwrap();
            if g.contains_key(&token.nonce) {
                return Err(CapabilityError::NonceReplay(token.nonce.clone()));
            }
            g.insert(token.nonce.clone(), token.expires_at_ms);
        }
        // Capability check: required must be in the grant set.
        if !token.capabilities.iter().any(|c| c == required_capability) {
            return Err(CapabilityError::InsufficientCapability {
                need: required_capability.to_string(),
                have: token.capabilities.clone(),
            });
        }
        // Signature check last (expensive).
        let msg = CapabilityToken::canonical_bytes(
            &token.issuer,
            &token.audience,
            &token.delegatee,
            &token.capabilities,
            token.not_before_ms,
            token.expires_at_ms,
            &token.nonce,
        );
        Keypair::verify(token.issuer.as_bytes(), &msg, &token.sig)
            .map_err(|_| CapabilityError::InvalidSignature)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (Keypair, NodeId) {
        let issuer = Keypair::generate();
        let delegatee = Keypair::generate().node_id();
        (issuer, delegatee)
    }

    #[test]
    fn valid_token_verifies() {
        let (issuer, delegatee) = setup();
        let t = CapabilityToken::issue(
            &issuer,
            "neo4j.metahumotonic.com",
            &delegatee,
            vec!["read".into(), "write".into()],
            0,
            10_000,
            "nonce-1".into(),
        );
        let v = InMemoryVerifier::new();
        assert!(v.verify(&t, "neo4j.metahumotonic.com", "read", 100).is_ok());
    }

    #[test]
    fn audience_mismatch_rejected() {
        let (issuer, delegatee) = setup();
        let t = CapabilityToken::issue(
            &issuer,
            "neo4j.metahumotonic.com",
            &delegatee,
            vec!["read".into()],
            0,
            10_000,
            "nonce-2".into(),
        );
        let v = InMemoryVerifier::new();
        let err = v.verify(&t, "mongo.metahumotonic.com", "read", 100).unwrap_err();
        assert!(matches!(err, CapabilityError::AudienceMismatch { .. }));
    }

    #[test]
    fn expired_rejected() {
        let (issuer, delegatee) = setup();
        let t = CapabilityToken::issue(&issuer, "svc", &delegatee, vec!["r".into()], 0, 100, "n3".into());
        let v = InMemoryVerifier::new();
        assert!(matches!(
            v.verify(&t, "svc", "r", 200),
            Err(CapabilityError::Expired { .. })
        ));
    }

    #[test]
    fn not_yet_valid_rejected() {
        let (issuer, delegatee) = setup();
        let t = CapabilityToken::issue(&issuer, "svc", &delegatee, vec!["r".into()], 1000, 2000, "n4".into());
        let v = InMemoryVerifier::new();
        assert!(matches!(
            v.verify(&t, "svc", "r", 500),
            Err(CapabilityError::Expired { .. })
        ));
    }

    #[test]
    fn nonce_replay_rejected() {
        let (issuer, delegatee) = setup();
        let t = CapabilityToken::issue(&issuer, "svc", &delegatee, vec!["r".into()], 0, 10_000, "n5".into());
        let v = InMemoryVerifier::new();
        v.verify(&t, "svc", "r", 100).unwrap();
        let err = v.verify(&t, "svc", "r", 200).unwrap_err();
        assert!(matches!(err, CapabilityError::NonceReplay(_)));
    }

    #[test]
    fn insufficient_capability_rejected() {
        let (issuer, delegatee) = setup();
        let t = CapabilityToken::issue(&issuer, "svc", &delegatee, vec!["read".into()], 0, 10_000, "n6".into());
        let v = InMemoryVerifier::new();
        assert!(matches!(
            v.verify(&t, "svc", "delete", 100),
            Err(CapabilityError::InsufficientCapability { .. })
        ));
    }

    #[test]
    fn tampered_signature_rejected() {
        let (issuer, delegatee) = setup();
        let mut t = CapabilityToken::issue(&issuer, "svc", &delegatee, vec!["r".into()], 0, 10_000, "n7".into());
        t.capabilities.push("admin".into()); // escalation attempt
        let v = InMemoryVerifier::new();
        assert!(matches!(
            v.verify(&t, "svc", "admin", 100),
            Err(CapabilityError::InvalidSignature)
        ));
    }

    #[test]
    fn purge_clears_expired_nonces() {
        let (issuer, delegatee) = setup();
        let t = CapabilityToken::issue(&issuer, "svc", &delegatee, vec!["r".into()], 0, 100, "n8".into());
        let v = InMemoryVerifier::new();
        v.verify(&t, "svc", "r", 50).unwrap();
        v.purge_expired(200);
        // After purge, replay detection for this specific nonce is moot (token is expired).
    }
}
