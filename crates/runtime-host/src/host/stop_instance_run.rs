// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #429 R-6, first slice (register §1.302), and R-6b2 (§1.314, §1.316; since 0.12.55;
//! `contracts/runtime-interface.md`, "Workbench request kinds", the row `StopInstanceRun`): a
//! person or an agent stops one run of one instance and the Runtime resets the device itself,
//! with the run's own lease, so the reset never hangs on the requesting client's connection.
//!
//! - **What it stops.** A client's direct run (`RunContainedTask`) and a policy run (a catalog
//!   task's dispatch, a fact refresh read included): asked to stop at its next checkpoint as a
//!   client's cancel asks it (`ClientRequested`), it ends `contained_task_cancelled`. A policy
//!   run's control is registered before its package's preparation, so a stop during the
//!   preparation reaches it too. Only a run the Runtime starts for itself (a startup package, a
//!   ladder rung, an idle action's rest) is refused `scheduled_contained_task_not_client_cancellable`
//!   with `run_origin` `host`. A refusal cancels nothing. A client's own `CancelContainedTask`
//!   still refuses every scheduled run.
//! - **The reset.** The run's end resets the device before its terminal and its lease end
//!   (`HostShared::reset_stopped_run`): an `Input` `reset` on the run's lease, recorded as every
//!   input is (`InputReset`, linked to the run). A reset that fails fences the run's instance only
//!   (#419) and the run's lease ends as a failed run's, handing the instance to no ladder; the
//!   stop answers `reset: failed` with the fence's failure as its receipt's cause. A run that
//!   ended without the stop's reset (it ended on its own before its check saw the stop, or its
//!   deadline or a pause ended it) needed none: `reset: not_needed`, as the client's own
//!   cancel-and-reset answers it.
//! - **The answer.** The request waits up to `wait_ms` for the run's end (the run left the
//!   registry: its terminal and its lease end written; a policy run's settlement record follows
//!   on its worker): then `settled: true` and `reset`; after `wait_ms`, `settled: false` and no
//!   `reset`, the stop staying asked. A later stop of the same run answers from the instance's
//!   latest stopped run (memory only), so a client that timed out reads the result on a new
//!   connection.
//! - **Records.** The request is a command (`command.received`, then `command.validated`, or
//!   `command.rejected` with its refusal); the run's own records, its reset among them, are the
//!   run's. A stop is no failure of the run's task: the routine summary counts it as an
//!   interruption, it starts no ladder and owes no rerun. A stopped policy run's terminal is the
//!   `task.failed` with `contained_task_cancelled` every scheduled cancel writes, and its
//!   `policy.execution_recorded` is of the kind `stopped`. A stop ends this run only: a task
//!   whose trigger still holds is dispatched again once its cooldown, counted from its dispatch,
//!   has passed; a closing row of the pack schedule or a pause keeps it from running.

use super::contained_task::{SCHEDULING_PAUSE_DRAIN_POLL_INTERVAL, StopReset};
use super::failure_scope::Contained;
use super::*;
use crate::failure_identity::identifier_text;
use actingcommand_contract::outcome::RunOrigin;
use actingcommand_contract::{InstanceRunReset, InstanceRunStop};
#[cfg(test)]
use std::sync::mpsc;

/// Each instance's latest stopped run (memory only; a restart starts with none). A leaf lock.
#[derive(Default)]
pub(super) struct StoppedRuns {
    latest: Mutex<BTreeMap<InstanceId, Arc<ContainedRunControl>>>,
    /// Test only: told the request id of each run a stop asked to end.
    #[cfg(test)]
    requested_for_test: Mutex<Option<mpsc::Sender<RequestId>>>,
}

impl HostShared {
    /// `StopInstanceRun` (Workflow #429 R-6): the run `run_request_id` of `instance_alias` is asked
    /// to stop and the request waits up to `wait_ms` for its end (see the module's description).
    pub(super) fn stop_instance_run(
        &self,
        original: &RuntimeRequest,
        validated: &ValidatedRuntimeRequest<'_>,
        instance_alias: &str,
        run_request_id: RequestId,
        wait_ms: u64,
    ) -> Result<OperationSuccess, RequestFailure> {
        let resolved = self.resolve_instance(instance_alias)?;
        let instance_id = resolved.instance_id();
        let links = self.append_client_command_intent(
            original,
            validated,
            instance_id,
            EventAction::RuntimeAction,
            None,
        )?;
        let (control, running) = match self.stop_target(instance_id, run_request_id)? {
            Ok(target) => target,
            Err(refusal) => return Err(self.refuse_stop(links, refusal)),
        };
        let accepted = self.append_event(
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
        if running {
            // A run already past its reset point ends on its own: the stop answers `not_needed`.
            control.request_runtime_stop();
            lock(&self.stopped_runs.latest, HostLocation::StopInstanceRun)?
                .insert(instance_id, Arc::clone(&control));
            #[cfg(test)]
            self.stopped_runs.note_requested_for_test(run_request_id)?;
        }
        let (stopped, reset_failure) = self.await_stopped_run(&control, wait_ms)?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(terminal(&accepted)),
            result: RuntimeResult::InstanceRunStopped { stopped },
            failure_outcome: reset_failure,
        })
    }

    /// The run a stop of `run_request_id` on `instance_id` takes, and whether it still runs: a
    /// registered run of the instance (a direct run or a policy run, Workflow #429 R-6b2), else
    /// the instance's latest stopped run when it is that one. Refused: a run the Runtime started
    /// for itself (`run_origin` `host`), and a run the instance neither runs nor stopped last.
    fn stop_target(
        &self,
        instance_id: InstanceId,
        run_request_id: RequestId,
    ) -> Result<Result<(Arc<ContainedRunControl>, bool), RuntimeHostError>, RequestFailure> {
        let active = lock(&self.contained_runs, HostLocation::StopInstanceRun)?
            .get(&run_request_id)
            .filter(|control| control.instance_id == instance_id)
            .cloned();
        if let Some(control) = active {
            return Ok(match control.stop_refused_origin() {
                Some(origin) => Err(not_client_stoppable(origin, run_request_id)),
                None => Ok((control, true)),
            });
        }
        let latest = lock(&self.stopped_runs.latest, HostLocation::StopInstanceRun)?
            .get(&instance_id)
            .filter(|control| control.request_id == run_request_id)
            .cloned();
        Ok(latest
            .map(|control| (control, false))
            .ok_or_else(|| stop_target_missing(run_request_id)))
    }

    /// Records the refusal `refusal` of a stop after its `command.received` (`links`) and returns
    /// it, Denied; nothing was asked of any run.
    fn refuse_stop(&self, links: EventLinksDraft, refusal: RuntimeHostError) -> RequestFailure {
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
                refusal.outcome(),
                AuditInput::new(),
            ),
        );
        match rejected {
            Ok(rejected) => RequestFailure::request(
                refusal,
                RuntimeReceiptState::Denied,
                Some(terminal(&rejected)),
            ),
            Err(failure) => failure,
        }
    }

    /// Waits up to `wait_ms` for the stopped run of `control` to end, as a pause's drain waits for
    /// its runs: the stop and, once the run is gone, how its reset went with a failed reset's
    /// failure. A shutdown answers what the stop knows then; a latched Runtime fails the request.
    fn await_stopped_run(
        &self,
        control: &Arc<ContainedRunControl>,
        wait_ms: u64,
    ) -> Result<(InstanceRunStop, Option<Outcome>), RequestFailure> {
        let deadline = Instant::now().checked_add(Duration::from_millis(wait_ms));
        let poll = SCHEDULING_PAUSE_DRAIN_POLL_INTERVAL;
        loop {
            if let Some(error) = self
                .fatal
                .current()
                .map_err(RequestFailure::poison_without_terminal)?
            {
                return Err(RequestFailure::poison_without_terminal(error));
            }
            let registered = lock(&self.contained_runs, HostLocation::StopInstanceRun)?
                .get(&control.request_id)
                .is_some_and(|current| Arc::ptr_eq(current, control));
            if !registered {
                let reset = control.stop_reset().unwrap_or_else(StopReset::not_needed);
                let stopped = InstanceRunStop {
                    request_id: control.request_id,
                    settled: true,
                    reset: Some(reset.reset),
                };
                return Ok((stopped, reset.failure));
            }
            let now = Instant::now();
            if self.fatal.is_shutdown_requested() || deadline.is_some_and(|until| now >= until) {
                let stopped = InstanceRunStop {
                    request_id: control.request_id,
                    settled: false,
                    reset: None,
                };
                return Ok((stopped, None));
            }
            let pause = match deadline {
                Some(until) => until.saturating_duration_since(now).min(poll),
                None => poll,
            };
            thread::sleep(pause);
        }
    }

    /// Workflow #429 R-6: the Runtime's own reset of the device after a run that a
    /// `StopInstanceRun` stopped (a client's direct run or, since R-6b2, a policy run), with the
    /// run's lease (`token`), before its terminal and its lease end: an `Input` `reset` linked to
    /// the run (`run_links`), recorded as every input is. A reset
    /// that fails is contained against the run's instance only (#419): the instance is fenced, and
    /// the stop answers `reset: failed` with the fence's failure. Returns whether the reset failed,
    /// so that the run's lease ends as a failed run's; a failure the Runtime does not survive (a
    /// ledger failure, a latched Runtime) ends the run with it.
    pub(super) fn reset_stopped_run(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        token: &LeaseToken,
        connection_id: ConnectionId,
        control: &ContainedRunControl,
        run_links: Option<RuntimeRunLinks>,
        execution_provenance: ExecutionBackendProvenance,
    ) -> Result<bool, RequestFailure> {
        let reset = self.input(
            request,
            token,
            &InputAction::Reset,
            connection_id,
            execution_provenance,
            RuntimeInputContext {
                run_links,
                ..RuntimeInputContext::default()
            },
        );
        let failure = match reset {
            Ok(_) => {
                control.note_stop_reset(StopReset {
                    reset: InstanceRunReset::Done,
                    failure: None,
                });
                return Ok(false);
            }
            Err(failure) => failure,
        };
        let links = self.events.request_links(
            request,
            Some(token.instance_id()),
            Some(token.lease_id()),
            None,
        );
        let links = match run_links {
            Some(run_links) => run_links.apply(links),
            None => links,
        };
        let contained = self
            .contain_failure(*failure.error, Some(token.instance_id()), links.clone())
            .map_err(RequestFailure::poison_without_terminal)?;
        let fence = match contained {
            Contained::Fenced(fence) => fence,
            // An operation's failure leaves the touch state unknown too: the instance is fenced
            // until a preparation proves it.
            Contained::Failed(error) => match self
                .fence_contained_failure(&error, token.instance_id(), links)
                .map_err(RequestFailure::poison_without_terminal)?
            {
                Some(fence) => fence,
                None => return Err(RequestFailure::poison_without_terminal(error.into_fatal())),
            },
            Contained::Latched(error) => {
                return Err(RequestFailure::poison_without_terminal(error));
            }
        };
        control.note_stop_reset(StopReset {
            reset: InstanceRunReset::Failed,
            failure: Some(fence.outcome()),
        });
        Ok(true)
    }
}

impl StoppedRuns {
    /// Test only: tells the receiver a test armed which run a stop asked to end.
    #[cfg(test)]
    fn note_requested_for_test(&self, run_request_id: RequestId) -> RuntimeHostResult<()> {
        let sender = lock(&self.requested_for_test, HostLocation::StopInstanceRun)?;
        if let Some(sender) = sender.as_ref() {
            let _ = sender.send(run_request_id);
        }
        Ok(())
    }
}

/// `StopInstanceRun`'s refusal of a run no client may stop, since Workflow #429 R-6b2 only a run
/// the Runtime started for itself: the run's request and who started it (`host`).
fn not_client_stoppable(origin: RunOrigin, run_request_id: RequestId) -> RuntimeHostError {
    RuntimeHostError::request(
        SharedCode::ScheduledContainedTaskNotClientCancellable,
        HostLocation::StopInstanceRun,
        RuntimeErrorCode::InvalidRequest,
    )
    .with_outcome_values(|outcome| {
        outcome
            .run_origin(origin)
            .request_id(identifier_text(&run_request_id))
    })
}

/// `StopInstanceRun`'s refusal of a run the instance neither runs nor stopped last.
fn stop_target_missing(run_request_id: RequestId) -> RuntimeHostError {
    RuntimeHostError::request(
        HostCode::ContainedTaskCancellationIdentityMissing,
        HostLocation::StopInstanceRun,
        RuntimeErrorCode::InvalidRequest,
    )
    .with_outcome_values(|outcome| outcome.request_id(identifier_text(&run_request_id)))
}

impl RuntimeHost {
    /// Workflow #429 R-6, tests: the request id of each run a stop asks to end from now on.
    #[cfg(test)]
    pub(crate) fn notify_run_stops_for_test(&self) -> RuntimeHostResult<mpsc::Receiver<RequestId>> {
        let (sender, receiver) = mpsc::channel();
        let shared = self.shared_ref(HostLocation::StopInstanceRun)?;
        let slot = &shared.stopped_runs.requested_for_test;
        *lock(slot, HostLocation::StopInstanceRun)? = Some(sender);
        Ok(receiver)
    }

    /// Workflow #429 R-6, tests: registers a run of `instance_id` under `request_id` that no
    /// client's own cancel may end, a host run (`host`) or a policy run, as such a run's start
    /// registers it. Since R-6b2 `StopInstanceRun` refuses the host run only.
    #[cfg(test)]
    pub(crate) fn begin_uncancellable_run_for_test(
        &self,
        request_id: RequestId,
        instance_id: InstanceId,
        host: bool,
    ) -> RuntimeHostResult<()> {
        let control =
            ContainedRunControl::not_client_cancellable_for_test(request_id, instance_id, host);
        let shared = self.shared_ref(HostLocation::StopInstanceRun)?;
        lock(&shared.contained_runs, HostLocation::StopInstanceRun)?
            .insert(request_id, Arc::new(control));
        Ok(())
    }

    /// Workflow #429 R-6, tests: takes the registered run `request_id` out of the registry and
    /// answers whether anything asked it to stop; `None` when no run is registered under it.
    #[cfg(test)]
    pub(crate) fn end_uncancellable_run_for_test(
        &self,
        request_id: RequestId,
    ) -> RuntimeHostResult<Option<bool>> {
        let shared = self.shared_ref(HostLocation::StopInstanceRun)?;
        Ok(lock(&shared.contained_runs, HostLocation::StopInstanceRun)?
            .remove(&request_id)
            .map(|control| control.stop_asked_for_test()))
    }
}
