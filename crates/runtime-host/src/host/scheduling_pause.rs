// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #361 B1: the operator's scheduling pauses survive a restart. The held pauses are
//! the runtime fact `host.scheduling_pause` (scope Runtime, a record list of existing
//! scalars, one row per held pause). A pause or a resume records the rows it leads to before
//! it changes the gate, under one persist gate; a start that is not installer-held restores
//! the gate from the fact before any instance is prepared and records the restored rows
//! again; an installer-held start keeps its transition's pauses and records them.

use super::policy_dispatch::SchedulingPauseTable;
use super::*;
use crate::codes::{HostCode, HostLocation};
use actingcommand_contract::outcome::Code;
use actingcommand_contract::{
    InstancePauseStage, InstancePauseState, OwnerEpoch, SchedulingPauseReason, SchedulingPauseState,
};

/// The runtime fact holding the operator's scheduling pauses.
pub(super) const SCHEDULING_PAUSE_FACT_KEY: &str = "host.scheduling_pause";

/// Where a held pause came from: the owner epoch it was set in and, for a restored pause, the
/// owner epoch before the start that restored it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PauseOrigin {
    set_in_owner_epoch: String,
    restored_from_owner_epoch: Option<String>,
}

impl PauseOrigin {
    /// A pause the operator sets in `owner_epoch`.
    pub(super) fn set_in(owner_epoch: OwnerEpoch) -> RuntimeHostResult<Self> {
        Ok(Self {
            set_in_owner_epoch: epoch_text(owner_epoch)?,
            restored_from_owner_epoch: None,
        })
    }
}

/// One persisted row: `scope` (`global` or `instance`), `instance_alias` (instance rows),
/// `pause_reason`, `since_unix_ms`, `set_in_owner_epoch` and, on a restored row,
/// `restored_from_owner_epoch`.
pub(super) fn pause_row(
    instance_alias: Option<&str>,
    pause_reason: &actingcommand_contract::SchedulingPauseReason,
    since_unix_ms: u64,
    origin: &PauseOrigin,
) -> BTreeMap<String, FactScalar> {
    let scope = if instance_alias.is_some() {
        "instance"
    } else {
        "global"
    };
    let mut row = BTreeMap::from([
        ("scope".to_owned(), FactScalar::String(scope.to_owned())),
        (
            "pause_reason".to_owned(),
            FactScalar::String(pause_reason.as_str().to_owned()),
        ),
        (
            "since_unix_ms".to_owned(),
            FactScalar::TimestampMs(since_unix_ms),
        ),
        (
            "set_in_owner_epoch".to_owned(),
            FactScalar::String(origin.set_in_owner_epoch.clone()),
        ),
    ]);
    if let Some(alias) = instance_alias {
        row.insert(
            "instance_alias".to_owned(),
            FactScalar::String(alias.to_owned()),
        );
    }
    if let Some(previous) = &origin.restored_from_owner_epoch {
        row.insert(
            "restored_from_owner_epoch".to_owned(),
            FactScalar::String(previous.clone()),
        );
    }
    row
}

fn epoch_text(owner_epoch: OwnerEpoch) -> RuntimeHostResult<String> {
    match serde_json::to_value(owner_epoch) {
        Ok(serde_json::Value::String(text)) => Ok(text),
        _ => Err(RuntimeHostError::fatal(
            HostCode::SchedulingPauseEpochEncodeFailed,
            HostLocation::PersistSchedulingPause,
            RuntimeErrorCode::RuntimeFatal,
        )),
    }
}

fn restore_invalid(code: impl Into<Code>) -> RuntimeHostError {
    RuntimeHostError::fatal(
        code,
        HostLocation::RestoreSchedulingPauses,
        RuntimeErrorCode::RuntimeFatal,
    )
}

fn row_text<'a>(row: &'a BTreeMap<String, FactScalar>, field: &str) -> RuntimeHostResult<&'a str> {
    match row.get(field) {
        Some(FactScalar::String(value)) => Ok(value),
        _ => Err(restore_invalid(HostCode::SchedulingPauseFactInvalid)),
    }
}

impl HostShared {
    /// Records `table`'s held pauses as `host.scheduling_pause`, observed strictly after the
    /// previous record. The caller holds the persist gate.
    pub(super) fn record_scheduling_pauses(
        &self,
        table: &SchedulingPauseTable,
    ) -> RuntimeHostResult<()> {
        let rows = table.persisted_rows()?;
        let now = self.clock.sample()?.unix_ms;
        let previous = lock(
            &self.runtime_facts,
            HostLocation::ReadSchedulingPauseFactClock,
        )?
        .get(&RuntimeFactScope::Runtime, SCHEDULING_PAUSE_FACT_KEY)
        .map(|record| record.observed_at_unix_ms);
        let observed_at_unix_ms = match previous {
            Some(previous) => now.max(previous.checked_add(1).ok_or_else(|| {
                RuntimeHostError::fatal(
                    HostCode::SchedulingPauseFactClockOverflow,
                    HostLocation::PersistSchedulingPause,
                    RuntimeErrorCode::RuntimeFatal,
                )
            })?),
            None => now,
        };
        self.record_runtime_fact_with_event(RuntimeFactRecord {
            scope: RuntimeFactScope::Runtime,
            key: SCHEDULING_PAUSE_FACT_KEY.to_owned(),
            value: ContractFactValue::RecordList(rows),
            observed_at_unix_ms,
            source: OriginModule::Runtime,
            ttl_ms: None,
        })
        .map(|_| ())
    }

    /// The record of a pause or resume request: a refused record fails the request with
    /// `scheduling_pause_persist_failed` (Failed) and leaves the gate unchanged; a fatal one
    /// poisons it.
    pub(super) fn persist_scheduling_pauses(
        &self,
        table: &SchedulingPauseTable,
    ) -> Result<(), RequestFailure> {
        self.record_scheduling_pauses(table).map_err(|error| {
            if error.is_fatal() {
                RequestFailure::poison_without_terminal(error)
            } else {
                RequestFailure::request(
                    RuntimeHostError::request(
                        HostCode::SchedulingPausePersistFailed,
                        HostLocation::PersistSchedulingPause,
                        RuntimeErrorCode::LedgerFailure,
                    )
                    .with_native_detail(format!("{error:?}")),
                    RuntimeReceiptState::Failed,
                    None,
                )
            }
        })
    }

    /// Workflow #369 E3 (#670 safety net): holds a pause of `instance_alias` with `pause_reason`
    /// at start, after the persisted pauses are restored and before any instance is prepared,
    /// unless the instance is paused already (that pause stays as it is). Records the pauses.
    pub(super) fn hold_instance_pause_at_start(
        &self,
        instance_alias: &str,
        pause_reason: Code,
    ) -> RuntimeHostResult<()> {
        let _persist = lock(
            &self.scheduling_pause_persist_gate,
            HostLocation::HoldStartInstancePause,
        )?;
        let mut prospective =
            lock(&self.scheduling_pause, HostLocation::HoldStartInstancePause)?.clone();
        let since_unix_ms = self.clock.sample()?.unix_ms;
        if !prospective.hold_instance_at_start(
            instance_alias,
            pause_reason,
            since_unix_ms,
            PauseOrigin::set_in(self.owner_epoch)?,
        )? {
            return Ok(());
        }
        self.record_scheduling_pauses(&prospective)?;
        *lock(&self.scheduling_pause, HostLocation::HoldStartInstancePause)? = prospective;
        Ok(())
    }

    /// A failed instance pause stage: the persisted pauses without the instance, then the gate
    /// lifted; both are attempted and the first failure is returned.
    pub(super) fn lift_failed_instance_pause(&self, instance_alias: &str) -> RuntimeHostResult<()> {
        let _persist = lock(
            &self.scheduling_pause_persist_gate,
            HostLocation::PersistFailedInstancePause,
        )?;
        let mut prospective = lock(
            &self.scheduling_pause,
            HostLocation::LiftFailedInstancePause,
        )?
        .clone();
        prospective.lift_instance(instance_alias)?;
        let recorded = self.record_scheduling_pauses(&prospective);
        lock(
            &self.scheduling_pause,
            HostLocation::LiftFailedInstancePause,
        )
        .and_then(|mut table| table.lift_instance(instance_alias))
        .and(recorded)
    }

    /// Restores the persisted pauses at start, before any instance is prepared, and returns
    /// one line per restored or dropped pause for the daemon's stdout.
    ///
    /// - An installer-held start keeps the pauses its transition restored and records them.
    /// - Any other start reads `host.scheduling_pause`: the global pause comes back at revision
    ///   1, an instance pause at revision 1 with stage `released` (the device is left as it
    ///   is), each with its reason and `since`; a pause of an alias no longer registered is
    ///   dropped. The restored rows are recorded again with `restored_from_owner_epoch`, which
    ///   is the ledger record of the restore and of every dropped row. A ledger without the
    ///   fact restores nothing.
    pub(super) fn restore_scheduling_pauses(
        &self,
        held: bool,
        previous_owner_epoch: Option<OwnerEpoch>,
    ) -> RuntimeHostResult<Vec<String>> {
        let _persist = lock(
            &self.scheduling_pause_persist_gate,
            HostLocation::RestoreSchedulingPauses,
        )?;
        let previous = previous_owner_epoch.map(epoch_text).transpose()?;
        let recorded = lock(&self.runtime_facts, HostLocation::ReadSchedulingPauseFact)?
            .get(&RuntimeFactScope::Runtime, SCHEDULING_PAUSE_FACT_KEY)
            .cloned();
        if held {
            let origin = PauseOrigin {
                set_in_owner_epoch: match &previous {
                    Some(previous) => previous.clone(),
                    None => epoch_text(self.owner_epoch)?,
                },
                restored_from_owner_epoch: previous,
            };
            let mut table = lock(
                &self.scheduling_pause,
                HostLocation::ReadInstallRestoredPauses,
            )?
            .clone();
            table.adopt_origins(&origin);
            if recorded.is_some() || table.holds_pauses() {
                self.record_scheduling_pauses(&table)?;
            }
            lock(
                &self.scheduling_pause,
                HostLocation::AdoptInstallPauseOrigins,
            )?
            .adopt_origins(&origin);
            return Ok(Vec::new());
        }
        let Some(recorded) = recorded else {
            return Ok(Vec::new());
        };
        if recorded.source != OriginModule::Runtime {
            return Err(restore_invalid(HostCode::SchedulingPauseFactInvalid));
        }
        let ContractFactValue::RecordList(rows) = &recorded.value else {
            return Err(restore_invalid(HostCode::SchedulingPauseFactInvalid));
        };
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        let registered = lock(
            &self.registered_instances,
            HostLocation::ReadSchedulingPauseScopes,
        )?
        .values()
        .map(|instance| instance.instance_alias.clone())
        .collect::<BTreeSet<_>>();
        let mut table = lock(
            &self.scheduling_pause,
            HostLocation::ReadSchedulingPauseTable,
        )?
        .clone();
        let mut lines = Vec::with_capacity(rows.len());
        for row in rows {
            let pause_reason = SchedulingPauseReason::new(row_text(row, "pause_reason")?)
                .map_err(|_| restore_invalid(HostCode::SchedulingPauseFactInvalid))?;
            let since_unix_ms = match row.get("since_unix_ms") {
                Some(FactScalar::TimestampMs(value)) => *value,
                _ => {
                    return Err(restore_invalid(HostCode::SchedulingPauseFactInvalid));
                }
            };
            let origin = PauseOrigin {
                set_in_owner_epoch: row_text(row, "set_in_owner_epoch")?.to_owned(),
                restored_from_owner_epoch: previous.clone(),
            };
            match row_text(row, "scope")? {
                "global" => {
                    if table.holds_global() {
                        return Err(restore_invalid(HostCode::SchedulingPauseFactInvalid));
                    }
                    let state = SchedulingPauseState {
                        revision: 1,
                        pause_reason,
                        since_unix_ms,
                    };
                    state
                        .validate()
                        .map_err(|_| restore_invalid(HostCode::SchedulingPauseFactInvalid))?;
                    table.set_global(state, origin);
                    lines.push(format!(
                        "scheduling_pause_restored scope=global since={since_unix_ms}"
                    ));
                }
                "instance" => {
                    let alias = row_text(row, "instance_alias")?;
                    if !registered.contains(alias) {
                        lines.push(format!(
                            "scheduling_pause_dropped scope=instance:{alias} reason=instance_not_registered"
                        ));
                        continue;
                    }
                    if table.holds_instance(alias) {
                        return Err(restore_invalid(HostCode::SchedulingPauseFactInvalid));
                    }
                    let state = InstancePauseState {
                        revision: 1,
                        pause_reason,
                        since_unix_ms,
                        stage: InstancePauseStage::Released,
                    };
                    state
                        .validate()
                        .map_err(|_| restore_invalid(HostCode::SchedulingPauseFactInvalid))?;
                    table.set_instance(alias, state, origin);
                    lines.push(format!(
                        "scheduling_pause_restored scope=instance:{alias} since={since_unix_ms}"
                    ));
                }
                _ => {
                    return Err(restore_invalid(HostCode::SchedulingPauseFactInvalid));
                }
            }
        }
        self.record_scheduling_pauses(&table)?;
        *lock(
            &self.scheduling_pause,
            HostLocation::RestoreSchedulingPauses,
        )? = table;
        Ok(lines)
    }
}
