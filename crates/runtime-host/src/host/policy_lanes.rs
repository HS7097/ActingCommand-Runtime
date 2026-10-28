// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #427-A (plan r2, comment 6460013989, points 1, 2, 5; Alice approved the issue on
//! 10-28): the scheduled policy runs of a round on the instance workers, one lane per instance.
//!
//! - **One evaluator.** The policy driver evaluates and admits on its own thread, as before. With
//!   `scheduling.parallel.enabled` on, a granted scheduled dispatch is handed, with its request, to
//!   its instance's worker (`actingd-instance-<alias>`, `host_claims.rs`) instead of being run on
//!   the driver's thread. The worker runs it and settles it exactly as the driver did
//!   (`run_scheduled_policy_task`, then `complete_scheduled_policy_run_recorded`) and reports the
//!   end to the driver's [`PolicyRunSink`]. One run per instance at a time; different instances
//!   at once.
//! - **One owner from hand-off to settlement (C2, C3).** The hand-off registers the run as in
//!   flight, by instance, with its decision id and a hand-off epoch, while its lease is still held.
//!   The registration ends only after the run's settlement (or its refusal's) is recorded. It is
//!   the one source for the instance's occupancy (the evaluator defers, and the admission refuses
//!   `dispatch_instance_held`, a new dispatch on a registered instance, also in the window
//!   "terminal written, lease released, not settled"), for the reconciliation (a run a worker
//!   still holds is skipped, never settled early or twice) and for a scheduling pause's and an
//!   install drain's wait.
//! - **Lifecycle.** The worker takes a continuation admission (`begin_continuation`) when it takes
//!   the run, and its counted work covers the run and the settlement. A run handed off before an
//!   install drain or a scheduling pause began goes ahead, as the synchronous round's run admitted
//!   before them does (fix 1, review L1): the drain waits for it (its registration and its counted
//!   work) and the pause drains it like any contained run admitted before its gate closed. Only a
//!   run not yet taken when the Runtime's stop was requested is refused explicitly: settled as not
//!   performed (`runtime_stopping`), its lease released and its registration ended.
//! - **Failures (C5, #419).** The run's failures go through the existing failure containment
//!   (`contain_failure`): an instance-level failure fences that instance and the worker reports a
//!   non-fatal failure; only a global fault latches the Runtime. A worker panic follows spec §7
//!   (`host_claims.rs`); the panicked run's registration is kept as abandoned, so its instance
//!   stays occupied. A run whose settlement failed without stopping the Runtime is left the same
//!   way. The online reconciliation settles an abandoned run by the start's rule once its lease
//!   ended (fix 1, review M1: `recover_abandoned_policy_runs`): from its terminal, or as
//!   interrupted, after recording the run-linked release the lease end left missing; then the
//!   registration is forgotten and the instance is free again.
//! - **One end.** A lane's registration is ended at most once: the job and what a panic leaves of
//!   it share an "ended" mark (fix 1, review L2).
//! - **No replay.** A run is posted once; nothing re-posts it after a lost receipt.

use super::*;
use crate::codes::HostCode;
use crate::codes::HostLocation;

/// Where an instance worker reports the end of a policy run the driver handed to it. Called on
/// the worker's thread once the run's registration has ended, with no host lock held.
pub trait PolicyRunSink: Send + Sync {
    fn settled(&self, settled: PolicyRunSettled);
}

/// A handed-off run that ran and whose scheduling result is settled, as the driver's own run
/// settles: what [`RuntimeHost::run_scheduled_contained_task`] and
/// [`RuntimeHost::complete_scheduled_policy_run`] return.
pub struct PolicyRunCompletion {
    pub context: Box<PolicyRunContext>,
    pub receipt: RuntimeReceipt,
    pub execution: PolicyExecutionEventData,
    pub projection: Option<SchedulingOutcomeProjection>,
}

/// How a handed-off policy run ended.
pub enum PolicyRunSettled {
    /// The run ran and its scheduling result is settled.
    Completed(Box<PolicyRunCompletion>),
    /// The run, its settlement or the worker's admission of it failed without stopping the
    /// Runtime. A failure went through the instance's failure containment; a run the worker
    /// refused (a stopping host) was settled as not performed.
    Failed {
        context: Box<PolicyRunContext>,
        error: Box<RuntimeHostError>,
    },
    /// The worker panicked while it held the run (spec §7): the run was dropped, not retried; its
    /// lease lapses at its TTL and the reconciliation then settles it (fix 1, review M1).
    Dropped {
        instance_alias: String,
        decision_id: String,
    },
}

/// Workflow #427-A, test builds and the `test-hooks` feature: the points of a handed-off run at
/// which a test hook runs, on the worker's thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyLanePoint {
    /// The worker took the run from its channel, before its lifecycle admission.
    Taken,
    /// The run returned its receipt (its terminal is written and its lease released) and its
    /// settlement has not started.
    Ran,
}

/// A test hook of the lanes: called at each [`PolicyLanePoint`] with the run's instance alias.
#[cfg(any(test, feature = "test-hooks"))]
pub type PolicyLaneHook = Arc<dyn Fn(PolicyLanePoint, &str) + Send + Sync>;

/// One handed-off policy run, as its instance's worker receives it.
pub(super) struct PolicyRunJob {
    context: Box<PolicyRunContext>,
    request: ContainedTaskRequest,
    sink: Arc<dyn PolicyRunSink>,
    epoch: u64,
    /// Fix 1 (review L2): set once the run's registration has ended; shared with what a panic
    /// leaves of the job, so a panic after the end never ends it a second time.
    ended: Arc<AtomicBool>,
}

/// What a panicked policy run leaves behind: its registration, its sink and its "ended" mark.
pub(super) struct PolicyLane {
    instance_id: InstanceId,
    instance_alias: String,
    decision_id: String,
    epoch: u64,
    sink: Arc<dyn PolicyRunSink>,
    ended: Arc<AtomicBool>,
}

impl PolicyRunJob {
    pub(super) fn context(&self) -> &PolicyRunContext {
        &self.context
    }

    pub(super) fn lane(&self) -> PolicyLane {
        PolicyLane {
            instance_id: self.context.lease_token().instance_id(),
            instance_alias: self.context.instance_alias().to_owned(),
            decision_id: self.context.decision_id().to_owned(),
            epoch: self.epoch,
            sink: Arc::clone(&self.sink),
            ended: Arc::clone(&self.ended),
        }
    }
}

/// The in-flight policy runs, one per instance at most.
#[derive(Default)]
pub(super) struct PolicyRunsInFlight {
    runs: BTreeMap<InstanceId, InFlightPolicyRun>,
    /// The last hand-off epoch given out; each registration takes the next one.
    last_epoch: u64,
}

struct InFlightPolicyRun {
    decision_id: String,
    epoch: u64,
    /// No worker holds the run any more (a panic, a settlement that failed without stopping the
    /// Runtime): the instance stays occupied until the reconciliation settles the run, once its
    /// lease ended (fix 1, review M1), and forgets the registration.
    abandoned: bool,
}

/// A registration conflict: a second run on an instance, or the end of a registration that is
/// not the one the run took. It can only come from a broken invariant, so it latches.
fn lane_conflict(location: HostLocation) -> RuntimeHostError {
    RuntimeHostError::fatal(
        HostCode::PolicyInstanceDispatchConflict,
        location,
        RuntimeErrorCode::RuntimeHalted,
    )
}

impl HostShared {
    /// Workflow #427-A: hands the admitted scheduled run `context` with its `request` to its
    /// instance's worker, registered as in flight. A hand-off that no worker takes (a stopping
    /// host, an ended worker) is settled as not performed, its lease and registration released,
    /// and its refusal returned: non-fatal when the host is stopping, otherwise through the
    /// instance's failure containment.
    pub(super) fn hand_off_scheduled_policy_run(
        &self,
        context: Box<PolicyRunContext>,
        request: ContainedTaskRequest,
        sink: Arc<dyn PolicyRunSink>,
    ) -> RuntimeHostResult<()> {
        // A context of another owner epoch is never this host's to run (as the run's own check).
        if context.lease_token().owner_epoch() != self.owner_epoch {
            return Err(RuntimeHostError::fatal(
                HostCode::PolicyRunIdentityMismatch,
                HostLocation::HandOffScheduledPolicyRun,
                RuntimeErrorCode::RuntimeHalted,
            ));
        }
        let instance_id = context.lease_token().instance_id();
        let epoch = self.register_policy_run(&context)?;
        let job = Box::new(PolicyRunJob {
            context,
            request,
            sink,
            epoch,
            ended: Arc::new(AtomicBool::new(false)),
        });
        let Err(job) = self.post_policy_run(instance_id, job)? else {
            return Ok(());
        };
        let refusal = if self.fatal.is_shutdown_requested() {
            RuntimeHostError::request(
                HostCode::RuntimeStopping,
                HostLocation::HandOffScheduledPolicyRun,
                RuntimeErrorCode::RuntimeUnavailable,
            )
        } else {
            RuntimeHostError::fatal(
                HostCode::InstanceWorkerUnavailable,
                HostLocation::HandOffScheduledPolicyRun,
                RuntimeErrorCode::RuntimeHalted,
            )
        };
        let error = self.refuse_policy_run(job.context(), job.epoch, &job.ended, refusal)?;
        Err(error)
    }

    /// Workflow #427-A: one handed-off run on its instance's worker. `Err` only for a failure
    /// that latched the Runtime; every other end is reported to the run's sink.
    pub(super) fn run_handed_policy_run(&self, job: PolicyRunJob) -> RuntimeHostResult<()> {
        let PolicyRunJob {
            context,
            request,
            sink,
            epoch,
            ended,
        } = job;
        self.policy_lane_point(PolicyLanePoint::Taken, &context);
        // Fix 1 (review L1): a run handed off before an install drain or a scheduling pause began
        // goes ahead, as the synchronous round's run admitted before them does, and is never
        // settled as a failure for it: the drain waits for the run (its registration and the
        // counted work below), and the pause drains it (`release_paused_instance` waits for the
        // lane, then drains its contained run under the pause's deadlines). The continuation
        // admission is closed outside a drain only while a start prepares or an install holds the
        // Runtime, which a registered run never meets (a drain ends only once no lane works); it is
        // waited for there, as a granted host claim waits. A run not yet taken when the Runtime's
        // stop was requested is refused: settled as not performed, never started.
        let _work = loop {
            if self.fatal.is_shutdown_requested() {
                let refusal = RuntimeHostError::request(
                    HostCode::RuntimeStopping,
                    HostLocation::RunHandedPolicyRun,
                    RuntimeErrorCode::RuntimeUnavailable,
                );
                let error = Box::new(self.refuse_policy_run(&context, epoch, &ended, refusal)?);
                sink.settled(PolicyRunSettled::Failed { context, error });
                return Ok(());
            }
            if let Some(admitted) = self.begin_continuation()? {
                break admitted;
            }
            thread::sleep(super::host_claims::HOST_CLAIM_ADMISSION_POLL);
        };
        let instance_id = context.lease_token().instance_id();
        let decision_id = context.decision_id().to_owned();
        let settled = match self.run_scheduled_policy_task(&context, &request) {
            Ok(receipt) => {
                self.policy_lane_point(PolicyLanePoint::Ran, &context);
                match self.complete_scheduled_policy_run_recorded(&context, &receipt) {
                    Ok((execution, projection)) => {
                        PolicyRunSettled::Completed(Box::new(PolicyRunCompletion {
                            context,
                            receipt,
                            execution,
                            projection,
                        }))
                    }
                    Err(error) if !error.is_fatal() => PolicyRunSettled::Failed {
                        context,
                        error: Box::new(error),
                    },
                    Err(error) => {
                        self.end_lane(instance_id, &decision_id, epoch, &ended)?;
                        return Err(error);
                    }
                }
            }
            Err(error) if !error.is_fatal() => PolicyRunSettled::Failed {
                context,
                error: Box::new(error),
            },
            Err(error) => {
                self.end_lane(instance_id, &decision_id, epoch, &ended)?;
                return Err(error);
            }
        };
        self.end_lane(instance_id, &decision_id, epoch, &ended)?;
        sink.settled(settled);
        Ok(())
    }

    /// Fix 1 (review L2): ends the lane's registration once. A lane already ended (its run
    /// settled, then a later step panicked) is not ended again, so the panic path never meets a
    /// registration that is gone (`lane_conflict`).
    fn end_lane(
        &self,
        instance_id: InstanceId,
        decision_id: &str,
        epoch: u64,
        ended: &AtomicBool,
    ) -> RuntimeHostResult<()> {
        if ended.load(Ordering::Acquire) {
            return Ok(());
        }
        self.end_policy_run(instance_id, decision_id, epoch)?;
        ended.store(true, Ordering::Release);
        Ok(())
    }

    /// Workflow #427-A: settles a handed-off run that never started as not performed (its lease
    /// released by the failure path), ends its registration and returns the refusal, a fatal one
    /// through the instance's failure containment. Counted as cleanup work, which a stop or a
    /// drain admits.
    fn refuse_policy_run(
        &self,
        context: &PolicyRunContext,
        epoch: u64,
        ended: &AtomicBool,
        refusal: RuntimeHostError,
    ) -> RuntimeHostResult<RuntimeHostError> {
        let _cleanup = self.work_guard()?;
        self.record_scheduled_policy_failure(context, &refusal)?;
        let failure = RequestFailure::request(refusal.clone(), RuntimeReceiptState::Denied, None);
        let settled = self.complete_scheduled_policy_failure(context, &failure);
        let mut error = refusal;
        if error.is_fatal() {
            let links = self.policy_run_event_links(context)?;
            error = match self.contain_failure(
                error,
                Some(context.lease_token().instance_id()),
                links,
            )? {
                failure_scope::Contained::Latched(error)
                | failure_scope::Contained::Fenced(error)
                | failure_scope::Contained::Failed(error) => error,
            };
        }
        self.end_lane(
            context.lease_token().instance_id(),
            context.decision_id(),
            epoch,
            ended,
        )?;
        match settled {
            Ok(_) => Ok(error),
            Err(settlement) if !settlement.is_fatal() => {
                self.record_scheduled_policy_failure(context, &settlement)?;
                Ok(settlement)
            }
            Err(settlement) => Err(settlement),
        }
    }

    /// Workflow #427-A: what a worker panic leaves of a policy run: its registration is kept as
    /// abandoned (its instance stays occupied until the reconciliation settles the run, after its
    /// lease lapsed at its TTL: fix 1, review M1), and the driver is woken. A lane the job had
    /// already ended (a panic after its settlement) is not ended again (fix 1, review L2).
    pub(super) fn drop_panicked_policy_run(&self, lane: PolicyLane) -> RuntimeHostResult<()> {
        let PolicyLane {
            instance_id,
            instance_alias,
            decision_id,
            epoch,
            sink,
            ended,
        } = lane;
        self.end_lane(instance_id, &decision_id, epoch, &ended)?;
        sink.settled(PolicyRunSettled::Dropped {
            instance_alias,
            decision_id,
        });
        Ok(())
    }

    /// Workflow #427-A: registers `context`'s run as in flight on its instance and returns its
    /// hand-off epoch. A second run on an instance latches (`policy_instance_dispatch_conflict`).
    fn register_policy_run(&self, context: &PolicyRunContext) -> RuntimeHostResult<u64> {
        let instance_id = context.lease_token().instance_id();
        let registered = (|| {
            let mut runs = lock(
                &self.policy_runs_in_flight,
                HostLocation::UpdatePolicyRunsInFlight,
            )?;
            if runs.runs.contains_key(&instance_id) {
                return Err(lane_conflict(HostLocation::UpdatePolicyRunsInFlight));
            }
            let epoch = runs
                .last_epoch
                .checked_add(1)
                .ok_or_else(|| lane_conflict(HostLocation::UpdatePolicyRunsInFlight))?;
            runs.last_epoch = epoch;
            runs.runs.insert(
                instance_id,
                InFlightPolicyRun {
                    decision_id: context.decision_id().to_owned(),
                    epoch,
                    abandoned: false,
                },
            );
            Ok(epoch)
        })();
        if let Err(error) = &registered {
            self.fatal.mark(error.clone())?;
        }
        registered
    }

    /// Workflow #427-A: ends the registration `(instance_id, decision_id, epoch)`. A run whose
    /// dispatch is settled is forgotten; one still unsettled stays registered as abandoned, for
    /// the reconciliation. A registration that is gone or another's latches: a late end can only
    /// come from a broken invariant, and it changes nothing.
    fn end_policy_run(
        &self,
        instance_id: InstanceId,
        decision_id: &str,
        epoch: u64,
    ) -> RuntimeHostResult<()> {
        let unsettled = lock(self.policy()?, HostLocation::ReadPolicyRunsInFlight)?
            .dispatch_needs_completion(decision_id)
            .map_err(RuntimeHostError::into_fatal)?;
        let ended = (|| {
            let mut runs = lock(
                &self.policy_runs_in_flight,
                HostLocation::UpdatePolicyRunsInFlight,
            )?;
            match runs.runs.get_mut(&instance_id) {
                Some(run) if run.decision_id == decision_id && run.epoch == epoch => {
                    if unsettled {
                        run.abandoned = true;
                    } else {
                        runs.runs.remove(&instance_id);
                    }
                    Ok(())
                }
                _ => Err(lane_conflict(HostLocation::UpdatePolicyRunsInFlight)),
            }
        })();
        if let Err(error) = &ended {
            self.fatal.mark(error.clone())?;
        }
        ended
    }

    /// Workflow #427-A: whether a policy run is registered on `instance_id`, in flight or
    /// abandoned: the instance is occupied for the evaluation and the admission.
    pub(super) fn policy_run_registered(&self, instance_id: InstanceId) -> RuntimeHostResult<bool> {
        Ok(lock(
            &self.policy_runs_in_flight,
            HostLocation::ReadPolicyRunsInFlight,
        )?
        .runs
        .contains_key(&instance_id))
    }

    /// Workflow #427-A: whether a worker still holds a policy run on `instance_id` (registered
    /// and not abandoned): a scheduling pause of the instance waits for it.
    pub(super) fn policy_run_working(&self, instance_id: InstanceId) -> RuntimeHostResult<bool> {
        Ok(lock(
            &self.policy_runs_in_flight,
            HostLocation::ReadPolicyRunsInFlight,
        )?
        .runs
        .get(&instance_id)
        .is_some_and(|run| !run.abandoned))
    }

    /// Workflow #427-A: whether a worker still holds any policy run: an install drain and a
    /// shutdown decision wait for it.
    pub(super) fn any_policy_run_working(&self) -> RuntimeHostResult<bool> {
        Ok(lock(
            &self.policy_runs_in_flight,
            HostLocation::ReadPolicyRunsInFlight,
        )?
        .runs
        .values()
        .any(|run| !run.abandoned))
    }

    /// Workflow #427-A: the decisions a worker still holds: the reconciliation skips them.
    pub(super) fn policy_runs_held_by_workers(&self) -> RuntimeHostResult<BTreeSet<String>> {
        Ok(lock(
            &self.policy_runs_in_flight,
            HostLocation::ReadPolicyRunsInFlight,
        )?
        .runs
        .values()
        .filter(|run| !run.abandoned)
        .map(|run| run.decision_id.clone())
        .collect())
    }

    /// Workflow #427-A fix 1 (review M1): the decisions of the abandoned registrations (no worker
    /// holds the run any more), which the reconciliation settles once their lease ended
    /// (`recover_abandoned_policy_runs`).
    pub(super) fn abandoned_policy_runs(&self) -> RuntimeHostResult<Vec<String>> {
        Ok(lock(
            &self.policy_runs_in_flight,
            HostLocation::ReadPolicyRunsInFlight,
        )?
        .runs
        .values()
        .filter(|run| run.abandoned)
        .map(|run| run.decision_id.clone())
        .collect())
    }

    /// Workflow #427-A: forgets every abandoned registration whose dispatch is settled (by the
    /// reconciliation, under the caller's policy guard).
    pub(super) fn forget_settled_abandoned_policy_runs(
        &self,
        policy: &PolicyHost,
    ) -> RuntimeHostResult<()> {
        let mut runs = lock(
            &self.policy_runs_in_flight,
            HostLocation::UpdatePolicyRunsInFlight,
        )?;
        let mut settled = Vec::new();
        for (instance_id, run) in &runs.runs {
            if run.abandoned
                && !policy
                    .dispatch_needs_completion(&run.decision_id)
                    .map_err(RuntimeHostError::into_fatal)?
            {
                settled.push(*instance_id);
            }
        }
        for instance_id in settled {
            runs.runs.remove(&instance_id);
        }
        Ok(())
    }

    /// Workflow #427-A: the factored body of [`RuntimeHost::run_scheduled_contained_task`], which
    /// the driver's own run and a worker's share: the run, and on its failure the same-run
    /// settlement and the failure containment (ISO-M #419 R0.5, ISO-2).
    pub(super) fn run_scheduled_policy_task(
        &self,
        context: &PolicyRunContext,
        request: &ContainedTaskRequest,
    ) -> RuntimeHostResult<RuntimeReceipt> {
        let (request, success) = match self.run_scheduled_contained_task(context, request) {
            Ok(success) => success,
            Err(failure) => {
                self.record_scheduled_policy_failure(context, &failure.error)?;
                let settlement = self.complete_scheduled_policy_failure(context, &failure);
                let mut error = *failure.error;
                // ISO-M (#419) R0.5, ISO-2: the scheduled run's boundary. A failure that would
                // stop the Runtime (a poison, or a fatal failure actingd's driver stops on) goes
                // to the one decision point against the run's instance: only a `Global` failure
                // latches; an `Instance` one fences that instance, and the run fails with the
                // fence's non-fatal failure, so the driver goes on with the other instances.
                if failure.poison_runtime || error.is_fatal() {
                    let links = self.policy_run_event_links(context)?;
                    error = match self.contain_failure(
                        error,
                        Some(context.lease_token().instance_id()),
                        links,
                    )? {
                        failure_scope::Contained::Latched(error) => error,
                        failure_scope::Contained::Fenced(error)
                        | failure_scope::Contained::Failed(error) => error,
                    };
                }
                if let Err(settlement_error) = settlement {
                    self.record_scheduled_policy_failure(context, &settlement_error)?;
                    return Err(settlement_error);
                }
                return Err(error);
            }
        };
        RuntimeReceipt::success(&request, success.state, success.terminal, success.result)
            .map_err(|_| receipt_error())
    }

    /// Workflow #427-A: the factored body of [`RuntimeHost::complete_scheduled_policy_run`]: the
    /// settlement, with a failure recorded.
    pub(super) fn complete_scheduled_policy_run_recorded(
        &self,
        context: &PolicyRunContext,
        receipt: &RuntimeReceipt,
    ) -> RuntimeHostResult<(
        PolicyExecutionEventData,
        Option<SchedulingOutcomeProjection>,
    )> {
        let result = self.complete_scheduled_policy_run(context, receipt);
        if let Err(error) = &result {
            self.record_scheduled_policy_failure(context, error)?;
        }
        result
    }

    #[cfg(any(test, feature = "test-hooks"))]
    fn policy_lane_point(&self, point: PolicyLanePoint, context: &PolicyRunContext) {
        let hook = self
            .policy_lane_hook_for_test
            .lock()
            .ok()
            .and_then(|slot| slot.clone());
        if let Some(hook) = hook {
            hook(point, context.instance_alias());
        }
    }

    #[cfg(not(any(test, feature = "test-hooks")))]
    #[allow(clippy::unused_self)]
    fn policy_lane_point(&self, _point: PolicyLanePoint, _context: &PolicyRunContext) {}
}
