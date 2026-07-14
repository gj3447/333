// KG: TASK_ATOM_P4OS_UCAN
// KG: CONTRACT_ATOM_P4OS_UCAN_capability_check
// KG: SPAN_333_Phase4_P2P_OS_MVP
//
// UCAN capability verification: stateless pure fn.
// Given a token + requested capability + wall clock, decide Granted/No or Err.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub type Did = String;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Capability {
    pub resource: String,
    pub action: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UcanPayload {
    pub iss: Did,              // issuer DID (contains pubkey)
    pub aud: Did,              // audience DID
    pub exp: u64,              // unix epoch seconds
    pub cap: Vec<Capability>,
}

#[derive(Debug, Clone)]
pub struct UcanToken {
    pub payload: UcanPayload,
    pub payload_bytes: Vec<u8>,
    pub signature: [u8; 64],
    pub issuer_pubkey: [u8; 32],
}

#[derive(Debug, PartialEq, Eq)]
pub enum Granted {
    Yes,
    No,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AuthError {
    #[error("token expired")]
    Expired,
    #[error("invalid signature")]
    InvalidSig,
    #[error("capability not granted by token")]
    CapNotGranted,
    #[error("malformed token: {0}")]
    MalformedToken(String),
}

/// Parse a compact JWT-shaped UCAN: `<b64 payload>.<b64 sig>.<b64 pubkey>`.
/// For MVP we use a simplified 3-segment format rather than full JWT header.
pub fn parse(jwt: &str) -> Result<UcanToken, AuthError> {
    let parts: Vec<&str> = jwt.split('.').collect();
    if parts.len() != 3 {
        return Err(AuthError::MalformedToken(format!(
            "expected 3 segments, got {}",
            parts.len()
        )));
    }
    let payload_bytes = URL_SAFE_NO_PAD
        .decode(parts[0])
        .map_err(|e| AuthError::MalformedToken(format!("payload b64: {}", e)))?;
    let sig_bytes = URL_SAFE_NO_PAD
        .decode(parts[1])
        .map_err(|e| AuthError::MalformedToken(format!("sig b64: {}", e)))?;
    let pubkey_bytes = URL_SAFE_NO_PAD
        .decode(parts[2])
        .map_err(|e| AuthError::MalformedToken(format!("pubkey b64: {}", e)))?;

    if sig_bytes.len() != 64 {
        return Err(AuthError::MalformedToken("sig must be 64 bytes".into()));
    }
    if pubkey_bytes.len() != 32 {
        return Err(AuthError::MalformedToken("pubkey must be 32 bytes".into()));
    }

    let payload: UcanPayload = serde_json::from_slice(&payload_bytes)
        .map_err(|e| AuthError::MalformedToken(format!("payload json: {}", e)))?;

    let mut signature = [0u8; 64];
    signature.copy_from_slice(&sig_bytes);
    let mut issuer_pubkey = [0u8; 32];
    issuer_pubkey.copy_from_slice(&pubkey_bytes);

    Ok(UcanToken {
        payload,
        payload_bytes,
        signature,
        issuer_pubkey,
    })
}

/// Verify token signature + exp + capability.
/// `now` is unix epoch seconds.
pub fn check(
    token: &UcanToken,
    requested_cap: &Capability,
    now: u64,
) -> Result<Granted, AuthError> {
    if requested_cap.resource.is_empty() || requested_cap.action.is_empty() {
        return Err(AuthError::MalformedToken("cap must have resource+action".into()));
    }
    if now >= token.payload.exp {
        return Err(AuthError::Expired);
    }

    let vk = VerifyingKey::from_bytes(&token.issuer_pubkey)
        .map_err(|_| AuthError::InvalidSig)?;
    let sig = Signature::from_bytes(&token.signature);
    vk.verify(&token.payload_bytes, &sig)
        .map_err(|_| AuthError::InvalidSig)?;

    if token.payload.cap.iter().any(|c| c == requested_cap) {
        Ok(Granted::Yes)
    } else {
        Ok(Granted::No)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use rand::rngs::OsRng;

    fn mint_token(exp: u64, caps: Vec<Capability>) -> (String, SigningKey) {
        let sk = SigningKey::generate(&mut OsRng);
        let payload = UcanPayload {
            iss: "did:key:alice".into(),
            aud: "did:key:bob".into(),
            exp,
            cap: caps,
        };
        let payload_bytes = serde_json::to_vec(&payload).unwrap();
        let sig = sk.sign(&payload_bytes);
        let pk = sk.verifying_key();
        let jwt = format!(
            "{}.{}.{}",
            URL_SAFE_NO_PAD.encode(&payload_bytes),
            URL_SAFE_NO_PAD.encode(sig.to_bytes()),
            URL_SAFE_NO_PAD.encode(pk.as_bytes()),
        );
        (jwt, sk)
    }

    fn cap(r: &str, a: &str) -> Capability {
        Capability { resource: r.into(), action: a.into() }
    }

    #[test]
    fn test_happy_path() {
        let (jwt, _) = mint_token(9999999999, vec![cap("wnfs://home", "write")]);
        let tok = parse(&jwt).unwrap();
        assert_eq!(
            check(&tok, &cap("wnfs://home", "write"), 1_700_000_000).unwrap(),
            Granted::Yes
        );
    }

    #[test]
    fn test_expired_token() {
        let (jwt, _) = mint_token(1, vec![cap("a", "b")]);
        let tok = parse(&jwt).unwrap();
        assert_eq!(
            check(&tok, &cap("a", "b"), 2).unwrap_err(),
            AuthError::Expired
        );
    }

    #[test]
    fn test_bad_sig() {
        let (jwt, _) = mint_token(9999999999, vec![cap("a", "b")]);
        let mut tok = parse(&jwt).unwrap();
        tok.signature[0] ^= 0xff; // tamper
        assert_eq!(
            check(&tok, &cap("a", "b"), 1_700_000_000).unwrap_err(),
            AuthError::InvalidSig
        );
    }

    #[test]
    fn test_cap_mismatch() {
        let (jwt, _) = mint_token(9999999999, vec![cap("a", "read")]);
        let tok = parse(&jwt).unwrap();
        assert_eq!(
            check(&tok, &cap("a", "write"), 1_700_000_000).unwrap(),
            Granted::No
        );
    }

    #[test]
    fn test_malformed_jwt() {
        assert!(matches!(parse("not-a-jwt"), Err(AuthError::MalformedToken(_))));
        assert!(matches!(parse("a.b"), Err(AuthError::MalformedToken(_))));
    }
}
