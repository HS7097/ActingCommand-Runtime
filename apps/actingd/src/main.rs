// SPDX-License-Identifier: AGPL-3.0-only

//! Thin process adapter for the resident ActingCommand Runtime.

#![forbid(unsafe_code)]

mod config;

use actingcommand_contract::{
    ApprovalDecisionRecord, ApprovalDisposition, ApprovalPayload, ApprovalTarget, EventActor,
    EventFamily, EventPayload, EventQuery, EventSource, EventType, MAX_RUNTIME_SUBSCRIPTION_EVENTS,
    ProjectedEvent, ProjectionPayload, ProjectionProfile, RuntimeEventQueryPageRequest,
    RuntimeSubscriptionRequest, SubscriptionCursor,
};
use actingcommand_policy::MAX_TASKS;
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig, RuntimeClientError};
use actingcommand_runtime_host::{
    PolicyAdmissionContext, PolicyCadence, PolicyCycle, PolicyDispatchAdmission,
    PolicyRecomputeKind, PolicyTrigger, RuntimeHost, RuntimeHostError,
};
use config::{PolicyBootstrap, RuntimeAssembly, ScheduledExecutionMode};
use std::collections::BTreeSet;
use std::env;
use std::error::Error;
use std::fmt;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(100);
const POLICY_EVENT_WAIT_MS: u64 = 250;
const POLICY_CLIENT_IO_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_POLICY_CYCLE_DURATION: Duration = Duration::from_secs(10 * 60);

fn main() -> ExitCode {
    match run(env::args_os().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("FATAL actingd: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: Vec<std::ffi::OsString>) -> Result<(), ActingdError> {
    let config_path = parse_arguments(arguments)?;
    let RuntimeAssembly {
        host,
        registry,
        policy,
    } = config::load(&config_path)
        .and_then(config::ActingdConfigFile::assemble)
        .map_err(ActingdError::config)?;
    let host = RuntimeHost::start(host, Arc::new(registry)).map_err(ActingdError::runtime)?;
    let initial_policy_cycle = policy
        .as_ref()
        .map(|policy| initialize_policy(&host, policy))
        .transpose();
    let initial_policy_cycle = match initial_policy_cycle {
        Ok(cycle) => cycle,
        Err(error) => {
            return match host.close() {
                Ok(()) => Err(error),
                Err(close_error) => Err(ActingdError::runtime(close_error)),
            };
        }
    };
    println!(
        "actingd ready pid={} host={} port={}",
        host.runtime_info().pid(),
        host.runtime_info().host(),
        host.runtime_info().port()
    );
    match (policy, initial_policy_cycle) {
        (Some(policy), Some(cycle)) => monitor_policy(host, policy, cycle),
        (None, None) => monitor(host),
        _ => {
            let error = ActingdError::process("policy_bootstrap_state_invalid");
            match host.close() {
                Ok(()) => Err(error),
                Err(close_error) => Err(ActingdError::runtime(close_error)),
            }
        }
    }
}

fn initialize_policy(
    host: &RuntimeHost,
    policy: &PolicyBootstrap,
) -> Result<PolicyCycle, ActingdError> {
    let generation = host
        .activate_policy_catalog(&policy.catalog)
        .map_err(ActingdError::runtime)?;
    let governance = RuntimeClient::connect(
        RuntimeClientConfig::new(&policy.state_root, EventActor::User, EventSource::Ui)
            .with_io_timeout(Duration::from_secs(5)),
    )
    .map_err(ActingdError::client)?;
    governance
        .authenticate_governance(&policy.governance_capability)
        .map_err(ActingdError::client)?;
    let approval_events = governance
        .query_events(
            EventQuery {
                event_type: Some(EventType::ApprovalDecision),
                ..EventQuery::default()
            },
            ProjectionProfile::Forensic,
        )
        .map_err(ActingdError::client)?;
    for approval_id in &policy.catalog_approval_ids {
        let decision = ApprovalDecisionRecord::new(
            approval_id,
            ApprovalDisposition::Approved,
            ApprovalTarget::Catalog {
                catalog_hash: generation.catalog_hash().to_owned(),
                catalog_version: generation.catalog_version(),
            },
            "configured_catalog_approval",
        )
        .map_err(|_| ActingdError::process("policy_catalog_approval_invalid"))?;
        let existing = approval_events
            .iter()
            .filter_map(|event| match &event.payload {
                ProjectionPayload::Full(payload) => match payload.as_ref() {
                    EventPayload::Approval(ApprovalPayload::Decision(payload))
                        if payload.decision().approval_id() == approval_id =>
                    {
                        Some((event.sequence, payload.decision()))
                    }
                    _ => None,
                },
                _ => None,
            })
            .max_by_key(|(sequence, _)| *sequence)
            .map(|(_, decision)| decision);
        if let Some(existing) = existing {
            // A persisted rejection or revocation cannot be silently replaced by startup config.
            if existing != &decision {
                return Err(ActingdError::process("policy_catalog_approval_conflict"));
            }
            continue;
        }
        governance
            .record_approval_decision(decision)
            .map_err(ActingdError::client)?;
    }
    drop(governance);
    execute_policy_cycle(host, policy, PolicyTrigger::Recovery)
}

fn execute_policy_cycle(
    host: &RuntimeHost,
    policy: &PolicyBootstrap,
    trigger: PolicyTrigger,
) -> Result<PolicyCycle, ActingdError> {
    let started = Instant::now();
    let cycle = host
        .evaluate_policy_cycle(trigger)
        .map_err(ActingdError::runtime)?;
    if cycle.pending_dispatch_intents.len() > MAX_TASKS {
        return Err(ActingdError::process(
            "policy_pending_dispatch_budget_exceeded",
        ));
    }
    let Some(evaluation) = cycle.evaluation.as_ref() else {
        if cycle.pending_dispatch_intents.is_empty() {
            return Ok(cycle);
        }
        return Err(ActingdError::process(
            "policy_pending_dispatch_without_evaluation",
        ));
    };
    for intent in &cycle.pending_dispatch_intents {
        if started.elapsed() >= MAX_POLICY_CYCLE_DURATION {
            return Err(ActingdError::process("policy_cycle_duration_exceeded"));
        }
        let reason_chain = evaluation
            .reason_chains
            .iter()
            .find(|reason_chain| reason_chain.id == intent.reason_chain_id)
            .ok_or_else(|| ActingdError::process("policy_reason_chain_missing"))?;
        let scheduled_task = policy.scheduled_tasks.get(&intent.procedure_ref);
        if let Some(task) = scheduled_task {
            validate_scheduled_execution_mode(
                task.mode,
                policy.registry_modes.get(&intent.instance_id).copied(),
            )?;
        }
        let admission = match host.admit_policy_dispatch(
            intent,
            reason_chain,
            &PolicyAdmissionContext {
                fact_ledger_position: intent.input_ledger_position,
                fact_snapshot_id: intent.fact_snapshot_id.clone(),
                approval_fact_ids: intent
                    .approval_refs
                    .iter()
                    .cloned()
                    .collect::<BTreeSet<_>>(),
                fencing_owner_epoch: host.runtime_info().owner_epoch(),
                now_unix_ms: intent.prerequisites.evaluated_at_unix_ms,
            },
        ) {
            Ok(admission) => admission,
            Err(error) if !error.is_fatal() => {
                eprintln!(
                    "WARNING actingd policy admission refused decision={} code={} operation={}",
                    intent.decision_id,
                    error.code(),
                    error.operation()
                );
                continue;
            }
            Err(error) => return Err(ActingdError::runtime(error)),
        };
        let PolicyDispatchAdmission::Granted { context } = admission else {
            continue;
        };
        if let Some(task) = scheduled_task {
            validate_scheduled_execution_mode(
                task.mode,
                policy.registry_modes.get(context.instance_alias()).copied(),
            )?;
            let receipt = host
                .run_scheduled_contained_task(&context, &task.request)
                .map_err(ActingdError::runtime)?;
            host.complete_scheduled_policy_run(&context, &receipt)
                .map_err(ActingdError::runtime)?;
        }
    }
    Ok(cycle)
}

fn validate_scheduled_execution_mode(
    expected: ScheduledExecutionMode,
    actual: Option<ScheduledExecutionMode>,
) -> Result<(), ActingdError> {
    let actual =
        actual.ok_or_else(|| ActingdError::process("scheduled_execution_instance_unknown"))?;
    if actual != expected {
        return Err(ActingdError::process(
            "scheduled_execution_backend_mode_mismatch",
        ));
    }
    Ok(())
}

fn monitor(host: RuntimeHost) -> Result<(), ActingdError> {
    loop {
        thread::sleep(HEALTH_POLL_INTERVAL);
        match host.fatal_error().map_err(ActingdError::runtime)? {
            Some(error) => {
                let close_error = host.close().err();
                return Err(
                    close_error.map_or_else(|| ActingdError::runtime(error), ActingdError::runtime)
                );
            }
            None => continue,
        }
    }
}

fn monitor_policy(
    host: RuntimeHost,
    policy: PolicyBootstrap,
    initial_cycle: PolicyCycle,
) -> Result<(), ActingdError> {
    let host = Arc::new(host);
    let policy = Arc::new(policy);
    let control = Arc::new(PolicyDriverControl::default());
    let client = RuntimeClient::connect(
        RuntimeClientConfig::new(&policy.state_root, EventActor::Agent, EventSource::Adapter)
            .with_io_timeout(POLICY_CLIENT_IO_TIMEOUT),
    )
    .map_err(ActingdError::client)?;
    let initial_page = client
        .query_event_page(
            EventQuery::default(),
            ProjectionProfile::Concise,
            RuntimeEventQueryPageRequest::new(1, None)
                .map_err(|_| ActingdError::process("policy_subscription_cursor_invalid"))?,
        )
        .map_err(ActingdError::client)?;
    let mut cursor = SubscriptionCursor {
        after_sequence: initial_page.snapshot_ledger_position(),
    };
    let driver = thread::Builder::new()
        .name("actingd-policy-driver".to_string())
        .spawn({
            let host = Arc::clone(&host);
            let policy = Arc::clone(&policy);
            let control = Arc::clone(&control);
            move || drive_policy(host, policy, control, initial_cycle)
        })
        .map_err(|_| ActingdError::process("policy_driver_spawn_failed"))?;

    let monitor_result = loop {
        if driver.is_finished() {
            break Err(ActingdError::process("policy_driver_stopped"));
        }
        match host.fatal_error() {
            Ok(Some(error)) => break Err(ActingdError::runtime(error)),
            Ok(None) => {}
            Err(error) => break Err(ActingdError::runtime(error)),
        }
        let request = match RuntimeSubscriptionRequest::new(
            EventQuery::default(),
            ProjectionProfile::Concise,
            cursor,
            POLICY_EVENT_WAIT_MS,
            MAX_RUNTIME_SUBSCRIPTION_EVENTS,
        ) {
            Ok(request) => request,
            Err(_) => {
                break Err(ActingdError::process("policy_subscription_request_invalid"));
            }
        };
        let batch = match client.subscribe_events(request) {
            Ok(batch) => batch,
            Err(error) => break Err(ActingdError::client(error)),
        };
        cursor = batch.next_cursor();
        if let Some(trigger) = policy_trigger_for_events(batch.events())
            && let Err(error) = control.notify(trigger)
        {
            break Err(error);
        }
    };

    let shutdown_result = control.shutdown();
    let driver_result = match driver.join() {
        Ok(result) => result,
        Err(_) => Err(ActingdError::process("policy_driver_panicked")),
    };
    drop(client);
    drop(policy);
    drop(control);
    let close_result = match Arc::try_unwrap(host) {
        Ok(host) => host.close().map_err(ActingdError::runtime),
        Err(host) => {
            drop(host);
            Err(ActingdError::process("policy_driver_reference_leaked"))
        }
    };
    combine_monitor_results(monitor_result, shutdown_result, driver_result, close_result)
}

fn combine_monitor_results(
    monitor: Result<(), ActingdError>,
    shutdown: Result<(), ActingdError>,
    driver: Result<(), ActingdError>,
    close: Result<(), ActingdError>,
) -> Result<(), ActingdError> {
    let mut failure = None;
    for (stage, result) in [
        ("driver", driver),
        ("monitor", monitor),
        ("shutdown", shutdown),
        ("close", close),
    ] {
        if let Err(error) = result {
            if failure.is_none() {
                failure = Some(error);
            } else {
                eprintln!("ERROR actingd secondary failure during {stage}: {error}");
            }
        }
    }
    failure.map_or(Ok(()), Err)
}

fn policy_trigger_for_events(events: &[ProjectedEvent]) -> Option<PolicyTrigger> {
    events
        .iter()
        .filter_map(|event| policy_trigger_for_family(event.event_type.family()))
        .reduce(coalesce_policy_trigger)
}

fn policy_trigger_for_family(family: EventFamily) -> Option<PolicyTrigger> {
    match family {
        EventFamily::Catalog => Some(PolicyTrigger::CatalogChanged),
        EventFamily::Performance | EventFamily::ResourceAuthoring => {
            Some(PolicyTrigger::ResourcesChanged)
        }
        EventFamily::Fact | EventFamily::Approval => Some(PolicyTrigger::FactsChanged),
        EventFamily::Runtime
        | EventFamily::Monitor
        | EventFamily::Command
        | EventFamily::Scheduler
        | EventFamily::Policy
        | EventFamily::Lease
        | EventFamily::Task
        | EventFamily::Application
        | EventFamily::Input
        | EventFamily::Capture
        | EventFamily::Recognition
        | EventFamily::Artifact
        | EventFamily::Client
        | EventFamily::State
        | EventFamily::Release
        | EventFamily::Agent
        | EventFamily::Ledger => None,
    }
}

fn coalesce_policy_trigger(left: PolicyTrigger, right: PolicyTrigger) -> PolicyTrigger {
    if trigger_priority(right) > trigger_priority(left) {
        right
    } else {
        left
    }
}

const fn trigger_priority(trigger: PolicyTrigger) -> u8 {
    match trigger {
        PolicyTrigger::CatalogChanged => 5,
        PolicyTrigger::Reconciliation => 4,
        PolicyTrigger::ClockObserved { .. } => 3,
        PolicyTrigger::ResourcesChanged => 2,
        PolicyTrigger::FactsChanged => 1,
        PolicyTrigger::Recovery => 0,
    }
}

#[derive(Default)]
struct PolicyDriverControl {
    state: Mutex<PolicyDriverSignal>,
    changed: Condvar,
}

#[derive(Default)]
struct PolicyDriverSignal {
    pending_trigger: Option<PolicyTrigger>,
    shutdown: bool,
}

enum PolicyDriverWake {
    Trigger(PolicyTrigger),
    Shutdown,
    TimedOut,
}

impl PolicyDriverControl {
    fn notify(&self, trigger: PolicyTrigger) -> Result<(), ActingdError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ActingdError::process("policy_driver_signal_poisoned"))?;
        if !state.shutdown {
            state.pending_trigger = Some(
                state
                    .pending_trigger
                    .map_or(trigger, |pending| coalesce_policy_trigger(pending, trigger)),
            );
            self.changed.notify_one();
        }
        Ok(())
    }

    fn shutdown(&self) -> Result<(), ActingdError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ActingdError::process("policy_driver_signal_poisoned"))?;
        state.shutdown = true;
        self.changed.notify_all();
        Ok(())
    }

    fn wait(&self, timeout: Duration) -> Result<PolicyDriverWake, ActingdError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ActingdError::process("policy_driver_signal_poisoned"))?;
        if state.shutdown {
            return Ok(PolicyDriverWake::Shutdown);
        }
        if let Some(trigger) = state.pending_trigger.take() {
            return Ok(PolicyDriverWake::Trigger(trigger));
        }
        if timeout.is_zero() {
            return Ok(PolicyDriverWake::TimedOut);
        }
        let (mut state, wait) = self
            .changed
            .wait_timeout(state, timeout)
            .map_err(|_| ActingdError::process("policy_driver_signal_poisoned"))?;
        if state.shutdown {
            Ok(PolicyDriverWake::Shutdown)
        } else if let Some(trigger) = state.pending_trigger.take() {
            Ok(PolicyDriverWake::Trigger(trigger))
        } else if wait.timed_out() {
            Ok(PolicyDriverWake::TimedOut)
        } else {
            Err(ActingdError::process("policy_driver_wake_missing"))
        }
    }
}

struct DeferredPolicyTrigger {
    trigger: PolicyTrigger,
    eligible_at_unix_ms: u64,
}

struct PolicyDriverSchedule {
    cadence: PolicyCadence,
    next_wake_unix_ms: Option<u64>,
    next_reconciliation_unix_ms: u64,
    next_clock_observation_unix_ms: u64,
    last_clock_observation_unix_ms: u64,
    deferred: Option<DeferredPolicyTrigger>,
}

impl PolicyDriverSchedule {
    fn new(
        cadence: PolicyCadence,
        initial_cycle: &PolicyCycle,
        now_unix_ms: u64,
    ) -> Result<Self, ActingdError> {
        let mut schedule = Self {
            next_wake_unix_ms: None,
            next_reconciliation_unix_ms: checked_deadline(
                now_unix_ms,
                cadence.reconciliation_interval_ms,
            )?,
            next_clock_observation_unix_ms: checked_deadline(
                now_unix_ms,
                clock_observation_interval_ms(&cadence),
            )?,
            last_clock_observation_unix_ms: now_unix_ms,
            deferred: None,
            cadence,
        };
        schedule.apply_cycle(PolicyTrigger::Recovery, initial_cycle, now_unix_ms)?;
        Ok(schedule)
    }

    fn apply_cycle(
        &mut self,
        trigger: PolicyTrigger,
        cycle: &PolicyCycle,
        now_unix_ms: u64,
    ) -> Result<(), ActingdError> {
        if let Some(evaluation) = cycle.evaluation.as_ref() {
            self.next_wake_unix_ms = evaluation.next_wake_unix_ms;
        }
        if cycle.directive.kind == PolicyRecomputeKind::Deferred {
            let pending = DeferredPolicyTrigger {
                trigger,
                eligible_at_unix_ms: cycle.directive.eligible_at_unix_ms,
            };
            self.deferred = Some(match self.deferred.take() {
                Some(existing) => DeferredPolicyTrigger {
                    trigger: coalesce_policy_trigger(existing.trigger, pending.trigger),
                    eligible_at_unix_ms: existing
                        .eligible_at_unix_ms
                        .max(pending.eligible_at_unix_ms),
                },
                None => pending,
            });
        } else {
            self.deferred = None;
            if cycle.directive.kind == PolicyRecomputeKind::Full {
                self.next_reconciliation_unix_ms =
                    checked_deadline(now_unix_ms, self.cadence.reconciliation_interval_ms)?;
            }
        }
        Ok(())
    }

    fn coalesce_deferred_event(&mut self, trigger: PolicyTrigger) -> bool {
        let Some(deferred) = self.deferred.as_mut() else {
            return false;
        };
        if trigger == PolicyTrigger::CatalogChanged {
            return false;
        }
        deferred.trigger = coalesce_policy_trigger(deferred.trigger, trigger);
        true
    }

    fn take_due_trigger(
        &mut self,
        now_unix_ms: u64,
    ) -> Result<Option<PolicyTrigger>, ActingdError> {
        let previous = self.last_clock_observation_unix_ms;
        let delta = now_unix_ms.saturating_sub(previous);
        if now_unix_ms < previous || delta > self.cadence.clock_jump_threshold_ms {
            self.record_clock_observation(now_unix_ms)?;
            return Ok(Some(PolicyTrigger::ClockObserved {
                previous_unix_ms: previous,
            }));
        }
        if now_unix_ms >= self.next_clock_observation_unix_ms {
            self.record_clock_observation(now_unix_ms)?;
        }
        if now_unix_ms >= self.next_reconciliation_unix_ms {
            return Ok(Some(PolicyTrigger::Reconciliation));
        }
        if self
            .deferred
            .as_ref()
            .is_some_and(|deferred| now_unix_ms >= deferred.eligible_at_unix_ms)
        {
            return Ok(self.deferred.take().map(|deferred| deferred.trigger));
        }
        if self
            .next_wake_unix_ms
            .is_some_and(|next_wake| now_unix_ms >= next_wake)
        {
            self.next_wake_unix_ms = None;
            let previous_unix_ms = self.last_clock_observation_unix_ms;
            self.record_clock_observation(now_unix_ms)?;
            return Ok(Some(PolicyTrigger::ClockObserved { previous_unix_ms }));
        }
        Ok(None)
    }

    fn wait_duration(&self, now_unix_ms: u64) -> Duration {
        let next = [
            self.next_wake_unix_ms,
            Some(self.next_reconciliation_unix_ms),
            Some(self.next_clock_observation_unix_ms),
            self.deferred
                .as_ref()
                .map(|deferred| deferred.eligible_at_unix_ms),
        ]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(now_unix_ms);
        Duration::from_millis(next.saturating_sub(now_unix_ms))
    }

    fn record_clock_observation(&mut self, now_unix_ms: u64) -> Result<(), ActingdError> {
        self.last_clock_observation_unix_ms = now_unix_ms;
        self.next_clock_observation_unix_ms =
            checked_deadline(now_unix_ms, clock_observation_interval_ms(&self.cadence))?;
        Ok(())
    }
}

fn drive_policy(
    host: Arc<RuntimeHost>,
    policy: Arc<PolicyBootstrap>,
    control: Arc<PolicyDriverControl>,
    initial_cycle: PolicyCycle,
) -> Result<(), ActingdError> {
    let mut schedule =
        PolicyDriverSchedule::new(policy.cadence.clone(), &initial_cycle, system_unix_ms()?)?;
    loop {
        match control.wait(Duration::ZERO)? {
            PolicyDriverWake::Shutdown => return Ok(()),
            PolicyDriverWake::Trigger(trigger) => {
                if !schedule.coalesce_deferred_event(trigger) {
                    let now = system_unix_ms()?;
                    let trigger = schedule
                        .take_due_trigger(now)?
                        .map_or(trigger, |due| coalesce_policy_trigger(due, trigger));
                    let cycle = execute_policy_cycle(&host, &policy, trigger)?;
                    schedule.apply_cycle(trigger, &cycle, now)?;
                }
                continue;
            }
            PolicyDriverWake::TimedOut => {}
        }

        let now = system_unix_ms()?;
        if let Some(trigger) = schedule.take_due_trigger(now)? {
            let cycle = execute_policy_cycle(&host, &policy, trigger)?;
            schedule.apply_cycle(trigger, &cycle, now)?;
            continue;
        }
        match control.wait(schedule.wait_duration(now))? {
            PolicyDriverWake::Shutdown => return Ok(()),
            PolicyDriverWake::Trigger(trigger) => {
                if schedule.coalesce_deferred_event(trigger) {
                    continue;
                }
                let now = system_unix_ms()?;
                let trigger = schedule
                    .take_due_trigger(now)?
                    .map_or(trigger, |due| coalesce_policy_trigger(due, trigger));
                let cycle = execute_policy_cycle(&host, &policy, trigger)?;
                schedule.apply_cycle(trigger, &cycle, now)?;
            }
            PolicyDriverWake::TimedOut => {}
        }
    }
}

fn clock_observation_interval_ms(cadence: &PolicyCadence) -> u64 {
    (cadence.clock_jump_threshold_ms / 2).max(1)
}

fn checked_deadline(now_unix_ms: u64, delay_ms: u64) -> Result<u64, ActingdError> {
    now_unix_ms
        .checked_add(delay_ms)
        .ok_or_else(|| ActingdError::process("policy_driver_time_overflow"))
}

fn system_unix_ms() -> Result<u64, ActingdError> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ActingdError::process("system_clock_invalid"))?;
    u64::try_from(elapsed.as_millis()).map_err(|_| ActingdError::process("system_clock_invalid"))
}

fn parse_arguments(arguments: Vec<std::ffi::OsString>) -> Result<PathBuf, ActingdError> {
    let [flag, path] = arguments.as_slice() else {
        return Err(ActingdError::config("usage_invalid"));
    };
    if flag != "--config" || path.is_empty() {
        return Err(ActingdError::config("usage_invalid"));
    }
    Ok(PathBuf::from(path))
}

#[derive(Debug)]
struct ActingdError {
    code: &'static str,
    runtime: Option<Box<RuntimeHostError>>,
    client: Option<Box<RuntimeClientError>>,
}

impl ActingdError {
    const fn config(code: &'static str) -> Self {
        Self {
            code,
            runtime: None,
            client: None,
        }
    }

    const fn process(code: &'static str) -> Self {
        Self {
            code,
            runtime: None,
            client: None,
        }
    }

    fn runtime(error: RuntimeHostError) -> Self {
        Self {
            code: error.code(),
            runtime: Some(Box::new(error)),
            client: None,
        }
    }

    fn client(error: RuntimeClientError) -> Self {
        Self {
            code: error.code(),
            runtime: None,
            client: Some(Box::new(error)),
        }
    }
}

impl fmt::Display for ActingdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.runtime {
            Some(error) => error.fmt(formatter),
            None => match &self.client {
                Some(error) => error.fmt(formatter),
                None => formatter.write_str(self.code),
            },
        }
    }
}

impl Error for ActingdError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ScheduledProcedureTask;
    use actingcommand_contract::{
        ApplicationLifecycleAction, ContainedTaskRequest, IdentifierIssuer,
    };
    use actingcommand_device::{
        CaptureBackend, CaptureBackendName, DeviceError, DeviceResult, Frame, InputBackend,
        PixelFormat,
    };
    use actingcommand_policy::{
        CatalogDocumentSource, CatalogSources, EvaluationFacts, EvaluationResources,
        HostResourceSnapshot, InstanceSnapshot, PoolValueSnapshot,
    };
    use actingcommand_runtime_host::{
        ExecutionBackendProvider, PolicyInputSnapshot, ProcedureBinding, ProcedureManifest,
        ResolvedExecutionInstance, RuntimeHostConfig,
    };
    use actingcommand_runtime_host::{PolicyRecomputeDirective, PolicyRecomputeReason};
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    use std::collections::{BTreeMap, VecDeque};
    use std::ffi::OsString;
    use std::io::{Cursor, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;
    use zip::ZipWriter;
    use zip::write::FileOptions;

    #[test]
    fn process_adapter_requires_exact_config_argument() {
        assert!(parse_arguments(Vec::new()).is_err());
        assert!(parse_arguments(vec![OsString::from("--config")]).is_err());
        assert!(
            parse_arguments(vec![
                OsString::from("--config"),
                OsString::from("actingd.json")
            ])
            .is_ok()
        );
    }

    #[test]
    fn resident_driver_coalesces_relevant_event_bursts_without_terminal_feedback() {
        let control = PolicyDriverControl::default();
        control
            .notify(PolicyTrigger::FactsChanged)
            .expect("fact notification");
        control
            .notify(PolicyTrigger::ResourcesChanged)
            .expect("resource notification");
        control
            .notify(PolicyTrigger::CatalogChanged)
            .expect("catalog notification");
        assert!(matches!(
            control.wait(Duration::ZERO).expect("coalesced wake"),
            PolicyDriverWake::Trigger(PolicyTrigger::CatalogChanged)
        ));
        assert_eq!(
            policy_trigger_for_family(EventFamily::Policy),
            None,
            "terminal/settlement policy events belong to #95 and must not wake #94"
        );
        assert_eq!(
            policy_trigger_for_family(EventFamily::Fact),
            Some(PolicyTrigger::FactsChanged)
        );
        assert_eq!(
            policy_trigger_for_family(EventFamily::Performance),
            Some(PolicyTrigger::ResourcesChanged)
        );
    }

    #[test]
    fn resident_driver_shutdown_wakes_and_reclaims_the_waiter() {
        let control = Arc::new(PolicyDriverControl::default());
        let waiter = thread::spawn({
            let control = Arc::clone(&control);
            move || control.wait(Duration::from_secs(5))
        });
        thread::sleep(Duration::from_millis(20));
        let started = Instant::now();
        control.shutdown().expect("request driver shutdown");
        assert!(matches!(
            waiter.join().expect("join waiter").expect("wait result"),
            PolicyDriverWake::Shutdown
        ));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn formal_provenance_provider_uses_the_same_scheduled_owner_chain() {
        let root = TempDir::new().expect("tempdir");
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        let instance_id = *ids.mint_instance_id().expect("instance id").transport();
        let package = resident_test_package();
        let package_sha256 = format!("{:x}", Sha256::digest(&package));
        let package_path = root.path().join("task.zip");
        std::fs::write(&package_path, &package).expect("write package");
        let input_count = Arc::new(AtomicUsize::new(0));
        let capture_count = Arc::new(AtomicUsize::new(0));
        let provider = Arc::new(RecordingDeviceProvider {
            instance_id,
            input_count: Arc::clone(&input_count),
            capture_count: Arc::clone(&capture_count),
        });
        let catalog = resident_test_catalog();
        let manifest = ProcedureManifest::new(vec![
            ProcedureBinding::new(
                "procedure.observe",
                format!("sha256:{package_sha256}"),
                "operation.observe",
                vec!["after_observation".to_string()],
            )
            .expect("procedure binding"),
        ])
        .expect("procedure manifest");
        let now = system_unix_ms().expect("system clock");
        let host = RuntimeHost::start(
            RuntimeHostConfig::new(root.path(), b"0123456789abcdef")
                .with_governance_capability("actingd-policy-bootstrap-capability")
                .with_policy_inputs(PolicyInputSnapshot::new(
                    resident_test_facts(),
                    resident_test_resources(now),
                ))
                .with_procedure_manifest(manifest),
            provider,
        )
        .expect("start formal-provider runtime");
        let policy = PolicyBootstrap {
            state_root: root.path().to_path_buf(),
            governance_capability: "actingd-policy-bootstrap-capability".to_string(),
            catalog_approval_ids: vec!["approval:fixture-a".to_string()],
            catalog,
            scheduled_tasks: BTreeMap::from([(
                "procedure.observe".to_string(),
                ScheduledProcedureTask {
                    request: ContainedTaskRequest::new(
                        package_path.to_string_lossy().into_owned(),
                        &package_sha256,
                    )
                    .expect("contained task request"),
                    mode: ScheduledExecutionMode::DeviceRegistry,
                },
            )]),
            registry_modes: BTreeMap::from([(
                "fixture-instance-a".to_string(),
                ScheduledExecutionMode::DeviceRegistry,
            )]),
            cadence: PolicyCadence::default(),
        };
        let cycle = initialize_policy(&host, &policy).expect("formal scheduled Recovery cycle");
        assert_eq!(cycle.pending_dispatch_intents.len(), 1);
        assert_eq!(input_count.load(Ordering::SeqCst), 1);
        assert_eq!(capture_count.load(Ordering::SeqCst), 2);

        let client = RuntimeClient::connect(
            RuntimeClientConfig::new(root.path(), EventActor::Agent, EventSource::Adapter)
                .with_io_timeout(Duration::from_secs(1)),
        )
        .expect("connect formal-provider runtime");
        let events = client
            .query_events(EventQuery::default(), ProjectionProfile::Forensic)
            .expect("query formal-provider owner chain");
        for event_type in [
            EventType::PolicyDispatchIntent,
            EventType::PolicyDispatchAdmitted,
            EventType::LeaseGranted,
            EventType::TaskRequested,
            EventType::InputCommitted,
            EventType::TaskCompleted,
            EventType::LeaseReleased,
            EventType::PolicyExecutionRecorded,
            EventType::PolicyDispatchCompleted,
        ] {
            assert_eq!(
                events
                    .iter()
                    .filter(|event| event.event_type == event_type)
                    .count(),
                1,
                "{event_type:?}"
            );
        }
        drop(client);
        host.close().expect("close formal-provider runtime");
    }

    #[test]
    fn resident_schedule_has_bounded_idle_reconciliation_and_clock_jump_wakes() {
        let now = 10_000;
        let cadence = PolicyCadence {
            debounce_ms: 10,
            cooldown_ms: 20,
            reconciliation_interval_ms: 1_000,
            clock_jump_threshold_ms: 2_000,
        };
        let cycle = policy_cycle(PolicyRecomputeKind::Full, now);
        let mut schedule =
            PolicyDriverSchedule::new(cadence, &cycle, now).expect("resident schedule");
        assert_eq!(schedule.wait_duration(now), Duration::from_millis(1_000));
        assert_eq!(
            schedule
                .take_due_trigger(now + 1_000)
                .expect("reconciliation deadline"),
            Some(PolicyTrigger::Reconciliation)
        );

        let cadence = PolicyCadence {
            debounce_ms: 10,
            cooldown_ms: 20,
            reconciliation_interval_ms: 60_000,
            clock_jump_threshold_ms: 1_000,
        };
        let cycle = policy_cycle(PolicyRecomputeKind::Full, now);
        let mut schedule = PolicyDriverSchedule::new(cadence, &cycle, now).expect("clock schedule");
        assert_eq!(
            schedule.take_due_trigger(now + 1_001).expect("clock jump"),
            Some(PolicyTrigger::ClockObserved {
                previous_unix_ms: now
            })
        );
    }

    struct RecordingDeviceProvider {
        instance_id: actingcommand_contract::InstanceId,
        input_count: Arc<AtomicUsize>,
        capture_count: Arc<AtomicUsize>,
    }

    impl ExecutionBackendProvider for RecordingDeviceProvider {
        fn instance_aliases(&self) -> Vec<String> {
            vec!["fixture-instance-a".to_string()]
        }

        fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance> {
            (instance_alias == "fixture-instance-a")
                .then(|| ResolvedExecutionInstance::new(self.instance_id, "recording-device"))
        }

        fn open_input(&self, _instance_alias: &str) -> DeviceResult<Box<dyn InputBackend>> {
            Ok(Box::new(RecordingInput {
                count: Arc::clone(&self.input_count),
            }))
        }

        fn open_capture(&self, _instance_alias: &str) -> DeviceResult<Box<dyn CaptureBackend>> {
            Ok(Box::new(RecordingCapture {
                count: Arc::clone(&self.capture_count),
                frames: VecDeque::from([
                    Frame::from_pixels(
                        2,
                        1,
                        vec![255, 0, 0, 0, 255, 0],
                        PixelFormat::Rgb8,
                        CaptureBackendName::AdbScreencap,
                    )
                    .expect("home frame"),
                    Frame::from_pixels(
                        2,
                        1,
                        vec![0, 0, 255, 0, 255, 0],
                        PixelFormat::Rgb8,
                        CaptureBackendName::AdbScreencap,
                    )
                    .expect("terminal frame"),
                ]),
            }))
        }

        fn control_application(
            &self,
            _instance_alias: &str,
            _action: ApplicationLifecycleAction,
        ) -> DeviceResult<()> {
            Err(DeviceError::fatal(
                "recording provider application control is forbidden",
            ))
        }
    }

    struct RecordingCapture {
        count: Arc<AtomicUsize>,
        frames: VecDeque<Frame>,
    }

    impl CaptureBackend for RecordingCapture {
        fn capture(&mut self) -> DeviceResult<Frame> {
            let frame = self
                .frames
                .pop_front()
                .ok_or_else(|| DeviceError::fatal("recording capture exhausted"))?;
            self.count.fetch_add(1, Ordering::SeqCst);
            Ok(frame)
        }
    }

    struct RecordingInput {
        count: Arc<AtomicUsize>,
    }

    impl RecordingInput {
        fn record(&self) {
            self.count.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl InputBackend for RecordingInput {
        fn tap(&mut self, _x: i32, _y: i32) -> DeviceResult<()> {
            self.record();
            Ok(())
        }

        fn long_tap(&mut self, _x: i32, _y: i32, _duration_ms: u64) -> DeviceResult<()> {
            self.record();
            Ok(())
        }

        fn swipe(
            &mut self,
            _x1: i32,
            _y1: i32,
            _x2: i32,
            _y2: i32,
            _duration_ms: u64,
        ) -> DeviceResult<()> {
            self.record();
            Ok(())
        }

        fn key(&mut self, _key: &str) -> DeviceResult<()> {
            self.record();
            Ok(())
        }

        fn text(&mut self, _text: &str) -> DeviceResult<()> {
            self.record();
            Ok(())
        }

        fn reset(&mut self) -> DeviceResult<()> {
            self.record();
            Ok(())
        }

        fn close(&mut self) -> DeviceResult<()> {
            Ok(())
        }
    }

    fn resident_test_catalog() -> CatalogSources {
        let mut sources = CatalogSources {
            tasks: CatalogDocumentSource::new(
                "memory://resident/tasks.json",
                include_bytes!("../../../contracts/scheduling/examples/catalog-a/tasks.json")
                    .to_vec(),
            ),
            pools: CatalogDocumentSource::new(
                "memory://resident/pools.json",
                include_bytes!("../../../contracts/scheduling/examples/catalog-a/pools.json")
                    .to_vec(),
            ),
            activity: CatalogDocumentSource::new(
                "memory://resident/activity.json",
                include_bytes!("../../../contracts/scheduling/examples/catalog-a/activity.json")
                    .to_vec(),
            ),
            timeline: CatalogDocumentSource::new(
                "memory://resident/timeline.json",
                include_bytes!("../../../contracts/scheduling/examples/catalog-a/timeline.json")
                    .to_vec(),
            ),
        };
        let mut tasks: Value =
            serde_json::from_slice(&sources.tasks.bytes).expect("task catalog JSON");
        tasks["tasks"][0]["feedback_stop"] = json!({
            "kind": "clock",
            "schedule": {
                "kind": "at",
                "clock_source": {
                    "kind": "server",
                    "timezone_id": "etc/utc",
                    "utc_offset_minutes": 0,
                    "dst_offset_minutes": 0,
                    "maintenance_drift_ms": 0
                },
                "at_ms": 4_102_444_800_000_u64
            }
        });
        sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("task catalog bytes");
        let mut activity: Value =
            serde_json::from_slice(&sources.activity.bytes).expect("activity catalog JSON");
        activity["profiles"][0]["windows"][0]["start_minute_of_day"] = json!(0);
        activity["profiles"][0]["windows"][0]["end_minute_of_day"] = json!(0);
        sources.activity.bytes =
            serde_json::to_vec_pretty(&activity).expect("activity catalog bytes");
        sources
    }

    fn resident_test_facts() -> EvaluationFacts {
        EvaluationFacts {
            ledger_position: 0,
            fact_snapshot_id: "snapshot:resident".to_string(),
            facts: Vec::new(),
            outcomes: Vec::new(),
            tasks: Vec::new(),
            instances: vec![InstanceSnapshot {
                instance_id: "fixture-instance-a".to_string(),
                server_id: "fixture-server-a".to_string(),
                game_id: "fixture-game-a".to_string(),
                host_id: "fixture-host-a".to_string(),
                available: true,
                capability_operation_ids: vec!["operation.observe".to_string()],
                preferred_task_ids: Vec::new(),
            }],
        }
    }

    fn resident_test_resources(now_unix_ms: u64) -> EvaluationResources {
        EvaluationResources {
            pools: vec![PoolValueSnapshot {
                pool_id: "fixture-pool-a".to_string(),
                value: 10,
                observed_at_unix_ms: now_unix_ms,
            }],
            hosts: vec![HostResourceSnapshot {
                host_id: "fixture-host-a".to_string(),
                cpu_available_milli: 1_000,
                gpu_available_milli: 1_000,
                io_available_milli: 1_000,
                host_responsiveness_basis_points: 10_000,
                third_party_pressure_basis_points: 0,
                heavy_dispatch_limit: 1,
                active_heavy_dispatches: 0,
            }],
        }
    }

    fn resident_test_package() -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut zip = ZipWriter::new(cursor);
        let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        let files: &[(&str, &[u8])] = &[
            (
                "control.json",
                br#"{
                    "schema_version":"Lab-1y.control.v1",
                    "package_id":"neutral.semantic.task",
                    "execution_mode":"navigable_route",
                    "game":"neutral",
                    "server":"test",
                    "resolution":{"width":2,"height":1},
                    "entry_task_id":"task",
                    "capture_interval_ms":50,
                    "step_timeout_ms":50,
                    "timeout_ms":1000,
                    "max_steps":2
                }"#,
            ),
            (
                "resources/manifest.json",
                br#"{"schema_version":"0.3","entry_task_id":"task"}"#,
            ),
            (
                "resources/operations/task/task.json",
                br#"{
                    "schema_version":"0.6",
                    "task_id":"task",
                    "game":"neutral",
                    "server_scope":["test"],
                    "coordinate_space":{"width":2,"height":1},
                    "entry_page":"home",
                    "target_page":"terminal",
                    "operations":[{
                        "id":"open_terminal",
                        "from":"home",
                        "to":"terminal",
                        "click":{"kind":"point","x":1,"y":0},
                        "unguarded_trusted_coordinate":true
                    }]
                }"#,
            ),
            (
                "resources/recognition/neutral.test.pack.json",
                br#"{
                    "schema_version":"0.3",
                    "game":"neutral",
                    "server":"test",
                    "coordinate_space":{"width":2,"height":1},
                    "defaults":{"color_max_distance":0.0},
                    "targets":[
                        {"type":"color","id":"page/home","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]},
                        {"type":"color","id":"page/terminal","region":{"x":0,"y":0,"width":1,"height":1},"expected":[0,0,255]}
                    ]
                }"#,
            ),
            (
                "resources/recognition/neutral.test.pages.json",
                br#"{
                    "schema_version":"0.3",
                    "pages":[
                        {"id":"neutral/home","required":["page/home"],"optional":[],"forbidden":[]},
                        {"id":"neutral/terminal","required":["page/terminal"],"optional":[],"forbidden":[]}
                    ]
                }"#,
            ),
        ];
        for (path, contents) in files {
            zip.start_file(*path, options).expect("zip entry");
            zip.write_all(contents).expect("zip content");
        }
        zip.finish().expect("finish zip").into_inner()
    }

    fn policy_cycle(kind: PolicyRecomputeKind, eligible_at_unix_ms: u64) -> PolicyCycle {
        PolicyCycle {
            directive: PolicyRecomputeDirective {
                kind,
                reason: PolicyRecomputeReason::StartupOrRecovery,
                eligible_at_unix_ms,
            },
            evaluation: None,
            pending_dispatch_intents: Vec::new(),
            detection_planning_signals: Vec::new(),
            measurement: None,
        }
    }
}
