// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::codes::{HostCode, HostLocation};
use crate::fact_store::{
    AppliedInvalidation, FactOrder, FactTrigger, priority_offset_milli,
    valid_priority_offset_task_id,
};
use actingcommand_contract::codes::{ContractLocation, SharedCode};
use actingcommand_contract::{
    CaptureExtent, CaptureGeometryObservation, CaptureItem, CaptureRotation,
    CaptureRotationObservation, EventOrigin, IssuedEventId, LedgerCapture, LedgerSource,
    RESOURCE_TARGETS_FACT_KEY, RuntimeStateStoreRevision, SanitizedEventDraft, SourceRevision,
    priority_offset_task_id,
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
    /// from the key's active record, which the `fact.published` `base_event` carried (Workflow
    /// #383 AO-3: identified by its publication, not its ledger position). Under
    /// `fact_write_gate` the record is refused when another record of the key has replaced that
    /// one since.
    ResourceAdjustment { base_event: EventId },
}

/// Where one publication landed: the event that holds its records, its place in the instance
/// store's history, its ledger sequence when its capture waited for the commit (a resource
/// target policy, whose receipt carries it until AO-4), the sequence of the record active before
/// it (resource target policies only) and whether nothing was appended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FactPublication {
    pub(super) event_id: EventId,
    pub(super) order: FactOrder,
    pub(super) sequence: Option<u64>,
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
                if let FactPublicationPurpose::ResourceAdjustment { base_event } = purpose {
                    let facts = lock(&self.facts, ContractLocation::PublishFact)?;
                    let current = match observation.records.as_slice() {
                        [record] => facts
                            .active_revision(&record.scope, &record.key)
                            .map(|active| active.event_id),
                        _ => None,
                    };
                    if current != Some(base_event) {
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
                    if let Some(active) = facts.active_revision(&record.scope, &record.key) {
                        if active.record.resource_bundle_hash == record.resource_bundle_hash
                            && !active.record.is_expired(now)
                        {
                            return Ok(FactPublication {
                                event_id: active.event_id,
                                order: active.order,
                                sequence: active.sequence,
                                previous_version: active.sequence,
                                replayed: true,
                            });
                        }
                        previous_version = active.sequence;
                    }
                }
                let preview = lock(&self.facts, ContractLocation::PublishFact)?
                    .preview_observation(&observation, now)?;
                if let Some(event_id) = preview {
                    let record = &observation.records[0];
                    let facts = lock(&self.facts, ContractLocation::PublishFact)?;
                    let active = facts
                        .active_revision(&record.scope, &record.key)
                        .filter(|active| active.event_id == event_id)
                        .ok_or_else(|| {
                            RuntimeHostError::fatal(
                                HostCode::FactPublicationSequenceMissing,
                                ContractLocation::PublishFacts,
                                RuntimeErrorCode::RuntimeHalted,
                            )
                        })?;
                    return Ok(FactPublication {
                        event_id,
                        order: active.order,
                        sequence: active.sequence,
                        previous_version: active.sequence,
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
                let records = observation.records.clone();
                let payload = FactPayloadDraft::observation(
                    observation,
                    scope_instances.clone(),
                    AuditInput::new(),
                )
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
                if !lock(&self.facts, ContractLocation::PublishFact)?.is_live() {
                    // Workflow #383 AO-3: until business opens the store follows the ledger, so
                    // the publication is appended and the store takes it from there.
                    let event = self.append_event_under_fact_gate(
                        EventSeverity::Info,
                        event_source,
                        OriginModule::FactStore,
                        event_actor,
                        links,
                        payload,
                    )?;
                    self.synchronize_fact_store_under_gate()?;
                    return Ok(FactPublication {
                        event_id: *event.event_id(),
                        order: FactOrder::Replayed(event.sequence()),
                        sequence: Some(event.sequence()),
                        previous_version,
                        replayed: false,
                    });
                }
                // Workflow #383 AO-3 (`contracts/ledger-observer.md` §3, G2): the store's one
                // entry applies the publication first; the gate copies it to the ledger without
                // waiting. A resource target policy's receipt still carries the ledger sequence
                // until AO-4, so that capture waits for its commit.
                let event_id = self.events.issue_event_id()?;
                let published = *event_id.transport();
                let revision = lock(&self.facts, ContractLocation::PublishFact)?
                    .apply_publication(&records, &scope_instances, event_actor, published)?;
                let (order, sequence) = self.capture_store_change_under_gate(
                    event_id,
                    EventType::FactPublished,
                    EventOrigin::new(event_source, OriginModule::FactStore, event_actor),
                    links,
                    payload.into(),
                    revision,
                    purpose == FactPublicationPurpose::ResourceTargets,
                )?;
                lock(&self.facts, ContractLocation::PublishFact)?
                    .settle_publication(published, order, sequence);
                Ok(FactPublication {
                    event_id: published,
                    order,
                    sequence,
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

    /// Workflow #384 D2-S3 (D2 model §3.6): `key`'s active record in `scope`, with its place in
    /// the store's history and the id of the `fact.published` that carried it (Workflow #383
    /// AO-3), read under `fact_write_gate`; `None` when the key has none.
    pub(super) fn active_fact_revision(
        &self,
        scope: &FactScope,
        key: &str,
    ) -> RuntimeHostResult<Option<(FactRecord, FactOrder, EventId)>> {
        let result: RuntimeHostResult<Option<(FactRecord, FactOrder, EventId)>> = (|| {
            let _gate = lock(&self.fact_write_gate, HostLocation::PublishResourceReadings)?;
            self.synchronize_fact_store_under_gate()?;
            Ok(lock(&self.facts, HostLocation::PublishResourceReadings)?
                .active_revision(scope, key)
                .map(|active| (active.record.clone(), active.order, active.event_id)))
        })();
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.fatal.mark(error.clone())?;
        }
        result
    }

    /// Workflow #384 D2-S3 (D2 model §3.6): drops `key`'s active record in `scope`, the one
    /// `fact.published` `base_event` carried, because a run confirmed an operation that declares
    /// an adjustment of the key and read no new value: one `fact.invalidated` under
    /// `fact_write_gate`, whose trigger is that step's `task.step_finished`, `trigger`. A key
    /// without an active record is left as it is; another record than that one is refused
    /// (`fact_observation_version_conflict`). Workflow #383 AO-3: the store's one entry applies it
    /// and the gate copies it (G2).
    pub(super) fn invalidate_adjusted_fact(
        &self,
        scope: &FactScope,
        key: &str,
        base_event: EventId,
        trigger: FactTrigger,
    ) -> RuntimeHostResult<()> {
        let result: RuntimeHostResult<()> = (|| {
            let _gate = lock(&self.fact_write_gate, HostLocation::PublishResourceReadings)?;
            self.synchronize_fact_store_under_gate()?;
            let applied = {
                let mut facts = lock(&self.facts, HostLocation::PublishResourceReadings)?;
                if facts.active_revision(scope, key).is_none() {
                    return Ok(());
                }
                facts.apply_adjustment_invalidation(scope, key, base_event, &trigger)?
            };
            let Some(applied) = applied else {
                return Err(RuntimeHostError::request(
                    HostCode::FactObservationVersionConflict,
                    ContractLocation::PublishFacts,
                    RuntimeErrorCode::InvalidRequest,
                ));
            };
            self.capture_invalidations_under_gate(vec![applied])
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
        // #411 A51 (R3): a position the writer did not answer in time fails only this read.
        let ledger_position = self
            .ledger
            .latest_sequence()
            .map_err(|error| ledger_read_error(&error, HostLocation::ReadInstanceFactPosition))?;
        // Workflow #383 AO-3 (§13): the snapshot id is the two stores' revisions and the records.
        let workbench = self.runtime_facts_revision();
        lock(&self.facts, HostLocation::ReadInstanceFactSnapshot)?.snapshot(
            context,
            ledger_position,
            workbench,
        )
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
        // Workflow #383 AO-3 (`contracts/ledger-observer.md` §3, G1): the fan-out point hands the
        // record to the host's one capture point. Its callers still read its ledger sequence
        // (receipts, projections) until AO-4, so the hand-off waits for the commit; inside the
        // gate it is the only writer round trip: the store no longer reads the ledger back.
        let mut ledger_observation = None;
        let observing = observation.is_some();
        let appended = self.capture_committed_under_gate(draft, |ledger, draft| {
            let result = if observing {
                let (result, observed) = ledger.append_with_observation(draft);
                ledger_observation = observed;
                result
            } else {
                ledger.append(draft)
            };
            // Workflow #383 AO-1: no latch refuses the later appends; a write the ledger gave up
            // on stops the Runtime by its dump (`contracts/ledger-observer.md` §9.2).
            result.map_err(|_| ledger_error(HostLocation::AppendRuntimeEvent))
        });
        if let Some(value) = observation.as_mut() {
            value.ledger = ledger_observation;
            value.writer_response.finish(appended.is_ok());
        }
        let event = appended?;
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
        Ok(event)
    }

    /// Workflow #383 AO-3, `contracts/ledger-observer.md` §3: the host's one capture point for a
    /// record whose caller needs it committed (until AO-4 its ledger sequence still reaches
    /// receipts and projections), under `fact_write_gate`. `hand_off` gives the sanitized record
    /// to the ledger and waits for its commit; the capture then observes it
    /// ([`Self::observe_committed_under_gate`]).
    pub(super) fn capture_committed_under_gate(
        &self,
        draft: SanitizedEventDraft,
        hand_off: impl FnOnce(&GlobalLedger, SanitizedEventDraft) -> RuntimeHostResult<PersistedEvent>,
    ) -> RuntimeHostResult<PersistedEvent> {
        let event = hand_off(&self.ledger, draft)?;
        #[cfg(test)]
        self.count_capture_for_test(event.event_type(), true);
        self.observe_committed_under_gate(&event)?;
        Ok(event)
    }

    /// Workflow #383 AO-3, `contracts/ledger-observer.md` §3: one committed record passes the
    /// capture's observation step, under `fact_write_gate`: the instance fact store observes it
    /// ([`Self::observe_store_under_gate`]), then the record's business notification is raised
    /// (AO-7) and the recovery ladder's direction ① counts it (#387 D1c, moved here from the
    /// append path).
    pub(super) fn observe_committed_under_gate(
        &self,
        event: &PersistedEvent,
    ) -> RuntimeHostResult<()> {
        let result: RuntimeHostResult<()> = (|| {
            self.observe_store_under_gate(event)?;
            // Workflow #383 AO-7: the record is committed; its wake, if any, reaches the driver
            // from here, not from the ledger.
            self.policy_wakes.raise(event.event_type());
            // Workflow #387 D1c (M387 §3.4): direction ①'s count of the committed record, in
            // memory under its own leaf lock; nothing is recorded and no ladder is admitted here
            // (the lease sweep admits what it registered).
            self.count_stability_record(event)
        })();
        if let Err(error) = &result {
            self.fatal.mark(error.clone())?;
        }
        result
    }

    /// Workflow #383 AO-3 (§3): the store's half of the observation step, under
    /// `fact_write_gate`. Once the instance fact store is live it observes the record
    /// (`observe_trigger`: the input boundary, then the `invalidate_on` invalidations it causes,
    /// applied at once) and the invalidations are captured after it. Records committed before it
    /// outside the fan-out point are observed first, in ledger order.
    fn observe_store_under_gate(&self, event: &PersistedEvent) -> RuntimeHostResult<()> {
        self.drain_deferred_observations_under_gate(Some(event.sequence()))?;
        let applied = {
            let mut facts = lock(&self.facts, HostLocation::DeriveFactInvalidation)?;
            if facts.is_live() {
                facts.observe_trigger(&FactTrigger::committed(event))?
            } else {
                Vec::new()
            }
        };
        self.capture_invalidations_under_gate(applied)
    }

    /// Workflow #383 AO-3 (§3): a record committed outside the fan-out point, by a writer that
    /// AO-4 moves to its gate (a critical execution, a joint transaction, an artifact sink, a
    /// direct append) or by the ledger itself (the settlement continuation, the Lab pin release).
    /// Its business notification is raised and the recovery ladder's direction ① counts it at
    /// once; the instance fact store observes it, in ledger order, at the next holder of
    /// `fact_write_gate` before that holder reads or changes the store
    /// ([`Self::synchronize_fact_store_under_gate`], [`Self::observe_store_under_gate`]). Takes no
    /// gate, so a writer may call it whatever it holds.
    pub(super) fn defer_observation(&self, event: &PersistedEvent) -> RuntimeHostResult<()> {
        let result: RuntimeHostResult<()> = (|| {
            self.policy_wakes.raise(event.event_type());
            self.count_stability_record(event)?;
            lock(
                &self.deferred_observations,
                HostLocation::DeriveFactInvalidation,
            )?
            .push(event.clone());
            Ok(())
        })();
        if let Err(error) = &result {
            self.fatal.mark(error.clone())?;
        }
        result
    }

    /// Workflow #383 AO-3 (§3): the deferred records committed before ledger sequence `before`
    /// (all of them when `None`), observed in ledger order by a live store. A replaying store
    /// takes them from the ledger itself.
    fn drain_deferred_observations_under_gate(&self, before: Option<u64>) -> RuntimeHostResult<()> {
        let mut drained = {
            let mut deferred = lock(
                &self.deferred_observations,
                HostLocation::DeriveFactInvalidation,
            )?;
            let (earlier, later): (Vec<_>, Vec<_>) = std::mem::take(&mut *deferred)
                .into_iter()
                .partition(|event| before.is_none_or(|before| event.sequence() < before));
            *deferred = later;
            earlier
        };
        let replayed_through = {
            let facts = lock(&self.facts, HostLocation::DeriveFactInvalidation)?;
            if !facts.is_live() {
                return Ok(());
            }
            facts.replayed_through()
        };
        // A record the start's replay already followed (committed before the store went live,
        // deferred after) is not observed twice.
        drained.retain(|event| event.sequence() > replayed_through);
        drained.sort_by_key(PersistedEvent::sequence);
        for event in &drained {
            let applied = lock(&self.facts, HostLocation::DeriveFactInvalidation)?
                .observe_trigger(&FactTrigger::committed(event))?;
            self.capture_invalidations_under_gate(applied)?;
        }
        Ok(())
    }

    /// Workflow #383 AO-3, G16 (§3): the close's device diagnostic summary, whose append is
    /// deferred and confirmed at the close boundary, under `fact_write_gate`. The store observes
    /// it at its capture, from its draft (its record's time is the draft's), and the invalidations
    /// it causes are captured after it.
    pub(super) fn capture_deferred_under_gate(
        &self,
        draft: SanitizedEventDraft,
        operation: HostLocation,
    ) -> RuntimeHostResult<()> {
        self.drain_deferred_observations_under_gate(None)?;
        let applied = {
            let mut facts = lock(&self.facts, HostLocation::DeriveFactInvalidation)?;
            if facts.is_live() {
                facts.observe_trigger(&FactTrigger::at_capture(&draft))?
            } else {
                Vec::new()
            }
        };
        #[cfg(test)]
        let event_type = draft.event_type();
        self.ledger
            .append_deferred(draft)
            .map(|_| ())
            .map_err(|_| ledger_error(operation))?;
        #[cfg(test)]
        self.count_capture_for_test(event_type, true);
        self.capture_invalidations_under_gate(applied)
    }

    /// Workflow #383 AO-3 (`contracts/ledger-observer.md` §2 call 1, §4 G2, §8.3): copies one
    /// change of the instance fact store, which its entry applied already, to the ledger: the
    /// record of `revision` in the store's lineage. It is captured through the capture interface
    /// without a wait; `committed` hands it over with a wait for its commit instead, for a caller
    /// whose receipt still carries the ledger sequence until AO-4 (a resource target policy).
    /// The capture interface is held here only (architecture guard AO3-j). Returns the record's
    /// place and, when it waited, its ledger sequence.
    #[allow(clippy::too_many_arguments)]
    fn capture_store_change_under_gate(
        &self,
        event_id: IssuedEventId,
        event_type: EventType,
        origin: EventOrigin,
        links: EventLinksDraft,
        payload: actingcommand_contract::EventPayloadDraft,
        revision: u64,
        committed: bool,
    ) -> RuntimeHostResult<(FactOrder, Option<u64>)> {
        let source = SourceRevision {
            source: LedgerSource::InstanceFacts,
            lineage: self.owner_epoch,
            revision,
        };
        let placed = if committed {
            let draft = actingcommand_contract::EventDraft::new(
                event_id,
                self.clock.sample()?.unix_ms,
                EventSeverity::Info,
                origin,
                links,
                payload,
            );
            let draft = self.events.sanitize(draft)?;
            let event = self
                .ledger
                .append_with_source(draft, source)
                .map_err(|_| ledger_error(HostLocation::AppendRuntimeEvent))?;
            #[cfg(test)]
            self.count_capture_for_test(event_type, true);
            let order = FactOrder::of_live_event(&event).ok_or_else(|| {
                RuntimeHostError::fatal(
                    HostCode::FactPublicationSequenceMissing,
                    HostLocation::AppendRuntimeEvent,
                    RuntimeErrorCode::RuntimeHalted,
                )
            })?;
            (order, Some(event.sequence()))
        } else {
            let ticket = self.ledger_capture.capture(CaptureItem {
                event_id,
                severity: EventSeverity::Info,
                payload,
                links,
                origin,
                source: Some(source),
            });
            #[cfg(test)]
            self.count_capture_for_test(event_type, false);
            (FactOrder::Captured(ticket.capture_seq), None)
        };
        // Workflow #383 AO-7: an instance fact publication or invalidation wakes the driver.
        self.policy_wakes.raise(event_type);
        Ok(placed)
    }

    /// Workflow #383 AO-3 (§3 step 3): captures, in order and after their trigger, the
    /// invalidations the store applied for it; each is one change of the store (G2).
    pub(super) fn capture_invalidations_under_gate(
        &self,
        applied: Vec<AppliedInvalidation>,
    ) -> RuntimeHostResult<()> {
        for invalidation in applied {
            let event_id = self.events.issue_event_id()?;
            let links = self.events.system_links()?;
            let (order, _) = self.capture_store_change_under_gate(
                event_id,
                EventType::FactInvalidated,
                EventOrigin::new(
                    EventSource::Runtime,
                    OriginModule::FactStore,
                    EventActor::Runtime,
                ),
                links,
                FactPayloadDraft::invalidated(invalidation.data.clone(), AuditInput::new()).into(),
                invalidation.revision,
                false,
            )?;
            lock(&self.facts, HostLocation::DeriveFactInvalidation)?
                .settle_invalidation(&invalidation.data, order);
        }
        Ok(())
    }

    /// Workflow #383 AO-3 (§13, register W1): the scheduler workbench's lineage (the owner
    /// epoch) and its revision, +1 per change it accepts once business is open. Read under
    /// `fact_write_gate` for a pair consistent with the instance fact store's.
    pub(super) fn runtime_facts_revision(&self) -> RuntimeStateStoreRevision {
        RuntimeStateStoreRevision {
            lineage: self.owner_epoch,
            revision: self.runtime_facts_revision.load(Ordering::Acquire),
        }
    }

    /// Workflow #383 AO-3: one change the scheduler workbench accepted, under `fact_write_gate`.
    pub(super) fn count_runtime_fact_change(&self) {
        self.runtime_facts_revision.fetch_add(1, Ordering::AcqRel);
    }

    /// Workflow #383 AO-3 (`contracts/ledger-observer.md` §3): business opens. The instance fact
    /// store follows the ledger one last time, writes what the start's replay derived, and goes
    /// live: from here on it reads no ledger and every recorded event reaches it at capture. Both
    /// stores start their revisions at 0 in this epoch's lineage. Fails Loud when the replay
    /// does not settle.
    pub(super) fn open_live_fact_stores(&self) -> RuntimeHostResult<()> {
        let result: RuntimeHostResult<()> = (|| {
            let _gate = lock(&self.fact_write_gate, HostLocation::SynchronizeFactStore)?;
            // A written invalidation settles when the store follows the ledger past it; writing
            // one derives nothing new, so two rounds settle a quiet start. Writers outside the
            // gate may append meanwhile; a few more rounds let them settle too.
            for _ in 0..8 {
                self.synchronize_fact_store_under_gate()?;
                if !lock(&self.facts, HostLocation::SynchronizeFactStore)?.has_pending() {
                    break;
                }
            }
            lock(&self.facts, HostLocation::SynchronizeFactStore)?.go_live()?;
            self.runtime_facts_revision.store(0, Ordering::Release);
            Ok(())
        })();
        if let Err(error) = &result {
            self.fatal.mark(error.clone())?;
        }
        result
    }

    /// Workflow #383 AO-3, tests: what the host's capture point handed the ledger so far.
    #[cfg(test)]
    fn count_capture_for_test(&self, event_type: EventType, committed: bool) {
        self.ledger_captures_for_test
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(LedgerCaptureForTest {
                event_type,
                committed,
            });
    }

    pub(super) fn synchronize_fact_store(&self) -> RuntimeHostResult<()> {
        let _gate = lock(&self.fact_write_gate, HostLocation::SynchronizeFactStore)?;
        self.synchronize_fact_store_under_gate()
    }

    /// Until business opens, the instance fact store follows the ledger tail, and the
    /// invalidations it derives are written here (Workflow #383 AO-3: as plain records; the store
    /// keeps its tombstones and the next start rebuilds their projection from the ledger). A live
    /// store reads no ledger: it has seen every record of the fan-out point at its capture, and
    /// here it observes the records committed outside it (Workflow #383 AO-3, §3).
    pub(super) fn synchronize_fact_store_under_gate(&self) -> RuntimeHostResult<()> {
        let result: RuntimeHostResult<()> = (|| {
            self.drain_deferred_observations_under_gate(None)?;
            let pending = {
                let mut facts = lock(&self.facts, HostLocation::SynchronizeFactStore)?;
                facts.synchronize(&self.ledger)?;
                facts.pending_invalidations()
            };
            for invalidation in pending {
                let links = self.events.system_links()?;
                let draft = self.events.draft(
                    EventSeverity::Info,
                    EventSource::Runtime,
                    OriginModule::FactStore,
                    EventActor::Runtime,
                    links,
                    FactPayloadDraft::invalidated(invalidation.data.clone(), AuditInput::new()),
                )?;
                let draft = self.events.sanitize(draft)?;
                let persisted = self.capture_committed_under_gate(draft, |ledger, draft| {
                    ledger
                        .append(draft)
                        .map_err(|_| ledger_error(HostLocation::AppendFactTransaction))
                })?;
                lock(&self.facts, HostLocation::SynchronizeFactStore)?
                    .acknowledge_generated_invalidation(&invalidation.data)
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

/// Workflow #383 AO-3 (`contracts/ledger-observer.md` §3): a critical execution's appender. The
/// execution still waits for each of its records' commit (until AO-4 lets it go on without);
/// every record it appended then passes the capture's observation step
/// ([`CapturingAppender::observe_under_gate`] for a caller that holds `fact_write_gate`,
/// [`CapturingAppender::defer`] otherwise).
pub(super) struct CapturingAppender<'a> {
    shared: &'a HostShared,
    appended: std::cell::RefCell<Vec<PersistedEvent>>,
}

impl HostShared {
    pub(super) fn capturing_appender(&self) -> CapturingAppender<'_> {
        CapturingAppender {
            shared: self,
            appended: std::cell::RefCell::new(Vec::new()),
        }
    }
}

impl actingcommand_ledger::critical::EventAppender for CapturingAppender<'_> {
    fn append_durable(
        &self,
        draft: SanitizedEventDraft,
    ) -> actingcommand_ledger::GlobalLedgerResult<PersistedEvent> {
        let event = self.shared.ledger.append_durable(draft)?;
        self.appended.borrow_mut().push(event.clone());
        Ok(event)
    }
}

impl CapturingAppender<'_> {
    /// The appended records pass the observation step now; the caller holds `fact_write_gate`.
    pub(super) fn observe_under_gate(self) -> RuntimeHostResult<()> {
        for event in self.appended.into_inner() {
            self.shared.observe_committed_under_gate(&event)?;
        }
        Ok(())
    }

    /// The appended records pass the observation step at the next holder of `fact_write_gate`.
    pub(super) fn defer(self) -> RuntimeHostResult<()> {
        for event in self.appended.into_inner() {
            self.shared.defer_observation(&event)?;
        }
        Ok(())
    }
}

/// Workflow #383 AO-3, tests: one record the host's capture point handed the ledger: its type,
/// and whether the hand-off waited for its commit (a synchronous ledger call) or was captured
/// without a wait.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LedgerCaptureForTest {
    pub(crate) event_type: EventType,
    pub(crate) committed: bool,
}
