// SPDX-License-Identifier: AGPL-3.0-only
// 333 modification notice (2026): Rust/WASM adaptation based on TPA design
// analysis of Puter (AGPL-3.0-only). See ../../../THIRD_PARTY_NOTICES.md.
// KG: puter-tpa-P1-service-registry-2026-04-16
// KG: puter-tpa-P2-manifest-2026-04-16
// KG: puter-tpa-P3-capability-2026-04-16
// KG: puter-tpa-P4-context-isolation-2026-04-16
// KG: puter-tpa-P5-lifecycle-2026-04-16
// KG: puter-tpa-P10-semver-2026-04-16
// KG: sprint7F-puter-quality-port-2026-04-16
//! 333 Platform Kernel — Puter Microkernel patterns ported to Rust/WASM
//!
//! Ported from Puter TPA analysis (coverage 0.88):
//!   P1  ServiceRegistry<S> sealed trait   (Microkernel 0.92)
//!   P2  Extension manifest hot-load       (modular extension loading)
//!   P3  Capability model                  (Ed25519 actor-auth 0.88)
//!   P4  Context isolation                 (scoped namespaces per service)
//!   P5  BaseService lifecycle machine     (TemplateMethod 0.88)
//!   P10 SemVer module export              (runtime version negotiation)

pub mod lifecycle;
pub mod capability;
pub mod manifest;
pub mod service;
pub mod channel;  // KG: puter-tpa-P7-event-pubsub-2026-04-16
pub mod worker;   // KG: puter-tpa-P8-worker-queue-2026-04-16
pub mod testutil; // test service helpers for integration tests
pub mod builtin;  // KG: sprint7F-kernel-platform-integration-2026-04-16

pub use lifecycle::{ServiceState, LifecycleMachine, LifecycleError};
pub use capability::{Capability, CapabilitySet, ServiceContext, CapabilityError};
pub use manifest::{SemVer, ModuleManifest, ManifestRegistry, ExtensionKind, ManifestError};
pub use service::{Service, ServiceRegistry, ServiceError};
pub use channel::{Channel, ChannelBroker, SubId};
pub use worker::{WorkerQueue, JobPriority, JobState, JobId, WorkerStats};

/// Current platform API version (all modules declare min_platform against this)
pub const PLATFORM_VERSION: SemVer = SemVer {
    major: 1,
    minor: 0,
    patch: 0,
    pre: None,
};
