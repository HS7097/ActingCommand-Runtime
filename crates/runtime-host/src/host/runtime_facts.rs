// SPDX-License-Identifier: AGPL-3.0-only

//! Host wiring for the Runtime's own fact store (Workflow #313).
//!
//! Every change is appended to the `GlobalLedger` before it enters the
//! memory-only store, under the same write gate as the instance fact store.
//! Nothing here reads a file: durability is the per-record event, and the
//! periodic snapshot set only shortens replay.

use super::facts::BACKEND_SELFCHECK_AVAILABILITY_SNAPSHOT_PREFIX;
use super::*;
use crate::policy_host::PolicySettlement;
use actingcommand_contract::{
    BackendObservationStatus, BackendOpenEntry, BackendOpenReport, DeviceSelfCheck,
    DeviceSelfCheckCapture, DeviceSelfCheckEntry, DeviceSelfCheckFailure, DeviceSelfCheckStatus,
    DeviceSelfCheckTouch, FactValue, MAX_RUNTIME_FACT_KEY_BYTES, MAX_RUNTIME_FACT_SNAPSHOT_BYTES,
    newest_complete_runtime_fact_snapshot_set,
};

/// First sequence window of the backward snapshot search at startup (the tail sequence
/// itself); each further window doubles (the Workflow #317 rf1 read-face precedent).
const RUNTIME_FACT_SNAPSHOT_SEARCH_WINDOW: u64 = 1;
/// Snapshot events per page inside one window: each part may carry 512 KiB.
const RUNTIME_FACT_SNAPSHOT_PAGE_EVENTS: usize = 16;

/// Key families that stop describing the device once a new owner epoch starts.
const TAKEOVER_INVALIDATED_FAMILIES: [&str; 3] = ["device.", "backend.", "application."];

/// Self-check fact keys are `backend.selfcheck.<entry>.<suffix>` (Workflow #317 slice sc1),
/// `<entry>` being `input`, `capture` or `nemu`.
const BACKEND_SELFCHECK_PREFIX: &str = "backend.selfcheck.";
const BACKEND_SELFCHECK_STATUS_SUFFIX: &str = "status";
const BACKEND_SELFCHECK_GENERATION_SUFFIX: &str = "generation";
const BACKEND_SELFCHECK_SELECTED_SUFFIX: &str = "selected";
const BACKEND_SELFCHECK_CHECKED_AT_SUFFIX: &str = "checked_at_unix_ms";
/// `backend_selfcheck:<cause>` of an availability withdrawal no recorded open caused: startup
/// before the preparation phase, an invalidation, or a failed preparation step (Workflow #317
/// sc3).
const BACKEND_SELFCHECK_UNCHECKED_CAUSE: &str = "unchecked";

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

/// The `device.self_check` status hint of one recorded open (Workflow #317 sc3): its entry,
/// the self-check `status`, the selected backend, the frame size (capture, nemu) and touch
/// bounds (input, nemu) the open reported, and why a failed one failed: the open itself
/// (`status` or `connection` failed), else the entry's own check.
fn device_self_check(
    instance_alias: String,
    report: &BackendOpenReport,
    status: &str,
) -> DeviceSelfCheck {
    let entry = match report.entry {
        BackendOpenEntry::Input => DeviceSelfCheckEntry::Input,
        BackendOpenEntry::Capture => DeviceSelfCheckEntry::Capture,
        BackendOpenEntry::NemuPair => DeviceSelfCheckEntry::Nemu,
    };
    let status = match status {
        "passed" => DeviceSelfCheckStatus::Passed,
        "failed" => DeviceSelfCheckStatus::Failed,
        _ => DeviceSelfCheckStatus::Unknown,
    };
    let failed = BackendObservationStatus::Failed;
    let failure_code = (status == DeviceSelfCheckStatus::Failed).then(|| {
        if report.status == failed || report.connection == failed {
            match entry {
                DeviceSelfCheckEntry::Input => DeviceSelfCheckFailure::InputBackendOpenFailed,
                DeviceSelfCheckEntry::Capture => DeviceSelfCheckFailure::CaptureBackendOpenFailed,
                DeviceSelfCheckEntry::Nemu => DeviceSelfCheckFailure::PairedBackendOpenFailed,
            }
        } else if entry != DeviceSelfCheckEntry::Input && report.capture_check == failed {
            DeviceSelfCheckFailure::CaptureCheckFailed
        } else {
            DeviceSelfCheckFailure::InputCheckFailed
        }
    });
    let positive =
        |x: i32, y: i32| (x > 0 && y > 0).then_some(DeviceSelfCheckTouch { max_x: x, max_y: y });
    DeviceSelfCheck {
        instance_alias,
        entry,
        status,
        selected: report.selected.clone(),
        capture: match (entry, report.frame_width, report.frame_height) {
            (DeviceSelfCheckEntry::Input, _, _) => None,
            (_, Some(width), Some(height)) if width > 0 && height > 0 => {
                Some(DeviceSelfCheckCapture { width, height })
            }
            _ => None,
        },
        touch: if entry == DeviceSelfCheckEntry::Capture {
            None
        } else {
            report
                .input_geometry
                .as_ref()
                .and_then(|geometry| positive(geometry.natural_max_x, geometry.natural_max_y))
                .or_else(|| {
                    report
                        .handshake
                        .as_ref()
                        .and_then(|handshake| positive(handshake.max_x, handshake.max_y))
                })
        },
        failure_code,
        generation: report.session_generation,
    }
}

/// Refuses a catalog with a task whose settlement fact keys would exceed the 128-byte runtime
/// fact key bound (`task_id_too_long_for_facts`): a task id may be at most 102 bytes. Refuses
/// one whose settlement outcome records would exceed the evaluator's 128-byte key bound
/// (`outcome_key_too_long_for_settlement`, Workflow #313 f5): every outcome key the catalog
/// references for a task may be at most 107 bytes. Both are checked when a catalog becomes
/// active, before any run of it.
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
        let outcome_keys = catalog.referenced_outcome_keys(&task.id);
        if let Some(outcome_key) = outcome_keys.iter().find(|outcome_key| {
            settlement_outcome_key(outcome_key).len() > actingcommand_policy::MAX_ID_BYTES
        }) {
            return Err(RuntimeHostError::request(
                "outcome_key_too_long_for_settlement",
                "activate_policy_catalog",
                RuntimeErrorCode::InvalidRequest,
            )
            .with_native_detail(format!(
                "outcome key '{outcome_key}' of task '{}' is {} bytes; its settlement outcome keys allow at most {} bytes",
                task.id,
                outcome_key.len(),
                actingcommand_policy::MAX_ID_BYTES - settlement_outcome_key("").len()
            )));
        }
    }
    Ok(())
}

/// The longest settlement outcome key a run of `outcome_key` projects (Workflow #313 f5).
fn settlement_outcome_key(outcome_key: &str) -> String {
    format!("{outcome_key}.completed_at_unix_ms")
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
    /// (Workflow #317 slice sc1), ledger-first like [`Self::record_runtime_fact`]: instance
    /// scope, source `device-proxy` (input, nemu) or `capture`, no lifetime. One clock sample is
    /// the `checked_at_unix_ms` value and each record's observation time, raised to one
    /// millisecond after the stored record of the same key when it is not later (an open in the
    /// same millisecond or after a wall-clock step back), read and written under one
    /// `fact_write_gate` hold. A newer open of the same entry therefore always replaces them;
    /// every refusal is returned. `checked_at_unix_ms` saturates at `i64::MAX` for a wall clock
    /// beyond it; the records' `observed_at_unix_ms` stays exact. Under the same hold, for a
    /// device self-checked instance, the open's `device.self_check` status hint follows the four
    /// facts (Workflow #317 sc3, under the open's `links`). The instance's policy availability
    /// then follows the recorded self-check (sc2, strict since sc3), all under one
    /// `backend_selfcheck_availability_gate` hold.
    pub(super) fn record_backend_selfcheck_facts(
        &self,
        instance_id: InstanceId,
        links: &EventLinksDraft,
        report: &BackendOpenReport,
    ) -> RuntimeHostResult<()> {
        let _availability = lock(
            &self.backend_selfcheck_availability_gate,
            "record_backend_selfcheck_facts",
        )?;
        let checked_at_unix_ms = self.clock.sample()?.unix_ms;
        let overflow = || {
            RuntimeHostError::fatal(
                "backend_selfcheck_fact_overflow",
                "record_backend_selfcheck_facts",
                RuntimeErrorCode::RuntimeFatal,
            )
        };
        let generation = i64::try_from(report.session_generation).map_err(|_| overflow())?;
        let (entry, source) = match report.entry {
            BackendOpenEntry::Input => ("input", OriginModule::DeviceProxy),
            BackendOpenEntry::Capture => ("capture", OriginModule::Capture),
            BackendOpenEntry::NemuPair => ("nemu", OriginModule::DeviceProxy),
        };
        let status = backend_selfcheck_status(report);
        let values = [
            (
                BACKEND_SELFCHECK_STATUS_SUFFIX,
                FactValue::String(status.to_owned()),
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
                FactValue::Integer(i64::try_from(checked_at_unix_ms).unwrap_or(i64::MAX)),
            ),
        ];
        let scope = RuntimeFactScope::Instance { instance_id };
        let result: RuntimeHostResult<()> = (|| {
            let (instance_alias, self_checked) = self.selfcheck_instance(instance_id)?;
            let _gate = lock(&self.fact_write_gate, "record_backend_selfcheck_facts")?;
            for (suffix, value) in values {
                let key = format!("{BACKEND_SELFCHECK_PREFIX}{entry}.{suffix}");
                let stored_at_unix_ms = lock(&self.runtime_facts, "read_backend_selfcheck_fact")?
                    .get(&scope, &key)
                    .map(|record| record.observed_at_unix_ms);
                let observed_at_unix_ms = match stored_at_unix_ms {
                    Some(stored) => stored
                        .checked_add(1)
                        .ok_or_else(overflow)?
                        .max(checked_at_unix_ms),
                    None => checked_at_unix_ms,
                };
                self.record_runtime_fact_under_gate(RuntimeFactRecord {
                    scope: scope.clone(),
                    key,
                    value,
                    observed_at_unix_ms,
                    source,
                    ttl_ms: None,
                })?;
            }
            // Workflow #317 sc3: the status hint of this open, right after its facts, for an
            // instance whose device connection is self-checked (a fixture or an endpoint-less
            // provider has no device connection to hint at).
            if self_checked {
                let check = device_self_check(instance_alias, report, status);
                self.append_event_under_fact_gate(
                    if check.status == DeviceSelfCheckStatus::Failed {
                        EventSeverity::Warning
                    } else {
                        EventSeverity::Info
                    },
                    EventSource::Runtime,
                    OriginModule::Runtime,
                    EventActor::Runtime,
                    links.clone(),
                    RuntimePayloadDraft::device_self_check(check),
                )?;
                self.synchronize_fact_store_under_gate()?;
            }
            Ok(())
        })()
        .and_then(|()| {
            self.gate_policy_availability_on_selfcheck(
                instance_id,
                Some((entry, report.session_generation)),
            )
        });
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.fatal.mark(error.clone())?;
        }
        result
    }

    /// Drops every `backend.selfcheck.*` fact the store holds for one instance with
    /// `device_closed`: the session they describe is closed and the instance endpoint was
    /// rebound (or returned to pending). A gated physical instance is then unavailable until
    /// its next self-check passes (Workflow #317 sc3). Every refusal is returned.
    pub(super) fn invalidate_backend_selfcheck_facts(
        &self,
        instance_id: InstanceId,
    ) -> RuntimeHostResult<()> {
        let _availability = lock(
            &self.backend_selfcheck_availability_gate,
            "invalidate_backend_selfcheck_facts",
        )?;
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
        let result = self.gate_policy_availability_on_selfcheck(instance_id, None);
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.fatal.mark(error.clone())?;
        }
        result
    }

    /// Workflow #317 sc3: withdraws the policy availability of a gated physical instance
    /// whatever its stored self-check says (`backend_selfcheck:unchecked`): at startup before
    /// its preparation phase, and when a preparation step other than an open failed. The next
    /// recorded self-check decides again. Every refusal is returned.
    pub(super) fn withhold_policy_instance_availability(
        &self,
        instance_id: InstanceId,
    ) -> RuntimeHostResult<()> {
        let _availability = lock(
            &self.backend_selfcheck_availability_gate,
            "withhold_policy_instance_availability",
        )?;
        let result: RuntimeHostResult<()> = (|| {
            let Some((instance_alias, _)) = self.gated_policy_instance(instance_id)? else {
                return Ok(());
            };
            if self.active_policy_instance_availability(&instance_alias)?.1 != Some(false) {
                self.publish_backend_selfcheck_unavailable(
                    &instance_alias,
                    BACKEND_SELFCHECK_UNCHECKED_CAUSE,
                )?;
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

    /// Workflow #317 sc3 (the strict reading of #316 goal 5, replacing sc2's bounded one):
    /// keeps the policy availability `session.instance.available` of a configured policy
    /// instance on a physical device equal to its configured seed **and** its self-check. The
    /// caller holds `backend_selfcheck_availability_gate`, so the decision and its publication
    /// through [`Self::publish_fact`] (which takes `fact_write_gate` itself) are one step
    /// against every other self-check write. `recorded` is the entry and generation just
    /// recorded, `None` after the facts were invalidated.
    ///
    /// - The self-check passes when no `backend.selfcheck.<entry>.status` of the instance holds
    ///   `failed` and either `nemu` holds `passed` or both `input` and `capture` do; `unknown`,
    ///   missing and `failed` all fail it.
    /// - A failing self-check publishes `false` (`backend_selfcheck:<entry>:<generation>`, or
    ///   `backend_selfcheck:unchecked` after an invalidation) unless the active record already
    ///   holds `false`.
    /// - A passing one republishes the configured seed when the active record is a withdrawal
    ///   this gate published.
    ///
    /// A fixture or other non-physical instance, and an instance without a configured policy
    /// seed, has no availability to gate. Every refusal is returned.
    fn gate_policy_availability_on_selfcheck(
        &self,
        instance_id: InstanceId,
        recorded: Option<(&str, u64)>,
    ) -> RuntimeHostResult<()> {
        let Some((instance_alias, seed_available)) = self.gated_policy_instance(instance_id)?
        else {
            return Ok(());
        };
        let (active, active_value) = self.active_policy_instance_availability(&instance_alias)?;
        if seed_available && self.backend_selfcheck_passed(instance_id)? {
            let withdrawn_by_selfcheck = active.as_ref().is_some_and(|record| {
                record
                    .source_snapshot_id
                    .starts_with(BACKEND_SELFCHECK_AVAILABILITY_SNAPSHOT_PREFIX)
            });
            if withdrawn_by_selfcheck && active_value != Some(true) {
                self.restore_policy_instance_availability(&instance_alias, seed_available)?;
            }
        } else if active_value != Some(false) {
            let cause = recorded.map_or_else(
                || BACKEND_SELFCHECK_UNCHECKED_CAUSE.to_owned(),
                |(entry, generation)| format!("{entry}:{generation}"),
            );
            self.publish_backend_selfcheck_unavailable(&instance_alias, &cause)?;
        }
        Ok(())
    }

    /// The alias and configured `available` seed of a registered physical policy instance, or
    /// `None` when the instance is not gated (not device self-checked, see
    /// `RegisteredInstance::device_self_checked`; no policy inputs; or not a configured policy
    /// instance).
    fn gated_policy_instance(
        &self,
        instance_id: InstanceId,
    ) -> RuntimeHostResult<Option<(String, bool)>> {
        let (instance_alias, self_checked) = self.selfcheck_instance(instance_id)?;
        if !self_checked {
            return Ok(None);
        }
        let seed_available = lock(&self.policy_inputs, "read_gated_policy_instance")?
            .as_ref()
            .and_then(|inputs| {
                inputs
                    .instance_seeds()
                    .find(|seed| seed.instance_id == instance_alias)
                    .map(|seed| seed.available)
            });
        Ok(seed_available.map(|available| (instance_alias, available)))
    }

    /// The instance's own active `session.instance.available` record and its boolean value.
    fn active_policy_instance_availability(
        &self,
        instance_alias: &str,
    ) -> RuntimeHostResult<(Option<FactRecord>, Option<bool>)> {
        let _gate = lock(&self.fact_write_gate, "read_policy_instance_availability")?;
        self.synchronize_fact_store_under_gate()?;
        let active = lock(&self.facts, "read_policy_instance_availability")?
            .active_record(
                &FactScope::Instance {
                    instance_id: instance_alias.to_owned(),
                },
                POLICY_INSTANCE_AVAILABLE_KEY,
            )
            .cloned();
        let value = active.as_ref().and_then(|record| match &record.content {
            FactContent::Inline {
                value: ContractFactValue::Boolean(value),
            } => Some(*value),
            _ => None,
        });
        Ok((active, value))
    }

    /// Whether the instance's stored self-check passes (Workflow #317 sc3): no entry holds
    /// `failed`, and `nemu`, or both `input` and `capture`, hold `passed`.
    fn backend_selfcheck_passed(&self, instance_id: InstanceId) -> RuntimeHostResult<bool> {
        let scope = RuntimeFactScope::Instance { instance_id };
        let store = lock(&self.runtime_facts, "read_backend_selfcheck_status")?;
        let status = |entry: &str| {
            store
                .get(
                    &scope,
                    &format!("{BACKEND_SELFCHECK_PREFIX}{entry}.{BACKEND_SELFCHECK_STATUS_SUFFIX}"),
                )
                .map(|record| record.value.clone())
        };
        let passed = Some(FactValue::String("passed".to_owned()));
        let failed = Some(FactValue::String("failed".to_owned()));
        let (input, capture, nemu) = (status("input"), status("capture"), status("nemu"));
        Ok(![&input, &capture, &nemu].contains(&&failed)
            && (nemu == passed || (input == passed && capture == passed)))
    }

    /// The alias of a registered instance and whether its device connection is self-checked
    /// (`RegisteredInstance::device_self_checked`).
    fn selfcheck_instance(&self, instance_id: InstanceId) -> RuntimeHostResult<(String, bool)> {
        lock(
            &self.registered_instances,
            "read_backend_selfcheck_instance",
        )?
        .get(&instance_id)
        .map(|instance| {
            (
                instance.instance_alias.clone(),
                instance.device_self_checked(),
            )
        })
        .ok_or_else(|| {
            RuntimeHostError::fatal(
                "backend_selfcheck_instance_unregistered",
                "record_backend_selfcheck_facts",
                RuntimeErrorCode::RuntimeFatal,
            )
        })
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
            self.record_runtime_fact_under_gate(record)
        })();
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.fatal.mark(error.clone())?;
        }
        result
    }

    /// [`Self::record_runtime_fact`] for a caller that already holds `fact_write_gate`; the
    /// caller marks a fatal error.
    fn record_runtime_fact_under_gate(
        &self,
        record: RuntimeFactRecord,
    ) -> RuntimeHostResult<RuntimeFactChange> {
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

    /// Seals the store when it changed since the last seal (Workflow #308 5d-1): the records
    /// are split into parts of at most `MAX_RUNTIME_FACT_SNAPSHOT_BYTES` and one
    /// `runtime.fact_snapshot` is appended per part, in part order, under one
    /// `fact_write_gate` hold. The store stays dirty until every part is appended; every
    /// failure is fatal. A never-changed store appends nothing.
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
            let sealed = lock(&self.runtime_facts, "append_runtime_fact_snapshot")?
                .snapshot(ledger_position, taken_at_unix_ms);
            let parts = split_runtime_fact_snapshot(sealed)?;
            // The typed size and position codes surface here as fatal host errors; sanitization
            // would report them as a `LedgerFailure` under `sanitize_runtime_event`.
            for part in &parts {
                part.validate_for_append().map_err(|error| {
                    RuntimeHostError::fatal(
                        error.code(),
                        "append_runtime_fact_snapshot",
                        RuntimeErrorCode::RuntimeFatal,
                    )
                })?;
            }
            for part in parts {
                let links = self.events.system_links()?;
                self.append_event_under_fact_gate(
                    EventSeverity::Info,
                    EventSource::Runtime,
                    OriginModule::RuntimeFacts,
                    EventActor::Runtime,
                    links,
                    RuntimePayloadDraft::fact_snapshot(part, AuditInput::new()),
                )?;
            }
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

/// Splits the sealed store into snapshot parts of at most `MAX_RUNTIME_FACT_SNAPSHOT_BYTES`
/// serialized bytes (Workflow #308 5d-1): records in store order, greedily, a new part
/// starting when the next record would not fit; every part carries the sealed identity with
/// its own `part` and the shared `parts`. An empty store is one empty part; a record too
/// large for any part forms a part of its own, which `validate_for_append` then refuses.
fn split_runtime_fact_snapshot(
    sealed: RuntimeFactSnapshot,
) -> RuntimeHostResult<Vec<RuntimeFactSnapshot>> {
    let fatal = |code| {
        RuntimeHostError::fatal(
            code,
            "append_runtime_fact_snapshot",
            RuntimeErrorCode::RuntimeFatal,
        )
    };
    let mut envelope = RuntimeFactSnapshot {
        part: u16::MAX,
        parts: u16::MAX,
        records: Vec::new(),
        ..sealed
    };
    // The widest part numbers, so the real envelope is never larger than the one measured.
    let envelope_bytes = serde_json::to_vec(&envelope)
        .map_err(|_| fatal("invalid_runtime_fact_snapshot"))?
        .len();
    let mut groups = Vec::new();
    let mut current = Vec::new();
    let mut current_bytes = envelope_bytes;
    for record in sealed.records {
        let record_bytes = serde_json::to_vec(&record)
            .map_err(|_| fatal("invalid_runtime_fact_snapshot"))?
            .len();
        // Every record after the first in a part adds one separating comma.
        if !current.is_empty() && current_bytes + 1 + record_bytes > MAX_RUNTIME_FACT_SNAPSHOT_BYTES
        {
            groups.push(std::mem::take(&mut current));
            current_bytes = envelope_bytes;
        }
        current_bytes += usize::from(!current.is_empty()) + record_bytes;
        current.push(record);
    }
    if !current.is_empty() || groups.is_empty() {
        groups.push(current);
    }
    let parts = u16::try_from(groups.len())
        .map_err(|_| fatal("runtime_fact_snapshot_payload_too_large"))?;
    envelope.parts = parts;
    Ok(groups
        .into_iter()
        .zip(1..=parts)
        .map(|(records, part)| RuntimeFactSnapshot {
            schema_version: envelope.schema_version.clone(),
            part,
            records,
            ..envelope
        })
        .collect())
}

/// `runtime.fact_snapshot` events at or below `upper`, newest first (Workflow #308 5d-1):
/// read backwards from the ledger tail in sequence windows that double in size, each through
/// type-indexed pages, so a search stops without reading older snapshot events. After a part
/// other than part 1, the next window stops at that part's `snapshot_id`: the set's earlier
/// parts were appended after it, so older sets are read only when the search needs them.
struct SnapshotEventsNewestFirst<'a> {
    ledger: &'a GlobalLedger,
    upper: u64,
    window: u64,
    floor: u64,
    buffered: Vec<PersistedEvent>,
}

impl SnapshotEventsNewestFirst<'_> {
    /// The next older snapshot part with its sequence, validated; an invalid one is fatal.
    fn next(&mut self) -> RuntimeHostResult<Option<(u64, RuntimeFactSnapshot)>> {
        let query = EventQuery {
            event_type: Some(EventType::RuntimeFactSnapshot),
            ..EventQuery::default()
        };
        while self.buffered.is_empty() && self.upper > 0 {
            let floor = if self.floor < self.upper {
                self.floor
            } else {
                0
            };
            let lower = self.upper.saturating_sub(self.window).max(floor);
            let mut after = lower;
            loop {
                let page = self
                    .ledger
                    .query_page(
                        query.clone(),
                        after,
                        self.upper,
                        RUNTIME_FACT_SNAPSHOT_PAGE_EVENTS,
                    )
                    .map_err(|_| ledger_error("recover_runtime_facts"))?;
                let exhausted = page.len() < RUNTIME_FACT_SNAPSHOT_PAGE_EVENTS;
                if let Some(last) = page.last() {
                    after = last.sequence();
                }
                self.buffered.extend(page);
                if exhausted {
                    break;
                }
            }
            self.upper = lower;
            self.window = self.window.saturating_mul(2);
        }
        let Some(event) = self.buffered.pop() else {
            return Ok(None);
        };
        let sequence = event.sequence();
        let EventPayload::Runtime(RuntimePayload::FactSnapshot(payload)) = event.payload() else {
            return Err(runtime_fact_replay_failed(
                sequence,
                "payload is not runtime.fact_snapshot",
            ));
        };
        let snapshot = payload.snapshot().clone();
        snapshot.validate().map_err(|error| {
            runtime_fact_replay_failed(
                sequence,
                &RuntimeFactError::Invalid { code: error.code() }.to_string(),
            )
        })?;
        self.floor = if snapshot.part > 1 {
            snapshot.snapshot_id
        } else {
            0
        };
        Ok(Some((sequence, snapshot)))
    }
}

/// Rebuilds the store from the newest complete `runtime.fact_snapshot` set plus every
/// `runtime.fact_recorded` / `runtime.fact_invalidated` appended after its last part, in
/// ledger order (Workflow #308 5d-1). The set is searched backwards from the ledger tail;
/// every incomplete set passed over is recorded as one `runtime.lifecycle_observed`
/// `fact_snapshot_set_skipped`. Without a complete set every record event is replayed. On
/// owner takeover, device-bound instance facts are then invalidated, ledger first. Returns
/// the store and whether it is dirty.
pub(super) fn recover_runtime_fact_store(
    ledger: &GlobalLedger,
    events: &RuntimeEvents,
    owner_epoch: actingcommand_contract::OwnerEpoch,
    takeover: bool,
    now_unix_ms: u64,
) -> RuntimeHostResult<(RuntimeFactStore, bool)> {
    let mut store = RuntimeFactStore::new();
    let mut newest_first = SnapshotEventsNewestFirst {
        ledger,
        upper: ledger
            .latest_sequence()
            .map_err(|_| ledger_error("recover_runtime_facts"))?,
        window: RUNTIME_FACT_SNAPSHOT_SEARCH_WINDOW,
        floor: 0,
        buffered: Vec::new(),
    };
    let search = newest_complete_runtime_fact_snapshot_set(|| newest_first.next())?;
    for skipped in &search.skipped {
        let draft = events.draft(
            EventSeverity::Warning,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            events.system_links()?,
            RuntimePayloadDraft::lifecycle_observed(
                owner_epoch,
                RuntimeLifecyclePhase::FactSnapshotSetSkipped {
                    snapshot_id: skipped.snapshot_id,
                    parts_found: skipped.parts_found,
                    parts: skipped.parts,
                },
                AuditInput::new(),
            ),
        )?;
        let draft = events.sanitize(draft)?;
        ledger
            .append(draft)
            .map_err(|_| ledger_error("append_runtime_lifecycle_observed"))?;
    }
    let mut from_sequence = 1;
    if let Some(set) = &search.set {
        // Part 1 replaces the empty store; the later parts' records join it in part order.
        for (index, (sequence, part)) in set.iter().enumerate() {
            let replayed = if index == 0 {
                store.replay(part).map(|_| ())
            } else {
                part.records
                    .iter()
                    .try_for_each(|record| store.record(record.clone()).map(|_| ()))
            };
            replayed.map_err(|error| runtime_fact_replay_failed(*sequence, &error.to_string()))?;
        }
        if let Some((last, _)) = set.last() {
            from_sequence = last
                .checked_add(1)
                .ok_or_else(|| runtime_fact_replay_failed(*last, "ledger sequence overflow"))?;
        }
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
            // A part of an incomplete set passed over above: the record events around it
            // already carry its content.
            EventPayload::Runtime(RuntimePayload::FactSnapshot(_)) => {}
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
