// KG: puter-tpa-P5-lifecycle-2026-04-16, SPAN_333_Kernel
// P5: BaseService lifecycle state machine — ported from Puter TemplateMethod(0.88) pattern
// Valid transitions: Created→Starting, Starting→Running, Starting→Failed,
//                   Running→Stopping, Stopping→Stopped, *→Failed

use std::fmt;

/// Service lifecycle states (mirrors Puter's BaseService._init/_run/_stop cycle)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ServiceState {
    /// Service constructed but not yet started
    Created,
    /// start() called, init work in progress
    Starting,
    /// Fully operational, accepting requests
    Running,
    /// stop() called, draining in-flight work
    Stopping,
    /// Cleanly stopped
    Stopped,
    /// Unrecoverable error — requires restart
    Failed(String),
}

impl fmt::Display for ServiceState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ServiceState::Created   => write!(f, "Created"),
            ServiceState::Starting  => write!(f, "Starting"),
            ServiceState::Running   => write!(f, "Running"),
            ServiceState::Stopping  => write!(f, "Stopping"),
            ServiceState::Stopped   => write!(f, "Stopped"),
            ServiceState::Failed(e) => write!(f, "Failed({})", e),
        }
    }
}

/// State machine guard — enforces legal transitions
pub struct LifecycleMachine {
    state: ServiceState,
}

impl LifecycleMachine {
    pub fn new() -> Self {
        Self { state: ServiceState::Created }
    }

    pub fn state(&self) -> &ServiceState {
        &self.state
    }

    pub fn is_running(&self) -> bool {
        self.state == ServiceState::Running
    }

    /// Attempt a transition. Returns Err if the transition is illegal.
    pub fn transition(&mut self, next: ServiceState) -> Result<(), LifecycleError> {
        let allowed = match &self.state {
            ServiceState::Created  => matches!(next, ServiceState::Starting | ServiceState::Failed(_)),
            ServiceState::Starting => matches!(next, ServiceState::Running  | ServiceState::Failed(_)),
            ServiceState::Running  => matches!(next, ServiceState::Stopping | ServiceState::Failed(_)),
            ServiceState::Stopping => matches!(next, ServiceState::Stopped  | ServiceState::Failed(_)),
            ServiceState::Stopped  => false, // terminal
            ServiceState::Failed(_)=> false, // terminal — must create new instance
        };
        if allowed {
            self.state = next;
            Ok(())
        } else {
            Err(LifecycleError::InvalidTransition {
                from: self.state.clone(),
                to: next,
            })
        }
    }
}

impl Default for LifecycleMachine {
    fn default() -> Self { Self::new() }
}

#[derive(Debug, Clone)]
pub enum LifecycleError {
    InvalidTransition { from: ServiceState, to: ServiceState },
}

impl fmt::Display for LifecycleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LifecycleError::InvalidTransition { from, to } =>
                write!(f, "illegal transition {} → {}", from, to),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn happy_path_created_to_stopped() {
        let mut m = LifecycleMachine::new();
        assert_eq!(*m.state(), ServiceState::Created);
        m.transition(ServiceState::Starting).unwrap();
        m.transition(ServiceState::Running).unwrap();
        assert!(m.is_running());
        m.transition(ServiceState::Stopping).unwrap();
        m.transition(ServiceState::Stopped).unwrap();
        assert_eq!(*m.state(), ServiceState::Stopped);
    }

    #[test]
    fn illegal_transition_created_to_running() {
        let mut m = LifecycleMachine::new();
        assert!(m.transition(ServiceState::Running).is_err());
    }

    #[test]
    fn failed_is_terminal() {
        let mut m = LifecycleMachine::new();
        m.transition(ServiceState::Starting).unwrap();
        m.transition(ServiceState::Failed("boom".into())).unwrap();
        assert!(m.transition(ServiceState::Running).is_err());
    }

    #[test]
    fn stopped_is_terminal() {
        let mut m = LifecycleMachine::new();
        m.transition(ServiceState::Starting).unwrap();
        m.transition(ServiceState::Running).unwrap();
        m.transition(ServiceState::Stopping).unwrap();
        m.transition(ServiceState::Stopped).unwrap();
        assert!(m.transition(ServiceState::Starting).is_err());
    }

    #[test]
    fn any_state_can_fail() {
        // Started → Failed
        let mut m = LifecycleMachine::new();
        m.transition(ServiceState::Starting).unwrap();
        m.transition(ServiceState::Failed("init error".into())).unwrap();
        assert!(matches!(m.state(), ServiceState::Failed(_)));
    }
}
