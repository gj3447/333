// KG: browser-gap-shallow-3-blockers-2026-07-15 (PR-1 wasm 지반),
//     rt1-harness-real-execution-unreceipted-2026-07-15 (RT1 = 커스터디얼 선행)
//
// Browser bindings for transfer333.
//
// WHAT THIS BUYS
// --------------
// In RT1 the coordinator derives the worker's owner key, so it can spend the
// worker's balance and the worker's only evidence of payment is the coordinator
// saying so. Two separate problems, and this crate is aimed at both:
//
//   1. CUSTODY. The key is generated here, in the page, and never leaves. Payment
//      lands in an account only this browser can spend from.
//   2. EVIDENCE. `verify_certificate` re-runs the real quorum check locally. The
//      claim stops being "the coordinator says you were paid" and becomes "a
//      quorum of the roster I pinned signed a transfer to my key".
//
// TRUST BOUNDARY — read before calling this "trustless"
// -----------------------------------------------------
// Verification is only as good as the roster it verifies against. If the page
// fetches the committee from the coordinator, the coordinator can hand over a
// committee of its own keys and sign anything. So the roster is an *input the
// caller must pin* (ship it with the page, or fetch it from somewhere with its
// own trust story). This crate refuses to fabricate that trust: `Config` is
// supplied by the caller and its provenance is the caller's problem.
//
// What is genuinely removed is coordinator *custody* and coordinator *say-so*
// against a pinned roster. What is not removed is roster distribution. Saying
// otherwise would be the cargo-cult version of this.

use ed25519_dalek::{SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};
use wasm_bindgen::prelude::*;

/// Route Rust panics to console.error with a real message instead of the opaque
/// "unreachable executed" / "memory access out of bounds" a wasm trap gives.
#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
}

use transfer333::{
    decode_certificate, encode_transfer, Committee, NetworkId, OwnerRegistry, SignedTransfer,
    Transfer, TransferPolicy,
};

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    if s.len() % 2 != 0 {
        return Err("hex: odd length".into());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| format!("hex: {e}")))
        .collect()
}

fn key_from_hex(s: &str) -> Result<VerifyingKey, String> {
    let raw = hex_decode(s)?;
    let arr: [u8; 32] = raw
        .try_into()
        .map_err(|_| "public key must be 32 bytes".to_string())?;
    VerifyingKey::from_bytes(&arr).map_err(|e| format!("bad public key: {e}"))
}

/// The roster a verifier checks against. Caller-pinned; see the trust note above.
///
/// Deliberately a plain struct built from explicit parts rather than a serde
/// derive over arbitrary JSON: every field here is trust-bearing, and a silent
/// default on any of them would be a hole.
struct Config {
    network_id: String,
    /// `(account, public key hex)` — every owner the policy knows.
    owners: Vec<(String, String)>,
    /// `(authority id, public key hex)` — the quorum roster.
    authorities: Vec<(String, String)>,
}

impl Config {
    /// Parse the minimal config shape:
    /// `{"network_id":"..","owners":{"alice":"<hex>",..},"authorities":{"a0":"<hex>",..}}`
    fn parse(json: &str) -> Result<Self, String> {
        let v = tiny_json::parse(json)?;
        let network_id = v.get_str("network_id")?;
        let owners = v.get_map("owners")?;
        let authorities = v.get_map("authorities")?;
        if owners.is_empty() {
            return Err("config: owners must not be empty".into());
        }
        if authorities.is_empty() {
            return Err("config: authorities must not be empty".into());
        }
        Ok(Self {
            network_id,
            owners,
            authorities,
        })
    }

    fn policy(&self) -> Result<TransferPolicy, String> {
        let mut bindings = Vec::with_capacity(self.owners.len());
        for (account, hex) in &self.owners {
            bindings.push((account.clone(), key_from_hex(hex)?));
        }
        let registry =
            OwnerRegistry::new(bindings).map_err(|e| format!("owner registry: {e:?}"))?;
        let network =
            NetworkId::new(self.network_id.clone()).map_err(|e| format!("network id: {e:?}"))?;
        Ok(TransferPolicy::new(network, registry))
    }

    fn committee(&self, policy: &TransferPolicy) -> Result<Committee, String> {
        let mut members = Vec::with_capacity(self.authorities.len());
        for (id, hex) in &self.authorities {
            members.push((id.clone(), key_from_hex(hex)?));
        }
        Committee::new(members, policy.clone()).ok_or_else(|| "committee: invalid roster".into())
    }
}

/// A browser-held owner identity.
#[wasm_bindgen]
pub struct BrowserOwner {
    key: SigningKey,
    account: String,
}

/// Pure constructor for native tests: derive the signing key from a seed without
/// crossing the wasm-bindgen boundary.
pub fn browser_owner_pubkey_hex(seed: &[u8; 32]) -> String {
    hex_encode(SigningKey::from_bytes(seed).verifying_key().as_bytes())
}

#[wasm_bindgen]
impl BrowserOwner {
    /// Derive an identity from a caller-supplied 32-byte seed (hex).
    ///
    /// Deterministic on purpose: a page that wants a fresh identity should call
    /// `crypto.getRandomValues` and pass the bytes in, which keeps the entropy
    /// source visible in the page rather than hidden behind this boundary. It
    /// also makes the whole thing testable without a browser.
    #[wasm_bindgen(js_name = fromSeedHex)]
    pub fn from_seed_hex(account: &str, seed_hex: &str) -> Result<BrowserOwner, JsError> {
        let raw = hex_decode(seed_hex).map_err(|e| JsError::new(&e))?;
        let seed: [u8; 32] = raw
            .try_into()
            .map_err(|_| JsError::new("seed must be 32 bytes"))?;
        Ok(BrowserOwner {
            key: SigningKey::from_bytes(&seed),
            account: account.to_string(),
        })
    }

    /// The public key to hand the payer so it can register this account.
    #[wasm_bindgen(js_name = publicKeyHex)]
    pub fn public_key_hex(&self) -> String {
        hex_encode(self.key.verifying_key().as_bytes())
    }

    #[wasm_bindgen(js_name = account)]
    pub fn account(&self) -> String {
        self.account.clone()
    }

    /// Sign a transfer *from* this account. Not used by RT2 (the browser is the
    /// payee there), but it is what makes the balance actually the browser's:
    /// without a local signer the funds would be unspendable, and "your key" would
    /// be decoration.
    #[wasm_bindgen(js_name = signTransfer)]
    pub fn sign_transfer(
        &self,
        config_json: &str,
        to: &str,
        from_seq: u64,
        amount: &str,
    ) -> Result<Vec<u8>, JsError> {
        let cfg = Config::parse(config_json).map_err(|e| JsError::new(&e))?;
        let policy = cfg.policy().map_err(|e| JsError::new(&e))?;
        let amount: u128 = amount
            .parse()
            .map_err(|_| JsError::new("amount must be a u128 decimal string"))?;
        let order = SignedTransfer::sign(
            &policy,
            Transfer {
                from: self.account.clone(),
                from_seq,
                to: to.to_string(),
                amount,
            },
            &self.key,
        );
        Ok(encode_transfer(&order))
    }
}

/// Verify a quorum certificate locally and report what it actually certifies.
///
/// Returns a JSON string on success. Every field is derived from the certificate
/// itself, never from the caller's expectations — the point is for the page to
/// compare these against what it was promised.
///
/// `config_json` must be the pinned roster. See the trust note at the top.
/// Pure core, testable on native. The `#[wasm_bindgen]` wrapper is a thin shell
/// so the actual verification logic can be exercised without a browser.
pub fn verify_certificate_core(config_json: &str, cert_bytes: &[u8]) -> Result<String, String> {
    let cfg = Config::parse(config_json)?;
    let policy = cfg.policy()?;
    let committee = cfg.committee(&policy)?;

    let cert = decode_certificate(cert_bytes).map_err(|e| format!("{e:?}"))?;
    // The real check: quorum size, committee binding, per-vote Ed25519 signatures,
    // owner proof. `Verified` cannot be constructed any other way.
    let verified = cert
        .verify(&committee)
        .ok_or_else(|| "certificate did not verify against the pinned committee".to_string())?;

    let t = verified.transfer();
    Ok(format!(
        r#"{{"verified":true,"from":"{}","to":"{}","from_seq":{},"amount":"{}","committee_size":{},"quorum":{},"order_id":"{}"}}"#,
        json_escape(&t.from),
        json_escape(&t.to),
        t.from_seq,
        t.amount,
        committee.size(),
        committee.quorum(),
        hex_encode(&verified.order().order_id()),
    ))
}

#[wasm_bindgen(js_name = verifyCertificate)]
pub fn verify_certificate(config_json: &str, cert_bytes: &[u8]) -> Result<String, JsError> {
    verify_certificate_core(config_json, cert_bytes).map_err(|e| JsError::new(&e))
}

/// Digest of the pinned roster, so a page can prove which roster it verified
/// against (and a receipt can record it).
pub fn config_fingerprint_core(config_json: &str) -> Result<String, String> {
    let cfg = Config::parse(config_json)?;
    let policy = cfg.policy()?;
    let committee = cfg.committee(&policy)?;
    let mut d = Sha256::new();
    d.update(b"transfer333-wasm/config-fingerprint/v1\0");
    d.update(policy.id().as_bytes());
    d.update(committee.id().as_bytes());
    Ok(hex_encode(&d.finalize()))
}

#[wasm_bindgen(js_name = configFingerprint)]
pub fn config_fingerprint(config_json: &str) -> Result<String, JsError> {
    config_fingerprint_core(config_json).map_err(|e| JsError::new(&e))
}

fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Enough JSON to read a pinned config, and no more.
///
/// A full serde stack would be a large dependency for three fields, and this
/// keeps the parse total: anything unexpected is an error rather than a default.
mod tiny_json {
    pub struct Value(String);

    pub fn parse(s: &str) -> Result<Value, String> {
        if !s.trim_start().starts_with('{') {
            return Err("config: expected a JSON object".into());
        }
        Ok(Value(s.to_string()))
    }

    impl Value {
        pub fn get_str(&self, key: &str) -> Result<String, String> {
            let needle = format!("\"{key}\"");
            let i = self
                .0
                .find(&needle)
                .ok_or_else(|| format!("config: missing \"{key}\""))?;
            let rest = &self.0[i + needle.len()..];
            let colon = rest.find(':').ok_or("config: expected ':'")?;
            let after = rest[colon + 1..].trim_start();
            if !after.starts_with('"') {
                return Err(format!("config: \"{key}\" must be a string"));
            }
            let body = &after[1..];
            let end = body.find('"').ok_or("config: unterminated string")?;
            Ok(body[..end].to_string())
        }

        /// Flat `{"k":"v",..}` object under `key`.
        pub fn get_map(&self, key: &str) -> Result<Vec<(String, String)>, String> {
            let needle = format!("\"{key}\"");
            let i = self
                .0
                .find(&needle)
                .ok_or_else(|| format!("config: missing \"{key}\""))?;
            let rest = &self.0[i + needle.len()..];
            let colon = rest.find(':').ok_or("config: expected ':'")?;
            let after = rest[colon + 1..].trim_start();
            if !after.starts_with('{') {
                return Err(format!("config: \"{key}\" must be an object"));
            }
            let end = after.find('}').ok_or("config: unterminated object")?;
            let body = &after[1..end];
            let mut out = Vec::new();
            for part in body.split(',') {
                let part = part.trim();
                if part.is_empty() {
                    continue;
                }
                let (k, v) = part
                    .split_once(':')
                    .ok_or_else(|| format!("config: bad entry in \"{key}\": {part}"))?;
                let k = k.trim().trim_matches('"').to_string();
                let v = v.trim().trim_matches('"').to_string();
                if k.is_empty() {
                    return Err(format!("config: empty key in \"{key}\""));
                }
                out.push((k, v));
            }
            Ok(out)
        }
    }
}
