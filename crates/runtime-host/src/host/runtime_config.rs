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
//! - Workflow #389 MCP-9a (register A5): `config.catalog`, the active policy catalog generation,
//!   at start, after each committed catalog transition and after each approval decision, each
//!   time only when it changed.
//! - #402 HS-2 (register §1.270): a `ConfigureSettings` that switches an instance's backends
//!   switches the device first (`backend_switch.rs`), then commits as any other change, with
//!   `device.backend_switched` before its `command.validated`.

use super::backend_switch::SwitchBackends;
use super::*;
use crate::codes::{ConfigHostCode, ConfigHostLocation};
use actingcommand_contract::{
    ApprovalTarget, CONFIG_CATALOG_FACT_KEY, CONFIG_OVERRIDES_FACT_KEY,
    CONFIG_PARAMETERS_FACT_PREFIX, CatalogPayload, ConfigCatalog, ConfigSelection,
    ConfigSettingChange, DeviceBackendSwitched, InstanceDeviceKind, InstanceStuckRecovery,
    config_parameters_fact_key,
};
use actingcommand_runtime_config::{
    AppliedSettings, CheckContext, EffectiveConfig, InstructionStamp, LadderConfig, LadderProfile,
    RestoreNote, STUCK_RECOVERY, STUCK_RECOVERY_COOLDOWN_SECS, instance_key,
};

/// Workflow #432 AGT-1b: an instruction that changes `agent_wake.rules` names no alias that is no
/// registered instance's (`config_value_invalid`, `instance_unknown`).
fn refuse_wake_rule_aliases(
    applied: &AppliedSettings,
    aliases: &[String],
) -> Result<(), actingcommand_runtime_config::ConfigFailure> {
    use actingcommand_runtime_config::agent_wake::RULES;
    if !applied.settings.iter().any(|setting| setting.key == RULES) {
        return Ok(());
    }
    let Some(section) = applied.config.agent_wake()? else {
        return Ok(());
    };
    section
        .refuse_unknown_instances(aliases.iter().map(String::as_str))
        .map_err(|_| {
            actingcommand_runtime_config::ConfigFailure::with_problem(
                actingcommand_runtime_config::ConfigCode::ConfigValueInvalid,
                RULES,
                actingcommand_runtime_config::ConfigValueProblem::InstanceUnknown,
            )
        })
}

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
                    RuntimeErrorCode::RuntimeHalted,
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
        let (notes, restored, rows_stored) = match &stored {
            None => (Vec::new(), false, 0),
            Some(record) => match (&record.value, record.source) {
                (ContractFactValue::RecordList(rows), OriginModule::Runtime) => (
                    effective
                        .restore(rows, &context)
                        .map_err(|_| overrides_invalid())?,
                    !rows.is_empty(),
                    rows.len(),
                ),
                _ => return Err(overrides_invalid()),
            },
        };
        // Workflow #421 INST-2, for `StartReport`: the instructions restored, the keys not
        // restored and (#421 S2-IF) the keys restored over a default that changed, which count
        // as restored and are never dropped.
        let (base_changed, dropped): (Vec<&RestoreNote>, Vec<&RestoreNote>) =
            notes.iter().partition(|note| note.restored());
        self.note_start_instructions(
            rows_stored.saturating_sub(dropped.len()),
            dropped
                .into_iter()
                .map(|note| note.config_key.clone())
                .collect(),
            base_changed
                .into_iter()
                .map(|note| note.config_key.clone())
                .collect(),
        )?;
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

    /// `ConfigureSettings` (Workflow #387 D1b, model v1.2 §5.6, §5.7). Workflow #429 R-4 (A.6
    /// rule 2; register §1.239; since 0.12.36): with `expected_version` the changes apply only
    /// while the settings' version ([`EffectiveConfig::version`]) is that one, compared under the
    /// gate every commit holds; another version refuses the request (Denied,
    /// `fact_observation_version_conflict` with `fact_key` `host.config_overrides` and the
    /// `current_version`) and changes nothing. Without it the last writer wins, as before. A
    /// completed receipt carries the version after the change.
    ///
    /// #402 HS-2 (design HS-M §3.2; register §1.270): a request whose changes switch an
    /// instance's backends (the instance's two backend keys, alone) switches the device before
    /// it commits (`switch_instance_backends`): the gate is left once the request is checked, and
    /// taken again for the commit. A change that leaves both backends as they are in effect
    /// commits at once, as any other change.
    pub(super) fn configure_settings(
        &self,
        original: &RuntimeRequest,
        validated: &ValidatedRuntimeRequest<'_>,
        changes: &[ConfigSettingChange],
        expected_version: Option<u64>,
        connection_id: ConnectionId,
    ) -> Result<OperationSuccess, RequestFailure> {
        let links = self.append_scoped_client_command_intent(
            original,
            validated,
            None,
            EventAction::RuntimeAction,
            None,
        )?;
        let context = self.config_check()?;
        let gate = lock(
            &self.config_override_gate,
            ConfigHostLocation::ConfigureSettings,
        )?;
        let current = lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?.clone();
        if let Some(expected) = expected_version {
            let current_version = current.version();
            if expected != current_version {
                let error = RuntimeHostError::request(
                    HostCode::FactObservationVersionConflict,
                    ConfigHostLocation::ConfigureSettings,
                    RuntimeErrorCode::InvalidRequest,
                )
                .with_outcome_values(|outcome| {
                    outcome
                        .fact_key(CONFIG_OVERRIDES_FACT_KEY)
                        .current_version(current_version)
                });
                return Err(self.refuse_settings(links, error, RuntimeReceiptState::Denied));
            }
        }
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
            // Workflow #389 U-2: `host.config_overrides` names the channel from U-2 on.
            channel: validated.channel().map(|binding| binding.channel),
        };
        let applied = match self.apply_settings(&current, changes, &stamp, &context) {
            Ok(applied) => applied,
            Err(error) if error.is_fatal() => {
                return Err(RequestFailure::poison_without_terminal(error));
            }
            Err(error) => {
                return Err(self.refuse_settings(links, error, RuntimeReceiptState::Denied));
            }
        };
        // #402 HS-2: the device switches first, outside the gate (status reads take it).
        if let Some(plan) = self.backend_switch_plan(&current, &applied.config, changes)? {
            drop(gate);
            return self.switch_instance_backends(
                original,
                validated,
                connection_id,
                super::backend_switch::SwitchRequest {
                    changes,
                    stamp: &stamp,
                    links,
                    plan,
                },
            );
        }
        if let Some(failure) = self.persist_settings(&applied)? {
            return Err(self.refuse_settings(links, failure, RuntimeReceiptState::Failed));
        }
        self.commit_settings(applied, links, None)
    }

    /// `changes` applied over `current` with the configuration's own checks and (Workflow #432
    /// AGT-1b) the aliases of `agent_wake.rules` checked against the registered instances, as
    /// the start checks them (the section check has no instance list). A refusal is the request's
    /// error, with its values; #402 HS-2: a broken Nemu pair carries the registry's own code as
    /// its `caused_by` (`nemu_input_requires_paired_capture`).
    pub(super) fn apply_settings(
        &self,
        current: &EffectiveConfig,
        changes: &[ConfigSettingChange],
        stamp: &InstructionStamp,
        context: &CheckContext,
    ) -> Result<AppliedSettings, RuntimeHostError> {
        let aliases = lock(
            &self.registered_instances,
            ConfigHostLocation::ConfigureSettings,
        )?
        .values()
        .map(|instance| instance.instance_alias.clone())
        .collect::<Vec<_>>();
        current
            .apply(changes, stamp, context)
            .and_then(|applied| {
                refuse_wake_rule_aliases(&applied, &aliases)?;
                Ok(applied)
            })
            .map_err(|failure| {
                let pair = failure.problem()
                    == Some(actingcommand_runtime_config::ConfigValueProblem::BackendPairRequired);
                let error = RuntimeHostError::request(
                    failure.failure_code(),
                    ConfigHostLocation::ConfigureSettings,
                    RuntimeErrorCode::InvalidRequest,
                )
                .with_outcome_values(|outcome| failure.with_values(outcome));
                if pair {
                    error.with_caused_by(Outcome::new(HostCode::NemuInputRequiresPairedCapture))
                } else {
                    error
                }
            })
    }

    /// The refusal of a `ConfigureSettings`: its `command.rejected` carrying `error` as its
    /// outcome, and the request's failure in `state` with that record as its terminal. A refused
    /// append stops the Runtime, as every refused record does.
    pub(super) fn refuse_settings(
        &self,
        links: EventLinksDraft,
        error: RuntimeHostError,
        state: RuntimeReceiptState,
    ) -> RequestFailure {
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
        );
        match rejected {
            Ok(rejected) => RequestFailure::request(error, state, Some(terminal(&rejected))),
            Err(failure) => failure,
        }
    }

    /// Records the whole new `host.config_overrides` of `applied` (§5.7). `Some` is the failure
    /// of a record the ledger refused (`host_config_overrides_persist_failed` with the ledger's
    /// refusal as its `caused_by`): nothing changed and the caller refuses the request; a fatal
    /// refusal stops the Runtime.
    pub(super) fn persist_settings(
        &self,
        applied: &AppliedSettings,
    ) -> Result<Option<RuntimeHostError>, RequestFailure> {
        // #402 HS-2, test builds: the ledger refuses this record, not as the Runtime's fatal.
        #[cfg(test)]
        if self
            .config_persist_failure_for_test
            .swap(false, std::sync::atomic::Ordering::AcqRel)
        {
            return Ok(Some(
                RuntimeHostError::request(
                    ConfigHostCode::OverridesPersistFailed,
                    ConfigHostLocation::PersistConfigOverrides,
                    RuntimeErrorCode::LedgerFailure,
                )
                .with_caused_by(Outcome::new(RuntimeErrorCode::LedgerFailure)),
            ));
        }
        match self.record_config_fact(overrides_record(applied.config.override_rows())) {
            Ok(()) => Ok(None),
            Err(error) if error.is_fatal() => Err(RequestFailure::poison_without_terminal(error)),
            Err(error) => Ok(Some(
                RuntimeHostError::request(
                    ConfigHostCode::OverridesPersistFailed,
                    ConfigHostLocation::PersistConfigOverrides,
                    RuntimeErrorCode::LedgerFailure,
                )
                .with_caused_by(error.outcome()),
            )),
        }
    }

    /// Commits `applied` once its `host.config_overrides` is recorded (`persist_settings`): each
    /// changed section's `config.parameters.<section>`, then (#402 HS-2) the switch's
    /// `device.backend_switched` when the request switched an instance's backends, then
    /// `command.validated`, and the values take effect. Once `host.config_overrides` is recorded,
    /// a refused section record would leave the ledger apart from the values in effect, so it
    /// stops the Runtime; the next start restores the recorded values (§5.6, all changes or
    /// none).
    pub(super) fn commit_settings(
        &self,
        applied: AppliedSettings,
        links: EventLinksDraft,
        switched: Option<(EventLinksDraft, DeviceBackendSwitched)>,
    ) -> Result<OperationSuccess, RequestFailure> {
        let in_effect = applied.config.manifest();
        for section in &applied.sections {
            self.record_config_fact(in_effect.parameter_section_fact_record(
                section,
                0,
                OriginModule::Runtime,
            ))
            .map_err(RequestFailure::poison_without_terminal)?;
        }
        if let Some((switch_links, switched)) = switched {
            self.append_event(
                EventSeverity::Info,
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
                switch_links,
                RuntimePayloadDraft::device_backend_switched(switched),
            )?;
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
        let config_version = config.version();
        *lock(&self.runtime_config, ConfigHostLocation::ConfigureSettings)? = config;
        // Workflow #387 D1c (§5.8): the next trigger, admission and renewal read the new values;
        // a ladder already admitted keeps its snapshot (§4.7).
        self.refresh_ladder_config()
            .map_err(RequestFailure::poison_without_terminal)?;
        // LN-9 (#422): the ledger's scrub reads its rate before its next chunk.
        self.refresh_ledger_scrub_rate()
            .map_err(RequestFailure::poison_without_terminal)?;
        // Workflow #407 PF-1: a stream opened from now on takes the chunk in effect.
        self.refresh_stream_admit_chunk()
            .map_err(RequestFailure::poison_without_terminal)?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(terminal(&event)),
            // Workflow #429 R-4: the settings' version after the change.
            result: RuntimeResult::SettingsConfigured {
                settings,
                config_version: Some(config_version),
            },
            failure_outcome: None,
        })
    }

    /// #402 HS-2 (design HS-M §3.2 step 1): the switch `changes` make, when they name an
    /// instance's backend keys and leave its backends in effect (`applied`) other than they are
    /// now (`current`): the registered instance and its two backends before and after. `None`
    /// for a request without a backend key, and for one that leaves both as they are (a clear
    /// back to the same values): that request commits at once, with no device step.
    fn backend_switch_plan(
        &self,
        current: &EffectiveConfig,
        applied: &EffectiveConfig,
        changes: &[ConfigSettingChange],
    ) -> RuntimeHostResult<Option<super::backend_switch::BackendSwitchPlan>> {
        let Some(instance) = actingcommand_runtime_config::backend_switch_instance(changes) else {
            return Ok(None);
        };
        let backends = |config: &EffectiveConfig| -> RuntimeHostResult<SwitchBackends> {
            let name = |member: &str| match config.value(&instance_key(instance, member)) {
                Some(FactScalar::String(name)) => Ok(name.clone()),
                _ => Err(value_type_invalid(ConfigHostLocation::ConfigureSettings)),
            };
            let touch = name(actingcommand_runtime_config::TOUCH_BACKEND)?;
            let capture = name(actingcommand_runtime_config::CAPTURE_BACKEND)?;
            Ok(SwitchBackends {
                touch: actingcommand_device::TouchBackendChoice::parse(&touch)
                    .map_err(|_| value_type_invalid(ConfigHostLocation::ConfigureSettings))?,
                capture: actingcommand_device::CaptureBackendChoice::parse(&capture)
                    .map_err(|_| value_type_invalid(ConfigHostLocation::ConfigureSettings))?,
            })
        };
        let from = backends(current)?;
        let to = backends(applied)?;
        if from == to {
            return Ok(None);
        }
        let registered = lock(
            &self.registered_instances,
            ConfigHostLocation::ConfigureSettings,
        )?
        .values()
        .map(|registered| (registered.instance_id, registered.instance_alias.clone()))
        .collect::<Vec<_>>();
        for (instance_id, instance_alias) in registered {
            if identity_text(&instance_id, ConfigHostLocation::ConfigureSettings)? == instance {
                return Ok(Some(super::backend_switch::BackendSwitchPlan {
                    instance_id,
                    instance_alias,
                    from,
                    to,
                }));
            }
        }
        // The key table has the key only for an instance of the configuration, which the host
        // registered: an internal invariant.
        Err(value_type_invalid(ConfigHostLocation::ConfigureSettings))
    }

    /// #402 HS-2 (design HS-M §3.8): the backends in effect of every registered instance whose
    /// configuration names them, as the start restored them; the start writes those that differ
    /// from its registration into the registry before any instance is prepared.
    pub(super) fn backends_in_effect(&self) -> RuntimeHostResult<Vec<(String, SwitchBackends)>> {
        let registered = lock(
            &self.registered_instances,
            ConfigHostLocation::ReadRuntimeConfig,
        )?
        .values()
        .map(|registered| (registered.instance_id, registered.instance_alias.clone()))
        .collect::<Vec<_>>();
        let config = lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?.clone();
        let mut backends = Vec::new();
        for (instance_id, instance_alias) in registered {
            let instance = identity_text(&instance_id, ConfigHostLocation::ReadRuntimeConfig)?;
            let name = |member: &str| match config.value(&instance_key(&instance, member)) {
                Some(FactScalar::String(name)) => Some(name.clone()),
                _ => None,
            };
            let (Some(touch), Some(capture)) = (
                name(actingcommand_runtime_config::TOUCH_BACKEND),
                name(actingcommand_runtime_config::CAPTURE_BACKEND),
            ) else {
                continue;
            };
            backends.push((
                instance_alias,
                SwitchBackends {
                    touch: actingcommand_device::TouchBackendChoice::parse(&touch)
                        .map_err(|_| value_type_invalid(ConfigHostLocation::ReadRuntimeConfig))?,
                    capture: actingcommand_device::CaptureBackendChoice::parse(&capture)
                        .map_err(|_| value_type_invalid(ConfigHostLocation::ReadRuntimeConfig))?,
                },
            ));
        }
        Ok(backends)
    }

    /// Workflow #429 R-4 (register §1.239): the runtime fact snapshot with the settings' version
    /// now, both read under the gate every commit of the settings holds, so the snapshot's
    /// `config.*` records are those of that version.
    pub(super) fn runtime_fact_snapshot_versioned(
        &self,
    ) -> RuntimeHostResult<(RuntimeFactSnapshot, u64)> {
        let _gate = lock(
            &self.config_override_gate,
            ConfigHostLocation::ReadRuntimeConfig,
        )?;
        let version = lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?.version();
        Ok((self.runtime_fact_snapshot()?, version))
    }

    /// Workflow #429 R-4 (rollback test): records `rows` as `host.config_overrides`, as a later
    /// Runtime recorded them; the configuration in effect does not change.
    #[cfg(test)]
    pub(super) fn record_config_overrides_for_test(
        &self,
        rows: Vec<BTreeMap<String, FactScalar>>,
    ) -> RuntimeHostResult<()> {
        let _gate = lock(
            &self.config_override_gate,
            ConfigHostLocation::PersistConfigOverrides,
        )?;
        self.record_config_fact(overrides_record(rows))
    }

    /// The stuck-recovery settings the ladder reads for one instance now: the values in effect
    /// of the instance's two per-instance keys, an instruction value included (Workflow #387
    /// D1b, §5.5 `settable`), whose defaults are the `recovery_ladder` section's `enabled` and
    /// `cooldown_secs` in effect (D1c, §2.1 row 18b). An instance the configuration does not
    /// carry takes the settings it was started with, or `ladder`'s.
    pub(super) fn instance_stuck_recovery(
        &self,
        instance_id: InstanceId,
        ladder: &LadderConfig,
    ) -> RuntimeHostResult<InstanceStuckRecovery> {
        let mut settings =
            self.stuck_recovery()?
                .get(&instance_id)
                .copied()
                .unwrap_or(InstanceStuckRecovery {
                    enabled: ladder.enabled,
                    cooldown_secs: ladder.cooldown_secs,
                });
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
    /// scheduler's heartbeat bound in effect, the device layer's bounds (Workflow #387 D1c),
    /// (#402 HS-1) the backends each kind of device has and (Workflow #429 R-3) the registered
    /// instances bound by discovery, the only ones whose `start_emulator` may be `true`.
    pub(super) fn config_check(&self) -> RuntimeHostResult<CheckContext> {
        let maximum_client_heartbeat_interval_ms =
            lock(&self.scheduler, ConfigHostLocation::ReadRuntimeConfig)?
                .config()
                .maximum_client_heartbeat_interval_ms;
        let discovery_bound = lock(
            &self.registered_instances,
            ConfigHostLocation::ReadRuntimeConfig,
        )?
        .values()
        .filter(|instance| instance.emulator_start_candidate())
        .map(|instance| crate::failure_identity::identifier_text(&instance.instance_id()))
        .collect::<BTreeSet<_>>();
        Ok(CheckContext::new(
            maximum_client_heartbeat_interval_ms,
            super::recovery_ladder::recovery_ladder_device_bounds(),
        )
        .with_backends(backend_kind_rules())
        .with_discovery_bound(discovery_bound))
    }

    /// Workflow #429 R-3 (register §1.272): whether `instance.<instance id>.start_emulator` of
    /// `instance_id` is on in effect, which every automatic emulator start reads at each decision
    /// (the daemon start, the emulator watch's restart, IDLE-1's wake, and each of their claims
    /// once more on its key): the configuration's value when it carries the key, the file's or an
    /// instruction's; otherwise (a host started without actingd's manifest, or an instance whose
    /// file does not name the key and no instruction sets it) whether the host was configured
    /// with the instance at its start (`with_emulator_autostart`, which actingd fills from the
    /// same file).
    pub(super) fn start_emulator_in_effect(
        &self,
        instance_id: InstanceId,
    ) -> RuntimeHostResult<bool> {
        let key = instance_key(
            &crate::failure_identity::identifier_text(&instance_id),
            actingcommand_runtime_config::START_EMULATOR,
        );
        let carried = lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?
            .value(&key)
            .cloned();
        match carried {
            Some(FactScalar::Boolean(on)) => Ok(on),
            Some(_) => Err(value_type_invalid(ConfigHostLocation::ReadRuntimeConfig)),
            None => Ok(self
                .emulator_autostart()?
                .iter()
                .any(|(configured, _)| *configured == instance_id)),
        }
    }

    /// Workflow #387 D1c: the `recovery_ladder` section in effect, read and checked once per
    /// change of the configuration. A ladder takes the one in effect when it is admitted as
    /// its snapshot (§4.7).
    pub(super) fn ladder_config(&self) -> RuntimeHostResult<Arc<LadderConfig>> {
        if let Some(config) = lock(
            &self.recovery_ladder_config,
            ConfigHostLocation::ReadRuntimeConfig,
        )?
        .as_ref()
        {
            return Ok(Arc::clone(config));
        }
        self.refresh_ladder_config()
    }

    /// Workflow #387 D1c: reads the `recovery_ladder` section again (at start and after every
    /// change of the configuration) and hands it to direction ①'s counter. The section passed
    /// its whole-section check when it was assembled or changed, so a refusal here is an
    /// invariant breach.
    pub(super) fn refresh_ladder_config(&self) -> RuntimeHostResult<Arc<LadderConfig>> {
        let context = self.config_check()?;
        let config = lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?
            .recovery_ladder(&context)
            .map_err(|failure| {
                RuntimeHostError::fatal(
                    failure.failure_code(),
                    ConfigHostLocation::ReadRuntimeConfig,
                    RuntimeErrorCode::RuntimeHalted,
                )
                .with_outcome_values(|outcome| failure.with_values(outcome))
            })?;
        let config = Arc::new(config);
        *lock(
            &self.recovery_ladder_config,
            ConfigHostLocation::ReadRuntimeConfig,
        )? = Some(Arc::clone(&config));
        lock(
            &self.stability_counter,
            ConfigHostLocation::ReadRuntimeConfig,
        )?
        .configure(Arc::clone(&config));
        Ok(config)
    }

    /// LN-9 (#422, LEAN-L §3.7): `ledger_observer.scrub_rows_per_second` in effect (settable),
    /// handed to the ledger's scrub at start, once the instruction values are restored, and after
    /// every change of the configuration; the scrub reads it before its next chunk.
    pub(super) fn refresh_ledger_scrub_rate(&self) -> RuntimeHostResult<()> {
        let section = lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?
            .ledger_observer()
            .map_err(|failure| {
                RuntimeHostError::fatal(
                    failure.failure_code(),
                    ConfigHostLocation::ReadRuntimeConfig,
                    RuntimeErrorCode::RuntimeHalted,
                )
                .with_outcome_values(|outcome| failure.with_values(outcome))
            })?;
        self.ledger
            .set_scrub_rows_per_second(section.scrub_rows_per_second)
            .map_err(|error| {
                RuntimeHostError::relayed_fatal(
                    error.code(),
                    error.operation(),
                    RuntimeErrorCode::LedgerFailure,
                )
            })
    }

    /// Workflow #407 PF-1: `artifact_store.stream_admit_chunk_bytes` in effect (settable), handed
    /// to the artifact store at start, once the instruction values are restored, and after every
    /// change of the configuration; a stream takes it when it opens.
    pub(super) fn refresh_stream_admit_chunk(&self) -> RuntimeHostResult<()> {
        let chunk_bytes = lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?
            .stream_admit_chunk_bytes()
            .map_err(|failure| {
                RuntimeHostError::fatal(
                    failure.failure_code(),
                    ConfigHostLocation::ReadRuntimeConfig,
                    RuntimeErrorCode::RuntimeHalted,
                )
                .with_outcome_values(|outcome| failure.with_values(outcome))
            })?;
        self.artifacts.set_stream_admit_chunk_bytes(chunk_bytes);
        Ok(())
    }

    /// #422 LN-7c Q (`rd-1026/ln7c/final-query.md` §5): the frame cleaner's keys in effect, read at
    /// each of its rounds: `frame_retention_evidence_max_age_days` (settable; a pass takes it at
    /// its start), `ledger_observer.frame_query_steps_per_second` (settable; read after each
    /// statement) and `ledger_observer.frame_query_chunk_frames` (not settable).
    pub(super) fn frame_cleaner_settings(
        &self,
    ) -> RuntimeHostResult<super::frame_retention::CleanerSettings> {
        let refused = |failure: actingcommand_runtime_config::ConfigFailure| {
            RuntimeHostError::fatal(
                failure.failure_code(),
                ConfigHostLocation::ReadRuntimeConfig,
                RuntimeErrorCode::RuntimeHalted,
            )
            .with_outcome_values(|outcome| failure.with_values(outcome))
        };
        let config = lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?;
        let section = config.ledger_observer().map_err(refused)?;
        Ok(super::frame_retention::CleanerSettings {
            evidence_max_age_days: config
                .frame_retention_evidence_max_age_days()
                .map_err(refused)?,
            steps_per_second: section.frame_query_steps_per_second,
            chunk_frames: usize::try_from(section.frame_query_chunk_frames).unwrap_or(usize::MAX),
        })
    }

    /// #411 A64 (ruling C-43): `host_claims.run_end_grace_ms` in effect, read when a run reaches
    /// its deadline.
    pub(super) fn run_end_grace_ms(&self) -> RuntimeHostResult<u64> {
        lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?
            .run_end_grace_ms()
            .map_err(|failure| {
                RuntimeHostError::fatal(
                    failure.failure_code(),
                    ConfigHostLocation::ReadRuntimeConfig,
                    RuntimeErrorCode::RuntimeHalted,
                )
                .with_outcome_values(|outcome| failure.with_values(outcome))
            })
    }

    /// Workflow #387 D1c: `host_claims.renewal_slack_ms` in effect, read at every renewal.
    pub(super) fn renewal_slack_ms(&self) -> RuntimeHostResult<u64> {
        lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?
            .renewal_slack_ms()
            .map_err(|failure| {
                RuntimeHostError::fatal(
                    failure.failure_code(),
                    ConfigHostLocation::ReadRuntimeConfig,
                    RuntimeErrorCode::RuntimeHalted,
                )
                .with_outcome_values(|outcome| failure.with_values(outcome))
            })
    }

    /// #411 A50: `emulator_control.restart_exit_wait_ms` in effect, read at every restart whose
    /// Stop's readiness wait expired: how long it waits for the old VM to exit.
    pub(super) fn restart_exit_wait_ms(&self) -> RuntimeHostResult<u64> {
        lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?
            .restart_exit_wait_ms()
            .map_err(|failure| {
                RuntimeHostError::fatal(
                    failure.failure_code(),
                    ConfigHostLocation::ReadRuntimeConfig,
                    RuntimeErrorCode::RuntimeHalted,
                )
                .with_outcome_values(|outcome| failure.with_values(outcome))
            })
    }

    /// emu-watch: the emulator watch's keys of the `emulator_control` section in effect
    /// (`watch_enabled`, `watch_interval_ms`, `restart_attempts`, `restart_window_ms`), read at
    /// every tick of the watch.
    pub(super) fn emulator_watch_settings(
        &self,
    ) -> RuntimeHostResult<actingcommand_runtime_config::EmulatorWatchConfig> {
        lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?
            .emulator_watch()
            .map_err(|failure| {
                RuntimeHostError::fatal(
                    failure.failure_code(),
                    ConfigHostLocation::ReadRuntimeConfig,
                    RuntimeErrorCode::RuntimeHalted,
                )
                .with_outcome_values(|outcome| failure.with_values(outcome))
            })
    }

    /// IDLE-1: the idle keys in effect for the instance `instance_id` (its own
    /// `instance.<id>.idle.*`, else the `idle` section's), read at every idle check; `None` for an
    /// instance the configuration in effect does not name (a host started without actingd's
    /// manifest), which has no idle action.
    pub(super) fn idle_settings(
        &self,
        instance_id: InstanceId,
    ) -> RuntimeHostResult<Option<actingcommand_runtime_config::IdleConfig>> {
        let id = crate::failure_identity::identifier_text(&instance_id);
        let config = lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?;
        let named = actingcommand_runtime_config::instance_key(
            &id,
            actingcommand_runtime_config::idle::ACTION,
        );
        if config.value(&named).is_none() {
            return Ok(None);
        }
        config.idle(&id).map(Some).map_err(|failure| {
            RuntimeHostError::fatal(
                failure.failure_code(),
                ConfigHostLocation::ReadRuntimeConfig,
                RuntimeErrorCode::RuntimeHalted,
            )
            .with_outcome_values(|outcome| failure.with_values(outcome))
        })
    }

    /// Workflow #392 DA-4b-D (Alice's ruling 69): the `display_safe_area` keys in effect for the
    /// instance `instance_id` (its own `instance.<id>.display_safe_area.model`, else the
    /// section's), read at every frame of a run whose game's display declaration measures on the
    /// safe area, so an instruction takes effect from the next frame.
    pub(super) fn display_safe_area_settings(
        &self,
        instance_id: InstanceId,
    ) -> RuntimeHostResult<actingcommand_runtime_config::DisplaySafeAreaConfig> {
        let id = crate::failure_identity::identifier_text(&instance_id);
        lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?
            .display_safe_area(&id)
            .map_err(|failure| {
                RuntimeHostError::fatal(
                    failure.failure_code(),
                    ConfigHostLocation::ReadRuntimeConfig,
                    RuntimeErrorCode::RuntimeHalted,
                )
                .with_outcome_values(|outcome| failure.with_values(outcome))
            })
    }

    /// Workflow #384 D2-S2: `scheduling.fact_refresh.recognition_join_timeout_ms` in effect, read
    /// when a run joins the readings it handed to recognition.
    pub(super) fn recognition_join_timeout(&self) -> RuntimeHostResult<Duration> {
        lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?
            .recognition_join_timeout_ms()
            .map(Duration::from_millis)
            .map_err(|failure| {
                RuntimeHostError::fatal(
                    failure.failure_code(),
                    ConfigHostLocation::ReadRuntimeConfig,
                    RuntimeErrorCode::RuntimeHalted,
                )
                .with_outcome_values(|outcome| failure.with_values(outcome))
            })
    }

    /// Workflow #384 D2-S4: the `scheduling.fact_refresh` pace in effect, read at every policy
    /// cycle (D2 model §3.4).
    pub(super) fn fact_refresh_settings(
        &self,
    ) -> RuntimeHostResult<actingcommand_policy::FactRefreshSettings> {
        let config = lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?
            .fact_refresh()
            .map_err(|failure| {
                RuntimeHostError::fatal(
                    failure.failure_code(),
                    ConfigHostLocation::ReadRuntimeConfig,
                    RuntimeErrorCode::RuntimeHalted,
                )
                .with_outcome_values(|outcome| failure.with_values(outcome))
            })?;
        Ok(actingcommand_policy::FactRefreshSettings {
            enabled: config.enabled,
            min_interval_ms: config.min_interval_ms,
            daily_limit: config.daily_limit,
            failure_backoff_ms: config.failure_backoff_ms,
            max_consecutive_failures: config.max_consecutive_failures,
            // SCH-M ED-2: the declared duration of a fact refresh read.
            expected_duration_ms: config.expected_duration_ms,
        })
    }

    /// SCH-M ED-2 (§3.4 "Configuration"): the expected-duration keys in effect for the instance
    /// `alias` names (`EffectiveConfig::expected_runtime` by its registry id; an alias the
    /// registry does not hold takes the section's keys), read at every write of a sample and
    /// every projection of the evaluation inputs. The keys are checked whenever the
    /// configuration is loaded or changed, so a value that cannot be read here is a halt.
    pub(super) fn expected_runtime_settings(
        &self,
        alias: &str,
    ) -> RuntimeHostResult<actingcommand_policy::ExpectedRuntimeSettings> {
        let id = lock(
            &self.registered_instances,
            ConfigHostLocation::ReadRuntimeConfig,
        )?
        .values()
        .find(|instance| instance.instance_alias == alias)
        .map(|instance| crate::failure_identity::identifier_text(&instance.instance_id));
        let config = lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?
            .expected_runtime(id.as_deref().unwrap_or(alias))
            .map_err(remedy_config_failure)?;
        Ok(actingcommand_policy::ExpectedRuntimeSettings {
            enabled: config.enabled,
            runs: config.runs,
            margin_ms: config.margin_ms,
        })
    }

    /// Workflow #386 SR-3: the `scheduling.pack_admission` keys in effect, read at each refusal
    /// of a scheduled run's preparation and by the task package admission after start.
    pub(super) fn pack_admission_settings(
        &self,
    ) -> RuntimeHostResult<actingcommand_runtime_config::PackAdmissionConfig> {
        lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?
            .pack_admission()
            .map_err(|failure| {
                RuntimeHostError::fatal(
                    failure.failure_code(),
                    ConfigHostLocation::ReadRuntimeConfig,
                    RuntimeErrorCode::RuntimeHalted,
                )
                .with_outcome_values(|outcome| failure.with_values(outcome))
            })
    }

    /// Workflow #384 D2-S5: `scheduling.fact_refresh.overdue_ms` in effect, read at every policy
    /// cycle's yield point; `None` when the configuration does not set it (nothing is reported).
    pub(super) fn fact_refresh_overdue_ms(&self) -> RuntimeHostResult<Option<u64>> {
        lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?
            .fact_refresh()
            .map(|config| config.overdue_ms)
            .map_err(|failure| {
                RuntimeHostError::fatal(
                    failure.failure_code(),
                    ConfigHostLocation::ReadRuntimeConfig,
                    RuntimeErrorCode::RuntimeHalted,
                )
                .with_outcome_values(|outcome| failure.with_values(outcome))
            })
    }

    /// Workflow #427-A: `scheduling.parallel.enabled` in effect, read by actingd at the start of
    /// every policy round, so an instruction takes effect at the next round.
    pub(super) fn scheduling_parallel_enabled(&self) -> RuntimeHostResult<bool> {
        lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?
            .scheduling_parallel_enabled()
            .map_err(|failure| {
                RuntimeHostError::fatal(
                    failure.failure_code(),
                    ConfigHostLocation::ReadRuntimeConfig,
                    RuntimeErrorCode::RuntimeHalted,
                )
                .with_outcome_values(|outcome| failure.with_values(outcome))
            })
    }

    /// Workflow #429 R-2 (A.1 契约 1): the shared run limits in effect, read at every policy
    /// evaluation and forward projection, so an instruction takes effect at the next round and no
    /// run in progress is cut: `performance.pacing.heavy_dispatch_limit.normal` / `.paused`, which
    /// the controller's resource overlay hands each host, and
    /// `performance.pacing.max_concurrent_runs`, which the policy host hands the evaluator. A limit
    /// the configuration does not carry is `None`, and the value the host was started with holds:
    /// an actingd manifest carries both heavy limits always and `max_concurrent_runs` only while
    /// the file or an instruction sets it (without, no limit, as the assembly read it); a host
    /// started without actingd's manifest keeps its own. The keys are checked whenever the
    /// configuration is loaded or changed, so a value that cannot be read here is a halt.
    pub(super) fn shared_run_limits(&self) -> RuntimeHostResult<SharedRunLimits> {
        use actingcommand_runtime_config::performance::pacing::{
            HEAVY_DISPATCH_LIMIT_NORMAL, HEAVY_DISPATCH_LIMIT_PAUSED, MAX_CONCURRENT_RUNS,
        };
        let config = lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?;
        let limit = |key: &str| match config.value(key) {
            None => Ok(None),
            Some(FactScalar::Integer(limit)) => u16::try_from(*limit)
                .map(Some)
                .map_err(|_| value_type_invalid(ConfigHostLocation::ReadRuntimeConfig)),
            Some(_) => Err(value_type_invalid(ConfigHostLocation::ReadRuntimeConfig)),
        };
        Ok(SharedRunLimits {
            heavy: crate::performance_control::HeavyDispatchLimits {
                normal: limit(HEAVY_DISPATCH_LIMIT_NORMAL)?,
                paused: limit(HEAVY_DISPATCH_LIMIT_PAUSED)?,
            },
            max_concurrent_runs: limit(MAX_CONCURRENT_RUNS)?,
        })
    }

    /// ISO-PE (#419): `scheduling.evaluation.retry_ms` and `.escalate_after` in effect, read at
    /// every policy evaluation the Runtime skips, so an instruction takes effect at the next one.
    pub(super) fn evaluation_retry_settings(
        &self,
    ) -> RuntimeHostResult<actingcommand_runtime_config::EvaluationRetryConfig> {
        Ok(
            lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?
                .scheduling()
                .map_err(remedy_config_failure)?
                .evaluation,
        )
    }

    /// Workflow #386 SR-2b-1: the `scheduling.rerun` values in effect, read at each live
    /// settlement and online reconciliation, which decide a run's `condition` by them.
    pub(super) fn rerun_condition_rule(
        &self,
    ) -> RuntimeHostResult<crate::policy_control::ConditionRule> {
        let scheduling = lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?
            .scheduling()
            .map_err(remedy_config_failure)?;
        Ok(crate::policy_control::ConditionRule::from_config(
            &scheduling.rerun,
        ))
    }

    /// Workflow #386 SR-2b-2: the `scheduling.rerun` values in effect that gate and rank an owed
    /// rerun (`resume_stage`, `resume_settle_ms`, `promote`), read at each policy evaluation.
    pub(super) fn rerun_gate_settings(
        &self,
    ) -> RuntimeHostResult<crate::policy_control::RerunGateSettings> {
        let scheduling = lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?
            .scheduling()
            .map_err(remedy_config_failure)?;
        Ok(crate::policy_control::RerunGateSettings::from_config(
            &scheduling.rerun,
        ))
    }

    /// Workflow #336 PKG-S3: `scheduling.suspension.lift_check_min_interval_ms` in effect, read
    /// at each suspension lift check.
    pub(super) fn lift_check_min_interval_ms(&self) -> RuntimeHostResult<u64> {
        lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?
            .lift_check_min_interval_ms()
            .map_err(|failure| {
                RuntimeHostError::fatal(
                    failure.failure_code(),
                    ConfigHostLocation::ReadRuntimeConfig,
                    RuntimeErrorCode::RuntimeHalted,
                )
                .with_outcome_values(|outcome| failure.with_values(outcome))
            })
    }

    /// Workflow #411 B15 R4 (model-b15 §5.2, §9.2): the remedy keys in effect for the instances
    /// `aliases` name (`EffectiveConfig::remedy` by each one's registry id):
    /// `scheduling.remedy.enabled` and `.max_per_round`, each instance's choice rows in effect
    /// (its own `instance.<id>.scheduling.remedy.choices` when it sets one, otherwise the
    /// section's list), and `scheduling.suspension.lift_check_min_interval_ms`, the least time
    /// between two rebuilds of the remedy table while a round is open or a stop is recorded.
    /// With no instance there is nothing to plan.
    pub(super) fn remedy_context(
        &self,
        aliases: &[&str],
        operation: HostLocation,
    ) -> RuntimeHostResult<crate::remedy::RemedyContext> {
        let ids = lock(&self.registered_instances, operation)?
            .values()
            .map(|instance| {
                (
                    instance.instance_alias.clone(),
                    crate::failure_identity::identifier_text(&instance.instance_id),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let config = lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?;
        let mut switch = None;
        let mut configuration = BTreeMap::new();
        for alias in aliases {
            // An instance the registry does not hold has no list of its own (the per-instance
            // keys name registry ids): the section's list is in effect.
            let id = ids.get(*alias).map_or(*alias, String::as_str);
            let remedy = config.remedy(id).map_err(remedy_config_failure)?;
            switch.get_or_insert((remedy.enabled, remedy.max_per_round));
            configuration.insert(
                (*alias).to_owned(),
                remedy.rows.iter().map(crate::remedy::policy_row).collect(),
            );
        }
        let table_interval_ms = config
            .lift_check_min_interval_ms()
            .map_err(remedy_config_failure)?;
        let (enabled, max_per_round) = switch.unwrap_or((false, 1));
        Ok(crate::remedy::RemedyContext {
            enabled,
            max_per_round,
            configuration,
            table_interval_ms,
        })
    }

    /// Workflow #405 D1: the random delay in effect for each instance `aliases` name
    /// (`EffectiveConfig::random_delay` by its registry id; an alias the registry does not hold
    /// takes the section's keys), by alias, read at every authoritative policy cycle.
    pub(super) fn random_delay_context(
        &self,
        aliases: &[&str],
        operation: HostLocation,
    ) -> RuntimeHostResult<BTreeMap<String, actingcommand_runtime_config::RandomDelay>> {
        let ids = lock(&self.registered_instances, operation)?
            .values()
            .map(|instance| {
                (
                    instance.instance_alias.clone(),
                    crate::failure_identity::identifier_text(&instance.instance_id),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let config = lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?;
        let mut delays = BTreeMap::new();
        for alias in aliases {
            let id = ids.get(*alias).map_or(*alias, String::as_str);
            // The keys are checked whenever the configuration is loaded or changed, so a pair
            // that cannot be read here is a halt, as a remedy key's is.
            let delay = config.random_delay(id).map_err(remedy_config_failure)?;
            delays.insert((*alias).to_owned(), delay);
        }
        Ok(delays)
    }

    /// SS-4 (register §1.231): `scheduling.no_progress.enabled` in effect for each instance
    /// `aliases` name (`EffectiveConfig::no_progress_enabled` by its registry id; an alias the
    /// registry does not hold takes the section's key), by alias, read at every authoritative
    /// policy cycle.
    pub(super) fn no_progress_context(
        &self,
        aliases: &[&str],
        operation: HostLocation,
    ) -> RuntimeHostResult<BTreeMap<String, bool>> {
        let ids = lock(&self.registered_instances, operation)?
            .values()
            .map(|instance| {
                (
                    instance.instance_alias.clone(),
                    crate::failure_identity::identifier_text(&instance.instance_id),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let config = lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?;
        let mut enabled = BTreeMap::new();
        for alias in aliases {
            let id = ids.get(*alias).map_or(*alias, String::as_str);
            // The key is checked whenever the configuration is loaded or changed, so a value
            // that cannot be read here is a halt, as a random delay key's is.
            let on = config
                .no_progress_enabled(id)
                .map_err(remedy_config_failure)?;
            enabled.insert((*alias).to_owned(), on);
        }
        Ok(enabled)
    }

    /// Workflow #386 SR-4a2 (SCH-M §3.2 D-1, D-4, D-6): the blackout keys in effect for the
    /// instances `aliases` name, read at every authoritative policy cycle and at every policy
    /// admission: the host list, each instance's own list (`EffectiveConfig::instance_blackouts`
    /// by its registry id; an alias the registry does not hold has none), the quiet keys and
    /// each instance's day-boundary switch in effect. The keys are checked whenever the
    /// configuration is loaded or changed, so one that cannot be read here is a halt.
    pub(super) fn blackout_context(
        &self,
        aliases: &[&str],
        operation: HostLocation,
    ) -> RuntimeHostResult<crate::policy_host::BlackoutContext> {
        let ids = lock(&self.registered_instances, operation)?
            .values()
            .map(|instance| {
                (
                    instance.instance_alias.clone(),
                    crate::failure_identity::identifier_text(&instance.instance_id),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let config = lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?;
        let scheduling = config.scheduling().map_err(remedy_config_failure)?;
        let blackout = scheduling.blackout;
        let mut instances = BTreeMap::new();
        let mut day_boundary_quiet = BTreeMap::new();
        for alias in aliases {
            // The per-instance keys name registry ids; an alias the registry does not hold names
            // no key of its own, so the section's switch and no list of its own are in effect.
            let id = ids.get(*alias).map_or(*alias, String::as_str);
            day_boundary_quiet.insert(
                (*alias).to_owned(),
                config
                    .day_boundary_quiet(id)
                    .map_err(remedy_config_failure)?,
            );
            let own = config
                .instance_blackouts(id)
                .map_err(remedy_config_failure)?;
            if !own.is_empty() {
                instances.insert(
                    (*alias).to_owned(),
                    own.iter()
                        .map(crate::policy_host::policy_blackout_entry)
                        .collect(),
                );
            }
        }
        Ok(crate::policy_host::BlackoutContext {
            host: blackout
                .host
                .iter()
                .map(crate::policy_host::policy_blackout_entry)
                .collect(),
            instances,
            day_boundary_quiet,
            day_boundary_section: scheduling.day_boundary_quiet,
            quiet_margin_ms: blackout.quiet_margin_ms,
            hard_quiet_ms: blackout.hard_quiet_ms,
            quiet_horizon: blackout.quiet_horizon,
        })
    }

    /// Workflow #398 RD-2: the `device_link` section in effect (model-398 §4.6), read at every
    /// link change, restore and watch restart.
    pub(super) fn device_link_config(
        &self,
    ) -> RuntimeHostResult<actingcommand_runtime_config::DeviceLinkConfig> {
        lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?
            .device_link()
            .map_err(|failure| {
                RuntimeHostError::fatal(
                    failure.failure_code(),
                    ConfigHostLocation::ReadRuntimeConfig,
                    RuntimeErrorCode::RuntimeHalted,
                )
                .with_outcome_values(|outcome| failure.with_values(outcome))
            })
    }

    /// Workflow #398 RD-5b: the `device_state` section in effect (model-398 §7.8), read at every
    /// sample, start gate and input of a handset.
    pub(super) fn device_state_config(
        &self,
    ) -> RuntimeHostResult<actingcommand_runtime_config::DeviceStateConfig> {
        lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?
            .device_state()
            .map_err(|failure| {
                RuntimeHostError::fatal(
                    failure.failure_code(),
                    ConfigHostLocation::ReadRuntimeConfig,
                    RuntimeErrorCode::RuntimeHalted,
                )
                .with_outcome_values(|outcome| failure.with_values(outcome))
            })
    }

    /// STORE-FG-2 (design-store-fg §2.2): whether `store_foreground.allowed_application_ids` in
    /// effect lists `store_application_id`, read at a store update package's run start and at
    /// each of its store taps. The key is not settable, so the list is the configuration file's
    /// as the Runtime started with it.
    pub(super) fn store_foreground_allowed(
        &self,
        store_application_id: &str,
    ) -> RuntimeHostResult<bool> {
        lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?
            .store_foreground()
            .map(|allowlist| allowlist.allows(store_application_id))
            .map_err(|failure| {
                RuntimeHostError::fatal(
                    failure.failure_code(),
                    ConfigHostLocation::ReadRuntimeConfig,
                    RuntimeErrorCode::RuntimeHalted,
                )
                .with_outcome_values(|outcome| failure.with_values(outcome))
            })
    }

    /// Workflow #386 SR-1: `scheduling.recognition.sample_timeout_retries` in effect, read at
    /// each step of a contained run (the kernel's `step_pacing()`).
    pub(super) fn sample_timeout_retries(&self) -> RuntimeHostResult<u32> {
        lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?
            .sample_timeout_retries()
            .map_err(|failure| {
                RuntimeHostError::fatal(
                    failure.failure_code(),
                    ConfigHostLocation::ReadRuntimeConfig,
                    RuntimeErrorCode::RuntimeHalted,
                )
                .with_outcome_values(|outcome| failure.with_values(outcome))
            })
    }

    /// Workflow #406 T12 (plan Q13): the `touch` section in effect, read at each step of a
    /// contained run (the kernel's `touch_sampling()`) and at each choice of a press's landing
    /// point.
    pub(super) fn touch_config(
        &self,
    ) -> RuntimeHostResult<actingcommand_runtime_config::TouchConfig> {
        lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?
            .touch()
            .map_err(|failure| {
                RuntimeHostError::fatal(
                    failure.failure_code(),
                    ConfigHostLocation::ReadRuntimeConfig,
                    RuntimeErrorCode::RuntimeHalted,
                )
                .with_outcome_values(|outcome| failure.with_values(outcome))
            })
    }

    /// PERF-B1 (Workflow #407 B1): the `execution.capture` keys in effect, read at each step of a
    /// contained run and handed to the kernel with the step's pacing. STILL-1 (register §1.249):
    /// with the still limit in effect for the run's instance
    /// (`EffectiveConfig::still_limit_ms` by its registry id; an alias the registry does not hold
    /// takes the section's key).
    pub(super) fn capture_cadence(
        &self,
        instance_alias: &str,
    ) -> RuntimeHostResult<actingcommand_execution_kernel::CaptureCadence> {
        let instance_id = lock(
            &self.registered_instances,
            ConfigHostLocation::ReadRuntimeConfig,
        )?
        .values()
        .find(|instance| instance.instance_alias == instance_alias)
        .map(|instance| crate::failure_identity::identifier_text(&instance.instance_id));
        let config = lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?;
        let cadence = config.capture_cadence().map_err(|failure| {
            RuntimeHostError::fatal(
                failure.failure_code(),
                ConfigHostLocation::ReadRuntimeConfig,
                RuntimeErrorCode::RuntimeHalted,
            )
            .with_outcome_values(|outcome| failure.with_values(outcome))
        })?;
        let still_limit_ms = config
            .still_limit_ms(instance_id.as_deref().unwrap_or(instance_alias))
            .map_err(remedy_config_failure)?;
        Ok(actingcommand_execution_kernel::CaptureCadence::new(
            cadence.max_interval_ms,
            cadence.backoff_enabled,
            cadence.idle_ceiling_ms,
        )
        .with_still_limit_ms(still_limit_ms))
    }

    /// LOAD-3 (Workflow #407, register §1.260): `execution.loading.mode` in effect for the instance
    /// `instance_alias` (`EffectiveConfig::loading_mode` by its registry id; an alias the registry
    /// does not hold takes the section's), read at each step of a contained run and handed to the
    /// kernel with the step's pacing.
    pub(super) fn loading_mode(
        &self,
        instance_alias: &str,
    ) -> RuntimeHostResult<actingcommand_execution_kernel::LoadingMode> {
        let instance_id = lock(
            &self.registered_instances,
            ConfigHostLocation::ReadRuntimeConfig,
        )?
        .values()
        .find(|instance| instance.instance_alias == instance_alias)
        .map(|instance| crate::failure_identity::identifier_text(&instance.instance_id));
        let config = lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?;
        let mode = config
            .loading_mode(instance_id.as_deref().unwrap_or(instance_alias))
            .map_err(|failure| {
                RuntimeHostError::fatal(
                    failure.failure_code(),
                    ConfigHostLocation::ReadRuntimeConfig,
                    RuntimeErrorCode::RuntimeHalted,
                )
                .with_outcome_values(|outcome| failure.with_values(outcome))
            })?;
        Ok(match mode {
            actingcommand_runtime_config::LoadingModeConfig::Off => {
                actingcommand_execution_kernel::LoadingMode::Off
            }
            actingcommand_runtime_config::LoadingModeConfig::Shadow => {
                actingcommand_execution_kernel::LoadingMode::Shadow
            }
            actingcommand_runtime_config::LoadingModeConfig::On => {
                actingcommand_execution_kernel::LoadingMode::On
            }
        })
    }

    /// ACT-1, ACT-2 (Workflow #407, register §1.250): the `execution.action_gap` keys in effect
    /// for the instance `instance_alias` (`EffectiveConfig::action_gap` by its registry id; an
    /// alias the registry does not hold takes the section's), read at each step of a contained run.
    pub(super) fn action_gap(
        &self,
        instance_alias: &str,
    ) -> RuntimeHostResult<actingcommand_runtime_config::ActionGapConfig> {
        let instance_id = lock(
            &self.registered_instances,
            ConfigHostLocation::ReadRuntimeConfig,
        )?
        .values()
        .find(|instance| instance.instance_alias == instance_alias)
        .map(|instance| crate::failure_identity::identifier_text(&instance.instance_id));
        let config = lock(&self.runtime_config, ConfigHostLocation::ReadRuntimeConfig)?;
        config
            .action_gap(instance_id.as_deref().unwrap_or(instance_alias))
            .map_err(|failure| {
                RuntimeHostError::fatal(
                    failure.failure_code(),
                    ConfigHostLocation::ReadRuntimeConfig,
                    RuntimeErrorCode::RuntimeHalted,
                )
                .with_outcome_values(|outcome| failure.with_values(outcome))
            })
    }

    /// ACT-1: whether `operation_label` of `package_id` is held off the settled path at `now`.
    pub(super) fn settled_path_held(
        &self,
        package_id: &str,
        operation_label: &str,
        now: Instant,
    ) -> RuntimeHostResult<bool> {
        Ok(
            lock(&self.settled_holds, ConfigHostLocation::ReadRuntimeConfig)?.held(
                package_id,
                operation_label,
                now,
            ),
        )
    }

    /// ACT-1: `operation_label` of `package_id` fell back from the settled path at `now`, held for
    /// `span`.
    pub(super) fn hold_settled_path(
        &self,
        package_id: &str,
        operation_label: &str,
        now: Instant,
        span: Duration,
    ) -> RuntimeHostResult<()> {
        lock(&self.settled_holds, ConfigHostLocation::ReadRuntimeConfig)?.hold(
            package_id,
            operation_label,
            now,
            span,
        );
        Ok(())
    }

    /// Records one configuration fact, observed now and strictly after the stored record of its
    /// key, ledger-first through [`Self::record_runtime_fact`].
    /// Workflow #389 MCP-9a (register A5): records `config.catalog`, the active policy catalog
    /// generation as the ledger records it: the last `catalog.activated` or `catalog.rolled_back`
    /// record that names its hash, the latest approval decision whose target is that generation,
    /// and the generation that record replaced. Nothing is recorded while no generation is active,
    /// for a generation only the legacy active-pointer migration made active (it has no such
    /// record), or when the value is the one already recorded. A fatal failure marks the
    /// Runtime.
    pub(super) fn record_config_catalog(&self) -> RuntimeHostResult<()> {
        let result = (|| -> RuntimeHostResult<()> {
            let Some(generation) = self.active_policy_catalog()? else {
                return Ok(());
            };
            // LN-5 (#422, LEAN-L §3.3): each type's history is read from the store page by page.
            let read = |event_type| {
                self.ledger
                    .latest_sequence()
                    .and_then(|through| {
                        crate::recovery_read::read_pages(
                            &self.ledger,
                            &EventQuery {
                                event_type: Some(event_type),
                                ..EventQuery::default()
                            },
                            through,
                            self.ledger.history_page_events(),
                        )
                    })
                    .map_err(|error| crate::policy_host::catalog_ledger_error(&error))
            };
            let mut transitions = Vec::new();
            for event_type in [EventType::CatalogActivated, EventType::CatalogRolledBack] {
                for event in read(event_type)? {
                    if let EventPayload::Catalog(
                        CatalogPayload::Activated(payload) | CatalogPayload::RolledBack(payload),
                    ) = event.payload()
                    {
                        transitions.push((event.sequence(), *event.event_id(), payload.clone()));
                    }
                }
            }
            transitions.sort_by_key(|(sequence, _, _)| *sequence);
            let Some((_, activated, made_active)) = transitions
                .iter()
                .rev()
                .find(|(_, _, payload)| payload.catalog_hash() == generation.catalog_hash())
            else {
                return Ok(());
            };
            let previous = made_active.previous_catalog_hash().and_then(|previous| {
                transitions
                    .iter()
                    .find(|(_, _, payload)| payload.catalog_hash() == previous)
                    .map(|(_, _, payload)| {
                        (payload.catalog_id().to_owned(), payload.catalog_version())
                    })
            });
            let approval = read(EventType::ApprovalDecision)?
                .into_iter()
                .filter_map(|event| match event.payload() {
                    EventPayload::Approval(ApprovalPayload::Decision(payload)) => {
                        Some(payload.decision().clone())
                    }
                    _ => None,
                })
                .rfind(|decision| {
                    matches!(
                        decision.target(),
                        ApprovalTarget::Catalog {
                            catalog_hash,
                            catalog_version,
                        } if catalog_hash == generation.catalog_hash()
                            && *catalog_version == generation.catalog_version()
                    )
                });
            let catalog = ConfigCatalog {
                catalog_id: generation.catalog_id().to_owned(),
                catalog_version: generation.catalog_version(),
                catalog_hash: generation.catalog_hash().to_owned(),
                activated_event_id: identity_text(
                    activated,
                    ConfigHostLocation::RecordRuntimeConfig,
                )?,
                approval_id: approval
                    .as_ref()
                    .map(|decision| decision.approval_id().to_owned()),
                approval_disposition: approval.as_ref().map(ApprovalDecisionRecord::disposition),
                previous_catalog_id: previous.as_ref().map(|(catalog_id, _)| catalog_id.clone()),
                previous_catalog_version: previous.map(|(_, version)| version),
            };
            let record = catalog.to_fact_record(0, OriginModule::Policy);
            let unchanged = lock(&self.runtime_facts, ConfigHostLocation::ReadRuntimeConfig)?
                .get(&RuntimeFactScope::Runtime, CONFIG_CATALOG_FACT_KEY)
                .is_some_and(|stored| stored.value == record.value);
            if unchanged {
                return Ok(());
            }
            self.record_config_fact(record)
        })();
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.fatal.mark(error.clone())?;
        }
        result
    }

    /// Records one configuration fact, observed now and strictly after the stored record of its
    /// key; #428 CW-1: the workbench records `config.catalog_warnings` with it.
    pub(super) fn record_config_fact(
        &self,
        mut record: RuntimeFactRecord,
    ) -> RuntimeHostResult<()> {
        let now = self.clock.sample()?.unix_ms;
        let previous = lock(&self.runtime_facts, ConfigHostLocation::ReadRuntimeConfig)?
            .get(&record.scope, &record.key)
            .map(|stored| stored.observed_at_unix_ms);
        record.observed_at_unix_ms = match previous {
            Some(previous) => now.max(previous.checked_add(1).ok_or_else(|| {
                RuntimeHostError::fatal(
                    ConfigHostCode::OverridesFactClockOverflow,
                    ConfigHostLocation::RecordRuntimeConfig,
                    RuntimeErrorCode::RuntimeHalted,
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
                    ConfigHostLocation::RestoreConfigOverrides,
                    "runtime_host",
                    ConfigHostLocation::RestoreConfigOverrides.as_str(),
                ),
                AuditInput::new(),
            ),
        )
        .map(|_| ())
    }
}

/// #402 HS-1 (HS-M §2.1): the backends each kind of device has, by the device layer's own rule
/// (`TouchBackendChoice::allowed_on`, `CaptureBackendChoice::allowed_on`, which actingd's device
/// check applies to the file too): an emulator has every touch backend and every capture backend
/// but `scrcpy`; a handset has no `nemu_ipc` and no `droidcast_raw`. The configuration's
/// whole-instance backend check reads them from its check context, since
/// `actingcommand-runtime-config` depends on the contract only; a fixture has none, by the check's
/// own rule.
pub fn backend_kind_rules() -> actingcommand_runtime_config::BackendKindRules {
    let backends_of = |kind: actingcommand_device::DeviceKind| {
        actingcommand_runtime_config::DeviceBackends::admitting(
            |name| {
                actingcommand_device::TouchBackendChoice::parse(name)
                    .is_ok_and(|choice| choice.allowed_on(kind))
            },
            |name| {
                actingcommand_device::CaptureBackendChoice::parse(name)
                    .is_ok_and(|choice| choice.allowed_on(kind))
            },
        )
    };
    actingcommand_runtime_config::BackendKindRules {
        emulator: backends_of(actingcommand_device::DeviceKind::Emulator),
        handset: backends_of(actingcommand_device::DeviceKind::Handset),
    }
}

/// Workflow #429 R-2: the shared run limits in effect at one policy evaluation
/// ([`HostShared::shared_run_limits`]); `None` where the configuration does not carry the key.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct SharedRunLimits {
    pub(super) heavy: crate::performance_control::HeavyDispatchLimits,
    pub(super) max_concurrent_runs: Option<u16>,
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
        RuntimeErrorCode::RuntimeHalted,
    )
}

/// Workflow #411 B15 R4: a remedy key the configuration in effect cannot give. The keys are
/// checked when the configuration is loaded or changed, so this is a halt.
fn remedy_config_failure(failure: actingcommand_runtime_config::ConfigFailure) -> RuntimeHostError {
    RuntimeHostError::fatal(
        failure.failure_code(),
        ConfigHostLocation::ReadRuntimeConfig,
        RuntimeErrorCode::RuntimeHalted,
    )
    .with_outcome_values(|outcome| failure.with_values(outcome))
}

fn value_type_invalid(operation: ConfigHostLocation) -> RuntimeHostError {
    RuntimeHostError::fatal(
        ConfigHostCode::ValueTypeInvalid,
        operation,
        RuntimeErrorCode::RuntimeHalted,
    )
}

/// Workflow #398 RD-4 (model-398 v1.1 §6.2): the ladder profile the registered instance
/// `resolved` climbs by in `ladder`: the one its own `recovery_ladder_profile` names, else the one
/// `kind_profiles` gives its device kind (a handset is an instance whose bound endpoint is one);
/// `None` when it climbs the section's `rungs`. A ladder admitted with `ladder` keeps it to its
/// end, as it keeps the section (§4.7).
pub(super) fn ladder_profile(
    resolved: &RegisteredInstance,
    ladder: &LadderConfig,
) -> RuntimeHostResult<Option<LadderProfile>> {
    let instance = identity_text(
        &resolved.instance_id(),
        ConfigHostLocation::ReadRuntimeConfig,
    )?;
    let kind = if resolved
        .bound_adb_endpoint()
        .and_then(ResolvedAdbEndpoint::handset)
        .is_some()
    {
        InstanceDeviceKind::Handset
    } else {
        InstanceDeviceKind::Emulator
    };
    Ok(ladder.profile_for(&instance, kind).cloned())
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
