// SPDX-License-Identifier: AGPL-3.0-only

//! DEPRECATED PROTOTYPE: this scheduler gate is not connected to the active
//! SessionLease path in `apps/actinglab`. Lab-2 L2 uses the session lease model
//! and the independent arbitrator crate instead of this parallel prototype.

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;

pub const DEFAULT_LAB_MAX_WAIT_SECONDS: u64 = 600;
pub const DEFAULT_LAB_POST_RELEASE_COOLDOWN_SECONDS: u64 = 5;

pub type ActingLabResult<T> = Result<T, ActingLabError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActingLabError {
    InvalidRequest(String),
}

impl fmt::Display for ActingLabError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRequest(message) => write!(f, "invalid ActingLab request: {message}"),
        }
    }
}

impl Error for ActingLabError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabMode {
    ExclusiveDrain,
    PassiveMirror,
    SchedulerNoop,
}

impl LabMode {
    pub fn click_policy(self) -> LabClickPolicy {
        match self {
            Self::ExclusiveDrain => LabClickPolicy::NavigationOnlyOnly,
            Self::PassiveMirror | Self::SchedulerNoop => LabClickPolicy::NoClick,
        }
    }

    pub fn defers_scheduler_tasks(self) -> bool {
        match self {
            Self::ExclusiveDrain | Self::SchedulerNoop => true,
            Self::PassiveMirror => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceScope {
    Selected,
    AffectedByOperation,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeferPolicy {
    DeferUntilLabRelease,
    DeferByLabDuration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabClickPolicy {
    NavigationOnlyOnly,
    NoClick,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabLeaseState {
    Idle,
    LabRequested,
    DrainingCurrentTask,
    LeaseAcquired,
    LabActive,
    Releasing,
    SchedulerRestored,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabLeaseRequest {
    pub lease_id: String,
    pub mode: LabMode,
    pub instance_scope: InstanceScope,
    pub instance_ids: Vec<String>,
    pub max_wait_seconds: u64,
    pub defer_policy: DeferPolicy,
    pub restore_scheduler_on_exit: bool,
    pub click_policy: LabClickPolicy,
}

impl LabLeaseRequest {
    pub fn new(
        lease_id: impl Into<String>,
        mode: LabMode,
        instance_scope: InstanceScope,
        instance_ids: Vec<String>,
    ) -> Self {
        Self {
            lease_id: lease_id.into(),
            mode,
            instance_scope,
            instance_ids,
            max_wait_seconds: DEFAULT_LAB_MAX_WAIT_SECONDS,
            defer_policy: DeferPolicy::DeferUntilLabRelease,
            restore_scheduler_on_exit: true,
            click_policy: mode.click_policy(),
        }
    }

    pub fn validate(&self) -> ActingLabResult<()> {
        validate_non_empty("lease_id", &self.lease_id)?;
        if self.max_wait_seconds == 0 {
            return Err(ActingLabError::InvalidRequest(
                "max_wait_seconds must be greater than zero".to_string(),
            ));
        }
        if self.click_policy != self.mode.click_policy() {
            return Err(ActingLabError::InvalidRequest(format!(
                "click_policy {:?} does not match lab mode {:?}",
                self.click_policy, self.mode
            )));
        }
        match self.instance_scope {
            InstanceScope::Selected | InstanceScope::AffectedByOperation => {
                validate_instance_ids(&self.instance_ids)?;
            }
            InstanceScope::All => {
                if self.instance_ids.iter().any(|id| id.trim().is_empty()) {
                    return Err(ActingLabError::InvalidRequest(
                        "all scope instance ids must not contain empty values".to_string(),
                    ));
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchedulerTaskState {
    Idle,
    Running {
        task_id: String,
        executing_device_actions: bool,
    },
    ManualReviewBlocked {
        reason: String,
    },
}

impl SchedulerTaskState {
    fn is_running(&self) -> bool {
        matches!(self, Self::Running { .. })
    }

    fn is_blocked(&self) -> bool {
        matches!(self, Self::ManualReviewBlocked { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulerInstanceSnapshot {
    pub instance_id: String,
    pub task_state: SchedulerTaskState,
}

impl SchedulerInstanceSnapshot {
    pub fn idle(instance_id: impl Into<String>) -> Self {
        Self {
            instance_id: instance_id.into(),
            task_state: SchedulerTaskState::Idle,
        }
    }

    pub fn running(
        instance_id: impl Into<String>,
        task_id: impl Into<String>,
        executing_device_actions: bool,
    ) -> Self {
        Self {
            instance_id: instance_id.into(),
            task_state: SchedulerTaskState::Running {
                task_id: task_id.into(),
                executing_device_actions,
            },
        }
    }

    pub fn manual_review_blocked(
        instance_id: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            instance_id: instance_id.into(),
            task_state: SchedulerTaskState::ManualReviewBlocked {
                reason: reason.into(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulerGateSnapshot {
    pub instances: Vec<SchedulerInstanceSnapshot>,
}

impl SchedulerGateSnapshot {
    pub fn new(instances: Vec<SchedulerInstanceSnapshot>) -> Self {
        Self { instances }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabLeaseDecision {
    pub lease_id: String,
    pub state: LabLeaseState,
    pub click_policy: LabClickPolicy,
    pub scoped_instance_ids: Vec<String>,
    pub draining_instance_ids: Vec<String>,
    pub blocked_instance_ids: Vec<String>,
    pub defer_next_tasks: bool,
    pub can_click: bool,
}

pub struct SchedulerGate;

impl SchedulerGate {
    pub fn evaluate(
        request: &LabLeaseRequest,
        snapshot: &SchedulerGateSnapshot,
    ) -> ActingLabResult<LabLeaseDecision> {
        request.validate()?;
        let scoped = resolve_scope(request, snapshot)?;
        if request.mode == LabMode::PassiveMirror {
            return Ok(LabLeaseDecision {
                lease_id: request.lease_id.clone(),
                state: LabLeaseState::LabActive,
                click_policy: request.click_policy,
                scoped_instance_ids: scoped,
                draining_instance_ids: Vec::new(),
                blocked_instance_ids: Vec::new(),
                defer_next_tasks: false,
                can_click: false,
            });
        }

        let by_id = instance_map(snapshot);
        let mut draining = Vec::new();
        let mut blocked = Vec::new();

        for instance_id in &scoped {
            let instance = by_id.get(instance_id.as_str()).ok_or_else(|| {
                ActingLabError::InvalidRequest(format!(
                    "scoped instance is not present in scheduler snapshot: {instance_id}"
                ))
            })?;
            if instance.task_state.is_blocked() {
                blocked.push(instance_id.clone());
            } else if instance.task_state.is_running() {
                draining.push(instance_id.clone());
            }
        }

        let state = if !blocked.is_empty() {
            LabLeaseState::Failed
        } else if !draining.is_empty() {
            LabLeaseState::DrainingCurrentTask
        } else {
            LabLeaseState::LeaseAcquired
        };
        let can_click = state == LabLeaseState::LeaseAcquired
            && request.click_policy == LabClickPolicy::NavigationOnlyOnly;

        Ok(LabLeaseDecision {
            lease_id: request.lease_id.clone(),
            state,
            click_policy: request.click_policy,
            scoped_instance_ids: scoped,
            draining_instance_ids: draining,
            blocked_instance_ids: blocked,
            defer_next_tasks: request.mode.defers_scheduler_tasks(),
            can_click,
        })
    }
}

fn resolve_scope(
    request: &LabLeaseRequest,
    snapshot: &SchedulerGateSnapshot,
) -> ActingLabResult<Vec<String>> {
    let scoped = match request.instance_scope {
        InstanceScope::Selected | InstanceScope::AffectedByOperation => {
            request.instance_ids.clone()
        }
        InstanceScope::All => {
            if request.instance_ids.is_empty() {
                snapshot
                    .instances
                    .iter()
                    .map(|instance| instance.instance_id.clone())
                    .collect()
            } else {
                request.instance_ids.clone()
            }
        }
    };
    validate_instance_ids(&scoped)?;
    Ok(scoped)
}

fn instance_map(snapshot: &SchedulerGateSnapshot) -> HashMap<&str, &SchedulerInstanceSnapshot> {
    snapshot
        .instances
        .iter()
        .map(|instance| (instance.instance_id.as_str(), instance))
        .collect()
}

fn validate_instance_ids(instance_ids: &[String]) -> ActingLabResult<()> {
    if instance_ids.is_empty() {
        return Err(ActingLabError::InvalidRequest(
            "instance_ids must not be empty for the requested scope".to_string(),
        ));
    }
    let mut seen = HashSet::new();
    for instance_id in instance_ids {
        validate_non_empty("instance_id", instance_id)?;
        if !seen.insert(instance_id) {
            return Err(ActingLabError::InvalidRequest(format!(
                "duplicate instance id: {instance_id}"
            )));
        }
    }
    Ok(())
}

fn validate_non_empty(label: &str, value: &str) -> ActingLabResult<()> {
    if value.trim().is_empty() {
        Err(ActingLabError::InvalidRequest(format!(
            "{label} must not be empty"
        )))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclusive_drain_acquires_idle_selected_instances() {
        let request = LabLeaseRequest::new(
            "lease-1",
            LabMode::ExclusiveDrain,
            InstanceScope::Selected,
            vec!["ba-jp".to_string()],
        );
        let snapshot = SchedulerGateSnapshot::new(vec![SchedulerInstanceSnapshot::idle("ba-jp")]);

        let decision = SchedulerGate::evaluate(&request, &snapshot).expect("decision");

        assert_eq!(decision.state, LabLeaseState::LeaseAcquired);
        assert_eq!(decision.scoped_instance_ids, vec!["ba-jp"]);
        assert!(decision.defer_next_tasks);
        assert!(decision.can_click);
    }

    #[test]
    fn exclusive_drain_waits_for_running_task_without_clicking() {
        let request = LabLeaseRequest::new(
            "lease-1",
            LabMode::ExclusiveDrain,
            InstanceScope::Selected,
            vec!["ba-jp".to_string()],
        );
        let snapshot = SchedulerGateSnapshot::new(vec![SchedulerInstanceSnapshot::running(
            "ba-jp",
            "daily-task",
            true,
        )]);

        let decision = SchedulerGate::evaluate(&request, &snapshot).expect("decision");

        assert_eq!(decision.state, LabLeaseState::DrainingCurrentTask);
        assert_eq!(decision.draining_instance_ids, vec!["ba-jp"]);
        assert!(decision.defer_next_tasks);
        assert!(!decision.can_click);
    }

    #[test]
    fn passive_mirror_observes_without_click_or_defer() {
        let request = LabLeaseRequest::new(
            "lease-1",
            LabMode::PassiveMirror,
            InstanceScope::Selected,
            vec!["ba-jp".to_string()],
        );
        let snapshot = SchedulerGateSnapshot::new(vec![SchedulerInstanceSnapshot::running(
            "ba-jp",
            "daily-task",
            true,
        )]);

        let decision = SchedulerGate::evaluate(&request, &snapshot).expect("decision");

        assert_eq!(decision.state, LabLeaseState::LabActive);
        assert_eq!(decision.click_policy, LabClickPolicy::NoClick);
        assert!(!decision.defer_next_tasks);
        assert!(!decision.can_click);
        assert!(decision.draining_instance_ids.is_empty());
    }

    #[test]
    fn scheduler_noop_defers_scoped_tasks_but_never_clicks() {
        let request = LabLeaseRequest::new(
            "lease-1",
            LabMode::SchedulerNoop,
            InstanceScope::Selected,
            vec!["ba-jp".to_string()],
        );
        let snapshot = SchedulerGateSnapshot::new(vec![SchedulerInstanceSnapshot::idle("ba-jp")]);

        let decision = SchedulerGate::evaluate(&request, &snapshot).expect("decision");

        assert_eq!(decision.state, LabLeaseState::LeaseAcquired);
        assert_eq!(decision.click_policy, LabClickPolicy::NoClick);
        assert!(decision.defer_next_tasks);
        assert!(!decision.can_click);
    }

    #[test]
    fn selected_scope_requires_instances() {
        let request = LabLeaseRequest::new(
            "lease-1",
            LabMode::ExclusiveDrain,
            InstanceScope::Selected,
            Vec::new(),
        );
        let snapshot = SchedulerGateSnapshot::new(Vec::new());

        let err = SchedulerGate::evaluate(&request, &snapshot).expect_err("invalid scope");

        assert!(err.to_string().contains("instance_ids"));
    }

    #[test]
    fn unknown_scoped_instance_is_fatal() {
        let request = LabLeaseRequest::new(
            "lease-1",
            LabMode::ExclusiveDrain,
            InstanceScope::Selected,
            vec!["missing".to_string()],
        );
        let snapshot = SchedulerGateSnapshot::new(vec![SchedulerInstanceSnapshot::idle("ba-jp")]);

        let err = SchedulerGate::evaluate(&request, &snapshot).expect_err("missing instance");

        assert!(err.to_string().contains("not present"));
    }

    #[test]
    fn all_scope_resolves_snapshot_instances() {
        let request = LabLeaseRequest::new(
            "lease-1",
            LabMode::PassiveMirror,
            InstanceScope::All,
            Vec::new(),
        );
        let snapshot = SchedulerGateSnapshot::new(vec![
            SchedulerInstanceSnapshot::idle("azur-jp"),
            SchedulerInstanceSnapshot::idle("ba-jp"),
        ]);

        let decision = SchedulerGate::evaluate(&request, &snapshot).expect("decision");

        assert_eq!(decision.scoped_instance_ids, vec!["azur-jp", "ba-jp"]);
    }

    #[test]
    fn manual_review_blocked_instance_fails_lease() {
        let request = LabLeaseRequest::new(
            "lease-1",
            LabMode::ExclusiveDrain,
            InstanceScope::Selected,
            vec!["ba-jp".to_string()],
        );
        let snapshot =
            SchedulerGateSnapshot::new(vec![SchedulerInstanceSnapshot::manual_review_blocked(
                "ba-jp",
                "restore failed",
            )]);

        let decision = SchedulerGate::evaluate(&request, &snapshot).expect("decision");

        assert_eq!(decision.state, LabLeaseState::Failed);
        assert_eq!(decision.blocked_instance_ids, vec!["ba-jp"]);
        assert!(!decision.can_click);
    }

    #[test]
    fn request_rejects_click_policy_mismatch() {
        let mut request = LabLeaseRequest::new(
            "lease-1",
            LabMode::PassiveMirror,
            InstanceScope::Selected,
            vec!["ba-jp".to_string()],
        );
        request.click_policy = LabClickPolicy::NavigationOnlyOnly;
        let snapshot = SchedulerGateSnapshot::new(vec![SchedulerInstanceSnapshot::idle("ba-jp")]);

        let err = SchedulerGate::evaluate(&request, &snapshot).expect_err("policy mismatch");

        assert!(err.to_string().contains("click_policy"));
    }
}
