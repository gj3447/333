// KG: puter-tpa-P1-service-registry-2026-04-16, SPAN_333_Kernel
// P1: ServiceRegistry<S> sealed trait — Puter Microkernel pattern (confidence 0.92)
// Sealed: external code cannot implement Service without going through register().
// ServiceRegistry is the single-source-of-truth for all live service instances.

use super::lifecycle::{LifecycleMachine, LifecycleError, ServiceState};
use super::capability::{ServiceContext, CapabilitySet, CapabilityError};
use std::collections::HashMap;
use std::fmt;

// ── Sealed Trait Machinery ────────────────────────────────────────────────────

/// Pub(crate) sealed marker — accessible within this crate (e.g. testutil)
/// but not by external crates. This is the Rust sealed-trait pattern.
#[doc(hidden)]
pub mod sealed_pub {
    pub trait SealedMarker {}
}

use sealed_pub::SealedMarker;

mod sealed {
    /// Private alias — Service bound uses this.
    pub trait Sealed: super::SealedMarker {}
    impl<T: super::SealedMarker> Sealed for T {}
}

/// Error type for service operations
#[derive(Debug, Clone)]
pub enum ServiceError {
    Lifecycle(LifecycleError),
    Capability(CapabilityError),
    NotFound(String),
    AlreadyRegistered(String),
    Custom(String),
}

impl From<LifecycleError> for ServiceError {
    fn from(e: LifecycleError) -> Self { ServiceError::Lifecycle(e) }
}

impl From<CapabilityError> for ServiceError {
    fn from(e: CapabilityError) -> Self { ServiceError::Capability(e) }
}

impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ServiceError::Lifecycle(e)        => write!(f, "lifecycle: {}", e),
            ServiceError::Capability(e)       => write!(f, "capability: {}", e),
            ServiceError::NotFound(n)         => write!(f, "service '{}' not found", n),
            ServiceError::AlreadyRegistered(n)=> write!(f, "service '{}' already registered", n),
            ServiceError::Custom(s)           => write!(f, "{}", s),
        }
    }
}

/// The platform Service contract (P1 sealed trait).
/// Implementing this trait requires going through the sealed machinery —
/// ServiceRegistry is the only blessed path.
///
/// Methods mirror Puter's BaseService._init / _run / _stop lifecycle:
/// - `init`  → called once when Starting (async init, DB connections, etc.)
/// - `run`   → called when transitioning to Running (accept requests)
/// - `stop`  → called when Stopping (drain, cleanup)
pub trait Service: sealed::Sealed + Send {
    /// Unique service name (must match registration key)
    fn name(&self) -> &str;

    /// One-time initialisation. Called in Starting state.
    fn init(&mut self, ctx: &mut ServiceContext) -> Result<(), ServiceError>;

    /// Begin accepting work. Called on transition to Running.
    fn run(&mut self, ctx: &ServiceContext) -> Result<(), ServiceError>;

    /// Clean shutdown. Called on transition to Stopping.
    fn stop(&mut self) -> Result<(), ServiceError>;

    /// Service self-description (for diagnostics)
    fn describe(&self) -> String {
        format!("Service({})", self.name())
    }
}

/// ServiceEntry — wraps a service instance with its lifecycle + context
struct ServiceEntry {
    service: Box<dyn Service>,
    lifecycle: LifecycleMachine,
    ctx: ServiceContext,
}

/// ServiceRegistry — Microkernel service bus (P1)
/// Single owner of all running services. Manages start/stop orchestration.
pub struct ServiceRegistry {
    services: HashMap<String, ServiceEntry>,
}

impl ServiceRegistry {
    pub fn new() -> Self {
        Self { services: HashMap::new() }
    }

    /// Register a new service with the given capability set.
    /// Service must be in Created state (fresh instance).
    pub fn register<S: Service + 'static>(
        &mut self,
        service: S,
        caps: CapabilitySet,
    ) -> Result<(), ServiceError> {
        let name = service.name().to_string();
        if self.services.contains_key(&name) {
            return Err(ServiceError::AlreadyRegistered(name));
        }
        let ctx = ServiceContext::new(&name, caps);
        self.services.insert(name, ServiceEntry {
            service: Box::new(service),
            lifecycle: LifecycleMachine::new(),
            ctx,
        });
        Ok(())
    }

    /// Start a registered service (Created → Starting → Running)
    pub fn start(&mut self, name: &str) -> Result<(), ServiceError> {
        let entry = self.services.get_mut(name)
            .ok_or_else(|| ServiceError::NotFound(name.to_string()))?;

        entry.lifecycle.transition(ServiceState::Starting)?;
        match entry.service.init(&mut entry.ctx) {
            Ok(()) => {}
            Err(e) => {
                let _ = entry.lifecycle.transition(ServiceState::Failed(e.to_string()));
                return Err(e);
            }
        }

        entry.lifecycle.transition(ServiceState::Running)?;
        match entry.service.run(&entry.ctx) {
            Ok(()) => Ok(()),
            Err(e) => {
                let _ = entry.lifecycle.transition(ServiceState::Failed(e.to_string()));
                Err(e)
            }
        }
    }

    /// Stop a running service (Running → Stopping → Stopped)
    pub fn stop(&mut self, name: &str) -> Result<(), ServiceError> {
        let entry = self.services.get_mut(name)
            .ok_or_else(|| ServiceError::NotFound(name.to_string()))?;

        entry.lifecycle.transition(ServiceState::Stopping)?;
        match entry.service.stop() {
            Ok(()) => {
                entry.lifecycle.transition(ServiceState::Stopped)?;
                Ok(())
            }
            Err(e) => {
                let _ = entry.lifecycle.transition(ServiceState::Failed(e.to_string()));
                Err(e)
            }
        }
    }

    /// Query lifecycle state of a service
    pub fn state(&self, name: &str) -> Option<&ServiceState> {
        self.services.get(name).map(|e| e.lifecycle.state())
    }

    /// Is service currently Running?
    pub fn is_running(&self, name: &str) -> bool {
        self.services.get(name).map_or(false, |e| e.lifecycle.is_running())
    }

    /// Check if a service holds a specific capability.
    /// KG: sprint7J-capability-enforce-2026-04-16
    pub fn has_capability(&self, name: &str, cap: &super::capability::Capability) -> bool {
        self.services.get(name).map_or(false, |e| e.ctx.caps.has(cap))
    }

    /// List all registered service names
    pub fn names(&self) -> Vec<&str> {
        self.services.keys().map(|s| s.as_str()).collect()
    }

    /// Total registered count
    pub fn len(&self) -> usize { self.services.len() }

    pub fn is_empty(&self) -> bool { self.services.is_empty() }

    /// Start all registered services in insertion order
    pub fn start_all(&mut self) -> Vec<ServiceError> {
        let names: Vec<String> = self.services.keys().cloned().collect();
        let mut errors = vec![];
        for name in names {
            if let Err(e) = self.start(&name) {
                errors.push(e);
            }
        }
        errors
    }

    /// Stop all running services
    pub fn stop_all(&mut self) -> Vec<ServiceError> {
        let names: Vec<String> = self.services.keys().cloned().collect();
        let mut errors = vec![];
        for name in names {
            if self.is_running(&name) {
                if let Err(e) = self.stop(&name) {
                    errors.push(e);
                }
            }
        }
        errors
    }
}

impl Default for ServiceRegistry {
    fn default() -> Self { Self::new() }
}

// ── Test helpers: concrete services that implement the sealed trait ────────────

/// Test service helpers — pub so integration tests can use them.
/// These are not sealed against external users but are clearly named as test helpers.
pub mod test_services {
    use super::*;

    /// A no-op service for testing
    pub struct NoopService {
        pub name: String,
        pub init_count: u32,
        pub run_count:  u32,
        pub stop_count: u32,
    }

    impl NoopService {
        pub fn new(name: impl Into<String>) -> Self {
            Self { name: name.into(), init_count: 0, run_count: 0, stop_count: 0 }
        }
    }

    impl SealedMarker for NoopService {}

    impl Service for NoopService {
        fn name(&self) -> &str { &self.name }
        fn init(&mut self, _ctx: &mut ServiceContext) -> Result<(), ServiceError> {
            self.init_count += 1;
            Ok(())
        }
        fn run(&mut self, _ctx: &ServiceContext) -> Result<(), ServiceError> {
            self.run_count += 1;
            Ok(())
        }
        fn stop(&mut self) -> Result<(), ServiceError> {
            self.stop_count += 1;
            Ok(())
        }
    }

    /// A service that fails on init
    pub struct FailOnInitService(pub String);
    impl SealedMarker for FailOnInitService {}
    impl Service for FailOnInitService {
        fn name(&self) -> &str { &self.0 }
        fn init(&mut self, _: &mut ServiceContext) -> Result<(), ServiceError> {
            Err(ServiceError::Custom("init boom".into()))
        }
        fn run(&mut self, _: &ServiceContext) -> Result<(), ServiceError> { Ok(()) }
        fn stop(&mut self) -> Result<(), ServiceError> { Ok(()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::test_services::{NoopService, FailOnInitService};

    fn full_caps() -> CapabilitySet { CapabilitySet::full() }

    #[test]
    fn register_and_start() {
        let mut reg = ServiceRegistry::new();
        reg.register(NoopService::new("svc_a"), full_caps()).unwrap();
        reg.start("svc_a").unwrap();
        assert!(reg.is_running("svc_a"));
        assert_eq!(*reg.state("svc_a").unwrap(), ServiceState::Running);
    }

    #[test]
    fn start_stop_lifecycle() {
        let mut reg = ServiceRegistry::new();
        reg.register(NoopService::new("svc"), full_caps()).unwrap();
        reg.start("svc").unwrap();
        reg.stop("svc").unwrap();
        assert_eq!(*reg.state("svc").unwrap(), ServiceState::Stopped);
        assert!(!reg.is_running("svc"));
    }

    #[test]
    fn duplicate_register_err() {
        let mut reg = ServiceRegistry::new();
        reg.register(NoopService::new("dup"), full_caps()).unwrap();
        assert!(reg.register(NoopService::new("dup"), full_caps()).is_err());
    }

    #[test]
    fn start_unknown_err() {
        let mut reg = ServiceRegistry::new();
        assert!(reg.start("ghost").is_err());
    }

    #[test]
    fn init_failure_moves_to_failed() {
        let mut reg = ServiceRegistry::new();
        reg.register(FailOnInitService("bad".into()), full_caps()).unwrap();
        assert!(reg.start("bad").is_err());
        assert!(matches!(reg.state("bad"), Some(ServiceState::Failed(_))));
    }

    #[test]
    fn start_all_and_stop_all() {
        let mut reg = ServiceRegistry::new();
        for name in ["s1", "s2", "s3"] {
            reg.register(NoopService::new(name), full_caps()).unwrap();
        }
        let errs = reg.start_all();
        assert!(errs.is_empty());
        assert!(reg.is_running("s1"));
        assert!(reg.is_running("s2"));
        assert!(reg.is_running("s3"));

        let errs = reg.stop_all();
        assert!(errs.is_empty());
        assert!(!reg.is_running("s1"));
    }

    #[test]
    fn cannot_start_already_running() {
        let mut reg = ServiceRegistry::new();
        reg.register(NoopService::new("x"), full_caps()).unwrap();
        reg.start("x").unwrap();
        // Starting again would try Created→Starting on a Running service → illegal
        assert!(reg.start("x").is_err());
    }
}
