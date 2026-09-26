// SPDX-License-Identifier: AGPL-3.0-only

//! Host wiring for the Runtime's own fact store (Workflow #313).
//!
//! Every change is appended to the `GlobalLedger` before it enters the
//! memory-only store, under the same write gate as the instance fact store.
//! Nothing here reads a file: durability is the per-record event, and the
//! periodic snapshot only shortens replay.

use super::*;
use crate::policy_host::PolicySettlement;
use actingcommand_contract::{
    BackendObservationStatus, BackendOpenEntry, BackendOpenReport, FactValue,
    MAX_RUNTIME_FACT_KEY_BYTES,
};

/// Key families that stop describing the device once a new owner epoch starts.
const TAKEOVER_INVALIDATED_FAMILIES: [&str; 3] = ["device.", "backend.", "application."];

/// Self-check fact keys are `backend.selfcheck.<entry>.<suffix>` (Workflow #317 slice sc1),
/// `<entry>` being `input`, `capture` or `nemu`.
const BACKEND_SELFCHECK_PREFIX: &str = "backend.selfcheck.";
const BACKEND_SELFCHECK_STATUS_SUFFIX: &str = "status";
const BACKEND_SELFCHECK_GENERATION_SUFFIX: &str = "generation";
const BACKEND_SELFCHECK_SELECTED_SUFFIX: &str = "selected";
const BACKEND_SELFCHECK_CHECKED_AT_SUFFIX: &str = "checked_at_unix_ms";

/// The admitted package's `control.json` `game` / `server` declarations, copied verbatim.
pub(super) const TASK_GAME_FACT_KEY: &str = "task.game";
pub(super) const TASK_SERVER_FACT_KEY: &str = "task.server";
/// The page label the last contained-task recognition matched.
pub(super) const TASK_PAGE_FACT_KEY: &str = "task.page";

/// Single keys outside [`TAKEOVER_INVALIDATED_FAMILIES`] that a takeover also drops.
const TAKEOVER_INVALIDATED_KEYS: [&str; 1] = [TASK_PAGE_FACT_KEY];

/// Settlement fact keys are `task.<catalog_task_id>.<suffix>` (Workflow #308 slice 5b).
const TASK_LAST_DURATION_SUFFIX: &str = "last_duration_ms";
const TASK_LAST_OUTCOME_SUFFIX: &str = "last_outcome";
const TASK_FAILURE_STREAK_SUFFIX: &str = "failure_streak";
const TASK_COMPLETED_AT_SUFFIX: &str = "completed_at_unix_ms";
const TASK_SETTLEMENT_SUFFIXES: [&str; 4] = [
    TASK_LAST_DURATION_SUFFIX,
    TASK_LAST_OUTCOME_SUFFIX,
    TASK_FAILURE_STREAK_SUFFIX,
    TASK_COMPLETED_AT_SUFFIX,
];

fn task_settlement_fact_key(task_id: &str, suffix: &str) -> String {
    format!("task.{task_id}.{suffix}")
}

/// `passed` when the open's `status`, its `connection` and the entry's own check
/// (`input_check`, `capture_check`, both for a Nemu pair) all passed; `failed` when any of them
/// failed; otherwise `unknown`.
fn backend_selfcheck_status(report: &BackendOpenReport) -> &'static str {
    let (check, paired_check) = match report.entry {
        BackendOpenEntry::Input => (report.input_check, None),
        BackendOpenEntry::Capture => (report.capture_check, None),
        BackendOpenEntry::NemuPair => (report.capture_check, Some(report.input_check)),
    };
    let mut observed = [report.status, report.connection, check]
        .into_iter()
        .chain(paired_check);
    if observed
        .clone()
        .any(|status| status == BackendObservationStatus::Failed)
    {
        "failed"
    } else if observed.all(|status| status == BackendObservationStatus::Passed) {
        "passed"
    } else {
        "unknown"
    }
}

/// Refuses a catalog with a task whose settlement fact keys would exceed the 128-byte runtime
/// fact key bound (`task_id_too_long_for_facts`): a task id may be at most 102 bytes, checked
/// when a catalog becomes active, before any run of it.
pub(super) fn validate_settlement_fact_keys(
    catalog: &actingcommand_policy::CompiledCatalog,
) -> RuntimeHostResult<()> {
    for task in &catalog.catalog().tasks.tasks {
        if TASK_SETTLEMENT_SUFFIXES.iter().any(|suffix| {
            task_settlement_fact_key(&task.id, suffix).len() > MAX_RUNTIME_FACT_KEY_BYTES
        }) {
            return Err(RuntimeHostError::request(
                "task_id_too_long_for_facts",
                "activate_policy_catalog",
                RuntimeErrorCode::InvalidRequest,
            )
            .with_native_detail(format!(
                "task id '{}' is {} bytes; its settlement fact keys allow at most {} bytes",
                task.id,
                task.id.len(),
                MAX_RUNTIME_FACT_KEY_BYTES
                    - task_settlement_fact_key("", TASK_COMPLETED_AT_SUFFIX).len()
            )));
        }
    }
    Ok(())
}

impl HostShared {
    /// Records the in-memory runtime configuration manifest as its two
    /// `config.*` facts, ledger-first through [`Self::record_runtime_fact`].
    /// The clock is sampled once so both records share one observation time;
    /// on a restart that time is newer than the replayed records, so they are
    /// replaced rather than refused as stale. Any refusal fails startup.
    pub(super) fn record_config_manifest(
        &self,
        manifest: &RuntimeConfigManifest,
    ) -> RuntimeHostResult<()> {
        let observed_at_unix_ms = self.clock.sample()?.unix_ms;
        for record in manifest.to_fact_records(observed_at_unix_ms, OriginModule::Runtime) {
            self.record_runtime_fact(record)?;
        }
        Ok(())
    }

    /// Records the four `backend.selfcheck.<entry>.*` facts of one recorded backend open
    /// (Workflow #317 slice sc1), ledger-first through [`Self::record_runtime_fact`]: instance
    /// scope, source `device-proxy` (input, nemu) or `capture`, no lifetime, one clock sample as
    /// the observation time of all four. A newer open of the same entry replaces them and an
    /// identical record appends nothing; every refusal (including `runtime_fact_stale`) is
    /// returned. `checked_at_unix_ms` saturates at `i64::MAX` for a wall clock beyond it; the
    /// records' `observed_at_unix_ms` stays exact.
    pub(super) fn record_backend_selfcheck_facts(
        &self,
        instance_id: InstanceId,
        report: &BackendOpenReport,
    ) -> RuntimeHostResult<()> {
        let observed_at_unix_ms = self.clock.sample()?.unix_ms;
        let generation = i64::try_from(report.session_generation).map_err(|_| {
            RuntimeHostError::fatal(
                "backend_selfcheck_fact_overflow",
                "record_backend_selfcheck_facts",
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
        let (entry, source) = match report.entry {
            BackendOpenEntry::Input => ("input", OriginModule::DeviceProxy),
            BackendOpenEntry::Capture => ("capture", OriginModule::Capture),
            BackendOpenEntry::NemuPair => ("nemu", OriginModule::DeviceProxy),
        };
        let values = [
            (
                BACKEND_SELFCHECK_STATUS_SUFFIX,
                FactValue::String(backend_selfcheck_status(report).to_owned()),
            ),
            (
                BACKEND_SELFCHECK_GENERATION_SUFFIX,
                FactValue::Integer(generation),
            ),
            (
                BACKEND_SELFCHECK_SELECTED_SUFFIX,
                FactValue::String(report.selected.clone().unwrap_or_else(|| "-".to_owned())),
            ),
            (
                BACKEND_SELFCHECK_CHECKED_AT_SUFFIX,
                FactValue::Integer(i64::try_from(observed_at_unix_ms).unwrap_or(i64::MAX)),
            ),
        ];
        for (suffix, value) in values {
            self.record_runtime_fact(RuntimeFactRecord {
                scope: RuntimeFactScope::Instance { instance_id },
                key: format!("{BACKEND_SELFCHECK_PREFIX}{entry}.{suffix}"),
                value,
                observed_at_unix_ms,
                source,
                ttl_ms: None,
            })?;
        }
        Ok(())
    }

    /// Drops every `backend.selfcheck.*` fact the store holds for one instance with
    /// `device_closed`: the session they describe is closed and the instance endpoint was
    /// rebound (or returned to pending). Every refusal is returned.
    pub(super) fn invalidate_backend_selfcheck_facts(
        &self,
        instance_id: InstanceId,
    ) -> RuntimeHostResult<()> {
        let scope = RuntimeFactScope::Instance { instance_id };
        let keys = lock(&self.runtime_facts, "read_backend_selfcheck_facts")?
            .records()
            .filter(|record| {
                record.scope == scope && record.key.starts_with(BACKEND_SELFCHECK_PREFIX)
            })
            .map(|record| record.key.clone())
            .collect::<Vec<_>>();
        for key in keys {
            self.invalidate_runtime_fact(
                &scope,
                &key,
                RuntimeFactInvalidationReason::DeviceClosed,
            )?;
        }
        Ok(())
    }

    /// Appends `runtime.fact_recorded` first, then accepts the record into
    /// memory. A record the store would reject is refused before the append;
    /// an identical record appends nothing. The host is the only writer; the
    /// producers are emulator instance control (`device.connected`) and the
    /// startup configuration manifest (`config.subsystems`, `config.parameters`).
    pub(super) fn record_runtime_fact(
        &self,
        record: RuntimeFactRecord,
    ) -> RuntimeHostResult<RuntimeFactChange> {
        let result: RuntimeHostResult<RuntimeFactChange> = (|| {
            let _gate = lock(&self.fact_write_gate, "record_runtime_fact")?;
            let precheck = {
                let store = lock(&self.runtime_facts, "record_runtime_fact")?;
                precheck_runtime_fact(&store, &record)?
            };
            if let Some(unchanged) = precheck {
                return Ok(unchanged);
            }
            let links = self.runtime_fact_links(&record.scope)?;
            self.append_event_under_fact_gate(
                EventSeverity::Info,
                EventSource::Runtime,
                OriginModule::RuntimeFacts,
                EventActor::Runtime,
                links,
                RuntimePayloadDraft::fact_recorded(record.clone(), AuditInput::new()),
            )?;
            let change = lock(&self.runtime_facts, "record_runtime_fact")?
                .record(record)
                .map_err(|error| runtime_fact_desync(&error, "record_runtime_fact"))?;
            self.runtime_facts_dirty.store(true, Ordering::Release);
            self.synchronize_fact_store_under_gate()?;
            Ok(change)
        })();
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.fatal.mark(error.clone())?;
        }
        result
    }

    /// Records an instance-scope string fact only when it differs from the stored value;
    /// an unchanged value appends nothing and a same-millisecond observation keeps the
    /// first (`runtime_fact_stale` is not an error). No lifetime.
    pub(super) fn record_changed_instance_string_fact(
        &self,
        instance_id: InstanceId,
        key: &str,
        value: &str,
    ) -> RuntimeHostResult<()> {
        let scope = RuntimeFactScope::Instance { instance_id };
        let value = FactValue::String(value.to_owned());
        let unchanged = lock(&self.runtime_facts, "read_instance_string_fact")?
            .get(&scope, key)
            .is_some_and(|record| record.value == value);
        if unchanged {
            return Ok(());
        }
        let observed_at_unix_ms = self.clock.sample()?.unix_ms;
        match self.record_runtime_fact(RuntimeFactRecord {
            scope,
            key: key.to_owned(),
            value,
            observed_at_unix_ms,
            source: OriginModule::Runtime,
            ttl_ms: None,
        }) {
            Ok(_) => Ok(()),
            Err(error) if error.code() == "runtime_fact_stale" => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Records the four settlement facts of one (task, instance) pair's latest executed policy
    /// run (Workflow #308 slice 5b), ledger-first through [`Self::record_runtime_fact`], in the
    /// instance scope of the run's registered instance with source `policy` and no lifetime.
    /// Identical stored records append nothing, so a replayed or reconciled settlement never
    /// appends a record twice. Any refusal is fatal: the settlement itself is already durable.
    pub(super) fn record_policy_settlement_facts(
        &self,
        settlement: &PolicySettlement,
    ) -> RuntimeHostResult<()> {
        let result: RuntimeHostResult<()> = (|| {
            let instance_id = self
                .settlement_instance_id(&settlement.instance_alias)?
                .ok_or_else(|| {
                    RuntimeHostError::fatal(
                        "policy_settlement_instance_unknown",
                        "record_policy_settlement_facts",
                        RuntimeErrorCode::RuntimeFatal,
                    )
                })?;
            self.record_settlement_fact_records(settlement, instance_id)
        })();
        let result = result.map_err(|error| {
            if error.is_fatal() {
                error
            } else {
                error.into_fatal()
            }
        });
        if let Err(error) = &result {
            self.fatal.mark(error.clone())?;
        }
        result
    }

    /// Re-derives the settlement facts of every registered instance's pairs from the recovered
    /// dispatches once per startup, so a settlement recorded or reconciled without its facts
    /// (a stop between the two appends, or startup reconciliation, which runs before this
    /// store exists) reaches the same store state as the live path. Identical records append
    /// nothing; a pair of an instance no longer registered has no instance scope and is left
    /// as the ledger holds it.
    pub(super) fn record_policy_settlement_facts_on_start(&self) -> RuntimeHostResult<()> {
        let settlements = lock(&self.policy, "read_policy_settlements")?.latest_settlements()?;
        for settlement in &settlements {
            if self
                .settlement_instance_id(&settlement.instance_alias)?
                .is_none()
            {
                continue;
            }
            self.record_policy_settlement_facts(settlement)?;
        }
        Ok(())
    }

    fn settlement_instance_id(
        &self,
        instance_alias: &str,
    ) -> RuntimeHostResult<Option<InstanceId>> {
        Ok(lock(
            &self.registered_instances,
            "resolve_policy_settlement_instance",
        )?
        .values()
        .find(|instance| instance.instance_alias == instance_alias)
        .map(|instance| instance.instance_id))
    }

    fn record_settlement_fact_records(
        &self,
        settlement: &PolicySettlement,
        instance_id: InstanceId,
    ) -> RuntimeHostResult<()> {
        let integer = |value: u64| {
            i64::try_from(value).map(FactValue::Integer).map_err(|_| {
                RuntimeHostError::fatal(
                    "policy_settlement_fact_overflow",
                    "record_policy_settlement_facts",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })
        };
        let values = [
            (TASK_LAST_DURATION_SUFFIX, integer(settlement.duration_ms)?),
            (
                TASK_LAST_OUTCOME_SUFFIX,
                FactValue::String(
                    if settlement.succeeded {
                        "succeeded"
                    } else {
                        "failed"
                    }
                    .to_owned(),
                ),
            ),
            (
                TASK_FAILURE_STREAK_SUFFIX,
                integer(settlement.failure_streak)?,
            ),
            (
                TASK_COMPLETED_AT_SUFFIX,
                integer(settlement.completed_at_unix_ms)?,
            ),
        ];
        for (suffix, value) in values {
            self.record_runtime_fact(RuntimeFactRecord {
                scope: RuntimeFactScope::Instance { instance_id },
                key: task_settlement_fact_key(&settlement.catalog_task_id, suffix),
                value,
                observed_at_unix_ms: settlement.completed_at_unix_ms,
                source: OriginModule::Policy,
                ttl_ms: None,
            })?;
        }
        Ok(())
    }

    /// Appends `runtime.fact_invalidated` first, then drops the record from
    /// memory. A key the store does not hold is refused before the append
    /// (`runtime_fact_missing`).
    pub(super) fn invalidate_runtime_fact(
        &self,
        scope: &RuntimeFactScope,
        key: &str,
        reason: RuntimeFactInvalidationReason,
    ) -> RuntimeHostResult<RuntimeFactInvalidation> {
        let result: RuntimeHostResult<RuntimeFactInvalidation> = (|| {
            let _gate = lock(&self.fact_write_gate, "invalidate_runtime_fact")?;
            if lock(&self.runtime_facts, "invalidate_runtime_fact")?
                .get(scope, key)
                .is_none()
            {
                return Err(runtime_fact_rejection(
                    &RuntimeFactError::Missing,
                    "invalidate_runtime_fact",
                ));
            }
            let at_unix_ms = self.clock.sample()?.unix_ms;
            let invalidation = RuntimeFactInvalidation {
                scope: scope.clone(),
                key: key.to_owned(),
                reason,
                at_unix_ms,
            };
            invalidation.validate().map_err(|error| {
                RuntimeHostError::request(
                    error.code(),
                    "invalidate_runtime_fact",
                    RuntimeErrorCode::InvalidRequest,
                )
            })?;
            let links = self.runtime_fact_links(scope)?;
            self.append_event_under_fact_gate(
                EventSeverity::Info,
                EventSource::Runtime,
                OriginModule::RuntimeFacts,
                EventActor::Runtime,
                links,
                RuntimePayloadDraft::fact_invalidated(invalidation, AuditInput::new()),
            )?;
            let invalidation = lock(&self.runtime_facts, "invalidate_runtime_fact")?
                .invalidate(scope, key, reason, at_unix_ms)
                .map_err(|error| runtime_fact_desync(&error, "invalidate_runtime_fact"))?;
            self.runtime_facts_dirty.store(true, Ordering::Release);
            self.synchronize_fact_store_under_gate()?;
            Ok(invalidation)
        })();
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.fatal.mark(error.clone())?;
        }
        result
    }

    /// Seals the live store at the ledger's latest sequence. Writes nothing.
    pub(super) fn runtime_fact_snapshot(&self) -> RuntimeHostResult<RuntimeFactSnapshot> {
        let _gate = lock(&self.fact_write_gate, "read_runtime_fact_snapshot")?;
        let ledger_position = self
            .ledger
            .latest_sequence()
            .map_err(|_| ledger_error("read_runtime_fact_position"))?;
        let taken_at_unix_ms = self.clock.sample()?.unix_ms;
        Ok(lock(&self.runtime_facts, "read_runtime_fact_snapshot")?
            .snapshot(ledger_position, taken_at_unix_ms))
    }

    /// Appends one `runtime.fact_snapshot` when the store changed since the
    /// last seal. A never-changed store appends nothing.
    pub(super) fn append_runtime_fact_snapshot_if_dirty(&self) -> RuntimeHostResult<bool> {
        let result: RuntimeHostResult<bool> = (|| {
            let _gate = lock(&self.fact_write_gate, "append_runtime_fact_snapshot")?;
            if !self.runtime_facts_dirty.load(Ordering::Acquire) {
                return Ok(false);
            }
            let ledger_position = self
                .ledger
                .latest_sequence()
                .map_err(|_| ledger_error("read_runtime_fact_position"))?;
            let taken_at_unix_ms = self.clock.sample()?.unix_ms;
            let snapshot = lock(&self.runtime_facts, "append_runtime_fact_snapshot")?
                .snapshot(ledger_position, taken_at_unix_ms);
            // The typed size and position codes surface here; sanitization would fold them.
            snapshot.validate().map_err(|error| {
                RuntimeHostError::fatal(
                    error.code(),
                    "append_runtime_fact_snapshot",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?;
            let links = self.events.system_links()?;
            self.append_event_under_fact_gate(
                EventSeverity::Info,
                EventSource::Runtime,
                OriginModule::RuntimeFacts,
                EventActor::Runtime,
                links,
                RuntimePayloadDraft::fact_snapshot(snapshot, AuditInput::new()),
            )?;
            self.runtime_facts_dirty.store(false, Ordering::Release);
            self.synchronize_fact_store_under_gate()?;
            Ok(true)
        })();
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.fatal.mark(error.clone())?;
        }
        result
    }

    /// System links, plus the registered instance link for an instance scope.
    fn runtime_fact_links(&self, scope: &RuntimeFactScope) -> RuntimeHostResult<EventLinksDraft> {
        let links = self.events.system_links()?;
        match scope {
            RuntimeFactScope::Runtime => Ok(links),
            RuntimeFactScope::Instance { instance_id } => {
                if !lock(&self.registered_instances, "bind_runtime_fact_scope")?
                    .contains_key(instance_id)
                {
                    return Err(RuntimeHostError::request(
                        "runtime_fact_instance_unknown",
                        "bind_runtime_fact_scope",
                        RuntimeErrorCode::InstanceUnknown,
                    ));
                }
                Ok(links
                    .with_instance_id(self.events.issuer().issue_registered_instance(*instance_id)))
            }
        }
    }
}

/// Rebuilds the store from the newest `runtime.fact_snapshot` plus every
/// `runtime.fact_recorded` / `runtime.fact_invalidated` appended after it, in
/// ledger order. On owner takeover, device-bound instance facts are then
/// invalidated, ledger first. Returns the store and whether it is dirty.
pub(super) fn recover_runtime_fact_store(
    ledger: &GlobalLedger,
    events: &RuntimeEvents,
    takeover: bool,
    now_unix_ms: u64,
) -> RuntimeHostResult<(RuntimeFactStore, bool)> {
    let mut store = RuntimeFactStore::new();
    let snapshots = ledger
        .query(EventQuery {
            event_type: Some(EventType::RuntimeFactSnapshot),
            ..EventQuery::default()
        })
        .map_err(|_| ledger_error("recover_runtime_facts"))?;
    let mut from_sequence = 1;
    if let Some(latest) = snapshots.last() {
        let EventPayload::Runtime(RuntimePayload::FactSnapshot(payload)) = latest.payload() else {
            return Err(runtime_fact_replay_failed(
                latest.sequence(),
                "payload is not runtime.fact_snapshot",
            ));
        };
        store
            .replay(payload.snapshot())
            .map_err(|error| runtime_fact_replay_failed(latest.sequence(), &error.to_string()))?;
        from_sequence = latest.sequence().checked_add(1).ok_or_else(|| {
            runtime_fact_replay_failed(latest.sequence(), "ledger sequence overflow")
        })?;
    }
    let appended = ledger
        .query(EventQuery {
            from_sequence: Some(from_sequence),
            origin_module: Some(OriginModule::RuntimeFacts),
            ..EventQuery::default()
        })
        .map_err(|_| ledger_error("recover_runtime_facts"))?;
    for event in &appended {
        let sequence = event.sequence();
        match event.payload() {
            EventPayload::Runtime(RuntimePayload::FactRecorded(payload)) => {
                store
                    .record(payload.record().clone())
                    .map_err(|error| runtime_fact_replay_failed(sequence, &error.to_string()))?;
            }
            EventPayload::Runtime(RuntimePayload::FactInvalidated(payload)) => {
                let invalidation = payload.invalidation();
                store
                    .invalidate(
                        &invalidation.scope,
                        &invalidation.key,
                        invalidation.reason,
                        invalidation.at_unix_ms,
                    )
                    .map_err(|error| runtime_fact_replay_failed(sequence, &error.to_string()))?;
            }
            _ => {
                return Err(runtime_fact_replay_failed(
                    sequence,
                    "unexpected event under origin module runtime-facts",
                ));
            }
        }
    }
    let dirty = if takeover {
        append_takeover_invalidations(ledger, events, &mut store, now_unix_ms)?
    } else {
        false
    };
    Ok((store, dirty))
}

/// Appends `runtime.fact_invalidated` (reason `runtime_takeover`) for every
/// `device.` / `backend.` / `application.` instance fact and `task.page`, then
/// drops them per instance. Zero matching records append nothing.
fn append_takeover_invalidations(
    ledger: &GlobalLedger,
    events: &RuntimeEvents,
    store: &mut RuntimeFactStore,
    at_unix_ms: u64,
) -> RuntimeHostResult<bool> {
    let mut targets: BTreeMap<InstanceId, Vec<String>> = BTreeMap::new();
    for record in store.records() {
        if let RuntimeFactScope::Instance { instance_id } = &record.scope
            && (TAKEOVER_INVALIDATED_FAMILIES
                .iter()
                .any(|family| record.key.starts_with(family))
                || TAKEOVER_INVALIDATED_KEYS.contains(&record.key.as_str()))
        {
            targets
                .entry(*instance_id)
                .or_default()
                .push(record.key.clone());
        }
    }
    if targets.is_empty() {
        return Ok(false);
    }
    for (instance_id, keys) in &targets {
        let links = events
            .system_links()?
            .with_instance_id(events.issuer().issue_registered_instance(*instance_id));
        for key in keys {
            let invalidation = RuntimeFactInvalidation {
                scope: RuntimeFactScope::Instance {
                    instance_id: *instance_id,
                },
                key: key.clone(),
                reason: RuntimeFactInvalidationReason::RuntimeTakeover,
                at_unix_ms,
            };
            let draft = events.draft(
                EventSeverity::Info,
                EventSource::Runtime,
                OriginModule::RuntimeFacts,
                EventActor::Runtime,
                links.clone(),
                RuntimePayloadDraft::fact_invalidated(invalidation, AuditInput::new()),
            )?;
            let draft = events.sanitize(draft)?;
            ledger
                .append(draft)
                .map_err(|_| ledger_error("append_runtime_fact_invalidated"))?;
        }
        let mut dropped = store.invalidate_instance(
            *instance_id,
            &TAKEOVER_INVALIDATED_FAMILIES,
            RuntimeFactInvalidationReason::RuntimeTakeover,
            at_unix_ms,
        );
        let scope = RuntimeFactScope::Instance {
            instance_id: *instance_id,
        };
        for key in TAKEOVER_INVALIDATED_KEYS {
            // An absent key (`Missing`) is simply not dropped; the comparison below catches desync.
            if let Ok(entry) = store.invalidate(
                &scope,
                key,
                RuntimeFactInvalidationReason::RuntimeTakeover,
                at_unix_ms,
            ) {
                dropped.push(entry);
            }
        }
        // `keys` is in store (key) order; the family and single-key drops are merged into it.
        dropped.sort_by(|left, right| left.key.cmp(&right.key));
        if dropped.iter().map(|entry| &entry.key).ne(keys.iter()) {
            return Err(RuntimeHostError::fatal(
                "runtime_fact_store_desync",
                "invalidate_runtime_facts_on_takeover",
                RuntimeErrorCode::RuntimeFatal,
            ));
        }
    }
    Ok(true)
}

/// Applies the store's acceptance rules without mutating it. `Some` carries
/// the idempotent outcome for an identical record; `None` means the record
/// would be inserted or would replace an older observation.
fn precheck_runtime_fact(
    store: &RuntimeFactStore,
    record: &RuntimeFactRecord,
) -> RuntimeHostResult<Option<RuntimeFactChange>> {
    record.validate().map_err(|error| {
        runtime_fact_rejection(
            &RuntimeFactError::Invalid { code: error.code() },
            "record_runtime_fact",
        )
    })?;
    match store.get(&record.scope, &record.key) {
        Some(existing) if existing == record => Ok(Some(RuntimeFactChange::Unchanged)),
        Some(existing) if existing.observed_at_unix_ms >= record.observed_at_unix_ms => {
            Err(runtime_fact_rejection(
                &RuntimeFactError::Stale {
                    existing_observed_at_unix_ms: existing.observed_at_unix_ms,
                },
                "record_runtime_fact",
            ))
        }
        Some(_) => Ok(None),
        None if store.len() >= MAX_RUNTIME_FACTS => Err(runtime_fact_rejection(
            &RuntimeFactError::CapacityExceeded {
                limit: MAX_RUNTIME_FACTS,
            },
            "record_runtime_fact",
        )),
        None => Ok(None),
    }
}

const fn runtime_fact_code(error: &RuntimeFactError) -> &'static str {
    match error {
        RuntimeFactError::Invalid { code } => code,
        RuntimeFactError::Stale { .. } => "runtime_fact_stale",
        RuntimeFactError::CapacityExceeded { .. } => "runtime_fact_capacity_exceeded",
        RuntimeFactError::Missing => "runtime_fact_missing",
    }
}

/// A store rule refused the change before anything was appended.
fn runtime_fact_rejection(error: &RuntimeFactError, operation: &'static str) -> RuntimeHostError {
    RuntimeHostError::request(
        runtime_fact_code(error),
        operation,
        RuntimeErrorCode::InvalidRequest,
    )
    .with_native_detail(error.to_string())
}

/// The store refused a change that was already appended: memory and ledger disagree.
fn runtime_fact_desync(error: &RuntimeFactError, operation: &'static str) -> RuntimeHostError {
    RuntimeHostError::fatal(
        "runtime_fact_store_desync",
        operation,
        RuntimeErrorCode::RuntimeFatal,
    )
    .with_native_detail(format!("{}: {error}", runtime_fact_code(error)))
}

/// Our own ledger holds a runtime-facts event the store refuses to replay.
fn runtime_fact_replay_failed(sequence: u64, detail: &str) -> RuntimeHostError {
    RuntimeHostError::fatal(
        "runtime_fact_replay_failed",
        "recover_runtime_facts",
        RuntimeErrorCode::RuntimeFatal,
    )
    .with_native_detail(format!("sequence {sequence}: {detail}"))
}
