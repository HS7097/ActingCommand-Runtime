// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #437 MAPP-2a (`contracts/device-group.md`, "The switch before a run"; register
//! §1.342): the parts of the switch that brings a member of a device group of several to the
//! front before its run, and their one entry, [`HostShared::switch_application`].
//!
//! - **The stop of another member under the device's key** ([`HostShared::stop_member_application`]):
//!   the key of the run that holds the device is validated as the device's, and the other
//!   member's application is stopped with that member's registration, package and links, every
//!   record under one causation id shared with the switching run. A key of another device and a
//!   member outside the holder's group are refused before any device step.
//! - **The startup run on a borrowed key** (`HostShared::run_startup_package_on_borrowed_key`,
//!   `startup_package.rs`): the member's startup package runs on the key its own run holds, which
//!   is never renewed and never ended, and goes back to that run valid, on success and on
//!   failure.
//! - **The stopped member's page forgotten** (`HostShared::forget_stopped_application_facts`,
//!   `runtime_facts.rs`): its `application.foreground` and `task.page` are dropped.
//!
//! No run goes through the switch yet: the run path calls it from MAPP-2b on, with the clock,
//! the failure's settlement and the member's cooldown. Until then the entry is reached only
//! through [`RuntimeHost::switch_application`], which nothing in the Runtime calls. Every part
//! begins from a member of a device group of several, and the assembly refuses every such group,
//! so an instance alone on its device never reaches this code.

use super::resource_fence::UnconfirmedResources;
use super::*;
use crate::codes::{ApplicationSwitchStep, HostCode, HostLocation};
use actingcommand_contract::IssuedCausationId;
use actingcommand_contract::codes::ContractCode;
use actingcommand_device::DeviceResourceQuiescence;

/// What the switch before a member's run did (Workflow #437 MAPP-2a).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApplicationSwitch {
    /// The package the device reported in front through the member's registration; `None` when
    /// it reported no resumed activity, and for an instance alone on its device, which the switch
    /// leaves alone.
    pub front: Option<String>,
    /// The members whose applications the switch stopped, by alias, in the order it stopped them.
    pub stopped: Vec<String>,
    /// Whether the member's startup package ran on the run's key and reached its home.
    pub started: bool,
}

/// `instance_mismatch` at the switch: a key that is not the device's of the member it names, a
/// member outside the key holder's device group, or an instance alone on its device, which never
/// takes part in a switch.
pub(super) fn member_mismatch() -> RuntimeHostError {
    RuntimeHostError::request(
        RuntimeErrorCode::InstanceMismatch,
        HostLocation::SwitchApplication,
        RuntimeErrorCode::InstanceMismatch,
    )
}

/// `application_effect_requires_assigned_application` at the switch: the registration names no
/// application (package) for the instance, so the switch cannot tell it in front.
fn no_assigned_application() -> RuntimeHostError {
    RuntimeHostError::request(
        SharedCode::ApplicationEffectRequiresAssignedApplication,
        HostLocation::SwitchApplication,
        RuntimeErrorCode::InvalidRequest,
    )
}

/// `application_switch_failed` of the switch before `alias`'s run, at `step`, concerning
/// `application_id`: `failure_code` is the step's own code and the step's failure is its cause.
fn switch_failed(
    alias: &str,
    step: ApplicationSwitchStep,
    application_id: &str,
    cause: &RuntimeHostError,
) -> RuntimeHostError {
    let failure_code = Code::relayed(cause.code())
        .unwrap_or_else(|| ContractCode::OutcomeRelayUnregistered.code());
    let mut error = RuntimeHostError::request(
        HostCode::ApplicationSwitchFailed,
        HostLocation::SwitchApplication,
        RuntimeErrorCode::BackendOperationFailed,
    )
    .with_outcome_values(|outcome| {
        outcome
            .alias(alias)
            .application_switch_step(step)
            .failure_code(failure_code)
            .application_id(application_id)
    })
    .with_caused_by(cause.outcome());
    error.lifecycle.instance_id = cause.lifecycle.instance_id;
    error
}

impl HostShared {
    /// The registration of `instance_id`, `None` when the registry holds none.
    fn switch_registration(
        &self,
        instance_id: InstanceId,
    ) -> RuntimeHostResult<Option<RegisteredInstance>> {
        Ok(lock(
            &self.registered_instances,
            HostLocation::ReadInstanceRegistry,
        )?
        .get(&instance_id)
        .cloned())
    }

    /// Workflow #437 MAPP-2a (`contracts/device-group.md`, "The switch before a run"): the one
    /// entry of the switch before a run of the member `token` names, on that run's key (`token`
    /// on `connection_id`); `request` is the member's run's request, whose links the startup
    /// package's scheduling record carries, and `after_ms` the time that run needs on the key
    /// after the switch (its own lease time: its response deadline and the heartbeat reserve).
    ///
    /// 1. An instance alone on its device is left alone: nothing is read, stopped or run.
    /// 2. The key must still validate as the device's, and the member's application must be
    ///    known; then one foreground read through the member's registration. Its ADB failure is
    ///    handled as the foreground gate's (the member's ADB facts dropped, an unconfirmed
    ///    cleanup fences the member). The member's own package in front: nothing more is done
    ///    and nothing is recorded (the contract's step 1).
    /// 3. Otherwise, before any stop, the key's remaining time must cover the startup
    ///    run and `after_ms` (`HostShared::borrowed_key_admits`). The member in front, when the
    ///    package in front is another member's, is stopped; with no member's package in front,
    ///    or none, every other member is stopped, in id order, one stop each. All share one
    ///    fresh causation id, and each stopped member's page is forgotten.
    /// 4. The member's startup package runs on the key under the same causation id, and the key
    ///    goes back to the member's run.
    ///
    /// A failed step is `application_switch_failed` with its step, the step's code and the
    /// package it concerned (`Ok(Err)`); the steps before it stay done. A member or a member to
    /// stop that the registry does not hold (`instance_mismatch`), or whose registration assigns
    /// no application (`application_effect_requires_assigned_application`), is refused as itself
    /// (`Ok(Err)`): the code's required `alias` or `application_id` would name nothing. `Err` is a
    /// Runtime-level failure of the step's own records.
    pub(super) fn switch_application(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        token: &LeaseToken,
        connection_id: ConnectionId,
        after_ms: u64,
    ) -> RuntimeHostResult<Result<ApplicationSwitch, RuntimeHostError>> {
        #[cfg(test)]
        self.switch_entries_for_test.fetch_add(1, Ordering::AcqRel);
        let member = token.instance_id();
        if !self.in_group_of_several(member) {
            return Ok(Ok(ApplicationSwitch::default()));
        }
        let Some(resolved) = self.switch_registration(member)? else {
            return Ok(Err(member_mismatch()));
        };
        let alias = resolved.instance_alias.as_str();
        let Some(own) = self.execution()?.assigned_application_id(alias) else {
            return Ok(Err(no_assigned_application()));
        };
        let now = self.monotonic_ms()?;
        if let Err(error) = lock(&self.scheduler, HostLocation::ValidateRuntimeLease)?
            .validate_write(token, connection_id, now)
        {
            let refusal = RuntimeHostError::scheduler(HostLocation::SwitchApplication, &error)
                .naming_lease(token.lease_id());
            return Ok(Err(switch_failed(
                alias,
                ApplicationSwitchStep::ForegroundRead,
                &own,
                &refusal,
            )));
        }
        let observation = match self.execution()?.observe_foreground_application(alias) {
            Ok(observation) => observation,
            Err(error) => {
                let cause = self.switch_read_failed(request, token, member, alias, &error)?;
                return Ok(Err(switch_failed(
                    alias,
                    ApplicationSwitchStep::ForegroundRead,
                    &own,
                    &cause,
                )));
            }
        };
        let front = observation.foreground;
        if front.as_deref() == Some(own.as_str()) {
            return Ok(Ok(ApplicationSwitch {
                front,
                stopped: Vec::new(),
                started: false,
            }));
        }
        let others = self
            .device_members(member)
            .into_iter()
            .filter(|other| *other != member)
            .collect::<Vec<_>>();
        let mut in_front = None;
        if let Some(front) = front.as_deref() {
            for other in &others {
                let Some(registration) = self.switch_registration(*other)? else {
                    continue;
                };
                if self
                    .execution()?
                    .assigned_application_id(&registration.instance_alias)
                    .as_deref()
                    == Some(front)
                {
                    in_front = Some(*other);
                    break;
                }
            }
        }
        let to_stop = in_front.map_or(others, |other| vec![other]);
        if let Err(refusal) = self.borrowed_key_admits(token, connection_id, after_ms)? {
            return Ok(Err(switch_failed(
                alias,
                ApplicationSwitchStep::StartupPackage,
                &own,
                &refusal,
            )));
        }
        let causation_id = self
            .events
            .issuer()
            .mint_causation_id()
            .map_err(|_| runtime_identifier_error())?;
        let mut stopped = Vec::new();
        for other in to_stop {
            match self.stop_member_application(token, connection_id, other, causation_id)? {
                Ok(stopped_alias) => stopped.push(stopped_alias),
                Err(failure) => return Ok(Err(failure)),
            }
        }
        let links = self
            .events
            .request_links(request, Some(member), Some(token.lease_id()), None);
        match self.run_startup_package_on_borrowed_key(
            token,
            connection_id,
            links,
            request.request_id(),
            causation_id,
            after_ms,
        )? {
            Ok(_) => Ok(Ok(ApplicationSwitch {
                front,
                stopped,
                started: true,
            })),
            Err(cause) => Ok(Err(switch_failed(
                alias,
                ApplicationSwitchStep::StartupPackage,
                &own,
                &cause,
            ))),
        }
    }

    /// The switch's foreground read through `member`'s registration failed: as the foreground
    /// gate's ADB failure, the member's `device.connected`, `application.foreground` and
    /// `task.page` are dropped with `adb_unreachable`, and an adb child or pipe left unconfirmed
    /// fences the member. The cause is `instance_resources_fenced` then, else
    /// `application_foreground_unknown`.
    fn switch_read_failed(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        token: &LeaseToken,
        member: InstanceId,
        alias: &str,
        error: &actingcommand_device::DeviceError,
    ) -> RuntimeHostResult<RuntimeHostError> {
        self.invalidate_adb_baseline(member)?;
        let detail = format!("instance_alias={alias}; adb_failed={error}");
        if error.resource_quiescence() == Some(DeviceResourceQuiescence::Unconfirmed) {
            let mut unconfirmed = RuntimeHostError::fatal(
                HostCode::ApplicationForegroundUnknown,
                HostLocation::SwitchApplication,
                RuntimeErrorCode::RuntimeHalted,
            )
            .with_native_detail(detail);
            unconfirmed.lifecycle.instance_id = Some(member);
            unconfirmed.lifecycle.resource_quiescence = Some(ResourceQuiescence::Unconfirmed);
            let links =
                self.events
                    .request_links(request, Some(member), Some(token.lease_id()), None);
            return self.settle_unconfirmed_adb(unconfirmed, member, links);
        }
        let mut unknown = RuntimeHostError::request(
            HostCode::ApplicationForegroundUnknown,
            HostLocation::SwitchApplication,
            RuntimeErrorCode::InvalidRequest,
        )
        .with_native_detail(detail);
        unknown.lifecycle.instance_id = Some(member);
        Ok(unknown)
    }

    /// Workflow #437 MAPP-2a (`contracts/device-group.md`, "The switch before a run", step 2): the
    /// application of `member`, another member of the device group whose key `token` is (the
    /// key of the run that holds the device), force-stopped under that key; the stopped
    /// member's alias.
    ///
    /// Before any device step: the key's holder must be a member of a device group of several
    /// and `member` another member of it, and the key must validate as the device's
    /// (`validate_write`); otherwise the stop is refused, `instance_mismatch` or the scheduler's
    /// refusal. Then a host-minted `ApplicationLifecycle { stop }` request for `member`, under
    /// `causation_id`, writes `command.received` and `command.validated` and runs the existing
    /// application command on the member (`control_member_application`): `scheduler.admitted`,
    /// `application.intent` and `application.completed` (or `application.failed`), all under
    /// the member's links with the key's lease. Once the force-stop succeeded, the stopped
    /// member's `application.foreground` and `task.page` are dropped; then the session the stop
    /// used is closed under the key, whatever the stop's result. A failure of the stop is
    /// written as the rung's stop writes it (the command's records, then the lifecycle failure);
    /// it, or a failed close, is returned as `application_switch_failed` at step `stop`, with the
    /// stopped member's package. The key stays with its run whatever the result.
    pub(super) fn stop_member_application(
        &self,
        token: &LeaseToken,
        connection_id: ConnectionId,
        member: InstanceId,
        causation_id: IssuedCausationId,
    ) -> RuntimeHostResult<Result<String, RuntimeHostError>> {
        let holder = token.instance_id();
        let Some(holder_registration) = self.switch_registration(holder)? else {
            return Ok(Err(member_mismatch()));
        };
        let Some(target) = self.switch_registration(member)? else {
            return Ok(Err(member_mismatch()));
        };
        let Some(application_id) = self
            .execution()?
            .assigned_application_id(&target.instance_alias)
        else {
            return Ok(Err(no_assigned_application()));
        };
        let failed = |cause: &RuntimeHostError| {
            switch_failed(
                &holder_registration.instance_alias,
                ApplicationSwitchStep::Stop,
                &application_id,
                cause,
            )
        };
        if !self.in_group_of_several(holder)
            || member == holder
            || !self.device_members(holder).contains(&member)
        {
            return Ok(Err(failed(&member_mismatch())));
        }
        let now = self.monotonic_ms()?;
        if let Err(error) = lock(&self.scheduler, HostLocation::ValidateRuntimeLease)?
            .validate_write(token, connection_id, now)
        {
            let refusal = RuntimeHostError::scheduler(HostLocation::SwitchApplication, &error)
                .naming_lease(token.lease_id());
            return Ok(Err(failed(&refusal)));
        }
        let action = ApplicationLifecycleAction::Stop;
        let issuer = self.events.issuer();
        let (actor, source) = scheduled_request_transport_origin(target.provenance());
        let invalid = || {
            RuntimeHostError::fatal(
                HostCode::HostKeyRequestInvalid,
                HostLocation::SwitchApplication,
                RuntimeErrorCode::RuntimeHalted,
            )
        };
        let message = RuntimeRequest::new(
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
            RuntimeOperation::ApplicationLifecycle {
                instance_alias: target.instance_alias.clone(),
                holder_id: token.holder_id(),
                action,
            },
        )
        .map_err(|_| invalid())?;
        let stop = message.validate().map_err(|_| invalid())?;
        let stop_links = self.events.request_links(&stop, Some(member), None, None);
        for payload in [
            CommandPayloadDraft::received(action.event_action(), AuditInput::new()),
            CommandPayloadDraft::validated(
                action.event_action(),
                EffectDisposition::NotPerformed,
                AuditInput::new(),
            ),
        ] {
            self.append_event_raw(
                EventSeverity::Info,
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
                stop_links.clone(),
                payload,
            )?;
        }
        let stop_failure =
            match self.control_member_application(&stop, token, &target, action, connection_id) {
                // F4: once the force-stop succeeded the member shows no page any more, whatever
                // the close of its session below then does.
                Ok(_) => {
                    self.forget_stopped_application_facts(member)?;
                    None
                }
                Err(mut failure) => {
                    // Workflow #403: an unconfirmed close fences the stopped member (or, when no
                    // single instance owns it, retains the owner and marks the Runtime fatal);
                    // the key stays with its run.
                    if let UnconfirmedResources::Fenced(fence) = self.settle_unconfirmed_resources(
                        &failure.error,
                        Some(member),
                        EventLinksDraft::default(),
                    )? {
                        *failure.error = fence;
                        failure.poison_runtime = false;
                    }
                    if failure.poison_runtime {
                        return Err((*failure.error).into_fatal());
                    }
                    if failure.error.is_fatal() {
                        return Err(*failure.error);
                    }
                    self.append_lifecycle_failure(
                        RuntimeLifecycleFailureStage::OperationCleanup,
                        RuntimeLifecycleFailure::Host(&failure.error),
                        stop_links.clone(),
                        None,
                    )?;
                    Some(*failure.error)
                }
            };
        // A member of a group of several keeps no session while another member holds the
        // device: the session the stop used is closed under the key, whatever the stop's result.
        // The close path records its own failure and settles an unconfirmed close.
        let closed = {
            let instance_guard = self
                .instance_guard(holder)
                .map_err(|failure| *failure.error)?;
            let _admission = lock(&instance_guard, HostLocation::LockInstanceAdmission)?;
            self.close_member_resources_result(token, member, connection_id, stop_links)
                .map_err(|failure| *failure.error)?
        };
        let close_failure = closed.err().map(|close_error| {
            RuntimeHostError::execution(HostLocation::CloseExecutionSession, &close_error)
        });
        #[cfg(test)]
        let close_failure = close_failure.or_else(|| self.take_member_close_failure_for_test());
        match (stop_failure, close_failure) {
            (Some(stop_failure), Some(close_failure)) => {
                return Ok(Err(failed(
                    &stop_failure.with_related_failure("cleanup", &close_failure),
                )));
            }
            (Some(failure), None) | (None, Some(failure)) => return Ok(Err(failed(&failure))),
            (None, None) => {}
        }
        Ok(Ok(target.instance_alias))
    }
}

impl RuntimeHost {
    /// Workflow #437 MAPP-2a (`contracts/device-group.md`, "The switch before a run"): the one
    /// entry of the switch before a run of the member `token` names, on that run's key (`token`
    /// on `connection_id`); `request` is the run's request and `after_ms` the time the run needs
    /// on the key after the switch. Nothing in the Runtime calls it in this slice: MAPP-2b calls
    /// the switch from the run path. An instance alone on its device is left alone (an empty
    /// [`ApplicationSwitch`]). `Ok(Err)` is the switch's failure, `application_switch_failed` with
    /// its step; `Err` a Runtime-level failure.
    pub fn switch_application(
        &self,
        request: &RuntimeRequest,
        token: &LeaseToken,
        connection_id: ConnectionId,
        after_ms: u64,
    ) -> RuntimeHostResult<Result<ApplicationSwitch, RuntimeHostError>> {
        let validated = request.validate().map_err(|_| {
            RuntimeHostError::request(
                RuntimeErrorCode::InvalidRequest,
                HostLocation::SwitchApplication,
                RuntimeErrorCode::InvalidRequest,
            )
        })?;
        self.work_ref(HostLocation::SwitchApplication)?
            .switch_application(&validated, token, connection_id, after_ms)
    }
}

/// Workflow #437 MAPP-2a, tests: the failed close a test asked for.
#[cfg(test)]
impl HostShared {
    /// The failure of the close of the session a member's stop used, once, when a test asked for
    /// it ([`RuntimeHost::fail_next_member_close_for_test`]); `None` otherwise.
    fn take_member_close_failure_for_test(&self) -> Option<RuntimeHostError> {
        self.member_close_failure_for_test
            .swap(false, Ordering::AcqRel)
            .then(|| {
                RuntimeHostError::request(
                    RuntimeErrorCode::BackendOperationFailed,
                    HostLocation::CloseExecutionSession,
                    RuntimeErrorCode::BackendOperationFailed,
                )
            })
    }
}

/// Workflow #437 MAPP-2a, tests: the switch's parts one by one, and how often its entry was
/// reached.
#[cfg(test)]
impl RuntimeHost {
    /// The stop part ([`HostShared::stop_member_application`]): `member_alias`'s application
    /// stopped under `token`'s key, with a fresh causation id.
    pub(crate) fn stop_member_application_for_test(
        &self,
        token: &LeaseToken,
        connection_id: ConnectionId,
        member_alias: &str,
    ) -> RuntimeHostResult<Result<String, RuntimeHostError>> {
        let location = crate::codes::test_spellings::TestLocation::StopMemberApplicationForTest;
        let shared = self.shared_ref(location)?;
        let member = shared
            .resolve_instance(member_alias)
            .map_err(|failure| *failure.error)?
            .instance_id();
        let causation_id = shared
            .events
            .issuer()
            .mint_causation_id()
            .map_err(|_| runtime_identifier_error())?;
        shared.stop_member_application(token, connection_id, member, causation_id)
    }

    /// The startup part (`HostShared::run_startup_package_on_borrowed_key`): the startup package
    /// of `token`'s member on that key, under the links of `request` (the member's run's request)
    /// and a fresh causation id.
    pub(crate) fn run_borrowed_startup_for_test(
        &self,
        request: &RuntimeRequest,
        token: &LeaseToken,
        connection_id: ConnectionId,
        after_ms: u64,
    ) -> RuntimeHostResult<Result<(), RuntimeHostError>> {
        let location = crate::codes::test_spellings::TestLocation::RunBorrowedStartupForTest;
        let shared = self.shared_ref(location)?;
        let validated = request.validate().map_err(|_| {
            RuntimeHostError::request(
                RuntimeErrorCode::InvalidRequest,
                location,
                RuntimeErrorCode::InvalidRequest,
            )
        })?;
        let causation_id = shared
            .events
            .issuer()
            .mint_causation_id()
            .map_err(|_| runtime_identifier_error())?;
        let links = shared.events.request_links(
            &validated,
            Some(token.instance_id()),
            Some(token.lease_id()),
            None,
        );
        Ok(shared
            .run_startup_package_on_borrowed_key(
                token,
                connection_id,
                links,
                validated.request_id(),
                causation_id,
                after_ms,
            )?
            .map(|_| ()))
    }

    /// The next close of the session a member's stop used fails after the stop (once), as a
    /// paired session's close can; the fake device keeps nothing open there to fail.
    pub(crate) fn fail_next_member_close_for_test(&self) -> RuntimeHostResult<()> {
        let location = crate::codes::test_spellings::TestLocation::FailNextMemberCloseForTest;
        self.shared_ref(location)?
            .member_close_failure_for_test
            .store(true, Ordering::Release);
        Ok(())
    }

    /// How many times the switch's entry was reached since the host started.
    pub(crate) fn switch_entries_for_test(&self) -> RuntimeHostResult<u64> {
        let location = crate::codes::test_spellings::TestLocation::SwitchEntriesForTest;
        Ok(self
            .shared_ref(location)?
            .switch_entries_for_test
            .load(Ordering::Acquire))
    }

    /// The time `token` has left on the Runtime clock while the scheduler still takes it, whole
    /// (expiry included), as the device's key on `connection_id`; `None` once it does not.
    pub(crate) fn key_remaining_ms_for_test(
        &self,
        token: &LeaseToken,
        connection_id: ConnectionId,
    ) -> RuntimeHostResult<Option<u64>> {
        let location = crate::codes::test_spellings::TestLocation::KeyRemainingForTest;
        let shared = self.shared_ref(location)?;
        let now = shared.monotonic_ms()?;
        Ok(lock(&shared.scheduler, location)?
            .validate_write(token, connection_id, now)
            .ok()
            .map(|()| token.expires_at_monotonic_ms().saturating_sub(now)))
    }
}
