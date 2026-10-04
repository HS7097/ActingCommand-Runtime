// SPDX-License-Identifier: AGPL-3.0-only

//! The formal entry of instance resource target policies (Workflow #308 RT-S1a, #335 S2b):
//! parse and check an `actingcommand.resource-targets.v1` or `.v2` document against the active
//! catalog and the authoritative projection, then store it as the instance fact
//! `session.resource_targets`. The instance and the lifetime are read through the parsed
//! document's accessors, which serve both versions.

use super::facts::{FactPublication, FactPublicationPurpose};
use super::*;
use actingcommand_contract::{
    ActiveResourceTargets, FactObservation, FactTtlPolicy, FactTtlSource, MAX_FACT_TTL_MS,
    MIN_FACT_TTL_MS, RESOURCE_TARGETS_FACT_KEY, ResourceTargetView, ResourceTargetsApplied,
    ResourceTargetsRejection, ResourceTargetsRejectionReason,
};
use actingcommand_policy::{
    ResourceTargetsError, StoredResourceTargetPolicy, StoredResourceTargets,
    active_resource_targets, check_resource_targets, parse_resource_targets, targetable_resources,
};

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
        let scope = FactScope::Instance {
            instance_id: parsed.instance().to_owned(),
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
        let (expires_at_unix_ms, ttl_policy) = match parsed.valid_until_unix_ms() {
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
                    instance_alias: parsed.instance().to_owned(),
                    policy_sha256: checked.policy_sha256,
                    version: sequence,
                    event_id,
                    previous_version,
                    replayed,
                    checked_catalog_hash,
                    valid_until_unix_ms: parsed.valid_until_unix_ms(),
                    conditions_at_position,
                    conditions: checked.conditions,
                }),
            },
        })
    }
}

const RESOURCE_TARGET_VIEW_OPERATION: &str = "resource_target_view";

impl HostShared {
    /// `ResourceTargetView` (Workflow #338 R5), read-only. The clock is sampled before any
    /// lock. Under the locks of `apply_resource_targets`' Phase A, in its order (policy outcome
    /// gate, policy, fact write gate), the authoritative inputs are projected, every pool of the
    /// active catalog is judged for the instance and its stored policy is read with the
    /// `fact.published` event that holds it; then every lock is released. Nothing is appended
    /// here; the projection's own fact-store synchronization is that of every policy read.
    pub(super) fn resource_target_view(
        &self,
        instance_alias: &str,
    ) -> Result<OperationSuccess, RequestFailure> {
        self.resolve_instance(instance_alias)?;
        self.resource_target_view_under_phase_a(instance_alias)
            .map_err(planning_request_failure)
    }

    fn resource_target_view_under_phase_a(
        &self,
        instance_alias: &str,
    ) -> RuntimeHostResult<OperationSuccess> {
        let sample = self.runtime_clock_sample()?;
        let time = EvaluationTime {
            unix_ms: sample.unix_ms,
            monotonic_ms: sample.monotonic_ms,
        };
        let _outcome_gate = lock(
            &self.policy_outcome_gate,
            "resource_target_view_outcome_state",
        )?;
        let mut policy = lock(&self.policy, "resource_target_view_outcome_keys")?;
        let outcome_keys = policy.outcome_key_snapshot()?;
        let _fact_gate = lock(&self.fact_write_gate, "resource_target_view_facts")?;
        let (facts, _) = self.project_authoritative_policy_inputs_under_gate(
            &mut policy,
            RESOURCE_TARGET_VIEW_OPERATION,
            &outcome_keys,
            None,
        )?;
        let instance = facts
            .instances
            .iter()
            .find(|instance| instance.instance_id == instance_alias)
            .ok_or_else(|| {
                RuntimeHostError::request(
                    "resource_target_view_instance_unconfigured",
                    RESOURCE_TARGET_VIEW_OPERATION,
                    RuntimeErrorCode::InstanceUnknown,
                )
            })?;
        let catalog = policy.active_loaded();
        let compiled = catalog.as_ref().map(|catalog| catalog.compiled());
        let (targetable, not_targetable) = match compiled {
            Some(compiled) => targetable_resources(compiled, &facts, instance, time)
                .map_err(resource_target_view_error)?,
            None => (Vec::new(), Vec::new()),
        };
        let active = match active_resource_targets(compiled, &facts, instance, time)
            .map_err(resource_target_view_error)?
        {
            StoredResourceTargets::None => None,
            StoredResourceTargets::Unreadable { code } => {
                return Err(RuntimeHostError::request(
                    "resource_target_view_policy_unreadable",
                    RESOURCE_TARGET_VIEW_OPERATION,
                    RuntimeErrorCode::LedgerFailure,
                )
                .with_native_detail(code.to_owned()));
            }
            StoredResourceTargets::Policy(stored) => {
                let scope = FactScope::Instance {
                    instance_id: instance.instance_id.clone(),
                };
                let store = lock(&self.facts, "resource_target_view_active_policy")?;
                let (_, version, event_id) = store
                    .active_revision(&scope, RESOURCE_TARGETS_FACT_KEY)
                    .filter(|(record, _, _)| {
                        stored.policy_sha256.strip_prefix("sha256:")
                            == Some(record.resource_bundle_hash.as_str())
                    })
                    .ok_or_else(|| {
                        RuntimeHostError::request(
                            "resource_target_view_policy_inconsistent",
                            RESOURCE_TARGET_VIEW_OPERATION,
                            RuntimeErrorCode::LedgerFailure,
                        )
                    })?;
                let StoredResourceTargetPolicy {
                    policy_sha256,
                    schema_version,
                    valid_until_unix_ms,
                    expired,
                    targets,
                    conditions,
                    ..
                } = *stored;
                Some(ActiveResourceTargets {
                    policy_sha256,
                    schema_version: schema_version.to_owned(),
                    version,
                    event_id,
                    valid_until_unix_ms: Some(valid_until_unix_ms),
                    expired,
                    targets,
                    conditions,
                })
            }
        };
        let view = ResourceTargetView {
            instance_alias: instance_alias.to_owned(),
            policy_instance: instance.instance_id.clone(),
            evaluated_at_unix_ms: sample.unix_ms,
            as_of_ledger_position: facts.ledger_position,
            catalog_hash: compiled.map(|compiled| compiled.catalog_hash().to_owned()),
            targetable,
            not_targetable,
            active,
        };
        view.validate().map_err(|_| {
            RuntimeHostError::request(
                "resource_target_view_invalid",
                RESOURCE_TARGET_VIEW_OPERATION,
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result: RuntimeResult::ResourceTargetView {
                view: Box::new(view),
            },
        })
    }
}

fn resource_target_view_error(error: ResourceTargetsError) -> RuntimeHostError {
    match error {
        ResourceTargetsError::Evaluation(error) => RuntimeHostError::request(
            "policy_evaluation_rejected",
            RESOURCE_TARGET_VIEW_OPERATION,
            RuntimeErrorCode::InvalidRequest,
        )
        .with_native_detail(error.to_string()),
        ResourceTargetsError::Rejected(_) | ResourceTargetsError::Internal(_) => {
            RuntimeHostError::request(
                "resource_target_view_internal",
                RESOURCE_TARGET_VIEW_OPERATION,
                RuntimeErrorCode::RuntimeFatal,
            )
        }
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
