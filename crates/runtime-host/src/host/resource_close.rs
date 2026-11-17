// SPDX-License-Identifier: AGPL-3.0-only

use super::emulator_autostart::AutomaticStartHold;
use super::host_claims::HostClaimWork;
use super::lease::HostClaim;
use super::policy_dispatch::scheduling_resume_selfcheck;
use super::recovery_ladder::PendingRecoveryLadder;
use super::resource_fence::UnconfirmedResources;
use super::selfcheck_emulator::EmulatorStatusRead;
use super::*;
use crate::codes::{HostCode, HostLocation};
use actingcommand_contract::codes::{DeviceSelfCheckFailure, RuntimeErrorCode};
use actingcommand_contract::{CONNECTION_PREPARATION_CLAIM_WAIT_MS, SchedulingResumeSelfCheck};
use actingcommand_scheduler::codes::SchedulerCode;

/// How a lease end treats the instance's device session (Workflow #191 H).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum LeaseDeviceEnd {
    Keep,
    /// Workflow #461: a holder's own end of its lease through `end_lease_ordered` (a client's
    /// disconnect, a backend failure's end, as a failed input's), whose caller closes, once the
    /// lease has ended, a session whose close the lapsed lease refused
    /// (`close_expired_lease_session_guarded`). Otherwise as `Keep`. Workflow #475: so a
    /// pre-emption's end and its hand-over (`perform_transfer`), and a failed run's hand-over of
    /// its key to its ladder (`hand_off_to_ladder`), which then hand the device to no one.
    HolderEnd,
    Expiry,
}

enum ResourceCloseTarget {
    Session,
    ApplicationBackends,
}

/// #480 PG-1 fix round 2 (the review's sequence 2, to its end): how the preparation of the
/// watch's recognition on its autostart claim's key (`prepare_recognized_instance`) ended.
pub(super) enum RecognitionPreparation {
    /// It ran: the ladder a failed preparation admitted, which the caller's release hands the
    /// key to (H-1, P1).
    Ran(Option<Box<PendingRecoveryLadder>>),
    /// `automatic_start_hold`, asked once the claim had the instance's admission guard, held it:
    /// nothing was read, bound, prepared or recorded.
    Held(AutomaticStartHold),
}

impl HostShared {
    /// Workflow #191 H: the device session belongs to the instance, not to the lease. The caller
    /// holds the instance admission guard. A kept session forgets its input frames, so the next
    /// holder starts from its own capture, as after a close. A session that must be closed, or
    /// one outside the multi-Nemu gate (`keeps_device_session`), is closed as before.
    pub(super) fn end_lease_device_use(
        &self,
        token: &LeaseToken,
        connection_id: ConnectionId,
        end: LeaseDeviceEnd,
    ) -> Result<(), RequestFailure> {
        let instance_id = token.instance_id();
        // ISO-5a (#419, ISO-M row B02): an expiry that meets a write still in flight on the
        // lease (an input writes without the admission guard) closes nothing under that write.
        // The scheduler defers the expiry under the step (`cleanup_token_guarded`), and a later
        // sweep tick ends the lease once the write returned: no close is refused, nothing is
        // recorded per tick, and nothing stops. Workflow #475 (the review of #461's fix): a
        // holder's own end gives way too, defensively. `begin_resource_close` asks the lease's
        // time before the step in flight, so its close under a lapsed lease with a write still in
        // flight would be taken for a refused close and owed under that write; no path reaches it
        // today (a connection's requests are answered one at a time).
        if matches!(end, LeaseDeviceEnd::Expiry | LeaseDeviceEnd::HolderEnd)
            && self.lease_step_active(token)?
        {
            return Ok(());
        }
        let needs_close = self
            .execution()?
            .session_needs_close(instance_id)
            .map_err(|error| {
                RequestFailure::poison_without_terminal(RuntimeHostError::execution(
                    HostLocation::InspectLeaseEndSession,
                    &error,
                ))
            })?;
        if needs_close
            || !self
                .keeps_device_session(instance_id)
                .map_err(RequestFailure::poison_without_terminal)?
        {
            return self.close_instance_resources(
                token,
                connection_id,
                EventLinksDraft::default(),
                end,
            );
        }
        if self.lease_step_active(token)? {
            // The scheduler defers an expiry under an in-flight step (`cleanup_token_inner`).
            if end == LeaseDeviceEnd::Expiry {
                return Ok(());
            }
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::scheduler(
                    HostLocation::EndLeaseDeviceUse,
                    &SchedulerError::DestructiveStateMismatch,
                ),
            ));
        }
        if !self
            .execution()?
            .has_owned_resources(instance_id)
            .map_err(|error| {
                RequestFailure::poison_without_terminal(RuntimeHostError::execution(
                    HostLocation::InspectExecutionSession,
                    &error,
                ))
            })?
        {
            self.record_owner_resource_close()?;
            return Ok(());
        }
        self.execution()?
            .forget_input_frames(instance_id)
            .map_err(|error| {
                RequestFailure::poison_without_terminal(RuntimeHostError::execution(
                    HostLocation::ForgetLeaseInputFrames,
                    &error,
                ))
            })
    }

    /// Whether a destructive step (an input write, a device-state step) is still in flight on
    /// `token`'s lease.
    pub(super) fn lease_step_active(&self, token: &LeaseToken) -> RuntimeHostResult<bool> {
        Ok(lock(&self.scheduler, HostLocation::ReadLeaseEndStep)?
            .active_lease(token.instance_id())
            .is_some_and(|active| active.token() == token && active.destructive_step_active()))
    }

    /// Workflow #191 H (the multi-Nemu gate): whether `instance_id` keeps its device session
    /// across lease ends, preparation and pause. Two Nemu sessions cannot stay open together (the
    /// vendor stdio redirection and its lock are process-wide, the DLL is shared), so while two or
    /// more registered instances use Nemu (`nemu_ipc` or an automatic choice for capture or
    /// input) each of them closes its session at every lease end as before. Every other
    /// instance keeps it, as does a registration without a device configuration (a fixture).
    /// Workflow #437 MAPP-1b (`contracts/device-group.md`, "Device-level actions", row 7): a member
    /// of a device group of several keeps no session across lease ends, whatever its backends:
    /// each member opens its own on its turn, so two members' sessions are never open together.
    pub(super) fn keeps_device_session(&self, instance_id: InstanceId) -> RuntimeHostResult<bool> {
        if self.in_group_of_several(instance_id) {
            return Ok(false);
        }
        let registered = lock(
            &self.registered_instances,
            HostLocation::ReadDeviceSessionGate,
        )?
        .values()
        .map(|instance| (instance.instance_id, instance.instance_alias.clone()))
        .collect::<Vec<_>>();
        let mut nemu_instances = 0usize;
        let mut uses_nemu = false;
        for (registered_id, instance_alias) in registered {
            let resolved = self
                .execution()?
                .resolve(&instance_alias)
                .map_err(|error| {
                    RuntimeHostError::execution(HostLocation::ResolveDeviceSessionGate, &error)
                })?;
            let nemu = resolved.configuration().is_some_and(|configuration| {
                [&configuration.capture_backend, &configuration.input_backend]
                    .into_iter()
                    .any(|backend| matches!(backend.as_str(), "nemu_ipc" | "auto" | "auto-fastest"))
            });
            if nemu {
                nemu_instances = nemu_instances.saturating_add(1);
                uses_nemu |= registered_id == instance_id;
            }
        }
        Ok(!(uses_nemu && nemu_instances >= 2))
    }

    pub(super) fn cleanup_composite_failure(
        &self,
        token: LeaseToken,
        connection_id: ConnectionId,
        failure: RequestFailure,
    ) -> RequestFailure {
        let failure = match self.settle_failure_resources(&token, failure) {
            Ok(failure) => failure,
            Err(failure) => return failure,
        };
        match self.cleanup_token(&token, connection_id, LeaseReleaseReason::BackendFailure) {
            Ok(()) => failure,
            // ISO-5a (#419, ISO-M row B22): a cleanup refusal not fatal fails only the work.
            Err(error) => failure.with_cleanup_failure(error),
        }
    }

    pub(super) fn cleanup_composite_failure_with_run_links(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        token: LeaseToken,
        connection_id: ConnectionId,
        run_links: Option<RuntimeRunLinks>,
        failure: RequestFailure,
    ) -> RequestFailure {
        // Workflow #437 MAPP-2a: a run on a borrowed key ends without its key's end: an
        // unconfirmed close is settled as for any failed run, and the key stays, still valid,
        // with the run that holds it and gets it back. The read tells only that run apart: a
        // read that fails (a poisoned run table) counts as "not borrowed", so every other run's
        // failed end takes its path unchanged, its release included, and the poison is
        // reported where that path reads the run table (the ladder's trigger, the lapsed close).
        let borrowed = matches!(self.run_borrows_key(request.request_id()), Ok(true));
        let failure = match self.settle_failure_resources(&token, failure) {
            Ok(failure) => failure,
            Err(failure) => return failure,
        };
        if borrowed {
            return failure;
        }
        let cleanup = match run_links {
            // Workflow #369 H-1 (review M-1): only a policy run's control carries a ladder
            // trigger; every other run-linked end passes `None`.
            Some(run_links) => self
                .ladder_trigger_for(request.request_id())
                .and_then(|trigger| {
                    self.cleanup_scheduled_failure_with_run_links(
                        request,
                        &token,
                        connection_id,
                        run_links,
                        trigger,
                    )
                }),
            // Workflow #387 D1c (B1): a direct run's failed end queues the ladder its trigger
            // admits once its key is released, only under `rules.direct_runs = trigger`.
            None => self
                .cleanup_token(&token, connection_id, LeaseReleaseReason::BackendFailure)
                .and_then(|()| self.ladder_after_direct_run(request, &token)),
        };
        // #411 A64 (ruling C-43): a close in the run's end phase that met its lapsed lease left
        // the session open; with the lease ended, it is closed now.
        let cleanup = cleanup.and_then(|()| self.close_lapsed_run_session(&token));
        match cleanup {
            Ok(()) => failure,
            // ISO-5a (#419, ISO-M row B22): a cleanup refusal not fatal fails only the work.
            Err(error) => failure.with_cleanup_failure(error),
        }
    }

    pub(super) fn finish_destructive_input(
        &self,
        step: Option<Arc<FencedWrite>>,
        connection_id: ConnectionId,
    ) -> Result<(), RequestFailure> {
        let Some(witness) = step.and_then(|step| Arc::try_unwrap(step).ok()) else {
            let mut error = RuntimeHostError::fatal(
                HostCode::FencedStepNotReclaimed,
                HostLocation::FinishDestructiveInput,
                RuntimeErrorCode::RuntimeHalted,
            );
            error.lifecycle.resource_quiescence = Some(ResourceQuiescence::Unconfirmed);
            // A fenced write still in flight is not one instance's state the host can fence:
            // the whole Runtime retains its owner, as before (Workflow #403 keeps it).
            self.settle_unconfirmed_resources(&error, None, EventLinksDraft::default())
                .map_err(RequestFailure::poison_without_terminal)?;
            return Err(RequestFailure::poison_without_terminal(error));
        };
        // Workflow #378 A4 (part B L1): a refusal of the step's lease names it.
        let lease_id = witness.token().lease_id();
        lock(&self.scheduler, HostLocation::FinishDestructiveInput)?
            .finish_destructive_step(witness, connection_id)
            .map_err(|error| {
                RequestFailure::poison_without_terminal(
                    RuntimeHostError::scheduler(HostLocation::FinishDestructiveInput, &error)
                        .naming_lease(lease_id),
                )
            })
    }

    pub(super) fn mark_resources_in_use(&self) -> RuntimeHostResult<MutexGuard<'_, OwnerGuard>> {
        let mut owner = lock(&self.owner, HostLocation::MarkOwnerResourcesInUse)?;
        owner.set_resource_disposition(OwnerResourceDisposition::InUse)?;
        Ok(owner)
    }

    fn record_owner_resource_close(&self) -> RuntimeHostResult<OwnerResourceDisposition> {
        // The same owner lock spans InUse and session registration on every acquisition.
        let mut owner = lock(&self.owner, HostLocation::RecordOwnerResourceClose)?;
        if let Some(disposition) = owner.retained_resource_disposition()? {
            return Ok(disposition);
        }
        let has_sessions = self.execution()?.has_sessions().map_err(|error| {
            RuntimeHostError::execution(HostLocation::InspectRemainingExecutionSessions, &error)
        })?;
        // Workflow #403: a fenced instance's resources are not confirmed closed until a
        // preparation proves it; the owner's stay in use meanwhile.
        let disposition = if has_sessions || self.any_resource_fence()? {
            OwnerResourceDisposition::InUse
        } else {
            OwnerResourceDisposition::ConfirmedClosed
        };
        owner.set_resource_disposition(disposition)?;
        Ok(disposition)
    }

    /// Workflow #403: a failed lease end's failure whose resources may still be live. An
    /// unconfirmed close of the token's instance fences that instance and the lease ends with
    /// the fence's non-fatal failure (`Ok`); unconfirmed resources no single instance owns
    /// keep the lease, as before, with the fatal failure (`Err`).
    fn settle_failure_resources(
        &self,
        token: &LeaseToken,
        failure: RequestFailure,
    ) -> Result<RequestFailure, RequestFailure> {
        match self.settle_unconfirmed_resources(
            &failure.error,
            Some(token.instance_id()),
            EventLinksDraft::default(),
        ) {
            Ok(UnconfirmedResources::Confirmed) => Ok(failure),
            Ok(UnconfirmedResources::Fenced(fence)) => Ok(failure.fenced(fence)),
            Ok(UnconfirmedResources::Retained) => Err(failure),
            Err(retain_failure) => Err(failure.replace_with_poison(retain_failure)),
        }
    }

    /// The close of the instance's resources at a lease end of `token` (`end_lease_device_use`).
    pub(super) fn close_instance_resources(
        &self,
        token: &LeaseToken,
        connection_id: ConnectionId,
        links: EventLinksDraft,
        end: LeaseDeviceEnd,
    ) -> Result<(), RequestFailure> {
        let closed = match self.close_instance_resources_result(token, connection_id, links.clone())
        {
            Ok(closed) => closed,
            Err(failure) => {
                // #411 A64: the lease lapsed before its close could begin.
                if self
                    .close_lease_lapsed(token, &failure.error)
                    .map_err(RequestFailure::poison_without_terminal)?
                {
                    return self.fence_lapsed_lease_close(token, failure, links);
                }
                // Workflow #458: any other lease's expiry whose close its lapsed lease refused;
                // Workflow #461: so a holder's own end of its lapsed lease (a client's disconnect,
                // a backend failure's end), before the sweep met it.
                if end != LeaseDeviceEnd::Keep && self.expiry_close_refused(&failure.error) {
                    return self
                        .fence_expired_lease_close(token, *failure.error, links)
                        .map_err(RequestFailure::poison_without_terminal);
                }
                // ISO-5a (#419, ISO-M row B18): a holder's own end that finds its lease already
                // ended has nothing of it to close.
                if self
                    .close_lease_ended(token, &failure.error)
                    .map_err(RequestFailure::poison_without_terminal)?
                {
                    return Ok(());
                }
                return Err(failure);
            }
        };
        match closed {
            Ok(()) => Ok(()),
            // Workflow #403: the unconfirmed close fenced the instance; the lease ends and the
            // fence keeps the instance out of use.
            Err(error)
                if error.resource_quiescence() == Some(ResourceQuiescence::Unconfirmed)
                    && self.resource_fenced(token.instance_id())? =>
            {
                Ok(())
            }
            // ISO-2 (#419, ISO-M row B35): a close at a lease end that failed with its
            // resources confirmed (the emulator is gone) left nothing held: the kernel took the
            // session out of its registry, and the close recorded its failure and ended its
            // step (`retire_instance_resources`). The lease ends. A shutdown keeps its close as
            // before.
            Err(error)
                if error.resource_quiescence() != Some(ResourceQuiescence::Unconfirmed)
                    && !self.fatal.is_shutdown_requested() =>
            {
                Ok(())
            }
            Err(error) => Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::execution(HostLocation::CloseExecutionSession, &error),
            )),
        }
    }

    /// #411 A64 (ruling C-43): whether `error` is the refusal of a lease end's resource close
    /// because the lease of a run in its end phase has lapsed (`retire_instance_resources`:
    /// `begin_resource_close` answered `lease_expired`) or is already gone (`lease_missing`,
    /// `lease_mismatch`: the sweep expired it after the run's end grace), met before any close was
    /// attempted. Any other lease's refusal, an ordinary client lease's included, keeps its
    /// fatality as before C-43.
    fn close_lease_lapsed(
        &self,
        token: &LeaseToken,
        error: &RuntimeHostError,
    ) -> RuntimeHostResult<bool> {
        if error.operation() != HostLocation::BeginDestructiveResourceClose.as_str()
            || (error.code() != RuntimeErrorCode::LeaseExpired.as_str()
                && error.code() != RuntimeErrorCode::LeaseMissing.as_str()
                && error.code() != RuntimeErrorCode::LeaseMismatch.as_str())
        {
            return Ok(false);
        }
        Ok(self.run_end_phase_of_lease(token.lease_id())?.is_some())
    }

    /// ISO-5a (#419, ISO-M row B18): whether `error` is the refusal of a lease end's resource
    /// close because the lease had already ended before the close began (`lease_missing`,
    /// `lease_mismatch` from `begin_resource_close`), and the scheduler no longer holds `token`:
    /// the lease sweep expired it, and its expiry end closed or fenced the instance's resources
    /// (the fence then owns the session), or the key went on to another holder, whose session it
    /// is. The holder's own end (an emulator, application or input failure's cleanup, a policy
    /// run's settlement release, any other holder's end) then closes nothing and goes on; it is
    /// never a poison. A lease still held keeps its refusal as before.
    fn close_lease_ended(
        &self,
        token: &LeaseToken,
        error: &RuntimeHostError,
    ) -> RuntimeHostResult<bool> {
        if error.operation() != HostLocation::BeginDestructiveResourceClose.as_str()
            || (error.code() != RuntimeErrorCode::LeaseMissing.as_str()
                && error.code() != RuntimeErrorCode::LeaseMismatch.as_str())
        {
            return Ok(false);
        }
        Ok(
            !lock(&self.scheduler, HostLocation::ReadResourceCloseLease)?
                .active_tokens()
                .into_iter()
                .any(|active| active == *token),
        )
    }

    /// #411 A64 (the 10-24 incident; ruling C-43): the close at a lease end in a run's end phase
    /// met the run's lease lapsed (`lease_expired`) or already expired by the sweep after the end
    /// grace (`lease_missing`, `lease_mismatch`), so `begin_resource_close` refused its write
    /// authority: nothing was closed and the instance's resources stay as they are. Outside
    /// shutdown they count as an unconfirmed close of the lease's instance only (Workflow #403):
    /// the refusal is recorded once for the run (it names the lease; its stage is the session
    /// close; a later close of the same end that meets the lapsed lease again, as the transfer of
    /// a sweep expiry that hands the key on does, records nothing more), the instance is fenced
    /// by it, and the lease end goes on. The run's control keeps the session as still to close:
    /// once the lapsed lease has ended, `close_lapsed_run_session_guarded` closes it under a
    /// dedicated close lease. During shutdown, or where no fence may take them, the lease end
    /// fails as before.
    fn fence_lapsed_lease_close(
        &self,
        token: &LeaseToken,
        failure: RequestFailure,
        links: EventLinksDraft,
    ) -> Result<(), RequestFailure> {
        if self.fatal.is_shutdown_requested() {
            return Err(failure);
        }
        let instance_id = token.instance_id();
        let mut error = *failure.error;
        error.lifecycle.instance_id = Some(instance_id);
        error.lifecycle.resource_quiescence = Some(ResourceQuiescence::Unconfirmed);
        let links =
            links.with_instance_id(self.events.issuer().issue_registered_instance(instance_id));
        let first = match self
            .run_end_control_of_lease(token.lease_id())
            .map_err(RequestFailure::poison_without_terminal)?
        {
            Some(control) => control.note_lapsed_close(),
            None => true,
        };
        if first {
            self.append_lifecycle_failure(
                RuntimeLifecycleFailureStage::SessionClose,
                RuntimeLifecycleFailure::Host(&error),
                links.clone(),
                None,
            )
            .map_err(RequestFailure::poison_without_terminal)?;
        }
        // Workflow #437 G1: a refused close fences as an unconfirmed one, but leaves the session to
        // the close that follows.
        match self
            .settle_refused_close(&error, Some(instance_id), links)
            .map_err(RequestFailure::poison_without_terminal)?
        {
            UnconfirmedResources::Fenced(_) => Ok(()),
            UnconfirmedResources::Confirmed | UnconfirmedResources::Retained => {
                Err(RequestFailure::poison_without_terminal(error))
            }
        }
    }

    /// Workflow #458 (`contracts/read-session-resource-close.md`): whether `error` is the refusal
    /// of a close at a lease's expiry by the lapsed lease itself (`begin_resource_close` answered
    /// `lease_expired`). The lease sweep and an acquire that meets a due lease end only a lease
    /// whose time is over, so a close they must make (a session that needs it, an instance outside
    /// the multi-Nemu gate, a member of a device group of several) is refused so every time. Not
    /// during shutdown, as A64's fence is not. Workflow #461: the same refusal met before the sweep
    /// ended the lapsed lease, by a holder's own end of it (`LeaseDeviceEnd::HolderEnd`) and by the
    /// close a failed input makes under its own lease (`finish_input_failure`). Workflow #475: and
    /// by the closes made under a held key between its validation and its end: an application
    /// command's preparation (`control_application_of`), a backend switch's
    /// (`close_switch_session`) and a member switch's (`stop_member_application`).
    pub(super) fn expiry_close_refused(&self, error: &RuntimeHostError) -> bool {
        self.refused_close_isolates()
            && !self.fatal.is_shutdown_requested()
            && error.operation() == HostLocation::BeginDestructiveResourceClose.as_str()
            && error.code() == RuntimeErrorCode::LeaseExpired.as_str()
    }

    /// Workflow #458: the close at the expiry of `token`, a lease no run's end phase holds (#411
    /// A64 takes those first), met the lapsed lease (`expiry_close_refused`): nothing was closed
    /// and the instance's session stays open. As A64's refusal does, it counts as an unconfirmed
    /// close of the lease's instance only (Workflow #403): the refusal is recorded once (it names
    /// the lease; its stage is the session close), the instance is fenced by it, the close is owed
    /// (`expired_lease_closes`) and the lease end goes on. Once the lease has ended, still under
    /// the instance's admission guard, `close_expired_lease_session_guarded` closes the session.
    /// Where no fence may take the instance the lease end fails as before. Workflow #461: so for
    /// the refusal `error` met by a holder's own end of the lapsed lease, and by a failed input's
    /// close under it (`finish_input_failure`), whose own lease end, met again by the refusal, then
    /// records nothing more.
    fn fence_expired_lease_close(
        &self,
        token: &LeaseToken,
        error: RuntimeHostError,
        links: EventLinksDraft,
    ) -> RuntimeHostResult<()> {
        self.fence_expired_close_of(token, token.instance_id(), error, links)
    }

    /// Workflow #475: [`Self::fence_expired_lease_close`] for the session of `instance_id`, whose
    /// close under `token`'s lapsed lease was refused (`error`): the token's own instance, or
    /// (Workflow #437 MAPP-2a) another member of its device group whose session a switch closes
    /// under the key. That instance is fenced, the refusal recorded once per lease and instance,
    /// and its close owed until `token`'s lease has ended (`close_expired_lease_session_guarded`).
    pub(super) fn fence_expired_close_of(
        &self,
        token: &LeaseToken,
        instance_id: InstanceId,
        error: RuntimeHostError,
        links: EventLinksDraft,
    ) -> RuntimeHostResult<()> {
        let newly = lock(
            &self.expired_lease_closes,
            HostLocation::FenceInstanceResources,
        )?
        .entry(token.lease_id())
        .or_default()
        .insert(instance_id);
        self.fence_refused_close(instance_id, error, links, newly)
    }

    /// Workflows #456 and #458: a close of `instance_id`'s session that was refused before it
    /// began (`error`) fences that instance, as an unconfirmed close does (Workflow #403; #411
    /// A64's `fence_lapsed_lease_close`): the refusal is recorded (when `record`, with the
    /// instance, under `links`) and settled as an unconfirmed close of that instance only. `Err`:
    /// the record failed (the Runtime's own failure), or no fence could take the instance and the
    /// owner was retained (the refusal, fatal, as before).
    fn fence_refused_close(
        &self,
        instance_id: InstanceId,
        mut error: RuntimeHostError,
        links: EventLinksDraft,
        record: bool,
    ) -> RuntimeHostResult<()> {
        #[cfg(test)]
        if self
            .refused_close_record_failure_for_test
            .load(Ordering::Acquire)
        {
            return Err(ledger_error(HostLocation::FenceInstanceResources));
        }
        error.lifecycle.instance_id = Some(instance_id);
        error.lifecycle.resource_quiescence = Some(ResourceQuiescence::Unconfirmed);
        let links =
            links.with_instance_id(self.events.issuer().issue_registered_instance(instance_id));
        if record {
            self.append_lifecycle_failure(
                RuntimeLifecycleFailureStage::SessionClose,
                RuntimeLifecycleFailure::Host(&error),
                links.clone(),
                None,
            )?;
        }
        // Workflow #437 G1: as at `fence_lapsed_lease_close`.
        match self.settle_refused_close(&error, Some(instance_id), links)? {
            UnconfirmedResources::Fenced(_) => Ok(()),
            UnconfirmedResources::Confirmed | UnconfirmedResources::Retained => {
                Err(error.into_fatal())
            }
        }
    }

    /// Workflow #458: the session the close at `token`'s expiry left open because it met the
    /// lapsed lease (`fence_expired_lease_close`; Workflow #461: or the close at a holder's own end
    /// of the lapsed lease, or a failed input's close under it), closed once the lease has ended,
    /// whichever end ended it (the expiry, a disconnect, a backend failure's end). The caller
    /// holds the instance's admission guard. Nothing is closed when no close is owed, during
    /// shutdown (the Host close's second pass closes it), or when a holder took the instance's key
    /// on (the session is its, and its lease end or its preparation closes it). The expiry handed
    /// the device to no one (`end_lease_ordered`, `expired_close_owed`), so the session is closed
    /// under the session close ahead on the free device, which no claim waiting in the device's
    /// queue refuses, before any waiting claim is granted: the claim that waited first, of the
    /// same instance or (on a device of several) of another member, is granted afterwards by the
    /// queue's own grant when the guard is let go, as MAPP-1d closes a member's lapsed run
    /// session. A refusal that is not fatal is recorded once and leaves the session to the fence's
    /// own paths: a preparation (the fence's recovery ladder, its recheck, a person's self-check or
    /// resume), which closes a kept session before it opens. A failed close is recorded, and an
    /// unconfirmed one settled, by the close path; a fatal failure (the Runtime's own) is returned.
    /// Workflow #475: every session the lease owes is closed so, the lease's own instance's or
    /// another member's of its device group (`fence_expired_close_of`); the entry is taken once,
    /// so no owed close is made twice and none is left behind by the lease's end. Fix round 1:
    /// only a holder of the owed instance's own key takes its session over (the session is the
    /// instance's, Workflow #191 H, and that holder's lease end or preparation closes it). A
    /// device that a lease of another member holds (no path reaches it today: the owing lease's
    /// end hands the device to no one, under the guard) passes nothing over silently: the close
    /// is tried, the session close ahead refuses the held device (`lease_busy`, not fatal), and
    /// that refusal is recorded once below and leaves the session to the fenced member's own
    /// paths, as every refusal of that close does.
    pub(super) fn close_expired_lease_session_guarded(
        &self,
        token: &LeaseToken,
        admission: &MutexGuard<'_, ()>,
    ) -> RuntimeHostResult<()> {
        let Some(owed) = lock(
            &self.expired_lease_closes,
            HostLocation::FenceInstanceResources,
        )?
        .remove(&token.lease_id()) else {
            return Ok(());
        };
        for instance_id in owed {
            if self.fatal.is_shutdown_requested()
                || lock(&self.scheduler, HostLocation::ReadResourceCloseLease)?
                    .active_lease(instance_id)
                    .is_some_and(|active| active.token().instance_id() == instance_id)
            {
                continue;
            }
            let links = EventLinksDraft::default()
                .with_instance_id(self.events.issuer().issue_registered_instance(instance_id));
            match self.close_session_ahead_while_guarded(instance_id, links.clone(), admission) {
                Ok(_) => {}
                Err(mut refused) if !refused.is_fatal() => {
                    refused.lifecycle.instance_id = Some(instance_id);
                    self.append_lifecycle_failure(
                        RuntimeLifecycleFailureStage::SessionClose,
                        RuntimeLifecycleFailure::Host(&refused),
                        links,
                        None,
                    )?;
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    /// Workflow #458: whether `token` ended at its expiry with its session's close owed
    /// (`fence_expired_lease_close`; Workflow #461: or at a holder's own end, a disconnect or a
    /// backend failure's end, or after a failed input). Such a lease end hands the device to no
    /// one, an instance alone on its device as a member of several (fix round 1: handed on, the
    /// key went to a claim of the same instance with the session open, and that transfer's own
    /// close met the lapsed lease again), and the session is closed under a session close ahead
    /// before any waiting claim is granted (`close_expired_lease_session_guarded`).
    pub(super) fn expired_close_owed(&self, token: &LeaseToken) -> RuntimeHostResult<bool> {
        Ok(lock(
            &self.expired_lease_closes,
            HostLocation::FenceInstanceResources,
        )?
        .contains_key(&token.lease_id()))
    }

    /// Workflows #456, #458, #461 and #475, test only: whether a refused close isolates its
    /// instance. A test turns it off (`RuntimeHost::close_isolates_off_for_test`), and then
    /// nothing of the slices applies: the refusals end the Runtime as before.
    #[cfg(test)]
    fn refused_close_isolates(&self) -> bool {
        !self.close_isolates_off_for_test.load(Ordering::Acquire)
    }

    /// Workflows #456, #458, #461 and #475: a refused close of one instance's session isolates
    /// that instance.
    #[cfg(not(test))]
    fn refused_close_isolates(&self) -> bool {
        true
    }

    /// #411 A64 (ruling C-43): `close_lapsed_run_session_guarded` under the instance's admission
    /// guard, which it takes, at the end of a run's lease end. Nothing is taken when the run owes
    /// no close.
    pub(super) fn close_lapsed_run_session(&self, token: &LeaseToken) -> RuntimeHostResult<()> {
        if !self
            .run_end_control_of_lease(token.lease_id())?
            .is_some_and(|control| control.lapsed_close_owed())
        {
            return Ok(());
        }
        let instance_guard = self
            .instance_guard(token.instance_id())
            .map_err(|failure| *failure.error)?;
        let admission = lock(&instance_guard, HostLocation::LockInstanceAdmission)?;
        self.close_lapsed_run_session_guarded(token, &admission)
    }

    /// #411 A64 (ruling C-43; Workflow #191 H, #403): the session a close in a run's end phase
    /// left open because it met the run's lapsed lease (`fence_lapsed_lease_close`). The caller
    /// holds the instance's admission guard, and the lapsed lease has ended. An instance outside
    /// the multi-Nemu gate (`keeps_device_session`) must not keep its session past a lease end,
    /// so the fenced instance's session is closed now through the fenced close path, as a
    /// lease-free path's retained session is (`close_retained_instance_while_guarded`: a
    /// dedicated resource-close lease, released once the close is confirmed). The fence stays: only
    /// a preparation whose self-check passes lifts it. An unconfirmed close is settled by the close
    /// path itself (the instance stays fenced). Nothing is closed, once taken: when no close is
    /// owed, during shutdown (the Host close's second pass closes it), for an instance that
    /// keeps its session, or when a holder took the instance's key on (the session is its, and its
    /// lease end closes it). A dedicated lease refused because an entry waits in the instance's
    /// queue is recorded once and leaves the session to that entry, which closes it at its lease
    /// end or in its preparation (#411 A52); so, since Workflow #458's fix round 1, is every other
    /// refusal of it that is not fatal (a queue ahead held back by the fenced instance's gate),
    /// whose session a preparation of the fenced instance closes.
    pub(super) fn close_lapsed_run_session_guarded(
        &self,
        token: &LeaseToken,
        admission: &MutexGuard<'_, ()>,
    ) -> RuntimeHostResult<()> {
        let Some(control) = self.run_end_control_of_lease(token.lease_id())? else {
            return Ok(());
        };
        if !control.take_lapsed_close() {
            return Ok(());
        }
        let instance_id = token.instance_id();
        if self.fatal.is_shutdown_requested() || self.keeps_device_session(instance_id)? {
            return Ok(());
        }
        if lock(&self.scheduler, HostLocation::ReadResourceCloseLease)?
            .active_lease(instance_id)
            .is_some()
        {
            return Ok(());
        }
        let links = EventLinksDraft::default()
            .with_instance_id(self.events.issuer().issue_registered_instance(instance_id));
        // Workflow #437 MAPP-1d (`contracts/device-group.md`, "Device-level actions", row 7): a
        // member of a device group of several closes under a session close ahead, which no claim
        // waiting in the device's queue refuses, so the session is closed before any other
        // member's lease begins (the lapsed lease's end handed the device to no one,
        // `cleanup_token_guarded`). A failed close is recorded, and an unconfirmed one settled
        // (the member stays fenced, the close lease kept), by the close path.
        let ahead = self.in_group_of_several(instance_id);
        #[cfg(test)]
        let ahead = ahead && !self.mapp1d_off_for_test.load(Ordering::Acquire);
        if ahead {
            return self
                .close_session_ahead_while_guarded(instance_id, links, admission)
                .map(|_| ());
        }
        match self.close_retained_instance_while_guarded(
            instance_id,
            links.clone(),
            false,
            LeaseReleaseReason::BackendFailure,
            admission,
        ) {
            // A failed close is recorded, and an unconfirmed one settled, by the close path.
            Ok(_) => Ok(()),
            // Workflow #458 fix round 1: every refusal of the dedicated close lease that is not
            // fatal is one instance's, as the queue's is: also the takeover cooldown's and a queue
            // ahead whose claim the fenced instance's routine gate holds back (`lease_busy`).
            // Recorded once, the session is left to the waiting entry or to a preparation of the
            // fenced instance; before, such a refusal reached the lease sweep, whose fatal mark
            // refuses a failure that is not fatal, and the sweep's thread ended unseen.
            Err(mut refused) if !refused.is_fatal() => {
                refused.lifecycle.instance_id = Some(instance_id);
                self.append_lifecycle_failure(
                    RuntimeLifecycleFailureStage::SessionClose,
                    RuntimeLifecycleFailure::Host(&refused),
                    links,
                    None,
                )?;
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    /// Workflow #437 MAPP-1d (`contracts/device-group.md`, "Device-level actions", row 7): whether
    /// `token` is the lease of a run of a member of a device group of several whose close in its
    /// end phase met the lapsed lease (`fence_lapsed_lease_close`) and is still owed. Such a lease
    /// end hands the device to no one, and the member's session is closed under a session close
    /// ahead before any other member's lease begins (`cleanup_token_guarded`). `false` for an
    /// instance alone on its device, before any lock.
    pub(super) fn lapsed_close_owed_in_group(&self, token: &LeaseToken) -> RuntimeHostResult<bool> {
        #[cfg(test)]
        if self.mapp1d_off_for_test.load(Ordering::Acquire) {
            return Ok(false);
        }
        if !self.in_group_of_several(token.instance_id()) {
            return Ok(false);
        }
        Ok(self
            .run_end_control_of_lease(token.lease_id())?
            .is_some_and(|control| control.lapsed_close_owed()))
    }

    /// Workflow #437 MAPP-1b, MAPP-1c (`contracts/device-group.md`, "Device-level actions", rows
    /// 7 and 15, and "The session close of a keyless observation"; register §1.330, §1.350): a
    /// keyless observation of a member of a device group of several, served on a free device
    /// under the device's admission guard (`admission`), keeps no session: the member's session is
    /// closed before the guard frees the device, so no other member's lease begins while it is
    /// open (every grant of the device's key waits for, or tries and leaves, that guard). The
    /// close runs under a session close ahead (`close_session_ahead_while_guarded`), which neither
    /// a claim that waits in the device's queue nor the device's takeover cooldown refuses (the
    /// two cases MAPP-1b left open), and which hands the device to no one when it ends: the
    /// guard's drop grants the claim that waited first, in its place. A device a lease holds (the
    /// member's own: another member's refused the observation) leaves the session to that lease's
    /// end, which closes it. A close that fails fails the observation, its records written by the
    /// close path. An instance alone on its device keeps its session, as before.
    pub(super) fn close_keyless_session_of_group(
        &self,
        instance_id: InstanceId,
        links: EventLinksDraft,
        admission: &MutexGuard<'_, ()>,
    ) -> Result<(), RequestFailure> {
        if !self.in_group_of_several(instance_id)
            || lock(&self.scheduler, HostLocation::ReadResourceCloseLease)?
                .active_lease(instance_id)
                .is_some()
        {
            return Ok(());
        }
        #[cfg(test)]
        if self.mapp1c_off_for_test.load(Ordering::Acquire) {
            return self.close_keyless_session_before_mapp1c(instance_id, links, admission);
        }
        match self.close_session_ahead_while_guarded(instance_id, links, admission) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(close_error)) => match self.settle_retained_close_failure(
                HostLocation::CloseExecutionSession,
                instance_id,
                &close_error,
            )? {
                None => Ok(()),
                Some(error) => Err(keyless_close_failed(error)),
            },
            Err(error) => Err(keyless_close_failed(error)),
        }
    }

    /// Workflow #437 MAPP-1c (`contracts/device-group.md`, "The session close of a keyless
    /// observation"): the close of the session a keyless observation of `instance_id`, a member
    /// of a device group of several, opened on a free device, under the device's admission guard
    /// (`admission`) that the observation holds, so no lease of any member begins before it ends.
    /// It closes under a session close ahead (`SeedScheduler::prepare_session_close_ahead`): a
    /// resource-close-only lease granted whatever claim waits in the device's queue and while
    /// the device's takeover cooldown runs, held for this one close and released, once the close
    /// is confirmed, with `LeaseReleaseReason::SessionClosed`, which hands the device to no one.
    /// The queue is not touched, so the guard's drop grants the claim that waited first, in its
    /// place, with the token its own grant gives it; in a cooldown the claim waits for the
    /// cooldown's end, as before. Nothing is taken when the member has no session. A failed close
    /// is returned as the close path recorded (and, unconfirmed, settled) it; an unconfirmed close
    /// keeps its lease, as every dedicated close lease is kept then.
    fn close_session_ahead_while_guarded(
        &self,
        instance_id: InstanceId,
        links: EventLinksDraft,
        admission: &MutexGuard<'_, ()>,
    ) -> RuntimeHostResult<Result<(), ExecutionKernelError>> {
        if !self
            .execution()?
            .has_owned_resources(instance_id)
            .map_err(|error| {
                RuntimeHostError::execution(HostLocation::InspectRetainedSession, &error)
            })?
        {
            return Ok(Ok(()));
        }
        let (token, connection_id) = self.grant_session_close_ahead_lease(instance_id)?;
        let result = self
            .close_instance_resources_result(&token, connection_id, links)
            .map_err(|failure| *failure.error)?;
        // As for every dedicated close lease (`close_retained_instance_while_guarded`): a close
        // that failed without leaving its resources unconfirmed ended its step and holds nothing.
        let confirmed =
            result
                .as_ref()
                .err()
                .is_none_or(|error| match error.resource_quiescence() {
                    Some(ResourceQuiescence::Confirmed) => true,
                    Some(ResourceQuiescence::Unconfirmed) => false,
                    None => !self.fatal.is_shutdown_requested(),
                });
        if confirmed {
            self.cleanup_token_inner(
                &token,
                connection_id,
                LeaseReleaseReason::SessionClosed,
                None,
                Some(admission),
            )?;
        }
        Ok(result)
    }

    pub(super) fn close_instance_resources_result(
        &self,
        token: &LeaseToken,
        connection_id: ConnectionId,
        links: EventLinksDraft,
    ) -> Result<Result<(), ExecutionKernelError>, RequestFailure> {
        self.retire_instance_resources(
            token,
            token.instance_id(),
            connection_id,
            links,
            ResourceCloseTarget::Session,
        )
    }

    /// Workflow #437 MAPP-2a: [`Self::close_instance_resources_result`] of `member`, a member of
    /// the device group whose key `token` is (another member's): the session the stop of its
    /// application used is closed under that key, so no two members' sessions stay open. A
    /// member that is not another member of the token's device group of several is refused
    /// before anything (`instance_mismatch`).
    pub(super) fn close_member_resources_result(
        &self,
        token: &LeaseToken,
        member: InstanceId,
        connection_id: ConnectionId,
        links: EventLinksDraft,
    ) -> Result<Result<(), ExecutionKernelError>, RequestFailure> {
        let holder = token.instance_id();
        if !self.in_group_of_several(holder)
            || member == holder
            || !self.device_members(holder).contains(&member)
        {
            return Err(RequestFailure::request(
                super::switch::member_mismatch(),
                RuntimeReceiptState::Denied,
                None,
            ));
        }
        self.retire_instance_resources(
            token,
            member,
            connection_id,
            links,
            ResourceCloseTarget::Session,
        )
    }

    /// The caller keeps admission through the following Business step so capture cannot
    /// reopen an independent backend between its fenced close and the application action.
    /// `instance_id` is the instance whose application the step drives: the token's own, or
    /// (Workflow #437 MAPP-2a) another member of the token's device group.
    pub(super) fn prepare_application_resources_result(
        &self,
        token: &LeaseToken,
        instance_id: InstanceId,
        connection_id: ConnectionId,
        links: EventLinksDraft,
        _admission: &MutexGuard<'_, ()>,
    ) -> Result<Result<(), ExecutionKernelError>, RequestFailure> {
        self.retire_instance_resources(
            token,
            instance_id,
            connection_id,
            links,
            ResourceCloseTarget::ApplicationBackends,
        )
    }

    /// Retires `instance_id`'s resources under `token`'s key: its own instance's, or (Workflow
    /// #437 MAPP-2a) another member's of its device group.
    fn retire_instance_resources(
        &self,
        token: &LeaseToken,
        instance_id: InstanceId,
        connection_id: ConnectionId,
        links: EventLinksDraft,
        target: ResourceCloseTarget,
    ) -> Result<Result<(), ExecutionKernelError>, RequestFailure> {
        if let Some(error) = self
            .execution()?
            .unconfirmed_instance_close_error(instance_id)
            .map_err(|error| {
                RequestFailure::poison_without_terminal(RuntimeHostError::execution(
                    HostLocation::ReadExecutionCloseResult,
                    &error,
                ))
            })?
        {
            return Ok(Err(error));
        }
        let has_session = self
            .execution()?
            .has_owned_resources(instance_id)
            .map_err(|error| {
                RequestFailure::poison_without_terminal(RuntimeHostError::execution(
                    HostLocation::InspectExecutionSession,
                    &error,
                ))
            })?;
        if !has_session {
            self.record_owner_resource_close()?;
            return Ok(Ok(()));
        }
        // Workflow #475, test only: a test's hook runs once here, before the close asks its write
        // authority, as a device call that outlasts the lease lets it lapse there.
        #[cfg(test)]
        {
            let hook = lock(
                &self.resource_close_hook_for_test,
                HostLocation::BeginDestructiveResourceClose,
            )?
            .take();
            if let Some(hook) = hook {
                hook();
            }
        }
        let witness = Arc::new(
            lock(&self.scheduler, HostLocation::BeginDestructiveResourceClose)?
                .begin_resource_close(token, connection_id, self.monotonic_ms()?)
                .map_err(|error| {
                    RequestFailure::poison_without_terminal(
                        RuntimeHostError::scheduler(
                            HostLocation::BeginDestructiveResourceClose,
                            &error,
                        )
                        .naming_lease(token.lease_id()),
                    )
                })?,
        );

        let authority = DeviceCloseAuthority::FencedDeviceWrite(Arc::clone(&witness));
        let input_check = self
            .nemu_close_check(instance_id, token, Arc::clone(&witness), connection_id)
            .map_err(RequestFailure::poison_without_terminal)?;
        let closed = match target {
            ResourceCloseTarget::Session => self.execution()?.close_instance_with_input_check(
                instance_id,
                authority,
                input_check,
            ),
            ResourceCloseTarget::ApplicationBackends => self
                .execution()?
                .prepare_application_resources(instance_id, authority, input_check),
        };
        match closed {
            Ok(outcome) => {
                self.append_stdio_close_observations(
                    outcome.vendor_stdio(),
                    Some(instance_id),
                    links.clone(),
                )
                .map_err(RequestFailure::poison_without_terminal)?;
                let owner_disposition = self.record_owner_resource_close()?;
                self.append_lifecycle_observed(
                    RuntimeLifecyclePhase::ResourceQuiescence {
                        instance_id,
                        resource_count: outcome.resource_count(),
                        quiescence: outcome.quiescence(),
                        owner_disposition,
                        resource_dispositions: outcome.resource_dispositions(),
                    },
                    links,
                )
                .map_err(RequestFailure::poison_without_terminal)?;
                self.finish_destructive_input(Some(witness), connection_id)?;
                Ok(Ok(()))
            }
            Err(execution_error) => {
                let error = RuntimeHostError::execution(
                    HostLocation::CloseExecutionSession,
                    &execution_error,
                );
                let lifecycle_result = self.append_lifecycle_failure(
                    RuntimeLifecycleFailureStage::SessionClose,
                    RuntimeLifecycleFailure::Host(&error),
                    links.clone(),
                    None,
                );
                if error.lifecycle.resource_quiescence == Some(ResourceQuiescence::Unconfirmed) {
                    // Workflow #403: during shutdown the whole Runtime retains its owner, as
                    // before (`settle_unconfirmed_resources`); the failure is recorded above.
                    if self.fatal.is_shutdown_requested() {
                        let retained = self.retain_owner_unconfirmed(&error);
                        lifecycle_result.map_err(RequestFailure::poison_without_terminal)?;
                        retained.map_err(RequestFailure::poison_without_terminal)?;
                        return Ok(Err(execution_error));
                    }
                    lifecycle_result.map_err(RequestFailure::poison_without_terminal)?;
                    // Workflow #403: the instance's own resources fence only that instance; its
                    // close step ends as after any failed close once its write authority is
                    // back (a write still in flight keeps the whole-Runtime retention).
                    let settled = self
                        .settle_unconfirmed_resources(&error, Some(instance_id), links)
                        .map_err(RequestFailure::poison_without_terminal)?;
                    if let UnconfirmedResources::Fenced(_) = settled {
                        self.finish_destructive_input(Some(witness), connection_id)?;
                    }
                    return Ok(Err(execution_error));
                }
                lifecycle_result.map_err(RequestFailure::poison_without_terminal)?;
                self.record_owner_resource_close()?;
                self.finish_destructive_input(Some(witness), connection_id)?;
                Ok(Err(execution_error))
            }
        }
    }

    /// Grants a dedicated lease on `instance_id` to the Runtime-owned connection
    /// `connection_value`: a resource-close-only lease (`prepare_resource_close`: a waiting entry
    /// the scheduling pause does not hold back is `TransferNotSafe`, Workflow #369 §5.1),
    /// granted with `CapacityUse::Drain`. `kind` names its holder (Workflow #369 Q-2; review
    /// L2). #480 PG-1 fix round 1 (item 3): while an automatic start of the instance is held
    /// (`automatic_start_hold`: a scheduling pause, its own or the global one), its own queued
    /// autostart claim, which ends at its grant without a Start, neither refuses the lease nor is
    /// jumped by it (`AutostartEndsAtGrant`); it is granted after the lease, in its place. Fix
    /// round 2: so for every caller, the resource-close lease of
    /// `close_retained_instance_while_guarded` (a pause's hand-back outside the multi-Nemu gate
    /// among them) and the daemon start's preparation of a physical instance
    /// (`ClaimKind::DaemonStartPreparation`, `prepare_physical_instance_on_start`). The
    /// caller holds the instance admission guard. Returns the token, its connection and the
    /// grant's request id.
    pub(super) fn grant_dedicated_instance_lease(
        &self,
        instance_id: InstanceId,
        connection_value: u64,
        kind: ClaimKind,
    ) -> RuntimeHostResult<(LeaseToken, ConnectionId, RequestId)> {
        self.grant_runtime_lease(
            instance_id,
            connection_value,
            |request_id, holder_id, connection_id| {
                let gate = self.routine_gate(instance_id)?;
                let autostart_ends = self.autostart_ends_at_grant(
                    instance_id,
                    HostLocation::PrepareResourceCloseLease,
                )?;
                let mut scheduler = lock(&self.scheduler, HostLocation::PrepareResourceCloseLease)?;
                let now_monotonic_ms = self.monotonic_ms()?;
                let prepared = if autostart_ends {
                    scheduler.prepare_dedicated_lease(
                        request_id,
                        instance_id,
                        holder_id,
                        connection_id,
                        kind,
                        actingcommand_scheduler::AutostartEndsAtGrant {
                            gates: gate,
                            member: instance_id,
                        },
                        now_monotonic_ms,
                    )
                } else {
                    scheduler.prepare_dedicated_lease(
                        request_id,
                        instance_id,
                        holder_id,
                        connection_id,
                        kind,
                        gate,
                        now_monotonic_ms,
                    )
                };
                prepared.map_err(|error| {
                    RuntimeHostError::scheduler(HostLocation::PrepareResourceCloseLease, &error)
                })
            },
        )
    }

    /// Workflow #437 MAPP-1c: the session close ahead of `instance_id`'s device
    /// (`SeedScheduler::prepare_session_close_ahead`), granted as a dedicated close lease is,
    /// to the Runtime's close connection. The caller holds the device's admission guard.
    fn grant_session_close_ahead_lease(
        &self,
        instance_id: InstanceId,
    ) -> RuntimeHostResult<(LeaseToken, ConnectionId)> {
        self.grant_runtime_lease(
            instance_id,
            RESOURCE_CLOSE_CONNECTION_VALUE,
            |request_id, holder_id, connection_id| {
                lock(&self.scheduler, HostLocation::PrepareResourceCloseLease)?
                    .prepare_session_close_ahead(
                        request_id,
                        instance_id,
                        holder_id,
                        connection_id,
                        self.monotonic_ms()?,
                    )
                    .map_err(|error| {
                        RuntimeHostError::scheduler(HostLocation::PrepareResourceCloseLease, &error)
                    })
            },
        )
        .map(|(token, connection_id, _)| (token, connection_id))
    }

    /// A Runtime-owned lease on `instance_id` to the connection `connection_value`, prepared by
    /// `prepare` with the grant's request id, holder id and connection and granted with
    /// `CapacityUse::Drain` under the grant's own links. Returns the token, its connection and the
    /// grant's request id.
    fn grant_runtime_lease(
        &self,
        instance_id: InstanceId,
        connection_value: u64,
        prepare: impl FnOnce(
            RequestId,
            actingcommand_contract::HolderId,
            ConnectionId,
        ) -> RuntimeHostResult<LeasePreparation>,
    ) -> RuntimeHostResult<(LeaseToken, ConnectionId, RequestId)> {
        let resolved = lock(
            &self.registered_instances,
            HostLocation::ReadResourceCloseInstance,
        )?
        .get(&instance_id)
        .cloned()
        .ok_or_else(|| {
            RuntimeHostError::fatal(
                HostCode::ResourceCloseInstanceMissing,
                HostLocation::AcquireResourceCloseLease,
                RuntimeErrorCode::RuntimeHalted,
            )
        })?;
        let request_id = self
            .events
            .issuer()
            .mint_request_id()
            .map_err(|_| runtime_identifier_error())?;
        let holder_id = self
            .events
            .issuer()
            .mint_holder_id()
            .map_err(|_| runtime_identifier_error())?;
        let connection_id = ConnectionId::new(connection_value).map_err(|error| {
            RuntimeHostError::scheduler(HostLocation::BuildResourceCloseConnection, &error)
        })?;
        let preparation = prepare(
            *request_id.transport(),
            *holder_id.transport(),
            connection_id,
        )?;
        let token = preparation.token().clone();
        let grant_request_id = *request_id.transport();
        let grant_links = self
            .events
            .synthetic_links(&token, self.events.action_id()?)?
            .with_request_id(request_id);
        self.grant_prepared_lease_with_links(
            &resolved,
            preparation,
            grant_links,
            CapacityUse::Drain,
        )
        .map_err(|failure| *failure.error)?;
        Ok((token, connection_id, grant_request_id))
    }

    /// The instance admission guard excludes capture registration and business native calls.
    /// A dedicated close lease is released with `release_reason` once the close is confirmed.
    pub(super) fn close_retained_instance_while_guarded(
        &self,
        instance_id: InstanceId,
        links: EventLinksDraft,
        reuse_active_lease: bool,
        release_reason: LeaseReleaseReason,
        admission: &MutexGuard<'_, ()>,
    ) -> RuntimeHostResult<Result<(), ExecutionKernelError>> {
        if !self
            .execution()?
            .has_owned_resources(instance_id)
            .map_err(|error| {
                RuntimeHostError::execution(HostLocation::InspectRetainedSession, &error)
            })?
        {
            return Ok(Ok(()));
        }
        let active = lock(&self.scheduler, HostLocation::ReadResourceCloseLease)?
            .active_tokens()
            .into_iter()
            .find(|token| token.instance_id() == instance_id);
        let (token, connection_id, acquired) = if let Some(token) = active {
            if !reuse_active_lease {
                return Err(RuntimeHostError::scheduler(
                    HostLocation::AcquireResourceCloseLease,
                    &SchedulerError::Busy {
                        holder_id: token.holder_id(),
                        lease_id: token.lease_id(),
                        expires_at_monotonic_ms: token.expires_at_monotonic_ms(),
                    },
                ));
            }
            let connection_id = lock(&self.scheduler, HostLocation::ReadResourceCloseConnection)?
                .connection_for_token(&token)
                .map_err(|error| {
                    RuntimeHostError::scheduler(HostLocation::ReadResourceCloseConnection, &error)
                })?;
            (token, connection_id, false)
        } else {
            let (token, connection_id, _) = self.grant_dedicated_instance_lease(
                instance_id,
                RESOURCE_CLOSE_CONNECTION_VALUE,
                ClaimKind::ResourceClose,
            )?;
            (token, connection_id, true)
        };
        let result = self
            .close_instance_resources_result(&token, connection_id, links)
            .map_err(|failure| *failure.error)?;
        // ISO-2 (#419): outside a shutdown, a close that failed without leaving its resources
        // unconfirmed ended its step and holds nothing (`retire_instance_resources`), so its
        // dedicated lease ends too, and the work that asked for the close goes on
        // (`settle_retained_close_failure`). A shutdown keeps its close as before.
        let confirmed =
            result
                .as_ref()
                .err()
                .is_none_or(|error| match error.resource_quiescence() {
                    Some(ResourceQuiescence::Confirmed) => true,
                    Some(ResourceQuiescence::Unconfirmed) => false,
                    None => !self.fatal.is_shutdown_requested(),
                });
        if acquired && confirmed {
            self.cleanup_token_inner(&token, connection_id, release_reason, None, Some(admission))?;
        }
        Ok(result)
    }

    pub(super) fn finish_input_failure(
        &self,
        primary: ExecutionKernelError,
        token: &LeaseToken,
        step: Option<Arc<FencedWrite>>,
        connection_id: ConnectionId,
        links: EventLinksDraft,
    ) -> RuntimeHostResult<ExecutionKernelError> {
        if primary.resource_quiescence() == Some(ResourceQuiescence::Unconfirmed) {
            // Workflow #403: the caller fences the instance; the input's own step ends once its
            // write authority is back (a write still in flight keeps the whole-Runtime
            // retention, `fenced_step_not_reclaimed`).
            if step.is_some() {
                self.finish_destructive_input(step, connection_id)
                    .map_err(|failure| *failure.error)?;
            }
            return Ok(primary);
        }
        let close_result: RuntimeHostResult<Result<(), ExecutionKernelError>> = (|| {
            let instance_guard = self
                .instance_guard(token.instance_id())
                .map_err(|failure| *failure.error)?;
            let _admission = lock(&instance_guard, HostLocation::LockInstanceAdmission)?;
            self.finish_destructive_input(step, connection_id)
                .map_err(|failure| *failure.error)?;
            match self.close_instance_resources_result(token, connection_id, links.clone()) {
                Ok(closed) => Ok(closed),
                // Workflow #461 (`contracts/read-session-resource-close.md`): the input's lease
                // lapsed while the input was under way, and the lapsed lease refused this close
                // before it began (`expiry_close_refused`). Nothing was closed. As at the lease's
                // expiry (#458), the refusal is recorded once, it fences this instance only and
                // the close is owed (`fence_expired_lease_close`), still under the admission
                // guard; the lease's end (the input's own release, or the expiry) closes the
                // session once the lease has ended (`close_expired_lease_session_guarded`). No
                // owner is retained and nothing is latched: the input fails with its own failure.
                // A failure of the Runtime itself in the fence (its record) is returned, and stays
                // fatal below.
                Err(refused) if self.expiry_close_refused(&refused.error) => self
                    .fence_expired_lease_close(token, *refused.error, links.clone())
                    .map(Ok),
                Err(failure) => Err(*failure.error),
            }
        })();
        match close_result {
            Ok(Ok(())) => Ok(primary),
            Ok(Err(cleanup)) => Ok(ExecutionKernelError::merge_cleanup(primary, cleanup)),
            Err(cleanup) => {
                let primary =
                    RuntimeHostError::execution(HostLocation::ExecuteInputBackend, &primary);
                self.append_lifecycle_failure(
                    RuntimeLifecycleFailureStage::SessionClose,
                    RuntimeLifecycleFailure::Host(&primary),
                    links.clone(),
                    None,
                )?;
                let cleanup = cleanup.into_fatal();
                self.append_lifecycle_failure(
                    RuntimeLifecycleFailureStage::SessionClose,
                    RuntimeLifecycleFailure::Host(&cleanup),
                    links,
                    None,
                )?;
                lock(&self.owner, HostLocation::RetainUnconfirmedOwner)?.retain_unconfirmed()?;
                self.fatal.mark(cleanup.clone())?;
                Err(cleanup)
            }
        }
    }

    pub(super) fn finish_capture_failure_while_guarded(
        &self,
        primary: ExecutionKernelError,
        links: EventLinksDraft,
        admission: &MutexGuard<'_, ()>,
    ) -> RuntimeHostResult<ExecutionKernelError> {
        if primary.code().as_str()
            == actingcommand_execution_kernel::ExecutionKernelCode::ExecutionSessionClosePending
                .as_str()
        {
            return Ok(primary);
        }
        let Some(instance_id) = primary.instance_id() else {
            return Ok(primary);
        };
        match self.close_retained_instance_while_guarded(
            instance_id,
            links.clone(),
            true,
            LeaseReleaseReason::HostShutdown,
            admission,
        ) {
            Ok(Ok(())) => Ok(primary),
            Ok(Err(cleanup)) => Ok(ExecutionKernelError::merge_cleanup(primary, cleanup)),
            // #411 A52: the dedicated close lease was refused because an entry waits in the
            // instance's queue (`prepare_resource_close`, Workflow #369 §5.1). No close was
            // attempted, so nothing is unconfirmed and nothing is fenced (Workflow #403 fences
            // an unconfirmed close): the refusal is recorded once and the capture failure ends
            // only its own work. The session keeps its backends and must close: the waiting
            // entry, granted once this work lets the instance go, closes it at its lease end or
            // in its preparation (`read-session-resource-close.md`).
            Err(mut refused) if close_lease_refused_by_queue(&refused) => {
                refused.lifecycle.instance_id = Some(instance_id);
                self.append_lifecycle_failure(
                    RuntimeLifecycleFailureStage::SessionClose,
                    RuntimeLifecycleFailure::Host(&refused),
                    links,
                    None,
                )?;
                Ok(primary)
            }
            // Workflow #456 (`contracts/read-session-resource-close.md`): the close was refused
            // before it began for a reason of this instance's own key (the takeover cooldown, a
            // queued claim its routine gate holds back, the lapsed lease the close reused). As an
            // unconfirmed close does, the refusal fences this instance only (Workflow #403): it is
            // recorded, the instance is fenced, and the capture failure ends only its own work.
            // The session keeps its backends and must close: the lapsed lease's expiry closes it
            // once that lease has ended (#458, `close_expired_lease_session_guarded`), a waiting
            // entry at its lease end, and a preparation (the fence's ladder or recheck, a person's
            // self-check or resume) before it opens. During shutdown it ends the Runtime as before.
            Err(refused)
                if self.refused_close_isolates()
                    && !self.fatal.is_shutdown_requested()
                    && close_refused_for_instance(&refused) =>
            {
                self.fence_refused_close(instance_id, refused, links, true)?;
                Ok(primary)
            }
            Err(cleanup) => {
                let primary =
                    RuntimeHostError::execution(HostLocation::FinishCaptureFailure, &primary);
                self.append_lifecycle_failure(
                    RuntimeLifecycleFailureStage::SessionClose,
                    RuntimeLifecycleFailure::Host(&primary),
                    links.clone(),
                    None,
                )?;
                let cleanup = cleanup.into_fatal();
                self.append_lifecycle_failure(
                    RuntimeLifecycleFailureStage::SessionClose,
                    RuntimeLifecycleFailure::Host(&cleanup),
                    links,
                    None,
                )?;
                lock(&self.owner, HostLocation::RetainUnconfirmedOwner)?.retain_unconfirmed()?;
                self.fatal.mark(cleanup.clone())?;
                Err(cleanup)
            }
        }
    }

    // The controlled preparation phase of a physical instance's device connection (Workflow #317
    // sc3, the strict reading of #316 goal 5).
    //
    // `prepare_instance_connection` connects and self-checks one physical instance outside any
    // client lease: under the instance admission guard it takes a dedicated preparation lease (the
    // close-lease precedent: a resource-close-only lease of a fixed Runtime connection, granted
    // with `CapacityUse::Drain`; a waiting lease queue is `TransferNotSafe`; Workflow #369 W-3: a
    // resume's reconnect and a self-check run on their operator claim's key instead, H-4 (S6b):
    // an emulator start or restart on its control's key, and a stuck-recovery ladder on its own
    // key), closes a session a
    // lease-free path retained (a read-only observe keeps its capture open, sc2; Workflow #191
    // h3) through the fenced close path, opens the instance's input and capture backends through
    // `ExecutionKernel::open_instance_backends` (a Nemu pair once) so the opens cover every
    // required entry, records the opens exactly as every open is recorded
    // (`backend.open_observed`, the `backend.selfcheck.*` facts, one `device.self_check` status
    // hint per entry and the policy availability they gate), then releases the lease and keeps
    // the session open for the instance's next leases (Workflow #191 H; a failed open, or an
    // instance outside the multi-Nemu gate, closes it first). It sends no input and keeps no
    // frame. A failing step is recorded and
    // leaves the instance unavailable; nothing is retried except, once, a daemon start's lease
    // refused by the takeover cooldown (Workflow #191 h2). Only a fatal failure (a ledger append,
    // an unconfirmed close) is returned.
    //
    // Triggers: daemon start (every registered physical instance in order, before the host
    // answers), emulator control `start` / `restart`, an instance `ResumeScheduling` and the
    // explicit `SelfCheckInstance` request. The automatic triggers and the availability gate
    // concern device self-checked instances only (`RegisteredInstance::device_self_checked`):
    // fixtures and providers without a device endpoint are exempt.

    /// Trigger (a): at daemon start, after the registry and the fact seeds and before any
    /// thread is spawned, withdraws the policy availability of every registered physical
    /// instance and runs its preparation phase, in registry order. A failed preparation leaves
    /// its instance unavailable and does not stop the start; only a fatal failure does.
    /// Workflow #191 h2: an instance whose preparation lease the takeover cooldown refused
    /// (`lease_cooldown`) is prepared once more after the first pass, once that cooldown ended;
    /// every takeover cooldown ends at the same deadline (the scheduler's start plus
    /// `takeover_cooldown_ms`), which bounds the whole wait. A second refusal is recorded like
    /// the first and is a failed self-check (`lease_cooldown`). One unreachable device leaves only
    /// its instance unavailable (Workflow #434 LAB-6S4a: the held start's verdict over every
    /// prepared instance, INST-1, went with the held start).
    pub(super) fn prepare_physical_instances_on_start(&self) -> RuntimeHostResult<()> {
        let instances = lock(
            &self.registered_instances,
            HostLocation::ListPhysicalInstances,
        )?
        .values()
        .filter(|instance| instance.device_self_checked())
        .map(|instance| (instance.instance_alias.clone(), instance.instance_id))
        .collect::<Vec<_>>();
        let mut cooling = Vec::new();
        for (instance_alias, instance_id) in instances {
            self.check_install_preparation()?;
            self.withhold_policy_instance_availability(instance_id)?;
            if lock(
                &self.scheduling_pause,
                HostLocation::ReadInstallStartupPause,
            )?
            .instance_state(&instance_alias)
            .is_some_and(|pause| {
                pause.stage == actingcommand_contract::InstancePauseStage::Released
            }) {
                continue;
            }
            if let (_, Some(cooldown_until)) =
                self.prepare_physical_instance_on_start(&instance_alias, instance_id)?
            {
                cooling.push((instance_alias, instance_id, cooldown_until));
            }
        }
        for (instance_alias, instance_id, cooldown_until) in cooling {
            let wait_ms = cooldown_until.saturating_sub(self.monotonic_ms()?);
            thread::sleep(Duration::from_millis(wait_ms));
            self.check_install_preparation()?;
            self.prepare_physical_instance_on_start(&instance_alias, instance_id)?;
        }
        Ok(())
    }

    /// One start preparation of `instance_id` under its admission guard. Returns its self-check
    /// and the monotonic deadline of the takeover cooldown that refused its preparation lease,
    /// if one did (the self-check then fails with `lease_cooldown`).
    pub(super) fn prepare_physical_instance_on_start(
        &self,
        instance_alias: &str,
        instance_id: InstanceId,
    ) -> RuntimeHostResult<(SchedulingResumeSelfCheck, Option<u64>)> {
        let instance_guard = self
            .instance_guard(instance_id)
            .map_err(|failure| *failure.error)?;
        let admission = lock(&instance_guard, HostLocation::LockInstanceAdmission)?;
        let prepared =
            self.prepare_physical_instance_while_guarded(instance_alias, instance_id, &admission)?;
        Ok((prepared.selfcheck, prepared.cooldown_until))
    }

    /// [`Self::prepare_physical_instance_on_start`] once the caller holds the instance's
    /// admission guard (`admission`): the start's preparation on its dedicated lease.
    fn prepare_physical_instance_while_guarded(
        &self,
        instance_alias: &str,
        instance_id: InstanceId,
        admission: &MutexGuard<'_, ()>,
    ) -> RuntimeHostResult<PreparationAttempt> {
        let links = self
            .events
            .system_links()?
            .with_instance_id(self.events.issuer().issue_registered_instance(instance_id));
        // Workflow #429 R-0b (register §1.238): the start's self-check shows while it prepares
        // the instance on its dedicated lease; no request names it.
        let _activity = self.begin_instance_activity(
            instance_id,
            super::instance_activity::ActivityStart::Selfcheck { request_id: None },
        )?;
        self.prepare_instance_connection_with_cooldown(
            instance_alias,
            instance_id,
            links,
            admission,
            actingcommand_contract::RecoveryTriggerStage::StartupPreparation,
            PreparationLease::Dedicated(ClaimKind::DaemonStartPreparation),
            EmulatorStatusRead::Skip,
        )
    }

    /// Workflow #437 G1 fix rounds 1 and 2 (`contracts/device-group.md`, "A failure stays where
    /// it belongs"): the lease sweep's step for each member of a device group of several whose
    /// start preparation the device's hold refused (`device_held_for_member_close`, recorded once
    /// as that preparation's failure, which left the member unavailable). Once the fence that held
    /// the device is lifted and the device is idle, it only queues, for each such member, one
    /// self-check claim of the Runtime's own (`queue_held_start_preparation`), as a fence's
    /// recheck is queued (`queue_fence_recheck`); the member's worker makes the start's
    /// preparation on that claim's key (`run_held_start_preparation`), so the sweep waits for no
    /// device step (Workflow #369 Q-8). A member is passed over while another member's fence
    /// holds its device, while its scheduling is paused with a released pause (as the start
    /// passes it over; its resume prepares it), inside a blackout of the member (a preparation the
    /// Runtime starts on its own, as a recheck: `scheduling-blackout.md`, "Complete silence") and
    /// while the device is not idle; it stays owed and is looked at again at the next tick. A
    /// member whose claim waits or runs is not queued again, and a member owes nothing once a
    /// preparation of it is granted its key or lease. Memory only: a restart prepares every
    /// member. The train's fix round 1: idle is no lease on the device, no takeover cooldown and
    /// no entry of its queue that is eligible now by the scheduler's own judgement
    /// (`SeedScheduler::eligible_count`, the predicate of `first_eligible` and of the queue's
    /// grant), each entry by its own member's gate as the pump reads them (`device_gates`, before
    /// the scheduler's lock): an entry parked behind its own member's closed gate (another
    /// member's routine claim behind its own pause or blackout) keeps no preparation back, and one
    /// that waits eligible does, so the claim is queued only when it is the device's next work.
    pub(super) fn queue_members_after_device_hold(&self) -> RuntimeHostResult<()> {
        if self.fatal.is_shutdown_requested() {
            return Ok(());
        }
        let owed = lock(
            &self.start_preparations_held,
            HostLocation::ListHeldStartPreparations,
        )?
        .iter()
        .filter(|(_, held)| !held.queued)
        .map(|(instance_id, held)| (*instance_id, held.instance_alias.clone()))
        .collect::<Vec<_>>();
        for (instance_id, instance_alias) in owed {
            if self.device_held_for(instance_id)?.is_some() {
                continue;
            }
            let released = lock(
                &self.scheduling_pause,
                HostLocation::ReadHeldStartPreparationPause,
            )?
            .instance_state(&instance_alias)
            .is_some_and(|pause| {
                pause.stage == actingcommand_contract::InstancePauseStage::Released
            });
            if released
                || self
                    .blackout_covering(instance_id, &instance_alias)?
                    .is_some()
            {
                continue;
            }
            let gates = self.device_gates(instance_id)?;
            let gate = |member: InstanceId| gates.gate_of(member);
            let now_monotonic_ms = self.monotonic_ms()?;
            let idle = {
                let scheduler = lock(
                    &self.scheduler,
                    HostLocation::ReadHeldStartPreparationDevice,
                )?;
                scheduler.active_lease(instance_id).is_none()
                    && scheduler.eligible_count(instance_id, gate, now_monotonic_ms) == 0
                    && !scheduler.cooldown_active(instance_id, now_monotonic_ms)
            };
            if !idle {
                continue;
            }
            let marked = match lock(
                &self.start_preparations_held,
                HostLocation::MarkHeldStartPreparationQueued,
            )?
            .get_mut(&instance_id)
            {
                Some(held) if !held.queued => {
                    held.queued = true;
                    true
                }
                _ => false,
            };
            if marked {
                self.queue_held_start_preparation(instance_id, &instance_alias)?;
            }
        }
        Ok(())
    }

    /// Workflow #437 G1 fix round 2: queues the start's preparation of `instance_id` on one
    /// self-check claim of the Runtime's own (a minted holder on the Runtime's fixed connection,
    /// the scheduler's lease time), as `queue_fence_recheck` queues a recheck (no new claim kind):
    /// granted at once or later in its place by the device's queue, it goes to the member's worker
    /// (`HostClaimWork::HeldStartPreparation`, a payload of its own: a recheck's runs only for a
    /// fence's wait for a person, which the member has not, and a link's restore reports to the
    /// handset link machine). A claim the queue refuses is recorded once (`runtime.failed`, stage
    /// `operation_cleanup`) and settles what the member owes by the refusal's own code: refused
    /// `device_held_for_member_close` (a self-check claim is an operator's, refused at once while
    /// another member's fence holds the device again), the member stays owed and is queued once
    /// that fence is lifted; refused otherwise, it owes nothing more and stays unavailable until a
    /// person's self-check or resume, as after a failed preparation. Only a failure the Runtime
    /// does not survive is returned.
    fn queue_held_start_preparation(
        &self,
        instance_id: InstanceId,
        instance_alias: &str,
    ) -> RuntimeHostResult<()> {
        let resolved = lock(
            &self.registered_instances,
            HostLocation::QueueHeldStartPreparation,
        )?
        .get(&instance_id)
        .cloned()
        .ok_or_else(|| {
            RuntimeHostError::fatal(
                HostCode::RuntimeInstanceRegistryIncomplete,
                HostLocation::QueueHeldStartPreparation,
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
                instance_alias: instance_alias.to_owned(),
                holder_id,
            },
        )
        .map_err(|_| {
            RuntimeHostError::fatal(
                HostCode::HostKeyRequestInvalid,
                HostLocation::QueueHeldStartPreparation,
                RuntimeErrorCode::RuntimeHalted,
            )
        })?;
        let connection_id =
            ConnectionId::new(STARTUP_PACKAGE_CONNECTION_VALUE).map_err(|error| {
                RuntimeHostError::scheduler(HostLocation::QueueHeldStartPreparation, &error)
            })?;
        let lease_ttl_ms = lock(
            &self.scheduler,
            HostLocation::ReadHeldStartPreparationClaimTtl,
        )?
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
                instance_alias,
                holder_id,
                connection_id,
                kind: ClaimKind::SelfCheck,
                priority: actingcommand_contract::LeasePriority::Normal,
                lease_ttl_ms,
            },
            HostClaimWork::HeldStartPreparation(PendingHeldStartPreparation {
                instance_alias: instance_alias.to_owned(),
                instance_id,
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
                    links,
                    None,
                )?;
                // The train's fix round 1: decided from the refusal's own code, not from a second
                // read of the hold: refused for another member's hold that came back between the
                // step's read and the claim, the member stays owed; any other refusal settles it.
                let held_again =
                    failure.error.code() == HostCode::DeviceHeldForMemberClose.as_str();
                let mut owed = lock(
                    &self.start_preparations_held,
                    HostLocation::SettleHeldStartPreparation,
                )?;
                if held_again {
                    if let Some(held) = owed.get_mut(&instance_id) {
                        held.queued = false;
                    }
                } else {
                    owed.remove(&instance_id);
                }
                Ok(())
            }
        }
    }

    /// Workflow #437 G1 fix round 2: a granted claim of [`Self::queue_held_start_preparation`] on
    /// the member's worker: the start's preparation (stage `startup_preparation`, no emulator
    /// status read first, as the start makes it) under the device's admission guard on the
    /// claim's key, then the key's release, to the ladder a failed preparation admitted when it
    /// admitted one (as a link's restore releases it). A member that owes no preparation any more
    /// (a person's self-check or resume prepared it while the claim waited), or whose scheduling
    /// is now paused with a released pause (it stays owed, and the sweep passes it over), runs
    /// nothing and releases the key. A failed preparation is recorded as every preparation is
    /// (`instance_preparation_finished` with its failure, the failing step's `runtime.failed`) and
    /// leaves the member unavailable; nothing queues it again (its ladder, a person's self-check or
    /// resume, or a restart prepares it). Only a fatal failure is returned.
    pub(super) fn run_held_start_preparation(
        &self,
        pending: &PendingHeldStartPreparation,
        key: super::host_claims::HostKey,
    ) -> RuntimeHostResult<()> {
        let instance_id = pending.instance_id;
        let released = lock(
            &self.scheduling_pause,
            HostLocation::RunHeldStartPreparation,
        )?
        .instance_state(&pending.instance_alias)
        .is_some_and(|pause| pause.stage == actingcommand_contract::InstancePauseStage::Released);
        let owed = match lock(
            &self.start_preparations_held,
            HostLocation::RunHeldStartPreparation,
        )?
        .get_mut(&instance_id)
        {
            Some(held) if held.queued && released => {
                held.queued = false;
                false
            }
            Some(held) => held.queued,
            None => false,
        };
        if !owed {
            return self.release_host_key(&key);
        }
        let validated = key.request.validate().map_err(|_| {
            RuntimeHostError::fatal(
                HostCode::HostKeyRequestInvalid,
                HostLocation::RunHeldStartPreparation,
                RuntimeErrorCode::RuntimeHalted,
            )
        })?;
        let links = self
            .events
            .request_links(&validated, Some(instance_id), None, None);
        let instance_guard = self
            .instance_guard(instance_id)
            .map_err(|failure| *failure.error)?;
        let prepared = {
            let admission = lock(&instance_guard, HostLocation::RunHeldStartPreparation)?;
            // Workflow #429 R-0b: the preparation shows as a self-check with its claim's request.
            let _activity = self.begin_instance_activity(
                instance_id,
                super::instance_activity::ActivityStart::Selfcheck {
                    request_id: Some(key.request.request_id()),
                },
            )?;
            self.prepare_instance_connection_with_cooldown(
                &pending.instance_alias,
                instance_id,
                links,
                &admission,
                actingcommand_contract::RecoveryTriggerStage::StartupPreparation,
                PreparationLease::Held {
                    token: &key.token,
                    connection_id: key.connection_id,
                },
                EmulatorStatusRead::Skip,
            )?
        };
        match prepared.ladder {
            Some(ladder) => match self.release_to_preparation_ladder(
                &validated,
                key.request.request_id(),
                &key.token,
                key.connection_id,
                ladder,
            ) {
                Ok(()) => Ok(()),
                Err(failure) if failure.poison_runtime || failure.error.is_fatal() => {
                    Err(*failure.error)
                }
                Err(failure) => self.append_lifecycle_failure(
                    RuntimeLifecycleFailureStage::OperationCleanup,
                    RuntimeLifecycleFailure::Host(&failure.error),
                    pending.links.clone(),
                    None,
                ),
            },
            None => self.release_host_key(&key),
        }
    }

    /// Trigger (d): `SelfCheckInstance` — the operator's manual reconnect and self-check of one
    /// physical instance; the receipt carries the self-check in the resume receipt's shape.
    /// Workflow #369 W-3: a `SelfCheck` claim waits at most
    /// `CONNECTION_PREPARATION_CLAIM_WAIT_MS` for its turn, without the admission guard, and
    /// the preparation runs on its key (`prepare_on_operator_claim`). Not granted in time, the
    /// request is refused `lease_queue_expired` and no preparation runs.
    pub(super) fn self_check_instance(
        &self,
        original: &RuntimeRequest,
        request: &ValidatedRuntimeRequest<'_>,
        instance_alias: &str,
        connection_id: ConnectionId,
    ) -> Result<OperationSuccess, RequestFailure> {
        let instance_id = self.resolve_instance(instance_alias)?.instance_id();
        // Workflow #398 RD-2 (model-398 §4.3): a person's self-check of a handset starts a failed
        // device-link watch again.
        self.device_link_rewatch(instance_id)
            .map_err(RequestFailure::poison_without_terminal)?;
        let token = self.await_preparation_claim(
            original,
            instance_alias,
            connection_id,
            ClaimKind::SelfCheck,
        )?;
        // Workflow #429 R-0b (register §1.238): the self-check shows from its grant, with the
        // person's request; its guard drops however the self-check returns.
        let _activity = self
            .begin_instance_activity(
                instance_id,
                super::instance_activity::ActivityStart::Selfcheck {
                    request_id: Some(original.request_id()),
                },
            )
            .map_err(RequestFailure::poison_without_terminal)?;
        let selfcheck = self.prepare_on_operator_claim(
            request,
            original.request_id(),
            instance_alias,
            instance_id,
            &token,
            connection_id,
        )?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result: RuntimeResult::InstanceSelfChecked {
                instance_alias: instance_alias.to_owned(),
                selfcheck,
            },
            failure_outcome: None,
        })
    }

    /// Workflow #369 Q-2, Q-2a, W-3: the claim of a self-check or of an instance resume's
    /// reconnect (`kind`): an operator claim with a minted holder, the scheduler's default lease
    /// TTL and drain capacity, which skips the bound-endpoint and performance-control checks. It
    /// waits at most `CONNECTION_PREPARATION_CLAIM_WAIT_MS` for its turn.
    pub(super) fn await_preparation_claim(
        &self,
        original: &RuntimeRequest,
        instance_alias: &str,
        connection_id: ConnectionId,
        kind: ClaimKind,
    ) -> Result<LeaseToken, RequestFailure> {
        let holder_id = *self
            .events
            .issuer()
            .mint_holder_id()
            .map_err(|_| RequestFailure::poison_without_terminal(runtime_identifier_error()))?
            .transport();
        let lease_ttl_ms = lock(&self.scheduler, HostLocation::ReadPreparationClaimTtl)?
            .config()
            .lease_ttl_ms;
        self.await_operator_claim(
            HostClaim {
                request: original,
                instance_alias,
                holder_id,
                connection_id,
                kind,
                priority: actingcommand_contract::LeasePriority::Normal,
                lease_ttl_ms,
            },
            CONNECTION_PREPARATION_CLAIM_WAIT_MS,
        )
    }

    /// Workflow #369 W-3: the preparation phase of one physical instance on an operator claim's
    /// key: the admission guard is taken for the preparation's device steps only, the
    /// preparation takes no dedicated lease, and the key is released afterwards (the release
    /// hands it on, or pumps). A failed preparation's admitted ladder gets the key straight from
    /// that release (H-1, P1; `release_to_preparation_ladder`). A failed preparation that admits
    /// no ladder (stuck recovery off, a ladder the cool-down suppresses, a failure no trigger
    /// item names) first cancels the instance's queued startup claim (`scheduler.denied
    /// lease.queue_cancelled`), as an emulator control's failed preparation does, so the release
    /// never hands the key to a startup run against an instance that failed its preparation
    /// (Workflow #369 S6c review low (b)); the failure stays in the self-check and in
    /// `instance_preparation_finished`. An unconfirmed close or a failed record fails the request
    /// and leaves the key held, as a dedicated preparation lease is left then. #411 A48: this
    /// preparation is a self-check (`SelfCheckInstance`, an instance resume's reconnect), so it
    /// reads the emulator status before its opens (`selfcheck_emulator`).
    pub(super) fn prepare_on_operator_claim(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        request_id: RequestId,
        instance_alias: &str,
        instance_id: InstanceId,
        token: &LeaseToken,
        connection_id: ConnectionId,
    ) -> Result<SchedulingResumeSelfCheck, RequestFailure> {
        let links = self
            .events
            .request_links(request, Some(instance_id), None, None);
        let instance_guard = self.instance_guard(instance_id)?;
        let admission = self.lock_admission(&instance_guard, instance_id)?;
        let prepared = self.prepare_instance_connection_with_cooldown(
            instance_alias,
            instance_id,
            links,
            &admission,
            actingcommand_contract::RecoveryTriggerStage::ConnectionPreparation,
            PreparationLease::Held {
                token,
                connection_id,
            },
            EmulatorStatusRead::First,
        );
        drop(admission);
        let prepared = prepared.map_err(RequestFailure::poison_without_terminal)?;
        match prepared.ladder {
            Some(ladder) => self.release_to_preparation_ladder(
                request,
                request_id,
                token,
                connection_id,
                ladder,
            )?,
            None => {
                if prepared.selfcheck.failure_code.is_some() {
                    self.cancel_queued_startup_claims(instance_id)?;
                }
                self.release_lease(request, request_id, token, connection_id, None)?;
            }
        }
        Ok(prepared.selfcheck)
    }

    /// The preparation phase of one physical instance; the caller holds its admission guard.
    /// The opens are recorded under `links` (which name the instance). Workflow #369 H-4 (S6b):
    /// it runs on the key of the operator's emulator control (`token` on `connection_id`), takes
    /// no dedicated lease and releases nothing. Returns the self-check projected from the opens
    /// it made, with the code of the failing step, if any, and the ladder a failed preparation
    /// admitted, which the control's release hands its key to (H-1, P1;
    /// `release_to_preparation_ladder`).
    pub(super) fn prepare_instance_connection(
        &self,
        instance_alias: &str,
        instance_id: InstanceId,
        links: EventLinksDraft,
        admission: &MutexGuard<'_, ()>,
        token: &LeaseToken,
        connection_id: ConnectionId,
    ) -> RuntimeHostResult<(SchedulingResumeSelfCheck, Option<PendingRecoveryLadder>)> {
        self.prepare_instance_connection_with_cooldown(
            instance_alias,
            instance_id,
            links,
            admission,
            actingcommand_contract::RecoveryTriggerStage::ConnectionPreparation,
            PreparationLease::Held {
                token,
                connection_id,
            },
            EmulatorStatusRead::Skip,
        )
        .map(|prepared| (prepared.selfcheck, prepared.ladder))
    }

    /// The stuck-recovery ladder's preparation (stage `recovery_preparation`): the self-check,
    /// the `instance_preparation_finished` event, and whether a failure is one the existing
    /// preparation rule calls recoverable (Workflow #369-1, review M3). Workflow #369 H-3: it
    /// runs on the ladder's key, takes no dedicated lease and releases nothing. #411 A50 (the A38
    /// follow-up): it first reads the emulator status as A48's self-check does
    /// (`selfcheck_emulator`), so a running emulator the Runtime holds stopped or on another port
    /// is recognized again (the reported port bound, the ADB baseline awaited) before the opens,
    /// instead of an open failing on the stale binding; that covers a fence's own recheck too.
    pub(super) fn prepare_recovery_connection(
        &self,
        resolved: &RegisteredInstance,
        links: EventLinksDraft,
        admission: &MutexGuard<'_, ()>,
        key: &super::host_claims::HostKey,
    ) -> RuntimeHostResult<(SchedulingResumeSelfCheck, Option<TerminalEvent>, bool)> {
        self.prepare_instance_connection_with_cooldown(
            &resolved.instance_alias,
            resolved.instance_id(),
            links,
            admission,
            actingcommand_contract::RecoveryTriggerStage::RecoveryPreparation,
            PreparationLease::Held {
                token: &key.token,
                connection_id: key.connection_id,
            },
            EmulatorStatusRead::First,
        )
        .map(|prepared| (prepared.selfcheck, prepared.event, prepared.recoverable))
    }

    /// EMU-BIND (0.12.28): the emulator watch's recognition of a running emulator whose endpoint
    /// the Runtime holds pending or bound to another port, on its autostart claim's `key`: the
    /// preparation phase of a self-check (stage `connection_preparation`), so it reads the
    /// emulator status first (#411 A48, `selfcheck_emulator`: the reported port bound with
    /// `runtime.instance_bound`, the ADB baseline, `device.connected`; a failure there opens
    /// nothing and is recorded on this instance only), under the instance's admission guard for
    /// the preparation's device steps only. It takes no dedicated lease and releases nothing;
    /// returns the ladder a failed preparation admitted, which the caller's release hands the key
    /// to (H-1, P1). #480 PG-1 fix round 2 (the review's sequence 2, to its end): once it has the
    /// guard, before the preparation reads the status and binds, it asks `automatic_start_hold`
    /// once more; held (a pause set while the claim renewed or waited for the guard), it reads,
    /// binds and prepares nothing and records nothing (`RecognitionPreparation::Held`).
    pub(super) fn prepare_recognized_instance(
        &self,
        instance_alias: &str,
        instance_id: InstanceId,
        links: EventLinksDraft,
        key: &super::host_claims::HostKey,
    ) -> Result<RecognitionPreparation, RequestFailure> {
        let instance_guard = self.instance_guard(instance_id)?;
        let admission = self.lock_admission(&instance_guard, instance_id)?;
        if let Some(hold) =
            self.automatic_start_hold(instance_alias, HostLocation::RunEmulatorAutostart)?
        {
            return Ok(RecognitionPreparation::Held(hold));
        }
        let prepared = self.prepare_instance_connection_with_cooldown(
            instance_alias,
            instance_id,
            links,
            &admission,
            actingcommand_contract::RecoveryTriggerStage::ConnectionPreparation,
            PreparationLease::Held {
                token: &key.token,
                connection_id: key.connection_id,
            },
            EmulatorStatusRead::First,
        );
        drop(admission);
        let prepared = prepared.map_err(RequestFailure::poison_without_terminal)?;
        Ok(RecognitionPreparation::Ran(prepared.ladder.map(Box::new)))
    }

    /// `prepare_instance_connection`, also returning the monotonic deadline of the takeover
    /// cooldown when it refused the preparation lease (`lease_cooldown`, Workflow #191 h2), and
    /// whether a failed open is recoverable: an ordinary acquisition failure (#411 A7: a failed
    /// capture-backend open whatever its diagnostic category) whose resources were disposed and
    /// whose cleanup succeeded (the rule a preparation trigger follows). #411 A48: with
    /// `EmulatorStatusRead::First` (a self-check) the emulator status is read before the opens;
    /// a check that ends there opens nothing and reports its code (`selfcheck_emulator`).
    #[allow(clippy::too_many_arguments)]
    fn prepare_instance_connection_with_cooldown(
        &self,
        instance_alias: &str,
        instance_id: InstanceId,
        links: EventLinksDraft,
        admission: &MutexGuard<'_, ()>,
        stage: actingcommand_contract::RecoveryTriggerStage,
        lease: PreparationLease<'_>,
        status: EmulatorStatusRead,
    ) -> RuntimeHostResult<PreparationAttempt> {
        self.prepare_connection_attempt(
            instance_alias,
            instance_id,
            links,
            admission,
            stage,
            lease,
            status,
            PreparationPurpose::Connection,
        )
    }

    /// `prepare_instance_connection_with_cooldown` for `purpose`. #402 HS-2 (design HS-M §3.2
    /// step 3.4): a backend switch's preparation of its candidate backends
    /// (`PreparationPurpose::Switch`) is that same preparation, except that its failure admits no
    /// recovery ladder: the switch restores the previous backends and prepares them again itself.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn prepare_connection_attempt(
        &self,
        instance_alias: &str,
        instance_id: InstanceId,
        links: EventLinksDraft,
        admission: &MutexGuard<'_, ()>,
        stage: actingcommand_contract::RecoveryTriggerStage,
        lease: PreparationLease<'_>,
        status: EmulatorStatusRead,
        purpose: PreparationPurpose,
    ) -> RuntimeHostResult<PreparationAttempt> {
        lock(
            &self.recovery_ladders,
            HostLocation::BeginPreparationAttempt,
        )?
        .entry(instance_id)
        .or_default()
        .preparation = None;
        let frame_owner = self
            .events
            .issuer()
            .mint_request_id()
            .map_err(|_| runtime_identifier_error())?;
        // The frame memory owner of every capture path: the first frame of a capture opened here
        // is charged to it and dropped inside the open.
        let frame_store = actingcommand_artifact_store::FrameStore::new(
            frame_retention::spill_root(self.artifacts.root(), frame_owner.transport())
                .map_err(RuntimeHostError::artifact)?,
            self.capture_frame_store_config(),
        )
        .map_err(RuntimeHostError::artifact)?;
        let held = matches!(lease, PreparationLease::Held { .. });
        let granted = match lease {
            PreparationLease::Held {
                token,
                connection_id,
            } => Ok((token.clone(), connection_id)),
            // Workflow #437 G1: a member whose device is held for another member's unconfirmed
            // close opens nothing: its preparation's own lease is refused, naming that member.
            // Fix rounds 1 and 2: the member is owed that preparation, which the lease sweep
            // queues once the hold has ended (`queue_members_after_device_hold`); a member whose
            // claim is queued already keeps it.
            PreparationLease::Dedicated(kind) => match self.device_held_error(instance_id)? {
                Some(held) => {
                    lock(
                        &self.start_preparations_held,
                        HostLocation::PrepareInstanceConnection,
                    )?
                    .entry(instance_id)
                    .or_insert_with(|| HeldStartPreparation {
                        instance_alias: instance_alias.to_owned(),
                        queued: false,
                    });
                    Err(held)
                }
                None => self
                    .grant_dedicated_instance_lease(
                        instance_id,
                        CONNECTION_PREPARATION_CONNECTION_VALUE,
                        kind,
                    )
                    .map(|(token, connection_id, _)| (token, connection_id)),
            },
        };
        let (token, connection_id) = match granted {
            // Workflow #437 G1 fix round 1: a preparation of the instance runs (fix round 2: its
            // queued one on its claim's key too), so it owes none the device's hold refused
            // before.
            Ok(granted) => {
                lock(
                    &self.start_preparations_held,
                    HostLocation::PrepareInstanceConnection,
                )?
                .remove(&instance_id);
                granted
            }
            Err(error) if error.is_fatal() => return Err(error),
            Err(error) => {
                let failure_code = error.code();
                let cooldown_until = match error.projection().retry_after_ms {
                    Some(retry_after_ms)
                        if failure_code == RuntimeErrorCode::LeaseCooldown.as_str() =>
                    {
                        Some(
                            self.monotonic_ms()?
                                .checked_add(retry_after_ms)
                                .ok_or_else(|| {
                                    RuntimeHostError::fatal(
                                        HostCode::ConnectionPreparationCooldownOverflow,
                                        HostLocation::PrepareInstanceConnection,
                                        RuntimeErrorCode::RuntimeHalted,
                                    )
                                })?,
                        )
                    }
                    _ => None,
                };
                let failure = error.outcome();
                self.record_connection_preparation_failure(instance_id, links, error)?;
                return Ok(PreparationAttempt {
                    selfcheck: scheduling_resume_selfcheck(&[], Some(failure_code)),
                    cooldown_until,
                    event: None,
                    recoverable: false,
                    ladder: None,
                    failure: Some(failure),
                    generation: None,
                });
            }
        };
        // Workflow #191 h3: the preparation phase starts from a closed session. A session a
        // lease-free path retained (a read-only observe keeps its capture open, sc2) or an
        // earlier lease kept (Workflow #191 H) is closed first through the fenced close path, so
        // the opens below are complete and the self-check covers every required entry: a
        // reconnect is a disconnect and a connect. A failed close skips the opens and ends the
        // phase as a failed final close does.
        let retained_close = if self
            .execution()?
            .has_session(instance_id)
            .map_err(|error| {
                RuntimeHostError::execution(HostLocation::InspectRetainedSession, &error)
            })? {
            self.close_instance_resources_result(&token, connection_id, links.clone())
                .map_err(|failure| *failure.error)?
        } else {
            Ok(())
        };
        // #411 A48: a self-check first reads the emulator status; one that ends there opens
        // nothing, and its code is the failing step's.
        let mut status_failure = None;
        let opened = match &retained_close {
            Ok(()) => {
                self.invalidate_backend_selfcheck_facts(instance_id)?;
                match self.read_emulator_status_first(status, instance_id, &links)? {
                    Some(code) => {
                        status_failure = Some(code.as_str());
                        None
                    }
                    None => Some(self.execution()?.open_instance_backends_until(
                        instance_alias,
                        self.mark_resources_in_use()?,
                        frame_store.memory_budget(),
                        // Workflow #434 LAB-6S4a: only a held start's release bounded the opens.
                        None,
                    )),
                }
            }
            Err(_) => None,
        };
        let open_failed = matches!(opened, Some(Err(_)));
        // Device acquisition errors can be fatal to that backend without being owner/ledger
        // failures. Only ordinary acquisition failures with confirmed temporary-resource
        // disposal qualify; invalid parameter/configuration observations never qualify. Which
        // codes then start a ladder is the `recovery_ladder` configuration's (Workflow #387 D1c,
        // M387 §3.5: the default trigger item `preparation_backend`). #411 A7: a failed
        // capture-backend open is ordinary whatever its diagnostic category (a Nemu IPC open
        // that gets no positive connect handle reports none); its disposal must still be
        // confirmed, which a worker that timed out never is.
        let recoverable = opened.as_ref().is_some_and(|result| result.as_ref().is_err_and(|error| {
            error.resource_quiescence() == Some(ResourceQuiescence::Confirmed)
                && (error.code() == DeviceSelfCheckFailure::CaptureBackendOpenFailed.code()
                    || error.diagnostic_detail().is_some_and(|detail| matches!(detail.category(),
                        "handshake" | "child_exit" | "command_write" | "command_flush" | "protocol" | "response" | "native")))
                && !error.failure_context().backend_open_observations().iter().any(|observation|
                    observation.report.attempts.iter().any(|attempt| attempt.input_parameters.as_ref()
                        .is_some_and(|check| check.status == actingcommand_contract::BackendObservationStatus::Failed)))
        }));
        // #402 HS-2: the failing open's own outcome, which a refused backend switch names as its
        // cause (the self-check keeps only its code).
        let mut open_failure = None;
        let (observations, mut failure_code) = match opened {
            None => (Vec::new(), status_failure),
            Some(Ok(observations)) => {
                self.append_backend_open_observations(
                    &observations,
                    links.clone(),
                    EventSource::Device,
                    OriginModule::DeviceProxy,
                )?;
                (observations, None)
            }
            Some(Err(error)) => {
                self.append_backend_open_failure_observations(
                    &error,
                    links.clone(),
                    EventSource::Device,
                    OriginModule::DeviceProxy,
                )?;
                let failure =
                    RuntimeHostError::execution(HostLocation::OpenInstanceBackends, &error);
                // The open's failure record keeps its resource dispositions. #402 HS-3 (register
                // §1.271): a paired open the registry refused for want of an application index
                // acquired nothing, so it has none; its record is still written, since only it
                // names the refusal (`nemu_app_index_missing` with `nemu_app_index_problem`).
                if error.resource_quiescence().is_some()
                    || error.failure_context().nemu_app_index_problem().is_some()
                {
                    self.append_lifecycle_failure(
                        RuntimeLifecycleFailureStage::ConnectionPreparation,
                        RuntimeLifecycleFailure::Host(&failure),
                        links.clone(),
                        None,
                    )?;
                }
                open_failure = Some(failure.outcome());
                (
                    error.failure_context().backend_open_observations().to_vec(),
                    Some(error.code().as_str()),
                )
            }
        };
        // Workflow #191 H: a session opened here stays open for the instance's next leases (the
        // preparation lease's release keeps it, `end_lease_device_use`). A failed open, or an
        // instance outside the multi-Nemu gate, closes the session before the release as before;
        // the self-check facts stay. The lease is released only once any close is confirmed, as
        // for every dedicated close lease; its queue is empty (checked at the grant, and the
        // admission guard is held).
        let closed = match retained_close {
            Ok(()) if !open_failed && self.keeps_device_session(instance_id)? => Ok(()),
            Ok(()) => self
                .close_instance_resources_result(&token, connection_id, links.clone())
                .map_err(|failure| *failure.error)?,
            Err(close_error) => Err(close_error),
        };
        let confirmed = closed
            .as_ref()
            .err()
            .is_none_or(|error| error.resource_quiescence() == Some(ResourceQuiescence::Confirmed));
        // Workflow #403: an unconfirmed close fenced the instance (the close path did): the
        // preparation ends as a failed self-check and its lease ends as after a confirmed close,
        // the fence keeping the instance out of use.
        let fenced = !confirmed && self.resource_fenced(instance_id)?;
        // A held key (the ladder's) stays with its holder.
        if (confirmed || fenced) && !held {
            self.cleanup_token_inner(
                &token,
                connection_id,
                LeaseReleaseReason::ConnectionPrepared,
                None,
                Some(admission),
            )?;
        }
        let cleanup_succeeded = closed.is_ok();
        if let Err(close_error) = closed {
            // The close path recorded the failure; an unconfirmed close that fenced no instance
            // retained the owner and marked the Runtime fatal, as on every close path.
            if close_error.resource_quiescence() == Some(ResourceQuiescence::Unconfirmed) && !fenced
            {
                return Err(RuntimeHostError::execution(
                    HostLocation::ClosePreparedInstanceSession,
                    &close_error,
                )
                .into_fatal());
            }
            failure_code = failure_code.or(Some(close_error.code().as_str()));
        }
        // An open failure is already a `failed` self-check; any other failing step (a failed
        // open without reports, a failed close) still leaves the instance unavailable.
        if failure_code.is_some() {
            self.withhold_policy_instance_availability(instance_id)?;
        }
        let selfcheck = scheduling_resume_selfcheck(&observations, failure_code);
        // #402 HS-2: a backend switch's failed candidate starts no ladder (design HS-M §3.2 step
        // 3.4); the switch prepares the previous backends again, and that preparation may.
        let recoverable =
            recoverable && confirmed && cleanup_succeeded && purpose != PreparationPurpose::Switch;
        let generation = observations
            .iter()
            .map(|observation| observation.report.session_generation)
            .max();
        let (event, ladder) = self.record_preparation_finished(
            instance_id,
            links.clone(),
            stage,
            &observations,
            &selfcheck,
            recoverable,
        )?;
        // Workflow #403: a passing self-check of a fenced instance proves its close.
        if selfcheck.capture.ok && selfcheck.touch.ok && selfcheck.failure_code.is_none() {
            self.lift_resource_fence(instance_id, event, links)?;
        }
        // Workflow #369-1: a recovery readiness wait retries a failed preparation the
        // preparation rule calls recoverable; Workflow #387 D1c: that rule is the configured
        // trigger items of a connection preparation.
        let recoverable =
            recoverable && self.preparation_retryable(selfcheck.failure_code.as_deref())?;
        // Workflow #369 H-1: a dedicated lease was released above, so a daemon-start
        // preparation's ladder waits as a claim and the pump grants it on the free instance. On
        // a held key the ladder goes back to the key's holder, whose release hands it the key.
        let ladder = match ladder {
            Some(ladder) if !held => {
                self.enqueue_ladder_claim(ladder)?;
                None
            }
            ladder => ladder,
        };
        let failure = selfcheck
            .failure_code
            .as_deref()
            .map(|code| open_failure.unwrap_or_else(|| Outcome::relayed(code)));
        Ok(PreparationAttempt {
            selfcheck,
            cooldown_until: None,
            event: Some(event),
            recoverable,
            ladder,
            failure,
            generation,
        })
    }

    /// A preparation step other than an open failed without being fatal (the preparation lease
    /// was refused): one `runtime.failed` record with stage
    /// `runtime_lifecycle_connection_preparation` names the instance and the code, and the
    /// instance stays or becomes unavailable.
    fn record_connection_preparation_failure(
        &self,
        instance_id: InstanceId,
        links: EventLinksDraft,
        mut error: RuntimeHostError,
    ) -> RuntimeHostResult<()> {
        error.lifecycle.instance_id = Some(instance_id);
        self.append_lifecycle_failure(
            RuntimeLifecycleFailureStage::ConnectionPreparation,
            RuntimeLifecycleFailure::Host(&error),
            links,
            None,
        )?;
        self.withhold_policy_instance_availability(instance_id)
    }
}

/// #411 A52: whether `error` is the refusal of a dedicated resource-close lease by an entry that
/// waits in the instance's queue (`grant_dedicated_instance_lease`: `lease_transfer_not_safe`),
/// met before any close was attempted. Every other failure of a close keeps its fatality.
fn close_lease_refused_by_queue(error: &RuntimeHostError) -> bool {
    !error.is_fatal()
        && error.code() == SchedulerCode::LeaseTransferNotSafe.as_str()
        && error.operation() == HostLocation::PrepareResourceCloseLease.as_str()
}

/// Workflows #456 and #458: whether `error` refused the close of one instance's session before it
/// began, for a reason of that instance's own key: its dedicated close lease refused by the
/// device's takeover cooldown (`lease_cooldown`), by a queued claim the instance's routine gate
/// holds back or a lease that holds the device (a queue ahead or busy, `lease_busy`) or by a
/// lapsed lease the device still holds (`lease_expired`); or the close's write authority refused on the instance's own lease because
/// it lapsed (`lease_expired`) or the takeover cooldown runs (`lease_cooldown`). The refusal by a
/// waiting entry (`close_lease_refused_by_queue`) is #411 A52's, and every other failure (a record,
/// a lock, an identifier, an invariant of the scheduler) keeps its fatality.
fn close_refused_for_instance(error: &RuntimeHostError) -> bool {
    let code = error.code();
    let lapsed = code == RuntimeErrorCode::LeaseExpired.as_str();
    let cooling = code == RuntimeErrorCode::LeaseCooldown.as_str();
    let busy = code == RuntimeErrorCode::LeaseBusy.as_str();
    if error.operation() == HostLocation::PrepareResourceCloseLease.as_str() {
        return lapsed || cooling || busy;
    }
    error.operation() == HostLocation::BeginDestructiveResourceClose.as_str() && (lapsed || cooling)
}

/// Workflow #437 MAPP-1b: the failure of a keyless observation whose session close failed (fail
/// loud): the observation fails with the close's error, and a fatal one poisons it.
fn keyless_close_failed(error: RuntimeHostError) -> RequestFailure {
    if error.is_fatal() {
        RequestFailure::poison_without_terminal(error)
    } else {
        RequestFailure::request(error, RuntimeReceiptState::Failed, None)
    }
}

/// Workflow #437 MAPP-1c, tests: the slice's branch turned off, and the device's queue-order lock.
#[cfg(test)]
impl HostShared {
    /// The close of a member's keyless session as MAPP-1b made it (register §1.330, point 11),
    /// while a test turned MAPP-1c off: a dedicated close lease, which a waiting claim refuses
    /// (recorded once, the session left open) and the takeover cooldown refuses (the observation
    /// fails), ended with `BackendFailure`.
    fn close_keyless_session_before_mapp1c(
        &self,
        instance_id: InstanceId,
        links: EventLinksDraft,
        admission: &MutexGuard<'_, ()>,
    ) -> Result<(), RequestFailure> {
        match self.close_retained_instance_while_guarded(
            instance_id,
            links.clone(),
            false,
            LeaseReleaseReason::BackendFailure,
            admission,
        ) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(close_error)) => match self.settle_retained_close_failure(
                HostLocation::CloseExecutionSession,
                instance_id,
                &close_error,
            )? {
                None => Ok(()),
                Some(error) => Err(keyless_close_failed(error)),
            },
            Err(mut refused) if close_lease_refused_by_queue(&refused) => {
                refused.lifecycle.instance_id = Some(instance_id);
                self.append_lifecycle_failure(
                    RuntimeLifecycleFailureStage::SessionClose,
                    RuntimeLifecycleFailure::Host(&refused),
                    links,
                    None,
                )?;
                Ok(())
            }
            Err(error) => Err(keyless_close_failed(error)),
        }
    }
}

/// Workflow #437 MAPP-1c, tests.
#[cfg(test)]
impl RuntimeHost {
    /// Turns the slice's branch off (`true`) or on again: while off, a member's keyless session
    /// closes as MAPP-1b closed it and the queue-order lock is keyed by the instance, as before
    /// the slice.
    pub(crate) fn mapp1c_off_for_test(&self, off: bool) -> RuntimeHostResult<()> {
        let location = crate::codes::test_spellings::TestLocation::Mapp1cOffForTest;
        self.shared_ref(location)?
            .mapp1c_off_for_test
            .store(off, Ordering::Release);
        Ok(())
    }

    /// Workflows #456, #458, #461 and #475: turns the slices off (`true`) or on again: while off,
    /// a refused close at a lease's expiry, at a holder's own end, in a failed input's clean-up,
    /// after a capture failure or on #475's five paths under a lapsed key (its hand-over to its
    /// ladder, a pre-emption's hand-over, an application command's preparation, a backend switch,
    /// a member switch's close) ends the Runtime, as before.
    pub(crate) fn close_isolates_off_for_test(&self, off: bool) -> RuntimeHostResult<()> {
        let location = crate::codes::test_spellings::TestLocation::CloseIsolatesOffForTest;
        self.shared_ref(location)?
            .close_isolates_off_for_test
            .store(off, Ordering::Release);
        Ok(())
    }

    /// Workflows #456 and #458: while `fail` holds, the record of a refused close that would fence
    /// its instance cannot be appended (`ledger_failure`).
    pub(crate) fn fail_refused_close_record_for_test(&self, fail: bool) -> RuntimeHostResult<()> {
        let location = crate::codes::test_spellings::TestLocation::FailRefusedCloseRecordForTest;
        self.shared_ref(location)?
            .refused_close_record_failure_for_test
            .store(fail, Ordering::Release);
        Ok(())
    }

    /// The queue-order lock `instance_alias`'s claims are ordered under.
    pub(crate) fn queue_order_lock_for_test(
        &self,
        instance_alias: &str,
    ) -> RuntimeHostResult<Arc<Mutex<()>>> {
        let location = crate::codes::test_spellings::TestLocation::QueueOrderLockForTest;
        let shared = self.shared_ref(location)?;
        let instance_id = shared
            .resolve_instance(instance_alias)
            .map_err(|failure| *failure.error)?
            .instance_id();
        shared
            .queue_order_lock(instance_id)
            .map_err(|failure| *failure.error)
    }

    /// The key the queue-order lock map holds `instance_alias`'s lock by (the lock
    /// [`Self::queue_order_lock_for_test`] answers); `None` when the map holds it by no key.
    pub(crate) fn queue_order_lock_key_for_test(
        &self,
        instance_alias: &str,
    ) -> RuntimeHostResult<Option<InstanceId>> {
        let order = self.queue_order_lock_for_test(instance_alias)?;
        let location = crate::codes::test_spellings::TestLocation::QueueOrderLockForTest;
        let locks = lock(
            &self.shared_ref(location)?.queue_order_locks,
            HostLocation::ReadInstanceQueueOrder,
        )?;
        Ok(locks
            .iter()
            .find_map(|(key, held)| Arc::ptr_eq(held, &order).then_some(*key)))
    }
}

/// One connection preparation attempt (`prepare_instance_connection_with_cooldown`).
pub(super) struct PreparationAttempt {
    pub(super) selfcheck: SchedulingResumeSelfCheck,
    /// The monotonic deadline of the takeover cooldown that refused the dedicated lease.
    cooldown_until: Option<u64>,
    /// The attempt's `instance_preparation_finished`, when the preparation got that far.
    event: Option<TerminalEvent>,
    /// Whether a failed open is one the preparation rule calls recoverable.
    recoverable: bool,
    /// Workflow #369 H-1, P1: on a held key, the ladder the failed preparation admitted, to
    /// which the key's holder hands the key at its release (`release_to_preparation_ladder`).
    pub(super) ladder: Option<PendingRecoveryLadder>,
    /// #402 HS-2: the failing step's outcome when the self-check failed (the open's own, else
    /// the self-check's code), which a refused backend switch names as its cause.
    pub(super) failure: Option<Outcome>,
    /// #402 HS-2: the session generation of the opens the preparation recorded, if any.
    pub(super) generation: Option<u64>,
}

impl PreparationAttempt {
    /// #402 HS-2: whether no step of the preparation failed (no failing step's code, the rule
    /// the instance's availability follows); a failed one carries its `failure`.
    pub(super) fn passed(&self) -> bool {
        self.selfcheck.failure_code.is_none()
    }
}

/// #402 HS-2: why a connection preparation runs. A backend switch's preparation of its
/// candidate backends admits no recovery ladder when it fails (design HS-M §3.2 step 3.4).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PreparationPurpose {
    /// Every preparation before HS-2: the start's, a self-check's, a resume's, an emulator
    /// control's, a ladder's.
    Connection,
    /// The candidate backends of a backend switch.
    Switch,
}

/// Workflow #369: whose lease a connection preparation runs under: a dedicated lease of the
/// given kind that it takes and releases, or a key its caller holds (the ladder's), which it
/// leaves held.
pub(super) enum PreparationLease<'a> {
    Dedicated(ClaimKind),
    Held {
        token: &'a LeaseToken,
        connection_id: ConnectionId,
    },
}

/// Workflow #437 G1 fix rounds 1 and 2 (`contracts/device-group.md`, "A failure stays where it
/// belongs"): a member of a device group of several whose start preparation the device's hold for
/// another member's unconfirmed close refused, owed that preparation until one of it is granted
/// its key or lease (memory only).
pub(super) struct HeldStartPreparation {
    instance_alias: String,
    /// Its self-check claim waits in the device's queue or runs on the member's worker
    /// (`HostShared::queue_held_start_preparation`): the lease sweep queues no other.
    queued: bool,
}

/// Workflow #437 G1 fix round 2: the start's preparation of a member the device's hold refused,
/// queued on a self-check claim of the Runtime's own (no new claim kind) and made on that claim's
/// key by the member's worker (`HostShared::run_held_start_preparation`).
pub(super) struct PendingHeldStartPreparation {
    pub(super) instance_alias: String,
    pub(super) instance_id: InstanceId,
    /// The member and the claim's causation: what a worker failure is recorded under.
    pub(super) links: EventLinksDraft,
}
