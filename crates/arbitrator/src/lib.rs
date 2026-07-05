// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_ledger::{IdIssuer, IdKind, LedgerRecord, LedgerRecordKind};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

pub const ARBITRATOR_SCHEMA_VERSION: &str = "actingcommand.arbitrator.v0.1";
pub const DEFAULT_QUEUE_DEADLINE_MS: u64 = 60_000;

pub type ArbitrationResult<T> = Result<T, ArbitrationError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArbitrationError {
    InvalidRequest(String),
    NotFound(String),
    Unauthorized(String),
}

impl fmt::Display for ArbitrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRequest(message) => write!(f, "invalid arbitration request: {message}"),
            Self::NotFound(message) => write!(f, "arbitration item not found: {message}"),
            Self::Unauthorized(message) => write!(f, "arbitration unauthorized: {message}"),
        }
    }
}

impl Error for ArbitrationError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestSource {
    Cli,
    User,
    Scheduler,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestVerb {
    Observe,
    Do,
    Ensure,
    Wait,
    RunTask,
}

impl RequestVerb {
    pub fn requires_lease(self) -> bool {
        !matches!(self, Self::Observe | Self::Wait)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Observe => "observe",
            Self::Do => "do",
            Self::Ensure => "ensure",
            Self::Wait => "wait",
            Self::RunTask => "run_task",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestPriority {
    Normal,
    High,
}

impl RequestPriority {
    fn cmp_holder(self, holder: Self) -> Ordering {
        priority_rank(self).cmp(&priority_rank(holder))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestEnvelope {
    pub schema_version: String,
    pub req_id: String,
    pub source: RequestSource,
    pub instance: String,
    pub verb: RequestVerb,
    #[serde(default)]
    pub payload: Value,
    pub priority: RequestPriority,
    #[serde(default)]
    pub allow_destructive: bool,
    pub created_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_deadline_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holder_pid: Option<u32>,
}

impl RequestEnvelope {
    pub fn new(
        req_id: impl Into<String>,
        source: RequestSource,
        instance: impl Into<String>,
        verb: RequestVerb,
        payload: Value,
        created_at_ms: u64,
    ) -> Self {
        Self {
            schema_version: ARBITRATOR_SCHEMA_VERSION.to_string(),
            req_id: req_id.into(),
            source,
            instance: instance.into(),
            verb,
            payload,
            priority: RequestPriority::Normal,
            allow_destructive: false,
            created_at_ms,
            queue_deadline_ms: None,
            holder_pid: None,
        }
    }

    pub fn high_priority(mut self) -> Self {
        self.priority = RequestPriority::High;
        self
    }

    pub fn with_deadline_ms(mut self, deadline_ms: u64) -> Self {
        self.queue_deadline_ms = Some(deadline_ms);
        self
    }

    pub fn validate(&self) -> ArbitrationResult<()> {
        validate_non_empty("schema_version", &self.schema_version)?;
        validate_non_empty("req_id", &self.req_id)?;
        validate_non_empty("instance", &self.instance)?;
        if self.schema_version != ARBITRATOR_SCHEMA_VERSION {
            return Err(ArbitrationError::InvalidRequest(format!(
                "unsupported request schema_version {}",
                self.schema_version
            )));
        }
        if self.verb == RequestVerb::RunTask && payload_contains_run_task_syntax(&self.payload) {
            return Err(ArbitrationError::InvalidRequest(
                "request payloads must not contain run_task syntax; only the request verb may start a task"
                    .to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LeaseGrant {
    pub schema_version: String,
    pub lease_id: String,
    pub req_id: String,
    pub instance: String,
    pub holder: RequestSource,
    pub priority: RequestPriority,
    pub acquired_at_ms: u64,
    pub updated_at_ms: u64,
    pub preempt_requested: bool,
    pub destructive_step_active: bool,
    pub alive: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holder_pid: Option<u32>,
}

impl LeaseGrant {
    fn new(request: &RequestEnvelope, lease_id: String, now_ms: u64) -> Self {
        Self {
            schema_version: ARBITRATOR_SCHEMA_VERSION.to_string(),
            lease_id,
            req_id: request.req_id.clone(),
            instance: request.instance.clone(),
            holder: request.source,
            priority: request.priority,
            acquired_at_ms: now_ms,
            updated_at_ms: now_ms,
            preempt_requested: false,
            destructive_step_active: false,
            alive: true,
            holder_pid: request.holder_pid,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueuedRequest {
    pub request: RequestEnvelope,
    pub queued_at_ms: u64,
    pub deadline_ms: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InstanceArbitration {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holder: Option<LeaseGrant>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queued: Option<QueuedRequest>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ArbitrationDecision {
    ReadonlyAccepted {
        req_id: String,
        instance: String,
    },
    LeaseGranted {
        req_id: String,
        instance: String,
        lease: LeaseGrant,
    },
    Queued {
        req_id: String,
        instance: String,
        holder: LeaseGrant,
        deadline_ms: u64,
        hint: String,
    },
    PreemptRequested {
        req_id: String,
        instance: String,
        holder: LeaseGrant,
        queued_req_id: String,
        yield_after_destructive: bool,
        hint: String,
    },
    Rejected {
        req_id: String,
        instance: String,
        error: String,
        holder: Option<LeaseGrant>,
        queued_req_id: Option<String>,
        hint: String,
    },
    Cancelled {
        req_id: String,
        instance: String,
        reason: String,
    },
    Released {
        req_id: String,
        instance: String,
        released_lease: LeaseGrant,
        next_lease: Option<LeaseGrant>,
    },
    Reclaimed {
        req_id: String,
        instance: String,
        reclaimed_lease: LeaseGrant,
        next_lease: Option<LeaseGrant>,
    },
    DeviceDenied {
        req_id: String,
        instance: String,
        error: String,
        holder: Option<LeaseGrant>,
        hint: String,
    },
}

impl ArbitrationDecision {
    pub fn req_id(&self) -> &str {
        match self {
            Self::ReadonlyAccepted { req_id, .. }
            | Self::LeaseGranted { req_id, .. }
            | Self::Queued { req_id, .. }
            | Self::PreemptRequested { req_id, .. }
            | Self::Rejected { req_id, .. }
            | Self::Cancelled { req_id, .. }
            | Self::Released { req_id, .. }
            | Self::Reclaimed { req_id, .. }
            | Self::DeviceDenied { req_id, .. } => req_id,
        }
    }

    pub fn instance(&self) -> &str {
        match self {
            Self::ReadonlyAccepted { instance, .. }
            | Self::LeaseGranted { instance, .. }
            | Self::Queued { instance, .. }
            | Self::PreemptRequested { instance, .. }
            | Self::Rejected { instance, .. }
            | Self::Cancelled { instance, .. }
            | Self::Released { instance, .. }
            | Self::Reclaimed { instance, .. }
            | Self::DeviceDenied { instance, .. } => instance,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ReadonlyAccepted { .. } => "readonly_accepted",
            Self::LeaseGranted { .. } => "lease_granted",
            Self::Queued { .. } => "queued",
            Self::PreemptRequested { .. } => "preempt_requested",
            Self::Rejected { .. } => "rejected",
            Self::Cancelled { .. } => "cancelled",
            Self::Released { .. } => "released",
            Self::Reclaimed { .. } => "reclaimed",
            Self::DeviceDenied { .. } => "device_denied",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ArbitrationOutcome {
    pub decision: ArbitrationDecision,
    pub ledger_records: Vec<LedgerRecord>,
}

#[derive(Debug)]
pub struct DegradedArbitrator {
    issuer: IdIssuer,
    instances: BTreeMap<String, InstanceArbitration>,
}

impl DegradedArbitrator {
    pub fn new(issuer: IdIssuer) -> Self {
        Self {
            issuer,
            instances: BTreeMap::new(),
        }
    }

    pub fn from_instances(
        issuer: IdIssuer,
        instances: BTreeMap<String, InstanceArbitration>,
    ) -> Self {
        Self { issuer, instances }
    }

    pub fn instances(&self) -> &BTreeMap<String, InstanceArbitration> {
        &self.instances
    }

    pub fn snapshot(&self, instance: &str) -> InstanceArbitration {
        self.instances.get(instance).cloned().unwrap_or_default()
    }

    pub fn admit(
        &mut self,
        request: RequestEnvelope,
        now_ms: u64,
    ) -> ArbitrationResult<ArbitrationOutcome> {
        request.validate()?;
        self.expire_queued_request(&request.instance, now_ms);
        if !request.verb.requires_lease() {
            return Ok(Self::outcome(ArbitrationDecision::ReadonlyAccepted {
                req_id: request.req_id,
                instance: request.instance,
            }));
        }

        let instance = self.instances.entry(request.instance.clone()).or_default();
        let Some(holder) = instance.holder.as_mut() else {
            let lease = LeaseGrant::new(&request, self.issuer.issue(IdKind::Lease).value, now_ms);
            instance.holder = Some(lease.clone());
            return Ok(Self::outcome(ArbitrationDecision::LeaseGranted {
                req_id: request.req_id,
                instance: request.instance,
                lease,
            }));
        };
        if holder.req_id == request.req_id {
            return Ok(Self::outcome(ArbitrationDecision::LeaseGranted {
                req_id: request.req_id,
                instance: request.instance,
                lease: holder.clone(),
            }));
        }
        if let Some(queued) = &instance.queued {
            return Ok(Self::outcome(ArbitrationDecision::Rejected {
                req_id: request.req_id,
                instance: request.instance,
                error: "queue_full".to_string(),
                holder: Some(holder.clone()),
                queued_req_id: Some(queued.request.req_id.clone()),
                hint: "retry-later|escalate-priority".to_string(),
            }));
        }

        match request.priority.cmp_holder(holder.priority) {
            Ordering::Greater => {
                holder.preempt_requested = true;
                holder.updated_at_ms = now_ms;
                let queued_req_id = request.req_id.clone();
                let deadline_ms = request
                    .queue_deadline_ms
                    .unwrap_or(now_ms + DEFAULT_QUEUE_DEADLINE_MS);
                instance.queued = Some(QueuedRequest {
                    request,
                    queued_at_ms: now_ms,
                    deadline_ms,
                });
                Ok(Self::outcome(ArbitrationDecision::PreemptRequested {
                    req_id: queued_req_id.clone(),
                    instance: holder.instance.clone(),
                    holder: holder.clone(),
                    queued_req_id,
                    yield_after_destructive: holder.destructive_step_active,
                    hint: "yield-at-next-safe-boundary".to_string(),
                }))
            }
            Ordering::Less => {
                let deadline_ms = request
                    .queue_deadline_ms
                    .unwrap_or(now_ms + DEFAULT_QUEUE_DEADLINE_MS);
                let req_id = request.req_id.clone();
                let instance_id = request.instance.clone();
                instance.queued = Some(QueuedRequest {
                    request,
                    queued_at_ms: now_ms,
                    deadline_ms,
                });
                Ok(Self::outcome(ArbitrationDecision::Queued {
                    req_id,
                    instance: instance_id,
                    holder: holder.clone(),
                    deadline_ms,
                    hint: "wait-for-current-holder".to_string(),
                }))
            }
            Ordering::Equal => Ok(Self::outcome(ArbitrationDecision::Rejected {
                req_id: request.req_id,
                instance: request.instance,
                error: "lease_held".to_string(),
                holder: Some(holder.clone()),
                queued_req_id: None,
                hint: "retry-later|escalate-priority".to_string(),
            })),
        }
    }

    pub fn admit_with_existing_lease(
        &mut self,
        request: RequestEnvelope,
        lease_id: &str,
        now_ms: u64,
    ) -> ArbitrationResult<ArbitrationOutcome> {
        request.validate()?;
        let instance = request.instance.clone();
        self.expire_queued_request(&instance, now_ms);
        if !request.verb.requires_lease() {
            return Ok(Self::outcome(ArbitrationDecision::ReadonlyAccepted {
                req_id: request.req_id,
                instance,
            }));
        }

        let holder = self
            .instances
            .get(&instance)
            .and_then(|state| state.holder.clone());
        match holder {
            Some(holder) if holder.lease_id == lease_id && holder.alive => {
                Ok(Self::outcome(ArbitrationDecision::LeaseGranted {
                    req_id: request.req_id,
                    instance: request.instance,
                    lease: holder,
                }))
            }
            holder => Ok(Self::outcome(ArbitrationDecision::Rejected {
                req_id: request.req_id,
                instance: request.instance,
                error: "lease_held".to_string(),
                holder,
                queued_req_id: self
                    .instances
                    .get(&instance)
                    .and_then(|state| state.queued.as_ref())
                    .map(|queued| queued.request.req_id.clone()),
                hint: "acquire-matching-lab2-arbitrator-lease".to_string(),
            })),
        }
    }

    pub fn release(
        &mut self,
        instance: &str,
        lease_id: &str,
        now_ms: u64,
    ) -> ArbitrationResult<ArbitrationOutcome> {
        let state = self.instances.entry(instance.to_string()).or_default();
        let holder = state
            .holder
            .take()
            .ok_or_else(|| ArbitrationError::NotFound(format!("no lease for {instance}")))?;
        if holder.lease_id != lease_id {
            state.holder = Some(holder.clone());
            return Err(ArbitrationError::Unauthorized(format!(
                "lease for {instance} is {}, not {lease_id}",
                holder.lease_id
            )));
        }
        let next_lease = self.promote_queued(instance, now_ms);
        Ok(Self::outcome(ArbitrationDecision::Released {
            req_id: holder.req_id.clone(),
            instance: instance.to_string(),
            released_lease: holder,
            next_lease,
        }))
    }

    pub fn cancel_queued(
        &mut self,
        instance: &str,
        req_id: &str,
        reason: impl Into<String>,
    ) -> ArbitrationResult<ArbitrationOutcome> {
        let state = self.instances.entry(instance.to_string()).or_default();
        let Some(queued) = state.queued.as_ref() else {
            return Err(ArbitrationError::NotFound(format!(
                "no queued request for {instance}"
            )));
        };
        if queued.request.req_id != req_id {
            return Err(ArbitrationError::NotFound(format!(
                "queued request for {instance} is {}, not {req_id}",
                queued.request.req_id
            )));
        }
        state.queued = None;
        Ok(Self::outcome(ArbitrationDecision::Cancelled {
            req_id: req_id.to_string(),
            instance: instance.to_string(),
            reason: reason.into(),
        }))
    }

    pub fn mark_holder_destructive_step(&mut self, instance: &str, active: bool) {
        if let Some(holder) = self
            .instances
            .get_mut(instance)
            .and_then(|state| state.holder.as_mut())
        {
            holder.destructive_step_active = active;
        }
    }

    pub fn mark_holder_dead(&mut self, instance: &str) {
        if let Some(holder) = self
            .instances
            .get_mut(instance)
            .and_then(|state| state.holder.as_mut())
        {
            holder.alive = false;
        }
    }

    pub fn reclaim_dead_holder(
        &mut self,
        instance: &str,
        now_ms: u64,
    ) -> ArbitrationResult<ArbitrationOutcome> {
        self.reclaim_dead_holder_with_liveness(instance, now_ms, |_| true)
    }

    pub fn reclaim_dead_holder_with_liveness(
        &mut self,
        instance: &str,
        now_ms: u64,
        is_process_alive: impl FnMut(u32) -> bool,
    ) -> ArbitrationResult<ArbitrationOutcome> {
        let state = self.instances.entry(instance.to_string()).or_default();
        let holder = state
            .holder
            .take()
            .ok_or_else(|| ArbitrationError::NotFound(format!("no lease for {instance}")))?;
        let process_alive = holder.holder_pid.map(is_process_alive).unwrap_or(false);
        if holder.alive && holder.holder_pid.is_none() {
            state.holder = Some(holder.clone());
            return Err(ArbitrationError::InvalidRequest(format!(
                "lease for {instance} has no holder_pid; liveness cannot be proven by reclaim-dead"
            )));
        }
        if holder.alive && process_alive {
            state.holder = Some(holder.clone());
            return Err(ArbitrationError::InvalidRequest(format!(
                "lease for {instance} is still alive"
            )));
        }
        let next_lease = self.promote_queued(instance, now_ms);
        Ok(Self::outcome(ArbitrationDecision::Reclaimed {
            req_id: holder.req_id.clone(),
            instance: instance.to_string(),
            reclaimed_lease: holder,
            next_lease,
        }))
    }

    pub fn authorize_device_drive(
        &self,
        instance: &str,
        req_id: impl Into<String>,
        lease_id: &str,
    ) -> ArbitrationOutcome {
        let req_id = req_id.into();
        let holder = self
            .instances
            .get(instance)
            .and_then(|state| state.holder.clone());
        match holder {
            Some(holder)
                if holder.lease_id == lease_id && holder.alive && !holder.preempt_requested =>
            {
                Self::outcome(ArbitrationDecision::ReadonlyAccepted {
                    req_id,
                    instance: instance.to_string(),
                })
            }
            holder => Self::outcome(ArbitrationDecision::DeviceDenied {
                req_id,
                instance: instance.to_string(),
                error: "non_holder_device_drive".to_string(),
                holder,
                hint: "acquire-matching-lease-before-device-io".to_string(),
            }),
        }
    }

    fn expire_queued_request(&mut self, instance: &str, now_ms: u64) {
        if let Some(state) = self.instances.get_mut(instance)
            && state
                .queued
                .as_ref()
                .is_some_and(|queued| now_ms > queued.deadline_ms)
        {
            state.queued = None;
        }
    }

    fn promote_queued(&mut self, instance: &str, now_ms: u64) -> Option<LeaseGrant> {
        let state = self.instances.get_mut(instance)?;
        let queued = state.queued.take()?;
        let lease = LeaseGrant::new(
            &queued.request,
            self.issuer.issue(IdKind::Lease).value,
            now_ms,
        );
        state.holder = Some(lease.clone());
        Some(lease)
    }

    fn outcome(decision: ArbitrationDecision) -> ArbitrationOutcome {
        let dispatch_payload = json!({
            "schema_version": ARBITRATOR_SCHEMA_VERSION,
            "decision": decision.as_str(),
            "instance": decision.instance(),
            "details": decision
        });
        let receipt_payload = json!({
            "schema_version": ARBITRATOR_SCHEMA_VERSION,
            "req_id": decision.req_id(),
            "state": decision.as_str(),
            "instance": decision.instance()
        });
        ArbitrationOutcome {
            ledger_records: vec![
                LedgerRecord::new(
                    LedgerRecordKind::Dispatch,
                    Some(decision.req_id().to_string()),
                    dispatch_payload,
                ),
                LedgerRecord::new(
                    LedgerRecordKind::Receipt,
                    Some(decision.req_id().to_string()),
                    receipt_payload,
                ),
            ],
            decision,
        }
    }
}

fn priority_rank(priority: RequestPriority) -> u8 {
    match priority {
        RequestPriority::Normal => 1,
        RequestPriority::High => 2,
    }
}

fn validate_non_empty(name: &str, value: &str) -> ArbitrationResult<()> {
    if value.trim().is_empty() {
        return Err(ArbitrationError::InvalidRequest(format!(
            "{name} must not be empty"
        )));
    }
    Ok(())
}

fn payload_contains_run_task_syntax(value: &Value) -> bool {
    match value {
        Value::String(text) => text == "run_task",
        Value::Array(items) => items.iter().any(payload_contains_run_task_syntax),
        Value::Object(object) => object
            .iter()
            .any(|(key, value)| key == "run_task" || payload_contains_run_task_syntax(value)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issuer() -> IdIssuer {
        IdIssuer::with_generation(42)
    }

    fn request(
        req_id: &str,
        source: RequestSource,
        instance: &str,
        verb: RequestVerb,
    ) -> RequestEnvelope {
        RequestEnvelope::new(req_id, source, instance, verb, json!({}), 100)
    }

    fn grant(outcome: ArbitrationOutcome) -> LeaseGrant {
        match outcome.decision {
            ArbitrationDecision::LeaseGranted { lease, .. } => lease,
            other => panic!("expected lease grant, got {other:?}"),
        }
    }

    #[test]
    fn readonly_request_does_not_take_lease() {
        let mut arbitrator = DegradedArbitrator::new(issuer());
        let outcome = arbitrator
            .admit(
                request("req-1", RequestSource::Cli, "ak", RequestVerb::Observe),
                100,
            )
            .expect("admit");

        assert!(matches!(
            outcome.decision,
            ArbitrationDecision::ReadonlyAccepted { .. }
        ));
        assert!(arbitrator.snapshot("ak").holder.is_none());
        assert_eq!(outcome.ledger_records.len(), 2);
        assert_eq!(outcome.ledger_records[0].kind, LedgerRecordKind::Dispatch);
        assert_eq!(outcome.ledger_records[1].kind, LedgerRecordKind::Receipt);
    }

    #[test]
    fn idle_instance_grants_lease() {
        let mut arbitrator = DegradedArbitrator::new(issuer());
        let lease = grant(
            arbitrator
                .admit(
                    request("req-1", RequestSource::Cli, "ak", RequestVerb::Do),
                    100,
                )
                .expect("admit"),
        );

        assert_eq!(lease.lease_id, "lease-000000000000002a-1");
        assert_eq!(lease.req_id, "req-1");
        assert_eq!(arbitrator.snapshot("ak").holder.as_ref(), Some(&lease));
    }

    #[test]
    fn same_request_is_idempotent_for_existing_holder() {
        let mut arbitrator = DegradedArbitrator::new(issuer());
        let first = grant(
            arbitrator
                .admit(
                    request("req-1", RequestSource::Cli, "ak", RequestVerb::Do),
                    100,
                )
                .expect("first"),
        );
        let second = grant(
            arbitrator
                .admit(
                    request("req-1", RequestSource::Cli, "ak", RequestVerb::Do),
                    200,
                )
                .expect("second"),
        );

        assert_eq!(first, second);
    }

    #[test]
    fn lower_priority_request_queues_behind_high_priority_holder() {
        let mut arbitrator = DegradedArbitrator::new(issuer());
        arbitrator
            .admit(
                request("req-1", RequestSource::Cli, "ak", RequestVerb::Do).high_priority(),
                100,
            )
            .expect("holder");

        let outcome = arbitrator
            .admit(
                request("req-2", RequestSource::Scheduler, "ak", RequestVerb::Do),
                200,
            )
            .expect("queue");

        assert!(matches!(
            outcome.decision,
            ArbitrationDecision::Queued { .. }
        ));
        assert_eq!(
            arbitrator
                .snapshot("ak")
                .queued
                .as_ref()
                .map(|queued| queued.request.req_id.as_str()),
            Some("req-2")
        );
    }

    #[test]
    fn equal_priority_request_is_rejected_with_hint() {
        let mut arbitrator = DegradedArbitrator::new(issuer());
        arbitrator
            .admit(
                request("req-1", RequestSource::Cli, "ak", RequestVerb::Do),
                100,
            )
            .expect("holder");

        let outcome = arbitrator
            .admit(
                request("req-2", RequestSource::Scheduler, "ak", RequestVerb::Do),
                200,
            )
            .expect("reject");

        match outcome.decision {
            ArbitrationDecision::Rejected { error, hint, .. } => {
                assert_eq!(error, "lease_held");
                assert_eq!(hint, "retry-later|escalate-priority");
            }
            other => panic!("expected rejection, got {other:?}"),
        }
    }

    #[test]
    fn high_priority_request_preempts_at_safe_boundary() {
        let mut arbitrator = DegradedArbitrator::new(issuer());
        arbitrator
            .admit(
                request("req-1", RequestSource::Cli, "ak", RequestVerb::Do),
                100,
            )
            .expect("holder");
        arbitrator.mark_holder_destructive_step("ak", true);

        let outcome = arbitrator
            .admit(
                request("req-2", RequestSource::User, "ak", RequestVerb::Ensure).high_priority(),
                200,
            )
            .expect("preempt");

        match outcome.decision {
            ArbitrationDecision::PreemptRequested {
                queued_req_id,
                yield_after_destructive,
                holder,
                ..
            } => {
                assert_eq!(queued_req_id, "req-2");
                assert!(yield_after_destructive);
                assert!(holder.preempt_requested);
            }
            other => panic!("expected preempt, got {other:?}"),
        }
    }

    #[test]
    fn queue_full_rejects_third_request_with_holder_and_queued_ids() {
        let mut arbitrator = DegradedArbitrator::new(issuer());
        arbitrator
            .admit(
                request("holder", RequestSource::Cli, "ak", RequestVerb::Do).high_priority(),
                100,
            )
            .expect("holder");
        arbitrator
            .admit(
                request("queued", RequestSource::Scheduler, "ak", RequestVerb::Do),
                200,
            )
            .expect("queued");

        let outcome = arbitrator
            .admit(
                request("third", RequestSource::User, "ak", RequestVerb::Do),
                300,
            )
            .expect("third");

        match outcome.decision {
            ArbitrationDecision::Rejected {
                error,
                holder,
                queued_req_id,
                ..
            } => {
                assert_eq!(error, "queue_full");
                assert_eq!(
                    holder.as_ref().map(|lease| lease.req_id.as_str()),
                    Some("holder")
                );
                assert_eq!(queued_req_id.as_deref(), Some("queued"));
            }
            other => panic!("expected queue_full, got {other:?}"),
        }
    }

    #[test]
    fn queued_deadline_expiry_allows_new_request_after_timeout() {
        let mut arbitrator = DegradedArbitrator::new(issuer());
        arbitrator
            .admit(
                request("holder", RequestSource::Cli, "ak", RequestVerb::Do).high_priority(),
                100,
            )
            .expect("holder");
        arbitrator
            .admit(
                request("queued", RequestSource::Scheduler, "ak", RequestVerb::Do)
                    .with_deadline_ms(250),
                200,
            )
            .expect("queued");

        let outcome = arbitrator
            .admit(
                request(
                    "replacement",
                    RequestSource::Scheduler,
                    "ak",
                    RequestVerb::Do,
                ),
                300,
            )
            .expect("replacement");

        assert!(matches!(
            outcome.decision,
            ArbitrationDecision::Queued { .. }
        ));
        assert_eq!(
            arbitrator
                .snapshot("ak")
                .queued
                .as_ref()
                .map(|queued| queued.request.req_id.as_str()),
            Some("replacement")
        );
    }

    #[test]
    fn cancel_removes_queued_request() {
        let mut arbitrator = DegradedArbitrator::new(issuer());
        arbitrator
            .admit(
                request("holder", RequestSource::Cli, "ak", RequestVerb::Do).high_priority(),
                100,
            )
            .expect("holder");
        arbitrator
            .admit(
                request("queued", RequestSource::Scheduler, "ak", RequestVerb::Do),
                200,
            )
            .expect("queued");

        let outcome = arbitrator
            .cancel_queued("ak", "queued", "operator_cancel")
            .expect("cancel");

        assert!(matches!(
            outcome.decision,
            ArbitrationDecision::Cancelled { .. }
        ));
        assert!(arbitrator.snapshot("ak").queued.is_none());
    }

    #[test]
    fn release_promotes_queued_request_to_lease() {
        let mut arbitrator = DegradedArbitrator::new(issuer());
        let holder = grant(
            arbitrator
                .admit(
                    request("holder", RequestSource::Cli, "ak", RequestVerb::Do).high_priority(),
                    100,
                )
                .expect("holder"),
        );
        arbitrator
            .admit(
                request("queued", RequestSource::Scheduler, "ak", RequestVerb::Do),
                200,
            )
            .expect("queued");

        let outcome = arbitrator
            .release("ak", &holder.lease_id, 300)
            .expect("release");

        match outcome.decision {
            ArbitrationDecision::Released { next_lease, .. } => {
                assert_eq!(
                    next_lease.as_ref().map(|lease| lease.req_id.as_str()),
                    Some("queued")
                );
            }
            other => panic!("expected release, got {other:?}"),
        }
        assert_eq!(
            arbitrator
                .snapshot("ak")
                .holder
                .as_ref()
                .map(|lease| lease.req_id.as_str()),
            Some("queued")
        );
    }

    #[test]
    fn dead_holder_reclaim_promotes_queued_request() {
        let mut arbitrator = DegradedArbitrator::new(issuer());
        arbitrator
            .admit(
                request("holder", RequestSource::Cli, "ak", RequestVerb::Do).high_priority(),
                100,
            )
            .expect("holder");
        arbitrator
            .admit(
                request("queued", RequestSource::Scheduler, "ak", RequestVerb::Do),
                200,
            )
            .expect("queued");
        arbitrator.mark_holder_dead("ak");

        let outcome = arbitrator.reclaim_dead_holder("ak", 300).expect("reclaim");

        match outcome.decision {
            ArbitrationDecision::Reclaimed { next_lease, .. } => {
                assert_eq!(
                    next_lease.as_ref().map(|lease| lease.req_id.as_str()),
                    Some("queued")
                );
            }
            other => panic!("expected reclaim, got {other:?}"),
        }
    }

    #[test]
    fn reclaim_dead_holder_checks_pid_liveness_before_reclaiming() {
        let mut arbitrator = DegradedArbitrator::new(issuer());
        let mut holder = request("holder", RequestSource::Cli, "ak", RequestVerb::Do);
        holder.holder_pid = Some(12_345);
        arbitrator.admit(holder, 100).expect("holder");

        let still_alive = arbitrator
            .reclaim_dead_holder_with_liveness("ak", 200, |pid| pid == 12_345)
            .expect_err("live pid should block reclaim");
        assert!(still_alive.to_string().contains("still alive"));
        assert_eq!(
            arbitrator
                .snapshot("ak")
                .holder
                .and_then(|lease| lease.holder_pid),
            Some(12_345)
        );

        let reclaimed = arbitrator
            .reclaim_dead_holder_with_liveness("ak", 300, |_| false)
            .expect("dead pid should reclaim");
        assert!(matches!(
            reclaimed.decision,
            ArbitrationDecision::Reclaimed { .. }
        ));
        assert!(arbitrator.snapshot("ak").holder.is_none());
    }

    #[test]
    fn reclaim_dead_holder_rejects_alive_lease_without_pid() {
        let mut arbitrator = DegradedArbitrator::new(issuer());
        arbitrator
            .admit(
                request("holder", RequestSource::Cli, "ak", RequestVerb::Do),
                100,
            )
            .expect("holder");

        let err = arbitrator
            .reclaim_dead_holder_with_liveness("ak", 200, |_| false)
            .expect_err("missing pid should be honest");

        assert!(err.to_string().contains("holder_pid"));
        assert_eq!(
            arbitrator
                .snapshot("ak")
                .holder
                .as_ref()
                .map(|lease| lease.req_id.as_str()),
            Some("holder")
        );
    }

    #[test]
    fn non_holder_device_drive_is_denied() {
        let mut arbitrator = DegradedArbitrator::new(issuer());
        let holder = grant(
            arbitrator
                .admit(
                    request("holder", RequestSource::Cli, "ak", RequestVerb::Do),
                    100,
                )
                .expect("holder"),
        );

        let allowed = arbitrator.authorize_device_drive("ak", "drive-1", &holder.lease_id);
        assert!(matches!(
            allowed.decision,
            ArbitrationDecision::ReadonlyAccepted { .. }
        ));

        let denied = arbitrator.authorize_device_drive("ak", "drive-2", "wrong-lease");
        assert!(matches!(
            denied.decision,
            ArbitrationDecision::DeviceDenied { .. }
        ));
    }

    #[test]
    fn instances_are_isolated() {
        let mut arbitrator = DegradedArbitrator::new(issuer());
        arbitrator
            .admit(
                request("ak-holder", RequestSource::Cli, "ak", RequestVerb::Do),
                100,
            )
            .expect("ak holder");
        let ba = grant(
            arbitrator
                .admit(
                    request("ba-holder", RequestSource::Cli, "ba", RequestVerb::Do),
                    100,
                )
                .expect("ba holder"),
        );

        assert_eq!(ba.req_id, "ba-holder");
        assert_eq!(
            arbitrator
                .snapshot("ak")
                .holder
                .as_ref()
                .map(|lease| lease.req_id.as_str()),
            Some("ak-holder")
        );
        assert_eq!(
            arbitrator
                .snapshot("ba")
                .holder
                .as_ref()
                .map(|lease| lease.req_id.as_str()),
            Some("ba-holder")
        );
    }

    #[test]
    fn request_payload_cannot_smuggle_run_task_syntax() {
        let mut request = request("req-1", RequestSource::Cli, "ak", RequestVerb::RunTask);
        request.payload = json!({"steps": [{"run_task": "daily"}]});

        let err = request.validate().expect_err("run_task syntax should fail");
        assert!(matches!(err, ArbitrationError::InvalidRequest(_)));
    }

    #[test]
    fn ledger_records_include_dispatch_and_receipt() {
        let mut arbitrator = DegradedArbitrator::new(issuer());
        let outcome = arbitrator
            .admit(
                request("req-1", RequestSource::Cli, "ak", RequestVerb::Do),
                100,
            )
            .expect("admit");

        assert_eq!(outcome.ledger_records[0].kind, LedgerRecordKind::Dispatch);
        assert_eq!(outcome.ledger_records[0].req_id.as_deref(), Some("req-1"));
        assert_eq!(outcome.ledger_records[1].kind, LedgerRecordKind::Receipt);
        assert_eq!(
            outcome.ledger_records[0].payload["decision"],
            "lease_granted"
        );
    }
}
