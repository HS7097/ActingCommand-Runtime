// SPDX-License-Identifier: AGPL-3.0-only

use crate::codes::{HostCode, HostLocation};
use crate::events::RuntimeEvents;
use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::outcome::{Code, Location};
use actingcommand_contract::{
    AuditInput, CommandPayloadDraft, EffectDisposition, EventAction, EventActor, EventQuery,
    EventSeverity, EventSource, MAX_MONITOR_IMPORT_BYTES, MAX_RUNTIME_OBSERVED_INSTANCES,
    MonitorDecision, OriginModule, OwnerEpoch, RuntimeErrorCode, RuntimeMonitorChange,
    RuntimeMonitorChangeKind, RuntimeMonitorImport, RuntimeMonitorInstanceStatus,
    RuntimeMonitorJournalSource, RuntimeMonitorPolicy, RuntimeMonitorRegistryStatus,
    RuntimeMonitorState, RuntimeStateFact,
};
use actingcommand_ledger::{GlobalLedger, LedgerIndexRole, PersistedEvent};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
use std::path::Path;

pub(crate) const MONITOR_FILE_NAME: &str = "monitor.journal";
const MONITOR_SCHEMA_VERSION: &str = "actingcommand.runtime-monitor.v1";
/// Workflow #422 LN-4: the `state` keys of the registry (`contracts/ledger-observer.md` §11.1):
/// the import, and each alias's latest applied change (or the import that brought it).
const MONITOR_IMPORT_KEY: &str = "monitor_import";
const MONITOR_KEY_PREFIX: &str = "monitor/";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MonitorRecord {
    schema_version: String,
    revision: u64,
    monitors: Vec<RuntimeMonitorInstanceStatus>,
}

/// Workflow #422 LN-4, tests: what a registry holds (`MonitorRegistry::state_for_test`).
#[cfg(test)]
pub(crate) type MonitorStateForTest = (
    u64,
    Option<RuntimeMonitorImport>,
    BTreeMap<String, RuntimeMonitorInstanceStatus>,
    BTreeMap<String, u64>,
);

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
        let recovered = Self::recover(state_root, allowed_aliases, owner_epoch, ledger, events);
        Self::restored(recovered, ledger, events)
    }

    /// Workflow #422 LN-4 (LEAN-L model §1.2 H-c, §3.2 step 5): the start's load from the tagged
    /// records instead of the replay of every Runtime record. The `monitor_import` row names the
    /// import; each `monitor/<alias>` row names the alias's latest applied change, or the import
    /// that brought it. The import is applied first, then each alias's change; the registry's
    /// revision is the largest the records carry. The journal file is checked against the import
    /// as the replay checks it. A failure is recorded as the replay's is.
    pub(crate) fn load(
        state_root: &Path,
        allowed_aliases: impl IntoIterator<Item = String>,
        owner_epoch: OwnerEpoch,
        ledger: &GlobalLedger,
        events: &RuntimeEvents,
    ) -> RuntimeHostResult<Self> {
        let loaded = Self::load_tagged(state_root, allowed_aliases, owner_epoch, ledger);
        Self::restored(loaded, ledger, events)
    }

    /// The error of a failed restore, recorded as a fatal `runtime.failed` unless the ledger
    /// itself failed.
    fn restored(
        restored: RuntimeHostResult<Self>,
        ledger: &GlobalLedger,
        events: &RuntimeEvents,
    ) -> RuntimeHostResult<Self> {
        match restored {
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
                            error.record_outcome(),
                            actingcommand_contract::DiagnosticDetailDraft::new(
                                "monitor_registry",
                                HostLocation::RuntimeMonitorRestore,
                                "runtime",
                                error.operation(),
                            ),
                            AuditInput::new(),
                        ),
                    )?;
                    let recorded = ledger.append(events.sanitize(draft)?).map_err(|_| {
                        monitor_ledger_error(HostLocation::RecordMonitorRestoreFailure)
                    })?;
                    let _ = error
                        .diagnostics()
                        .recorded_event()
                        .set(*recorded.event_id());
                }
                Err(error)
            }
        }
    }

    /// An empty registry of `allowed_aliases`.
    fn empty(
        allowed_aliases: impl IntoIterator<Item = String>,
        owner_epoch: OwnerEpoch,
    ) -> RuntimeHostResult<Self> {
        let allowed_aliases = allowed_aliases.into_iter().collect::<BTreeSet<_>>();
        if allowed_aliases.len() > MAX_RUNTIME_OBSERVED_INSTANCES {
            return Err(monitor_error(
                HostCode::InvalidMonitorInstanceRegistry,
                HostLocation::OpenMonitorRegistry,
            ));
        }
        Ok(Self {
            owner_epoch,
            revision: 0,
            last_sequence: 0,
            imported: None,
            monitors: BTreeMap::new(),
            configuration_versions: BTreeMap::new(),
            allowed_aliases,
        })
    }

    fn load_tagged(
        state_root: &Path,
        allowed_aliases: impl IntoIterator<Item = String>,
        owner_epoch: OwnerEpoch,
        ledger: &GlobalLedger,
    ) -> RuntimeHostResult<Self> {
        let mut registry = Self::empty(allowed_aliases, owner_epoch)?;
        let rows = crate::tagged_state::index_records(
            ledger,
            LedgerIndexRole::State,
            &[MONITOR_IMPORT_KEY, MONITOR_KEY_PREFIX],
        )?;
        let import = rows
            .records
            .iter()
            .find(|record| record.key == MONITOR_IMPORT_KEY)
            .ok_or_else(|| invalid_monitor_record(HostLocation::OpenMonitorRegistry))?;
        registry.apply(&import.event)?;
        for record in &rows.records {
            match record.event.payload().runtime_state() {
                // The import's own rows: its aliases are applied with it.
                Some(RuntimeStateFact::MonitorImported { .. })
                    if record.event.sequence() == import.event.sequence() => {}
                Some(RuntimeStateFact::MonitorChanged { change, .. }) if change.applied => {
                    registry.load_change(&record.event, change)?;
                }
                _ => return Err(invalid_monitor_record(HostLocation::ReplayMonitorEvent)),
            }
        }
        registry.last_sequence = rows.through_sequence;
        registry.check_import_source(state_root)?;
        Ok(registry)
    }

    /// Workflow #422 LN-4: one alias's latest applied change, loaded with the checks the replay
    /// gives a record.
    fn load_change(
        &mut self,
        event: &PersistedEvent,
        change: &RuntimeMonitorChange,
    ) -> RuntimeHostResult<()> {
        if event.origin().module() != OriginModule::Runtime
            || event.origin().source() != EventSource::Runtime
            || event.origin().actor() != EventActor::Runtime
        {
            return Err(invalid_monitor_record(HostLocation::ReplayMonitorEvent));
        }
        change
            .validate()
            .map_err(|_| invalid_monitor_record(HostLocation::ReplayMonitorEvent))?;
        let alias = change.status.instance_alias();
        self.require_allowed(alias)?;
        if change.status.policy().is_some() {
            self.monitors
                .insert(alias.to_owned(), change.status.clone());
        } else {
            self.monitors.remove(alias);
        }
        self.configuration_versions
            .insert(alias.to_owned(), change.configuration_version);
        self.revision = self.revision.max(change.revision);
        Ok(())
    }

    /// The import must still match the journal file it was read from.
    fn check_import_source(&self, state_root: &Path) -> RuntimeHostResult<()> {
        let original = read_legacy_snapshot(state_root, &self.allowed_aliases)?;
        if self.imported.as_ref() != Some(&original) {
            return Err(monitor_error(
                HostCode::MonitorImportSourceConflict,
                HostLocation::OpenMonitorRegistry,
            ));
        }
        Ok(())
    }

    /// Workflow #422 LN-4 (LEAN-L model §3.6; `contracts/ledger-observer.md` §11.6): seeds the
    /// registry's tagged records once, on a start without the marker. An import the index already
    /// names came with the index rules, and so did every change after it: nothing is written.
    /// Otherwise the import is recorded again (the same import changes nothing when replayed),
    /// then one applied change with the complete state of every alias whose state moved since the
    /// import, so each alias's row names a record that carries its state. Returns the records
    /// written.
    pub(crate) fn seed(
        &mut self,
        ledger: &GlobalLedger,
        events: &RuntimeEvents,
    ) -> RuntimeHostResult<usize> {
        let tagged = crate::tagged_state::index_records(
            ledger,
            LedgerIndexRole::State,
            &[MONITOR_IMPORT_KEY],
        )?;
        if !tagged.records.is_empty() {
            return Ok(0);
        }
        let import = self
            .imported
            .clone()
            .ok_or_else(|| invalid_monitor_record(HostLocation::ImportMonitorRegistry))?;
        let fact = RuntimeStateFact::MonitorImported {
            owner_epoch: self.owner_epoch,
            import: import.clone(),
        };
        let event = append_runtime_state(
            ledger,
            events,
            EventAction::RuntimeAction,
            EffectDisposition::NotPerformed,
            fact,
            HostLocation::ImportMonitorRegistry,
        )?;
        self.apply(&event)?;
        let mut written = 1;
        for alias in self.allowed_aliases.clone() {
            let status = self.instance_status(&alias)?;
            let version = self.configuration_version(&alias);
            let unchanged = match import
                .instances
                .iter()
                .find(|imported| imported.instance_alias() == alias)
            {
                Some(imported) => *imported == status && version == import.revision,
                None => status.policy().is_none() && version == 0,
            };
            if unchanged {
                continue;
            }
            let (kind, action) = if status.policy().is_some() {
                (
                    RuntimeMonitorChangeKind::Configure,
                    EventAction::MonitorConfigure,
                )
            } else {
                (RuntimeMonitorChangeKind::Clear, EventAction::MonitorClear)
            };
            let update = self.update(kind, status, true, None, None)?;
            let event = append_runtime_state(
                ledger,
                events,
                action,
                EffectDisposition::Performed,
                update.fact,
                HostLocation::PrepareMonitorUpdate,
            )?;
            self.apply(&event)?;
            written += 1;
        }
        Ok(written)
    }

    /// Workflow #422 LN-4, tests: the revision, the import, the configured aliases and the
    /// configuration versions.
    #[cfg(test)]
    pub(crate) fn state_for_test(&self) -> MonitorStateForTest {
        (
            self.revision,
            self.imported.clone(),
            self.monitors.clone(),
            self.configuration_versions.clone(),
        )
    }

    fn recover(
        state_root: &Path,
        allowed_aliases: impl IntoIterator<Item = String>,
        owner_epoch: OwnerEpoch,
        ledger: &GlobalLedger,
        events: &RuntimeEvents,
    ) -> RuntimeHostResult<Self> {
        let mut registry = Self::empty(allowed_aliases, owner_epoch)?;
        registry.replay_runtime_records(ledger)?;
        let original = read_legacy_snapshot(state_root, &registry.allowed_aliases)?;

        if let Some(imported) = &registry.imported {
            if imported != &original {
                return Err(monitor_error(
                    HostCode::MonitorImportSourceConflict,
                    HostLocation::OpenMonitorRegistry,
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
                .map_err(|_| monitor_ledger_error(HostLocation::ImportMonitorRegistry))?;
            registry.apply(&event)?;
        }
        Ok(registry)
    }

    /// Applies every Runtime record of the ledger, in pages through one position.
    fn replay_runtime_records(&mut self, ledger: &GlobalLedger) -> RuntimeHostResult<()> {
        let through = ledger
            .latest_sequence()
            .map_err(|_| monitor_ledger_error(HostLocation::ReadMonitorRegistry))?;
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
                .map_err(|_| monitor_ledger_error(HostLocation::ReadMonitorRegistry))?;
            if page.is_empty() {
                break;
            }
            for event in page {
                after = event.sequence();
                self.apply(&event)?;
            }
        }
        Ok(())
    }

    /// Workflow #422 LN-4, tests: the registry today's replay of every Runtime record rebuilds,
    /// without its import step: the reference a loaded registry is compared with.
    #[cfg(test)]
    pub(crate) fn replayed_for_test(
        allowed_aliases: impl IntoIterator<Item = String>,
        owner_epoch: OwnerEpoch,
        ledger: &GlobalLedger,
    ) -> RuntimeHostResult<Self> {
        let mut registry = Self::empty(allowed_aliases, owner_epoch)?;
        registry.replay_runtime_records(ledger)?;
        Ok(registry)
    }

    #[cfg(test)]
    pub(crate) fn aliases_for_test(&self) -> Vec<String> {
        self.allowed_aliases.iter().cloned().collect()
    }

    pub(crate) fn prepare_configure(
        &self,
        instance_alias: &str,
        policy: RuntimeMonitorPolicy,
        now_unix_ms: u64,
    ) -> RuntimeHostResult<MonitorUpdate> {
        self.require_allowed(instance_alias)?;
        policy.validate().map_err(|_| {
            monitor_error(
                HostCode::MonitorPolicyInvalid,
                HostLocation::ConfigureMonitorPolicy,
            )
        })?;
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
        let state = RuntimeMonitorState::scheduled(now_unix_ms).map_err(|_| {
            monitor_error(
                HostCode::MonitorStateInvalid,
                HostLocation::ConfigureMonitorPolicy,
            )
        })?;
        let status = RuntimeMonitorInstanceStatus::configured(instance_alias, policy, state)
            .map_err(|_| invalid_monitor_record(HostLocation::ConfigureMonitorPolicy))?;
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
            .map_err(|_| invalid_monitor_record(HostLocation::ClearMonitorPolicy))?;
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
            .map_err(|_| invalid_monitor_record(HostLocation::ReadMonitorStatus))
    }

    pub(crate) fn due(
        &self,
        now_unix_ms: u64,
        maximum: usize,
    ) -> RuntimeHostResult<Vec<DueMonitorProbe>> {
        if now_unix_ms == 0 || maximum == 0 {
            return Err(monitor_error(
                HostCode::MonitorDueQueryInvalid,
                HostLocation::ReadDueMonitors,
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
                        .ok_or_else(|| invalid_monitor_record(HostLocation::ReadDueMonitors))?,
                    state: status
                        .state()
                        .cloned()
                        .ok_or_else(|| invalid_monitor_record(HostLocation::ReadDueMonitors))?,
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
            .map_err(|_| {
                monitor_error(
                    HostCode::MonitorStateInvalid,
                    HostLocation::CompleteMonitorProbe,
                )
            })?;
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
            .map_err(|_| {
                monitor_error(
                    HostCode::MonitorStateInvalid,
                    HostLocation::FailMonitorProbe,
                )
            })?;
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
            .map_err(|_| invalid_monitor_record(HostLocation::FinishMonitorProbe))?
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
            .ok_or_else(|| {
                monitor_error(
                    HostCode::MonitorRevisionOverflow,
                    HostLocation::PrepareMonitorUpdate,
                )
            })?;
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
            .map_err(|_| invalid_monitor_record(HostLocation::PrepareMonitorUpdate))?;
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
            return Err(invalid_monitor_record(HostLocation::ReplayMonitorEvent));
        }
        fact.validate()
            .map_err(|_| invalid_monitor_record(HostLocation::ReplayMonitorEvent))?;
        match fact {
            RuntimeStateFact::MonitorImported { import, .. } => {
                // Workflow #422 LN-4 (§11.6): the seeding records the import again so the index
                // names it; the same import changes nothing.
                if self.imported.as_ref() == Some(import) {
                    self.last_sequence = event.sequence();
                    return Ok(());
                }
                if self.imported.is_some() || self.revision != 0 {
                    return Err(monitor_error(
                        HostCode::MonitorImportConflict,
                        HostLocation::ReplayMonitorEvent,
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
                    return Err(invalid_monitor_record(HostLocation::ReplayMonitorRevision));
                }
                let alias = change.status.instance_alias();
                self.require_allowed(alias)?;
                let current_version = self.configuration_version(alias);
                if (!change.applied || change.kind == RuntimeMonitorChangeKind::Probe)
                    && current_version != change.configuration_version
                {
                    return Err(invalid_monitor_record(
                        HostLocation::ReplayMonitorConfiguration,
                    ));
                }
                if !change.applied && self.instance_status(alias)? != change.status {
                    return Err(invalid_monitor_record(
                        HostLocation::ReplayMonitorUnchangedState,
                    ));
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
            .map_err(|_| invalid_monitor_record(HostLocation::ReadMonitorStatus))
    }

    fn require_allowed(&self, instance_alias: &str) -> RuntimeHostResult<()> {
        if !self.allowed_aliases.contains(instance_alias) {
            return Err(monitor_error(
                HostCode::MonitorInstanceUnknown,
                HostLocation::ValidateMonitorInstance,
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
                HostCode::MonitorFileOpenFailed,
                HostLocation::ReadMonitorRegistry,
            ));
        }
    };
    let length = file
        .metadata()
        .map_err(|_| {
            monitor_error(
                HostCode::MonitorMetadataFailed,
                HostLocation::ReadMonitorRegistry,
            )
        })?
        .len();
    if length > MAX_MONITOR_IMPORT_BYTES {
        return Err(monitor_error(
            HostCode::MonitorJournalTooLarge,
            HostLocation::ReadMonitorRegistry,
        ));
    }
    let mut content = Vec::new();
    file.take(MAX_MONITOR_IMPORT_BYTES + 1)
        .read_to_end(&mut content)
        .map_err(|_| {
            monitor_error(
                HostCode::MonitorFileReadFailed,
                HostLocation::ReadMonitorRegistry,
            )
        })?;
    if content.len() as u64 != length {
        return Err(monitor_error(
            HostCode::MonitorSourceChanged,
            HostLocation::ReadMonitorRegistry,
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
        return Err(invalid_monitor_record(HostLocation::ReadMonitorRegistry));
    }
    let content = std::str::from_utf8(&content)
        .map_err(|_| invalid_monitor_record(HostLocation::ReadMonitorRegistry))?;
    let mut previous_revision = 0_u64;
    let mut last = None;
    for line in content.lines().filter(|line| !line.trim().is_empty()) {
        let record = serde_json::from_str::<MonitorRecord>(line)
            .map_err(|_| invalid_monitor_record(HostLocation::ReadMonitorRegistry))?;
        validate_record(&record, allowed_aliases)?;
        if previous_revision.checked_add(1) != Some(record.revision) {
            return Err(invalid_monitor_record(
                HostLocation::ValidateMonitorRegistry,
            ));
        }
        previous_revision = record.revision;
        last = Some(record);
    }
    let record = last.ok_or_else(|| invalid_monitor_record(HostLocation::ReadMonitorRegistry))?;
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
        return Err(invalid_monitor_record(
            HostLocation::ValidateMonitorRegistry,
        ));
    }
    let mut previous_alias = None;
    let mut aliases = BTreeSet::new();
    for status in &record.monitors {
        status
            .validate()
            .map_err(|_| invalid_monitor_record(HostLocation::ValidateMonitorRegistry))?;
        let alias = status.instance_alias();
        if status.policy().is_none()
            || !allowed_aliases.contains(alias)
            || !aliases.insert(alias)
            || previous_alias.is_some_and(|previous| previous >= alias)
        {
            return Err(invalid_monitor_record(
                HostLocation::ValidateMonitorRegistry,
            ));
        }
        previous_alias = Some(alias);
    }
    Ok(())
}

fn monitor_error(code: impl Into<Code>, operation: impl Into<Location>) -> RuntimeHostError {
    RuntimeHostError::fatal(code, operation, RuntimeErrorCode::RuntimeHalted)
}

/// Workflow #422 LN-4: appends one registry record as the Runtime's own validated command.
fn append_runtime_state(
    ledger: &GlobalLedger,
    events: &RuntimeEvents,
    action: EventAction,
    effect: EffectDisposition,
    fact: RuntimeStateFact,
    operation: HostLocation,
) -> RuntimeHostResult<PersistedEvent> {
    let draft = events.draft(
        EventSeverity::Info,
        EventSource::Runtime,
        OriginModule::Runtime,
        EventActor::Runtime,
        events.system_links()?,
        CommandPayloadDraft::validated_runtime_state(action, effect, fact, AuditInput::new()),
    )?;
    ledger
        .append(events.sanitize(draft)?)
        .map_err(|_| monitor_ledger_error(operation))
}

fn monitor_ledger_error(operation: impl Into<Location>) -> RuntimeHostError {
    RuntimeHostError::fatal(
        HostCode::MonitorLedgerFailure,
        operation,
        RuntimeErrorCode::LedgerFailure,
    )
}

fn invalid_monitor_record(operation: impl Into<Location>) -> RuntimeHostError {
    monitor_error(HostCode::MonitorRecordInvalid, operation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_contract::{
        MonitorDiagnosis, MonitorDisposition, MonitorObservation, MonitorPayloadDraft,
    };
    use actingcommand_ledger::{GlobalLedgerConfig, GlobalLedgerError};
    use actingcommand_runtime_database::RuntimeDatabase;
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
        let database = Arc::new(
            RuntimeDatabase::open_with_initializer::<GlobalLedgerError>(
                root.path(),
                b"monitor-specification-salt",
                |_| Ok(()),
                |_| Ok(()),
            )
            .unwrap(),
        );
        let ledger = GlobalLedger::open_sqlite_candidate(
            GlobalLedgerConfig::new(root.path(), "monitor-first"),
            Arc::clone(&database),
        )
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

        let ledger = GlobalLedger::open_sqlite_candidate(
            GlobalLedgerConfig::new(root.path(), "monitor-reopened"),
            database,
        )
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
        let database = Arc::new(
            RuntimeDatabase::open_with_initializer::<GlobalLedgerError>(
                root.path(),
                b"monitor-specification-salt",
                |_| Ok(()),
                |_| Ok(()),
            )
            .unwrap(),
        );
        let ledger = GlobalLedger::open_sqlite_candidate(
            GlobalLedgerConfig::new(root.path(), "monitor-corruption"),
            database,
        )
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
            assert_eq!(detail.stage(), "runtime_monitor_restore");
            // Ledger revision 3: the original cause is the record's outcome; a detail carries
            // no message and no sensitivity, so a record without foreign text is internal.
            assert_eq!(failure.outcome().code().as_str(), "monitor_record_invalid");
            assert_eq!(
                event.payload().sensitivity(),
                actingcommand_contract::Sensitivity::Internal
            );
            // The public projection carries the outcome codes only.
            let actingcommand_contract::PublicEventPayload::Runtime(public) =
                event.payload().public_projection()
            else {
                panic!("runtime public projection");
            };
            assert_eq!(public.outcome(), Some(&failure.outcome().codes_only()));
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
        let database = Arc::new(
            RuntimeDatabase::open_with_initializer::<GlobalLedgerError>(
                root.path(),
                b"monitor-specification-salt",
                |_| Ok(()),
                |_| Ok(()),
            )
            .unwrap(),
        );
        let ledger = GlobalLedger::open_sqlite_candidate(
            GlobalLedgerConfig::new(root.path(), "monitor-probe"),
            database,
        )
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
            .prepare_failure(&replacement_probe, 20, 25, RuntimeErrorCode::RuntimeHalted)
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
        let database = Arc::new(
            RuntimeDatabase::open_with_initializer::<GlobalLedgerError>(
                root.path(),
                b"monitor-specification-salt",
                |_| Ok(()),
                |_| Ok(()),
            )
            .unwrap(),
        );
        let ledger = GlobalLedger::open_sqlite_candidate(
            GlobalLedgerConfig::new(root.path(), "monitor-batch"),
            database,
        )
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

    /// Workflow #422 LN-3: the sequence of the record the index row `monitor/<alias>` names.
    fn monitor_row(database: &RuntimeDatabase, alias: &str) -> Option<u64> {
        let connection = database
            .connection(actingcommand_ledger::LedgerLocation::LedgerIndexRows.location())
            .expect("database connection");
        let mut statement = connection
            .prepare("SELECT sequence FROM ledger_index WHERE role='state' AND key=?1")
            .expect("index read");
        let mut rows = statement
            .query_map([format!("monitor/{alias}")], |row| row.get::<_, i64>(0))
            .expect("index rows");
        // The ledger's ordered integer encoding.
        rows.next()
            .map(|row| (row.expect("an index row") as u64) ^ (1_u64 << 63))
    }

    // Workflow #422 LN-3, test row 1 for the monitor registry (LEAN-L §3.6; `ledger-observer.md`
    // §11.1): every change of an alias's monitor state writes one record with the alias's
    // complete state, and the index row `monitor/<alias>` names the latest: the import for each
    // alias it brings, then each applied change. A change that was not applied changed no state
    // and leaves the row where it was; an alias that never changed has no row.
    #[test]
    fn ln3_a_the_monitor_row_names_the_latest_change_of_each_alias() {
        let root = TempDir::new().unwrap();
        let events = RuntimeEvents::new(
            b"monitor-specification-salt",
            Arc::new(crate::SystemRuntimeClock::new()),
        )
        .unwrap();
        let owner_epoch = *events.issuer().mint_owner_epoch().unwrap().transport();
        let imported = RuntimeMonitorInstanceStatus::configured(
            "node.a",
            RuntimeMonitorPolicy::new(2_000, "campaign", false).unwrap(),
            RuntimeMonitorState::scheduled(5).unwrap(),
        )
        .unwrap();
        let mut journal = serde_json::to_vec(&MonitorRecord {
            schema_version: MONITOR_SCHEMA_VERSION.to_owned(),
            revision: 1,
            monitors: vec![imported.clone()],
        })
        .unwrap();
        journal.push(b'\n');
        std::fs::write(root.path().join(MONITOR_FILE_NAME), &journal).unwrap();
        let database = Arc::new(
            RuntimeDatabase::open_with_initializer::<GlobalLedgerError>(
                root.path(),
                b"monitor-specification-salt",
                |_| Ok(()),
                |_| Ok(()),
            )
            .unwrap(),
        );
        let ledger = GlobalLedger::open_sqlite_candidate(
            GlobalLedgerConfig::new(root.path(), "monitor-ln3"),
            Arc::clone(&database),
        )
        .unwrap();
        let mut registry = MonitorRegistry::open(
            root.path(),
            ["node.a".to_owned(), "node.b".to_owned()],
            owner_epoch,
            &ledger,
            &events,
        )
        .unwrap();
        let import = ledger
            .query(EventQuery::default())
            .unwrap()
            .into_iter()
            .find(|event| {
                matches!(
                    event.payload().runtime_state(),
                    Some(RuntimeStateFact::MonitorImported { import, .. })
                        if import.instances == vec![imported.clone()]
                )
            })
            .expect("the import record");

        let policy = RuntimeMonitorPolicy::new(1_000, "home", false).unwrap();
        let mut rows = BTreeMap::from([("node.a", Some(import.sequence())), ("node.b", None)]);
        for (alias, row) in &rows {
            assert_eq!(monitor_row(&database, alias), *row, "{alias}");
        }
        for (alias, clear, applied) in [
            ("node.b", false, true),
            ("node.b", false, false),
            ("node.a", true, true),
            ("node.a", true, false),
        ] {
            let update = if clear {
                registry.prepare_clear(alias).unwrap()
            } else {
                registry
                    .prepare_configure(alias, policy.clone(), 10)
                    .unwrap()
            };
            assert_eq!(update.changed, applied, "{alias}");
            let RuntimeStateFact::MonitorChanged { change, .. } = &update.fact else {
                panic!("a monitor change");
            };
            let status = change.status.clone();
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
                        if applied {
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
            registry.apply(&event).unwrap();
            // The record carries the alias's complete state after the change.
            assert_eq!(status, registry.instance_status(alias).unwrap(), "{alias}");
            if applied {
                rows.insert(alias, Some(event.sequence()));
            }
            for (alias, row) in &rows {
                assert_eq!(monitor_row(&database, alias), *row, "{alias}");
            }
        }
        ledger.close().unwrap();
    }
}
