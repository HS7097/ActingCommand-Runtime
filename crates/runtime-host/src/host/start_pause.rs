// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #453 S2 (`contracts/scheduling-pause.md`, "Pauses held at a first start"; register
//! §1.357): an `instances[]` entry that declares `start_paused` is held at the first start that sees
//! it, before any instance is prepared, with an instance pause of the existing reason
//! `instance.added` (stage `released`, set in this owner epoch), unless a pause of its own holds the
//! instance already (that pause stays exactly as it is). The start then names the entry's instance
//! id in the tagged runtime fact `host.start_pause_applied` (scope Runtime, a record list of
//! `{instance_id, applied_at_unix_ms}`), so that no later start holds it again: once a person has
//! resumed it, it stays resumed, and the member may stay in the file.
//!
//! - **Order.** The pause rows are recorded first (`host.scheduling_pause`, by the start hold's own
//!   record), the applied rows second. A refused record fails the start, as every start hold's
//!   does. A start that ends between the two records holds the entry again at the next start (or
//!   finds its restored pause and keeps it) and then records it: the hold is idempotent.
//! - **Nothing without the member.** A configuration whose entries do not declare it hands the
//!   host no instance id: the start neither reads nor writes the fact.
//! - **An installer-held start** holds after its own restore, the same way.

use super::*;
use crate::codes::{HostCode, HostLocation};
use actingcommand_contract::codes::SanitizationCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{INSTANCE_ADDED_PAUSE_REASON, SchedulingPauseReason};

/// The tagged runtime fact of the entries whose first-start pause was applied
/// (`ledger_observer.tag.workbench_patterns`).
pub(super) const START_PAUSE_APPLIED_FACT_KEY: &str = "host.start_pause_applied";
/// The members of one row: the entry's instance id as the configuration names it, and the start's
/// clock when the row was recorded.
const ROW_INSTANCE_ID: &str = "instance_id";
const ROW_APPLIED_AT: &str = "applied_at_unix_ms";

#[cfg(test)]
thread_local! {
    /// Workflow #453 S2, test builds only: the next record of `host.start_pause_applied` on this
    /// thread is refused, after the pause rows were recorded, as a start that ends between the
    /// two records; nothing is written.
    pub(crate) static START_PAUSE_APPLIED_REFUSED_FOR_TEST: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

/// Workflow #453 S2: check-config's note when an entry declares `start_paused` (`start_paused`)
/// while `workbench_patterns`, the configuration's `ledger_observer.tag.workbench_patterns` in
/// effect, leaves out `host.start_pause_applied`: no start would load the fact, so every start
/// would hold such an entry again. The note is `ledger_tag_key_unclassified` with `fact_key`;
/// `None` otherwise, and always `None` without the member.
pub fn start_pause_tag_note(start_paused: bool, workbench_patterns: &[String]) -> Option<Outcome> {
    (start_paused
        && !actingcommand_ledger::workbench_patterns_tag(
            workbench_patterns,
            START_PAUSE_APPLIED_FACT_KEY,
        ))
    .then(|| {
        Outcome::new(actingcommand_ledger::LedgerCode::LedgerTagKeyUnclassified.code())
            .fact_key(START_PAUSE_APPLIED_FACT_KEY)
    })
}

/// The instance id as the configuration and the fact's rows spell it.
fn instance_id_text(instance_id: InstanceId) -> RuntimeHostResult<String> {
    match serde_json::to_value(instance_id) {
        Ok(serde_json::Value::String(text)) => Ok(text),
        _ => Err(fact_invalid()),
    }
}

/// A `host.start_pause_applied` this build cannot read, or an instance id it cannot spell: the
/// start fails, as for a malformed `host.scheduling_pause`.
fn fact_invalid() -> RuntimeHostError {
    RuntimeHostError::fatal(
        SanitizationCode::InvalidRuntimeStateFact,
        HostLocation::HoldStartInstancePause,
        RuntimeErrorCode::RuntimeHalted,
    )
}

/// The rows of a `host.start_pause_applied` record as this build writes them (the Runtime's, a
/// record list, each row exactly a string `instance_id` and a `timestamp_ms`
/// `applied_at_unix_ms`); `None` for a record of another shape.
fn kept_rows(record: &RuntimeFactRecord) -> Option<Vec<BTreeMap<String, FactScalar>>> {
    if record.source != OriginModule::Runtime {
        return None;
    }
    let ContractFactValue::RecordList(rows) = &record.value else {
        return None;
    };
    rows.iter()
        .all(|row| {
            row.len() == 2
                && matches!(row.get(ROW_INSTANCE_ID), Some(FactScalar::String(_)))
                && matches!(row.get(ROW_APPLIED_AT), Some(FactScalar::TimestampMs(_)))
        })
        .then(|| rows.clone())
}

/// The instance id one kept row names.
fn row_instance_id(row: &BTreeMap<String, FactScalar>) -> Option<&str> {
    match row.get(ROW_INSTANCE_ID) {
        Some(FactScalar::String(instance_id)) => Some(instance_id),
        _ => None,
    }
}

impl HostShared {
    /// Workflow #453 S2 (module doc): the start's hold of the entries that declare `start_paused`
    /// and that `host.start_pause_applied` does not name yet, after the persisted pauses are
    /// restored and the other start holds are taken, before any instance is prepared. Each is held
    /// with `instance.added` unless a pause of its own holds it already, every pause recorded
    /// before the applied rows; then one record of the fact with the rows it held and the new ones.
    pub(super) fn hold_start_paused_instances(&self) -> RuntimeHostResult<()> {
        if self.start_paused.is_empty() {
            return Ok(());
        }
        let location = HostLocation::HoldStartInstancePause;
        let recorded = lock(&self.runtime_facts, location)?
            .get(&RuntimeFactScope::Runtime, START_PAUSE_APPLIED_FACT_KEY)
            .cloned();
        let mut rows = match &recorded {
            Some(record) => kept_rows(record).ok_or_else(fact_invalid)?,
            None => Vec::new(),
        };
        let applied = rows
            .iter()
            .filter_map(row_instance_id)
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        let declared = lock(&self.registered_instances, location)?
            .values()
            .filter(|instance| self.start_paused.contains(&instance.instance_id))
            .map(|instance| (instance.instance_alias.clone(), instance.instance_id))
            .collect::<Vec<_>>();
        let mut pending = Vec::with_capacity(declared.len());
        for (instance_alias, instance_id) in declared {
            let instance_id = instance_id_text(instance_id)?;
            if !applied.contains(&instance_id) {
                pending.push((instance_alias, instance_id));
            }
        }
        if pending.is_empty() {
            return Ok(());
        }
        let pause_reason =
            SchedulingPauseReason::new(INSTANCE_ADDED_PAUSE_REASON).map_err(|_| {
                RuntimeHostError::fatal(
                    HostCode::SchedulingPauseStateInvalid,
                    location,
                    RuntimeErrorCode::RuntimeHalted,
                )
            })?;
        for (instance_alias, _) in &pending {
            self.hold_named_instance_pause_at_start(instance_alias, &pause_reason)?;
        }
        #[cfg(test)]
        if START_PAUSE_APPLIED_REFUSED_FOR_TEST.with(|refused| refused.replace(false)) {
            return Err(RuntimeHostError::request(
                SanitizationCode::RuntimeFactRecordListTooLarge,
                location,
                RuntimeErrorCode::InvalidRequest,
            ));
        }
        let now = self.clock.sample()?.unix_ms;
        rows.extend(pending.into_iter().map(|(_, instance_id)| {
            BTreeMap::from([
                (ROW_INSTANCE_ID.to_owned(), FactScalar::String(instance_id)),
                (ROW_APPLIED_AT.to_owned(), FactScalar::TimestampMs(now)),
            ])
        }));
        let observed_at_unix_ms = recorded.as_ref().map_or(now, |record| {
            now.max(record.observed_at_unix_ms.saturating_add(1))
        });
        self.record_runtime_fact_with_event(RuntimeFactRecord {
            scope: RuntimeFactScope::Runtime,
            key: START_PAUSE_APPLIED_FACT_KEY.to_owned(),
            value: ContractFactValue::RecordList(rows),
            observed_at_unix_ms,
            source: OriginModule::Runtime,
            ttl_ms: None,
        })
        .map(|_| ())
    }
}
