// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #429 R-12 (register §1.264; `contracts/runtime-interface.md`, "Workbench request
//! kinds", the row `RoutineSummary`): the host's side of the routine summary. Each policy run and
//! direct run of an instance is counted as the host settles it, by the task its run record names
//! (`PackageAdmitted.task_label`) and the code of its terminal ([`HostShared::note_routine_run`]);
//! `RoutineSummary` and the daily summary of the Runtime's messages (Workflow #432 AGT-2b) read
//! the memory through [`HostShared::routine_summary`]. Memory only: nothing is read from or
//! written to the ledger, and a restart begins it again with its owner epoch. The memory's lock
//! is a leaf: nothing is taken while it is held but, in tests, the injected local offset's.

use super::*;
use crate::routine_summary::{RoutineSummaryMemory, RunEnd, SettledRun};
use actingcommand_contract::outcome::CodeStr;
use actingcommand_contract::{RoutineSummaryGranularity, RoutineSummaryPage};
#[cfg(test)]
use std::sync::PoisonError;

/// The routine summary of this owner epoch, and in tests the host's local offset.
pub(super) struct RoutineSummaryShared {
    memory: Mutex<RoutineSummaryMemory>,
    /// Test only: the host's local zone, milliseconds east of UTC, in place of the machine's.
    #[cfg(test)]
    local_offset_for_test: Mutex<Option<i64>>,
}

impl RoutineSummaryShared {
    /// The memory of an epoch that began at `started_at_unix_ms` (the start's clock reading).
    pub(super) fn new(started_at_unix_ms: u64) -> Self {
        Self {
            memory: Mutex::new(RoutineSummaryMemory::new(started_at_unix_ms)),
            #[cfg(test)]
            local_offset_for_test: Mutex::new(None),
        }
    }

    /// The host's offset from UTC at `unix_ms`, in milliseconds east, daylight saving time
    /// included; UTC when the system cannot convert the instant (a bucket opened then shows it
    /// by its bounds).
    fn local_offset_ms(&self, unix_ms: u64) -> i64 {
        #[cfg(test)]
        if let Some(offset_ms) = *self
            .local_offset_for_test
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
        {
            return offset_ms;
        }
        actingcommand_ledger::local_time::machine_local_offset_ms(unix_ms).unwrap_or(0)
    }
}

impl HostShared {
    /// What `RoutineSummary` answers (Workflow #429 R-12): the routine summary's page from the
    /// host's memory, by `granularity`, with `since_revision` only the buckets changed after it.
    /// `instance` names one registered instance; an alias no instance has is refused
    /// `instance_unknown`, as the instance reads refuse it; without it every registered instance
    /// answers, by alias. Nothing is read from or written to the ledger. The read and the daily
    /// summary of the Runtime's messages (Workflow #432 AGT-2b) both answer from here.
    pub(super) fn routine_summary(
        &self,
        instance: Option<&str>,
        granularity: RoutineSummaryGranularity,
        since_revision: Option<u64>,
    ) -> RuntimeHostResult<RoutineSummaryPage> {
        let registered = lock(&self.registered_instances, HostLocation::RoutineSummary)?
            .values()
            .map(|registered| registered.instance_alias.clone())
            .collect::<BTreeSet<_>>();
        let aliases = match instance {
            Some(alias) if registered.contains(alias) => vec![alias.to_owned()],
            Some(_) => {
                return Err(RuntimeHostError::request(
                    RuntimeErrorCode::InstanceUnknown,
                    HostLocation::ResolveRuntimeInstance,
                    RuntimeErrorCode::InstanceUnknown,
                ));
            }
            None => registered.into_iter().collect::<Vec<_>>(),
        };
        Ok(
            lock(&self.routine_summary.memory, HostLocation::RoutineSummary)?.page(
                self.owner_epoch,
                &aliases,
                granularity,
                since_revision,
            ),
        )
    }

    /// Counts a policy run or a direct run of `instance_alias` and its task `task_id` that
    /// `executed` settled, at the host clock's reading now. A run that wrote no terminal (refused
    /// before it, or a failure the Runtime does not survive) settled nothing and is not counted.
    pub(super) fn note_routine_run(
        &self,
        instance_alias: &str,
        task_id: &str,
        executed: &Result<OperationSuccess, RequestFailure>,
    ) -> RuntimeHostResult<()> {
        match routine_run_end(executed) {
            Some(end) => self.count_routine_run(instance_alias, task_id, end),
            None => Ok(()),
        }
    }

    /// Counts one settled run that ended `end`, at the host clock's reading now.
    fn count_routine_run(
        &self,
        instance_alias: &str,
        task_id: &str,
        end: RunEnd,
    ) -> RuntimeHostResult<()> {
        let settled_at_unix_ms = self.clock.sample()?.unix_ms;
        let shared = &self.routine_summary;
        let offset_ms = |unix_ms: u64| shared.local_offset_ms(unix_ms);
        lock(&shared.memory, HostLocation::RoutineSummary)?.count(
            &SettledRun {
                instance_alias,
                task_id,
                end,
                settled_at_unix_ms,
            },
            &offset_ms,
        );
        Ok(())
    }
}

/// How a policy run or a direct run ended, as its terminal records it, from what its execution
/// returned: a completed run succeeded; a cancelled direct run carries its cancellation's code;
/// a failed run with a terminal its task failure's code (the run's own, kept when a cleanup
/// failure replaced the error), else its error's. `None` when no terminal was written.
fn routine_run_end(executed: &Result<OperationSuccess, RequestFailure>) -> Option<RunEnd> {
    match executed {
        Ok(OperationSuccess {
            result: RuntimeResult::ContainedTaskCompleted { .. },
            ..
        }) => Some(RunEnd::Succeeded),
        Ok(OperationSuccess {
            result: RuntimeResult::ContainedTaskCancelled { reason, .. },
            ..
        }) => Some(RunEnd::of_terminal(Some(cancelled_code(*reason)))),
        Ok(_) => None,
        Err(failure) if failure.terminal.is_some() => {
            let code = match &failure.task_failure {
                Some(evidence) => evidence.outcome.code().clone(),
                None => failure.error.outcome().code().clone(),
            };
            Some(RunEnd::of_terminal(Some(code)))
        }
        Err(_) => None,
    }
}

/// The code a cancelled direct run's terminal carries for `reason`, as the run's own check ends
/// it: its client's cancel `contained_task_cancelled`, its deadline
/// `contained_task_deadline_exceeded`, a pause's drain timeout `contained_task_paused`. A reason
/// no live run's control holds is that invalid state's code.
fn cancelled_code(reason: ContainedTaskCancellationReason) -> CodeStr {
    let code = match reason {
        ContainedTaskCancellationReason::ClientRequested => {
            RuntimeErrorCode::ContainedTaskCancelled.code()
        }
        ContainedTaskCancellationReason::DeadlineExceeded => {
            RuntimeErrorCode::ContainedTaskDeadlineExceeded.code()
        }
        ContainedTaskCancellationReason::PausedByOperator => {
            RuntimeErrorCode::ContainedTaskPaused.code()
        }
        ContainedTaskCancellationReason::RecoveredAfterRestart
        | ContainedTaskCancellationReason::BlackoutStop
        | ContainedTaskCancellationReason::PerformanceYield => {
            HostCode::ContainedTaskCancellationStateInvalid.code()
        }
    };
    code.into()
}

impl RuntimeHost {
    /// Test only (Workflow #429 R-12): the host's local zone for the routine summary,
    /// `offset_ms` east of UTC, in place of the machine's.
    #[cfg(test)]
    pub(crate) fn routine_summary_set_local_offset_for_test(
        &self,
        offset_ms: i64,
    ) -> RuntimeHostResult<()> {
        *self
            .shared_ref(HostLocation::RoutineSummary)?
            .routine_summary
            .local_offset_for_test
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(offset_ms);
        Ok(())
    }

    /// Test only (Workflow #429 R-12): one settled run of `instance_alias` and `task_id` that
    /// ended `end`, counted at the host clock's reading now through the function the run sites
    /// count with.
    #[cfg(test)]
    pub(crate) fn routine_summary_count_for_test(
        &self,
        instance_alias: &str,
        task_id: &str,
        end: RunEnd,
    ) -> RuntimeHostResult<()> {
        self.shared_ref(HostLocation::RoutineSummary)?
            .count_routine_run(instance_alias, task_id, end)
    }
}
