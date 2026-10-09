// SPDX-License-Identifier: AGPL-3.0-only

//! The stuck-recovery ladder (Runtime slice #316-B4; Workflow #369 H-1, H-3).
//!
//! A scheduled contained task run on a physical instance that commits `task.failed` with
//! `contained_task_page_unknown`, a `contained_task_recovery_*` or a
//! `contained_task_home_recovery_*` code starts a ladder for its instance, unless the
//! instance's `stuck_recovery` is off. A direct task run (CLI or console task-run, MCP
//! `ac_run_pack`) never starts one (Workflow #369-3, coordinator rulings Q1 and P1: the ladder
//! exists for the routine). A failed daemon-start or connection preparation (emulator start or
//! restart, resume, self-check) still starts one.
//!
//! Workflow #369 H-1: a scheduled run's ladder is decided inside the run's failed lease end
//! (`cleanup_scheduled_failure_with_run_links`, to which only the policy-run path passes a
//! `LadderTrigger`). An admitted ladder turns that end into a transfer of the key to the
//! ladder's claim, so nothing can take the instance between the failure and the climb. A
//! preparation's ladder is queued as a claim after the preparation released its lease. Either
//! way the ladder climbs on its instance's worker (`host_claims`) and holds the key from its
//! first rung to its end (H-3): rung runs run on its token, each after the ladder queued its
//! own continuation, to which the run's release hands the key back; the ladder renews before
//! every step whose bound exceeds what is left; at every step it checks that it still holds
//! the key and that no pause, capacity refusal, drain or shutdown ends the climb; and it
//! releases once.
//!
//! The rungs, in this fixed order, are existing work under the instance's key:
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
//! One ladder per instance per cool-down window: a trigger while a ladder holds or waits for
//! the instance (read from the scheduler), or inside the window the last ladder that got the
//! key opened, records `recovery_ladder_suppressed` and nothing else. Every fact is a
//! `runtime.lifecycle_observed` linked to the instance and the trigger run's correlation id,
//! under the ladder's own request and causation ids; the `return_home` run carries that
//! causation id, the startup package runs the one of their scheduling event.

use super::contained_task::{FailedTaskTerminal, PackageIdentity};
use super::host_claims::{HostClaimWork, HostKey};
use super::lease::{ClaimGrantSlot, HostClaim, HostClaimAdmission};
use super::startup_package::{HostPackageRun, PendingStartupPackage};
use super::*;
use crate::codes::HostCode;
use actingcommand_contract::{
    BackendObservationStatus, BackendOpenEntry, ContainedTaskRecoveryBinding,
    EmulatorInstanceAction, HolderId, InstanceStuckRecovery, IssuedCausationId,
    IssuedCorrelationId, IssuedRequestId, LeasePriority, RecoveryLadderOutcome,
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
/// Workflow #369-1 (review L1): how often a readiness wait checks shutdown, install drain and
/// (Workflow #369 H-3, review2 L7) the scheduling pause.
const RECOVERY_READINESS_POLL: Duration = Duration::from_millis(500);
/// Workflow #369 Q-5 (B1): a hold renews to a step's bound plus this margin.
const HOLD_MARGIN_MS: u64 = 5_000;
/// Workflow #369 H-1, H-3: the TTL a ladder claim, and each continuation, is granted with; the
/// ladder renews before every longer step.
pub(super) const LADDER_CLAIM_TTL_MS: u64 = 120_000;
/// Rung 2's bound before its startup run: the entry's ADB probe (30 s) and the application
/// stop (60 s, an ADB application command).
const APPLICATION_RESTART_ENTRY_BOUND_MS: u64 = 90_000;
/// Rung 3's Stop: the provider's control bound, 60 + 120 + 10 s (`runtime-client`'s emulator
/// control receipt wait); review L-3.
const EMULATOR_STOP_BOUND_MS: u64 = 190_000;
/// Rung 3's Start: the Stop bound plus the ADB baseline (30 s).
const EMULATOR_START_BOUND_MS: u64 = 220_000;

/// One admitted ladder: its trigger, the facts' links and its claim.
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
    /// The ladder's request id: task timing admission id of its rung runs, and its claim's.
    request_id: RequestId,
    correlation_id: IssuedCorrelationId,
    causation_id: IssuedCausationId,
    cooldown_ms: u64,
    /// Workflow #369 H-1: the ladder claim's own synthetic request (`AcquireLease` of the
    /// ladder's holder), whose links carry the claim's records; its key keeps it.
    claim: RuntimeRequest,
    holder_id: HolderId,
}

impl PendingRecoveryLadder {
    pub(super) const fn claim(&self) -> &RuntimeRequest {
        &self.claim
    }

    pub(super) const fn holder_id(&self) -> HolderId {
        self.holder_id
    }

    pub(super) const fn links(&self) -> &EventLinksDraft {
        &self.links
    }
}

/// Per instance: where the cool-down window the last ladder that got the key opened ends, and
/// the identity of the latest preparation attempt (`PreparationSuperseded`). Whether a ladder
/// holds or waits is the scheduler's (Workflow #369 §3.5).
#[derive(Default)]
pub(super) struct RecoveryLadderWindow {
    until_unix_ms: u64,
    pub(super) preparation: Option<TerminalEvent>,
}

/// Workflow #369 H-1 (review M-1): what only the policy-run path notes on its run's control, so
/// that its failed lease end, and no other, can hand the key to a ladder.
pub(super) struct LadderSource {
    pub(super) correlation_id: IssuedCorrelationId,
    pub(super) recovery: Option<ContainedTaskRecoveryBinding>,
}

/// Workflow #369 H-1: the trigger a scheduled policy run's failed lease end passes to
/// `cleanup_scheduled_failure_with_run_links`: the run's committed `task.failed`, its
/// correlation, its bound recovery package and its package's identity.
pub(super) struct LadderTrigger {
    terminal: FailedTaskTerminal,
    correlation_id: IssuedCorrelationId,
    recovery: Option<ContainedTaskRecoveryBinding>,
    package: Option<PackageIdentity>,
}

impl LadderTrigger {
    pub(super) fn new(
        terminal: FailedTaskTerminal,
        source: &LadderSource,
        package: Option<PackageIdentity>,
    ) -> Self {
        Self {
            terminal,
            correlation_id: source.correlation_id,
            recovery: source.recovery.clone(),
            package,
        }
    }
}

/// Workflow #369 H-3: the ladder's continuation, queued before a rung run so that the run's
/// release hands the key back to the ladder.
pub(super) struct LadderContinuation {
    request_id: RequestId,
    grant: Arc<ClaimGrantSlot>,
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
        let issuer = self.events.issuer();
        let request_id = issuer
            .mint_request_id()
            .map_err(|_| runtime_identifier_error())?;
        let correlation_id = issuer
            .mint_correlation_id()
            .map_err(|_| runtime_identifier_error())?;
        let causation_id = issuer
            .mint_causation_id()
            .map_err(|_| runtime_identifier_error())?;
        let holder_id = *issuer
            .mint_holder_id()
            .map_err(|_| runtime_identifier_error())?
            .transport();
        let claim = self.ladder_claim_request(
            &resolved,
            request_id,
            correlation_id,
            causation_id,
            holder_id,
        )?;
        // Workflow #369 H-1: a preparation's ladder waits as a claim; the preparation released
        // its lease before this record, so the pump grants it on the free instance.
        self.queue_recovery_ladder(PendingRecoveryLadder {
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
            correlation_id,
            causation_id,
            cooldown_ms: u64::from(settings.cooldown_secs) * 1_000,
            claim,
            holder_id,
        })?;
        Ok(event)
    }

    /// Workflow #369 H-1: the ladder claim's own synthetic request.
    fn ladder_claim_request(
        &self,
        resolved: &RegisteredInstance,
        request_id: IssuedRequestId,
        correlation_id: IssuedCorrelationId,
        causation_id: IssuedCausationId,
        holder_id: HolderId,
    ) -> RuntimeHostResult<RuntimeRequest> {
        let (actor, source) = scheduled_request_transport_origin(resolved.provenance());
        RuntimeRequest::new(
            request_id,
            correlation_id,
            Some(causation_id),
            actor,
            source,
            unix_ms_now()?,
            RuntimeOperation::AcquireLease {
                instance_alias: resolved.instance_alias.clone(),
                holder_id,
            },
        )
        .map_err(|_| ladder_invariant("recovery_ladder_claim_request_invalid"))
    }

    /// The two parts of a ladder's admission that can change while it holds: the scheduling
    /// pause and business capacity. Nothing is recorded.
    fn recovery_gate_open(&self, pending: &PendingRecoveryLadder) -> RuntimeHostResult<bool> {
        let paused = lock(&self.scheduling_pause, "gate_recovery_pause")?
            .deferral(&pending.instance_alias)
            .is_some();
        if paused || self.fatal.is_shutdown_requested() {
            return Ok(false);
        }
        match self.admit_capacity() {
            Ok(_) => Ok(true),
            Err(error) if error.is_fatal() => Err(error),
            Err(_) => Ok(false),
        }
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

    /// Workflow #369 H-1: the trigger of the scheduled run whose task request is `request_id`,
    /// when that run is a policy run (only its path notes a ladder source) that committed
    /// `task.failed`.
    pub(super) fn ladder_trigger_for(
        &self,
        request_id: RequestId,
    ) -> RuntimeHostResult<Option<LadderTrigger>> {
        let control = lock(&self.contained_runs, "read_recovery_ladder_trigger")?
            .get(&request_id)
            .cloned();
        match control {
            Some(control) => control.ladder_trigger(),
            None => Ok(None),
        }
    }

    /// Workflow #369 H-1: decides, inside a scheduled run's failed lease end, whether its
    /// trigger starts a ladder: a trigger code, a physical instance, `stuck_recovery` on, then
    /// the admission (`recovery_admitted`, and no ladder holding or waiting for the instance,
    /// outside the cool-down window). Returns the admitted ladder.
    pub(super) fn ladder_at_lease_end(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        resolved: &RegisteredInstance,
        trigger: LadderTrigger,
    ) -> RuntimeHostResult<Option<PendingRecoveryLadder>> {
        let instance_id = resolved.instance_id();
        let settings = self
            .stuck_recovery()?
            .get(&instance_id)
            .copied()
            .unwrap_or_default();
        if !is_stuck_recovery_trigger(trigger.terminal.failure_code)
            || resolved.provenance() != ExecutionBackendProvenance::PhysicalDevice
            || !settings.enabled
        {
            return Ok(None);
        }
        // Workflow #336 L2d (R23): a run that binds no recovery package takes the return-home
        // package actingd configures for its package's game and server.
        let (recovery, recovery_configured) = match trigger.recovery {
            Some(binding) => (Some(binding), None),
            None => {
                let failed = trigger
                    .package
                    .ok_or_else(|| ladder_invariant("recovery_ladder_package_missing"))?;
                match self.configured_return_home(&failed.game, &failed.server)? {
                    Some(binding) => (Some(binding.clone()), Some(failed)),
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
        let holder_id = *issuer
            .mint_holder_id()
            .map_err(|_| runtime_identifier_error())?
            .transport();
        let claim = self.ladder_claim_request(
            resolved,
            request_id,
            trigger.correlation_id,
            causation_id,
            holder_id,
        )?;
        let pending = PendingRecoveryLadder {
            instance_id,
            instance_alias: resolved.instance_alias.clone(),
            trigger: RecoveryLadderTrigger {
                stage: RecoveryTriggerStage::Task,
                run_id: Some(*trigger.terminal.run_id.transport()),
                task_id: Some(*trigger.terminal.task_id.transport()),
                preparation: None,
                failure_code: trigger.terminal.failure_code.to_owned(),
            },
            recovery,
            recovery_configured,
            links: request
                .event_links(Some(instance_id), None, None)
                .with_request_id(request_id)
                .with_causation_id(causation_id),
            request_id: *request_id.transport(),
            correlation_id: trigger.correlation_id,
            causation_id,
            cooldown_ms: u64::from(settings.cooldown_secs) * 1_000,
            claim,
            holder_id,
        };
        Ok(self.admit_recovery_ladder(&pending)?.then_some(pending))
    }

    /// Admits a ladder, or records why it is suppressed: a ladder already holds or waits for
    /// the instance (Workflow #369 §5.3, read from the scheduler), or the cool-down window the
    /// last ladder that got the key opened.
    fn admit_recovery_ladder(&self, pending: &PendingRecoveryLadder) -> RuntimeHostResult<bool> {
        if !self.recovery_admitted(pending)? {
            return Ok(false);
        }
        let now_unix_ms = self.clock.sample()?.unix_ms;
        let ladder_active =
            lock(&self.scheduler, "read_recovery_ladder_hold")?.ladder_active(pending.instance_id);
        let until_unix_ms = lock(&self.recovery_ladders, "admit_recovery_ladder")?
            .get(&pending.instance_id)
            .map_or(0, |window| window.until_unix_ms);
        let suppressed = if ladder_active {
            Some(RecoveryLadderSuppression::AlreadyRunning)
        } else if now_unix_ms < until_unix_ms {
            Some(RecoveryLadderSuppression::Cooldown)
        } else {
            None
        };
        let Some(reason) = suppressed else {
            return Ok(true);
        };
        self.append_recovery_ladder_fact(
            pending,
            EventSeverity::Info,
            RuntimeLifecyclePhase::RecoveryLadderSuppressed {
                reason,
                until_unix_ms,
            },
        )?;
        Ok(false)
    }

    /// Admits a preparation's ladder and queues its claim.
    fn queue_recovery_ladder(&self, pending: PendingRecoveryLadder) -> RuntimeHostResult<()> {
        if !self.admit_recovery_ladder(&pending)? {
            return Ok(());
        }
        self.enqueue_ladder_claim(pending)
    }

    /// Workflow #369 H-1: queues an admitted ladder's claim (routine, high, no deadline); the
    /// pump grants it, and its instance's worker climbs it. A refusal to queue is recorded.
    pub(super) fn enqueue_ladder_claim(
        &self,
        pending: PendingRecoveryLadder,
    ) -> RuntimeHostResult<()> {
        let connection_id = ConnectionId::new(STARTUP_PACKAGE_CONNECTION_VALUE)
            .map_err(|error| RuntimeHostError::scheduler("build_ladder_connection", &error))?;
        let claim = pending.claim.clone();
        let links = pending.links.clone();
        let instance_alias = pending.instance_alias.clone();
        let holder_id = pending.holder_id;
        match self.request_host_claim_work(
            HostClaim {
                request: &claim,
                instance_alias: &instance_alias,
                holder_id,
                connection_id,
                kind: ClaimKind::Ladder,
                priority: LeasePriority::High,
                lease_ttl_ms: LADDER_CLAIM_TTL_MS,
            },
            HostClaimWork::RecoveryLadder(Box::new(pending)),
        ) {
            Ok(()) => Ok(()),
            Err(failure) if failure.poison_runtime || failure.error.is_fatal() => {
                Err(*failure.error)
            }
            Err(failure) => self.append_lifecycle_failure(
                RuntimeLifecycleFailureStage::OperationCleanup,
                RuntimeLifecycleFailure::Host(&failure.error),
                links,
                None,
            ),
        }
    }

    /// Climbs one ladder on its instance's worker, holding `key`, and releases the key once
    /// whatever the outcome. Only a fatal failure is returned.
    pub(super) fn run_recovery_ladder(
        &self,
        pending: &PendingRecoveryLadder,
        mut key: HostKey,
    ) -> RuntimeHostResult<()> {
        let climbed = self.climb_recovery_ladder(pending, &mut key);
        let released = self.release_host_key(&key);
        climbed.and(released)
    }

    fn climb_recovery_ladder(
        &self,
        pending: &PendingRecoveryLadder,
        key: &mut HostKey,
    ) -> RuntimeHostResult<()> {
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
        // Workflow #369 §5.3: only a ladder that got the key opens its cool-down window.
        let now_unix_ms = self.clock.sample()?.unix_ms;
        lock(&self.recovery_ladders, "open_recovery_ladder_window")?
            .entry(pending.instance_id)
            .or_default()
            .until_unix_ms = now_unix_ms.saturating_add(pending.cooldown_ms);
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
        // Review L6 (#666): the holding interruption that ended the last rung that ran, if one
        // did; a rung that ran and failed on its own clears it.
        let mut last_interruption = None;
        for (rung, skipped) in RecoveryRung::LADDER.into_iter().zip(skipped) {
            // Workflow #369 H-3: the holding check before every rung (so after every rung run
            // but the last one, whose own failure decides the outcome; review L6).
            if let Some(reason) = self.ladder_hold_interrupted(pending, key)? {
                return self.finish_interrupted_ladder(pending, Some(rung), reason, rungs_tried);
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
            let attempt = match rung {
                RecoveryRung::ReturnHome => match recovery_resolution_error {
                    Some(reason) => RungAttempt::Failed {
                        run_id: None,
                        reason,
                    },
                    None => self.recovery_return_home(pending, key)?,
                },
                RecoveryRung::ApplicationRestart => {
                    self.recovery_application_restart(pending, &resolved, key)?
                }
                RecoveryRung::EmulatorRestart => {
                    self.recovery_emulator_restart(pending, &resolved, key)?
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
                RungAttempt::Failed { run_id, reason } => {
                    last_interruption = is_hold_interruption(reason).then_some(reason);
                    self.append_recovery_rung_finished(
                        pending,
                        rung,
                        RecoveryRungOutcome::Failed,
                        run_id,
                        Some(reason),
                        None,
                    )?;
                }
            }
        }
        // Review L6 (#666), §5.10: after the last rung the climb is exhausted at Error when the
        // rungs that ran failed on their own, whatever holds at this moment; it ends at Warning
        // only when the last rung that ran was itself ended by a holding interruption.
        if let Some(reason) = last_interruption {
            return self.finish_interrupted_ladder(pending, None, reason, rungs_tried);
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

    /// Workflow #369 H-3 (ruling Q8): the holding check. With shutdown requested the reason is
    /// `recovery_ladder_shutdown_requested`; a ladder that no longer holds its key (in practice
    /// only shutdown) has lost it (`recovery_ladder_key_lost`; review L3); while holding, an
    /// install drain is `recovery_ladder_drain_requested`, and a scheduling pause or a capacity
    /// refusal (the two parts of `recovery_admitted`) is `recovery_admission_denied`.
    fn ladder_hold_interrupted(
        &self,
        pending: &PendingRecoveryLadder,
        key: &HostKey,
    ) -> RuntimeHostResult<Option<&'static str>> {
        if self.fatal.is_shutdown_requested() {
            return Ok(Some("recovery_ladder_shutdown_requested"));
        }
        if !self.holds_key(key)? {
            return Ok(Some(HostCode::RecoveryLadderKeyLost.as_str()));
        }
        if self.lifecycle_draining()? {
            return Ok(Some("recovery_ladder_drain_requested"));
        }
        if !self.recovery_gate_open(pending)? {
            return Ok(Some("recovery_admission_denied"));
        }
        Ok(None)
    }

    /// Workflow #369 H-3 (ruling Q8; review L-7): an interrupted climb records the current rung
    /// (if one was about to start) `failed` with the interruption, then
    /// `recovery_ladder_finished` `exhausted` at Warning: no error point. The key is released
    /// once by the caller.
    fn finish_interrupted_ladder(
        &self,
        pending: &PendingRecoveryLadder,
        rung: Option<RecoveryRung>,
        reason: &'static str,
        rungs_tried: u8,
    ) -> RuntimeHostResult<()> {
        if let Some(rung) = rung {
            self.append_recovery_rung_finished(
                pending,
                rung,
                RecoveryRungOutcome::Failed,
                None,
                Some(reason),
                None,
            )?;
        }
        self.append_recovery_ladder_fact(
            pending,
            EventSeverity::Warning,
            RuntimeLifecyclePhase::RecoveryLadderFinished {
                outcome: RecoveryLadderOutcome::Exhausted,
                rungs_tried,
            },
        )
    }

    /// Q-5: a renewal the ladder could not make fails the step with the renewal's own refusal
    /// (`lease_expired`, `lease_missing`); the next holding check ends the climb.
    fn ladder_step_renewal(
        &self,
        key: &mut HostKey,
        lease_ttl_ms: u64,
    ) -> RuntimeHostResult<Option<RungAttempt>> {
        match self.renew_host_key(key, lease_ttl_ms) {
            Ok(()) => Ok(None),
            Err(failure) if failure.poison_runtime || failure.error.is_fatal() => {
                Err(*failure.error)
            }
            Err(failure) => Ok(Some(RungAttempt::Failed {
                run_id: None,
                reason: failure.error.code(),
            })),
        }
    }

    /// R1: the failed run's bound recovery package (or the configured return-home package,
    /// Workflow #336 L2d) as a standalone contained task under the ladder's causation id.
    fn recovery_return_home(
        &self,
        pending: &PendingRecoveryLadder,
        key: &mut HostKey,
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
            pending,
            key,
        )
    }

    /// R2: restarts the instance's assigned application. Workflow #369-2: the entry checks the
    /// startup package's run would make come first (review M2), and a known unavailable channel
    /// or an ADB baseline that does not answer skips the rung without touching the game. Then
    /// the assigned application is force-stopped on the ladder's key
    /// (`stop_assigned_application`), and the startup package is scheduled
    /// (`startup_package_scheduled` under the ladder's links) and run, which launches the game
    /// and confirms its page. A failed stop fails the rung with its code; after the stop the
    /// rung no longer skips. A startup package that cannot be admitted, or whose prerequisite
    /// chain or startup compatibility refuses it, is run without the stop, so its refusal is
    /// recorded as before and the game is left alone.
    fn recovery_application_restart(
        &self,
        pending: &PendingRecoveryLadder,
        resolved: &RegisteredInstance,
        key: &mut HostKey,
    ) -> RuntimeHostResult<RungAttempt> {
        let request = self
            .startup_packages()?
            .get(&pending.instance_id)
            .cloned()
            .ok_or_else(|| ladder_invariant("recovery_ladder_startup_package_missing"))?;
        // Workflow #369 Q-5 (B1): the entry's ADB probe and the stop run on the key.
        if let Some(failed) = self.ladder_step_renewal(
            key,
            APPLICATION_RESTART_ENTRY_BOUND_MS.saturating_add(HOLD_MARGIN_MS),
        )? {
            return Ok(failed);
        }
        let stops = match self.recovery_restart_entry(pending, &request)? {
            RestartEntry::Skip(reason) => return Ok(RungAttempt::Skipped { reason }),
            RestartEntry::Stop => true,
            RestartEntry::RunOnly => false,
        };
        if stops && let Err(reason) = self.stop_assigned_application(pending, resolved, key)? {
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
        self.run_recovery_rung_package(&startup, stops, pending, key)
    }

    /// Workflow #369-2 (review M2): the checks the rung's startup package run makes before its
    /// run (`prepare_package_run`), made before the game is stopped: its admission, its
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
    /// `ApplicationLifecycle { Stop }` request under the ladder's causation id:
    /// `command.received` / `command.validated`, then the existing `application.intent` /
    /// `application.completed`. Workflow #369 H-3: it runs on the ladder's key (its holder and
    /// its connection), with no lease of its own, and leaves the key with the ladder. A
    /// non-fatal failure is recorded as a runtime lifecycle failure and returned as the rung's
    /// reason.
    fn stop_assigned_application(
        &self,
        pending: &PendingRecoveryLadder,
        resolved: &RegisteredInstance,
        key: &HostKey,
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
                holder_id: key.token.holder_id(),
                action,
            },
        )
        .map_err(|_| ladder_invariant("recovery_application_stop_request_invalid"))?;
        let validated = message
            .validate()
            .map_err(|_| ladder_invariant("recovery_application_stop_request_invalid"))?;
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
        match self.application_control(&validated, &key.token, action, key.connection_id, None) {
            Ok(_) => Ok(Ok(())),
            Err(failure) => {
                // An unconfirmed close is retained and marks the Runtime fatal, as on every
                // failed device path; the key itself stays with the ladder.
                self.retain_unconfirmed_resources(&failure.error, EventLinksDraft::default())?;
                if failure.poison_runtime || failure.error.is_fatal() {
                    return Err(*failure.error);
                }
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
    /// Workflow #369 H-3: Stop and Start run on the ladder's key (the control's fence admits
    /// it, and the retained session is closed under it), each renewed before it (review L-3);
    /// the readiness wait renews to its window's end. Workflow #369-1: the rung then waits for
    /// a usable environment (`await_recovery_readiness`), records
    /// `recovery_environment_ready`, and runs the instance's startup package on the key; only
    /// that run's success recovers the rung.
    fn recovery_emulator_restart(
        &self,
        pending: &PendingRecoveryLadder,
        resolved: &RegisteredInstance,
        key: &mut HostKey,
    ) -> RuntimeHostResult<RungAttempt> {
        let instance_guard = self
            .instance_guard(pending.instance_id)
            .map_err(|failure| *failure.error)?;
        let admission = lock(&instance_guard, "lock_instance_admission")?;
        let mut stopped = None;
        for action in [EmulatorInstanceAction::Stop, EmulatorInstanceAction::Start] {
            if let Some(reason) = self.ladder_hold_interrupted(pending, key)? {
                return Ok(RungAttempt::Failed {
                    run_id: None,
                    reason,
                });
            }
            let bound_ms = if action == EmulatorInstanceAction::Stop {
                EMULATOR_STOP_BOUND_MS
            } else {
                EMULATOR_START_BOUND_MS
            };
            if let Some(failed) =
                self.ladder_step_renewal(key, bound_ms.saturating_add(HOLD_MARGIN_MS))?
            {
                return Ok(failed);
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
                Some(&key.token),
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
                key,
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
                    self.run_recovery_rung_package(&startup, true, pending, key)
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
    /// policy thread is never blocked by the wait (Workflow #369: the ladder's key withholds the
    /// instance from policy). The wait runs on this instance's worker and holds only this
    /// instance; other instances' host work runs on their own workers. A failed preparation is
    /// retried only when the existing rule calls it recoverable or the ADB baseline does not
    /// answer (review M3); every retry first polls the ADB baseline until it answers (bounded by
    /// `deadline`), then waits 5 s, 10 s, then 20 s, never past `deadline`. Each attempt makes
    /// the ladder's holding check (Workflow #369 H-3) and renews the key to the window's end;
    /// the preparation runs on the key and does not release it. Returns the current binding
    /// and the passing preparation event, or the rung's failure reason:
    /// `recovery_android_not_booted` when the window ends before the boot check ever passed
    /// (so no preparation ran, review L-R4-2), otherwise `recovery_environment_not_ready`.
    fn await_recovery_readiness<'guard>(
        &self,
        pending: &PendingRecoveryLadder,
        key: &mut HostKey,
        instance_guard: &'guard Mutex<()>,
        held: MutexGuard<'guard, ()>,
        deadline: Instant,
    ) -> RuntimeHostResult<Result<(RegisteredInstance, TerminalEvent), &'static str>> {
        let alias = pending.instance_alias.as_str();
        let mut held = Some(held);
        let mut retries = 0_usize;
        let mut prepared = false;
        loop {
            if let Some(reason) = self.ladder_hold_interrupted(pending, key)? {
                return Ok(Err(reason));
            }
            let window_ms = u64::try_from(
                deadline
                    .saturating_duration_since(Instant::now())
                    .as_millis(),
            )
            .unwrap_or(u64::MAX);
            if let Some(RungAttempt::Failed { reason, .. }) =
                self.ladder_step_renewal(key, window_ms.saturating_add(HOLD_MARGIN_MS))?
            {
                return Ok(Err(reason));
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
                let (check, preparation, recoverable) = self.prepare_recovery_connection(
                    &current,
                    pending.links.clone(),
                    &admission,
                    key,
                )?;
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
            if let Some(reason) =
                self.await_recovery_retry(pending, key, wait, deadline, not_ready)?
            {
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
    /// by `deadline`, and every poll makes the ladder's holding check (review L1; Workflow #369
    /// H-3, review2 L7: the scheduling pause too). `Some(reason)` ends the rung: shutdown,
    /// drain, a pause, a lost key, or no time left in the window (`not_ready`).
    fn await_recovery_retry(
        &self,
        pending: &PendingRecoveryLadder,
        key: &HostKey,
        wait: Duration,
        deadline: Instant,
        not_ready: &'static str,
    ) -> RuntimeHostResult<Option<&'static str>> {
        let alias = pending.instance_alias.as_str();
        let stopped = || self.fatal.is_shutdown_requested();
        loop {
            if let Some(reason) = self.ladder_hold_interrupted(pending, key)? {
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
            if let Some(reason) = self.ladder_hold_interrupted(pending, key)? {
                return Ok(Some(reason));
            }
            let now = Instant::now();
            if now >= until {
                return Ok(None);
            }
            thread::sleep(RECOVERY_READINESS_POLL.min(until - now));
        }
    }

    /// Runs a rung's package on the ladder's key; it recovers when the run completes with
    /// `success` (its target page reached). Workflow #369 H-3: the ladder queues its
    /// continuation before the run, so the run's release hands the key back. A failure was
    /// already recorded by the runner. After a destructive action (the emulator restart,
    /// Workflow #369-1; the application stop, Workflow #369-2) an unavailable entry channel or
    /// ADB fails the rung instead of skipping it, so `rungs_tried` counts the action that ran.
    fn run_recovery_rung_package(
        &self,
        run: &PendingStartupPackage,
        destructive: bool,
        pending: &PendingRecoveryLadder,
        key: &mut HostKey,
    ) -> RuntimeHostResult<RungAttempt> {
        Ok(match self.run_package_on_key(run, key, Some(pending))? {
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

    /// Workflow #369 H-3: queues the ladder's holder-owned continuation (high, no deadline,
    /// ordered first and never gated) under the ladder's correlation and causation, just
    /// before a rung run.
    pub(super) fn queue_ladder_continuation(
        &self,
        pending: &PendingRecoveryLadder,
        key: &HostKey,
    ) -> Result<LadderContinuation, RequestFailure> {
        let request_id = self
            .events
            .issuer()
            .mint_request_id()
            .map_err(|_| RequestFailure::poison_without_terminal(runtime_identifier_error()))?;
        let resolved = self.resolve_instance(&pending.instance_alias)?;
        let request = self
            .ladder_claim_request(
                &resolved,
                request_id,
                pending.correlation_id,
                pending.causation_id,
                key.token.holder_id(),
            )
            .map_err(RequestFailure::poison_without_terminal)?;
        match self.request_host_claim(HostClaim {
            request: &request,
            instance_alias: &pending.instance_alias,
            holder_id: key.token.holder_id(),
            connection_id: key.connection_id,
            kind: ClaimKind::LadderContinuation,
            priority: LeasePriority::High,
            lease_ttl_ms: LADDER_CLAIM_TTL_MS,
        })? {
            HostClaimAdmission::Queued { grant, .. } => Ok(LadderContinuation {
                request_id: *request_id.transport(),
                grant,
            }),
            HostClaimAdmission::Granted(_) => Err(RequestFailure::poison_without_terminal(
                ladder_invariant("recovery_ladder_continuation_granted_early"),
            )),
        }
    }

    /// Workflow #369 H-3: after a rung run, the key is the continuation's grant. A
    /// continuation that was not granted is cancelled (unless W-2's shutdown order already
    /// did); the next holding check then finds whether the ladder still holds its key.
    pub(super) fn take_ladder_continuation(
        &self,
        key: &mut HostKey,
        continuation: LadderContinuation,
    ) -> RuntimeHostResult<()> {
        if let Some(token) = continuation.grant.wait(Duration::ZERO)? {
            key.token = token;
            return Ok(());
        }
        let cancelled = self.cancel_host_claim(
            key.token.instance_id(),
            continuation.request_id,
            key.connection_id,
        )?;
        if !cancelled && let Some(token) = continuation.grant.wait(Duration::ZERO)? {
            key.token = token;
        }
        Ok(())
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

/// Review L6 (#666): whether a rung's failure reason ended the rung rather than being its own
/// failure: a reason `ladder_hold_interrupted` gives; a renewal refusal of the key
/// (`lease_expired`, `lease_missing`), which its next holding check reports as the key lost; or
/// a rung run that a scheduling pause's drain cancelled (`contained_task_paused`, which only the
/// pause drain sets).
fn is_hold_interruption(reason: &str) -> bool {
    matches!(
        reason,
        "recovery_ladder_shutdown_requested"
            | "recovery_ladder_drain_requested"
            | "recovery_admission_denied"
            | "contained_task_paused"
    ) || [
        HostCode::RecoveryLadderKeyLost,
        HostCode::LeaseExpired,
        HostCode::LeaseMissing,
    ]
    .into_iter()
    .any(|code| code.as_str() == reason)
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
