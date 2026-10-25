// SPDX-License-Identifier: AGPL-3.0-only

//! IDLE-1 (Alice's ruling 41 of 10-24: when an instance has no task, close the game inside the
//! emulator, and the startup package relaunches it when a task is due; her ruling of 10-25:
//! 「没有任务的时候直接关闭模拟器……任务结束或者接下来一段时间比如30分钟内没有任务的时候关闭应用程序」, two
//! options of one function; Workflow #386 comment 6393646823): the host side of the idle action.
//!
//! - **When.** At every policy cycle, right after its evaluation and before actingd admits its
//!   dispatches, and at every lease sweep, the idle check reads, for every registered physical
//!   instance, its idle keys in effect (`idle.*`, the instance's own or the section's), what it
//!   shows (its lease, its queue, a keyless hold of its admission guard, the scheduling pauses
//!   and a #403 fence through its routine gate, its emulator state, its startup package and
//!   configured emulator start) and the scheduler's latest plan for it: the latest evaluation's
//!   not-run explanation (`PolicyHost::schedule_explanation`, the evaluator's own due candidates
//!   and next wake of the instance; no other estimate). The pure planner (`crate::idle_action`)
//!   decides; this module carries its steps out. Nothing here blocks the policy cycle: the
//!   action runs on the instance's worker, the wake only queues claims.
//! - **The action** runs on an emulator-control claim of the instance's key (no new claim kind,
//!   as #411 A38's recheck reuses the self-check claim), which no dispatch can pass. On the key
//!   the instance is checked once more: a pause or a queued request that came meanwhile leaves it
//!   alone. `close_app` reads the application id the game's standard pack root declares for the
//!   instance's server (`<standard_packs.root>/<game>/applications.json`,
//!   `servers.<server>.application_id`; no game word in the code), checks that it is the
//!   instance's assigned application (the read-only foreground query), and stops it through the
//!   existing application lifecycle path on the claim's key (`command.received`,
//!   `command.validated`, `application.intent`, `application.completed`), as the ladder's
//!   application-restart rung does. `stop_emulator` is an emulator Stop on the claim's key
//!   through the existing emulator-control path (the fence on the key, the retained session's
//!   close, the provider Stop through the configured or internal control, never `api`, the
//!   endpoint back to pending), so the next start finds no dead retained session.
//! - **The wake** at the next due time minus the action's wake lead, or at once for an earlier
//!   due task: after `close_app` the startup package is queued (`startup_package_scheduled`);
//!   after `stop_emulator` the configured emulator start (emu-watch's autostart claim, with its
//!   `emulator_autostart_queued`), whose start hands the key to the startup package as every
//!   configured start does. Queued at the cycle, the wake's claim holds the instance before the
//!   cycle's dispatch is admitted. While an instance's emulator is held stopped, the emulator
//!   watch does not start it again.
//! - **Records.** One `instance_idle_action` record per action and per wake (`operation`
//!   `idle_action` or `idle_wake`, `idle_action` naming the action), a `device_state_observed`
//!   record under the instance and the same report line.
//! - **Fail loud, one instance (#419).** A failed close or stop is recorded (the existing
//!   failure records, or `runtime.failed` with the failure) and fails only this instance's
//!   action, through the one decision point (`contain_failure`): the instance is left as it is,
//!   no wake follows, and its next due task goes through the normal path. Only a failure that is
//!   the Runtime's as a whole latches it.

use super::failure_scope::Contained;
use super::host_claims::{HostClaimWork, HostKey};
use super::input::ApplicationKey;
use super::lease::HostClaim;
use super::resource_fence::UnconfirmedResources;
use super::*;
use crate::codes::{HostCode, IdleCode, IdleLocation};
use crate::idle_action::{IdlePlan, IdleStep, IdleView, WakeWork};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{EmulatorRunningState, RuntimeScheduleExplanation};
use actingcommand_device::DeviceResourceQuiescence;
use actingcommand_runtime_config::IdleAction;

/// The root file of a game's standard pack that declares its application per server.
const APPLICATIONS_FILE: &str = "applications.json";
/// How often the lease sweep runs the idle check between policy cycles (a wake's timer). Every
/// policy cycle runs it too.
const IDLE_SWEEP_INTERVAL_MS: u64 = 1_000;

/// One instance's idle action, run on its worker under its emulator-control claim.
pub(super) struct PendingIdleAction {
    pub(super) instance_id: InstanceId,
    pub(super) instance_alias: String,
    action: IdleAction,
    next_due_unix_ms: Option<u64>,
    wake_at_unix_ms: Option<u64>,
    /// The game and the server the instance's policy identity names, when one does.
    identity: Option<(String, String)>,
    /// Shared by the claim's request and the action's own requests.
    causation_id: actingcommand_contract::IssuedCausationId,
}

/// The scheduler's plan for `alias` in the latest evaluation: a task is due when the evaluation
/// selected one or one is due and did not run; its next wake is the instance's earliest. An
/// instance the evaluation decided nothing for has no due task and no wake; none before the
/// first evaluated cycle.
fn idle_plan(explanation: &RuntimeScheduleExplanation, alias: &str) -> Option<IdlePlan> {
    let evaluated_at_unix_ms = explanation.evaluated_at_unix_ms?;
    let entry = explanation
        .instances
        .iter()
        .find(|entry| entry.instance_alias == alias);
    Some(IdlePlan {
        evaluated_at_unix_ms,
        due_now: entry
            .is_some_and(|entry| entry.selected.is_some() || entry.not_running.task_id.is_some()),
        next_wake_unix_ms: entry.and_then(|entry| entry.next_wake_unix_ms),
    })
}

/// `instance_idle_action` for `alias` and `action` at `operation`.
fn idle_outcome(
    alias: &str,
    action: IdleAction,
    operation: IdleLocation,
    next_due_unix_ms: Option<u64>,
    wake_at_unix_ms: Option<u64>,
) -> Outcome {
    let outcome = Outcome::new(IdleCode::Taken)
        .operation(operation)
        .alias(alias)
        .idle_action(action.as_str());
    let outcome = match next_due_unix_ms {
        Some(due) => outcome.next_occurrence_unix_ms(due),
        None => outcome,
    };
    match wake_at_unix_ms {
        Some(at) => outcome.at_unix_ms(at),
        None => outcome,
    }
}

/// A failure of one instance's idle action, `code` at `operation`, with `detail` as its foreign
/// cause.
fn idle_error(
    code: HostCode,
    operation: IdleLocation,
    instance_id: InstanceId,
    detail: String,
) -> RuntimeHostError {
    let mut error = RuntimeHostError::request(code, operation, RuntimeErrorCode::InvalidRequest)
        .with_native_detail(detail);
    error.lifecycle.instance_id = Some(instance_id);
    error
}

impl HostShared {
    /// The idle check of every registered physical instance now: at every policy cycle (after
    /// its evaluation) and, `from_sweep`, at most every second of the lease sweep, which skips
    /// it while the policy host is busy rather than wait for it. A step that fails for one
    /// instance is contained to it and the next instance goes on; only a failure the Runtime does
    /// not survive is the error.
    pub(super) fn check_idle_instances(&self, from_sweep: bool) -> RuntimeHostResult<()> {
        if self.fatal.is_shutdown_requested() || self.lifecycle_draining()? {
            return Ok(());
        }
        if from_sweep {
            let now_ms = self.monotonic_ms()?;
            if !lock(&self.idle, IdleLocation::Check)?.sweep_due(now_ms, IDLE_SWEEP_INTERVAL_MS) {
                return Ok(());
            }
        }
        let identities = lock(&self.policy_inputs, IdleLocation::Check)?
            .as_ref()
            .map(|inputs| {
                inputs
                    .instance_identities()
                    .map(|identity| {
                        (
                            identity.instance_id.to_owned(),
                            (identity.game_id.to_owned(), identity.server_id.to_owned()),
                        )
                    })
                    .collect::<BTreeMap<_, _>>()
            })
            .unwrap_or_default();
        let instances = lock(&self.registered_instances, IdleLocation::Check)?
            .values()
            .filter(|instance| instance.provenance() == ExecutionBackendProvenance::PhysicalDevice)
            .map(|instance| {
                (
                    instance.instance_id(),
                    instance.instance_alias.clone(),
                    matches!(instance.emulator_state, EmulatorRunningState::On { .. })
                        && !instance.endpoint_pending(),
                )
            })
            .collect::<Vec<_>>();
        if instances.is_empty() {
            return Ok(());
        }
        let explanation = {
            let policy = self.policy()?;
            if from_sweep {
                match policy.try_lock() {
                    Ok(policy) => policy.schedule_explanation(None, false),
                    Err(TryLockError::WouldBlock) => return Ok(()),
                    Err(TryLockError::Poisoned(_)) => {
                        return Err(lock_poison_error(IdleLocation::Check));
                    }
                }
            } else {
                lock(policy, IdleLocation::Check)?.schedule_explanation(None, false)
            }
        };
        let now_unix_ms = self.runtime_clock_sample()?.unix_ms;
        let mut planner = lock(&self.idle, IdleLocation::Check)?;
        for (instance_id, alias, emulator_on) in instances {
            if self.fatal.is_shutdown_requested() {
                break;
            }
            let Some(config) = self.idle_settings(instance_id)? else {
                continue;
            };
            let view = self.idle_view(instance_id, emulator_on)?;
            let plan = idle_plan(&explanation, &alias);
            let Some(step) = planner.check(&alias, &config, view, plan, now_unix_ms) else {
                continue;
            };
            let taken = match step {
                IdleStep::Act {
                    action,
                    next_due_unix_ms,
                    wake_at_unix_ms,
                } => self.request_idle_action(PendingIdleAction {
                    instance_id,
                    instance_alias: alias.clone(),
                    action,
                    next_due_unix_ms,
                    wake_at_unix_ms,
                    identity: identities.get(&alias).cloned(),
                    causation_id: self
                        .events
                        .issuer()
                        .mint_causation_id()
                        .map_err(|_| runtime_identifier_error())?,
                }),
                IdleStep::Wake { action, work } => {
                    self.queue_idle_wake(instance_id, &alias, action, work)
                }
            };
            if let Err(error) = taken {
                if matches!(step, IdleStep::Act { .. }) {
                    planner.finished(&alias, false);
                }
                self.contain_idle_failure(instance_id, error, self.idle_links(instance_id)?)?;
            }
        }
        Ok(())
    }

    /// What the instance shows to the idle check now. Read without waiting: a held admission
    /// guard (a keyless observe, probe or Lab look, or work starting on the instance) is work.
    fn idle_view(&self, instance_id: InstanceId, emulator_on: bool) -> RuntimeHostResult<IdleView> {
        let paused = self.routine_gate(instance_id)? == ClaimGate::RoutineHeld;
        let (leased, queued) = {
            let scheduler = lock(&self.scheduler, IdleLocation::Check)?;
            (
                scheduler.active_lease(instance_id).is_some(),
                scheduler.queued_count(instance_id) > 0,
            )
        };
        let instance_guard = self
            .instance_guard(instance_id)
            .map_err(|failure| *failure.error)?;
        let manual = match instance_guard.try_lock() {
            Ok(_) => false,
            Err(TryLockError::WouldBlock) => true,
            Err(TryLockError::Poisoned(_)) => return Err(lock_poison_error(IdleLocation::Check)),
        };
        let startup_package = self.startup_packages()?.contains_key(&instance_id);
        let configured_start = self
            .emulator_autostart()?
            .iter()
            .any(|(configured, _)| *configured == instance_id);
        Ok(IdleView {
            leased,
            queued,
            manual,
            paused,
            emulator_on,
            app_wakeable: startup_package,
            emulator_wakeable: startup_package && configured_start,
        })
    }

    /// Queues the action's emulator-control claim (normal priority, the scheduler's default
    /// TTL); its worker runs the action once the key is granted.
    fn request_idle_action(&self, pending: PendingIdleAction) -> RuntimeHostResult<()> {
        let resolved = self
            .resolve_instance(&pending.instance_alias)
            .map_err(|failure| *failure.error)?;
        let connection_id = ConnectionId::new(STARTUP_PACKAGE_CONNECTION_VALUE)
            .map_err(|error| RuntimeHostError::scheduler(IdleLocation::Action, &error))?;
        let issuer = self.events.issuer();
        let holder_id = *issuer
            .mint_holder_id()
            .map_err(|_| runtime_identifier_error())?
            .transport();
        let (actor, source) = scheduled_request_transport_origin(resolved.provenance());
        let request = RuntimeRequest::new(
            issuer
                .mint_request_id()
                .map_err(|_| runtime_identifier_error())?,
            issuer
                .mint_correlation_id()
                .map_err(|_| runtime_identifier_error())?,
            Some(pending.causation_id),
            actor,
            source,
            unix_ms_now()?,
            RuntimeOperation::AcquireLease {
                instance_alias: pending.instance_alias.clone(),
                holder_id,
            },
        )
        .map_err(|_| {
            RuntimeHostError::fatal(
                HostCode::HostKeyRequestInvalid,
                IdleLocation::Action,
                RuntimeErrorCode::RuntimeHalted,
            )
        })?;
        let lease_ttl_ms = lock(&self.scheduler, IdleLocation::Action)?
            .config()
            .lease_ttl_ms;
        let instance_alias = pending.instance_alias.clone();
        self.request_host_claim_work(
            HostClaim {
                request: &request,
                instance_alias: &instance_alias,
                holder_id,
                connection_id,
                kind: ClaimKind::EmulatorControl,
                priority: actingcommand_contract::LeasePriority::Normal,
                lease_ttl_ms,
            },
            HostClaimWork::IdleAction(Box::new(pending)),
        )
        .map_err(|failure| {
            if failure.poison_runtime {
                (*failure.error).into_fatal()
            } else {
                *failure.error
            }
        })
    }

    /// The wake of an idle instance after `action`: its record, then `work` queued in order. The
    /// configured emulator start hands the key to the startup package itself, so nothing after
    /// it is queued here.
    fn queue_idle_wake(
        &self,
        instance_id: InstanceId,
        alias: &str,
        action: IdleAction,
        work: &[WakeWork],
    ) -> RuntimeHostResult<()> {
        let links = self.idle_links(instance_id)?;
        self.record_idle(
            links.clone(),
            idle_outcome(alias, action, IdleLocation::Wake, None, None),
        )?;
        for item in work {
            match item {
                WakeWork::EmulatorStart => {
                    let position = self
                        .emulator_autostart()?
                        .iter()
                        .position(|(configured, _)| *configured == instance_id)
                        .ok_or_else(|| {
                            RuntimeHostError::request(
                                HostCode::EmulatorAutostartInstanceUnknown,
                                IdleLocation::Wake,
                                RuntimeErrorCode::InvalidRequest,
                            )
                            .with_outcome_values(|outcome| outcome.alias(alias))
                        })?;
                    self.record_idle(
                        links.clone(),
                        Outcome::new(HostCode::EmulatorAutostartQueued)
                            .count(1_usize)
                            .alias(alias)
                            .idle_action(action.as_str()),
                    )?;
                    return self.request_watch_autostart(position, instance_id, alias);
                }
                WakeWork::StartupPackage => {
                    let resolved = self
                        .resolve_instance(alias)
                        .map_err(|failure| *failure.error)?;
                    let control_request_id = *self
                        .events
                        .issuer()
                        .mint_request_id()
                        .map_err(|_| runtime_identifier_error())?
                        .transport();
                    let pending = self
                        .prepare_startup_package(&resolved, links.clone(), control_request_id)
                        .map_err(|failure| *failure.error)?;
                    self.schedule_startup_package(pending)
                        .map_err(|failure| *failure.error)?;
                }
            }
        }
        Ok(())
    }

    /// A granted idle action's claim on its instance's worker. The instance is checked once
    /// more on the key; then the action runs, its end is reported to the planner, the key is
    /// released and the action recorded. A failure is contained to the instance (#419); only one the
    /// Runtime does not survive is the error, which ends the worker.
    pub(super) fn run_idle_action(
        &self,
        pending: &PendingIdleAction,
        key: HostKey,
    ) -> RuntimeHostResult<()> {
        let alias = pending.instance_alias.as_str();
        let paused = self.routine_gate(pending.instance_id)? == ClaimGate::RoutineHeld;
        let queued =
            lock(&self.scheduler, IdleLocation::Action)?.queued_count(pending.instance_id) > 0;
        if paused
            || queued
            || pending.action == IdleAction::None
            || self.fatal.is_shutdown_requested()
        {
            lock(&self.idle, IdleLocation::Action)?.abandoned(alias);
            return self.release_host_key(&key);
        }
        let claim = key.request.clone();
        let validated = claim.validate().map_err(|_| {
            RuntimeHostError::fatal(
                HostCode::HostKeyRequestInvalid,
                IdleLocation::Action,
                RuntimeErrorCode::RuntimeHalted,
            )
        })?;
        let links = self
            .events
            .request_links(&validated, Some(pending.instance_id), None, None);
        // The stop ends its key's hold itself; the close leaves the key with the claim, which
        // releases it only once the planner knows the action's end, so no check meanwhile takes
        // the free instance for one that was never acted on.
        let (ended, held) = match pending.action {
            IdleAction::StopEmulator => (
                self.idle_stop_emulator(pending, &validated, links.clone(), key),
                None,
            ),
            _ => (self.idle_close_app(pending, &key, links.clone()), Some(key)),
        };
        lock(&self.idle, IdleLocation::Action)?.finished(alias, matches!(ended, Ok(Ok(_))));
        if let Some(key) = &held
            && let Err(error) = self.release_host_key(key)
        {
            return self.contain_idle_failure(pending.instance_id, error, links);
        }
        let application_id = match ended {
            Ok(Ok(application_id)) => application_id,
            Ok(Err(())) => return Ok(()),
            Err(error) => return self.contain_idle_failure(pending.instance_id, error, links),
        };
        let outcome = idle_outcome(
            alias,
            pending.action,
            IdleLocation::Action,
            pending.next_due_unix_ms,
            pending.wake_at_unix_ms,
        );
        let outcome = match application_id {
            Some(application_id) => outcome.application_id(application_id),
            None => outcome,
        };
        self.record_idle(links, outcome)
    }

    /// `close_app` on the claim's `key`: the declared application, the check that it is the
    /// assigned one, then the existing application stop on the key, which stays with the claim
    /// whatever the stop's result. `Ok(Ok(application))` when it stopped; `Ok(Err(()))` when it
    /// failed and the failure is recorded; `Err` for a failure not recorded yet.
    fn idle_close_app(
        &self,
        pending: &PendingIdleAction,
        key: &HostKey,
        links: EventLinksDraft,
    ) -> RuntimeHostResult<Result<Option<String>, ()>> {
        let alias = pending.instance_alias.as_str();
        let declared = match self.declared_application(pending) {
            Ok(declared) => declared,
            Err(error) => {
                self.append_lifecycle_failure(
                    RuntimeLifecycleFailureStage::OperationCleanup,
                    RuntimeLifecycleFailure::Host(&error),
                    links,
                    None,
                )?;
                return Ok(Err(()));
            }
        };
        // The read-only foreground query reports the instance's assigned application.
        let refusal = match self.execution()?.observe_foreground_application(alias) {
            Ok(observation) if observation.assigned == declared => None,
            Ok(observation) => Some(
                idle_error(
                    HostCode::InvalidApplicationIdentity,
                    IdleLocation::Action,
                    pending.instance_id,
                    format!(
                        "instance_alias={alias}; declared={declared}; assigned={}",
                        observation.assigned
                    ),
                )
                .with_outcome_values(|outcome| {
                    outcome.alias(alias).application_id(declared.as_str())
                }),
            ),
            Err(error) => {
                // As the foreground gate: the ADB baseline is gone, and an adb child or pipe left
                // unconfirmed fences the instance.
                self.invalidate_adb_baseline(pending.instance_id)?;
                let detail = format!("instance_alias={alias}; adb_failed={error}");
                if error.resource_quiescence() == Some(DeviceResourceQuiescence::Unconfirmed) {
                    let mut unconfirmed = RuntimeHostError::fatal(
                        HostCode::ApplicationForegroundUnknown,
                        IdleLocation::Action,
                        RuntimeErrorCode::RuntimeHalted,
                    )
                    .with_native_detail(detail.clone());
                    unconfirmed.lifecycle.instance_id = Some(pending.instance_id);
                    unconfirmed.lifecycle.resource_quiescence =
                        Some(ResourceQuiescence::Unconfirmed);
                    let _fence = self.settle_unconfirmed_adb(
                        unconfirmed,
                        pending.instance_id,
                        links.clone(),
                    )?;
                }
                Some(idle_error(
                    HostCode::ApplicationForegroundUnknown,
                    IdleLocation::Action,
                    pending.instance_id,
                    detail,
                ))
            }
        };
        if let Some(refusal) = refusal {
            self.append_lifecycle_failure(
                RuntimeLifecycleFailureStage::OperationCleanup,
                RuntimeLifecycleFailure::Host(&refusal),
                links,
                None,
            )?;
            return Ok(Err(()));
        }
        let action = actingcommand_contract::ApplicationLifecycleAction::Stop;
        let resolved = self
            .resolve_instance(alias)
            .map_err(|failure| *failure.error)?;
        let issuer = self.events.issuer();
        let (actor, source) = scheduled_request_transport_origin(resolved.provenance());
        let message = RuntimeRequest::new(
            issuer
                .mint_request_id()
                .map_err(|_| runtime_identifier_error())?,
            issuer
                .mint_correlation_id()
                .map_err(|_| runtime_identifier_error())?,
            Some(pending.causation_id),
            actor,
            source,
            unix_ms_now()?,
            RuntimeOperation::ApplicationLifecycle {
                instance_alias: alias.to_owned(),
                holder_id: key.token.holder_id(),
                action,
            },
        )
        .map_err(|_| {
            RuntimeHostError::fatal(
                HostCode::HostKeyRequestInvalid,
                IdleLocation::Action,
                RuntimeErrorCode::RuntimeHalted,
            )
        })?;
        let stop = message.validate().map_err(|_| {
            RuntimeHostError::fatal(
                HostCode::HostKeyRequestInvalid,
                IdleLocation::Action,
                RuntimeErrorCode::RuntimeHalted,
            )
        })?;
        let stop_links = self
            .events
            .request_links(&stop, Some(pending.instance_id), None, None);
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
        // The key is the claim's and stays with it whatever the stop's result, as a ladder's.
        match self.application_control(
            &stop,
            &key.token,
            action,
            key.connection_id,
            None,
            ApplicationKey::Ladder,
        ) {
            Ok(_) => Ok(Ok(Some(declared))),
            Err(mut failure) => {
                if let UnconfirmedResources::Fenced(fence) = self.settle_unconfirmed_resources(
                    &failure.error,
                    Some(pending.instance_id),
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
                    stop_links,
                    None,
                )?;
                Ok(Err(()))
            }
        }
    }

    /// `stop_emulator` on the claim's `key` through the existing emulator-control path
    /// (`drive_idle_stop`), which records its own failure pair and ends the key's hold.
    fn idle_stop_emulator(
        &self,
        pending: &PendingIdleAction,
        validated: &ValidatedRuntimeRequest<'_>,
        links: EventLinksDraft,
        key: HostKey,
    ) -> RuntimeHostResult<Result<Option<String>, ()>> {
        let resolved = match self.resolve_instance(&pending.instance_alias) {
            Ok(resolved) => resolved,
            Err(failure) => {
                self.release_host_key(&key)?;
                return Err(*failure.error);
            }
        };
        match self.drive_idle_stop(validated, &resolved, links, key) {
            Ok(()) => Ok(Ok(None)),
            Err(failure) if failure.poison_runtime => Err((*failure.error).into_fatal()),
            Err(failure) if failure.error.is_fatal() => Err(*failure.error),
            Err(_) => Ok(Err(())),
        }
    }

    /// The application id `<standard_packs.root>/<game>/applications.json` declares for the
    /// instance's server (`servers.<server>.application_id`); `invalid_application_identity`
    /// (with what is missing) when there is no root, no file, no such member, or the file does
    /// not read.
    fn declared_application(&self, pending: &PendingIdleAction) -> RuntimeHostResult<String> {
        let missing = |detail: String| {
            idle_error(
                HostCode::InvalidApplicationIdentity,
                IdleLocation::Action,
                pending.instance_id,
                detail,
            )
            .with_outcome_values(|outcome| {
                let outcome = outcome.alias(pending.instance_alias.as_str());
                match &pending.identity {
                    Some((game, server)) => outcome.game(game.as_str()).server(server.as_str()),
                    None => outcome,
                }
            })
        };
        let (game, server) = pending.identity.as_ref().ok_or_else(|| {
            missing("no policy identity names the instance's game and server".to_owned())
        })?;
        let root = self
            .standard_packs
            .root()
            .ok_or_else(|| missing("no standard_packs.root is configured".to_owned()))?;
        let path = root.join(game).join(APPLICATIONS_FILE);
        let bytes = fs::read(&path)
            .map_err(|error| missing(format!("{APPLICATIONS_FILE} does not read: {error}")))?;
        let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
        let value = serde_json::from_slice::<serde_json::Value>(bytes)
            .map_err(|error| missing(format!("{APPLICATIONS_FILE} is no JSON: {error}")))?;
        value
            .get("servers")
            .and_then(|servers| servers.get(server))
            .and_then(|server| server.get("application_id"))
            .and_then(serde_json::Value::as_str)
            .filter(|application_id| !application_id.trim().is_empty())
            .map(str::to_owned)
            .ok_or_else(|| {
                missing(format!(
                    "{APPLICATIONS_FILE} declares no servers.<server>.application_id for the instance's server"
                ))
            })
    }

    /// One instance's idle failure through the one decision point (#419): a failure of the
    /// instance or of its operation is recorded under `links` and ends only this action; one
    /// that is the Runtime's latches it and is the error.
    fn contain_idle_failure(
        &self,
        instance_id: InstanceId,
        error: RuntimeHostError,
        links: EventLinksDraft,
    ) -> RuntimeHostResult<()> {
        match self.contain_failure(error, Some(instance_id), links.clone())? {
            Contained::Latched(error) => Err(error),
            Contained::Fenced(failure) | Contained::Failed(failure) => self
                .append_lifecycle_failure(
                    RuntimeLifecycleFailureStage::OperationCleanup,
                    RuntimeLifecycleFailure::Host(&failure),
                    links,
                    None,
                ),
        }
    }

    /// Whether the idle action holds the instance's emulator stopped (`stop_emulator` running or
    /// done, its wake not come yet): the emulator watch does not start it.
    pub(super) fn idle_holds_emulator_stopped(&self, alias: &str) -> RuntimeHostResult<bool> {
        Ok(lock(&self.idle, IdleLocation::Check)?.emulator_held_stopped(alias))
    }

    /// One `device_state_observed` record carrying `outcome` under the instance (`links`), and
    /// the same report line.
    fn record_idle(&self, links: EventLinksDraft, outcome: Outcome) -> RuntimeHostResult<()> {
        self.append_event_raw(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links,
            RuntimePayloadDraft::lifecycle_observed(
                self.owner_epoch,
                RuntimeLifecyclePhase::DeviceStateObserved {
                    outcome: outcome.clone(),
                },
                AuditInput::new(),
            ),
        )?;
        self.report_emulator_autostart(outcome)
    }

    /// The links of the idle check's own records: the instance under fresh system links.
    fn idle_links(&self, instance_id: InstanceId) -> RuntimeHostResult<EventLinksDraft> {
        Ok(self
            .events
            .system_links()?
            .with_instance_id(self.events.issuer().issue_registered_instance(instance_id)))
    }
}
