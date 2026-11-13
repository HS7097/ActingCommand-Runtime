// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #453 S2 (`contracts/scheduling-pause.md`, "Pauses held at a first start"; register
//! §1.357): an `instances[]` entry that declares `start_paused` is held at the first start that sees
//! it, before any instance is prepared, with an instance pause of the existing reason
//! `instance.added` (stage `released`, set in this owner epoch). The start then names the entry's
//! instance id in the tagged runtime fact `host.start_pause_applied` (scope Runtime, a record list
//! of `{instance_id, applied_at_unix_ms}`), so that no later start holds it again: once a person has
//! resumed it, it stays resumed, and the member may stay in the file.
//!
//! - **Order.** The pause rows are recorded first (`host.scheduling_pause`, by the start hold's own
//!   record), the applied rows second. A refused record fails the start, as every start hold's
//!   does. A start that ends between the two records finds the restored `instance.added` pause at
//!   the next start, keeps it and then records the row: the hold is idempotent.
//! - **Applied only by its own pause** (fix 1). An entry the start finds held by a pause of another
//!   reason keeps that pause exactly as it is and is not recorded. When that pause is released
//!   while the Runtime runs (`ResumeScheduling` of the instance: a person's resume from any entry,
//!   the installer's release of its hold, the Lab's hand-back), the release holds the instance
//!   with `instance.added` instead, under the same persist gate and on the same claim's key, its
//!   pause recorded before its applied row (`HostShared::hold_start_pause_at_release`).
//! - **Bounded rows** (fix 1). Each record of the fact keeps only the rows of the instance ids
//!   configured at this start, so the list holds at most one row per configured instance; an
//!   instance id removed from the configuration that comes back with the member is held again.
//! - **Nothing without the member.** A configuration whose entries do not declare it hands the
//!   host no instance id: the start neither reads nor writes the fact, nothing is unapplied, and
//!   every release returns before anything else.
//! - **An installer-held start** holds after its own restore, the same way.

use super::policy_dispatch::SchedulingPauseTable;
use super::scheduling_pause::{PauseOrigin, scheduling_pause_persist_failure};
use super::*;
use crate::codes::{HostCode, HostLocation};
use actingcommand_contract::codes::{ContractLocation, SanitizationCode};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{INSTANCE_ADDED_PAUSE_REASON, SchedulingPauseReason};

/// The tagged runtime fact of the entries whose first-start pause was applied
/// (`ledger_observer.tag.workbench_patterns`).
pub(super) const START_PAUSE_APPLIED_FACT_KEY: &str = "host.start_pause_applied";
/// The members of one row: the entry's instance id as the configuration names it, and the clock
/// when the row was recorded.
const ROW_INSTANCE_ID: &str = "instance_id";
const ROW_APPLIED_AT: &str = "applied_at_unix_ms";

#[cfg(test)]
thread_local! {
    /// Workflow #453 S2, test builds only: the next record of `host.start_pause_applied` a start
    /// makes on this thread is refused, after the pause rows were recorded, as a start that ends
    /// between the two records; nothing is written.
    pub(crate) static START_PAUSE_APPLIED_REFUSED_FOR_TEST: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
    /// Workflow #453 S2 fix 1, test builds only: while set on this thread, the start's hold and
    /// the release's hold return at once, as a build without the slice's code path would; the
    /// comparison of a configuration without the member runs its host beside one with it set.
    pub(crate) static START_PAUSE_OFF_FOR_TEST: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

/// Whether the start's hold and the release's hold run: always in a build.
#[cfg(not(test))]
const fn start_pause_entries_on() -> bool {
    true
}

/// Whether the start's hold and the release's hold run: in a test build, unless the thread set
/// `START_PAUSE_OFF_FOR_TEST`.
#[cfg(test)]
fn start_pause_entries_on() -> bool {
    !START_PAUSE_OFF_FOR_TEST.with(std::cell::Cell::get)
}

/// Workflow #453 S2: check-config's note when an entry declares `start_paused` (`start_paused`)
/// while `workbench_patterns`, the configuration's `ledger_observer.tag.workbench_patterns` in
/// effect, leaves out `host.start_pause_applied`. Today's start replays the program facts from the
/// latest snapshot and every later record whatever the tags, so such a start still reads the
/// fact; the list decides only the fact's index row and its synchronous write. The default list
/// holds the key because the rule for program state is tagged records, and a later ledger change
/// is to load a start's program facts by tag. The note is `ledger_tag_key_unclassified` with
/// `fact_key`; `None` otherwise, and always `None` without the member.
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

/// The pause reason of the member, `instance.added`.
fn instance_added() -> RuntimeHostResult<SchedulingPauseReason> {
    SchedulingPauseReason::new(INSTANCE_ADDED_PAUSE_REASON).map_err(|_| {
        RuntimeHostError::fatal(
            HostCode::SchedulingPauseStateInvalid,
            HostLocation::HoldStartInstancePause,
            RuntimeErrorCode::RuntimeHalted,
        )
    })
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

/// The record of `host.start_pause_applied` a host holds, if any, and its rows.
struct AppliedFact {
    recorded: Option<RuntimeFactRecord>,
    rows: Vec<BTreeMap<String, FactScalar>>,
}

impl HostShared {
    /// The record of `host.start_pause_applied` this host holds and its rows; a record of another
    /// shape fails.
    fn start_pause_applied_fact(&self) -> RuntimeHostResult<AppliedFact> {
        let recorded = lock(&self.runtime_facts, HostLocation::HoldStartInstancePause)?
            .get(&RuntimeFactScope::Runtime, START_PAUSE_APPLIED_FACT_KEY)
            .cloned();
        let rows = match &recorded {
            Some(record) => kept_rows(record).ok_or_else(fact_invalid)?,
            None => Vec::new(),
        };
        Ok(AppliedFact { recorded, rows })
    }

    /// Records `host.start_pause_applied` with one new row per instance of `applied`: the rows it
    /// holds now, less every row whose instance id this start's configuration does not name (fix
    /// 1: the list stays bounded by the configured instances), then the new rows.
    fn record_start_pause_applied(&self, applied: &[InstanceId]) -> RuntimeHostResult<()> {
        let location = HostLocation::HoldStartInstancePause;
        let AppliedFact { recorded, rows } = self.start_pause_applied_fact()?;
        let configured = lock(&self.registered_instances, location)?
            .keys()
            .copied()
            .map(instance_id_text)
            .collect::<RuntimeHostResult<BTreeSet<_>>>()?;
        let applied = applied
            .iter()
            .copied()
            .map(instance_id_text)
            .collect::<RuntimeHostResult<Vec<_>>>()?;
        let replaced = applied.iter().map(String::as_str).collect::<BTreeSet<_>>();
        let mut rows = rows
            .into_iter()
            .filter(|row| {
                row_instance_id(row).is_some_and(|instance_id| {
                    configured.contains(instance_id) && !replaced.contains(instance_id)
                })
            })
            .collect::<Vec<_>>();
        let now = self.clock.sample()?.unix_ms;
        rows.extend(applied.into_iter().map(|instance_id| {
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

    /// Workflow #453 S2 (module doc): the start's hold of the entries that declare `start_paused`
    /// and that `host.start_pause_applied` does not name yet, after the persisted pauses are
    /// restored and the other start holds are taken, before any instance is prepared, in the order
    /// of their instance ids:
    ///
    /// - not paused: held with `instance.added`, its pause recorded, and applied;
    /// - paused with `instance.added` (a start that ended between the two records, or the
    ///   installer's pause of an added instance): that pause kept, and applied;
    /// - paused for another reason: that pause kept exactly as it is and nothing recorded; the
    ///   instance waits in `start_pause_unapplied` for that pause's release (fix 1).
    ///
    /// Then one record of the fact with the applied rows, after every pause record.
    pub(super) fn hold_start_paused_instances(&self) -> RuntimeHostResult<()> {
        if !start_pause_entries_on() || self.start_paused.is_empty() {
            return Ok(());
        }
        let location = HostLocation::HoldStartInstancePause;
        let applied = self
            .start_pause_applied_fact()?
            .rows
            .iter()
            .filter_map(row_instance_id)
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        let declared = lock(&self.registered_instances, location)?
            .values()
            .filter(|instance| self.start_paused.contains(&instance.instance_id))
            .map(|instance| (instance.instance_alias.clone(), instance.instance_id))
            .collect::<Vec<_>>();
        let pause_reason = instance_added()?;
        let mut applying = Vec::with_capacity(declared.len());
        let mut unapplied = BTreeSet::new();
        for (instance_alias, instance_id) in declared {
            if applied.contains(&instance_id_text(instance_id)?) {
                continue;
            }
            if self.hold_named_instance_pause_at_start(&instance_alias, &pause_reason)? {
                applying.push(instance_id);
                continue;
            }
            let held_by = lock(&self.scheduling_pause, location)?
                .instance_state(&instance_alias)
                .map(|pause| pause.pause_reason);
            if held_by.as_ref() == Some(&pause_reason) {
                applying.push(instance_id);
            } else {
                unapplied.insert(instance_id);
            }
        }
        *lock(&self.start_pause_unapplied, location)? = unapplied;
        if applying.is_empty() {
            return Ok(());
        }
        #[cfg(test)]
        if START_PAUSE_APPLIED_REFUSED_FOR_TEST.with(|refused| refused.replace(false)) {
            return Err(RuntimeHostError::request(
                SanitizationCode::RuntimeFactRecordListTooLarge,
                location,
                RuntimeErrorCode::InvalidRequest,
            ));
        }
        self.record_start_pause_applied(&applying)
    }

    /// Workflow #453 S2 fix 1 (module doc): the release of `instance_alias`'s pause while its
    /// entry's member is not applied yet. Called by an instance resume under the persist gate and
    /// on its reconnect claim's key, with `prospective`, the pause table the resume would lift,
    /// after the resume's own refusals. Without an unapplied member it returns `None` before
    /// anything else, and the resume lifts the pause as before. Otherwise the pause is replaced in
    /// one step by an `instance.added` pause (the lift and the hold each take the scope's next
    /// revision, stage `released`, set in this owner epoch): its rows are recorded, the live table
    /// changes from the old pause to the new one under one lock (the instance is never without a
    /// pause in between), then the applied row is recorded. Returns the new pause's revision.
    /// A refused record fails the request with `scheduling_pause_persist_failed`: before the
    /// change, nothing changed; after it, the instance stays held with `instance.added` and its
    /// member unapplied, so its next release holds it again.
    pub(super) fn hold_start_pause_at_release(
        &self,
        instance_alias: &str,
        instance_id: InstanceId,
        prospective: &mut SchedulingPauseTable,
    ) -> Result<Option<u64>, RequestFailure> {
        // No configured entry declares the member (every installation without it): return before
        // any lock, so that a release takes exactly the locks it takes without this slice.
        if !start_pause_entries_on() || self.start_paused.is_empty() {
            return Ok(None);
        }
        {
            let unapplied = lock(
                &self.start_pause_unapplied,
                HostLocation::HoldStartInstancePause,
            )?;
            if unapplied.is_empty() || !unapplied.contains(&instance_id) {
                return Ok(None);
            }
        }
        if !prospective.holds_instance(instance_alias) {
            return Ok(None);
        }
        let pause_reason = instance_added()?;
        let since_unix_ms = self
            .runtime_clock_sample()
            .map_err(RequestFailure::poison_without_terminal)?
            .unix_ms;
        let origin = PauseOrigin::set_in(self.owner_epoch)?;
        prospective.lift_instance(instance_alias)?;
        prospective.hold_named_instance_at_start(
            instance_alias,
            pause_reason.clone(),
            since_unix_ms,
            origin.clone(),
        )?;
        self.persist_scheduling_pauses(prospective)?;
        let held = {
            let mut table = lock(&self.scheduling_pause, ContractLocation::ResumeScheduling)?;
            table.lift_instance(instance_alias)?;
            table.hold_named_instance_at_start(
                instance_alias,
                pause_reason,
                since_unix_ms,
                origin,
            )?;
            table.instance_state(instance_alias)
        };
        let Some(held) = held else {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::fatal(
                    HostCode::SchedulingPauseStateLost,
                    ContractLocation::ResumeScheduling,
                    RuntimeErrorCode::RuntimeHalted,
                ),
            ));
        };
        self.record_start_pause_applied(&[instance_id])
            .map_err(scheduling_pause_persist_failure)?;
        lock(
            &self.start_pause_unapplied,
            HostLocation::HoldStartInstancePause,
        )?
        .remove(&instance_id);
        Ok(Some(held.revision))
    }
}
