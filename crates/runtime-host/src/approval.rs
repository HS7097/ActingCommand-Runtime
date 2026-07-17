// SPDX-License-Identifier: AGPL-3.0-only

//! Ledger-rebuilt approval projection used by Runtime policy admission.

use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::{
    ApprovalDecisionRecord, ApprovalPayload, ApprovalTarget, EventActor, EventPayload, EventQuery,
    EventSource, EventType, OriginModule, RuntimeErrorCode,
};
use actingcommand_ledger::GlobalLedger;
use actingcommand_policy::DispatchIntent;
use std::collections::{BTreeMap, BTreeSet};

const MAX_APPROVAL_FACTS: usize = 256;

pub(crate) struct ApprovalProjection {
    latest: BTreeMap<String, ApprovalDecisionRecord>,
}

impl ApprovalProjection {
    pub(crate) fn recover(ledger: &GlobalLedger) -> RuntimeHostResult<Self> {
        let events = ledger
            .query(EventQuery {
                event_type: Some(EventType::ApprovalDecision),
                ..EventQuery::default()
            })
            .map_err(|_| approval_fatal("approval_projection_query_failed"))?;
        let mut latest = BTreeMap::<String, ApprovalDecisionRecord>::new();
        for event in events {
            if event.origin().module() != OriginModule::Governance
                || !valid_client_origin(event.origin().actor(), event.origin().source())
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
            if let Some(existing) = latest.get(decision.approval_id())
                && existing.target() != decision.target()
            {
                return Err(approval_fatal("approval_target_identity_conflict"));
            }
            if !latest.contains_key(decision.approval_id()) && latest.len() >= MAX_APPROVAL_FACTS {
                return Err(approval_fatal("approval_projection_capacity_exceeded"));
            }
            latest.insert(decision.approval_id().to_owned(), decision.clone());
        }
        Ok(Self { latest })
    }

    pub(crate) fn validate_transition(
        &self,
        decision: &ApprovalDecisionRecord,
    ) -> RuntimeHostResult<()> {
        decision
            .validate()
            .map_err(|_| approval_request("approval_decision_invalid"))?;
        if let Some(existing) = self.latest.get(decision.approval_id())
            && existing.target() != decision.target()
        {
            return Err(approval_request("approval_target_identity_conflict"));
        }
        if !self.latest.contains_key(decision.approval_id())
            && self.latest.len() >= MAX_APPROVAL_FACTS
        {
            return Err(approval_request("approval_projection_capacity_exceeded"));
        }
        Ok(())
    }

    pub(crate) fn active_for_dispatch(&self, intent: &DispatchIntent) -> BTreeSet<String> {
        self.latest
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
        self.latest
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
        self.latest
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

fn valid_client_origin(actor: EventActor, source: EventSource) -> bool {
    matches!(
        source,
        EventSource::Cli | EventSource::Ui | EventSource::Lab | EventSource::Adapter
    ) && matches!(
        actor,
        EventActor::User | EventActor::Cli | EventActor::Ui | EventActor::Lab | EventActor::Agent
    )
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
