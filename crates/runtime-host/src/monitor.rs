// SPDX-License-Identifier: AGPL-3.0-only

use crate::events::RuntimeEvents;
use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::{
    AuditInput, CommandPayloadDraft, EffectDisposition, EventAction, EventActor, EventQuery,
    EventSeverity, EventSource, MAX_MONITOR_IMPORT_BYTES, MAX_RUNTIME_OBSERVED_INSTANCES,
    MonitorDecision, OriginModule, OwnerEpoch, RuntimeErrorCode, RuntimeMonitorChange,
    RuntimeMonitorChangeKind, RuntimeMonitorImport, RuntimeMonitorInstanceStatus,
    RuntimeMonitorJournalSource, RuntimeMonitorPolicy, RuntimeMonitorRegistryStatus,
    RuntimeMonitorState, RuntimeStateFact,
};
use actingcommand_ledger::{GlobalLedger, PersistedEvent};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
use std::path::Path;

pub(crate) const MONITOR_FILE_NAME: &str = "monitor.journal";
const MONITOR_SCHEMA_VERSION: &str = "actingcommand.runtime-monitor.v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MonitorRecord {
    schema_version: String,
    revision: u64,
    monitors: Vec<RuntimeMonitorInstanceStatus>,
}

pub(crate) struct MonitorUpdate {
    pub(crate) changed: bool,
    pub(crate) fact: RuntimeStateFact,
}

#[derive(Clone)]
pub(crate) struct DueMonitorProbe {
    pub(crate) instance_alias: String,
    pub(crate) policy: RuntimeMonitorPolicy,
    configuration_version: u64,
    state: RuntimeMonitorState,
}

pub(crate) struct MonitorRegistry {
    owner_epoch: OwnerEpoch,
    revision: u64,
    last_sequence: u64,
    imported: Option<RuntimeMonitorImport>,
    monitors: BTreeMap<String, RuntimeMonitorInstanceStatus>,
    configuration_versions: BTreeMap<String, u64>,
    allowed_aliases: BTreeSet<String>,
}

impl MonitorRegistry {
    pub(crate) fn open(
        state_root: &Path,
        allowed_aliases: impl IntoIterator<Item = String>,
        owner_epoch: OwnerEpoch,
        ledger: &GlobalLedger,
        events: &RuntimeEvents,
    ) -> RuntimeHostResult<Self> {
        match Self::recover(state_root, allowed_aliases, owner_epoch, ledger, events) {
            Ok(registry) => Ok(registry),
            Err(error) => {
                if error.projection().code != RuntimeErrorCode::LedgerFailure {
                    let draft = events.draft(
                        EventSeverity::Fatal,
                        EventSource::Runtime,
                        OriginModule::Runtime,
                        EventActor::Runtime,
                        events.system_links()?,
                        actingcommand_contract::RuntimePayloadDraft::failed(
                            actingcommand_contract::DiagnosticCode::RuntimeDiagnostic,
                            EffectDisposition::NotPerformed,
                            actingcommand_contract::DiagnosticDetailDraft::new(
                                "monitor_registry",
                                "runtime.monitor.restore",
                                "runtime",
                                error.operation(),
                                error.code(),
                                actingcommand_contract::Sensitivity::Sensitive,
                            ),
                            AuditInput::new(),
                        ),
                    )?;
                    let recorded = ledger
                        .append(events.sanitize(draft)?)
                        .map_err(|_| monitor_ledger_error("record_monitor_restore_failure"))?;
                    let _ = error.lifecycle.recorded_event.set(*recorded.event_id());
                }
                Err(error)
            }
        }
    }

    fn recover(
        state_root: &Path,
        allowed_aliases: impl IntoIterator<Item = String>,
        owner_epoch: OwnerEpoch,
        ledger: &GlobalLedger,
        events: &RuntimeEvents,
    ) -> RuntimeHostResult<Self> {
        let allowed_aliases = allowed_aliases.into_iter().collect::<BTreeSet<_>>();
        if allowed_aliases.is_empty() || allowed_aliases.len() > MAX_RUNTIME_OBSERVED_INSTANCES {
            return Err(monitor_error(
                "invalid_monitor_instance_registry",
                "open_monitor_registry",
            ));
        }
        let mut registry = Self {
            owner_epoch,
            revision: 0,
            last_sequence: 0,
            imported: None,
            monitors: BTreeMap::new(),
            configuration_versions: BTreeMap::new(),
            allowed_aliases,
        };
        let through = ledger
            .latest_sequence()
            .map_err(|_| monitor_ledger_error("read_monitor_registry"))?;
        let mut after = 0;
        while after < through {
            let page = ledger
                .query_page(
                    EventQuery {
                        origin_module: Some(OriginModule::Runtime),
                        ..EventQuery::default()
                    },
                    after,
                    through,
                    128,
                )
                .map_err(|_| monitor_ledger_error("read_monitor_registry"))?;
            if page.is_empty() {
                break;
            }
            for event in page {
                after = event.sequence();
                registry.apply(&event)?;
            }
        }
        let original = read_legacy_snapshot(state_root, &registry.allowed_aliases)?;
        if let Some(imported) = &registry.imported {
            if imported != &original {
                return Err(monitor_error(
                    "monitor_import_source_conflict",
                    "open_monitor_registry",
                ));
            }
        } else {
            let fact = RuntimeStateFact::MonitorImported {
                owner_epoch,
                import: original,
            };
            let draft = events.draft(
                EventSeverity::Info,
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
                events.system_links()?,
                CommandPayloadDraft::validated_runtime_state(
                    EventAction::RuntimeAction,
                    EffectDisposition::NotPerformed,
                    fact,
                    AuditInput::new(),
                ),
            )?;
            let event = ledger
                .append(events.sanitize(draft)?)
                .map_err(|_| monitor_ledger_error("import_monitor_registry"))?;
            registry.apply(&event)?;
        }
        Ok(registry)
    }

    pub(crate) fn prepare_configure(
        &self,
        instance_alias: &str,
        policy: RuntimeMonitorPolicy,
        now_unix_ms: u64,
    ) -> RuntimeHostResult<MonitorUpdate> {
        self.require_allowed(instance_alias)?;
        policy
            .validate()
            .map_err(|_| monitor_error("monitor_policy_invalid", "configure_monitor_policy"))?;
        if let Some(existing) = self.monitors.get(instance_alias)
            && existing.policy() == Some(&policy)
        {
            return self.update(
                RuntimeMonitorChangeKind::Configure,
                existing.clone(),
                false,
                None,
                None,
            );
        }
        let state = RuntimeMonitorState::scheduled(now_unix_ms)
            .map_err(|_| monitor_error("monitor_state_invalid", "configure_monitor_policy"))?;
        let status = RuntimeMonitorInstanceStatus::configured(instance_alias, policy, state)
            .map_err(|_| invalid_monitor_record("configure_monitor_policy"))?;
        self.update(
            RuntimeMonitorChangeKind::Configure,
            status,
            true,
            None,
            None,
        )
    }

    pub(crate) fn prepare_clear(&self, instance_alias: &str) -> RuntimeHostResult<MonitorUpdate> {
        self.require_allowed(instance_alias)?;
        let status = RuntimeMonitorInstanceStatus::unconfigured(instance_alias)
            .map_err(|_| invalid_monitor_record("clear_monitor_policy"))?;
        self.update(
            RuntimeMonitorChangeKind::Clear,
            status,
            self.monitors.contains_key(instance_alias),
            None,
            None,
        )
    }

    pub(crate) fn status(
        &self,
        owner_epoch: OwnerEpoch,
    ) -> RuntimeHostResult<RuntimeMonitorRegistryStatus> {
        let instances = self
            .allowed_aliases
            .iter()
            .map(|alias| self.instance_status(alias))
            .collect::<RuntimeHostResult<Vec<_>>>()?;
        RuntimeMonitorRegistryStatus::new(owner_epoch, instances)
            .map_err(|_| invalid_monitor_record("read_monitor_status"))
    }

    pub(crate) fn due(
        &self,
        now_unix_ms: u64,
        maximum: usize,
    ) -> RuntimeHostResult<Vec<DueMonitorProbe>> {
        if now_unix_ms == 0 || maximum == 0 {
            return Err(monitor_error(
                "monitor_due_query_invalid",
                "read_due_monitors",
            ));
        }
        self.monitors
            .values()
            .filter(|status| {
                status
                    .state()
                    .is_some_and(|state| state.next_due_unix_ms() <= now_unix_ms)
            })
            .take(maximum)
            .map(|status| {
                Ok(DueMonitorProbe {
                    instance_alias: status.instance_alias().to_string(),
                    policy: status
                        .policy()
                        .cloned()
                        .ok_or_else(|| invalid_monitor_record("read_due_monitors"))?,
                    state: status
                        .state()
                        .cloned()
                        .ok_or_else(|| invalid_monitor_record("read_due_monitors"))?,
                    configuration_version: self.configuration_version(status.instance_alias()),
                })
            })
            .collect()
    }

    pub(crate) fn prepare_completion(
        &self,
        probe: &DueMonitorProbe,
        started_at_unix_ms: u64,
        completed_at_unix_ms: u64,
        decision: MonitorDecision,
    ) -> RuntimeHostResult<MonitorUpdate> {
        let state = probe
            .state
            .completed(
                probe.policy.interval_ms(),
                started_at_unix_ms,
                completed_at_unix_ms,
                decision,
            )
            .map_err(|_| monitor_error("monitor_state_invalid", "complete_monitor_probe"))?;
        self.finish_probe(probe, state)
    }

    pub(crate) fn prepare_failure(
        &self,
        probe: &DueMonitorProbe,
        started_at_unix_ms: u64,
        completed_at_unix_ms: u64,
        error: RuntimeErrorCode,
    ) -> RuntimeHostResult<MonitorUpdate> {
        let state = probe
            .state
            .failed(
                probe.policy.interval_ms(),
                started_at_unix_ms,
                completed_at_unix_ms,
                error,
            )
            .map_err(|_| monitor_error("monitor_state_invalid", "fail_monitor_probe"))?;
        self.finish_probe(probe, state)
    }

    fn finish_probe(
        &self,
        probe: &DueMonitorProbe,
        state: RuntimeMonitorState,
    ) -> RuntimeHostResult<MonitorUpdate> {
        let current = self.instance_status(&probe.instance_alias)?;
        let applied = self.configuration_version(&probe.instance_alias)
            == probe.configuration_version
            && current.policy() == Some(&probe.policy)
            && current.state() == Some(&probe.state);
        let status = if applied {
            RuntimeMonitorInstanceStatus::configured(
                &probe.instance_alias,
                probe.policy.clone(),
                state.clone(),
            )
            .map_err(|_| invalid_monitor_record("finish_monitor_probe"))?
        } else {
            current
        };
        self.update(
            RuntimeMonitorChangeKind::Probe,
            status,
            applied,
            Some(probe.configuration_version),
            Some(state),
        )
    }

    fn update(
        &self,
        kind: RuntimeMonitorChangeKind,
        status: RuntimeMonitorInstanceStatus,
        applied: bool,
        probe_configuration_version: Option<u64>,
        probe_state: Option<RuntimeMonitorState>,
    ) -> RuntimeHostResult<MonitorUpdate> {
        let revision = self
            .revision
            .checked_add(u64::from(applied))
            .ok_or_else(|| monitor_error("monitor_revision_overflow", "prepare_monitor_update"))?;
        let configuration_version = if applied && kind != RuntimeMonitorChangeKind::Probe {
            revision
        } else {
            self.configuration_version(status.instance_alias())
        };
        let fact = RuntimeStateFact::MonitorChanged {
            owner_epoch: self.owner_epoch,
            change: Box::new(RuntimeMonitorChange {
                kind,
                previous_revision: self.revision,
                revision,
                configuration_version,
                applied,
                status: status.clone(),
                probe_configuration_version,
                probe_state,
            }),
        };
        fact.validate()
            .map_err(|_| invalid_monitor_record("prepare_monitor_update"))?;
        Ok(MonitorUpdate {
            changed: applied,
            fact,
        })
    }

    /// Apply only the state embedded in a committed native event.
    pub(crate) fn apply(&mut self, event: &PersistedEvent) -> RuntimeHostResult<()> {
        let Some(fact) = event.payload().runtime_state() else {
            return Ok(());
        };
        if matches!(fact, RuntimeStateFact::Observed { .. }) {
            return Ok(());
        }
        if event.origin().module() != OriginModule::Runtime
            || event.origin().source() != EventSource::Runtime
            || event.origin().actor() != EventActor::Runtime
            || event.sequence() <= self.last_sequence
        {
            return Err(invalid_monitor_record("replay_monitor_event"));
        }
        fact.validate()
            .map_err(|_| invalid_monitor_record("replay_monitor_event"))?;
        match fact {
            RuntimeStateFact::MonitorImported { import, .. } => {
                if self.imported.is_some() || self.revision != 0 {
                    return Err(monitor_error(
                        "monitor_import_conflict",
                        "replay_monitor_event",
                    ));
                }
                for status in &import.instances {
                    self.require_allowed(status.instance_alias())?;
                    self.configuration_versions
                        .insert(status.instance_alias().to_owned(), import.revision);
                    self.monitors
                        .insert(status.instance_alias().to_owned(), status.clone());
                }
                self.revision = import.revision;
                self.imported = Some(import.clone());
            }
            RuntimeStateFact::MonitorChanged { change, .. } => {
                if self.imported.is_none() || self.revision != change.previous_revision {
                    return Err(invalid_monitor_record("replay_monitor_revision"));
                }
                let alias = change.status.instance_alias();
                self.require_allowed(alias)?;
                let current_version = self.configuration_version(alias);
                if (!change.applied || change.kind == RuntimeMonitorChangeKind::Probe)
                    && current_version != change.configuration_version
                {
                    return Err(invalid_monitor_record("replay_monitor_configuration"));
                }
                if !change.applied && self.instance_status(alias)? != change.status {
                    return Err(invalid_monitor_record("replay_monitor_unchanged_state"));
                }
                if change.applied {
                    if change.status.policy().is_some() {
                        self.monitors
                            .insert(alias.to_owned(), change.status.clone());
                    } else {
                        self.monitors.remove(alias);
                    }
                    self.configuration_versions
                        .insert(alias.to_owned(), change.configuration_version);
                }
                self.revision = change.revision;
            }
            RuntimeStateFact::Observed { .. } => unreachable!(),
        }
        self.last_sequence = event.sequence();
        Ok(())
    }

    fn configuration_version(&self, alias: &str) -> u64 {
        self.configuration_versions.get(alias).copied().unwrap_or(0)
    }

    fn instance_status(&self, alias: &str) -> RuntimeHostResult<RuntimeMonitorInstanceStatus> {
        self.require_allowed(alias)?;
        self.monitors
            .get(alias)
            .cloned()
            .map_or_else(|| RuntimeMonitorInstanceStatus::unconfigured(alias), Ok)
            .map_err(|_| invalid_monitor_record("read_monitor_status"))
    }

    fn require_allowed(&self, instance_alias: &str) -> RuntimeHostResult<()> {
        if !self.allowed_aliases.contains(instance_alias) {
            return Err(monitor_error(
                "monitor_instance_unknown",
                "validate_monitor_instance",
            ));
        }
        Ok(())
    }
}

fn read_legacy_snapshot(
    state_root: &Path,
    allowed_aliases: &BTreeSet<String>,
) -> RuntimeHostResult<RuntimeMonitorImport> {
    let file = match File::open(state_root.join(MONITOR_FILE_NAME)) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RuntimeMonitorImport {
                source: None,
                revision: 0,
                instances: Vec::new(),
            });
        }
        Err(_) => {
            return Err(monitor_error(
                "monitor_file_open_failed",
                "read_monitor_registry",
            ));
        }
    };
    let length = file
        .metadata()
        .map_err(|_| monitor_error("monitor_metadata_failed", "read_monitor_registry"))?
        .len();
    if length > MAX_MONITOR_IMPORT_BYTES {
        return Err(monitor_error(
            "monitor_journal_too_large",
            "read_monitor_registry",
        ));
    }
    let mut content = Vec::new();
    file.take(MAX_MONITOR_IMPORT_BYTES + 1)
        .read_to_end(&mut content)
        .map_err(|_| monitor_error("monitor_file_read_failed", "read_monitor_registry"))?;
    if content.len() as u64 != length {
        return Err(monitor_error(
            "monitor_source_changed",
            "read_monitor_registry",
        ));
    }
    let source = Some(RuntimeMonitorJournalSource {
        sha256: format!("{:x}", Sha256::digest(&content)),
        length,
    });
    if content.is_empty() {
        return Ok(RuntimeMonitorImport {
            source,
            revision: 0,
            instances: Vec::new(),
        });
    }
    if content.last() != Some(&b'\n') {
        return Err(invalid_monitor_record("read_monitor_registry"));
    }
    let content = std::str::from_utf8(&content)
        .map_err(|_| invalid_monitor_record("read_monitor_registry"))?;
    let mut previous_revision = 0_u64;
    let mut last = None;
    for line in content.lines().filter(|line| !line.trim().is_empty()) {
        let record = serde_json::from_str::<MonitorRecord>(line)
            .map_err(|_| invalid_monitor_record("read_monitor_registry"))?;
        validate_record(&record, allowed_aliases)?;
        if previous_revision.checked_add(1) != Some(record.revision) {
            return Err(invalid_monitor_record("validate_monitor_registry"));
        }
        previous_revision = record.revision;
        last = Some(record);
    }
    let record = last.ok_or_else(|| invalid_monitor_record("read_monitor_registry"))?;
    Ok(RuntimeMonitorImport {
        source,
        revision: record.revision,
        instances: record.monitors,
    })
}

fn validate_record(
    record: &MonitorRecord,
    allowed_aliases: &BTreeSet<String>,
) -> RuntimeHostResult<()> {
    if record.schema_version != MONITOR_SCHEMA_VERSION || record.revision == 0 {
        return Err(invalid_monitor_record("validate_monitor_registry"));
    }
    let mut previous_alias = None;
    let mut aliases = BTreeSet::new();
    for status in &record.monitors {
        status
            .validate()
            .map_err(|_| invalid_monitor_record("validate_monitor_registry"))?;
        let alias = status.instance_alias();
        if status.policy().is_none()
            || !allowed_aliases.contains(alias)
            || !aliases.insert(alias)
            || previous_alias.is_some_and(|previous| previous >= alias)
        {
            return Err(invalid_monitor_record("validate_monitor_registry"));
        }
        previous_alias = Some(alias);
    }
    Ok(())
}

fn monitor_error(code: &'static str, operation: &'static str) -> RuntimeHostError {
    RuntimeHostError::fatal(code, operation, RuntimeErrorCode::RuntimeFatal)
}

fn monitor_ledger_error(operation: &'static str) -> RuntimeHostError {
    RuntimeHostError::fatal(
        "monitor_ledger_failure",
        operation,
        RuntimeErrorCode::LedgerFailure,
    )
}

fn invalid_monitor_record(operation: &'static str) -> RuntimeHostError {
    monitor_error("monitor_record_invalid", operation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_contract::{
        MonitorDiagnosis, MonitorDisposition, MonitorObservation, MonitorPayloadDraft,
    };
    use actingcommand_ledger::GlobalLedgerConfig;
    use std::sync::Arc;
    use tempfile::TempDir;

    #[test]
    fn monitor_registry_recovers_ledger_state_and_preserves_import_source() {
        let root = TempDir::new().expect("tempdir");
        let events = RuntimeEvents::new(
            b"monitor-specification-salt",
            Arc::new(crate::SystemRuntimeClock::new()),
        )
        .unwrap();
        let owner_epoch = *events.issuer().mint_owner_epoch().unwrap().transport();
        let legacy = MonitorRecord {
            schema_version: MONITOR_SCHEMA_VERSION.to_owned(),
            revision: 1,
            monitors: vec![
                RuntimeMonitorInstanceStatus::configured(
                    "node.a",
                    RuntimeMonitorPolicy::new(2_000, "campaign", false).unwrap(),
                    RuntimeMonitorState::scheduled(5).unwrap(),
                )
                .unwrap(),
            ],
        };
        let mut original = serde_json::to_vec(&legacy).unwrap();
        original.push(b'\n');
        std::fs::write(root.path().join(MONITOR_FILE_NAME), &original).unwrap();
        let ledger = GlobalLedger::open(GlobalLedgerConfig::new(
            root.path().join("ledger"),
            "monitor-first",
        ))
        .unwrap();
        let mut registry = MonitorRegistry::open(
            root.path(),
            ["node.a".to_owned()],
            owner_epoch,
            &ledger,
            &events,
        )
        .unwrap();
        let policy = RuntimeMonitorPolicy::new(1_000, "home", false).unwrap();
        for (now, changed) in [(10, true), (20, false)] {
            let update = registry
                .prepare_configure("node.a", policy.clone(), now)
                .unwrap();
            assert_eq!(update.changed, changed);
            let before = registry.revision;
            let draft = events
                .draft(
                    EventSeverity::Info,
                    EventSource::Runtime,
                    OriginModule::Runtime,
                    EventActor::Runtime,
                    events.system_links().unwrap(),
                    CommandPayloadDraft::validated_runtime_state(
                        EventAction::MonitorConfigure,
                        if changed {
                            EffectDisposition::Performed
                        } else {
                            EffectDisposition::NotPerformed
                        },
                        update.fact,
                        AuditInput::new(),
                    ),
                )
                .unwrap();
            let event = ledger.append(events.sanitize(draft).unwrap()).unwrap();
            assert_eq!(
                registry.revision, before,
                "preparation and append do not replace the projection"
            );
            registry.apply(&event).unwrap();
            assert_eq!(registry.revision, before + u64::from(changed));
        }
        assert_eq!(
            std::fs::read(root.path().join(MONITOR_FILE_NAME)).unwrap(),
            original
        );
        drop(registry);
        ledger.close().unwrap();

        let ledger = GlobalLedger::open(GlobalLedgerConfig::new(
            root.path().join("ledger"),
            "monitor-reopened",
        ))
        .unwrap();
        let next_epoch = *events.issuer().mint_owner_epoch().unwrap().transport();
        let mut reopened = MonitorRegistry::open(
            root.path(),
            ["node.a".to_owned()],
            next_epoch,
            &ledger,
            &events,
        )
        .unwrap();
        assert_eq!(
            reopened.status(next_epoch).unwrap().instances()[0].policy(),
            Some(&policy)
        );
        for changed in [true, false] {
            let update = reopened.prepare_clear("node.a").unwrap();
            assert_eq!(update.changed, changed);
            let before = reopened.revision;
            let draft = events
                .draft(
                    EventSeverity::Info,
                    EventSource::Runtime,
                    OriginModule::Runtime,
                    EventActor::Runtime,
                    events.system_links().unwrap(),
                    CommandPayloadDraft::validated_runtime_state(
                        EventAction::MonitorClear,
                        if changed {
                            EffectDisposition::Performed
                        } else {
                            EffectDisposition::NotPerformed
                        },
                        update.fact,
                        AuditInput::new(),
                    ),
                )
                .unwrap();
            let event = ledger.append(events.sanitize(draft).unwrap()).unwrap();
            reopened.apply(&event).unwrap();
            assert_eq!(reopened.revision, before + u64::from(changed));
        }
        assert_eq!(
            std::fs::read(root.path().join(MONITOR_FILE_NAME)).unwrap(),
            original
        );
        let imports = ledger
            .query(EventQuery::default())
            .unwrap()
            .into_iter()
            .filter(|event| {
                matches!(
                    event.payload().runtime_state(),
                    Some(RuntimeStateFact::MonitorImported { .. })
                )
            })
            .count();
        assert_eq!(imports, 1, "recovery reuses the original import marker");
        original.push(b'\n');
        std::fs::write(root.path().join(MONITOR_FILE_NAME), &original).unwrap();
        assert_eq!(
            MonitorRegistry::open(
                root.path(),
                ["node.a".to_owned()],
                next_epoch,
                &ledger,
                &events
            )
            .err()
            .unwrap()
            .code(),
            "monitor_import_source_conflict"
        );
        ledger.close().unwrap();
    }

    #[test]
    fn monitor_registry_rejects_corrupt_or_unknown_persisted_state() {
        let root = TempDir::new().unwrap();
        let events = RuntimeEvents::new(
            b"monitor-specification-salt",
            Arc::new(crate::SystemRuntimeClock::new()),
        )
        .unwrap();
        let owner_epoch = *events.issuer().mint_owner_epoch().unwrap().transport();
        let ledger = GlobalLedger::open(GlobalLedgerConfig::new(
            root.path().join("ledger"),
            "monitor-corruption",
        ))
        .unwrap();
        let unknown = MonitorRecord {
            schema_version: MONITOR_SCHEMA_VERSION.to_owned(),
            revision: 1,
            monitors: vec![
                RuntimeMonitorInstanceStatus::configured(
                    "node.unknown",
                    RuntimeMonitorPolicy::new(1_000, "home", false).unwrap(),
                    RuntimeMonitorState::scheduled(10).unwrap(),
                )
                .unwrap(),
            ],
        };
        let mut unknown_bytes = serde_json::to_vec(&unknown).unwrap();
        let truncated = unknown_bytes.clone();
        unknown_bytes.push(b'\n');
        for bytes in [b"not-json\n".to_vec(), truncated, unknown_bytes] {
            std::fs::write(root.path().join(MONITOR_FILE_NAME), &bytes).unwrap();
            assert_eq!(
                MonitorRegistry::open(
                    root.path(),
                    ["node.a".to_owned()],
                    owner_epoch,
                    &ledger,
                    &events
                )
                .err()
                .unwrap()
                .code(),
                "monitor_record_invalid"
            );
            assert_eq!(
                std::fs::read(root.path().join(MONITOR_FILE_NAME)).unwrap(),
                bytes
            );
        }
        let recorded = ledger.query(EventQuery::default()).unwrap();
        assert!(recorded.iter().all(|event| !matches!(
            event.payload().runtime_state(),
            Some(RuntimeStateFact::MonitorImported { .. })
        )));
        assert_eq!(recorded.len(), 3);
        for event in &recorded {
            let actingcommand_contract::EventPayload::Runtime(
                actingcommand_contract::RuntimePayload::Failed(failure),
            ) = event.payload()
            else {
                panic!("committed monitor restore failure");
            };
            let detail = failure.detail().expect("original monitor cause");
            assert_eq!(detail.category(), "monitor_registry");
            assert_eq!(detail.stage(), "runtime.monitor.restore");
            assert_eq!(detail.message(), "monitor_record_invalid");
            assert_eq!(
                detail.declared_sensitivity(),
                actingcommand_contract::Sensitivity::Sensitive
            );
            assert_eq!(
                event.payload().sensitivity(),
                actingcommand_contract::Sensitivity::Sensitive
            );
            let public = serde_json::to_string(&event.payload().public_projection()).unwrap();
            assert!(!public.contains("monitor_record_invalid"));
        }
        ledger.close().unwrap();
    }

    #[test]
    fn due_probe_completion_does_not_overwrite_a_newer_policy() {
        let root = TempDir::new().unwrap();
        let events = RuntimeEvents::new(
            b"monitor-specification-salt",
            Arc::new(crate::SystemRuntimeClock::new()),
        )
        .unwrap();
        let owner_epoch = *events.issuer().mint_owner_epoch().unwrap().transport();
        let ledger = GlobalLedger::open(GlobalLedgerConfig::new(
            root.path().join("ledger"),
            "monitor-probe",
        ))
        .unwrap();
        let mut registry = MonitorRegistry::open(
            root.path(),
            ["node.a".to_owned()],
            owner_epoch,
            &ledger,
            &events,
        )
        .unwrap();
        let first = RuntimeMonitorPolicy::new(1_000, "home", false).unwrap();
        let replacement = RuntimeMonitorPolicy::new(2_000, "campaign", false).unwrap();
        let mut first_probe = None;
        for (policy, now) in [(first, 10), (replacement.clone(), 20)] {
            let update = registry.prepare_configure("node.a", policy, now).unwrap();
            let draft = events
                .draft(
                    EventSeverity::Info,
                    EventSource::Runtime,
                    OriginModule::Runtime,
                    EventActor::Runtime,
                    events.system_links().unwrap(),
                    CommandPayloadDraft::validated_runtime_state(
                        EventAction::MonitorConfigure,
                        EffectDisposition::Performed,
                        update.fact,
                        AuditInput::new(),
                    ),
                )
                .unwrap();
            let event = ledger.append(events.sanitize(draft).unwrap()).unwrap();
            registry.apply(&event).unwrap();
            if now == 10 {
                assert!(registry.due(9, 1).unwrap().is_empty());
                first_probe = Some(registry.due(10, 1).unwrap().remove(0));
            }
        }
        let replacement_probe = registry.due(20, 1).unwrap().remove(0);
        let decision =
            MonitorDecision::new(MonitorDiagnosis::Healthy, MonitorDisposition::Healthy, None)
                .unwrap();
        for (probe, start, end, applied, page) in [
            (first_probe.unwrap(), 10, 11, false, "home"),
            (replacement_probe.clone(), 20, 25, true, "campaign"),
        ] {
            let update = registry
                .prepare_completion(&probe, start, end, decision.clone())
                .unwrap();
            assert_eq!(update.changed, applied);
            let draft = events
                .draft(
                    EventSeverity::Info,
                    EventSource::Runtime,
                    OriginModule::Runtime,
                    EventActor::Runtime,
                    events.system_links().unwrap(),
                    MonitorPayloadDraft::completed(
                        EffectDisposition::Performed,
                        MonitorObservation::new(
                            MonitorDiagnosis::Healthy,
                            page,
                            Some(page.to_owned()),
                        )
                        .unwrap(),
                        decision.clone(),
                        AuditInput::new(),
                    )
                    .with_runtime_state(update.fact)
                    .unwrap(),
                )
                .unwrap();
            let event = ledger.append(events.sanitize(draft).unwrap()).unwrap();
            registry.apply(&event).unwrap();
        }
        let status = registry.status(owner_epoch).unwrap();
        assert_eq!(status.instances()[0].policy(), Some(&replacement));
        assert_eq!(status.instances()[0].state().unwrap().run_count(), 1);
        // A clear/configure cycle cannot revive a probe from that same policy and source time.
        for clear in [true, false] {
            let update = if clear {
                registry.prepare_clear("node.a").unwrap()
            } else {
                registry
                    .prepare_configure("node.a", replacement.clone(), 20)
                    .unwrap()
            };
            let draft = events
                .draft(
                    EventSeverity::Info,
                    EventSource::Runtime,
                    OriginModule::Runtime,
                    EventActor::Runtime,
                    events.system_links().unwrap(),
                    CommandPayloadDraft::validated_runtime_state(
                        if clear {
                            EventAction::MonitorClear
                        } else {
                            EventAction::MonitorConfigure
                        },
                        EffectDisposition::Performed,
                        update.fact,
                        AuditInput::new(),
                    ),
                )
                .unwrap();
            let event = ledger.append(events.sanitize(draft).unwrap()).unwrap();
            registry.apply(&event).unwrap();
        }
        let stale = registry
            .prepare_failure(&replacement_probe, 20, 25, RuntimeErrorCode::RuntimeFatal)
            .unwrap();
        assert!(!stale.changed);
        let RuntimeStateFact::MonitorChanged { change, .. } = stale.fact else {
            panic!("monitor fact");
        };
        assert!(change.probe_configuration_version.unwrap() < change.configuration_version);
        assert_eq!(change.status.state().unwrap().run_count(), 0);
        assert!(change.probe_state.as_ref().unwrap().last_error().is_some());
        ledger.close().unwrap();
    }

    #[test]
    fn due_probe_batches_are_bounded_and_deterministic() {
        let root = TempDir::new().unwrap();
        let events = RuntimeEvents::new(
            b"monitor-specification-salt",
            Arc::new(crate::SystemRuntimeClock::new()),
        )
        .unwrap();
        let owner_epoch = *events.issuer().mint_owner_epoch().unwrap().transport();
        let ledger = GlobalLedger::open(GlobalLedgerConfig::new(
            root.path().join("ledger"),
            "monitor-batch",
        ))
        .unwrap();
        let aliases = (0..20)
            .map(|index| format!("instance-{index:02}"))
            .collect::<Vec<_>>();
        let mut registry =
            MonitorRegistry::open(root.path(), aliases.clone(), owner_epoch, &ledger, &events)
                .unwrap();
        let policy = RuntimeMonitorPolicy::new(100, "home", false).unwrap();
        for alias in &aliases {
            let update = registry
                .prepare_configure(alias, policy.clone(), 10)
                .unwrap();
            let draft = events
                .draft(
                    EventSeverity::Info,
                    EventSource::Runtime,
                    OriginModule::Runtime,
                    EventActor::Runtime,
                    events.system_links().unwrap(),
                    CommandPayloadDraft::validated_runtime_state(
                        EventAction::MonitorConfigure,
                        EffectDisposition::Performed,
                        update.fact,
                        AuditInput::new(),
                    ),
                )
                .unwrap();
            let event = ledger.append(events.sanitize(draft).unwrap()).unwrap();
            registry.apply(&event).unwrap();
        }
        let due = registry.due(10, 16).unwrap();
        assert_eq!(due.len(), 16);
        assert_eq!(
            due.iter()
                .map(|probe| probe.instance_alias.as_str())
                .collect::<Vec<_>>(),
            aliases[..16]
        );
        assert!(
            !root.path().join(MONITOR_FILE_NAME).exists(),
            "absent source remains absent after import and updates"
        );
        ledger.close().unwrap();
    }
}
