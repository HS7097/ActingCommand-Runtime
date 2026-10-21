// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #403: an unconfirmed close of one instance's own resources fences that instance
//! instead of ending the whole Runtime.
//!
//! The fenced instance gets no new dispatch: its policy availability is withheld, a policy
//! admission is refused with `instance_resources_fenced`, and the routine gate holds its routine
//! claims until the fence is handed on. The lease sweep hands every new fence to a recovery
//! ladder admitted as every ladder is (`resource_fence` trigger), or, when none may start, to a
//! person at once; a ladder that ends with the instance still fenced leaves it waiting for a
//! person. A fence that manual work met (a direct task run, a client lease, an application or
//! input command, a Lab operation: the instance's key held by a person's claim) starts no ladder
//! and waits for a person at once (M387 B1, Alice 10-10: the ladder exists for the routine),
//! unless `recovery_ladder.rules.direct_runs` is `trigger` (Workflow #387 D1c).
//! Only a preparation of the instance whose self-check passes proves the close and lifts the
//! fence (`instance_resources_fence_lifted`): the ladder's re-preparation after an
//! application or emulator restart, or a person's resume, emulator start or restart, or
//! self-check. The kernel forgets its record of the unconfirmed close once the instance is
//! fenced, so that such a preparation can open a fresh session, and the owner journal keeps
//! the owner's resources `in_use` while any fence holds instead of retaining the whole owner:
//! a restart prepares and self-checks the fenced instance again, as every start does, and no
//! other instance is affected.
//!
//! An unconfirmed failure that names no registered instance, or that comes with a ledger
//! failure, still retains the owner and ends the whole Runtime, as before. So does every
//! unconfirmed failure once the Runtime is shutting down (a requested shutdown, or after a
//! fatal): the fence would end with the process before a ladder or a person could take it, and
//! forgetting the kernel's record of the close would drop it from the shutdown's own owner
//! retention, so the owner journal would not say `unconfirmed`.
//!
//! #411 A46 (10-21 incident): a command of one registered instance's own application backend
//! that fails with a fatal device error (`application_backend_operation_failed`), its resources
//! not left unconfirmed, fences that instance the same way, caused by that failure
//! (`fence_application_backend_failure`). Before, the failure kept its fatal projection, a
//! ladder rung returned it to the instance's worker, and the worker marked the whole Runtime
//! fatal: the host stopped, and every start that met the same instance stopped again.
//!
//! #411 A38 (10-20 incident: a capture channel that could not open for a moment fenced an
//! instance, its ladder's emulator restart timed out, and the instance waited two hours for a
//! person although one self-check would have passed at once): a fence that waits for a person
//! records `instance_resources_fence_awaiting_person` and runs the instance's self-check
//! periodically (the recheck). Every `recovery_ladder.fence_recheck.interval_ms` after the
//! wait began, or after a failed recheck ended, the lease sweep queues one self-check claim on
//! the instance's key (`recheck_resource_fences`), at most `max_attempts` per wait (`0`: never).
//! The instance's worker runs the same preparation and self-check as a person's
//! `actingctl selfcheck` (no game input, no application launch, no emulator start), as the
//! Runtime's own recovery preparation: it starts no ladder, and direction ① counts none of its
//! records. A passing recheck lifts the fence as a person's self-check does, its lift record
//! carrying `instance_resources_fence_recheck_passed` with the attempt; a failing one leaves the
//! instance fenced and waiting (`instance_resources_fence_recheck_failed`). After the last one
//! the instance waits for a person with no further recheck. `Status` shows the wait as
//! `resource_fence.person_wait`.

use super::host_claims::{HostClaimWork, HostKey};
use super::lease::HostClaim;
use super::*;
use crate::codes::{HostCode, HostLocation};
use crate::failure_identity::identifier_text;
use crate::time::RuntimeClockSample;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::{CodeStr, Outcome};
use actingcommand_contract::{InstanceFencePersonWait, InstanceResourceFence, LeasePriority};
use actingcommand_runtime_config::FenceRecheckConfig;
use std::collections::btree_map::Entry;

/// Where a fence stands.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FenceStage {
    /// Fenced; the lease sweep has not handed it to a ladder or a person yet (nor has the end
    /// of a ladder that held the instance when it was fenced).
    Pending,
    /// A recovery ladder holds or waits for the instance; its end decides.
    Ladder,
    /// No ladder proved the close: the instance waits for a person.
    Person,
}

/// One instance's resource fence (memory only).
pub(super) struct ResourceFence {
    /// The code of the unconfirmed failure that fenced the instance.
    cause: CodeStr,
    since_unix_ms: u64,
    stage: FenceStage,
    /// #411 A38: present exactly while the stage is `Person`.
    person_wait: Option<PersonWait>,
}

/// #411 A38: a fence's wait for a person and its periodic self-check (memory only).
struct PersonWait {
    /// When the wait began.
    since_unix_ms: u64,
    /// When the interval to the next recheck began: the wait's start, then the end of each
    /// failed recheck (the host clock's monotonic reading and Unix time).
    from: RuntimeClockSample,
    /// The rechecks started in this wait.
    attempts: u32,
    recheck: Recheck,
}

/// #411 A38: where a fence's recheck stands.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Recheck {
    /// None is queued or running.
    Idle,
    /// Its self-check claim waits for the instance's key, or its worker has not begun.
    Queued,
    /// Its preparation runs on the instance's key: a lift now is the recheck's.
    Preparing,
}

impl PersonWait {
    fn new(now: RuntimeClockSample) -> Self {
        Self {
            since_unix_ms: now.unix_ms,
            from: now,
            attempts: 0,
            recheck: Recheck::Idle,
        }
    }

    /// When the next recheck is due under `config` (monotonic, Unix): none while one is queued
    /// or runs, once `max_attempts` have started, or while the recheck is off.
    fn next_due(&self, config: FenceRecheckConfig) -> Option<RuntimeClockSample> {
        (self.recheck == Recheck::Idle && self.attempts < config.max_attempts).then(|| {
            RuntimeClockSample {
                monotonic_ms: self.from.monotonic_ms.saturating_add(config.interval_ms),
                unix_ms: self.from.unix_ms.saturating_add(config.interval_ms),
            }
        })
    }

    /// The wait as `Status` reports it.
    fn status(&self, config: FenceRecheckConfig) -> InstanceFencePersonWait {
        InstanceFencePersonWait {
            since_unix_ms: self.since_unix_ms,
            recheck_attempts: self.attempts,
            recheck_max_attempts: config.max_attempts,
            next_recheck_unix_ms: self.next_due(config).map(|due| due.unix_ms),
        }
    }
}

/// #411 A38: one recheck of a fence that waits for a person, on a self-check claim of its
/// instance's queue (no new claim kind).
pub(super) struct PendingFenceRecheck {
    pub(super) instance_id: InstanceId,
    /// The recheck's number in its wait.
    pub(super) attempt: u32,
    /// The instance and the recheck's causation: what a worker failure is recorded under.
    pub(super) links: EventLinksDraft,
}

/// #411 A38: `outcome` with `failure_code` = the registered code `spelling` names; a spelling
/// the embedded catalog does not list leaves the value out.
fn with_failure_code(outcome: Outcome, spelling: Option<&str>) -> Outcome {
    let registered = spelling.and_then(|spelling| {
        let relayed = Outcome::relayed(spelling);
        if relayed.code().as_str() == spelling {
            relayed.code().registered()
        } else {
            None
        }
    });
    match registered {
        Some(code) => outcome.failure_code(code),
        None => outcome,
    }
}

/// What an unconfirmed failure led to (`HostShared::settle_unconfirmed_resources`).
pub(super) enum UnconfirmedResources {
    /// The failure is not an unconfirmed one: the caller goes on as before.
    Confirmed,
    /// The failure's instance is fenced (now or already): the caller ends its work with this
    /// non-fatal failure instead of the unconfirmed one.
    Fenced(RuntimeHostError),
    /// No single registered instance owns the resources: the owner is retained and the Runtime
    /// is marked fatal, as before; the caller ends with the fatal failure.
    Retained,
}

/// The non-fatal failure of work that met `instance_id`'s fence, and the refusal of new work
/// while it holds: `instance_resources_fenced` with the instance, projected on a receipt as
/// `runtime_code`.
pub(super) fn resource_fence_error(
    instance_id: InstanceId,
    runtime_code: RuntimeErrorCode,
) -> RuntimeHostError {
    let mut error = RuntimeHostError::request(
        HostCode::InstanceResourcesFenced,
        HostLocation::FenceInstanceResources,
        runtime_code,
    )
    .with_outcome_values(|outcome| outcome.instance_id(identifier_text(&instance_id)));
    error.lifecycle.instance_id = Some(instance_id);
    error
}

/// M387 B1: whether a key of `kind` is held by a person's manual work (a direct task run, a
/// client lease, an application or input command, a Lab operation), whose fence starts no ladder
/// and whose records direction ① does not count (Workflow #387 D1c-H) while
/// `rules.direct_runs` is `exempt`.
pub(super) fn manual_claim(kind: ClaimKind) -> bool {
    matches!(
        kind,
        ClaimKind::DirectTaskRun
            | ClaimKind::ClientLease
            | ClaimKind::InputLifecycle
            | ClaimKind::LabOperation
    )
}

/// The receipt projection of a fence that `error` caused: the kind of failure it was (an open,
/// an operation or a capture), never the whole Runtime's.
fn fenced_projection_code(error: &RuntimeHostError) -> RuntimeErrorCode {
    match error.projection().code {
        RuntimeErrorCode::RuntimeHalted | RuntimeErrorCode::LedgerFailure => {
            RuntimeErrorCode::BackendOperationFailed
        }
        code => code,
    }
}

impl RequestFailure {
    /// This failure ended by `instance_id`'s fence: the fence's non-fatal failure. A task
    /// failure the run already committed stays its evidence; otherwise a scheduled run commits
    /// the fence as its task failure.
    pub(super) fn fenced(self, fence: RuntimeHostError) -> Self {
        Self {
            state: self.state,
            terminal: self.terminal,
            task_failure: self.task_failure.or_else(|| {
                Some(TaskFailureEvidence {
                    outcome: fence.outcome(),
                    severe: false,
                })
            }),
            error: Box::new(fence),
            poison_runtime: false,
        }
    }

    /// A failure that the fence `fence` ended, with no terminal of its own.
    pub(super) fn fence_without_terminal(fence: RuntimeHostError) -> Self {
        Self {
            state: RuntimeReceiptState::Failed,
            terminal: None,
            task_failure: Some(TaskFailureEvidence {
                outcome: fence.outcome(),
                severe: false,
            }),
            error: Box::new(fence),
            poison_runtime: false,
        }
    }
}

impl HostShared {
    /// Workflow #403: settles a failure whose resources may still be live. Not unconfirmed:
    /// `Confirmed`. Unconfirmed resources of one registered instance (the failure's own
    /// instance, else `instance_id`, the instance whose work met it) fence that instance: one
    /// `runtime.failed` record of `instance_resources_fenced` caused by the failure when it is
    /// newly fenced, its policy availability withheld, and the kernel's record of the close
    /// forgotten. Anything else, and a ledger failure, retains the owner and marks the Runtime
    /// fatal as before.
    pub(super) fn settle_unconfirmed_resources(
        &self,
        error: &RuntimeHostError,
        instance_id: Option<InstanceId>,
        links: EventLinksDraft,
    ) -> RuntimeHostResult<UnconfirmedResources> {
        if error.lifecycle.resource_quiescence != Some(ResourceQuiescence::Unconfirmed) {
            return Ok(UnconfirmedResources::Confirmed);
        }
        // During shutdown a fence would outlive nothing: the whole Runtime retains its owner,
        // as before.
        if self.fatal.is_shutdown_requested() {
            return self.retain_unconfirmed_owner(error, links);
        }
        let Some(instance_id) = self.fence_owner(error, instance_id)? else {
            return self.retain_unconfirmed_owner(error, links);
        };
        self.fence_instance(instance_id, error, links)
            .map(UnconfirmedResources::Fenced)
    }

    /// #411 A46: settles a failed command of `instance_id`'s own application backend whose
    /// resources were not left unconfirmed (`settle_unconfirmed_resources` found it
    /// `Confirmed`). A fatal `application_backend_operation_failed` of one registered instance
    /// fences that instance, as an unconfirmed close does: one `runtime.failed` record of
    /// `instance_resources_fenced` caused by the failure when it is newly fenced, its policy
    /// availability withheld; the caller ends its work with the returned non-fatal failure
    /// instead of the fatal one, and the fence's existing rules hand the instance on (to the
    /// ladder that holds it, a new ladder, or a person). `None`: any other failure, a failure
    /// that names no registered instance, or a ledger failure, which the caller ends with as
    /// before.
    pub(super) fn fence_application_backend_failure(
        &self,
        error: &RuntimeHostError,
        instance_id: InstanceId,
        links: EventLinksDraft,
    ) -> RuntimeHostResult<Option<RuntimeHostError>> {
        if !error.is_fatal()
            || error.code() != SharedCode::ApplicationBackendOperationFailed.as_str()
        {
            return Ok(None);
        }
        let Some(instance_id) = self.fence_owner(error, Some(instance_id))? else {
            return Ok(None);
        };
        self.fence_instance(instance_id, error, links).map(Some)
    }

    /// The instance a fence for `error` takes: the failure's own instance, else `instance_id`
    /// (the instance whose work met it), when it is registered and the failure is no ledger
    /// failure.
    fn fence_owner(
        &self,
        error: &RuntimeHostError,
        instance_id: Option<InstanceId>,
    ) -> RuntimeHostResult<Option<InstanceId>> {
        let owner = error.lifecycle.instance_id.or(instance_id);
        let registered = match owner {
            Some(instance_id) => lock(
                &self.registered_instances,
                HostLocation::FenceInstanceResources,
            )?
            .contains_key(&instance_id),
            None => false,
        };
        Ok(owner
            .filter(|_| registered && error.projection().code != RuntimeErrorCode::LedgerFailure))
    }

    /// Fences `instance_id` for `error` (Workflow #403; #411 A46): the fence entered unless one
    /// holds already, the kernel's record of an unconfirmed close of the instance forgotten (none
    /// is kept for a confirmed failure), and, when the instance is newly fenced, one
    /// `runtime.failed` record of `instance_resources_fenced` caused by `error` and the
    /// instance's policy availability withheld. Returns the fence's non-fatal failure.
    fn fence_instance(
        &self,
        instance_id: InstanceId,
        error: &RuntimeHostError,
        links: EventLinksDraft,
    ) -> RuntimeHostResult<RuntimeHostError> {
        let now = self.runtime_clock_sample()?;
        let since_unix_ms = now.unix_ms;
        // M387 B1: manual work that met the fence starts no ladder; its instance waits for a
        // person at once. Workflow #387 D1c (coordinator ruling 4): `rules.direct_runs` decides
        // it, so under `trigger` such a fence goes to a ladder as a routine one does.
        let manual = lock(&self.scheduler, HostLocation::FenceInstanceResources)?
            .active_lease(instance_id)
            .is_some_and(|lease| manual_claim(lease.kind()));
        let manual = manual
            && self.ladder_config()?.rules.direct_runs
                == actingcommand_runtime_config::DirectRuns::Exempt;
        let newly = match lock(&self.resource_fences, HostLocation::FenceInstanceResources)?
            .entry(instance_id)
        {
            Entry::Occupied(_) => false,
            Entry::Vacant(vacant) => {
                vacant.insert(ResourceFence {
                    cause: error.outcome().code().clone(),
                    since_unix_ms,
                    stage: if manual {
                        FenceStage::Person
                    } else {
                        FenceStage::Pending
                    },
                    person_wait: manual.then(|| PersonWait::new(now)),
                });
                true
            }
        };
        self.execution()?
            .forget_unconfirmed_instance_close(instance_id)
            .map_err(|kernel_error| {
                RuntimeHostError::execution(HostLocation::FenceInstanceResources, &kernel_error)
            })?;
        let fence = resource_fence_error(instance_id, fenced_projection_code(error))
            .with_caused_by(error.outcome());
        if newly {
            // Workflow #387 D1c-H: the fence record enters from the failure's own record, so
            // direction ① counts the two as one failure even where the fence's links name the
            // instance alone (a failed lease end).
            self.append_lifecycle_failure(
                RuntimeLifecycleFailureStage::SessionClose,
                RuntimeLifecycleFailure::Host(&fence),
                links.with_instance_id(self.events.issuer().issue_registered_instance(instance_id)),
                error.diagnostics().recorded_event().get().copied(),
            )?;
            self.withhold_policy_instance_availability(instance_id)?;
            // #411 A38: a fence that waits for a person at once says so after its own record.
            if manual {
                self.record_person_wait_entered(instance_id)?;
            }
        }
        Ok(fence)
    }

    /// Workflow #387 D1c-G: settles `error`, the failure of an ADB check of `instance_id`'s own
    /// (the startup package's probe, the foreground gate's observation, the emulator control's
    /// ADB wait, a ladder's ADB checks). An adb child or pipe it left unconfirmed belongs to that
    /// instance: the instance is fenced and the check's work fails with the fence's non-fatal
    /// failure (`Ok`). Where no fence may take it (no registered owner, a ledger failure, a
    /// shutdown) the owner is retained and the Runtime marked fatal, as before (`Err`, the fatal
    /// failure). A failure whose cleanup was confirmed is returned as it is (`Ok`).
    pub(super) fn settle_unconfirmed_adb(
        &self,
        error: RuntimeHostError,
        instance_id: InstanceId,
        links: EventLinksDraft,
    ) -> RuntimeHostResult<RuntimeHostError> {
        match self.settle_unconfirmed_resources(&error, Some(instance_id), links)? {
            UnconfirmedResources::Confirmed => Ok(error),
            UnconfirmedResources::Fenced(fence) => Ok(fence),
            UnconfirmedResources::Retained => Err(error.into_fatal()),
        }
    }

    /// The whole-Runtime retention of unconfirmed resources that no single instance owns: the
    /// failure recorded, the owner retained and the Runtime marked fatal.
    fn retain_unconfirmed_owner(
        &self,
        error: &RuntimeHostError,
        links: EventLinksDraft,
    ) -> RuntimeHostResult<UnconfirmedResources> {
        let error = error.clone().into_fatal();
        let lifecycle_result = self.append_lifecycle_failure(
            RuntimeLifecycleFailureStage::SessionClose,
            RuntimeLifecycleFailure::Host(&error),
            links,
            None,
        );
        let retained = self.retain_owner_unconfirmed(&error);
        lifecycle_result?;
        retained?;
        Ok(UnconfirmedResources::Retained)
    }

    /// The owner retained and the Runtime marked fatal by `error`, which the caller records.
    pub(super) fn retain_owner_unconfirmed(
        &self,
        error: &RuntimeHostError,
    ) -> RuntimeHostResult<()> {
        let retain_result = lock(&self.owner, HostLocation::RetainUnconfirmedOwner)
            .and_then(|mut owner| owner.retain_unconfirmed());
        let fatal_result = self.fatal.mark(error.clone().into_fatal());
        retain_result?;
        fatal_result
    }

    /// Whether `instance_id` is fenced.
    pub(super) fn resource_fenced(&self, instance_id: InstanceId) -> RuntimeHostResult<bool> {
        Ok(lock(
            &self.resource_fences,
            HostLocation::ReadInstanceResourceFence,
        )?
        .contains_key(&instance_id))
    }

    /// Whether any instance is fenced: the owner's resources then stay `in_use`.
    pub(super) fn any_resource_fence(&self) -> RuntimeHostResult<bool> {
        Ok(!lock(
            &self.resource_fences,
            HostLocation::ReadInstanceResourceFence,
        )?
        .is_empty())
    }

    /// Whether the fence of `instance_id` holds its routine claims: before the sweep hands it
    /// to a ladder, and while it waits for a person. A fence a ladder holds lets that ladder's
    /// claim through.
    pub(super) fn resource_fence_holds_routine(
        &self,
        instance_id: InstanceId,
    ) -> RuntimeHostResult<bool> {
        Ok(lock(
            &self.resource_fences,
            HostLocation::ReadInstanceResourceFence,
        )?
        .get(&instance_id)
        .is_some_and(|fence| fence.stage != FenceStage::Ladder))
    }

    /// Every fence as `Status` reports it; #411 A38: one that waits for a person with since
    /// when, its rechecks so far and allowed (the configuration in effect) and the next one's
    /// time.
    pub(super) fn resource_fence_statuses(
        &self,
    ) -> RuntimeHostResult<BTreeMap<InstanceId, InstanceResourceFence>> {
        let recheck = self.fence_recheck_config()?;
        Ok(lock(
            &self.resource_fences,
            HostLocation::ReadInstanceResourceFence,
        )?
        .iter()
        .map(|(instance_id, fence)| {
            (
                *instance_id,
                InstanceResourceFence {
                    cause: fence.cause.clone(),
                    since_unix_ms: fence.since_unix_ms,
                    awaiting_person: fence.stage == FenceStage::Person,
                    person_wait: fence.person_wait.as_ref().map(|wait| wait.status(recheck)),
                },
            )
        })
        .collect())
    }

    /// The fenced instance `alias` names, if it is fenced.
    pub(super) fn resource_fenced_alias(
        &self,
        alias: &str,
    ) -> RuntimeHostResult<Option<InstanceId>> {
        let instance_id = lock(
            &self.registered_instances,
            HostLocation::ReadInstanceResourceFence,
        )?
        .values()
        .find(|instance| instance.instance_alias == alias)
        .map(RegisteredInstance::instance_id);
        match instance_id {
            Some(instance_id) if self.resource_fenced(instance_id)? => Ok(Some(instance_id)),
            _ => Ok(None),
        }
    }

    /// A preparation of `instance_id` passed its self-check (`preparation` is its
    /// `instance_preparation_finished`). When the instance is fenced, that proves the close:
    /// the fence is lifted with one `resource_fence_lifted` record naming the preparation, and
    /// the instance's policy availability follows its self-check again. #411 A38: the record of
    /// a lift that the fence's own recheck made carries `instance_resources_fence_recheck_passed`
    /// with the recheck's number; it ends the wait for a person.
    pub(super) fn lift_resource_fence(
        &self,
        instance_id: InstanceId,
        preparation: TerminalEvent,
        links: EventLinksDraft,
    ) -> RuntimeHostResult<()> {
        let limit = self.fence_recheck_config()?.max_attempts;
        let lifted = lock(
            &self.resource_fences,
            HostLocation::LiftInstanceResourceFence,
        )?
        .remove(&instance_id);
        let Some(lifted) = lifted else {
            return Ok(());
        };
        let outcome = match lifted
            .person_wait
            .filter(|wait| wait.recheck == Recheck::Preparing)
        {
            Some(wait) => Outcome::new(HostCode::InstanceResourcesFenceRecheckPassed)
                .instance_id(identifier_text(&instance_id))
                .attempts(wait.attempts)
                .limit_count(limit),
            None => Outcome::new(HostCode::InstanceResourcesFenceLifted)
                .instance_id(identifier_text(&instance_id)),
        };
        self.append_lifecycle_observed(
            RuntimeLifecyclePhase::ResourceFenceLifted {
                instance_id,
                preparation,
                outcome,
            },
            links,
        )?;
        self.regate_policy_instance_availability(instance_id)
    }

    /// The lease sweep's step: hands each fence not handed on yet to a recovery ladder, or to a
    /// person when none may start. A ladder that already holds or waits for the instance decides
    /// at its end. Otherwise a `resource_fence` ladder is admitted as every ladder is, and its
    /// claim waits for the instance's key.
    pub(super) fn hand_resource_fences_to_ladders(&self) -> RuntimeHostResult<()> {
        let pending = lock(
            &self.resource_fences,
            HostLocation::HandResourceFencesToLadders,
        )?
        .iter()
        .filter(|(_, fence)| fence.stage == FenceStage::Pending)
        .map(|(instance_id, fence)| (*instance_id, fence.cause.clone()))
        .collect::<Vec<_>>();
        for (instance_id, cause) in pending {
            if self.fatal.is_shutdown_requested() {
                return Ok(());
            }
            let ladder_active = lock(&self.scheduler, HostLocation::HandResourceFencesToLadders)?
                .ladder_active(instance_id);
            if ladder_active {
                self.set_resource_fence_stage(instance_id, FenceStage::Ladder)?;
                continue;
            }
            match self.resource_fence_ladder(instance_id, cause)? {
                Some(ladder) => {
                    self.set_resource_fence_stage(instance_id, FenceStage::Ladder)?;
                    self.enqueue_ladder_claim(ladder)?;
                    // A claim the scheduler refused to queue (recorded by the enqueue) starts
                    // no ladder: the instance waits for a person.
                    let queued = lock(&self.scheduler, HostLocation::HandResourceFencesToLadders)?
                        .ladder_active(instance_id);
                    if !queued {
                        self.resource_fence_awaits_person(instance_id)?;
                    }
                }
                None => self.resource_fence_awaits_person(instance_id)?,
            }
        }
        Ok(())
    }

    /// A ladder of `instance_id` ended: a fence it held without lifting now waits for a person.
    /// Workflow #387 D1c-G: so does a fence made while the ladder held the instance (one of its
    /// own ADB checks, say) that the sweep has not handed on yet: the ladder holding the
    /// instance decides it at its end, whether or not a sweep tick fell inside the climb. Left
    /// `Pending`, a later tick would either start a second ladder for it, or, while the ladder
    /// still holds the key before its release, hand it to this ladder after its decision, so
    /// that it never waits for a person. The sweep hands a fence no ladder may take here too.
    /// #411 A38: a fence that begins to wait now records it, and its wait's first recheck
    /// counts its interval from now; one that already waits keeps its wait.
    pub(super) fn resource_fence_awaits_person(
        &self,
        instance_id: InstanceId,
    ) -> RuntimeHostResult<()> {
        let now = self.runtime_clock_sample()?;
        let entered = {
            let mut fences = lock(
                &self.resource_fences,
                HostLocation::HandResourceFencesToLadders,
            )?;
            match fences.get_mut(&instance_id) {
                Some(fence) if fence.stage != FenceStage::Person => {
                    fence.stage = FenceStage::Person;
                    fence.person_wait = Some(PersonWait::new(now));
                    true
                }
                _ => false,
            }
        };
        if entered {
            self.record_person_wait_entered(instance_id)?;
        }
        Ok(())
    }

    fn set_resource_fence_stage(
        &self,
        instance_id: InstanceId,
        stage: FenceStage,
    ) -> RuntimeHostResult<()> {
        let mut fences = lock(
            &self.resource_fences,
            HostLocation::HandResourceFencesToLadders,
        )?;
        if let Some(fence) = fences.get_mut(&instance_id) {
            fence.stage = stage;
            // #411 A38: only a fence that waits for a person has a wait.
            if stage != FenceStage::Person {
                fence.person_wait = None;
            }
        }
        Ok(())
    }

    /// #411 A38: `recovery_ladder.fence_recheck` in effect, read at every use: a change through
    /// `ac_config_set` applies from the next sweep tick.
    fn fence_recheck_config(&self) -> RuntimeHostResult<FenceRecheckConfig> {
        Ok(self.ladder_config()?.fence_recheck)
    }

    /// #411 A38: `instance_resources_fence_awaiting_person` for the wait `instance_id` began:
    /// the fence's cause, the most rechecks, their interval and the first one's time.
    fn record_person_wait_entered(&self, instance_id: InstanceId) -> RuntimeHostResult<()> {
        let config = self.fence_recheck_config()?;
        let entered = lock(
            &self.resource_fences,
            HostLocation::AwaitResourceFencePerson,
        )?
        .get(&instance_id)
        .and_then(|fence| {
            let wait = fence.person_wait.as_ref()?;
            Some((fence.cause.registered(), wait.next_due(config)))
        });
        let Some((cause, first)) = entered else {
            return Ok(());
        };
        let outcome = Outcome::new(HostCode::InstanceResourcesFenceAwaitingPerson)
            .operation(HostLocation::AwaitResourceFencePerson)
            .instance_id(identifier_text(&instance_id))
            .limit_count(config.max_attempts)
            .retry_after_ms(config.interval_ms);
        let outcome = match cause {
            Some(cause) => outcome.failure_code(cause),
            None => outcome,
        };
        let outcome = match first {
            Some(first) => outcome.next_occurrence_unix_ms(first.unix_ms),
            None => outcome,
        };
        let links = self
            .events
            .system_links()?
            .with_instance_id(self.events.issuer().issue_registered_instance(instance_id));
        self.record_person_wait(links, outcome, HostLocation::AwaitResourceFencePerson)
    }

    /// #411 A38: one record of a fence's wait for a person (`runtime.failed`, its severity the
    /// outcome's category, warning).
    fn record_person_wait(
        &self,
        links: EventLinksDraft,
        outcome: Outcome,
        operation: HostLocation,
    ) -> RuntimeHostResult<()> {
        self.append_event_raw(
            EventSeverity::Warning,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links,
            RuntimePayloadDraft::failed(
                DiagnosticCode::RuntimeDiagnostic,
                EffectDisposition::NotPerformed,
                outcome,
                DiagnosticDetailDraft::new(
                    "resource_fence",
                    operation,
                    "runtime_host",
                    operation.as_str(),
                ),
                AuditInput::new(),
            ),
        )
        .map(|_| ())
    }

    /// #411 A38: the lease sweep's step after the hand-off: for each fence that waits for a
    /// person whose next recheck is due under `recovery_ladder.fence_recheck` in effect, one
    /// self-check claim on its instance's queue. Only a failure the Runtime does not survive is
    /// returned; a claim the queue refuses is recorded and ends that recheck as failed.
    pub(super) fn recheck_resource_fences(&self) -> RuntimeHostResult<()> {
        Ok(())
    }

    /// #411 A38: queues one recheck on a self-check claim of the instance's queue (as a
    /// person's `actingctl selfcheck` waits for the key; no new claim kind).
    fn queue_fence_recheck(&self, instance_id: InstanceId, attempt: u32) -> RuntimeHostResult<()> {
        let resolved = lock(
            &self.registered_instances,
            HostLocation::QueueResourceFenceRechecks,
        )?
        .get(&instance_id)
        .cloned()
        .ok_or_else(|| {
            RuntimeHostError::fatal(
                HostCode::RuntimeInstanceRegistryIncomplete,
                HostLocation::QueueResourceFenceRechecks,
                RuntimeErrorCode::RuntimeHalted,
            )
        })?;
        let issuer = self.events.issuer();
        let holder_id = *issuer
            .mint_holder_id()
            .map_err(|_| runtime_identifier_error())?
            .transport();
        let causation_id = issuer
            .mint_causation_id()
            .map_err(|_| runtime_identifier_error())?;
        let (actor, source) = scheduled_request_transport_origin(resolved.provenance());
        let request = RuntimeRequest::new(
            issuer
                .mint_request_id()
                .map_err(|_| runtime_identifier_error())?,
            issuer
                .mint_correlation_id()
                .map_err(|_| runtime_identifier_error())?,
            Some(causation_id),
            actor,
            source,
            unix_ms_now()?,
            RuntimeOperation::AcquireLease {
                instance_alias: resolved.instance_alias.clone(),
                holder_id,
            },
        )
        .map_err(|_| {
            RuntimeHostError::fatal(
                HostCode::HostKeyRequestInvalid,
                HostLocation::QueueResourceFenceRechecks,
                RuntimeErrorCode::RuntimeHalted,
            )
        })?;
        let connection_id =
            ConnectionId::new(STARTUP_PACKAGE_CONNECTION_VALUE).map_err(|error| {
                RuntimeHostError::scheduler(HostLocation::QueueResourceFenceRechecks, &error)
            })?;
        let lease_ttl_ms = lock(&self.scheduler, HostLocation::QueueResourceFenceRechecks)?
            .config()
            .lease_ttl_ms;
        let links = self
            .events
            .system_links()?
            .with_instance_id(issuer.issue_registered_instance(instance_id))
            .with_causation_id(causation_id);
        let queued = self.request_host_claim_work(
            HostClaim {
                request: &request,
                instance_alias: &resolved.instance_alias,
                holder_id,
                connection_id,
                kind: ClaimKind::SelfCheck,
                priority: LeasePriority::Normal,
                lease_ttl_ms,
            },
            HostClaimWork::FenceRecheck(PendingFenceRecheck {
                instance_id,
                attempt,
                links: links.clone(),
            }),
        );
        match queued {
            Ok(()) => Ok(()),
            Err(failure) if failure.poison_runtime || failure.error.is_fatal() => {
                Err(*failure.error)
            }
            Err(failure) => {
                self.append_lifecycle_failure(
                    RuntimeLifecycleFailureStage::OperationCleanup,
                    RuntimeLifecycleFailure::Host(&failure.error),
                    links.clone(),
                    None,
                )?;
                self.settle_fence_recheck(instance_id, attempt, Some(failure.error.code()), links)
            }
        }
    }

    /// #411 A38: a granted recheck on the instance's worker: the preparation and self-check a
    /// person's `actingctl selfcheck` makes, on the claim's key, as the Runtime's own recovery
    /// preparation (no ladder starts from it, and direction ① counts none of its records), then
    /// the key's release. A passing self-check lifted the fence on its way; a failing one is
    /// recorded and the next recheck counts its interval from now. A fence lifted, or a recheck
    /// superseded, while the claim waited runs nothing. Only a fatal failure is returned.
    pub(super) fn run_fence_recheck(
        &self,
        pending: &PendingFenceRecheck,
        key: HostKey,
    ) -> RuntimeHostResult<()> {
        let instance_id = pending.instance_id;
        let began = {
            let mut fences = lock(&self.resource_fences, HostLocation::RunResourceFenceRecheck)?;
            match fences
                .get_mut(&instance_id)
                .and_then(|fence| fence.person_wait.as_mut())
            {
                Some(wait)
                    if wait.recheck == Recheck::Queued && wait.attempts == pending.attempt =>
                {
                    wait.recheck = Recheck::Preparing;
                    true
                }
                _ => false,
            }
        };
        if !began {
            return self.release_host_key(&key);
        }
        let validated = key.request.validate().map_err(|_| {
            RuntimeHostError::fatal(
                HostCode::HostKeyRequestInvalid,
                HostLocation::RunResourceFenceRecheck,
                RuntimeErrorCode::RuntimeHalted,
            )
        })?;
        let links = self
            .events
            .request_links(&validated, Some(instance_id), None, None);
        let resolved = lock(
            &self.registered_instances,
            HostLocation::RunResourceFenceRecheck,
        )?
        .get(&instance_id)
        .cloned()
        .ok_or_else(|| {
            RuntimeHostError::fatal(
                HostCode::RuntimeInstanceRegistryIncomplete,
                HostLocation::RunResourceFenceRecheck,
                RuntimeErrorCode::RuntimeHalted,
            )
        })?;
        lock(
            &self.stability_counter,
            HostLocation::RunResourceFenceRecheck,
        )?
        .set_climbing(instance_id, true);
        let prepared = self
            .instance_guard(instance_id)
            .map_err(|failure| *failure.error)
            .and_then(|instance_guard| {
                let admission = lock(&instance_guard, HostLocation::RunResourceFenceRecheck)?;
                self.prepare_recovery_connection(&resolved, links.clone(), &admission, &key)
            });
        let unmarked = lock(
            &self.stability_counter,
            HostLocation::RunResourceFenceRecheck,
        )
        .map(|mut counter| counter.set_climbing(instance_id, false));
        let released = self.release_host_key(&key);
        let (selfcheck, _, _) = prepared?;
        unmarked?;
        released?;
        let passed = selfcheck.capture.ok && selfcheck.touch.ok && selfcheck.failure_code.is_none();
        if passed {
            return Ok(());
        }
        self.settle_fence_recheck(
            instance_id,
            pending.attempt,
            selfcheck.failure_code.as_deref(),
            links,
        )
    }

    /// #411 A38: the recheck `attempt` of `instance_id` failed (`cause_spelling` the code of its
    /// self-check's failure, or of the refusal of its claim): the fence stays and waits, the
    /// next recheck counts its interval from now, and `instance_resources_fence_recheck_failed`
    /// is recorded with that code as its `failure_code`. A fence lifted in the meantime records
    /// nothing.
    fn settle_fence_recheck(
        &self,
        instance_id: InstanceId,
        attempt: u32,
        cause_spelling: Option<&str>,
        links: EventLinksDraft,
    ) -> RuntimeHostResult<()> {
        let config = self.fence_recheck_config()?;
        let now = self.runtime_clock_sample()?;
        let next = {
            let mut fences = lock(&self.resource_fences, HostLocation::RunResourceFenceRecheck)?;
            let Some(wait) = fences
                .get_mut(&instance_id)
                .and_then(|fence| fence.person_wait.as_mut())
                .filter(|wait| wait.attempts == attempt && wait.recheck != Recheck::Idle)
            else {
                return Ok(());
            };
            wait.recheck = Recheck::Idle;
            wait.from = now;
            wait.next_due(config)
        };
        let outcome = Outcome::new(HostCode::InstanceResourcesFenceRecheckFailed)
            .operation(HostLocation::RunResourceFenceRecheck)
            .instance_id(identifier_text(&instance_id))
            .attempts(attempt)
            .limit_count(config.max_attempts);
        let outcome = with_failure_code(outcome, cause_spelling);
        let outcome = match next {
            Some(next) => outcome.next_occurrence_unix_ms(next.unix_ms),
            None => outcome,
        };
        self.record_person_wait(links, outcome, HostLocation::RunResourceFenceRecheck)
    }
}

impl HostShared {
    /// Workflow #403: the failure a caller reports for a retained-session close that failed:
    /// the fence's non-fatal failure, caused by the close's, when the close was unconfirmed and
    /// fenced the instance; otherwise the close's own failure, as before.
    pub(super) fn retained_close_failure(
        &self,
        operation: HostLocation,
        instance_id: InstanceId,
        close_error: &ExecutionKernelError,
    ) -> RuntimeHostResult<RuntimeHostError> {
        let error = RuntimeHostError::execution(operation, close_error);
        if close_error.resource_quiescence() == Some(ResourceQuiescence::Unconfirmed)
            && self.resource_fenced(instance_id)?
        {
            return Ok(
                resource_fence_error(instance_id, fenced_projection_code(&error))
                    .with_caused_by(error.outcome()),
            );
        }
        Ok(error)
    }
}
