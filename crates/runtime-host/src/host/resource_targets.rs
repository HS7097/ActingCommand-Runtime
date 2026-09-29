// SPDX-License-Identifier: AGPL-3.0-only

//! The formal entry of instance resource target policies (Workflow #308 RT-S1a): parse and
//! check an `actingcommand.resource-targets.v1` document against the active catalog and the
//! authoritative projection, then store it as the instance fact `session.resource_targets`.

use super::facts::{FactPublication, FactPublicationPurpose};
use super::*;
use actingcommand_contract::{
    FactObservation, FactTtlPolicy, FactTtlSource, MAX_FACT_TTL_MS, MIN_FACT_TTL_MS,
    RESOURCE_TARGETS_FACT_KEY, ResourceTargetsApplied, ResourceTargetsRejection,
    ResourceTargetsRejectionReason,
};
use actingcommand_policy::{ResourceTargetsError, check_resource_targets, parse_resource_targets};

const RESOURCE_TARGETS_OPERATION: &str = "apply_resource_targets";
const RESOURCE_TARGETS_DETECTOR: &str = "runtime.resource-targets";
const RESOURCE_TARGETS_FACT_SCHEMA: &str = "fact.v1";

impl HostShared {
    /// `ApplyResourceTargets`. The clock is sampled before any lock and the document parsed
    /// without one. Phase A takes the policy outcome gate, the policy and the fact write gate
    /// in the order of `project_policy_input_identity`, checks the document against the
    /// active catalog and the authoritative projection and releases every lock. Phase B
    /// publishes the record through `publish_facts`, which replays an equal active policy.
    /// A refused document records nothing and keeps the active policy.
    pub(super) fn apply_resource_targets(
        &self,
        validated: &ValidatedRuntimeRequest<'_>,
        document_json: &str,
    ) -> Result<OperationSuccess, RequestFailure> {
        self.apply_resource_targets_document(validated, document_json)
            .map_err(planning_request_failure)
    }

    fn apply_resource_targets_document(
        &self,
        validated: &ValidatedRuntimeRequest<'_>,
        document_json: &str,
    ) -> RuntimeHostResult<OperationSuccess> {
        let sample = self.runtime_clock_sample()?;
        let time = EvaluationTime {
            unix_ms: sample.unix_ms,
            monotonic_ms: sample.monotonic_ms,
        };
        let parsed =
            parse_resource_targets(document_json.as_bytes()).map_err(resource_targets_error)?;
        let (checked, checked_catalog_hash, conditions_at_position) = {
            let _outcome_gate = lock(
                &self.policy_outcome_gate,
                "apply_resource_targets_outcome_state",
            )?;
            let mut policy = lock(&self.policy, "apply_resource_targets_outcome_keys")?;
            let outcome_keys = policy.outcome_key_snapshot()?;
            let _fact_gate = lock(&self.fact_write_gate, "apply_resource_targets_facts")?;
            let (facts, _) = self.project_authoritative_policy_inputs_under_gate(
                &mut policy,
                RESOURCE_TARGETS_OPERATION,
                &outcome_keys,
                None,
            )?;
            let catalog = policy.active_loaded().ok_or_else(|| {
                RuntimeHostError::resource_targets_rejected(ResourceTargetsRejection {
                    field_path: String::new(),
                    line: 1,
                    column: 1,
                    reason: ResourceTargetsRejectionReason::CatalogUnavailable,
                })
            })?;
            let checked = check_resource_targets(&parsed, catalog.compiled(), &facts, time)
                .map_err(resource_targets_error)?;
            (
                checked,
                catalog.compiled().catalog_hash().to_owned(),
                facts.ledger_position,
            )
        };
        let document = parsed.document();
        let scope = FactScope::Instance {
            instance_id: document.instance.clone(),
        };
        let bundle_hash = checked
            .policy_sha256
            .strip_prefix("sha256:")
            .ok_or_else(record_invalid)?
            .to_owned();
        let identity = serde_json::to_vec(&(
            &scope,
            RESOURCE_TARGETS_FACT_KEY,
            &checked.policy_sha256,
            sample.unix_ms,
        ))
        .map_err(|_| record_invalid())?;
        let (expires_at_unix_ms, ttl_policy) = match document.valid_until_unix_ms {
            Some(valid_until) => (
                Some(valid_until),
                Some(FactTtlPolicy {
                    minimum_ms: MIN_FACT_TTL_MS,
                    maximum_ms: MAX_FACT_TTL_MS,
                    source: FactTtlSource::RuntimeDefault,
                }),
            ),
            None => (None, None),
        };
        let record = FactRecord {
            scope,
            key: RESOURCE_TARGETS_FACT_KEY.to_owned(),
            content: FactContent::Inline {
                value: ContractFactValue::RecordList(checked.rows),
            },
            observed_at_unix_ms: sample.unix_ms,
            expires_at_unix_ms,
            ttl_policy,
            confidence_milli: 1_000,
            source_detector: RESOURCE_TARGETS_DETECTOR.to_owned(),
            source_snapshot_id: format!("snapshot:resource-targets:{:x}", Sha256::digest(identity)),
            schema_version: RESOURCE_TARGETS_FACT_SCHEMA.to_owned(),
            resource_bundle_hash: bundle_hash,
            invalidate_on: Vec::new(),
        };
        record.validate().map_err(|_| record_invalid())?;
        let FactPublication {
            event_id,
            sequence,
            previous_version,
            replayed,
        } = self.publish_facts(
            FactObservation {
                records: vec![record],
            },
            Some(validated),
            FactPublicationPurpose::ResourceTargets,
        )?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(TerminalEvent { sequence, event_id }),
            result: RuntimeResult::ResourceTargetsApplied {
                applied: Box::new(ResourceTargetsApplied {
                    instance_alias: document.instance.clone(),
                    policy_sha256: checked.policy_sha256,
                    version: sequence,
                    event_id,
                    previous_version,
                    replayed,
                    checked_catalog_hash,
                    valid_until_unix_ms: document.valid_until_unix_ms,
                    conditions_at_position,
                    conditions: checked.conditions,
                }),
            },
        })
    }
}

fn resource_targets_error(error: ResourceTargetsError) -> RuntimeHostError {
    match error {
        ResourceTargetsError::Rejected(rejection) => {
            RuntimeHostError::resource_targets_rejected(*rejection)
        }
        ResourceTargetsError::Evaluation(error) => RuntimeHostError::request(
            "policy_evaluation_rejected",
            RESOURCE_TARGETS_OPERATION,
            RuntimeErrorCode::InvalidRequest,
        )
        .with_native_detail(error.to_string()),
        ResourceTargetsError::Internal(code) => RuntimeHostError::fatal(
            code,
            RESOURCE_TARGETS_OPERATION,
            RuntimeErrorCode::RuntimeFatal,
        ),
    }
}

fn record_invalid() -> RuntimeHostError {
    RuntimeHostError::fatal(
        "resource_targets_record_invalid",
        RESOURCE_TARGETS_OPERATION,
        RuntimeErrorCode::RuntimeFatal,
    )
}
