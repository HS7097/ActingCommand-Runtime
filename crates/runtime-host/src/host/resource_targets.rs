// SPDX-License-Identifier: AGPL-3.0-only

//! The formal entry of instance resource target policies (Workflow #308 RT-S1a, #335 S2b):
//! parse and check an `actingcommand.resource-targets.v1` or `.v2` document against the active
//! catalog and the authoritative projection, then store it as the instance fact
//! `session.resource_targets`. The instance and the lifetime are read through the parsed
//! document's accessors, which serve both versions.

use super::facts::{FactPublication, FactPublicationPurpose};
use super::*;
use crate::codes::{HostCode, HostLocation};
use actingcommand_contract::codes::ContractLocation;
use actingcommand_contract::{
    ActiveResourceTargets, FactObservation, FactTtlPolicy, FactTtlSource, MAX_FACT_TTL_MS,
    MIN_FACT_TTL_MS, RESOURCE_TARGETS_FACT_KEY, ResourceTargetView, ResourceTargetsApplied,
    ResourceTargetsRejection, ResourceTargetsRejectionReason,
};
use actingcommand_policy::{
    ComposedResourceTargets, ResourceTargetsError, StoredResourceTargetPolicy,
    StoredResourceTargets, TargetSetter, active_resource_targets, check_resource_targets,
    parse_resource_targets, targetable_resources,
};

const RESOURCE_TARGETS_OPERATION: ContractLocation = ContractLocation::ApplyResourceTargets;
const RESOURCE_TARGETS_DETECTOR: &str = "runtime.resource-targets";
const RESOURCE_TARGETS_FACT_SCHEMA: &str = "fact.v1";

impl HostShared {
    /// `ApplyResourceTargets`. The clock is sampled before any lock and the document parsed
    /// without one. Phase A takes the policy outcome gate, the policy and the fact write gate
    /// in the order of `project_policy_input_identity`, checks the document against the
    /// active catalog and the authoritative projection and releases every lock. Phase B, in one
    /// hold of the fact write gate (Workflow #455 G-R2a), reads the instance's active record,
    /// composes the record the document writes over it (`CheckedResourceTargets::compose`: a v1
    /// or v2 document over a record that is not a v3 record exactly as before G-R2a; the v3 rules
    /// otherwise) and publishes it through `publish_facts_under_gate`, which replays an equal
    /// active policy. The rows a v3 record takes from this request name the Runtime's clock at the
    /// request, the channel the request is recorded with and, for a request from the Lab, the
    /// initiator its connection declared; never anything of the document. Only a v3 document
    /// needs that channel: on a request that names none it is refused `channel_binding_missing`
    /// (request-level, nothing appended), and a v1 or v2 document applies as on any request. A
    /// refused document records nothing and keeps the active policy. Workflow #455 T1-R: with
    /// `expected_version`,
    /// Phase B, in the same hold, first compares it with the instance's active record's version
    /// (its `fact.published` sequence, `0` without one); another is refused
    /// `fact_observation_version_conflict` with `fact_key` and `current_version` and appends
    /// nothing. Without it nothing is compared.
    pub(super) fn apply_resource_targets(
        &self,
        validated: &ValidatedRuntimeRequest<'_>,
        document_json: &str,
        expected_version: Option<u64>,
        connection_id: ConnectionId,
    ) -> Result<OperationSuccess, RequestFailure> {
        self.apply_resource_targets_document(
            validated,
            document_json,
            expected_version,
            connection_id,
        )
        .map_err(planning_request_failure)
    }

    fn apply_resource_targets_document(
        &self,
        validated: &ValidatedRuntimeRequest<'_>,
        document_json: &str,
        expected_version: Option<u64>,
        connection_id: ConnectionId,
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
                HostLocation::ApplyResourceTargetsOutcomeState,
            )?;
            let mut policy = lock(
                self.policy()?,
                HostLocation::ApplyResourceTargetsOutcomeKeys,
            )?;
            let outcome_keys = policy.outcome_key_snapshot()?;
            let _fact_gate = lock(
                &self.fact_write_gate,
                HostLocation::ApplyResourceTargetsFacts,
            )?;
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
        // Workflow #455 G-R2a: who sets the rows a v3 record takes from this request, as its
        // receipt names them (the receipt rule of `contracts/lab-two-entries.md` §4.3): the
        // recorded channel, and the declared initiator only for a request from the Lab. Only a v3
        // document's rows name it (the coordinator's ruling of 11-14 on #1702): a request without
        // a channel (the door tags every client request with one) applies a v1 or v2 document as
        // any other, and its v3 document is refused `channel_binding_missing` below, nothing
        // appended and the Runtime's health untouched.
        let initiator = if validated.source() == EventSource::Lab {
            self.connection_channel(connection_id)?
                .and_then(|bound| bound.initiator)
        } else {
            None
        };
        let setter = validated.channel().map(|binding| TargetSetter {
            set_unix_ms: sample.unix_ms,
            channel: binding.channel,
            initiator,
        });
        let scope = FactScope::Instance {
            instance_id: parsed.instance().to_owned(),
        };
        let written: RuntimeHostResult<WrittenTargets> = (|| {
            let _gate = lock(
                &self.fact_write_gate,
                HostLocation::ApplyResourceTargetsFacts,
            )?;
            self.synchronize_fact_store_under_gate()?;
            let stored = lock(&self.facts, HostLocation::ApplyResourceTargetsFacts)?
                .active_revision(&scope, RESOURCE_TARGETS_FACT_KEY)
                .and_then(|(record, _, _)| match &record.content {
                    FactContent::Inline {
                        value: ContractFactValue::RecordList(rows),
                    } => Some(rows.clone()),
                    _ => None,
                });
            let composed = checked
                .compose(stored.as_deref(), setter)
                .map_err(resource_targets_error)?
                .ok_or_else(channel_missing)?;
            let record = policy_record(&scope, &composed, sample.unix_ms)?;
            let publication = self.publish_facts_under_gate(
                FactObservation {
                    records: vec![record],
                },
                Some(validated),
                FactPublicationPurpose::ResourceTargets { expected_version },
                (validated.source(), validated.actor()),
            )?;
            Ok(WrittenTargets {
                publication,
                composed,
            })
        })();
        if let Err(error) = &written
            && error.is_fatal()
        {
            self.fatal.mark(error.clone())?;
        }
        let WrittenTargets {
            publication:
                FactPublication {
                    event_id,
                    sequence,
                    previous_version,
                    replayed,
                },
            composed,
        } = written?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(TerminalEvent { sequence, event_id }),
            result: RuntimeResult::ResourceTargetsApplied {
                applied: Box::new(ResourceTargetsApplied {
                    instance_alias: parsed.instance().to_owned(),
                    policy_sha256: composed.policy_sha256,
                    version: sequence,
                    event_id,
                    previous_version,
                    replayed,
                    checked_catalog_hash,
                    valid_until_unix_ms: composed.valid_until_unix_ms,
                    conditions_at_position,
                    conditions: composed.conditions,
                }),
            },
            failure_outcome: None,
        })
    }
}

/// What Phase B wrote: the publication and the record it composed.
struct WrittenTargets {
    publication: FactPublication,
    composed: ComposedResourceTargets,
}

/// The instance fact that holds `composed`, observed at `observed_at_unix_ms`, the clock of the
/// request: its rows, its expiry (a v1 or v2 policy's lifetime, with the runtime default TTL
/// policy; none for a v3 record or a withdrawal) and its identity.
fn policy_record(
    scope: &FactScope,
    composed: &ComposedResourceTargets,
    observed_at_unix_ms: u64,
) -> RuntimeHostResult<FactRecord> {
    let bundle_hash = composed
        .policy_sha256
        .strip_prefix("sha256:")
        .ok_or_else(record_invalid)?
        .to_owned();
    let identity = serde_json::to_vec(&(
        scope,
        RESOURCE_TARGETS_FACT_KEY,
        &composed.policy_sha256,
        observed_at_unix_ms,
    ))
    .map_err(|_| record_invalid())?;
    let (expires_at_unix_ms, ttl_policy) = match composed.expires_at_unix_ms {
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
        scope: scope.clone(),
        key: RESOURCE_TARGETS_FACT_KEY.to_owned(),
        content: FactContent::Inline {
            value: ContractFactValue::RecordList(composed.rows.clone()),
        },
        observed_at_unix_ms,
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
    Ok(record)
}

const RESOURCE_TARGET_VIEW_OPERATION: ContractLocation = ContractLocation::ResourceTargetView;

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
            HostLocation::ResourceTargetViewOutcomeState,
        )?;
        let mut policy = lock(self.policy()?, HostLocation::ResourceTargetViewOutcomeKeys)?;
        let outcome_keys = policy.outcome_key_snapshot()?;
        let _fact_gate = lock(&self.fact_write_gate, HostLocation::ResourceTargetViewFacts)?;
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
                    HostCode::ResourceTargetViewInstanceUnconfigured,
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
                    HostCode::ResourceTargetViewPolicyUnreadable,
                    RESOURCE_TARGET_VIEW_OPERATION,
                    RuntimeErrorCode::LedgerFailure,
                )
                .with_native_detail(code.as_str().to_owned()));
            }
            StoredResourceTargets::Policy(stored) => {
                let scope = FactScope::Instance {
                    instance_id: instance.instance_id.clone(),
                };
                let store = lock(&self.facts, HostLocation::ResourceTargetViewActivePolicy)?;
                let (_, version, event_id) = store
                    .active_revision(&scope, RESOURCE_TARGETS_FACT_KEY)
                    .filter(|(record, _, _)| {
                        stored.policy_sha256.strip_prefix("sha256:")
                            == Some(record.resource_bundle_hash.as_str())
                    })
                    .ok_or_else(|| {
                        RuntimeHostError::request(
                            HostCode::ResourceTargetViewPolicyInconsistent,
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
                    // Workflow #455 G-R2a: absent for a v3 record, whose targets carry their own.
                    valid_until_unix_ms,
                    expired,
                    targets,
                    conditions,
                    // Workflow #429 R-0: the console's read model is R-13e's; none in 0.12.30.
                    rows: Vec::new(),
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
            // Workflow #429 R-0: the stale-schema mark is R-13a's; none in 0.12.30.
            stale_schema: None,
        };
        view.validate().map_err(|_| {
            RuntimeHostError::request(
                HostCode::ResourceTargetViewInvalid,
                RESOURCE_TARGET_VIEW_OPERATION,
                RuntimeErrorCode::RuntimeHalted,
            )
        })?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result: RuntimeResult::ResourceTargetView {
                view: Box::new(view),
            },
            failure_outcome: None,
        })
    }
}

fn resource_target_view_error(error: ResourceTargetsError) -> RuntimeHostError {
    match error {
        ResourceTargetsError::Evaluation(error) => RuntimeHostError::request(
            HostCode::PolicyEvaluationRejected,
            RESOURCE_TARGET_VIEW_OPERATION,
            RuntimeErrorCode::InvalidRequest,
        )
        .with_native_detail(error.to_string()),
        ResourceTargetsError::Rejected(_) | ResourceTargetsError::Internal(_) => {
            RuntimeHostError::request(
                HostCode::ResourceTargetViewInternal,
                RESOURCE_TARGET_VIEW_OPERATION,
                RuntimeErrorCode::RuntimeHalted,
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
            HostCode::PolicyEvaluationRejected,
            RESOURCE_TARGETS_OPERATION,
            RuntimeErrorCode::InvalidRequest,
        )
        .with_native_detail(error.to_string()),
        ResourceTargetsError::Internal(code) => RuntimeHostError::fatal(
            code,
            RESOURCE_TARGETS_OPERATION,
            RuntimeErrorCode::RuntimeHalted,
        ),
    }
}

fn record_invalid() -> RuntimeHostError {
    RuntimeHostError::fatal(
        HostCode::ResourceTargetsRecordInvalid,
        RESOURCE_TARGETS_OPERATION,
        RuntimeErrorCode::RuntimeHalted,
    )
}

/// Workflow #455 G-R2a (the coordinator's ruling of 11-14 on #1702): a v3 document's rows name
/// the channel that set them, and the request names none (the door tags every client request
/// with its connection's channel). A request-level refusal, Denied `invalid_request` with the host
/// code `channel_binding_missing` at `apply_resource_targets`: nothing is appended and nothing
/// latches.
fn channel_missing() -> RuntimeHostError {
    RuntimeHostError::request(
        HostCode::ChannelBindingMissing,
        RESOURCE_TARGETS_OPERATION,
        RuntimeErrorCode::InvalidRequest,
    )
}
