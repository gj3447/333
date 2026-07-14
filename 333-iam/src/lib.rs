// KG: SPAN_333_L9_IAM, SPAN_333_L9_IAM_Prod, plan-333-three-way-hybrid-2026-04-17, plan-333-p2p-os-synthesis-execution-2026-04-18
// User/role/policy model with salted SHA-256 password hashing.
// Inspired by SeaweedFS weed/credential/credential_store.go.
//
// ⚠️ LEGACY TESTING-ONLY PATH ⚠️ (the CredentialStore below)
// - Hash = SHA-256 (fast = weak against brute force).
// - Salt = deterministic `format!("salt-{name}")` (rainbow-table vulnerable).
// - Kept for backward-compat with existing 333-iam callers.
//
// P6 production extensions (prom32-333-p2p-2026-04-18):
//   - production_hasher: PasswordHasher trait + IterationHasher (D30 Argon2 path)
//   - capability:        UCAN-lite CapabilityToken + audience-bound verifier (D28/D31)
//   - revocation:        CRDT tombstone RevocationStore (D30/D31)

#![forbid(unsafe_code)]

pub mod production_hasher;
pub mod capability;
pub mod revocation;

pub use production_hasher::{Hash as ProdHash, IterationHasher, PasswordHasher, Salt};
pub use capability::{CapabilityError, CapabilityToken, CapabilityVerifier, InMemoryVerifier};
pub use revocation::{InMemoryRevocationStore, RevocationEntry, RevocationMap, RevocationStore};

use std::collections::{BTreeMap, BTreeSet};
use std::sync::RwLock;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum IamError {
    #[error("user not found: {0}")]
    UserNotFound(String),
    #[error("user already exists: {0}")]
    UserExists(String),
    #[error("authentication failed")]
    AuthFailed,
    #[error("backend: {0}")]
    Backend(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct User {
    pub name: String,
    pub pw_hash: String, // hex-encoded SHA-256 of salt||password
    pub salt: String,
    pub roles: BTreeSet<String>,
}

pub trait CredentialStore: Send + Sync {
    fn create(&self, name: &str, password: &str) -> Result<User, IamError>;
    fn get(&self, name: &str) -> Result<User, IamError>;
    fn list(&self) -> Result<Vec<String>, IamError>;
    fn authenticate(&self, name: &str, password: &str) -> Result<User, IamError>;
    fn grant_role(&self, name: &str, role: &str) -> Result<(), IamError>;
    fn revoke_role(&self, name: &str, role: &str) -> Result<(), IamError>;
    fn delete(&self, name: &str) -> Result<User, IamError>;
}

fn hex_of(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

fn pw_hash(salt: &str, password: &str) -> String {
    let mut h = Sha256::new();
    h.update(salt.as_bytes());
    h.update(password.as_bytes());
    hex_of(&h.finalize())
}

pub struct InMemoryCredentialStore {
    users: RwLock<BTreeMap<String, User>>,
}

impl Default for InMemoryCredentialStore {
    fn default() -> Self {
        Self { users: RwLock::new(BTreeMap::new()) }
    }
}

impl InMemoryCredentialStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl CredentialStore for InMemoryCredentialStore {
    fn create(&self, name: &str, password: &str) -> Result<User, IamError> {
        let mut g = self.users.write().map_err(|e| IamError::Backend(e.to_string()))?;
        if g.contains_key(name) {
            return Err(IamError::UserExists(name.to_string()));
        }
        // Salt = name for determinism in tests. Production should use RNG.
        let salt = format!("salt-{name}");
        let user = User {
            name: name.to_string(),
            salt: salt.clone(),
            pw_hash: pw_hash(&salt, password),
            roles: BTreeSet::new(),
        };
        g.insert(name.to_string(), user.clone());
        Ok(user)
    }

    fn get(&self, name: &str) -> Result<User, IamError> {
        let g = self.users.read().map_err(|e| IamError::Backend(e.to_string()))?;
        g.get(name).cloned().ok_or_else(|| IamError::UserNotFound(name.to_string()))
    }

    fn list(&self) -> Result<Vec<String>, IamError> {
        let g = self.users.read().map_err(|e| IamError::Backend(e.to_string()))?;
        Ok(g.keys().cloned().collect())
    }

    fn authenticate(&self, name: &str, password: &str) -> Result<User, IamError> {
        let user = self.get(name)?;
        let expected = pw_hash(&user.salt, password);
        if expected == user.pw_hash {
            Ok(user)
        } else {
            Err(IamError::AuthFailed)
        }
    }

    fn grant_role(&self, name: &str, role: &str) -> Result<(), IamError> {
        let mut g = self.users.write().map_err(|e| IamError::Backend(e.to_string()))?;
        let user = g.get_mut(name).ok_or_else(|| IamError::UserNotFound(name.to_string()))?;
        user.roles.insert(role.to_string());
        Ok(())
    }

    fn revoke_role(&self, name: &str, role: &str) -> Result<(), IamError> {
        let mut g = self.users.write().map_err(|e| IamError::Backend(e.to_string()))?;
        let user = g.get_mut(name).ok_or_else(|| IamError::UserNotFound(name.to_string()))?;
        user.roles.remove(role);
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<User, IamError> {
        let mut g = self.users.write().map_err(|e| IamError::Backend(e.to_string()))?;
        g.remove(name).ok_or_else(|| IamError::UserNotFound(name.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_then_authenticate() {
        let cs = InMemoryCredentialStore::new();
        cs.create("alice", "pw123").unwrap();
        let u = cs.authenticate("alice", "pw123").unwrap();
        assert_eq!(u.name, "alice");
    }

    #[test]
    fn wrong_password_fails() {
        let cs = InMemoryCredentialStore::new();
        cs.create("alice", "right").unwrap();
        assert!(matches!(cs.authenticate("alice", "wrong"), Err(IamError::AuthFailed)));
    }

    #[test]
    fn plaintext_never_stored() {
        let cs = InMemoryCredentialStore::new();
        let u = cs.create("alice", "secret").unwrap();
        assert_ne!(u.pw_hash, "secret");
        assert!(!u.pw_hash.contains("secret"));
        assert_eq!(u.pw_hash.len(), 64); // SHA-256 hex
    }

    #[test]
    fn duplicate_create_rejected() {
        let cs = InMemoryCredentialStore::new();
        cs.create("bob", "x").unwrap();
        assert!(matches!(cs.create("bob", "y"), Err(IamError::UserExists(_))));
    }

    #[test]
    fn role_grant_revoke() {
        let cs = InMemoryCredentialStore::new();
        cs.create("carol", "p").unwrap();
        cs.grant_role("carol", "admin").unwrap();
        cs.grant_role("carol", "reader").unwrap();
        let u = cs.get("carol").unwrap();
        assert!(u.roles.contains("admin"));
        assert!(u.roles.contains("reader"));

        cs.revoke_role("carol", "admin").unwrap();
        assert!(!cs.get("carol").unwrap().roles.contains("admin"));
    }

    #[test]
    fn delete_missing_errors() {
        let cs = InMemoryCredentialStore::new();
        assert!(matches!(cs.delete("ghost"), Err(IamError::UserNotFound(_))));
    }

    #[test]
    fn list_returns_all() {
        let cs = InMemoryCredentialStore::new();
        cs.create("a", "x").unwrap();
        cs.create("b", "y").unwrap();
        let names = cs.list().unwrap();
        assert_eq!(names, vec!["a", "b"]);
    }
}
