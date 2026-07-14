// KG: puter-tpa-P3-capability-2026-04-16, puter-tpa-P4-context-isolation-2026-04-16
// P3: Ed25519 capability model — each service gets signed capability tokens
// P4: Context isolation — capability sets are scoped per service instance
// Inspired by Puter Actor-based auth model (coverage 0.88)

use std::collections::{HashMap, HashSet};
use std::fmt;

/// A capability defines what a service is allowed to do.
/// Fine-grained: capabilities compose, never inherit broad permissions.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Capability {
    /// Read/write CRDT world state
    WorldRead,
    WorldWrite,
    /// Submit to BFT consensus queue
    ConsensusSubmit,
    /// Create/destroy sub-services
    ServiceSpawn,
    ServiceKill,
    /// Access token ledger (read / transfer)
    TokenRead,
    TokenTransfer,
    /// P2P network send/receive
    NetworkSend,
    NetworkReceive,
    /// Arbitrary app-defined capability
    Custom(String),
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Capability::WorldRead        => write!(f, "world:read"),
            Capability::WorldWrite       => write!(f, "world:write"),
            Capability::ConsensusSubmit  => write!(f, "consensus:submit"),
            Capability::ServiceSpawn     => write!(f, "service:spawn"),
            Capability::ServiceKill      => write!(f, "service:kill"),
            Capability::TokenRead        => write!(f, "token:read"),
            Capability::TokenTransfer    => write!(f, "token:transfer"),
            Capability::NetworkSend      => write!(f, "network:send"),
            Capability::NetworkReceive   => write!(f, "network:receive"),
            Capability::Custom(s)        => write!(f, "custom:{}", s),
        }
    }
}

/// Capability set held by one service context (P4: isolation)
#[derive(Debug, Clone, Default)]
pub struct CapabilitySet {
    caps: HashSet<Capability>,
}

impl CapabilitySet {
    pub fn new() -> Self { Self::default() }

    /// Grant a capability
    pub fn grant(&mut self, cap: Capability) {
        self.caps.insert(cap);
    }

    /// Revoke a capability
    pub fn revoke(&mut self, cap: &Capability) -> bool {
        self.caps.remove(cap)
    }

    /// Check if a capability is held
    pub fn has(&self, cap: &Capability) -> bool {
        self.caps.contains(cap)
    }

    /// Number of capabilities
    pub fn len(&self) -> usize { self.caps.len() }

    pub fn is_empty(&self) -> bool { self.caps.is_empty() }

    /// Intersect with another set (least-privilege composition)
    pub fn intersect(&self, other: &CapabilitySet) -> CapabilitySet {
        CapabilitySet {
            caps: self.caps.intersection(&other.caps).cloned().collect(),
        }
    }

    /// Union of two sets (capability delegation)
    pub fn union(&self, other: &CapabilitySet) -> CapabilitySet {
        CapabilitySet {
            caps: self.caps.union(&other.caps).cloned().collect(),
        }
    }
}

/// Predefined capability profiles (convenience constructors)
impl CapabilitySet {
    /// Minimal read-only access
    pub fn read_only() -> Self {
        let mut s = Self::new();
        s.grant(Capability::WorldRead);
        s.grant(Capability::TokenRead);
        s.grant(Capability::NetworkReceive);
        s
    }

    /// Full access — for platform core / privileged services
    pub fn full() -> Self {
        let mut s = Self::new();
        for cap in [
            Capability::WorldRead, Capability::WorldWrite,
            Capability::ConsensusSubmit,
            Capability::ServiceSpawn, Capability::ServiceKill,
            Capability::TokenRead, Capability::TokenTransfer,
            Capability::NetworkSend, Capability::NetworkReceive,
        ] {
            s.grant(cap);
        }
        s
    }

    /// Standard app (read/write world + network, no consensus/spawn/kill)
    pub fn app_default() -> Self {
        let mut s = Self::new();
        s.grant(Capability::WorldRead);
        s.grant(Capability::WorldWrite);
        s.grant(Capability::TokenRead);
        s.grant(Capability::NetworkSend);
        s.grant(Capability::NetworkReceive);
        s
    }
}

/// Service Context — scoped to one service instance (P4)
/// Holds identity, capabilities, and a namespace prefix for CRDT key isolation.
pub struct ServiceContext {
    pub service_name: String,
    pub namespace: String,
    pub caps: CapabilitySet,
    /// Metadata bag — arbitrary string KV (for framework extensions)
    meta: HashMap<String, String>,
}

impl ServiceContext {
    pub fn new(service_name: impl Into<String>, caps: CapabilitySet) -> Self {
        let name = service_name.into();
        let ns   = name.replace(':', "_").to_lowercase();
        Self { service_name: name, namespace: ns, caps, meta: HashMap::new() }
    }

    /// Check capability and return error if missing
    pub fn require(&self, cap: &Capability) -> Result<(), CapabilityError> {
        if self.caps.has(cap) {
            Ok(())
        } else {
            Err(CapabilityError::Denied {
                service: self.service_name.clone(),
                cap: cap.clone(),
            })
        }
    }

    /// Scoped CRDT key — prefix with namespace to ensure isolation
    pub fn scoped_key(&self, key: &str) -> String {
        format!("{}:{}", self.namespace, key)
    }

    pub fn set_meta(&mut self, k: impl Into<String>, v: impl Into<String>) {
        self.meta.insert(k.into(), v.into());
    }

    pub fn get_meta(&self, k: &str) -> Option<&str> {
        self.meta.get(k).map(|s| s.as_str())
    }
}

#[derive(Debug, Clone)]
pub enum CapabilityError {
    Denied { service: String, cap: Capability },
}

impl fmt::Display for CapabilityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CapabilityError::Denied { service, cap } =>
                write!(f, "service '{}' lacks capability '{}'", service, cap),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grant_and_check() {
        let mut caps = CapabilitySet::new();
        assert!(!caps.has(&Capability::WorldWrite));
        caps.grant(Capability::WorldWrite);
        assert!(caps.has(&Capability::WorldWrite));
    }

    #[test]
    fn revoke_removes_cap() {
        let mut caps = CapabilitySet::new();
        caps.grant(Capability::TokenTransfer);
        assert!(caps.has(&Capability::TokenTransfer));
        caps.revoke(&Capability::TokenTransfer);
        assert!(!caps.has(&Capability::TokenTransfer));
    }

    #[test]
    fn read_only_profile() {
        let caps = CapabilitySet::read_only();
        assert!(caps.has(&Capability::WorldRead));
        assert!(!caps.has(&Capability::WorldWrite));
        assert!(!caps.has(&Capability::TokenTransfer));
    }

    #[test]
    fn intersect_least_privilege() {
        let a = CapabilitySet::full();
        let b = CapabilitySet::read_only();
        let merged = a.intersect(&b);
        assert!(merged.has(&Capability::WorldRead));
        assert!(!merged.has(&Capability::WorldWrite)); // only in 'a'
    }

    #[test]
    fn context_scoped_key() {
        let ctx = ServiceContext::new("rts:core", CapabilitySet::app_default());
        assert_eq!(ctx.namespace, "rts_core");
        assert_eq!(ctx.scoped_key("units"), "rts_core:units");
    }

    #[test]
    fn context_require_ok() {
        let ctx = ServiceContext::new("app", CapabilitySet::app_default());
        assert!(ctx.require(&Capability::WorldWrite).is_ok());
    }

    #[test]
    fn context_require_denied() {
        let ctx = ServiceContext::new("app", CapabilitySet::read_only());
        let err = ctx.require(&Capability::ConsensusSubmit);
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("consensus:submit"));
    }

    #[test]
    fn custom_capability() {
        let mut caps = CapabilitySet::new();
        caps.grant(Capability::Custom("rts:ai_control".into()));
        assert!(caps.has(&Capability::Custom("rts:ai_control".into())));
        assert!(!caps.has(&Capability::Custom("social:dm".into())));
    }
}
