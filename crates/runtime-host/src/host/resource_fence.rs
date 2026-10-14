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

use super::*;
use crate::codes::{HostCode, HostLocation};
use crate::failure_identity::identifier_text;
use actingcommand_contract::InstanceResourceFence;
use actingcommand_contract::outcome::{CodeStr, Outcome};
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
        RuntimeErrorCode::RuntimeFatal | RuntimeErrorCode::LedgerFailure => {
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
        let owner = error.lifecycle.instance_id.or(instance_id);
        let registered = match owner {
            Some(instance_id) => lock(
                &self.registered_instances,
                HostLocation::FenceInstanceResources,
            )?
            .contains_key(&instance_id),
            None => false,
        };
        let Some(instance_id) = owner
            .filter(|_| registered && error.projection().code != RuntimeErrorCode::LedgerFailure)
        else {
            return self.retain_unconfirmed_owner(error, links);
        };
        let since_unix_ms = self.clock.sample()?.unix_ms;
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
            // Workflow #387 D1c-H: the fence record enters from the unconfirmed failure's own
            // record, so direction ① counts the two as one failure even where the fence's
            // links name the instance alone (a failed lease end).
            self.append_lifecycle_failure(
                RuntimeLifecycleFailureStage::SessionClose,
                RuntimeLifecycleFailure::Host(&fence),
                links.with_instance_id(self.events.issuer().issue_registered_instance(instance_id)),
                error.diagnostics().recorded_event().get().copied(),
            )?;
            self.withhold_policy_instance_availability(instance_id)?;
        }
        Ok(UnconfirmedResources::Fenced(fence))
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

    /// Every fence as `Status` reports it.
    pub(super) fn resource_fence_statuses(
        &self,
    ) -> RuntimeHostResult<BTreeMap<InstanceId, InstanceResourceFence>> {
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
    /// the instance's policy availability follows its self-check again.
    pub(super) fn lift_resource_fence(
        &self,
        instance_id: InstanceId,
        preparation: TerminalEvent,
        links: EventLinksDraft,
    ) -> RuntimeHostResult<()> {
        let lifted = lock(
            &self.resource_fences,
            HostLocation::LiftInstanceResourceFence,
        )?
        .remove(&instance_id)
        .is_some();
        if !lifted {
            return Ok(());
        }
        self.append_lifecycle_observed(
            RuntimeLifecyclePhase::ResourceFenceLifted {
                instance_id,
                preparation,
                outcome: Outcome::new(HostCode::InstanceResourcesFenceLifted)
                    .instance_id(identifier_text(&instance_id)),
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
                None => self.set_resource_fence_stage(instance_id, FenceStage::Person)?,
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
    /// that it never waits for a person.
    pub(super) fn resource_fence_awaits_person(
        &self,
        instance_id: InstanceId,
    ) -> RuntimeHostResult<()> {
        let mut fences = lock(
            &self.resource_fences,
            HostLocation::HandResourceFencesToLadders,
        )?;
        if let Some(fence) = fences.get_mut(&instance_id) {
            fence.stage = FenceStage::Person;
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
        }
        Ok(())
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
