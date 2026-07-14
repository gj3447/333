// KG: SPAN_333_Security — Security module
// KG: plan-333-longinus-full-coverage-2026-04-14 — src-security-CheatType/TrustScore/Referee/ContentModerator/EncryptedWallet/derive_key/encrypt_wallet/decrypt_wallet
// Anti-cheat, content moderation, wallet encryption

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

// --- Anti-Cheat Referee ---

/// Cheat detection events
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CheatType {
    SpeedHack { expected_ms: u64, actual_ms: u64 },
    StateManipulation { field: String, expected: String, actual: String },
    ImpossibleAction { description: String },
    DesyncDetected { local_hash: String, remote_hash: String },
}

/// Player trust score
#[derive(Debug, Clone)]
pub struct TrustScore {
    pub peer_id: u32,
    pub score: f64,       // 0.0 (banned) to 1.0 (trusted)
    pub violations: u32,
    pub verified_actions: u64,
}

impl TrustScore {
    pub fn new(peer_id: u32) -> Self {
        Self { peer_id, score: 0.5, violations: 0, verified_actions: 0 }
    }

    pub fn record_violation(&mut self) {
        self.violations += 1;
        self.score = (self.score - 0.1).max(0.0);
    }

    pub fn record_verified(&mut self) {
        self.verified_actions += 1;
        self.score = (self.score + 0.001).min(1.0);
    }

    pub fn is_banned(&self) -> bool { self.score < 0.1 }
    pub fn is_trusted(&self) -> bool { self.score > 0.8 }
}

/// Anti-cheat referee node
pub struct Referee {
    trust_scores: HashMap<u32, TrustScore>,
    cheat_log: Vec<(u32, CheatType)>,
    banned: HashSet<u32>,
}

impl Default for Referee {
    fn default() -> Self {
        Self::new()
    }
}

impl Referee {
    pub fn new() -> Self {
        Self { trust_scores: HashMap::new(), cheat_log: Vec::new(), banned: HashSet::new() }
    }

    pub fn register_player(&mut self, peer_id: u32) {
        self.trust_scores.entry(peer_id).or_insert_with(|| TrustScore::new(peer_id));
    }

    /// Report a cheat event
    pub fn report_cheat(&mut self, peer_id: u32, cheat: CheatType) -> bool {
        self.cheat_log.push((peer_id, cheat));
        if let Some(ts) = self.trust_scores.get_mut(&peer_id) {
            ts.record_violation();
            if ts.is_banned() {
                self.banned.insert(peer_id);
                return true; // player banned
            }
        }
        false
    }

    /// Verify a legitimate action
    pub fn verify_action(&mut self, peer_id: u32) {
        if let Some(ts) = self.trust_scores.get_mut(&peer_id) {
            ts.record_verified();
        }
    }

    /// Check if player is banned
    pub fn is_banned(&self, peer_id: u32) -> bool {
        self.banned.contains(&peer_id)
    }

    /// Get trust score
    pub fn trust_score(&self, peer_id: u32) -> Option<f64> {
        self.trust_scores.get(&peer_id).map(|ts| ts.score)
    }

    /// Get cheat log count
    pub fn cheat_count(&self) -> usize { self.cheat_log.len() }
}

// --- Content Moderation ---

/// Content moderation decision
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModerationDecision {
    Approved,
    Rejected { reason: String },
    Flagged { reason: String, votes_needed: u32 },
}

/// Community-based content moderation
pub struct ContentModerator {
    flagged: HashMap<String, Vec<(u32, String)>>,  // content_id → (reporter, reason)
    threshold: u32,  // flags needed to remove
}

impl ContentModerator {
    pub fn new(threshold: u32) -> Self {
        Self { flagged: HashMap::new(), threshold }
    }

    /// Flag content
    pub fn flag(&mut self, content_id: &str, reporter: u32, reason: &str) -> ModerationDecision {
        let reports = self.flagged.entry(content_id.to_string()).or_default();
        // One report per user
        if reports.iter().any(|(r, _)| *r == reporter) {
            return ModerationDecision::Flagged {
                reason: "already reported".into(),
                votes_needed: self.threshold.saturating_sub(reports.len() as u32),
            };
        }
        reports.push((reporter, reason.to_string()));

        if reports.len() as u32 >= self.threshold {
            ModerationDecision::Rejected { reason: format!("{} reports reached threshold", reports.len()) }
        } else {
            ModerationDecision::Flagged {
                reason: reason.to_string(),
                votes_needed: self.threshold - reports.len() as u32,
            }
        }
    }

    /// Check if content is rejected
    pub fn is_rejected(&self, content_id: &str) -> bool {
        self.flagged.get(content_id)
            .is_some_and(|r| r.len() as u32 >= self.threshold)
    }

    /// Get flag count
    pub fn flag_count(&self, content_id: &str) -> u32 {
        self.flagged.get(content_id).map_or(0, |r| r.len() as u32)
    }
}

// --- Wallet Encryption ---

/// Encrypted wallet data (for localStorage)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedWallet {
    pub salt: Vec<u8>,
    pub nonce: Vec<u8>,
    pub ciphertext: Vec<u8>,
    pub version: u8,
}

/// Derive encryption key from password (simplified PBKDF2-like)
pub fn derive_key(password: &[u8], salt: &[u8]) -> [u8; 32] {
    // Simple but deterministic HMAC-like derivation
    let mut state = [0u64; 4];
    for (i, &b) in password.iter().chain(salt.iter()).enumerate() {
        state[i % 4] = state[i % 4]
            .wrapping_mul(6364136223846793005)
            .wrapping_add(b as u64)
            .wrapping_add(i as u64);
    }
    for _ in 0..10000 {
        for i in 0..4 {
            state[i] = state[i].wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            state[i] ^= state[(i + 1) % 4];
        }
    }
    let mut key = [0u8; 32];
    for (i, s) in state.iter().enumerate() {
        let bytes = s.to_le_bytes();
        key[i * 8..(i + 1) * 8].copy_from_slice(&bytes);
    }
    key
}

/// XOR-encrypt wallet data (simplified; real would use AES-GCM)
pub fn encrypt_wallet(data: &[u8], key: &[u8; 32], nonce: &[u8]) -> Vec<u8> {
    data.iter().enumerate()
        .map(|(i, b)| b ^ key[i % 32] ^ nonce[i % nonce.len()])
        .collect()
}

/// XOR-decrypt (symmetric)
pub fn decrypt_wallet(ciphertext: &[u8], key: &[u8; 32], nonce: &[u8]) -> Vec<u8> {
    encrypt_wallet(ciphertext, key, nonce) // XOR is its own inverse
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn referee_tracks_cheats() {
        let mut ref_ = Referee::new();
        ref_.register_player(1);
        ref_.report_cheat(1, CheatType::SpeedHack { expected_ms: 100, actual_ms: 10 });
        assert_eq!(ref_.cheat_count(), 1);
        assert!(ref_.trust_score(1).unwrap() < 0.5);
    }

    #[test]
    fn referee_bans_after_many_violations() {
        let mut ref_ = Referee::new();
        ref_.register_player(1);
        for _ in 0..10 {
            ref_.report_cheat(1, CheatType::ImpossibleAction { description: "fly".into() });
        }
        assert!(ref_.is_banned(1));
    }

    #[test]
    fn verified_actions_increase_trust() {
        let mut ref_ = Referee::new();
        ref_.register_player(1);
        for _ in 0..500 {
            ref_.verify_action(1);
        }
        assert!(ref_.trust_score(1).unwrap() > 0.8);
    }

    #[test]
    fn moderation_flag_threshold() {
        let mut mod_ = ContentModerator::new(3);
        mod_.flag("post-1", 1, "spam");
        mod_.flag("post-1", 2, "spam");
        assert!(!mod_.is_rejected("post-1"));
        assert_eq!(mod_.flag_count("post-1"), 2);

        mod_.flag("post-1", 3, "spam");
        assert!(mod_.is_rejected("post-1"));
    }

    #[test]
    fn moderation_no_duplicate_reports() {
        let mut mod_ = ContentModerator::new(3);
        mod_.flag("post-1", 1, "spam");
        mod_.flag("post-1", 1, "spam again"); // same reporter
        assert_eq!(mod_.flag_count("post-1"), 1); // still 1
    }

    #[test]
    fn wallet_encrypt_decrypt_roundtrip() {
        let password = b"my-secure-password";
        let salt = b"random-salt-1234";
        let nonce = b"nonce-12";
        let key = derive_key(password, salt);

        let data = b"private key data here";
        let encrypted = encrypt_wallet(data, &key, nonce);
        assert_ne!(encrypted, data.to_vec());

        let decrypted = decrypt_wallet(&encrypted, &key, nonce);
        assert_eq!(decrypted, data.to_vec());
    }

    #[test]
    fn derive_key_deterministic() {
        let k1 = derive_key(b"pass", b"salt");
        let k2 = derive_key(b"pass", b"salt");
        assert_eq!(k1, k2);

        let k3 = derive_key(b"pass", b"other");
        assert_ne!(k1, k3);
    }
}
