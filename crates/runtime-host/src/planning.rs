// SPDX-License-Identifier: AGPL-3.0-only

//! Ledger evidence adapter for predictive maintenance assessments.

use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::{
    EventPayload, EventQuery, FactPayload, FactScope, PolicyPayload, RuntimeErrorCode,
};
use actingcommand_ledger::GlobalLedger;
use actingcommand_policy::{
    ConfidenceEvidence, DurationEvidence, MAX_MAINTENANCE_SAMPLES, MaintenanceEvidence,
    MaintenanceTrendPolicy,
};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaintenanceLedgerQuery {
    instance_id: String,
    task_id: String,
    fact_scope: FactScope,
    fact_key: String,
    as_of_unix_ms: u64,
    trend_policy: MaintenanceTrendPolicy,
}

impl MaintenanceLedgerQuery {
    pub fn new(
        instance_id: impl Into<String>,
        task_id: impl Into<String>,
        fact_scope: FactScope,
        fact_key: impl Into<String>,
        as_of_unix_ms: u64,
        trend_policy: MaintenanceTrendPolicy,
    ) -> RuntimeHostResult<Self> {
        let query = Self {
            instance_id: instance_id.into(),
            task_id: task_id.into(),
            fact_scope,
            fact_key: fact_key.into(),
            as_of_unix_ms,
            trend_policy,
        };
        query.validate()?;
        Ok(query)
    }

    pub(crate) fn validate(&self) -> RuntimeHostResult<()> {
        if self.instance_id.is_empty()
            || self.task_id.is_empty()
            || self.fact_key.is_empty()
            || self.instance_id.len() > 512
            || self.task_id.len() > 512
            || self.fact_key.len() > 512
            || self
                .instance_id
                .chars()
                .chain(self.task_id.chars())
                .chain(self.fact_key.chars())
                .any(char::is_control)
            || self.as_of_unix_ms == 0
            || self.fact_scope.validate().is_err()
            || matches!(
                &self.fact_scope,
                FactScope::Instance { instance_id } if instance_id != &self.instance_id
            )
            || self.trend_policy.validate().is_err()
        {
            return Err(request("maintenance_ledger_query_invalid"));
        }
        Ok(())
    }

    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    pub fn task_id(&self) -> &str {
        &self.task_id
    }

    pub fn fact_scope(&self) -> &FactScope {
        &self.fact_scope
    }

    pub fn fact_key(&self) -> &str {
        &self.fact_key
    }

    pub const fn as_of_unix_ms(&self) -> u64 {
        self.as_of_unix_ms
    }

    pub const fn trend_policy(&self) -> MaintenanceTrendPolicy {
        self.trend_policy
    }

    pub fn subject_id(&self) -> String {
        let (scope_kind, scope_id) = match &self.fact_scope {
            FactScope::Instance { instance_id } => ("instance", instance_id.as_str()),
            FactScope::Server { server_id } => ("server", server_id.as_str()),
            FactScope::Game { game_id } => ("game", game_id.as_str()),
        };
        let mut hasher = Sha256::new();
        hasher.update(b"actingcommand-maintenance-subject-v1");
        for part in [
            self.instance_id.as_str(),
            self.task_id.as_str(),
            scope_kind,
            scope_id,
            self.fact_key.as_str(),
        ] {
            hasher.update((part.len() as u64).to_be_bytes());
            hasher.update(part.as_bytes());
        }
        format!("maintenance:{:x}", hasher.finalize())
    }
}

pub(crate) fn collect_maintenance_evidence(
    ledger: &GlobalLedger,
    query: &MaintenanceLedgerQuery,
) -> RuntimeHostResult<MaintenanceEvidence> {
    query.validate()?;
    let window_start = query
        .as_of_unix_ms
        .saturating_sub(query.trend_policy.lookback_ms);
    let events = ledger
        .query(EventQuery::default())
        .map_err(|_| fatal("maintenance_ledger_query_failed"))?;
    let mut durations = Vec::new();
    let mut confidences = Vec::new();
    for event in events {
        match event.payload() {
            EventPayload::Policy(PolicyPayload::ExecutionRecorded(payload))
                if payload.instance_id() == query.instance_id()
                    && payload.task_id() == query.task_id()
                    && payload.observed_at_unix_ms() >= window_start
                    && payload.observed_at_unix_ms() <= query.as_of_unix_ms() =>
            {
                let duration_ms = match payload.outcome() {
                    actingcommand_contract::PolicyExecutionOutcome::Succeeded { runtime_ms } => {
                        *runtime_ms
                    }
                    actingcommand_contract::PolicyExecutionOutcome::Failed { failure } => {
                        failure.runtime_ms
                    }
                };
                durations.push(DurationEvidence {
                    ledger_sequence: event.sequence(),
                    observed_at_unix_ms: payload.observed_at_unix_ms(),
                    duration_ms,
                });
                if durations.len() > MAX_MAINTENANCE_SAMPLES {
                    return Err(request("maintenance_evidence_capacity_exceeded"));
                }
            }
            EventPayload::Fact(FactPayload::Published(payload)) => {
                let record = payload.record();
                if &record.scope == query.fact_scope()
                    && record.key == query.fact_key()
                    && record.observed_at_unix_ms >= window_start
                    && record.observed_at_unix_ms <= query.as_of_unix_ms
                {
                    confidences.push(ConfidenceEvidence {
                        ledger_sequence: event.sequence(),
                        observed_at_unix_ms: record.observed_at_unix_ms,
                        confidence_milli: record.confidence_milli,
                    });
                    if confidences.len() > MAX_MAINTENANCE_SAMPLES {
                        return Err(request("maintenance_evidence_capacity_exceeded"));
                    }
                }
            }
            _ => {}
        }
    }
    Ok(MaintenanceEvidence {
        subject_id: query.subject_id(),
        as_of_unix_ms: query.as_of_unix_ms,
        durations,
        confidences,
    })
}

fn request(code: &'static str) -> RuntimeHostError {
    RuntimeHostError::request(
        code,
        "assess_predictive_maintenance",
        RuntimeErrorCode::InvalidRequest,
    )
}

fn fatal(code: &'static str) -> RuntimeHostError {
    RuntimeHostError::fatal(
        code,
        "assess_predictive_maintenance",
        RuntimeErrorCode::RuntimeFatal,
    )
}
