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
//! Workflow #437 MAPP-2b (register §1.353): the callers. A policy run (`run_scheduled_contained_task`)
//! and a direct run (`run_contained_task`) of a member switch after their own checks, timed so
//! that their own clock leaves the switch out ([`HostShared::switch_before_run`]); an
//! `ApplicationLifecycle` `launch` or `restart` of a member and the ladder's application restart
//! rung stop the member in front first ([`HostShared::stop_others_before_start`]). A failed
//! switch holds the member's candidates for the switch cooldown
//! (`scheduling.device_group.switch_failure_cooldown_ms`, in memory). Every part and every caller
//! begins from a member of a device group of several ([`HostShared::switch_applies`]), and the
//! assembly refuses every such group, so an instance alone on its device never reaches this code.
//!
//! Workflow #437 MAPP-2d (register §1.360): the lease across a switch. A member's dispatch is
//! granted the lease of an instance alone (its run's response deadline and the heartbeat
//! reserve), and the switch runs inside it: its startup run takes what is left of the key
//! ([`RunNeed::Renews`]). Once the startup run has handed the key back, the run renews its lease
//! to its own response deadline and the reserve, counted from then
//! (`HostShared::renew_after_switch`), before its request lifecycle. The renewal keeps the lease
//! and replaces its token, so the run's later steps resolve the token the scheduler holds
//! ([`HostShared::run_token`]).

use super::resource_fence::UnconfirmedResources;
use super::*;
use crate::codes::{ApplicationSwitchStep, HostCode, HostLocation};
use actingcommand_contract::IssuedCausationId;
use actingcommand_contract::codes::ContractCode;
use actingcommand_device::DeviceResourceQuiescence;

/// What a member's run still needs on the device's key after the startup run of the switch before
/// it: the startup run's deadline on the borrowed key leaves it (`HostShared::borrowed_key_admits`
/// says what the key's remaining time must cover before the startup run).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RunNeed {
    /// This much time on the key from each moment the switch reads the key: the key's remaining
    /// time must cover the startup run's lease time and it (Workflow #437 MAPP-2a's form, which
    /// the public entry [`RuntimeHost::switch_application`] takes).
    For(u64),
    /// Workflow #437 MAPP-2d: nothing, because the run renews its lease once the startup run has
    /// handed the key back (`HostShared::renew_after_switch`). The startup run takes what is left
    /// of the key: its deadline is the earlier of its response deadline and the key's expiry less
    /// the heartbeat reserve, and the key is too little for it to start at all when no more than
    /// the reserve is left (that deadline would not lie after now).
    Renews,
}

impl RunNeed {
    /// The time this need keeps on the key after the startup run.
    pub(super) const fn after_ms(self) -> u64 {
        match self {
            Self::For(after_ms) => after_ms,
            Self::Renews => 0,
        }
    }
}

/// Workflow #437 MAPP-2b: a Runtime-level failure of the switch before a run (the `Err` of its
/// entry), as the run's failure: a fatal one poisons, as on every run path; any other fails the
/// run before its request lifecycle.
pub(super) fn switch_runtime_failure(error: RuntimeHostError) -> RequestFailure {
    if error.is_fatal() {
        RequestFailure::poison_without_terminal(error)
    } else {
        RequestFailure::request(error, RuntimeReceiptState::Failed, None)
    }
}

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

    /// Workflow #437 MAPP-2b (`contracts/device-group.md`, "The switch before a run"): whether the
    /// switch and everything MAPP-2b builds around it (the lease sized for it, the clock that
    /// leaves it out, the cooldown) applies to `instance_id`: a member of a device group of
    /// several. An instance alone on its device never meets any of it.
    pub(super) fn switch_applies(&self, instance_id: InstanceId) -> bool {
        #[cfg(test)]
        if self.switch_off_for_test.load(Ordering::Acquire) {
            return false;
        }
        self.in_group_of_several(instance_id)
    }

    /// Workflow #437 MAPP-2a (`contracts/device-group.md`, "The switch before a run"): the one
    /// entry of the switch before a run of the member `token` names, on that run's key (`token`
    /// on `connection_id`); `links` are the member's run's request links with the member and the
    /// key's lease (the startup package's scheduling record carries them), `request_id` that
    /// run's request, and `need` the time that run still needs on the key after the switch's
    /// startup run.
    ///
    /// 1. An instance alone on its device is left alone: nothing is read, stopped or run.
    /// 2. The key must still validate as the device's, and the member's application must be
    ///    known; then one foreground read through the member's registration. Its ADB failure is
    ///    handled as the foreground gate's (the member's ADB facts dropped, an unconfirmed
    ///    cleanup fences the member). The member's own package in front: nothing more is done
    ///    and nothing is recorded (the contract's step 1).
    /// 3. Otherwise, before any stop, the key's remaining time must cover the startup
    ///    run and `need` (`HostShared::borrowed_key_admits`). The member in front, when the
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
        links: EventLinksDraft,
        request_id: RequestId,
        token: &LeaseToken,
        connection_id: ConnectionId,
        need: RunNeed,
    ) -> RuntimeHostResult<Result<ApplicationSwitch, RuntimeHostError>> {
        self.switch_front(links, token, connection_id, Some((request_id, need)))
    }

    /// Workflow #437 MAPP-2b (`contracts/device-group.md`, "The switch before a run", "Where the
    /// switch runs"): before a start of the member's own application outside a run (an
    /// `ApplicationLifecycle` `launch` or `restart`, the ladder's application restart rung), the
    /// switch's steps 1 and 2 on the key `token` (the request's or the ladder's): the member in
    /// front stopped, or every other member with the front unknown, and nothing when the
    /// member's own package is in front. No startup run follows, since the caller's own action
    /// starts the member's application; so no time is asked of the key. `links` are the caller's
    /// links with the member and the key's lease. The result as
    /// [`Self::switch_application`]'s.
    pub(super) fn stop_others_before_start(
        &self,
        links: EventLinksDraft,
        token: &LeaseToken,
        connection_id: ConnectionId,
    ) -> RuntimeHostResult<Result<ApplicationSwitch, RuntimeHostError>> {
        self.switch_front(links, token, connection_id, None)
    }

    /// Workflow #437 MAPP-2b (`contracts/device-group.md`, "The switch before a run"): the switch
    /// before a member's run ([`Self::switch_application`]), timed on the Runtime clock: how long
    /// it took (the time the run's own clock leaves out; no sample of it is kept), and its result.
    /// A switch that failed with `application_switch_failed` starts the member's switch cooldown
    /// before it returns, so the run's failed settlement that follows finds it running.
    ///
    /// Workflow #437 MAPP-2d: the switch runs inside the run's own lease, its startup run taking
    /// what is left of the key ([`RunNeed::Renews`]); once a startup run has handed the key back,
    /// the run renews its lease to `lease_ttl_ms` (its response deadline and the heartbeat
    /// reserve) from then (`HostShared::renew_after_switch`, its records under `request`, the
    /// run's request), inside the time measured here, so the renewal adds no time to the run's
    /// own. A switch that ran no startup run (the member's own package in front) renews nothing.
    pub(super) fn switch_before_run(
        &self,
        links: EventLinksDraft,
        request: &ValidatedRuntimeRequest<'_>,
        token: &LeaseToken,
        connection_id: ConnectionId,
        lease_ttl_ms: u64,
    ) -> RuntimeHostResult<(u64, Result<ApplicationSwitch, RuntimeHostError>)> {
        let started_ms = self.monotonic_ms()?;
        let mut switched = self.switch_application(
            links,
            request.request_id(),
            token,
            connection_id,
            RunNeed::Renews,
        )?;
        if matches!(&switched, Ok(switch) if switch.started)
            && let Err(failure) =
                self.renew_after_switch(request, token, connection_id, lease_ttl_ms)?
        {
            switched = Err(failure);
        }
        let took_ms = self.monotonic_ms()?.saturating_sub(started_ms);
        if let Err(failure) = &switched
            && failure.code() == HostCode::ApplicationSwitchFailed.as_str()
        {
            self.start_switch_cooldown(token.instance_id())?;
        }
        Ok((took_ms, switched))
    }

    /// Workflow #437 MAPP-2b (`contracts/device-group.md`, "The member's switch cooldown"): the
    /// member's switch failed, so it does not compete for its device until now plus
    /// `scheduling.device_group.switch_failure_cooldown_ms` in effect, on the Runtime's monotonic
    /// clock, in memory only (a restart drops it). A later failure starts it again.
    fn start_switch_cooldown(&self, member: InstanceId) -> RuntimeHostResult<()> {
        let cooldown_ms = self.switch_failure_cooldown_ms()?;
        let until_monotonic_ms = self.monotonic_ms()?.saturating_add(cooldown_ms);
        lock(&self.switch_cooldowns, HostLocation::SwitchApplication)?
            .insert(member, until_monotonic_ms);
        Ok(())
    }

    /// Workflow #437 MAPP-2b (`contracts/device-group.md`, "The member's switch cooldown"): every
    /// registered member whose switch cooldown still runs as a policy cycle starts at
    /// `now_unix_ms`, by alias, with the Unix instant it ends; a cooldown that has ended is
    /// forgotten here. Empty while no switch has failed, so for every instance alone on its
    /// device.
    pub(super) fn policy_switch_cooldowns(
        &self,
        now_unix_ms: u64,
    ) -> RuntimeHostResult<BTreeMap<String, u64>> {
        let running = {
            let mut cooldowns = lock(&self.switch_cooldowns, HostLocation::SwitchApplication)?;
            if cooldowns.is_empty() {
                return Ok(BTreeMap::new());
            }
            let now_monotonic_ms = self.monotonic_ms()?;
            cooldowns.retain(|_, until| *until > now_monotonic_ms);
            cooldowns
                .iter()
                .map(|(member, until)| (*member, until.saturating_sub(now_monotonic_ms)))
                .collect::<Vec<_>>()
        };
        let registered = lock(
            &self.registered_instances,
            HostLocation::ReadPolicyInstanceRegistry,
        )?;
        let mut held = BTreeMap::new();
        for (member, remaining_ms) in running {
            let Some(instance) = registered.get(&member) else {
                continue;
            };
            let until_unix_ms = now_unix_ms.checked_add(remaining_ms).ok_or_else(|| {
                policy_admission_fatal(
                    HostCode::PolicyAdmissionClockOverflow,
                    HostLocation::ReadPolicyInstanceOccupancy,
                )
            })?;
            held.insert(instance.instance_alias.clone(), until_unix_ms);
        }
        Ok(held)
    }

    /// Workflow #437 MAPP-2d (`contracts/device-group.md`, "The switch before a run", step 4 and
    /// "The key's time"): after the switch's startup run has handed the key back, the run renews
    /// its lease to `lease_ttl_ms` (its response deadline and the heartbeat reserve) counted from
    /// now, with the host's renewal of a Runtime-held lease (`HostShared::renew_held_lease`,
    /// `scheduler.admitted`, `lease.transition_intent` and `lease.renewed` under `request`, the
    /// run's request). The renewal keeps the lease (its id, holder, instance and owner epoch) and
    /// replaces its token; the run's later steps take that token through [`Self::run_token`]. The
    /// queue is not read: a claim queued on the device stays queued behind the run. A renewal the
    /// scheduler refuses (the lease lost, lapsed or taken over) is `application_switch_failed` at
    /// step `lease_renewal`, caused by the refusal, with the member's own package; a Runtime-level
    /// failure of its records is the error.
    fn renew_after_switch(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        token: &LeaseToken,
        connection_id: ConnectionId,
        lease_ttl_ms: u64,
    ) -> RuntimeHostResult<Result<(), RuntimeHostError>> {
        let Some(resolved) = self.switch_registration(token.instance_id())? else {
            return Ok(Err(member_mismatch()));
        };
        let alias = resolved.instance_alias.as_str();
        let Some(own) = self.execution()?.assigned_application_id(alias) else {
            return Ok(Err(no_assigned_application()));
        };
        #[cfg(test)]
        let refused_for_test = self.renewal_token_for_test(token);
        #[cfg(test)]
        let token = refused_for_test.as_ref().unwrap_or(token);
        match self.renew_held_lease(request, token, connection_id, lease_ttl_ms) {
            Ok(_) => Ok(Ok(())),
            Err(failure) if failure.poison_runtime => Err((*failure.error).into_fatal()),
            Err(failure) if failure.error.is_fatal() => Err(*failure.error),
            Err(failure) => Ok(Err(switch_failed(
                alias,
                ApplicationSwitchStep::LeaseRenewal,
                &own,
                &failure.error,
            ))),
        }
    }

    /// Workflow #437 MAPP-2d (`contracts/device-group.md`, "The key's time"): the token of the
    /// lease `granted` names, as the scheduler holds it now. A member's run renews its lease once
    /// the switch's startup run has handed the key back, and the renewal replaces the token (the
    /// lease id, holder, instance and owner epoch stay), so every step of the run after the switch
    /// takes its token here: the device's lease with the same lease id and holder on
    /// `connection_id`, found as `release_host_key` finds a host key. When the device holds no
    /// such lease any more (it ended, lapsed or was handed on), the granted token itself, so what
    /// then validates or releases it meets that end as before. An instance alone on its device
    /// never renews: its granted token is returned and nothing is read.
    pub(super) fn run_token(
        &self,
        granted: &LeaseToken,
        connection_id: ConnectionId,
    ) -> RuntimeHostResult<LeaseToken> {
        if !self.switch_applies(granted.instance_id()) {
            return Ok(granted.clone());
        }
        Ok(lock(&self.scheduler, HostLocation::ReadHostKey)?
            .active_lease(granted.instance_id())
            .filter(|lease| {
                lease.token().lease_id() == granted.lease_id()
                    && lease.token().holder_id() == granted.holder_id()
                    && lease.connection_id() == connection_id
            })
            .map_or_else(|| granted.clone(), |lease| lease.token().clone()))
    }

    /// The switch's steps on the key `token` (`connection_id`) of the member it names: steps 1
    /// and 2 always, and with `startup` (the run's request and its need) the time check before
    /// any stop and step 3, the startup run on the borrowed key ([`Self::switch_application`],
    /// [`Self::stop_others_before_start`]).
    fn switch_front(
        &self,
        links: EventLinksDraft,
        token: &LeaseToken,
        connection_id: ConnectionId,
        startup: Option<(RequestId, RunNeed)>,
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
                let cause = self.switch_read_failed(links, member, alias, &error)?;
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
        if let Some(need) = startup.as_ref().map(|(_, need)| *need)
            && let Err(refusal) = self.borrowed_key_admits(token, connection_id, need)?
        {
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
        // Workflow #437 MAPP-2b: before a start outside a run the caller's own action starts the
        // member's application; only a run's switch runs its startup package.
        let Some((request_id, need)) = startup else {
            return Ok(Ok(ApplicationSwitch {
                front,
                stopped,
                started: false,
            }));
        };
        match self.run_startup_package_on_borrowed_key(
            token,
            connection_id,
            links,
            request_id,
            causation_id,
            need,
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
    /// fences the member, under `links` (the caller's, with the member and the key's lease). The
    /// cause is `instance_resources_fenced` then, else `application_foreground_unknown`.
    fn switch_read_failed(
        &self,
        links: EventLinksDraft,
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
    /// on the key after the switch. Workflow #437 MAPP-2b: the run paths call the switch inside
    /// the host (`HostShared::switch_before_run`); this entry stays for a caller that holds a
    /// member's key itself. An instance alone on its device is left alone (an empty
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
        let shared = self.work_ref(HostLocation::SwitchApplication)?;
        let links = shared.events.request_links(
            &validated,
            Some(token.instance_id()),
            Some(token.lease_id()),
            None,
        );
        shared.switch_application(
            links,
            validated.request_id(),
            token,
            connection_id,
            RunNeed::For(after_ms),
        )
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

    /// Workflow #437 MAPP-2d: once, when a test asked for it
    /// ([`RuntimeHost::refuse_next_switch_renewal_for_test`]), the token the next renewal after a
    /// switch is asked with: `token`'s lease with an expiry the scheduler does not hold, as a token
    /// left behind by a renewal or a takeover elsewhere, so the scheduler refuses it
    /// (`lease_mismatch`) and the key itself is untouched; `None` otherwise.
    fn renewal_token_for_test(&self, token: &LeaseToken) -> Option<LeaseToken> {
        if !self
            .switch_renewal_refusal_for_test
            .swap(false, Ordering::AcqRel)
        {
            return None;
        }
        LeaseToken::new(
            token.owner_epoch(),
            token.lease_id(),
            token.instance_id(),
            token.holder_id(),
            token.expires_at_monotonic_ms().saturating_add(1),
        )
        .ok()
    }

    /// Workflow #437 MAPP-2d: once, when a test asked for it
    /// ([`RuntimeHost::fail_next_run_after_switch_for_test`]), the failure of the next member's
    /// run right after its switch (its renewal included) and before anything else of the run:
    /// `backend_operation_failed`, as a failure of the run's next step would be; `None`
    /// otherwise.
    pub(super) fn take_run_failure_after_switch_for_test(&self) -> Option<RequestFailure> {
        self.run_failure_after_switch_for_test
            .swap(false, Ordering::AcqRel)
            .then(|| {
                RequestFailure::request(
                    RuntimeHostError::request(
                        RuntimeErrorCode::BackendOperationFailed,
                        HostLocation::SwitchApplication,
                        RuntimeErrorCode::BackendOperationFailed,
                    ),
                    RuntimeReceiptState::Failed,
                    None,
                )
            })
    }

    /// Workflow #437 MAPP-2d fix round 1: once, when a test asked for it
    /// ([`RuntimeHost::release_failed_policy_run_with_granted_token_for_test`]), `granted`, the
    /// token a policy run was granted, for the release of its failed settlement in place of the
    /// token [`Self::run_token`] resolves, as a build that released with the granted token would;
    /// `None` otherwise.
    pub(super) fn take_granted_token_release_for_test(
        &self,
        granted: &LeaseToken,
    ) -> Option<LeaseToken> {
        self.granted_token_release_for_test
            .swap(false, Ordering::AcqRel)
            .then(|| granted.clone())
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
                RunNeed::For(after_ms),
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

    /// Workflow #437 MAPP-2d: the next renewal after a member's switch is asked with a token the
    /// scheduler does not hold, so the scheduler refuses it (`lease_mismatch`); the key stays the
    /// run's, untouched (`HostShared::renewal_token_for_test`).
    pub(crate) fn refuse_next_switch_renewal_for_test(&self) -> RuntimeHostResult<()> {
        let location = crate::codes::test_spellings::TestLocation::RefuseNextSwitchRenewalForTest;
        self.shared_ref(location)?
            .switch_renewal_refusal_for_test
            .store(true, Ordering::Release);
        Ok(())
    }

    /// Workflow #437 MAPP-2d: the next member's run fails right after its switch, its renewal
    /// included, before anything else of the run (`backend_operation_failed`): a policy run before
    /// its request lifecycle, so its failed settlement releases its key; a direct run before its
    /// deadline is set again, so its failure path releases it.
    pub(crate) fn fail_next_run_after_switch_for_test(&self) -> RuntimeHostResult<()> {
        let location = crate::codes::test_spellings::TestLocation::FailNextRunAfterSwitchForTest;
        self.shared_ref(location)?
            .run_failure_after_switch_for_test
            .store(true, Ordering::Release);
        Ok(())
    }

    /// Workflow #437 MAPP-2d fix round 1: the next release of a policy run's failed settlement
    /// (`HostShared::ensure_scheduled_policy_lease_released`) takes the token the run was granted,
    /// not the one the scheduler holds (`HostShared::take_granted_token_release_for_test`): what a
    /// build that released with the granted token would do after a renewal.
    pub(crate) fn release_failed_policy_run_with_granted_token_for_test(
        &self,
    ) -> RuntimeHostResult<()> {
        let location =
            crate::codes::test_spellings::TestLocation::ReleaseFailedPolicyRunWithGrantedTokenForTest;
        self.shared_ref(location)?
            .granted_token_release_for_test
            .store(true, Ordering::Release);
        Ok(())
    }

    /// Workflow #437 MAPP-2b: with `off`, nothing of MAPP-2b applies to any instance from now on
    /// (`HostShared::switch_applies` answers `false`), as before the slice. It exists in this
    /// crate's test build only (the field and its check in `switch_applies` are `#[cfg(test)]`):
    /// a member's run with it off shows that the group check is the slice's only entry.
    pub(crate) fn switch_off_for_test(&self, off: bool) -> RuntimeHostResult<()> {
        let location = crate::codes::test_spellings::TestLocation::SwitchOffForTest;
        self.shared_ref(location)?
            .switch_off_for_test
            .store(off, Ordering::Release);
        Ok(())
    }

    /// Workflow #437 MAPP-2b: the time left of `alias`'s switch cooldown on the Runtime clock,
    /// `None` when none runs (as the host keeps it, in memory).
    pub(crate) fn switch_cooldown_remaining_ms_for_test(
        &self,
        alias: &str,
    ) -> RuntimeHostResult<Option<u64>> {
        let location = crate::codes::test_spellings::TestLocation::SwitchCooldownForTest;
        let shared = self.shared_ref(location)?;
        let member = shared
            .resolve_instance(alias)
            .map_err(|failure| *failure.error)?
            .instance_id();
        let now = shared.monotonic_ms()?;
        Ok(lock(&shared.switch_cooldowns, location)?
            .get(&member)
            .filter(|until| **until > now)
            .map(|until| until - now))
    }

    /// Workflow #437 MAPP-2b: how many members the host keeps a switch cooldown for, ended ones
    /// included until a policy cycle forgets them.
    pub(crate) fn switch_cooldown_entries_for_test(&self) -> RuntimeHostResult<usize> {
        let location = crate::codes::test_spellings::TestLocation::SwitchCooldownForTest;
        Ok(lock(&self.shared_ref(location)?.switch_cooldowns, location)?.len())
    }

    /// Workflow #437 MAPP-2b: the switch time the settlement of the dispatch `decision_id` will
    /// take out of its run's own time, as its dispatch clock holds it now; `None` when the
    /// dispatch has no clock.
    pub(crate) fn policy_switch_ms_for_test(
        &self,
        decision_id: &str,
    ) -> RuntimeHostResult<Option<u64>> {
        let location = crate::codes::test_spellings::TestLocation::PolicySwitchTimeForTest;
        Ok(
            lock(&self.shared_ref(location)?.policy_dispatch_clocks, location)?
                .get(decision_id)
                .map(|clock| clock.switch_ms),
        )
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
