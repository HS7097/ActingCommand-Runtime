// SPDX-License-Identifier: AGPL-3.0-only

//! #503 PACE (0.12.100; `contracts/policy-suspension.md`, "A pair whose runs keep failing";
//! register §1.545): the pairs (instance, catalog task) whose scheduled runs keep failing, kept in
//! the host's memory beside Workflow #490 ENTRY-A's pairs (`entry_blocks.rs`), not inside them.
//!
//! The live settlement of a scheduled run tells its pair what it settled
//! ([`crate::failure_pacing::PacedSettlement`]) under the policy outcome gate, after ENTRY-A's
//! settlement; the evaluation reads the pairs under the same gate
//! ([`HostShared::failure_pacing_gates`]), so no evaluation sees a run's settlement without its
//! pair. A catalogue that becomes active and the scheduler workbench's check that finds a package of
//! a game in use changed clear every pair, at the places that clear ENTRY-A's
//! ([`HostShared::clear_failure_pairs`]). The owner's ruling on this slice: the situation has its
//! code, which says the result and the operation that follows. So each failure that sets a wait
//! (the second in a row and every later one) writes one record under the instance, with the wait's
//! code and values (`instance_task_failure_waiting`), at that settlement; a failure that sets the
//! pair aside writes, in its place, `instance_task_failure_set_aside_reported`, once per pair and
//! window. Both are `runtime.failed` records handed to the daemon's log, as ENTRY-A's set-aside
//! report; no evaluation writes either, and no replay reads them. Memory only: a restart starts
//! with no pair. A leaf lock.

use super::*;
use crate::codes::{HostCode, HostLocation};
use crate::failure_pacing::{
    FailurePacingGate, FailurePacingGates, FailurePair, PacedRecord, PacedSettlement, PacingRule,
};
use actingcommand_contract::outcome::Outcome;

/// Test only: a pair as a test reads it: its failures in a row, those against it, the time before
/// which it is not dispatched on its instance and whether it is set aside.
#[cfg(test)]
pub(crate) type FailurePairForTest = (u32, u32, u64, bool);

impl HostShared {
    /// #503 PACE: what the execution record `data`, recorded now by the live settlement of a
    /// scheduled run, tells its pair (`context` the run when the settlement had it, `rule` what
    /// the policy knows of its dispatch and task). A failure that sets a wait writes its record;
    /// one that sets the pair aside tells a person instead.
    pub(super) fn settle_failure_pacing(
        &self,
        data: &PolicyExecutionEventData,
        context: Option<&PolicyRunContext>,
        rule: PacingRule,
    ) -> RuntimeHostResult<()> {
        let Some(instance_id) = self.settled_instance(data, context)? else {
            return Ok(());
        };
        let config = self.unavailable_probe_config()?;
        let record = lock(&self.failure_pairs, HostLocation::InstanceTaskFailurePacing)?.settle(
            instance_id,
            &data.task_id,
            PacedSettlement::of(data),
            rule,
            config,
        );
        let outcome = match record {
            Some(PacedRecord::Waits(pair)) => {
                pair_outcome(HostCode::InstanceTaskFailureWaiting, data, &pair)
                    .count(pair.in_a_row)
                    .next_eligible_unix_ms(pair.wait_end())
            }
            Some(PacedRecord::SetAside(pair)) => {
                pair_outcome(HostCode::InstanceTaskFailureSetAsideReported, data, &pair)
                    .count(pair.against)
                    .limit_count(pair.threshold)
                    .window_end_unix_ms(pair.window_end)
            }
            None => return Ok(()),
        };
        self.record_failure_pacing(instance_id, outcome)
    }

    /// #503 PACE: the pairs whose scheduled runs keep failing, by alias, as the evaluation reads
    /// them (read under the policy outcome gate a settlement counts them under).
    pub(super) fn failure_pacing_gates(&self) -> RuntimeHostResult<FailurePacingGates> {
        let pairs = lock(&self.failure_pairs, HostLocation::InstanceTaskFailurePacing)?
            .pairs()
            .iter()
            .map(|((instance_id, task_id), pair)| {
                (*instance_id, task_id.clone(), FailurePacingGate::of(pair))
            })
            .collect::<Vec<_>>();
        if pairs.is_empty() {
            return Ok(FailurePacingGates::default());
        }
        let instances = lock(
            &self.registered_instances,
            HostLocation::InstanceTaskFailurePacing,
        )?;
        let pairs = pairs
            .into_iter()
            .filter_map(|(instance_id, task_id, gate)| {
                instances
                    .get(&instance_id)
                    .map(|instance| ((instance.instance_alias.clone(), task_id), gate))
            })
            .collect::<BTreeMap<_, _>>();
        Ok(FailurePacingGates::new(pairs))
    }

    /// #503 PACE: a catalogue became active (an activation or a rollback), or the scheduler
    /// workbench's check found a package of a game in use changed: every pair of every instance
    /// starts from nothing, its count, its wait and its being set aside cleared, as ENTRY-A's pairs
    /// are at the same events.
    pub(super) fn clear_failure_pairs(&self) -> RuntimeHostResult<()> {
        lock(&self.failure_pairs, HostLocation::InstanceTaskFailurePacing)?.clear();
        Ok(())
    }

    /// One record of the pairs of `instance_id`, `outcome`: the wait a failure set
    /// (`instance_task_failure_waiting`, its category info) or a pair set aside
    /// (`instance_task_failure_set_aside_reported`, a warning), as a `runtime.failed` record under
    /// the instance's links, handed to the daemon's log too, as ENTRY-A's set-aside report is.
    fn record_failure_pacing(
        &self,
        instance_id: InstanceId,
        outcome: Outcome,
    ) -> RuntimeHostResult<()> {
        let operation = HostLocation::InstanceTaskFailurePacing;
        let links = self
            .events
            .system_links()?
            .with_instance_id(self.events.issuer().issue_registered_instance(instance_id));
        self.append_event_raw(
            EventSeverity::Warning,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links,
            RuntimePayloadDraft::failed(
                DiagnosticCode::RuntimeDiagnostic,
                EffectDisposition::NotPerformed,
                outcome.clone(),
                DiagnosticDetailDraft::new(
                    "failure_pacing",
                    operation,
                    "runtime_host",
                    operation.as_str(),
                ),
                AuditInput::new(),
            ),
        )?;
        self.fatal.report_warning(&outcome);
        Ok(())
    }

    /// Test only: the pair of `task_id` on `instance_id`, if it has one.
    #[cfg(test)]
    pub(super) fn failure_pair_for_test(
        &self,
        instance_id: InstanceId,
        task_id: &str,
    ) -> RuntimeHostResult<Option<FailurePairForTest>> {
        Ok(
            lock(&self.failure_pairs, HostLocation::InstanceTaskFailurePacing)?
                .pair(instance_id, task_id)
                .map(|pair| {
                    (
                        pair.in_a_row,
                        pair.against,
                        pair.until_unix_ms,
                        pair.set_aside,
                    )
                }),
        )
    }

    /// Test only: the pair of `task_id` on `instance_id` stands as `in_a_row` failures in a row in
    /// the window ending at `window_end`, its latest settled at `latest_unix_ms`, waiting until
    /// `until_unix_ms`; with `set_aside`, set aside by one failure against it under a threshold of
    /// 1, else none against it; it replaces the pair's own.
    #[cfg(test)]
    pub(super) fn seed_failure_pair_for_test(
        &self,
        instance_id: InstanceId,
        task_id: &str,
        (in_a_row, latest_unix_ms, until_unix_ms, window_end, set_aside): (
            u32,
            u64,
            u64,
            u64,
            bool,
        ),
    ) -> RuntimeHostResult<()> {
        lock(&self.failure_pairs, HostLocation::InstanceTaskFailurePacing)?.seed(
            instance_id,
            task_id,
            FailurePair {
                window_end,
                decision_id: format!("seeded-{task_id}"),
                failure_code: actingcommand_contract::scheduled_failure_code_str(&Outcome::new(
                    actingcommand_contract::codes::SharedCode::ContainedTaskPageUnknown,
                )),
                observed_at_unix_ms: latest_unix_ms,
                in_a_row,
                until_unix_ms,
                succeeded: 0,
                against: u32::from(set_aside),
                threshold: if set_aside { 1 } else { u16::MAX },
                set_aside,
            },
        );
        Ok(())
    }
}

/// The record `code` of the pair `pair` whose latest failure is `data`'s: its instance (the alias
/// the execution record names, as the candidates' reasons name it), its task, the latest failure's
/// dispatch, code (when the catalog registers it) and settlement.
fn pair_outcome(code: HostCode, data: &PolicyExecutionEventData, pair: &FailurePair) -> Outcome {
    let outcome = Outcome::new(code)
        .operation(HostLocation::InstanceTaskFailurePacing)
        .instance_id(data.instance_id.as_str())
        .task_id(data.task_id.as_str())
        .decision_id(pair.decision_id.as_str())
        .observed_at_unix_ms(pair.observed_at_unix_ms);
    match pair.failure_code.registered() {
        Some(failure_code) => outcome.failure_code(failure_code),
        None => outcome,
    }
}
