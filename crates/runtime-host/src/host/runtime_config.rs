// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #387 D1b (frozen model v1.2 §5.6-§5.9): the configuration in effect. The host holds
//! the file layer actingd assembled (the manifest) with the instruction layer over it, both
//! through `actingcommand-runtime-config`.
//!
//! - A start restores the instruction values of `host.config_overrides` before any instance is
//!   prepared, by the later-change-wins rule (§12 question 3), records each value not restored
//!   as it was, and records `config.subsystems`, `config.selection`, the restored
//!   `host.config_overrides` and one `config.parameters.<section>` per section.
//! - `ConfigureSettings` (a general instruction): the command record, then, all changes or
//!   none, the new `host.config_overrides` and each changed `config.parameters.<section>`
//!   (§5.7), then the values take effect at once. A refusal leaves everything as it was and
//!   records only `command.rejected`.

use super::*;
use crate::codes::{ConfigHostCode, ConfigHostLocation};
use actingcommand_contract::{
    CONFIG_OVERRIDES_FACT_KEY, CONFIG_PARAMETERS_FACT_PREFIX, ConfigSelection, ConfigSettingChange,
    InstanceStuckRecovery, config_parameters_fact_key,
};
use actingcommand_runtime_config::{
    AppliedSettings, CheckContext, EffectiveConfig, InstructionStamp, RestoreNote, STUCK_RECOVERY,
    STUCK_RECOVERY_COOLDOWN_SECS, instance_key,
};

impl HostShared {
    /// Workflow #387 D1b: the configuration facts of a start, with the instruction values the
    /// earlier runs set restored first (§5.6, §5.7). Any refusal fails the start.
    pub(super) fn start_runtime_config(
        &self,
        manifest: &RuntimeConfigManifest,
        selection: Option<&ConfigSelection>,
    ) -> RuntimeHostResult<()> {
        let context = self.config_check()?;
        let mut effective = EffectiveConfig::from_manifest(manifest)
            .and_then(|effective| effective.check(&context).map(|()| effective))
            .map_err(|failure| {
                RuntimeHostError::fatal(
                    failure.failure_code(),
                    ConfigHostLocation::RecordRuntimeConfig,
                    RuntimeErrorCode::RuntimeFatal,
                )
                .with_outcome_values(|outcome| failure.with_values(outcome))
            })?;
        let _gate = lock(
            &self.config_override_gate,
            ConfigHostLocation::RestoreConfigOverrides,
        )?;
        let (stored, stale_sections) = {
            let facts = lock(&self.runtime_facts, ConfigHostLocation::ReadRuntimeConfig)?;
            let stored = facts
                .get(&RuntimeFactScope::Runtime, CONFIG_OVERRIDES_FACT_KEY)
                .cloned();
            // A section an earlier start recorded that this manifest no longer has is recorded
            // empty, so no stale row is read as in effect.
            let sections = manifest.parameter_sections();
            let stale = facts
                .records()
                .filter(|record| {
                    record.scope == RuntimeFactScope::Runtime
                        && matches!(&record.value, ContractFactValue::RecordList(rows) if !rows.is_empty())
                })
                .filter_map(|record| record.key.strip_prefix(CONFIG_PARAMETERS_FACT_PREFIX))
                .filter(|section| !sections.contains(section))
                .map(str::to_owned)
                .collect::<Vec<_>>();
            (stored, stale)
        };
        let (notes, restored) = match &stored {
            None => (Vec::new(), false),
            Some(record) => match (&record.value, record.source) {
                (ContractFactValue::RecordList(rows), OriginModule::Runtime) => (
                    effective
                        .restore(rows, &context)
                        .map_err(|_| overrides_invalid())?,
                    !rows.is_empty(),
                ),
                _ => return Err(overrides_invalid()),
            },
        };
        let observed_at_unix_ms = self.clock.sample()?.unix_ms;
        self.record_config_fact(
            manifest.subsystems_fact_record(observed_at_unix_ms, OriginModule::Runtime),
        )?;
        if let Some(selection) = selection {
            self.record_config_fact(
                selection.to_fact_record(observed_at_unix_ms, OriginModule::Runtime),
            )?;
        }
        for note in &notes {
            self.record_restore_note(note)?;
        }
        if restored {
            self.record_config_fact(overrides_record(effective.override_rows()))?;
        }
        for record in effective
            .manifest()
            .parameter_fact_records(observed_at_unix_ms, OriginModule::Runtime)
        {
            self.record_config_fact(record)?;
        }
        for section in stale_sections {
            self.record_config_fact(RuntimeFactRecord {
                scope: RuntimeFactScope::Runtime,
                key: config_parameters_fact_key(&section),
                value: ContractFactValue::RecordList(Vec::new()),
                observed_at_unix_ms,
                source: OriginModule::Runtime,
                ttl_ms: None,
            })?;
        }
        *lock(
            &self.runtime_config,
            ConfigHostLocation::RecordRuntimeConfig,
        )? = effective;
        Ok(())
    }

    /// `ConfigureSettings` (Workflow #387 D1b, model v1.2 §5.6, §5.7).
    pub(super) fn configure_settings(
        &self,
        original: &RuntimeRequest,
        validated: &ValidatedRuntimeRequest<'_>,
        changes: &[ConfigSettingChange],
    ) -> Result<OperationSuccess, RequestFailure> {
        let links = self.append_scoped_client_command_intent(
            original,
            validated,
            None,
            EventAction::RuntimeAction,
            None,
        )?;
        let context = self.config_check()?;
        let _gate = lock(
            &self.config_override_gate,
            ConfigHostLocation::ConfigureSettings,
        )?;
        let current = lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?.clone();
        let stamp = InstructionStamp {
            set_unix_ms: self
                .runtime_clock_sample()
                .map_err(RequestFailure::poison_without_terminal)?
                .unix_ms,
            set_in_owner_epoch: identity_text(
                &self.owner_epoch,
                ConfigHostLocation::ConfigureSettings,
            )?,
            request_id: identity_text(
                &original.request_id(),
                ConfigHostLocation::ConfigureSettings,
            )?,
        };
        let applied = match current.apply(changes, &stamp, &context) {
            Ok(applied) => applied,
            Err(failure) => {
                let error = RuntimeHostError::request(
                    failure.failure_code(),
                    ConfigHostLocation::ConfigureSettings,
                    RuntimeErrorCode::InvalidRequest,
                )
                .with_outcome_values(|outcome| failure.with_values(outcome));
                let rejected = self.append_event(
                    EventSeverity::Error,
                    EventSource::Runtime,
                    OriginModule::Runtime,
                    EventActor::Runtime,
                    links,
                    CommandPayloadDraft::rejected_with_outcome(
                        EventAction::RuntimeAction,
                        DiagnosticCode::CommandRejected,
                        EffectDisposition::NotPerformed,
                        error.outcome(),
                        AuditInput::new(),
                    ),
                )?;
                return Err(RequestFailure::request(
                    error,
                    RuntimeReceiptState::Denied,
                    Some(terminal(&rejected)),
                ));
            }
        };
        // Recorded before the change takes effect (§5.7): first the whole new
        // `host.config_overrides`, then each changed section's `config.parameters.<section>`. A
        // refused `host.config_overrides` changes nothing and refuses the request. Once it is
        // recorded, a refused section record would leave the ledger apart from the values in
        // effect, so it stops the Runtime; the next start restores the recorded values (§5.6,
        // all changes or none).
        if let Err(error) =
            self.record_config_fact(overrides_record(applied.config.override_rows()))
        {
            if error.is_fatal() {
                return Err(RequestFailure::poison_without_terminal(error));
            }
            let failure = RuntimeHostError::request(
                ConfigHostCode::OverridesPersistFailed,
                ConfigHostLocation::PersistConfigOverrides,
                RuntimeErrorCode::LedgerFailure,
            )
            .with_caused_by(error.outcome());
            let rejected = self.append_event(
                EventSeverity::Error,
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
                links,
                CommandPayloadDraft::rejected_with_outcome(
                    EventAction::RuntimeAction,
                    DiagnosticCode::CommandRejected,
                    EffectDisposition::NotPerformed,
                    failure.outcome(),
                    AuditInput::new(),
                ),
            )?;
            return Err(RequestFailure::request(
                failure,
                RuntimeReceiptState::Failed,
                Some(terminal(&rejected)),
            ));
        }
        let in_effect = applied.config.manifest();
        for section in &applied.sections {
            self.record_config_fact(in_effect.parameter_section_fact_record(
                section,
                0,
                OriginModule::Runtime,
            ))
            .map_err(RequestFailure::poison_without_terminal)?;
        }
        let event = self.append_event(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links,
            CommandPayloadDraft::validated(
                EventAction::RuntimeAction,
                EffectDisposition::Performed,
                AuditInput::new(),
            ),
        )?;
        let AppliedSettings {
            config, settings, ..
        } = applied;
        *lock(&self.runtime_config, ConfigHostLocation::ConfigureSettings)? = config;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(terminal(&event)),
            result: RuntimeResult::SettingsConfigured { settings },
            failure_outcome: None,
        })
    }

    /// The stuck-recovery settings the ladder reads for one instance now: those actingd
    /// assembled, with the values in effect of the instance's two per-instance keys over them,
    /// an instruction value included (Workflow #387 D1b, §5.5 `settable`).
    pub(super) fn instance_stuck_recovery(
        &self,
        instance_id: InstanceId,
    ) -> RuntimeHostResult<InstanceStuckRecovery> {
        let mut settings = self
            .stuck_recovery()?
            .get(&instance_id)
            .copied()
            .unwrap_or_default();
        let instance = identity_text(&instance_id, ConfigHostLocation::ReadRuntimeConfig)?;
        let invalid = || value_type_invalid(ConfigHostLocation::ReadRuntimeConfig);
        let config = lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?;
        if let Some(value) = config.value(&instance_key(&instance, STUCK_RECOVERY)) {
            settings.enabled = match value {
                FactScalar::Boolean(enabled) => *enabled,
                _ => return Err(invalid()),
            };
        }
        if let Some(value) = config.value(&instance_key(&instance, STUCK_RECOVERY_COOLDOWN_SECS)) {
            settings.cooldown_secs = match value {
                FactScalar::Integer(secs) => u32::try_from(*secs).map_err(|_| invalid())?,
                _ => return Err(invalid()),
            };
        }
        Ok(settings)
    }

    /// The facts of other crates the configuration's whole-section checks read (§5.3): the
    /// scheduler's heartbeat bound in effect.
    fn config_check(&self) -> RuntimeHostResult<CheckContext> {
        let maximum_client_heartbeat_interval_ms =
            lock(&self.scheduler, ConfigHostLocation::ReadRuntimeConfig)?
                .config()
                .maximum_client_heartbeat_interval_ms;
        Ok(CheckContext::new(maximum_client_heartbeat_interval_ms))
    }

    /// Records one configuration fact, observed now and strictly after the stored record of its
    /// key, ledger-first through [`Self::record_runtime_fact`].
    fn record_config_fact(&self, mut record: RuntimeFactRecord) -> RuntimeHostResult<()> {
        let now = self.clock.sample()?.unix_ms;
        let previous = lock(&self.runtime_facts, ConfigHostLocation::ReadRuntimeConfig)?
            .get(&record.scope, &record.key)
            .map(|stored| stored.observed_at_unix_ms);
        record.observed_at_unix_ms = match previous {
            Some(previous) => now.max(previous.checked_add(1).ok_or_else(|| {
                RuntimeHostError::fatal(
                    ConfigHostCode::OverridesFactClockOverflow,
                    ConfigHostLocation::RecordRuntimeConfig,
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?),
            None => now,
        };
        self.record_runtime_fact(record).map(|_| ())
    }

    /// One instruction value a start did not restore as it was (§5.7): a `runtime.failed`
    /// record whose outcome is the note.
    fn record_restore_note(&self, note: &RestoreNote) -> RuntimeHostResult<()> {
        self.append_event_raw(
            // The envelope derives the record's severity from the note's category (ledger
            // revision 3); this value is not read.
            EventSeverity::Warning,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            self.events.system_links()?,
            RuntimePayloadDraft::failed(
                DiagnosticCode::RuntimeDiagnostic,
                EffectDisposition::NotPerformed,
                note.outcome()
                    .operation(ConfigHostLocation::RestoreConfigOverrides),
                DiagnosticDetailDraft::new(
                    "config_overrides",
                    ConfigHostLocation::RestoreConfigOverrides.as_str(),
                    "runtime_host",
                    ConfigHostLocation::RestoreConfigOverrides.as_str(),
                ),
                AuditInput::new(),
            ),
        )
        .map(|_| ())
    }
}

/// The `host.config_overrides` fact of these rows; its observation time is set when recorded.
fn overrides_record(rows: Vec<BTreeMap<String, FactScalar>>) -> RuntimeFactRecord {
    RuntimeFactRecord {
        scope: RuntimeFactScope::Runtime,
        key: CONFIG_OVERRIDES_FACT_KEY.to_owned(),
        value: ContractFactValue::RecordList(rows),
        observed_at_unix_ms: 0,
        source: OriginModule::Runtime,
        ttl_ms: None,
    }
}

fn overrides_invalid() -> RuntimeHostError {
    RuntimeHostError::fatal(
        ConfigHostCode::OverridesFactInvalid,
        ConfigHostLocation::RestoreConfigOverrides,
        RuntimeErrorCode::RuntimeFatal,
    )
}

fn value_type_invalid(operation: ConfigHostLocation) -> RuntimeHostError {
    RuntimeHostError::fatal(
        ConfigHostCode::ValueTypeInvalid,
        operation,
        RuntimeErrorCode::RuntimeFatal,
    )
}

/// The canonical text of an opaque identifier (an owner epoch, a request or instance id).
fn identity_text(
    value: &impl serde::Serialize,
    operation: ConfigHostLocation,
) -> RuntimeHostResult<String> {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(text)) => Ok(text),
        _ => Err(value_type_invalid(operation)),
    }
}
