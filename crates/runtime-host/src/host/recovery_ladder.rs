// SPDX-License-Identifier: AGPL-3.0-only

//! The stuck-recovery ladder (Runtime slice #316-B4).
//!
//! A scheduled contained task run on a physical instance that commits `task.failed` with
//! `contained_task_page_unknown`, a `contained_task_recovery_*` or a
//! `contained_task_home_recovery_*` code starts a ladder for its instance, unless the
//! instance's `stuck_recovery` is off. A direct task run (CLI or console task-run, MCP
//! `ac_run_pack`) never starts one (Workflow #369-3, coordinator rulings Q1 and P1: the ladder
//! exists for the routine). A failed daemon-start or connection preparation (emulator start or
//! restart, resume, self-check) still starts one. The ladder never runs on the run's own
//! thread: a scheduled run's trigger is admitted when the run returned, and the accepted
//! ladder waits in the startup package queue for the host's scheduling thread.
//!
//! The rungs, in this fixed order, are existing work under the instance lease:
//! `return_home` runs the failed run's bound recovery package as a standalone contained task
//! (when it bound none, the return-home package actingd configures for its package's game and
//! server, with the longest response deadline; Workflow #336 L2d),
//! `application_restart` force-stops the instance's assigned application and then schedules
//! and runs the instance's startup package (Workflow #369-2), and
//! `emulator_restart` confirms Stop before Start through the existing control owner, then
//! prepares the discovered binding. Environment readiness and a configured package's target
//! attainment are separate facts. Missing packages or known unavailable entry channels skip
//! only the affected rung. The original task is never re-run.
//!
//! One ladder per instance per cool-down window: a trigger while a ladder is queued or
//! running, or inside the window, records `recovery_ladder_suppressed` and nothing else. Every
//! fact is a `runtime.lifecycle_observed` linked to the instance and the trigger run's
//! correlation id, under the ladder's own request and causation ids; the `return_home` run
//! carries that causation id, the startup package runs the one of their scheduling event.

use super::contained_task::{ContainedRunControl, PackageIdentity};
use super::startup_package::{HostPackageRun, PendingHostWork, PendingStartupPackage};
use super::*;
use actingcommand_contract::{
    BackendObservationStatus, BackendOpenEntry, ContainedTaskRecoveryBinding,
    EmulatorInstanceAction, InstanceStuckRecovery, IssuedCausationId, RecoveryLadderOutcome,
    RecoveryLadderSuppression, RecoveryLadderTrigger, RecoveryRung, RecoveryRungOutcome,
    RecoveryRungPlan, RecoveryRungSkipReason, RecoveryRungState, RecoveryTriggerStage,
    SchedulingResumeSelfCheck, is_stuck_recovery_trigger,
};
use actingcommand_device::DeviceResourceQuiescence;

const LADDER_OPERATION: &str = "run_recovery_ladder";
/// Workflow #369-1: how long after a confirmed Start the emulator restart rung waits for a
/// usable environment.
const RECOVERY_READINESS_WINDOW: Duration = Duration::from_secs(120);
/// Workflow #369-1: the waits between readiness attempts: 5 s, 10 s, then 20 s each.
const RECOVERY_READINESS_BACKOFF: [Duration; 3] = [
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(20),
];
/// Workflow #369-1 (review L1): how often a readiness wait checks shutdown and install drain.
const RECOVERY_READINESS_POLL: Duration = Duration::from_millis(500);

/// One triggered ladder, queued for the scheduling thread.
#[derive(Clone)]
pub(super) struct PendingRecoveryLadder {
    instance_id: InstanceId,
    instance_alias: String,
    trigger: RecoveryLadderTrigger,
    recovery: Option<ContainedTaskRecoveryBinding>,
    /// Workflow #336 L2d: when `recovery` is the configured return-home package (the run bound
    /// none), the failed package it must match.
    recovery_configured: Option<PackageIdentity>,
    /// Instance, trigger correlation id, the ladder's request id and causation id.
    links: EventLinksDraft,
    /// The ladder's request id: task timing admission id of its rung runs.
    request_id: RequestId,
    causation_id: IssuedCausationId,
    cooldown_ms: u64,
}

/// Per instance: whether a ladder is queued or running, and where the cool-down window the
/// last accepted ladder opened ends.
#[derive(Default)]
pub(super) struct RecoveryLadderWindow {
    running: bool,
    until_unix_ms: u64,
    pub(super) preparation: Option<TerminalEvent>,
}

impl RecoveryLadderWindow {
    pub(super) fn is_running(&self) -> bool {
        self.running
    }
}

/// Workflow #369-2: what the application restart rung does after its entry checks.
enum RestartEntry {
    /// A known unavailable entry channel or ADB: skip the rung, the game untouched.
    Skip(RecoveryRungSkipReason),
    /// Stop the assigned application, then run the startup package.
    Stop,
    /// The startup package cannot be admitted, its prerequisite chain is refused, or it is
    /// incompatible with a startup run: run it without the stop, so its refusal is recorded as
    /// before.
    RunOnly,
}

enum RungAttempt {
    Recovered {
        run_id: RunId,
    },
    EnvironmentReady {
        event: TerminalEvent,
    },
    Skipped {
        reason: RecoveryRungSkipReason,
    },
    Failed {
        run_id: Option<RunId>,
        reason: &'static str,
    },
}

/// Binds the configured stuck-recovery settings to registered instances at startup; an alias
/// that is not registered fails with `stuck_recovery_instance_unknown`. An instance without
/// settings uses the defaults.
pub(super) fn resolve_stuck_recovery(
    configured: &BTreeMap<String, InstanceStuckRecovery>,
    instances: &BTreeMap<InstanceId, RegisteredInstance>,
) -> RuntimeHostResult<BTreeMap<InstanceId, InstanceStuckRecovery>> {
    configured
        .iter()
        .map(|(alias, settings)| {
            instances
                .values()
                .find(|instance| instance.instance_alias == *alias)
                .map(|instance| (instance.instance_id(), *settings))
                .ok_or_else(|| {
                    RuntimeHostError::fatal(
                        "stuck_recovery_instance_unknown",
                        "start_runtime_host",
                        RuntimeErrorCode::RuntimeFatal,
                    )
                    .with_native_detail(format!("instance_alias={alias}"))
                })
        })
        .collect()
}

/// Folds a staging failure (always fatal: a lock, an identifier or the ledger) into the
/// run's own result.
pub(super) fn with_recovery_ladder_staging<T>(
    executed: Result<T, RequestFailure>,
    staged: RuntimeHostResult<()>,
) -> Result<T, RequestFailure> {
    match (executed, staged) {
        (executed, Ok(())) => executed,
        (Ok(_), Err(error)) => Err(RequestFailure::poison_without_terminal(error)),
        (Err(failure), Err(error)) => Err(failure.replace_with_poison(error)),
    }
}

fn ladder_invariant(code: &'static str) -> RuntimeHostError {
    RuntimeHostError::fatal(code, LADDER_OPERATION, RuntimeErrorCode::RuntimeFatal)
}

impl HostShared {
    /// The summary is appended only after original acquisition disposal and the existing
    /// session close/release path returned. Its event identity is the preparation attempt.
    pub(super) fn record_preparation_finished(
        &self,
        instance_id: InstanceId,
        links: EventLinksDraft,
        stage: RecoveryTriggerStage,
        observations: &[actingcommand_device::BackendOpenObservation],
        check: &SchedulingResumeSelfCheck,
        recoverable: bool,
    ) -> RuntimeHostResult<TerminalEvent> {
        let channel = |entry, passed| {
            if passed {
                return BackendObservationStatus::Passed;
            }
            observations
                .iter()
                .rev()
                .map(|value| &value.report)
                .find(|report| report.entry == entry || report.entry == BackendOpenEntry::NemuPair)
                .map_or(BackendObservationStatus::Unknown, |report| {
                    let own = if entry == BackendOpenEntry::Input {
                        report.input_check
                    } else {
                        report.capture_check
                    };
                    if [report.status, report.connection, own]
                        .contains(&BackendObservationStatus::Failed)
                    {
                        BackendObservationStatus::Failed
                    } else {
                        BackendObservationStatus::Unknown
                    }
                })
        };
        let event = self.append_event_raw(
            if check.failure_code.is_some() {
                EventSeverity::Warning
            } else {
                EventSeverity::Info
            },
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links.clone(),
            RuntimePayloadDraft::lifecycle_observed(
                self.owner_epoch,
                RuntimeLifecyclePhase::InstancePreparationFinished {
                    stage,
                    capture: channel(BackendOpenEntry::Capture, check.capture.ok),
                    input: channel(BackendOpenEntry::Input, check.touch.ok),
                    failure_code: check.failure_code.clone(),
                },
                AuditInput::new(),
            ),
        )?;
        let event = terminal(&event);
        lock(&self.recovery_ladders, "record_preparation_attempt")?
            .entry(instance_id)
            .or_default()
            .preparation = Some(event);
        // Workflow #369-3 (coordinator ruling P1): the direct-run exemption covers task runs
        // only, which make no preparation of their own. A failed daemon-start or connection
        // preparation (emulator start or restart, resume, self-check) still starts a ladder, as
        // before: nothing else retries those. Only the ladder's own preparation never does.
        if !recoverable
            || stage == RecoveryTriggerStage::RecoveryPreparation
            || self.fatal.is_shutdown_requested()
        {
            return Ok(event);
        }
        let Some(failure_code) = &check.failure_code else {
            return Ok(event);
        };
        let settings = self
            .stuck_recovery()?
            .get(&instance_id)
            .copied()
            .unwrap_or_default();
        let resolved = lock(&self.registered_instances, "read_preparation_instance")?
            .get(&instance_id)
            .cloned()
            .ok_or_else(|| ladder_invariant("recovery_ladder_instance_missing"))?;
        if !settings.enabled || resolved.provenance() != ExecutionBackendProvenance::PhysicalDevice
        {
            return Ok(event);
        }
        let request_id = self
            .events
            .issuer()
            .mint_request_id()
            .map_err(|_| runtime_identifier_error())?;
        let causation_id = self
            .events
            .issuer()
            .mint_causation_id()
            .map_err(|_| runtime_identifier_error())?;
        self.admit_recovery_ladder(PendingRecoveryLadder {
            instance_id,
            instance_alias: resolved.instance_alias,
            trigger: RecoveryLadderTrigger {
                stage,
                run_id: None,
                task_id: None,
                preparation: Some(event),
                failure_code: failure_code.clone(),
            },
            recovery: None,
            recovery_configured: None,
            links: links
                .with_request_id(request_id)
                .with_causation_id(causation_id),
            request_id: *request_id.transport(),
            causation_id,
            cooldown_ms: u64::from(settings.cooldown_secs) * 1_000,
        })?;
        Ok(event)
    }

    fn recovery_admitted(&self, pending: &PendingRecoveryLadder) -> RuntimeHostResult<bool> {
        let paused = lock(&self.scheduling_pause, "gate_recovery_pause")?
            .deferral(&pending.instance_alias)
            .is_some();
        let capacity = if paused || self.fatal.is_shutdown_requested() {
            false
        } else {
            match self.require_business_capacity(pending.links.clone()) {
                Ok(()) => true,
                Err(failure) if failure.poison_runtime || failure.error.is_fatal() => {
                    return Err(*failure.error);
                }
                Err(_) => false,
            }
        };
        if !capacity {
            self.append_recovery_ladder_fact(
                pending,
                EventSeverity::Warning,
                RuntimeLifecyclePhase::RecoveryLadderSuppressed {
                    reason: RecoveryLadderSuppression::AdmissionDenied,
                    until_unix_ms: 0,
                },
            )?;
        }
        Ok(capacity)
    }

    /// Takes a scheduled run's committed `task.failed` terminal and, when it triggers a ladder,
    /// admits one for the instance.
    pub(super) fn stage_recovery_ladder(
        &self,
        control: &ContainedRunControl,
        request: &ValidatedRuntimeRequest<'_>,
        resolved: &RegisteredInstance,
        task_request: &ContainedTaskRequest,
    ) -> RuntimeHostResult<()> {
        let Some(terminal) = control.take_failed_terminal()? else {
            return Ok(());
        };
        let instance_id = resolved.instance_id();
        let settings = self
            .stuck_recovery()?
            .get(&instance_id)
            .copied()
            .unwrap_or_default();
        if !is_stuck_recovery_trigger(terminal.failure_code)
            || resolved.provenance() != ExecutionBackendProvenance::PhysicalDevice
            || !settings.enabled
        {
            return Ok(());
        }
        // Workflow #336 L2d (R23): a run that binds no recovery package takes the return-home
        // package actingd configures for its package's game and server.
        let (recovery, recovery_configured) = match task_request.recovery() {
            Some(binding) => (Some(binding.clone()), None),
            None => {
                let failed = control
                    .package()
                    .ok_or_else(|| ladder_invariant("recovery_ladder_package_missing"))?;
                match self.configured_return_home(&failed.game, &failed.server)? {
                    Some(binding) => (Some(binding.clone()), Some(failed.clone())),
                    None => (None, None),
                }
            }
        };
        let issuer = self.events.issuer();
        let request_id = issuer
            .mint_request_id()
            .map_err(|_| runtime_identifier_error())?;
        let causation_id = issuer
            .mint_causation_id()
            .map_err(|_| runtime_identifier_error())?;
        let pending = PendingRecoveryLadder {
            instance_id,
            instance_alias: resolved.instance_alias.clone(),
            trigger: RecoveryLadderTrigger {
                stage: RecoveryTriggerStage::Task,
                run_id: Some(*terminal.run_id.transport()),
                task_id: Some(*terminal.task_id.transport()),
                preparation: None,
                failure_code: terminal.failure_code.to_owned(),
            },
            recovery,
            recovery_configured,
            links: request
                .event_links(Some(instance_id), None, None)
                .with_request_id(request_id)
                .with_causation_id(causation_id),
            request_id: *request_id.transport(),
            causation_id,
            cooldown_ms: u64::from(settings.cooldown_secs) * 1_000,
        };
        self.admit_recovery_ladder(pending)
    }

    /// Queues the ladder, or records why it is suppressed: one already queued or running
    /// for the instance, or the cool-down window the last accepted one opened.
    fn admit_recovery_ladder(&self, pending: PendingRecoveryLadder) -> RuntimeHostResult<()> {
        if !self.recovery_admitted(&pending)? {
            return Ok(());
        }
        let now_unix_ms = self.clock.sample()?.unix_ms;
        let suppressed = {
            let mut ladders = lock(&self.recovery_ladders, "admit_recovery_ladder")?;
            let window = ladders.entry(pending.instance_id).or_default();
            if window.running {
                Some((
                    RecoveryLadderSuppression::AlreadyRunning,
                    window.until_unix_ms,
                ))
            } else if now_unix_ms < window.until_unix_ms {
                Some((RecoveryLadderSuppression::Cooldown, window.until_unix_ms))
            } else {
                window.running = true;
                window.until_unix_ms = now_unix_ms.saturating_add(pending.cooldown_ms);
                None
            }
        };
        match suppressed {
            Some((reason, until_unix_ms)) => self.append_recovery_ladder_fact(
                &pending,
                EventSeverity::Info,
                RuntimeLifecyclePhase::RecoveryLadderSuppressed {
                    reason,
                    until_unix_ms,
                },
            ),
            None => {
                lock(&self.pending_host_work, "admit_recovery_ladder")?
                    .push_back(PendingHostWork::RecoveryLadder(Box::new(pending)));
                Ok(())
            }
        }
    }

    /// Climbs one ladder on the scheduling thread and releases the instance's window
    /// whatever the outcome. Only a fatal failure is returned (the thread poisons the host).
    pub(super) fn run_recovery_ladder(
        &self,
        pending: &PendingRecoveryLadder,
    ) -> RuntimeHostResult<()> {
        let climbed = self.climb_recovery_ladder(pending);
        let released = lock(&self.recovery_ladders, "finish_recovery_ladder").map(|mut ladders| {
            if let Some(window) = ladders.get_mut(&pending.instance_id) {
                window.running = false;
            }
        });
        climbed.and(released)
    }

    fn climb_recovery_ladder(&self, pending: &PendingRecoveryLadder) -> RuntimeHostResult<()> {
        if !self.recovery_admitted(pending)? {
            return Ok(());
        }
        if let Some(trigger) = pending.trigger.preparation {
            let current = lock(&self.recovery_ladders, "gate_recovery_preparation")?
                .get(&pending.instance_id)
                .and_then(|window| window.preparation);
            if current != Some(trigger) {
                return self.append_recovery_ladder_fact(
                    pending,
                    EventSeverity::Info,
                    RuntimeLifecyclePhase::RecoveryLadderSuppressed {
                        reason: RecoveryLadderSuppression::PreparationSuperseded,
                        until_unix_ms: 0,
                    },
                );
            }
        }
        let resolved = lock(&self.registered_instances, "read_recovery_ladder_instance")?
            .get(&pending.instance_id)
            .cloned()
            .ok_or_else(|| ladder_invariant("recovery_ladder_instance_missing"))?;
        // A preparation has no failed task identity. Resolve its recovery binding from the
        // configured startup resource through the same hash/containment admission owner.
        // Each eventual run still admits its own material, so a mutable locator cannot carry
        // this identity read across a changed package.
        let mut effective = pending.clone();
        let mut recovery_resolution_error = None;
        if pending.trigger.preparation.is_some()
            && let Some(startup) = self.startup_packages()?.get(&pending.instance_id)
        {
            match super::contained_task::prepare_contained_task(
                &pending.instance_alias,
                startup,
                self.execution()?.vision_provider(),
                Instant::now() + Duration::from_millis(startup.response_deadline_ms()),
            ) {
                Ok(prepared) => {
                    if let Some(binding) = startup.recovery() {
                        effective.recovery = Some(binding.clone());
                    } else if let Some(binding) =
                        self.configured_return_home(prepared.game(), prepared.server())?
                    {
                        effective.recovery = Some(binding.clone());
                        effective.recovery_configured = Some(PackageIdentity::of(&prepared));
                    }
                }
                Err(failure) => {
                    self.append_lifecycle_failure(
                        RuntimeLifecycleFailureStage::OperationCleanup,
                        RuntimeLifecycleFailure::Host(&failure.error),
                        pending.links.clone(),
                        None,
                    )?;
                    if failure.poison_runtime || failure.error.is_fatal() {
                        return Err(*failure.error);
                    }
                    recovery_resolution_error = Some(failure.error.code());
                }
            }
        }
        let pending = &effective;
        let startup_package = self.startup_packages()?.contains_key(&pending.instance_id);
        let emulator_control = resolved
            .adb_endpoint
            .as_ref()
            .and_then(ResolvedInstanceEndpoint::discovered_binding)
            .is_some();
        let skipped = [
            (pending.recovery.is_none() && recovery_resolution_error.is_none())
                .then_some(RecoveryRungSkipReason::NoRecoveryPackage),
            (!startup_package).then_some(RecoveryRungSkipReason::NoStartupPackage),
            (!emulator_control).then_some(RecoveryRungSkipReason::NoEmulatorControl),
        ];
        let rungs = RecoveryRung::LADDER
            .into_iter()
            .zip(skipped)
            .map(|(rung, reason)| RecoveryRungPlan {
                rung,
                state: if reason.is_some() {
                    RecoveryRungState::Skipped
                } else {
                    RecoveryRungState::Pending
                },
                reason,
            })
            .collect();
        self.append_recovery_ladder_fact(
            pending,
            EventSeverity::Info,
            RuntimeLifecyclePhase::RecoveryLadderStarted {
                trigger: pending.trigger.clone(),
                rungs,
            },
        )?;
        let mut rungs_tried = 0_u8;
        for (rung, skipped) in RecoveryRung::LADDER.into_iter().zip(skipped) {
            if !self.recovery_admitted(pending)? {
                return Ok(());
            }
            if let Some(reason) = skipped {
                self.append_recovery_rung_finished(
                    pending,
                    rung,
                    RecoveryRungOutcome::Skipped,
                    None,
                    Some(reason.as_str()),
                    None,
                )?;
                continue;
            }
            rungs_tried += 1;
            let attempt = if self.fatal.is_shutdown_requested() {
                RungAttempt::Failed {
                    run_id: None,
                    reason: "recovery_ladder_shutdown_requested",
                }
            } else {
                match rung {
                    RecoveryRung::ReturnHome => match recovery_resolution_error {
                        Some(reason) => RungAttempt::Failed {
                            run_id: None,
                            reason,
                        },
                        None => self.recovery_return_home(pending)?,
                    },
                    RecoveryRung::ApplicationRestart => {
                        self.recovery_application_restart(pending, &resolved)?
                    }
                    RecoveryRung::EmulatorRestart => {
                        self.recovery_emulator_restart(pending, &resolved)?
                    }
                }
            };
            match attempt {
                RungAttempt::Recovered { run_id } => {
                    self.append_recovery_rung_finished(
                        pending,
                        rung,
                        RecoveryRungOutcome::Recovered,
                        Some(run_id),
                        None,
                        None,
                    )?;
                    return self.append_recovery_ladder_fact(
                        pending,
                        EventSeverity::Info,
                        RuntimeLifecyclePhase::RecoveryLadderFinished {
                            outcome: RecoveryLadderOutcome::Recovered,
                            rungs_tried,
                        },
                    );
                }
                RungAttempt::EnvironmentReady { event } => {
                    self.append_recovery_rung_finished(
                        pending,
                        rung,
                        RecoveryRungOutcome::EnvironmentReady,
                        None,
                        None,
                        Some(event),
                    )?;
                    return self.append_recovery_ladder_fact(
                        pending,
                        EventSeverity::Info,
                        RuntimeLifecyclePhase::RecoveryLadderFinished {
                            outcome: RecoveryLadderOutcome::EnvironmentReady,
                            rungs_tried,
                        },
                    );
                }
                RungAttempt::Skipped { reason } => {
                    rungs_tried -= 1;
                    self.append_recovery_rung_finished(
                        pending,
                        rung,
                        RecoveryRungOutcome::Skipped,
                        None,
                        Some(reason.as_str()),
                        None,
                    )?;
                }
                RungAttempt::Failed { run_id, reason } => self.append_recovery_rung_finished(
                    pending,
                    rung,
                    RecoveryRungOutcome::Failed,
                    run_id,
                    Some(reason),
                    None,
                )?,
            }
        }
        self.append_recovery_ladder_fact(
            pending,
            EventSeverity::Error,
            RuntimeLifecyclePhase::RecoveryLadderFinished {
                outcome: RecoveryLadderOutcome::Exhausted,
                rungs_tried,
            },
        )
    }

    /// R1: the failed run's bound recovery package (or the configured return-home package,
    /// Workflow #336 L2d) as a standalone contained task under the ladder's causation id.
    fn recovery_return_home(
        &self,
        pending: &PendingRecoveryLadder,
    ) -> RuntimeHostResult<RungAttempt> {
        let binding = pending
            .recovery
            .as_ref()
            .ok_or_else(|| ladder_invariant("recovery_ladder_recovery_package_missing"))?;
        // Workflow #336 L2d (R24): the configured package takes the longest response deadline,
        // as a startup package and a scheduled run do; a bound one keeps the default.
        let request = ContainedTaskRequest::new(binding.package_path(), binding.expected_sha256())
            .and_then(|request| {
                if pending.recovery_configured.is_some() {
                    request
                        .with_response_deadline_ms(ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS)
                } else {
                    Ok(request)
                }
            });
        let request = match request {
            Ok(request) => request,
            Err(error) => {
                return Ok(RungAttempt::Failed {
                    run_id: None,
                    reason: error.code(),
                });
            }
        };
        self.run_recovery_rung_package(
            &PendingStartupPackage {
                instance_id: pending.instance_id,
                instance_alias: pending.instance_alias.clone(),
                request,
                causation_id: pending.causation_id,
                control_request_id: pending.request_id,
                run: HostPackageRun::ReturnHome,
                recovery_rung: true,
                configured_return_home: pending.recovery_configured.clone().map(Box::new),
            },
            false,
        )
    }

    /// R2: restarts the instance's assigned application. Workflow #369-2: the entry checks the
    /// startup package's run would make come first (review M2), and a known unavailable channel
    /// or an ADB baseline that does not answer skips the rung without touching the game. Then
    /// the assigned application is force-stopped through the existing application lifecycle
    /// path (`stop_assigned_application`), and the startup package is scheduled
    /// (`startup_package_scheduled` under the ladder's links) and run, which launches the game
    /// and confirms its page. A failed stop fails the rung with its code; after the stop the
    /// rung no longer skips. A startup package that cannot be admitted, or whose prerequisite
    /// chain or startup compatibility refuses it, is run without the stop, so its refusal is
    /// recorded as before and the game is left alone.
    fn recovery_application_restart(
        &self,
        pending: &PendingRecoveryLadder,
        resolved: &RegisteredInstance,
    ) -> RuntimeHostResult<RungAttempt> {
        let request = self
            .startup_packages()?
            .get(&pending.instance_id)
            .cloned()
            .ok_or_else(|| ladder_invariant("recovery_ladder_startup_package_missing"))?;
        let stops = match self.recovery_restart_entry(pending, &request)? {
            RestartEntry::Skip(reason) => return Ok(RungAttempt::Skipped { reason }),
            RestartEntry::Stop => true,
            RestartEntry::RunOnly => false,
        };
        if stops && let Err(reason) = self.stop_assigned_application(pending, resolved)? {
            return Ok(RungAttempt::Failed {
                run_id: None,
                reason,
            });
        }
        let mut startup = self
            .prepare_startup_package(resolved, pending.links.clone(), pending.request_id)
            .map_err(|failure| *failure.error)?
            .ok_or_else(|| ladder_invariant("recovery_ladder_startup_package_missing"))?;
        startup.recovery_rung = true;
        self.run_recovery_rung_package(&startup, stops)
    }

    /// Workflow #369-2 (review M2): the checks the rung's startup package run makes before any
    /// lease (`run_startup_package`), made before the game is stopped: its admission, its
    /// prerequisite chain and its startup compatibility (no resource readings; review L-R5-1),
    /// a known unavailable capture or input channel its entry needs, and the ADB baseline the
    /// stop and the launch need (30 s, as the run waits). The run makes the admission
    /// refusals again and records them as before.
    fn recovery_restart_entry(
        &self,
        pending: &PendingRecoveryLadder,
        request: &ContainedTaskRequest,
    ) -> RuntimeHostResult<RestartEntry> {
        let material_deadline =
            Instant::now() + Duration::from_millis(request.response_deadline_ms());
        let prepared = match super::contained_task::prepare_contained_task(
            &pending.instance_alias,
            request,
            self.execution()?.vision_provider(),
            material_deadline,
        ) {
            Ok(prepared) => prepared,
            Err(failure) if failure.poison_runtime || failure.error.is_fatal() => {
                return Err(*failure.error);
            }
            Err(_) => return Ok(RestartEntry::RunOnly),
        };
        if let Err(failure) =
            self.resolve_prerequisite_chain(&pending.instance_alias, &prepared, || {
                Ok(material_deadline)
            })
        {
            if failure.poison_runtime || failure.error.is_fatal() {
                return Err(*failure.error);
            }
            return Ok(RestartEntry::RunOnly);
        }
        if prepared.startup_incompatibility().is_some() {
            return Ok(RestartEntry::RunOnly);
        }
        let (capture, input) = prepared.recovery_entry_channels();
        if let Some(reason) = self
            .recovery_entry_unavailable(pending.instance_id, capture, input)?
            .and_then(recovery_entry_skip)
        {
            return Ok(RestartEntry::Skip(reason));
        }
        if let Err(error) = self.execution()?.probe_adb_baseline_until(
            &pending.instance_alias,
            Instant::now() + Duration::from_secs(30),
            &|| self.fatal.is_shutdown_requested(),
        ) {
            if error.resource_quiescence() == Some(ResourceQuiescence::Unconfirmed) {
                return Err(RuntimeHostError::execution(LADDER_OPERATION, &error));
            }
            return Ok(RestartEntry::Skip(RecoveryRungSkipReason::AdbUnavailable));
        }
        Ok(RestartEntry::Stop)
    }

    /// Workflow #369-2: force-stops the instance's assigned application as a host-minted
    /// `ApplicationLifecycle { Stop }` request under the ladder's causation id, minted as a
    /// startup package run's request is, with its own lease on the startup package connection:
    /// `command.received` / `command.validated`, then the existing `application.intent` /
    /// `application.completed`, then the lease's release. A non-fatal failure is recorded as a
    /// runtime lifecycle failure and returned as the rung's reason.
    fn stop_assigned_application(
        &self,
        pending: &PendingRecoveryLadder,
        resolved: &RegisteredInstance,
    ) -> RuntimeHostResult<Result<(), &'static str>> {
        let action = actingcommand_contract::ApplicationLifecycleAction::Stop;
        let instance_alias = resolved.instance_alias.as_str();
        let issuer = self.events.issuer();
        let request_id = issuer
            .mint_request_id()
            .map_err(|_| runtime_identifier_error())?;
        let correlation_id = issuer
            .mint_correlation_id()
            .map_err(|_| runtime_identifier_error())?;
        let holder_id = *issuer
            .mint_holder_id()
            .map_err(|_| runtime_identifier_error())?
            .transport();
        let (actor, source) = scheduled_request_transport_origin(resolved.provenance());
        let message = RuntimeRequest::new(
            request_id,
            correlation_id,
            Some(pending.causation_id),
            actor,
            source,
            unix_ms_now()?,
            RuntimeOperation::ApplicationLifecycle {
                instance_alias: instance_alias.to_owned(),
                holder_id,
                action,
            },
        )
        .map_err(|_| ladder_invariant("recovery_application_stop_request_invalid"))?;
        let validated = message
            .validate()
            .map_err(|_| ladder_invariant("recovery_application_stop_request_invalid"))?;
        let connection_id =
            ConnectionId::new(STARTUP_PACKAGE_CONNECTION_VALUE).map_err(|error| {
                RuntimeHostError::scheduler("build_recovery_application_stop_connection", &error)
            })?;
        let links = self
            .events
            .request_links(&validated, Some(pending.instance_id), None, None);
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
                links.clone(),
                payload,
            )?;
        }
        let stopped = self
            .acquire_lease(RuntimeLeaseAcquisition {
                request: &validated,
                request_id: message.request_id(),
                instance_alias,
                holder_id,
                connection_id,
                run_links: None,
                lease_ttl_ms: None,
            })
            .and_then(|acquired| {
                let RuntimeResult::LeaseGranted { token } = acquired.result else {
                    return Err(RequestFailure::poison_without_terminal(ladder_invariant(
                        "recovery_application_stop_lease_result_invalid",
                    )));
                };
                match self
                    .application_control(&validated, &token, action, connection_id, None)
                    .and_then(|_| {
                        self.release_lease(
                            &validated,
                            message.request_id(),
                            &token,
                            connection_id,
                            None,
                        )
                    }) {
                    Ok(_) => Ok(()),
                    Err(failure) => {
                        Err(self.cleanup_composite_failure(token, connection_id, failure))
                    }
                }
            });
        match stopped {
            Ok(()) => Ok(Ok(())),
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
                Ok(Err(failure.error.code()))
            }
        }
    }

    /// R3: the existing provider confirms the old process gone before the current instance
    /// is started. One admission guard spans both actions and the first readiness attempt.
    /// Workflow #369-1: the rung then waits for a usable environment (`await_recovery_readiness`),
    /// records `recovery_environment_ready`, and runs the instance's startup package; only that
    /// run's success recovers the rung.
    fn recovery_emulator_restart(
        &self,
        pending: &PendingRecoveryLadder,
        resolved: &RegisteredInstance,
    ) -> RuntimeHostResult<RungAttempt> {
        let instance_guard = self
            .instance_guard(pending.instance_id)
            .map_err(|failure| *failure.error)?;
        let admission = lock(&instance_guard, "lock_instance_admission")?;
        let mut stopped = None;
        for action in [EmulatorInstanceAction::Stop, EmulatorInstanceAction::Start] {
            if !self.recovery_admitted(pending)? {
                return Ok(RungAttempt::Failed {
                    run_id: None,
                    reason: "recovery_admission_denied",
                });
            }
            self.append_event_raw(
                EventSeverity::Info,
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
                pending.links.clone(),
                CommandPayloadDraft::received(action.event_action(), AuditInput::new()),
            )?;
            // Read the registered binding again after Stop; Start discovers its new port.
            let current = self
                .resolve_instance(&resolved.instance_alias)
                .map_err(|failure| *failure.error)?;
            let driven = match self.drive_emulator_control_while_guarded(
                &current,
                pending.links.clone(),
                action,
                pending.request_id,
                &admission,
                true,
            ) {
                Ok(driven) => driven,
                Err(failure) if failure.poison_runtime || failure.error.is_fatal() => {
                    return Err(*failure.error);
                }
                Err(failure) => {
                    return Ok(RungAttempt::Failed {
                        run_id: None,
                        reason: failure.error.code(),
                    });
                }
            };
            if action == EmulatorInstanceAction::Stop {
                if driven.outcome.process_started {
                    return Ok(RungAttempt::Failed {
                        run_id: None,
                        reason: "recovery_stop_unconfirmed",
                    });
                }
                stopped = Some(driven.terminal);
                self.append_recovery_ladder_fact(
                    pending,
                    EventSeverity::Info,
                    RuntimeLifecyclePhase::RecoveryInstanceStopped {
                        stop: driven.terminal,
                    },
                )?;
                continue;
            }
            if !driven.outcome.running {
                return Ok(RungAttempt::Failed {
                    run_id: None,
                    reason: "recovery_environment_not_ready",
                });
            }
            let stop = stopped.ok_or_else(|| ladder_invariant("recovery_stop_missing"))?;
            let (current, preparation) = match self.await_recovery_readiness(
                pending,
                &instance_guard,
                admission,
                Instant::now() + RECOVERY_READINESS_WINDOW,
            )? {
                Ok(ready) => ready,
                Err(reason) => {
                    return Ok(RungAttempt::Failed {
                        run_id: None,
                        reason,
                    });
                }
            };
            let event = self.append_event_raw(
                EventSeverity::Info,
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
                pending.links.clone(),
                RuntimePayloadDraft::lifecycle_observed(
                    self.owner_epoch,
                    RuntimeLifecyclePhase::RecoveryEnvironmentReady {
                        stop,
                        start: driven.terminal,
                        preparation,
                    },
                    AuditInput::new(),
                ),
            )?;
            let startup = self
                .prepare_startup_package(&current, pending.links.clone(), pending.request_id)
                .map_err(|failure| *failure.error)?;
            return match startup {
                Some(mut startup) => {
                    startup.recovery_rung = true;
                    self.run_recovery_rung_package(&startup, true)
                }
                None => Ok(RungAttempt::EnvironmentReady {
                    event: terminal(&event),
                }),
            };
        }
        Err(ladder_invariant("recovery_start_missing"))
    }

    /// Workflow #369-1: after a confirmed Start, waits until Android reports a resumed activity
    /// (the read-only boot check, review M5) and a fresh preparation passes (`capture.ok &&
    /// touch.ok`, no failure code), within `deadline`. The admission guard Start held covers the
    /// first attempt; later attempts take it again, and it is released while waiting, so the
    /// policy thread is never blocked by the wait (a failed preparation already withholds the
    /// instance from policy). The wait runs on the host-work thread, so other instances'
    /// queued startup packages and ladders wait behind it, for at most the window (review L1).
    /// A failed preparation is retried only when the existing rule calls it recoverable or the
    /// ADB baseline does not answer (review M3); every retry first polls the ADB baseline until
    /// it answers (bounded by `deadline`), then waits 5 s, 10 s, then 20 s, never past
    /// `deadline`. Each attempt is rechecked for admission (pause, shutdown, capacity). Returns
    /// the current binding and the passing preparation event, or the rung's failure reason:
    /// `recovery_android_not_booted` when the window ends before the boot check ever passed
    /// (so no preparation ran, review L-R4-2), otherwise `recovery_environment_not_ready`.
    fn await_recovery_readiness<'guard>(
        &self,
        pending: &PendingRecoveryLadder,
        instance_guard: &'guard Mutex<()>,
        held: MutexGuard<'guard, ()>,
        deadline: Instant,
    ) -> RuntimeHostResult<Result<(RegisteredInstance, TerminalEvent), &'static str>> {
        let alias = pending.instance_alias.as_str();
        let mut held = Some(held);
        let mut retries = 0_usize;
        let mut prepared = false;
        loop {
            if !self.recovery_admitted(pending)? {
                return Ok(Err("recovery_admission_denied"));
            }
            if self.recovery_android_booted(pending)? {
                prepared = true;
                let admission = match held.take() {
                    Some(admission) => admission,
                    None => lock(instance_guard, "lock_instance_admission")?,
                };
                let current = self
                    .resolve_instance(alias)
                    .map_err(|failure| *failure.error)?;
                if !current.device_self_checked() {
                    return Ok(Err("recovery_preparation_unconfirmed"));
                }
                let (check, preparation, recoverable) =
                    self.prepare_recovery_connection(&current, pending.links.clone(), &admission)?;
                if check.capture.ok && check.touch.ok && check.failure_code.is_none() {
                    return Ok(match preparation {
                        Some(preparation) => Ok((current, preparation)),
                        None => Err("recovery_preparation_unconfirmed"),
                    });
                }
                drop(admission);
                if !recoverable && !self.recovery_adb_unanswered(pending)? {
                    return Ok(Err("recovery_environment_not_ready"));
                }
            }
            drop(held.take());
            let wait =
                RECOVERY_READINESS_BACKOFF[retries.min(RECOVERY_READINESS_BACKOFF.len() - 1)];
            let not_ready = if prepared {
                "recovery_environment_not_ready"
            } else {
                "recovery_android_not_booted"
            };
            if let Some(reason) = self.await_recovery_retry(alias, wait, deadline, not_ready)? {
                return Ok(Err(reason));
            }
            retries += 1;
        }
    }

    /// Workflow #369-1 (review M5): Android has booted once it reports a resumed activity
    /// (read-only, through the ADB baseline; no session is opened). An ADB failure is a "not
    /// yet"; one whose child or pipe cleanup is unconfirmed poisons the host, as the
    /// foreground gate does.
    fn recovery_android_booted(&self, pending: &PendingRecoveryLadder) -> RuntimeHostResult<bool> {
        match self
            .execution()?
            .observe_foreground_application(&pending.instance_alias)
        {
            Ok(observation) => Ok(observation.foreground.is_some()),
            Err(error) => {
                if error.resource_quiescence() == Some(DeviceResourceQuiescence::Unconfirmed) {
                    return Err(recovery_adb_fault(pending, &error));
                }
                Ok(false)
            }
        }
    }

    /// Workflow #369-1 (review M3): whether the ADB baseline fails to answer one probe now.
    fn recovery_adb_unanswered(&self, pending: &PendingRecoveryLadder) -> RuntimeHostResult<bool> {
        match self
            .execution()?
            .probe_adb_baseline(&pending.instance_alias)
        {
            Ok(()) => Ok(false),
            Err(error) => {
                if error.resource_quiescence() == Some(DeviceResourceQuiescence::Unconfirmed) {
                    return Err(recovery_adb_fault(pending, &error));
                }
                Ok(true)
            }
        }
    }

    /// Before the next readiness attempt: polls the ADB baseline every 500 ms until it answers
    /// again, as `await_adb_baseline` does (review M-R4-1), then waits `wait`. Both are bounded
    /// by `deadline` and check shutdown and install drain at every poll (review L1).
    /// `Some(reason)` ends the rung: shutdown, drain, or no time left in the window
    /// (`not_ready`).
    fn await_recovery_retry(
        &self,
        alias: &str,
        wait: Duration,
        deadline: Instant,
        not_ready: &'static str,
    ) -> RuntimeHostResult<Option<&'static str>> {
        let stopped = || self.fatal.is_shutdown_requested();
        loop {
            if let Some(reason) = self.recovery_wait_interrupted()? {
                return Ok(Some(reason));
            }
            if Instant::now() >= deadline {
                return Ok(Some(not_ready));
            }
            match self
                .execution()?
                .probe_adb_baseline_until(alias, deadline, &stopped)
            {
                Ok(()) => break,
                Err(error) => {
                    if error.resource_quiescence() == Some(ResourceQuiescence::Unconfirmed) {
                        return Err(RuntimeHostError::execution(LADDER_OPERATION, &error));
                    }
                    thread::sleep(
                        RECOVERY_READINESS_POLL
                            .min(deadline.saturating_duration_since(Instant::now())),
                    );
                }
            }
        }
        let until = Instant::now() + wait;
        if until >= deadline {
            return Ok(Some(not_ready));
        }
        loop {
            if let Some(reason) = self.recovery_wait_interrupted()? {
                return Ok(Some(reason));
            }
            let now = Instant::now();
            if now >= until {
                return Ok(None);
            }
            thread::sleep(RECOVERY_READINESS_POLL.min(until - now));
        }
    }

    /// A readiness wait ends at once on shutdown or an install drain (review L1).
    fn recovery_wait_interrupted(&self) -> RuntimeHostResult<Option<&'static str>> {
        if self.fatal.is_shutdown_requested() {
            return Ok(Some("recovery_ladder_shutdown_requested"));
        }
        if self.lifecycle_draining()? {
            return Ok(Some("recovery_ladder_drain_requested"));
        }
        Ok(None)
    }

    /// Runs a rung's package; it recovers when the run completes with `success` (its target
    /// page reached). A failure was already recorded by the runner. After a destructive action
    /// (the emulator restart, Workflow #369-1; the application stop, Workflow #369-2) an
    /// unavailable entry channel or ADB fails the rung instead of skipping it, so `rungs_tried`
    /// counts the action that ran.
    fn run_recovery_rung_package(
        &self,
        run: &PendingStartupPackage,
        destructive: bool,
    ) -> RuntimeHostResult<RungAttempt> {
        Ok(match self.run_pending_startup_package(run)? {
            Ok(OperationSuccess {
                result:
                    RuntimeResult::ContainedTaskCompleted {
                        run_id, outcome, ..
                    },
                ..
            }) => {
                if outcome == TaskOutcome::Success {
                    RungAttempt::Recovered { run_id }
                } else {
                    RungAttempt::Failed {
                        run_id: Some(run_id),
                        reason: "recovery_rung_target_not_reached",
                    }
                }
            }
            Ok(_) => return Err(ladder_invariant("recovery_rung_result_invalid")),
            Err(code) => match recovery_entry_skip(code) {
                Some(reason) if !destructive => RungAttempt::Skipped { reason },
                _ => RungAttempt::Failed {
                    run_id: None,
                    reason: code,
                },
            },
        })
    }

    fn append_recovery_rung_finished(
        &self,
        pending: &PendingRecoveryLadder,
        rung: RecoveryRung,
        outcome: RecoveryRungOutcome,
        run_id: Option<RunId>,
        reason: Option<&'static str>,
        environment: Option<TerminalEvent>,
    ) -> RuntimeHostResult<()> {
        self.append_recovery_ladder_fact(
            pending,
            if outcome == RecoveryRungOutcome::Failed {
                EventSeverity::Warning
            } else {
                EventSeverity::Info
            },
            RuntimeLifecyclePhase::RecoveryRungFinished {
                rung,
                outcome,
                run_id,
                reason: reason.map(str::to_owned),
                environment,
            },
        )
    }

    fn append_recovery_ladder_fact(
        &self,
        pending: &PendingRecoveryLadder,
        severity: EventSeverity,
        phase: RuntimeLifecyclePhase,
    ) -> RuntimeHostResult<()> {
        self.append_event_raw(
            severity,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            pending.links.clone(),
            RuntimePayloadDraft::lifecycle_observed(self.owner_epoch, phase, AuditInput::new()),
        )
        .map(|_| ())
    }
}

/// The skip a rung package's entry refusal stands for: a known unavailable capture or input
/// channel, or an ADB baseline that does not answer.
fn recovery_entry_skip(code: &str) -> Option<RecoveryRungSkipReason> {
    match code {
        "recovery_capture_unavailable" => Some(RecoveryRungSkipReason::CaptureUnavailable),
        "recovery_input_unavailable" => Some(RecoveryRungSkipReason::InputUnavailable),
        "startup_package_adb_not_ready" | "recovery_ladder_adb_not_ready" => {
            Some(RecoveryRungSkipReason::AdbUnavailable)
        }
        _ => None,
    }
}

/// Workflow #369-1: an ADB child or pipe cleanup left unconfirmed during a readiness check is
/// a host fault (as in the foreground gate), not a "not yet".
fn recovery_adb_fault(
    pending: &PendingRecoveryLadder,
    error: &actingcommand_device::DeviceError,
) -> RuntimeHostError {
    let mut fatal = RuntimeHostError::fatal(
        "recovery_readiness_adb_unconfirmed",
        LADDER_OPERATION,
        RuntimeErrorCode::RuntimeFatal,
    )
    .with_native_detail(format!(
        "instance_alias={}; adb_failed={error}",
        pending.instance_alias
    ));
    fatal.lifecycle.instance_id = Some(pending.instance_id);
    fatal.lifecycle.resource_quiescence = Some(ResourceQuiescence::Unconfirmed);
    fatal
}
