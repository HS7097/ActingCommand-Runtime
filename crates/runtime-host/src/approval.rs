// SPDX-License-Identifier: AGPL-3.0-only

//! Ledger-rebuilt approval projection used by Runtime policy admission.

use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::{
    ApprovalDecisionRecord, ApprovalPayload, ApprovalTarget, EventActor, EventPayload, EventQuery,
    EventSource, EventType, OriginModule, RuntimeErrorCode,
};
use actingcommand_ledger::{
    GlobalLedger, GlobalLedgerError, LedgerTransactionWork, PersistedEvent,
    TransactionStateObservation, TransactionWorkError,
};
use actingcommand_policy::DispatchIntent;
use actingcommand_runtime_state::{
    ApprovalStateObservation, PreparedApprovalProjection, RuntimeStateStore,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

const MAX_ACTIVE_APPROVAL_FACTS: usize = 256;
const MAX_RECENT_APPROVAL_FACTS: usize = 256;

pub(crate) struct ApprovalProjection {
    active: BTreeMap<String, ApprovalDecisionRecord>,
    recent: BTreeMap<String, (u64, ApprovalDecisionRecord)>,
    latest: BTreeMap<String, PersistedEvent>,
    state: Arc<RuntimeStateStore>,
}

impl ApprovalProjection {
    pub(crate) fn recover(
        ledger: &GlobalLedger,
        state: Arc<RuntimeStateStore>,
    ) -> RuntimeHostResult<Self> {
        Self::project(ledger, state, None, true)
    }

    pub(crate) fn records_at(
        ledger: &GlobalLedger,
        state: Arc<RuntimeStateStore>,
        ledger_position: u64,
    ) -> RuntimeHostResult<Vec<ApprovalDecisionRecord>> {
        if ledger_position == 0 {
            return Err(approval_fatal("approval_projection_position_invalid"));
        }
        Ok(Self::project(ledger, state, Some(ledger_position), false)?.records())
    }

    fn project(
        ledger: &GlobalLedger,
        state: Arc<RuntimeStateStore>,
        to_sequence: Option<u64>,
        persist: bool,
    ) -> RuntimeHostResult<Self> {
        let events = ledger
            .query(EventQuery {
                event_type: Some(EventType::ApprovalDecision),
                to_sequence,
                ..EventQuery::default()
            })
            .map_err(|_| approval_fatal("approval_projection_query_failed"))?;
        let mut active = BTreeMap::<String, ApprovalDecisionRecord>::new();
        let mut recent = BTreeMap::<String, (u64, ApprovalDecisionRecord)>::new();
        let mut recent_order = BTreeMap::<u64, String>::new();
        let mut latest = BTreeMap::<String, PersistedEvent>::new();
        for event in &events {
            if event.origin().module() != OriginModule::Governance
                || event.origin().actor() != EventActor::User
                || event.origin().source() != EventSource::Ui
            {
                return Err(approval_fatal("approval_projection_origin_invalid"));
            }
            let EventPayload::Approval(ApprovalPayload::Decision(payload)) = event.payload() else {
                return Err(approval_fatal("approval_projection_payload_mismatch"));
            };
            let decision = payload.decision();
            decision
                .validate()
                .map_err(|_| approval_fatal("approval_projection_record_invalid"))?;
            if let Some(previous) = latest.get(decision.approval_id()) {
                let EventPayload::Approval(ApprovalPayload::Decision(payload)) = previous.payload()
                else {
                    return Err(approval_fatal("approval_projection_payload_mismatch"));
                };
                if payload.decision().target() != decision.target() {
                    return Err(approval_fatal("approval_target_identity_conflict"));
                }
            }
            latest.insert(decision.approval_id().to_owned(), event.clone());
            if decision.disposition().grants_authority() {
                active.insert(decision.approval_id().to_owned(), decision.clone());
            } else {
                active.remove(decision.approval_id());
            }
            if let Some((previous_sequence, _)) = recent.insert(
                decision.approval_id().to_owned(),
                (event.sequence(), decision.clone()),
            ) {
                recent_order.remove(&previous_sequence);
            }
            recent_order.insert(event.sequence(), decision.approval_id().to_owned());
            while recent.len() > MAX_RECENT_APPROVAL_FACTS {
                let Some((sequence, approval_id)) = recent_order.pop_first() else {
                    return Err(approval_fatal("approval_projection_order_invalid"));
                };
                if recent
                    .get(&approval_id)
                    .is_some_and(|(latest_sequence, _)| *latest_sequence == sequence)
                {
                    recent.remove(&approval_id);
                }
            }
        }
        if active.len() > MAX_ACTIVE_APPROVAL_FACTS {
            return Err(approval_fatal("approval_projection_capacity_exceeded"));
        }
        if persist {
            state
                .recover_approval_projections(&events)
                .map_err(approval_state_error)?;
        }
        Ok(Self {
            active,
            recent,
            latest,
            state,
        })
    }

    pub(crate) fn validate_transition(
        &self,
        decision: &ApprovalDecisionRecord,
    ) -> RuntimeHostResult<()> {
        decision
            .validate()
            .map_err(|_| approval_request("approval_decision_invalid"))?;
        if let Some(previous) = self.latest.get(decision.approval_id()) {
            let EventPayload::Approval(ApprovalPayload::Decision(payload)) = previous.payload()
            else {
                return Err(approval_fatal("approval_projection_payload_mismatch"));
            };
            if payload.decision().target() != decision.target() {
                return Err(approval_request("approval_target_identity_conflict"));
            }
        }
        if decision.disposition().grants_authority()
            && !self.active.contains_key(decision.approval_id())
            && self.active.len() >= MAX_ACTIVE_APPROVAL_FACTS
        {
            return Err(approval_request("approval_projection_capacity_exceeded"));
        }
        Ok(())
    }

    pub(crate) fn prepare_decision(
        &self,
        decision: &ApprovalDecisionRecord,
    ) -> RuntimeHostResult<Box<dyn LedgerTransactionWork>> {
        self.state
            .prepare_approval_projection(decision, self.latest.get(decision.approval_id()))
            .map(|state| Box::new(ApprovalTransaction { state }) as Box<dyn LedgerTransactionWork>)
            .map_err(approval_state_error)
    }

    pub(crate) fn records(&self) -> Vec<ApprovalDecisionRecord> {
        let mut records = self.active.clone();
        records.extend(
            self.recent
                .iter()
                .map(|(approval_id, (_, decision))| (approval_id.clone(), decision.clone())),
        );
        records.into_values().collect()
    }

    pub(crate) fn active_for_dispatch(&self, intent: &DispatchIntent) -> BTreeSet<String> {
        self.active
            .values()
            .filter(|decision| {
                decision.disposition().grants_authority()
                    && target_matches_dispatch(decision.target(), intent)
            })
            .map(|decision| decision.approval_id().to_owned())
            .collect()
    }

    pub(crate) fn active_for_plan(
        &self,
        plan_id: &str,
        catalog_hash: &str,
        catalog_version: u64,
    ) -> BTreeSet<String> {
        self.active
            .values()
            .filter(|decision| {
                decision.disposition().grants_authority()
                    && matches!(
                        decision.target(),
                        ApprovalTarget::Plan {
                            plan_id: target_plan,
                            catalog_hash: target_hash,
                            catalog_version: target_version,
                        } if target_plan == plan_id
                            && target_hash == catalog_hash
                            && *target_version == catalog_version
                    )
            })
            .map(|decision| decision.approval_id().to_owned())
            .collect()
    }

    pub(crate) fn active_for_catalog(
        &self,
        catalog_hash: &str,
        catalog_version: u64,
    ) -> BTreeSet<String> {
        self.active
            .values()
            .filter(|decision| {
                decision.disposition().grants_authority()
                    && matches!(
                        decision.target(),
                        ApprovalTarget::Catalog {
                            catalog_hash: target_hash,
                            catalog_version: target_version,
                        } if target_hash == catalog_hash && *target_version == catalog_version
                    )
            })
            .map(|decision| decision.approval_id().to_owned())
            .collect()
    }
}

struct ApprovalTransaction {
    state: PreparedApprovalProjection,
}

impl LedgerTransactionWork for ApprovalTransaction {
    fn apply(
        &self,
        transaction: &actingcommand_runtime_database::RuntimeTransaction<'_, '_>,
        event: &PersistedEvent,
    ) -> Result<(), TransactionWorkError> {
        self.state
            .apply(transaction, event)
            .map_err(approval_work_error)
    }

    fn observe(
        &self,
        transaction: &actingcommand_runtime_database::RuntimeTransaction<'_, '_>,
    ) -> Result<TransactionStateObservation, TransactionWorkError> {
        self.state
            .observe(transaction)
            .map(|observation| match observation {
                ApprovalStateObservation::Applied => TransactionStateObservation::Applied,
                ApprovalStateObservation::Unchanged => TransactionStateObservation::Unchanged,
                ApprovalStateObservation::Unknown => TransactionStateObservation::Unknown,
            })
            .map_err(approval_work_error)
    }
}

fn approval_state_error(error: actingcommand_runtime_state::RuntimeStateError) -> RuntimeHostError {
    RuntimeHostError::state(&error).with_native_detail(error.to_string())
}

fn approval_work_error(
    error: actingcommand_runtime_state::RuntimeStateError,
) -> TransactionWorkError {
    TransactionWorkError {
        code: error.code(),
        operation: error.operation(),
        fatal: error.is_fatal(),
        detail: error.to_string(),
    }
}

pub(crate) fn approval_transaction_error(error: GlobalLedgerError) -> RuntimeHostError {
    if let Some(work) = error.rolled_back_work() {
        let mapped = if work.fatal {
            RuntimeHostError::fatal(work.code, work.operation, RuntimeErrorCode::RuntimeFatal)
        } else {
            RuntimeHostError::request(work.code, work.operation, RuntimeErrorCode::InvalidRequest)
        };
        mapped.with_native_detail(work.detail.clone())
    } else {
        RuntimeHostError::fatal(
            error.code(),
            error.operation(),
            RuntimeErrorCode::LedgerFailure,
        )
        .with_native_detail(format!("{error}; detail={:?}", error.detail()))
    }
}

fn target_matches_dispatch(target: &ApprovalTarget, intent: &DispatchIntent) -> bool {
    if target.catalog_hash() != intent.catalog_hash
        || target.catalog_version() != intent.catalog_version
    {
        return false;
    }
    match target {
        ApprovalTarget::Catalog { .. } => true,
        ApprovalTarget::Decision { decision_id, .. } => decision_id == &intent.decision_id,
        ApprovalTarget::Plan { .. } => false,
    }
}

fn approval_request(code: &'static str) -> RuntimeHostError {
    RuntimeHostError::request(
        code,
        "record_approval_decision",
        RuntimeErrorCode::InvalidRequest,
    )
}

fn approval_fatal(code: &'static str) -> RuntimeHostError {
    RuntimeHostError::fatal(
        code,
        "rebuild_approval_projection",
        RuntimeErrorCode::RuntimeFatal,
    )
}
