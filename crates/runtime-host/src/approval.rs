// SPDX-License-Identifier: AGPL-3.0-only

//! Ledger-rebuilt approval projection used by Runtime policy admission.

use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::{
    ApprovalDecisionRecord, ApprovalPayload, ApprovalTarget, EventActor, EventPayload, EventQuery,
    EventSource, EventType, OriginModule, RuntimeErrorCode,
};
use actingcommand_ledger::GlobalLedger;
use actingcommand_policy::DispatchIntent;
use actingcommand_runtime_state::RuntimeStateStore;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

const APPROVAL_PROJECTION_NAMESPACE: &str = "approval.latest.v1";
const MAX_ACTIVE_APPROVAL_FACTS: usize = 256;
const MAX_RECENT_APPROVAL_FACTS: usize = 256;

pub(crate) struct ApprovalProjection {
    active: BTreeMap<String, ApprovalDecisionRecord>,
    recent: BTreeMap<String, (u64, ApprovalDecisionRecord)>,
    state: Arc<RuntimeStateStore>,
}

impl ApprovalProjection {
    pub(crate) fn recover(
        ledger: &GlobalLedger,
        state: Arc<RuntimeStateStore>,
    ) -> RuntimeHostResult<Self> {
        let events = ledger
            .query(EventQuery {
                event_type: Some(EventType::ApprovalDecision),
                ..EventQuery::default()
            })
            .map_err(|_| approval_fatal("approval_projection_query_failed"))?;
        let mut active = BTreeMap::<String, ApprovalDecisionRecord>::new();
        let mut recent = BTreeMap::<String, (u64, ApprovalDecisionRecord)>::new();
        let mut recent_order = BTreeMap::<u64, String>::new();
        for event in events {
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
            validate_persisted_target(&state, decision)?;
            persist_approval(&state, event.sequence(), decision)?;
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
        Ok(Self {
            active,
            recent,
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
        validate_persisted_target(&self.state, decision).map_err(|error| {
            if error.code() == "approval_target_identity_conflict" {
                approval_request("approval_target_identity_conflict")
            } else {
                error
            }
        })?;
        if decision.disposition().grants_authority()
            && !self.active.contains_key(decision.approval_id())
            && self.active.len() >= MAX_ACTIVE_APPROVAL_FACTS
        {
            return Err(approval_request("approval_projection_capacity_exceeded"));
        }
        Ok(())
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

fn validate_persisted_target(
    state: &RuntimeStateStore,
    decision: &ApprovalDecisionRecord,
) -> RuntimeHostResult<()> {
    let key = approval_projection_key(decision.approval_id());
    let Some(entry) = state
        .read_projection_entry(APPROVAL_PROJECTION_NAMESPACE, &key)
        .map_err(|error| RuntimeHostError::state(&error))?
    else {
        return Ok(());
    };
    let existing = serde_json::from_slice::<ApprovalDecisionRecord>(entry.payload())
        .map_err(|_| approval_fatal("approval_projection_payload_invalid"))?;
    existing
        .validate()
        .map_err(|_| approval_fatal("approval_projection_payload_invalid"))?;
    if existing.approval_id() != decision.approval_id() {
        return Err(approval_fatal("approval_projection_identity_mismatch"));
    }
    if existing.target() != decision.target() {
        return Err(approval_fatal("approval_target_identity_conflict"));
    }
    Ok(())
}

fn persist_approval(
    state: &RuntimeStateStore,
    sequence: u64,
    decision: &ApprovalDecisionRecord,
) -> RuntimeHostResult<()> {
    let payload = serde_json::to_vec(decision)
        .map_err(|_| approval_fatal("approval_projection_encode_failed"))?;
    state
        .write_projection_entry(
            APPROVAL_PROJECTION_NAMESPACE,
            &approval_projection_key(decision.approval_id()),
            sequence,
            &payload,
        )
        .map_err(|error| RuntimeHostError::state(&error))?;
    Ok(())
}

fn approval_projection_key(approval_id: &str) -> String {
    format!("{:x}", Sha256::digest(approval_id.as_bytes()))
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
