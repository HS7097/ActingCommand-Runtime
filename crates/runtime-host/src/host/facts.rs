// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::codes::{HostCode, HostLocation};
use crate::fact_store::{priority_offset_milli, valid_priority_offset_task_id};
use actingcommand_contract::codes::{ContractLocation, SharedCode};
use actingcommand_contract::{
    CaptureExtent, CaptureGeometryObservation, CaptureRotation, CaptureRotationObservation,
    RESOURCE_TARGETS_FACT_KEY, priority_offset_task_id,
};

/// `source_detector` of the three configuration-seeded policy instance facts.
const POLICY_INSTANCE_SEED_DETECTOR: &str = "runtime.policy-configuration";
const POLICY_INSTANCE_SEED_SCHEMA: &str = "fact.v1";
/// `source_detector` and `source_snapshot_id` prefix of the `session.instance.available = false`
/// records a failed backend self-check publishes (Workflow #317 sc2).
const BACKEND_SELFCHECK_AVAILABILITY_DETECTOR: &str = "runtime.backend-selfcheck";
pub(super) const BACKEND_SELFCHECK_AVAILABILITY_SNAPSHOT_PREFIX: &str = "backend_selfcheck:";
/// Workflow #398 RD-1b (model-398 v1.1 §3.6, §4.2): `source_detector` and `source_snapshot_id`
/// prefix of the `session.instance.available = false` records a handset's device link
/// publishes, `device_link:<state>`, beside `backend_selfcheck:`.
const DEVICE_LINK_AVAILABILITY_DETECTOR: &str = "runtime.device-link";
const DEVICE_LINK_AVAILABILITY_SNAPSHOT_PREFIX: &str = "device_link:";
/// The device-link state of a handset the private binding file binds no device to.
pub(super) const DEVICE_LINK_UNBOUND: &str = "unbound";
/// Workflow #398 RD-5b (model-398 v1.1 §7.6): `source_detector` and `source_snapshot_id` prefix
/// of the `session.instance.available = false` records a handset's device-state hold publishes,
/// `device_state:<hold>`, beside `backend_selfcheck:` and `device_link:`.
const DEVICE_STATE_AVAILABILITY_DETECTOR: &str = "runtime.device-state";
const DEVICE_STATE_AVAILABILITY_SNAPSHOT_PREFIX: &str = "device_state:";

// Workflow #392 DA-A (model 392 v1.1, section 8.1): the instance's display, recorded and never
// checked. `display.aspect` has no reader in the Runtime (architecture guard DA-A-T3).
/// `source_detector` of the instance display facts.
const DISPLAY_FACT_DETECTOR: &str = "runtime.display-adapter";
const DISPLAY_FACT_SCHEMA: &str = "fact.v1";
/// The device frame: one `record_list` row `{width, height}`.
const DISPLAY_FRAME_EXTENT_FACT_KEY: &str = "display.frame_extent";
/// The display rotation: `r0`, `r90`, `r180`, `r270` or `unknown`.
const DISPLAY_ROTATION_FACT_KEY: &str = "display.rotation";
/// The frame's orientation: `landscape`, `portrait` or `square`.
const DISPLAY_ORIENTATION_FACT_KEY: &str = "display.orientation";
/// The frame's aspect ratio: one `record_list` row `{width, height}` reduced by their greatest
/// common divisor.
const DISPLAY_ASPECT_FACT_KEY: &str = "display.aspect";

/// The display of one device frame (Workflow #392 DA-A, model 392 section 8.1).
struct DisplayFacts {
    width: u32,
    height: u32,
    rotation: &'static str,
    orientation: &'static str,
    aspect_width: u32,
    aspect_height: u32,
}

impl DisplayFacts {
    /// The frame's extent and its display's rotation (`None`: the source does not provide it).
    fn new(extent: CaptureExtent, rotation: Option<CaptureRotation>) -> Self {
        let (width, height) = (extent.width(), extent.height());
        let (mut divisor, mut remainder) = (width.max(height), width.min(height));
        while remainder != 0 {
            (divisor, remainder) = (remainder, divisor % remainder);
        }
        Self {
            width,
            height,
            rotation: match rotation {
                Some(CaptureRotation::R0) => "r0",
                Some(CaptureRotation::R90) => "r90",
                Some(CaptureRotation::R180) => "r180",
                Some(CaptureRotation::R270) => "r270",
                None => "unknown",
            },
            orientation: match width.cmp(&height) {
                std::cmp::Ordering::Greater => "landscape",
                std::cmp::Ordering::Less => "portrait",
                std::cmp::Ordering::Equal => "square",
            },
            aspect_width: width / divisor,
            aspect_height: height / divisor,
        }
    }

    /// One `record_list` row `{width, height}`.
    fn extent_value(width: u32, height: u32) -> ContractFactValue {
        ContractFactValue::RecordList(vec![BTreeMap::from([
            ("width".to_owned(), FactScalar::Integer(i64::from(width))),
            ("height".to_owned(), FactScalar::Integer(i64::from(height))),
        ])])
    }

    /// The four facts, in the key order of model 392 section 8.1.
    fn values(&self) -> [(&'static str, ContractFactValue); 4] {
        [
            (
                DISPLAY_FRAME_EXTENT_FACT_KEY,
                Self::extent_value(self.width, self.height),
            ),
            (
                DISPLAY_ROTATION_FACT_KEY,
                ContractFactValue::String(self.rotation.to_owned()),
            ),
            (
                DISPLAY_ORIENTATION_FACT_KEY,
                ContractFactValue::String(self.orientation.to_owned()),
            ),
            (
                DISPLAY_ASPECT_FACT_KEY,
                Self::extent_value(self.aspect_width, self.aspect_height),
            ),
        ]
    }

    /// Lowercase hex SHA-256 of the instance alias and the four values: the observation's
    /// `source_snapshot_id` (`display:<digest>`) and `resource_bundle_hash`.
    fn digest(&self, instance_alias: &str) -> String {
        format!(
            "{:x}",
            Sha256::digest(
                format!(
                    "{instance_alias}\n{}x{}\n{}\n{}\n{}x{}",
                    self.width,
                    self.height,
                    self.rotation,
                    self.orientation,
                    self.aspect_width,
                    self.aspect_height
                )
                .as_bytes()
            )
        )
    }
}

/// One `record_list` row per identifier, each holding the single string field `field`.
fn policy_instance_string_list_value(field: &str, identifiers: &[String]) -> ContractFactValue {
    ContractFactValue::RecordList(
        identifiers
            .iter()
            .map(|identifier| {
                BTreeMap::from([(field.to_owned(), FactScalar::String(identifier.clone()))])
            })
            .collect(),
    )
}

/// The stable hash a seed carries as `source_snapshot_id` and `resource_bundle_hash`: SHA-256
/// over the canonical JSON of (instance alias, key, configured value), as lowercase hex.
fn policy_instance_seed_digest(
    instance_id: &str,
    key: &str,
    value: &ContractFactValue,
) -> RuntimeHostResult<String> {
    let bytes = serde_json::to_vec(&(instance_id, key, value)).map_err(|_| {
        RuntimeHostError::fatal(
            HostCode::PolicyInstanceSeedEncodeFailed,
            HostLocation::SeedPolicyInstanceFacts,
            RuntimeErrorCode::RuntimeHalted,
        )
    })?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

/// Which entry publishes an observation (Workflow #308 RT-S1a).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FactPublicationPurpose {
    /// `PublishFact` / `PublishFacts` and the Runtime's own publications; never the resource
    /// target policy key.
    Ordinary,
    /// `ApplyResourceTargets`: exactly one instance-scoped `session.resource_targets` record.
    ResourceTargets,
    /// Workflow #384 D2-S3 (D2 model §3.6, A-1): one record a run's resource adjustment wrote
    /// from the key's active record, which the `fact.published` at ledger sequence
    /// `base_sequence` carried. Under `fact_write_gate` the record is refused when another
    /// record of the key has replaced that one since.
    ResourceAdjustment { base_sequence: u64 },
}

/// Where one publication landed: the event that holds its records, the sequence of the
/// record active before it (resource target policies only) and whether nothing was appended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FactPublication {
    pub(super) event_id: EventId,
    pub(super) sequence: u64,
    pub(super) previous_version: Option<u64>,
    pub(super) replayed: bool,
}

impl HostShared {
    pub(super) fn publish_fact(&self, record: FactRecord) -> RuntimeHostResult<EventId> {
        self.publish_facts(
            actingcommand_contract::FactObservation {
                records: vec![record],
            },
            None,
            FactPublicationPurpose::Ordinary,
        )
        .map(|publication| publication.event_id)
    }

    /// Seeds the policy instance facts from the configured policy inputs (Workflow #313
    /// item 4): for every configured instance, `session.instance.available`,
    /// `session.instance.capabilities` and `session.instance.preferred_tasks`, each with
    /// scope = that instance. A record's `source_snapshot_id` is a stable hash of the
    /// instance alias, the key and the configured value: a key whose stored record already
    /// carries that id is already published and appends nothing (its content is verified
    /// against the configured value); otherwise the seed is a new observation at the host
    /// clock. Any refusal is returned to the caller, which fails startup. The
    /// `session.instance.available` seed of a device self-checked instance is left to its
    /// connection preparation phase (Workflow #317 sc3).
    pub(super) fn seed_policy_instance_facts(&self) -> RuntimeHostResult<()> {
        let Some(inputs) =
            lock(&self.policy_inputs, HostLocation::SeedPolicyInstanceFacts)?.clone()
        else {
            return Ok(());
        };
        for instance in inputs.instance_seeds() {
            let scope = FactScope::Instance {
                instance_id: instance.instance_id.clone(),
            };
            let seeds = [
                (
                    POLICY_INSTANCE_AVAILABLE_KEY,
                    ContractFactValue::Boolean(instance.available),
                ),
                (
                    POLICY_INSTANCE_CAPABILITIES_KEY,
                    policy_instance_string_list_value(
                        POLICY_INSTANCE_OPERATION_FIELD,
                        &instance.capability_operation_ids,
                    ),
                ),
                (
                    POLICY_INSTANCE_PREFERRED_TASKS_KEY,
                    policy_instance_string_list_value(
                        POLICY_INSTANCE_TASK_FIELD,
                        &instance.preferred_task_ids,
                    ),
                ),
            ];
            // Workflow #317 sc3: a device self-checked instance's availability is its seed and
            // its self-check; its startup preparation phase publishes it, so seeding it here too
            // would only add a record the preparation withdraws at once.
            let self_checked = lock(
                &self.registered_instances,
                HostLocation::SeedPolicyInstanceFacts,
            )?
            .values()
            .any(|registered| {
                registered.instance_alias == instance.instance_id
                    && registered.device_self_checked()
            });
            for (key, value) in seeds {
                if self_checked && key == POLICY_INSTANCE_AVAILABLE_KEY {
                    continue;
                }
                let digest = policy_instance_seed_digest(&instance.instance_id, key, &value)?;
                let source_snapshot_id = format!("snapshot:policy-config:{digest}");
                let content = FactContent::Inline { value };
                let already_published = {
                    let _gate = lock(&self.fact_write_gate, HostLocation::SeedPolicyInstanceFacts)?;
                    let stored = lock(&self.facts, HostLocation::SeedPolicyInstanceFacts)?;
                    match stored.active_record(&scope, key) {
                        Some(existing) if existing.source_snapshot_id == source_snapshot_id => {
                            if existing.content != content {
                                return Err(RuntimeHostError::fatal(
                                    HostCode::PolicyInstanceSeedIdentityConflict,
                                    HostLocation::SeedPolicyInstanceFacts,
                                    RuntimeErrorCode::RuntimeHalted,
                                ));
                            }
                            true
                        }
                        _ => false,
                    }
                };
                if already_published {
                    continue;
                }
                self.publish_fact(FactRecord {
                    scope: scope.clone(),
                    key: key.to_owned(),
                    content,
                    observed_at_unix_ms: self.clock.sample()?.unix_ms,
                    expires_at_unix_ms: None,
                    ttl_policy: None,
                    confidence_milli: 1_000,
                    source_detector: POLICY_INSTANCE_SEED_DETECTOR.to_owned(),
                    source_snapshot_id,
                    schema_version: POLICY_INSTANCE_SEED_SCHEMA.to_owned(),
                    resource_bundle_hash: digest,
                    invalidate_on: Vec::new(),
                })?;
            }
        }
        Ok(())
    }

    pub(super) fn fact_scope_instances(
        &self,
        scope: &actingcommand_contract::FactScope,
    ) -> RuntimeHostResult<Vec<InstanceId>> {
        let inputs = lock(&self.policy_inputs, HostLocation::BindFactScope)?;
        let registered = lock(&self.registered_instances, HostLocation::BindFactScope)?;
        Ok(registered
            .values()
            .filter(|instance| match scope {
                actingcommand_contract::FactScope::Instance { instance_id } => {
                    instance_id == &instance.instance_alias
                }
                _ => inputs.as_ref().is_some_and(|inputs| {
                    inputs.facts().instances.iter().any(|context| {
                        context.instance_id == instance.instance_alias
                            && scope.matches(&InstanceFactContext {
                                instance_id: context.instance_id.clone(),
                                server_id: context.server_id.clone(),
                                game_id: context.game_id.clone(),
                            })
                    })
                }),
            })
            .map(|instance| instance.instance_id)
            .collect())
    }

    pub(super) fn publish_facts(
        &self,
        observation: actingcommand_contract::FactObservation,
        source_request: Option<&ValidatedRuntimeRequest<'_>>,
        purpose: FactPublicationPurpose,
    ) -> RuntimeHostResult<FactPublication> {
        let result: RuntimeHostResult<FactPublication> =
            (|| {
                // Workflow #411 B15 R5: `session.dispatch.` holds a run's read-only parameters,
                // which exist only as an overlay of its snapshots; no origin publishes under it.
                if let Some(record) = observation.records.iter().find(|record| {
                    record
                        .key
                        .starts_with(actingcommand_execution_kernel::RUN_PARAMETERS_KEY_PREFIX)
                }) {
                    let fact_key = record.key.clone();
                    return Err(RuntimeHostError::request(
                        crate::codes::RemedyHostCode::SessionDispatchKeyReserved,
                        ContractLocation::PublishFacts,
                        RuntimeErrorCode::InvalidRequest,
                    )
                    .with_outcome_values(|outcome| outcome.fact_key(fact_key.as_str())));
                }
                // Workflow #308 RT-S1a: the resource target policy key has one formal entry.
                match purpose {
                    FactPublicationPurpose::Ordinary
                    | FactPublicationPurpose::ResourceAdjustment { .. } => {
                        if observation
                            .records
                            .iter()
                            .any(|record| record.key == RESOURCE_TARGETS_FACT_KEY)
                        {
                            return Err(RuntimeHostError::request(
                                HostCode::ResourceTargetsFormalEntryRequired,
                                ContractLocation::PublishFacts,
                                RuntimeErrorCode::InvalidRequest,
                            ));
                        }
                    }
                    FactPublicationPurpose::ResourceTargets => {
                        if !matches!(
                            observation.records.as_slice(),
                            [record] if record.key == RESOURCE_TARGETS_FACT_KEY
                                && matches!(record.scope, FactScope::Instance { .. })
                        ) {
                            return Err(RuntimeHostError::fatal(
                                HostCode::ResourceTargetsRecordInvalid,
                                ContractLocation::PublishFacts,
                                RuntimeErrorCode::RuntimeHalted,
                            ));
                        }
                    }
                }
                // A priority offset (Workflow #308 slice 4a-2) names a task of the scheduling
                // identifier charset, holds an inline integer within ±1 000 000 milli, and is never
                // invalidated by events (its lifetime is the publisher's TTL).
                for record in &observation.records {
                    if let Some(task_id) = priority_offset_task_id(&record.key)
                        && (!valid_priority_offset_task_id(task_id)
                            || priority_offset_milli(record).is_none()
                            || !record.invalidate_on.is_empty())
                    {
                        return Err(RuntimeHostError::request(
                            SharedCode::PriorityOffsetInvalid,
                            ContractLocation::PublishFacts,
                            RuntimeErrorCode::InvalidRequest,
                        ));
                    }
                }
                // Workflow #411 B15 R6 (model-b15 §5.1): a write of the remedy choices is checked
                // whole before the fact write gate; a refusal appends nothing.
                for record in &observation.records {
                    if record.key == actingcommand_contract::REMEDY_CHOICES_FACT_KEY {
                        self.check_remedy_choices(record)?;
                    }
                }
                // An observation of a family with the origin exception (priority offsets, remedy
                // choices) and a resource target policy keep their request's origin on the
                // `fact.published` event, so the ledger shows who set them; every other
                // publication stays a Runtime fact-store event.
                let (event_source, event_actor) = match source_request {
                    Some(request)
                        if purpose == FactPublicationPurpose::ResourceTargets
                            || (!observation.records.is_empty()
                                && observation.records.iter().all(|record| {
                                    actingcommand_contract::fact_key_family(&record.key)
                                        .takes_origin_exception()
                                })) =>
                    {
                        (request.source(), request.actor())
                    }
                    _ => (EventSource::Runtime, EventActor::Runtime),
                };
                let _gate = lock(&self.fact_write_gate, ContractLocation::PublishFact)?;
                self.synchronize_fact_store_under_gate()?;
                // Workflow #384 D2-S3: the adjustment's base is read and its record written in
                // this one hold of the gate.
                if let FactPublicationPurpose::ResourceAdjustment { base_sequence } = purpose {
                    let facts = lock(&self.facts, ContractLocation::PublishFact)?;
                    let current = match observation.records.as_slice() {
                        [record] => facts
                            .active_revision(&record.scope, &record.key)
                            .map(|(_, sequence, _)| sequence),
                        _ => None,
                    };
                    if current != Some(base_sequence) {
                        return Err(RuntimeHostError::request(
                            HostCode::FactObservationVersionConflict,
                            ContractLocation::PublishFacts,
                            RuntimeErrorCode::InvalidRequest,
                        ));
                    }
                }
                let now = self.clock.sample()?.unix_ms;
                // A resource target policy equal to the active, unexpired one is a replay: the
                // active revision answers and nothing is appended.
                let mut previous_version = None;
                if purpose == FactPublicationPurpose::ResourceTargets {
                    let record = &observation.records[0];
                    let facts = lock(&self.facts, ContractLocation::PublishFact)?;
                    if let Some((active, sequence, event_id)) =
                        facts.active_revision(&record.scope, &record.key)
                    {
                        if active.resource_bundle_hash == record.resource_bundle_hash
                            && !active.is_expired(now)
                        {
                            return Ok(FactPublication {
                                event_id,
                                sequence,
                                previous_version: Some(sequence),
                                replayed: true,
                            });
                        }
                        previous_version = Some(sequence);
                    }
                }
                let preview = lock(&self.facts, ContractLocation::PublishFact)?
                    .preview_observation(&observation, now, &self.ledger)?;
                if let Some(event_id) = preview {
                    let record = &observation.records[0];
                    let facts = lock(&self.facts, ContractLocation::PublishFact)?;
                    let (_, sequence, _) = facts
                        .active_revision(&record.scope, &record.key)
                        .filter(|(_, _, active_event)| *active_event == event_id)
                        .ok_or_else(|| {
                            RuntimeHostError::fatal(
                                HostCode::FactPublicationSequenceMissing,
                                ContractLocation::PublishFacts,
                                RuntimeErrorCode::RuntimeHalted,
                            )
                        })?;
                    return Ok(FactPublication {
                        event_id,
                        sequence,
                        previous_version: Some(sequence),
                        replayed: true,
                    });
                }
                let scope = &observation.records[0].scope;
                let inputs = lock(&self.policy_inputs, HostLocation::BindFactScope)?;
                if inputs.as_ref().is_some_and(|inputs| {
                    observation.records.iter().any(|record| {
                        inputs.facts().facts.iter().any(|fact| {
                            fact.scope == crate::fact_store::policy_scope(&record.scope)
                                && fact.fact_key == record.key
                        })
                    })
                }) {
                    return Err(policy_admission_request(
                        HostCode::PolicyFactAuthorityConflict,
                        ContractLocation::PublishFacts,
                    ));
                }
                drop(inputs);
                let scope_instances = self.fact_scope_instances(scope)?;
                lock(&self.facts, HostLocation::ValidateFactInputBoundary)?
                    .validate_input_boundaries(&observation.records[0], &scope_instances)?;
                if scope_instances.is_empty()
                    && observation.records[0].invalidate_on.iter().any(|event| {
                        matches!(event, EventType::InputCommitted | EventType::InputFailed)
                    })
                {
                    return Err(policy_admission_request(
                        HostCode::FactInvalidationScopeUnbound,
                        ContractLocation::PublishFacts,
                    ));
                }
                let payload =
                    FactPayloadDraft::observation(observation, scope_instances, AuditInput::new())
                        .map_err(|_| {
                            policy_admission_request(
                                HostCode::FactObservationInvalid,
                                ContractLocation::PublishFacts,
                            )
                        })?;
                let links = match source_request {
                    Some(request) => self.events.request_links(request, None, None, None),
                    None => self.events.system_links()?,
                };
                let event = self.append_event_under_fact_gate(
                    EventSeverity::Info,
                    event_source,
                    OriginModule::FactStore,
                    event_actor,
                    links,
                    payload,
                )?;
                self.synchronize_fact_store_under_gate()?;
                Ok(FactPublication {
                    event_id: *event.event_id(),
                    sequence: event.sequence(),
                    previous_version,
                    replayed: false,
                })
            })();
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.fatal.mark(error.clone())?;
        }
        result
    }

    /// Workflow #411 B15 R6 (model-b15 §5.1): the write rule of one `session.remedy.choices`
    /// record (`crate::remedy_choices`), against the packages the active catalog binds, read
    /// now, on the instances of the policy inputs its scope covers; a refusal is
    /// `session_remedy_choices_invalid`. Each lock is released before the next is taken.
    fn check_remedy_choices(&self, record: &FactRecord) -> RuntimeHostResult<()> {
        let instances = lock(&self.policy_inputs, ContractLocation::PublishFacts)?
            .as_ref()
            .map(|inputs| inputs.facts().instances.clone())
            .unwrap_or_default();
        let manifest = lock(&self.procedure_manifest, ContractLocation::PublishFacts)?.clone();
        let catalog = lock(self.policy()?, ContractLocation::PublishFacts)?.active_loaded();
        let now_unix_ms = self.clock.sample()?.unix_ms;
        crate::remedy_choices::check(
            record,
            catalog.as_ref().map(|catalog| catalog.compiled()),
            manifest.as_ref(),
            &instances,
            now_unix_ms,
        )
        .map_err(crate::remedy_choices::Refusal::into_error)
    }

    /// Workflow #384 D2-S3 (D2 model §3.6): `key`'s active record in `scope`, with the ledger
    /// sequence and the id of the `fact.published` that carried it, read under
    /// `fact_write_gate`; `None` when the key has none.
    pub(super) fn active_fact_revision(
        &self,
        scope: &FactScope,
        key: &str,
    ) -> RuntimeHostResult<Option<(FactRecord, u64, EventId)>> {
        let result: RuntimeHostResult<Option<(FactRecord, u64, EventId)>> = (|| {
            let _gate = lock(&self.fact_write_gate, HostLocation::PublishResourceReadings)?;
            self.synchronize_fact_store_under_gate()?;
            Ok(lock(&self.facts, HostLocation::PublishResourceReadings)?
                .active_revision(scope, key)
                .map(|(record, sequence, event_id)| (record.clone(), sequence, event_id)))
        })();
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.fatal.mark(error.clone())?;
        }
        result
    }

    /// Workflow #384 D2-S3 (D2 model §3.6): drops `key`'s active record in `scope`, published at
    /// ledger sequence `base_sequence`, because a run confirmed an operation that declares an
    /// adjustment of the key and read no new value: one `fact.invalidated` under
    /// `fact_write_gate`, whose trigger is that step's `task.step_finished`, the event at
    /// `trigger_sequence`. A key without an active record is left as it is; another record than
    /// that one is refused (`fact_observation_version_conflict`).
    pub(super) fn invalidate_adjusted_fact(
        &self,
        scope: &FactScope,
        key: &str,
        base_sequence: u64,
        trigger_sequence: u64,
    ) -> RuntimeHostResult<()> {
        let result: RuntimeHostResult<()> = (|| {
            let _gate = lock(&self.fact_write_gate, HostLocation::PublishResourceReadings)?;
            self.synchronize_fact_store_under_gate()?;
            let prepared = {
                let facts = lock(&self.facts, HostLocation::PublishResourceReadings)?;
                if facts.active_revision(scope, key).is_none() {
                    return Ok(());
                }
                facts.prepare_adjustment_invalidation(
                    &self.ledger,
                    scope,
                    key,
                    base_sequence,
                    trigger_sequence,
                )?
            };
            let Some((data, work)) = prepared else {
                return Err(RuntimeHostError::request(
                    HostCode::FactObservationVersionConflict,
                    ContractLocation::PublishFacts,
                    RuntimeErrorCode::InvalidRequest,
                ));
            };
            let draft = self.events.draft(
                EventSeverity::Info,
                EventSource::Runtime,
                OriginModule::FactStore,
                EventActor::Runtime,
                self.events.system_links()?,
                FactPayloadDraft::invalidated(data, AuditInput::new()),
            )?;
            let draft = self.events.sanitize(draft)?;
            let persisted = self
                .ledger
                .append_transaction(draft, Box::new(work))
                .map_err(crate::fact_store::fact_transaction_error)?;
            // Workflow #383 AO-7: a committed invalidation changes the instance fact store.
            self.policy_wakes.raise(persisted.event_type());
            self.synchronize_fact_store_under_gate()
        })();
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.fatal.mark(error.clone())?;
        }
        result
    }

    /// Workflow #317 sc2: publishes `session.instance.available = false` for one configured
    /// policy instance because its self-check does not pass, through the seed's publisher
    /// ([`Self::publish_fact`]) and scope. The record differs from the seed only in
    /// `source_detector` `runtime.backend-selfcheck` and `source_snapshot_id`
    /// `backend_selfcheck:<cause>`, `<cause>` being `<entry>:<generation>` of the recorded open
    /// or `unchecked` (sc3); `resource_bundle_hash` is the seed digest of the value `false`.
    pub(super) fn publish_backend_selfcheck_unavailable(
        &self,
        instance_alias: &str,
        cause: &str,
    ) -> RuntimeHostResult<()> {
        self.publish_policy_instance_availability(
            instance_alias,
            false,
            BACKEND_SELFCHECK_AVAILABILITY_DETECTOR,
            |_| format!("{BACKEND_SELFCHECK_AVAILABILITY_SNAPSHOT_PREFIX}{cause}"),
        )
    }

    /// Workflow #398 RD-1b (model-398 v1.1 §3.6): withdraws the policy availability of a
    /// handset instance for its device link's `state` (`device_link:<state>`), through
    /// [`Self::publish_fact`].
    pub(super) fn publish_device_link_unavailable(
        &self,
        instance_alias: &str,
        state: &str,
    ) -> RuntimeHostResult<()> {
        self.publish_policy_instance_availability(
            instance_alias,
            false,
            DEVICE_LINK_AVAILABILITY_DETECTOR,
            |_| format!("{DEVICE_LINK_AVAILABILITY_SNAPSHOT_PREFIX}{state}"),
        )
    }

    /// Workflow #398 RD-2 (model-398 v1.1 §4.2): whether `record` is a withdrawal the device
    /// link published, whatever its state.
    pub(super) fn withdrawn_by_device_link(record: Option<&FactRecord>) -> bool {
        record.is_some_and(|record| {
            record.source_detector == DEVICE_LINK_AVAILABILITY_DETECTOR
                && record
                    .source_snapshot_id
                    .starts_with(DEVICE_LINK_AVAILABILITY_SNAPSHOT_PREFIX)
        })
    }

    /// Workflow #398 RD-1b: whether `record` is a withdrawal for the device link's `state`.
    pub(super) fn withdrawn_for_device_link(record: Option<&FactRecord>, state: &str) -> bool {
        record.is_some_and(|record| {
            record.source_detector == DEVICE_LINK_AVAILABILITY_DETECTOR
                && record
                    .source_snapshot_id
                    .strip_prefix(DEVICE_LINK_AVAILABILITY_SNAPSHOT_PREFIX)
                    == Some(state)
                && matches!(
                    record.content,
                    FactContent::Inline {
                        value: ContractFactValue::Boolean(false)
                    }
                )
        })
    }

    /// Workflow #398 RD-5b (model-398 v1.1 §7.3-§7.6): withdraws the policy availability of a
    /// handset instance for its device-state `hold` (`device_state:<hold>`), through
    /// [`Self::publish_fact`].
    pub(super) fn publish_device_state_unavailable(
        &self,
        instance_alias: &str,
        hold: &str,
    ) -> RuntimeHostResult<()> {
        self.publish_policy_instance_availability(
            instance_alias,
            false,
            DEVICE_STATE_AVAILABILITY_DETECTOR,
            |_| format!("{DEVICE_STATE_AVAILABILITY_SNAPSHOT_PREFIX}{hold}"),
        )
    }

    /// Workflow #398 RD-5b: whether `record` is a withdrawal a device-state hold published,
    /// whatever the hold.
    pub(super) fn withdrawn_by_device_state(record: Option<&FactRecord>) -> bool {
        record.is_some_and(|record| {
            record.source_detector == DEVICE_STATE_AVAILABILITY_DETECTOR
                && record
                    .source_snapshot_id
                    .starts_with(DEVICE_STATE_AVAILABILITY_SNAPSHOT_PREFIX)
        })
    }

    /// Workflow #398 RD-5b: whether `record` is a withdrawal for the device-state `hold`.
    pub(super) fn withdrawn_for_device_state(record: Option<&FactRecord>, hold: &str) -> bool {
        record.is_some_and(|record| {
            record.source_detector == DEVICE_STATE_AVAILABILITY_DETECTOR
                && record
                    .source_snapshot_id
                    .strip_prefix(DEVICE_STATE_AVAILABILITY_SNAPSHOT_PREFIX)
                    == Some(hold)
                && matches!(
                    record.content,
                    FactContent::Inline {
                        value: ContractFactValue::Boolean(false)
                    }
                )
        })
    }

    /// Workflow #317 sc2: republishes the configured seed of `session.instance.available`
    /// (`available`, with the seed's own detector, snapshot id and digest) as a new observation
    /// at the host clock, through [`Self::publish_fact`].
    pub(super) fn restore_policy_instance_availability(
        &self,
        instance_alias: &str,
        available: bool,
    ) -> RuntimeHostResult<()> {
        self.publish_policy_instance_availability(
            instance_alias,
            available,
            POLICY_INSTANCE_SEED_DETECTOR,
            |digest| format!("snapshot:policy-config:{digest}"),
        )
    }

    fn publish_policy_instance_availability(
        &self,
        instance_alias: &str,
        available: bool,
        source_detector: &str,
        source_snapshot_id: impl FnOnce(&str) -> String,
    ) -> RuntimeHostResult<()> {
        let value = ContractFactValue::Boolean(available);
        let digest =
            policy_instance_seed_digest(instance_alias, POLICY_INSTANCE_AVAILABLE_KEY, &value)?;
        self.publish_fact(FactRecord {
            scope: FactScope::Instance {
                instance_id: instance_alias.to_owned(),
            },
            key: POLICY_INSTANCE_AVAILABLE_KEY.to_owned(),
            content: FactContent::Inline { value },
            observed_at_unix_ms: self.clock.sample()?.unix_ms,
            expires_at_unix_ms: None,
            ttl_policy: None,
            confidence_milli: 1_000,
            source_detector: source_detector.to_owned(),
            source_snapshot_id: source_snapshot_id(&digest),
            schema_version: POLICY_INSTANCE_SEED_SCHEMA.to_owned(),
            resource_bundle_hash: digest,
            invalidate_on: Vec::new(),
        })
        .map(|_| ())
    }

    /// Workflow #392 DA-A (model 392 v1.1, sections 8.1 and 12.1): records the display of a
    /// task's first frame whose geometry passed as the four instance facts `display.frame_extent`,
    /// `display.rotation`, `display.orientation` and `display.aspect`. Nothing checks or refuses
    /// them. Only an instance bound to a device (physical provenance with a device endpoint, the
    /// instances whose connection is self-checked) has a display to record; any other instance
    /// records nothing. When the instance's active records already hold these four values,
    /// nothing is appended; otherwise the four records are one new observation at the host
    /// clock, through [`Self::publish_facts`] (source `runtime`, system links), whose snapshot
    /// id `display:<digest>` and `resource_bundle_hash` are the SHA-256 of the instance alias
    /// and the values. A refusal is returned to the caller.
    pub(super) fn publish_display_facts(
        &self,
        instance_id: InstanceId,
        instance_alias: &str,
        extent: CaptureExtent,
        geometry: &CaptureGeometryObservation,
    ) -> RuntimeHostResult<()> {
        let device_bound = lock(
            &self.registered_instances,
            HostLocation::ObserveTaskGeometry,
        )?
        .get(&instance_id)
        .is_some_and(RegisteredInstance::device_self_checked);
        if !device_bound {
            return Ok(());
        }
        let rotation = match geometry {
            CaptureGeometryObservation::Observed(geometry) => match geometry.rotation {
                CaptureRotationObservation::Observed { rotation, .. } => Some(rotation),
                CaptureRotationObservation::NotProvidedBySource => None,
            },
            CaptureGeometryObservation::Unknown(_)
            | CaptureGeometryObservation::NotApplicable(_) => None,
        };
        let display = DisplayFacts::new(extent, rotation);
        let values = display.values();
        let scope = FactScope::Instance {
            instance_id: instance_alias.to_owned(),
        };
        let recorded = {
            let _gate = lock(&self.fact_write_gate, HostLocation::ObserveTaskGeometry)?;
            let stored = lock(&self.facts, HostLocation::ObserveTaskGeometry)?;
            values.iter().all(|(key, value)| {
                stored.active_record(&scope, key).is_some_and(|record| {
                    matches!(&record.content, FactContent::Inline { value: active } if active == value)
                })
            })
        };
        if recorded {
            return Ok(());
        }
        let digest = display.digest(instance_alias);
        let observed_at_unix_ms = self.clock.sample()?.unix_ms;
        self.publish_facts(
            actingcommand_contract::FactObservation {
                records: values
                    .into_iter()
                    .map(|(key, value)| FactRecord {
                        scope: scope.clone(),
                        key: key.to_owned(),
                        content: FactContent::Inline { value },
                        observed_at_unix_ms,
                        expires_at_unix_ms: None,
                        ttl_policy: None,
                        confidence_milli: 1_000,
                        source_detector: DISPLAY_FACT_DETECTOR.to_owned(),
                        source_snapshot_id: format!("display:{digest}"),
                        schema_version: DISPLAY_FACT_SCHEMA.to_owned(),
                        resource_bundle_hash: digest.clone(),
                        invalidate_on: Vec::new(),
                    })
                    .collect(),
            },
            None,
            FactPublicationPurpose::Ordinary,
        )
        .map(|_| ())
    }

    pub(super) fn instance_fact_snapshot(
        &self,
        context: InstanceFactContext,
    ) -> RuntimeHostResult<InstanceFactSnapshot> {
        let _gate = lock(
            &self.fact_write_gate,
            HostLocation::ReadInstanceFactSnapshot,
        )?;
        self.synchronize_fact_store_under_gate()?;
        let ledger_position = self
            .ledger
            .latest_sequence()
            .map_err(|_| ledger_error(HostLocation::ReadInstanceFactPosition))?;
        lock(&self.facts, HostLocation::ReadInstanceFactSnapshot)?
            .snapshot(context, ledger_position)
    }

    pub(super) fn append_event(
        &self,
        severity: EventSeverity,
        source: EventSource,
        module: OriginModule,
        actor: EventActor,
        links: EventLinksDraft,
        payload: impl Into<actingcommand_contract::EventPayloadDraft>,
    ) -> Result<PersistedEvent, RequestFailure> {
        self.append_event_raw(severity, source, module, actor, links, payload)
            .map_err(RequestFailure::poison_without_terminal)
    }

    pub(super) fn append_event_observed(
        &self,
        severity: EventSeverity,
        source: EventSource,
        module: OriginModule,
        actor: EventActor,
        links: EventLinksDraft,
        payload: impl Into<actingcommand_contract::EventPayloadDraft>,
    ) -> (
        Result<PersistedEvent, RequestFailure>,
        task_timing::AppendObservation,
    ) {
        let mut observation = task_timing::AppendObservation::default();
        let result = self
            .append_event_raw_with_observation(
                severity,
                source,
                module,
                actor,
                links,
                (payload, Some(&mut observation)),
            )
            .map_err(RequestFailure::poison_without_terminal);
        (result, observation)
    }

    pub(super) fn append_event_raw(
        &self,
        severity: EventSeverity,
        source: EventSource,
        module: OriginModule,
        actor: EventActor,
        links: EventLinksDraft,
        payload: impl Into<actingcommand_contract::EventPayloadDraft>,
    ) -> RuntimeHostResult<PersistedEvent> {
        self.append_event_raw_with_observation(
            severity,
            source,
            module,
            actor,
            links,
            (payload, None),
        )
    }

    fn append_event_raw_with_observation(
        &self,
        severity: EventSeverity,
        source: EventSource,
        module: OriginModule,
        actor: EventActor,
        links: EventLinksDraft,
        input: (
            impl Into<actingcommand_contract::EventPayloadDraft>,
            Option<&mut task_timing::AppendObservation>,
        ),
    ) -> RuntimeHostResult<PersistedEvent> {
        let (payload, mut observation) = input;
        if let Some(value) = observation.as_mut() {
            value.fact_gate.begin();
        }
        let gate = lock(&self.fact_write_gate, HostLocation::AppendRuntimeEvent);
        if let Some(value) = observation.as_mut() {
            value.fact_gate.finish(gate.is_ok());
        }
        let gate = gate?;
        let event = self.append_event_under_fact_gate_with_observation(
            severity,
            source,
            module,
            actor,
            links,
            (payload, observation.as_deref_mut()),
        )?;
        if let Some(value) = observation.as_mut() {
            value.fact_sync.begin();
        }
        let synchronized = self.synchronize_fact_store_under_gate();
        if let Some(value) = observation.as_mut() {
            value.fact_sync.finish(synchronized.is_ok());
        }
        synchronized?;
        drop(gate);
        if let Some(value) = observation.as_mut() {
            value.pipeline.begin();
        }
        let pipeline = self.observe_pipeline_event(&event);
        if let Some(value) = observation.as_mut() {
            value.pipeline.finish(pipeline.is_ok());
        }
        pipeline?;
        Ok(event)
    }

    pub(super) fn append_event_under_fact_gate(
        &self,
        severity: EventSeverity,
        source: EventSource,
        module: OriginModule,
        actor: EventActor,
        links: EventLinksDraft,
        payload: impl Into<actingcommand_contract::EventPayloadDraft>,
    ) -> RuntimeHostResult<PersistedEvent> {
        self.append_event_under_fact_gate_with_observation(
            severity,
            source,
            module,
            actor,
            links,
            (payload, None),
        )
    }

    fn append_event_under_fact_gate_with_observation(
        &self,
        severity: EventSeverity,
        source: EventSource,
        module: OriginModule,
        actor: EventActor,
        links: EventLinksDraft,
        input: (
            impl Into<actingcommand_contract::EventPayloadDraft>,
            Option<&mut task_timing::AppendObservation>,
        ),
    ) -> RuntimeHostResult<PersistedEvent> {
        let (payload, mut observation) = input;
        if let Some(value) = observation.as_mut() {
            value.draft.begin();
        }
        let draft = (|| {
            let draft =
                self.events
                    .draft(severity, source, module, actor, links.clone(), payload)?;
            self.events.sanitize(draft)
        })();
        if let Some(value) = observation.as_mut() {
            value.draft.finish(draft.is_ok());
        }
        let draft = draft?;
        if let Some(value) = observation.as_mut() {
            value.writer_response.begin();
        }
        let appended = if let Some(value) = observation.as_mut() {
            let (result, ledger_observation) = self.ledger.append_with_observation(draft);
            value.ledger = ledger_observation;
            result
        } else {
            self.ledger.append(draft)
        };
        if let Some(value) = observation.as_mut() {
            value.writer_response.finish(appended.is_ok());
        }
        // Workflow #383 AO-1: no latch refuses the later appends; a write the ledger gave up on
        // stops the Runtime by its dump (`contracts/ledger-observer.md` §9.2).
        let event = appended.map_err(|_| ledger_error(HostLocation::AppendRuntimeEvent))?;
        // Workflow #383 AO-7: the record is committed; its wake, if any, reaches the driver
        // from here, not from the ledger.
        self.policy_wakes.raise(event.event_type());
        if let Some(value) = observation.as_mut() {
            value.device_diagnostics.begin();
        }
        let diagnostic = self.observe_device_diagnostics_under_fact_gate(&event);
        if let Some(value) = observation.as_mut() {
            value.device_diagnostics.finish(diagnostic.is_ok());
        }
        if let Err(error) = diagnostic {
            self.fatal.mark(error.clone())?;
            return Err(error);
        }
        // Workflow #387 D1c (M387 §3.4): direction ①'s count of the committed record, in memory
        // under its own leaf lock; nothing is recorded and no ladder is admitted on the write
        // path (the lease sweep admits what it registered).
        if let Err(error) = self.count_stability_record(&event) {
            self.fatal.mark(error.clone())?;
            return Err(error);
        }
        Ok(event)
    }

    pub(super) fn synchronize_fact_store(&self) -> RuntimeHostResult<()> {
        let _gate = lock(&self.fact_write_gate, HostLocation::SynchronizeFactStore)?;
        self.synchronize_fact_store_under_gate()
    }

    pub(super) fn synchronize_fact_store_under_gate(&self) -> RuntimeHostResult<()> {
        let result: RuntimeHostResult<()> = (|| {
            let mut facts = lock(&self.facts, HostLocation::SynchronizeFactStore)?;
            facts.synchronize(&self.ledger)?;
            for invalidation in facts.pending_invalidations() {
                let work = facts.prepare_invalidation(&self.ledger, &invalidation)?;
                let links = self.events.system_links()?;
                let draft = self.events.draft(
                    EventSeverity::Info,
                    EventSource::Runtime,
                    OriginModule::FactStore,
                    EventActor::Runtime,
                    links.clone(),
                    FactPayloadDraft::invalidated(invalidation.data.clone(), AuditInput::new()),
                )?;
                let draft = self.events.sanitize(draft)?;
                let persisted = self
                    .ledger
                    .append_transaction(draft, Box::new(work))
                    .map_err(crate::fact_store::fact_transaction_error)?;
                // Workflow #383 AO-7: a committed invalidation changes the instance fact store.
                self.policy_wakes.raise(persisted.event_type());
                facts
                    .acknowledge_generated_invalidation(&invalidation.data, persisted.sequence())
                    .and_then(|()| self.observe_device_diagnostics_under_fact_gate(&persisted))
                    .inspect_err(|error| {
                        let _ = error
                            .diagnostics()
                            .recorded_event()
                            .set(*persisted.event_id());
                    })?;
            }
            Ok(())
        })();
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.fatal.mark(error.clone())?;
        }
        result
    }
}
