// SPDX-License-Identifier: AGPL-3.0-only

//! Thin process adapter for the resident ActingCommand Runtime.

#![forbid(unsafe_code)]

mod config;

use actingcommand_contract::{
    ApprovalDecisionRecord, ApprovalDisposition, ApprovalPayload, ApprovalTarget, EventActor,
    EventFamily, EventPayload, EventQuery, EventSource, EventType, MAX_RUNTIME_SUBSCRIPTION_EVENTS,
    PolicyExecutionEventData, ProjectedEvent, ProjectionPayload, ProjectionProfile, RunId,
    RuntimeEventQueryPageRequest, RuntimeReceipt, RuntimeSubscriptionRequest,
    SchedulingOutcomeProjection, SubscriptionCursor,
};
use actingcommand_policy::MAX_TASKS;
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig, RuntimeClientError};
use actingcommand_runtime_host::{
    PolicyAdmissionContext, PolicyCadence, PolicyCycle, PolicyDispatchAdmission,
    PolicyRecomputeKind, PolicyRunContext, PolicyTrigger, RuntimeHost, RuntimeHostError,
    RuntimeLifecycleFailure, RuntimeLifecycleFailureStage,
};
use config::{PolicyBootstrap, RuntimeAssembly, ScheduledExecutionMode};
use std::collections::{BTreeMap, BTreeSet};
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
            let recorded = error.record_lifecycle_failure(
                &host,
                RuntimeLifecycleFailureStage::PolicyInitialization,
            );
            let closed = host.close();
            recorded?;
            return match closed {
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
) -> Result<PolicyCycleExecution, ActingdError> {
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
) -> Result<PolicyCycleExecution, ActingdError> {
    let started = Instant::now();
    let cycle = host
        .evaluate_policy_cycle(trigger)
        .map_err(ActingdError::runtime)?;
    let mut recompute_wakes = Vec::new();
    if cycle.pending_dispatch_intents.len() > MAX_TASKS {
        return Err(ActingdError::process(
            "policy_pending_dispatch_budget_exceeded",
        ));
    }
    let Some(evaluation) = cycle.evaluation.as_ref() else {
        if cycle.pending_dispatch_intents.is_empty() {
            return Ok(PolicyCycleExecution {
                cycle,
                recompute_wakes,
            });
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
            let (execution, projection) = host
                .complete_scheduled_policy_run(&context, &receipt)
                .map_err(ActingdError::runtime)?;
            if let Some(projection) = projection {
                if recompute_wakes.len() >= MAX_TASKS {
                    return Err(ActingdError::process(
                        "policy_recompute_wake_budget_exceeded",
                    ));
                }
                recompute_wakes.push(PolicyRecomputeWake::new(
                    &context, &receipt, &execution, projection,
                )?);
            }
        }
    }
    Ok(PolicyCycleExecution {
        cycle,
        recompute_wakes,
    })
}

struct PolicyCycleExecution {
    cycle: PolicyCycle,
    recompute_wakes: Vec<PolicyRecomputeWake>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PolicyRecomputeWake {
    projection: SchedulingOutcomeProjection,
}

impl PolicyRecomputeWake {
    fn new(
        context: &PolicyRunContext,
        receipt: &RuntimeReceipt,
        execution: &PolicyExecutionEventData,
        projection: SchedulingOutcomeProjection,
    ) -> Result<Self, ActingdError> {
        let terminal = receipt
            .terminal()
            .ok_or_else(|| ActingdError::process("policy_recompute_terminal_missing"))?;
        let identity = projection.outcome().identity();
        if projection.ledger_position() < terminal.sequence
            || identity.terminal_event_id() != terminal.event_id
            || identity.terminal_sequence() != terminal.sequence
            || identity.instance_id() != context.lease_token().instance_id()
            || identity.task_id() != context.task_id()
            || identity.run_id() != context.run_id()
            || identity.request_id() != receipt.request_id()
            || identity.correlation_id() != context.correlation_id()
            || identity.lease_id() != context.lease_token().lease_id()
            || identity.decision_id() != context.decision_id()
            || identity.catalog_task_id() != execution.task_id
            || identity.instance_alias() != context.instance_alias()
            || execution.decision_id != context.decision_id()
            || execution.instance_id != context.instance_alias()
        {
            return Err(ActingdError::process(
                "policy_recompute_wake_identity_conflict",
            ));
        }
        Ok(Self { projection })
    }

    fn key(&self) -> PolicyRecomputeKey {
        let identity = self.projection.outcome().identity();
        PolicyRecomputeKey {
            run_id: identity.run_id(),
            terminal_event_id: identity.terminal_event_id(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct PolicyRecomputeKey {
    run_id: RunId,
    terminal_event_id: actingcommand_contract::EventId,
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
    initial_cycle: PolicyCycleExecution,
) -> Result<(), ActingdError> {
    let host = Arc::new(host);
    let policy = Arc::new(policy);
    let control = Arc::new(PolicyDriverControl::default());
    let setup = (|| {
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
        let cursor = SubscriptionCursor {
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
        Ok((client, cursor, driver))
    })();
    let (client, mut cursor, driver) = match setup {
        Ok(setup) => setup,
        Err(error) => {
            let error: ActingdError = error;
            let recorded =
                error.record_lifecycle_failure(&host, RuntimeLifecycleFailureStage::PolicyMonitor);
            let closed = close_policy_host(host);
            recorded?;
            closed?;
            return Err(error);
        }
    };

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

    let recorded = match &monitor_result {
        Err(error) => {
            error.record_lifecycle_failure(&host, RuntimeLifecycleFailureStage::PolicyMonitor)
        }
        Ok(()) => Ok(()),
    };
    let shutdown_result = control.shutdown();
    let driver_result = match driver.join() {
        Ok(result) => result,
        Err(_) => Err(ActingdError::process("policy_driver_panicked")),
    };
    drop(client);
    drop(policy);
    drop(control);
    let close_result = close_policy_host(host);
    let result = if close_result.as_ref().err().is_some_and(|error| {
        error
            .runtime
            .as_ref()
            .is_some_and(|error| error.operation() == "append_runtime_lifecycle_observed")
    }) {
        close_result
    } else {
        combine_monitor_results(monitor_result, shutdown_result, driver_result, close_result)
    };
    recorded.and(result)
}

fn close_policy_host(host: Arc<RuntimeHost>) -> Result<(), ActingdError> {
    match Arc::try_unwrap(host) {
        Ok(host) => host.close().map_err(ActingdError::runtime),
        Err(host) => {
            drop(host);
            Err(ActingdError::process("policy_driver_reference_leaked"))
        }
    }
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
    pending_recomputes: BTreeMap<PolicyRecomputeKey, PolicyRecomputeWake>,
    active_recomputes: BTreeMap<PolicyRecomputeKey, PolicyRecomputeWake>,
    completed_recomputes: BTreeMap<PolicyRecomputeKey, PolicyRecomputeWake>,
    shutdown: bool,
    #[cfg(test)]
    wait_predicate_checks: usize,
}

enum PolicyDriverWake {
    Trigger(PolicyTrigger, Vec<PolicyRecomputeWake>),
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

    fn notify_recompute(&self, wake: PolicyRecomputeWake) -> Result<(), ActingdError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ActingdError::process("policy_driver_signal_poisoned"))?;
        if state.shutdown {
            return Ok(());
        }
        let key = wake.key();
        if let Some(existing) = state.completed_recomputes.get(&key) {
            return if existing == &wake {
                Ok(())
            } else {
                Err(ActingdError::process(
                    "policy_recompute_wake_identity_conflict",
                ))
            };
        }
        if let Some(existing) = state.active_recomputes.get(&key) {
            return if existing == &wake {
                Ok(())
            } else {
                Err(ActingdError::process(
                    "policy_recompute_wake_identity_conflict",
                ))
            };
        }
        if let Some(existing) = state.pending_recomputes.get(&key)
            && existing != &wake
        {
            return Err(ActingdError::process(
                "policy_recompute_wake_identity_conflict",
            ));
        }
        if !state.pending_recomputes.contains_key(&key)
            && state.pending_recomputes.len() + state.active_recomputes.len() >= MAX_TASKS
        {
            return Err(ActingdError::process(
                "policy_recompute_wake_budget_exceeded",
            ));
        }
        state.pending_recomputes.insert(key, wake);
        state.pending_trigger = Some(
            state
                .pending_trigger
                .map_or(PolicyTrigger::FactsChanged, |pending| {
                    coalesce_policy_trigger(pending, PolicyTrigger::FactsChanged)
                }),
        );
        self.changed.notify_one();
        Ok(())
    }

    fn complete_recomputes(&self, recomputes: &[PolicyRecomputeWake]) -> Result<(), ActingdError> {
        let completed = recompute_wakes_by_key(recomputes.iter().cloned())?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| ActingdError::process("policy_driver_signal_poisoned"))?;
        for (key, wake) in &completed {
            if state
                .completed_recomputes
                .get(key)
                .is_some_and(|existing| existing != wake)
                || state
                    .active_recomputes
                    .get(key)
                    .is_some_and(|existing| existing != wake)
                || state
                    .pending_recomputes
                    .get(key)
                    .is_some_and(|existing| existing != wake)
            {
                return Err(ActingdError::process(
                    "policy_recompute_wake_identity_conflict",
                ));
            }
        }
        for (key, wake) in completed {
            state.active_recomputes.remove(&key);
            state.pending_recomputes.remove(&key);
            state.completed_recomputes.insert(key, wake);
        }
        while state.completed_recomputes.len() > MAX_TASKS {
            state.completed_recomputes.pop_first();
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
        if let Some((trigger, recomputes)) = take_policy_driver_work(&mut state)? {
            return Ok(PolicyDriverWake::Trigger(trigger, recomputes));
        }
        if timeout.is_zero() {
            return Ok(PolicyDriverWake::TimedOut);
        }
        let (mut state, wait) = self
            .changed
            .wait_timeout_while(state, timeout, |state| {
                #[cfg(test)]
                {
                    state.wait_predicate_checks += 1;
                    self.changed.notify_all();
                }
                !state.shutdown && state.pending_trigger.is_none()
            })
            .map_err(|_| ActingdError::process("policy_driver_signal_poisoned"))?;
        if state.shutdown {
            Ok(PolicyDriverWake::Shutdown)
        } else if let Some((trigger, recomputes)) = take_policy_driver_work(&mut state)? {
            Ok(PolicyDriverWake::Trigger(trigger, recomputes))
        } else if wait.timed_out() {
            Ok(PolicyDriverWake::TimedOut)
        } else {
            Err(ActingdError::process("policy_driver_wake_missing"))
        }
    }
}

fn take_policy_driver_work(
    state: &mut PolicyDriverSignal,
) -> Result<Option<(PolicyTrigger, Vec<PolicyRecomputeWake>)>, ActingdError> {
    let Some(trigger) = state.pending_trigger.take() else {
        if state.pending_recomputes.is_empty() {
            return Ok(None);
        }
        return Err(ActingdError::process("policy_recompute_trigger_missing"));
    };
    let pending = state
        .pending_recomputes
        .values()
        .cloned()
        .collect::<Vec<_>>();
    merge_recompute_wakes(&mut state.active_recomputes, pending)?;
    let recomputes = std::mem::take(&mut state.pending_recomputes);
    Ok(Some((trigger, recomputes.into_values().collect())))
}

fn recompute_wakes_by_key(
    recomputes: impl IntoIterator<Item = PolicyRecomputeWake>,
) -> Result<BTreeMap<PolicyRecomputeKey, PolicyRecomputeWake>, ActingdError> {
    let mut keyed = BTreeMap::new();
    for wake in recomputes {
        let key = wake.key();
        if keyed
            .get(&key)
            .is_some_and(|existing: &PolicyRecomputeWake| existing != &wake)
        {
            return Err(ActingdError::process(
                "policy_recompute_wake_identity_conflict",
            ));
        }
        keyed.insert(key, wake);
        if keyed.len() > MAX_TASKS {
            return Err(ActingdError::process(
                "policy_recompute_wake_budget_exceeded",
            ));
        }
    }
    Ok(keyed)
}

fn merge_recompute_wakes(
    target: &mut BTreeMap<PolicyRecomputeKey, PolicyRecomputeWake>,
    recomputes: impl IntoIterator<Item = PolicyRecomputeWake>,
) -> Result<(), ActingdError> {
    let additions = recompute_wakes_by_key(recomputes)?;
    for (key, wake) in &additions {
        if target.get(key).is_some_and(|existing| existing != wake) {
            return Err(ActingdError::process(
                "policy_recompute_wake_identity_conflict",
            ));
        }
    }
    let new_keys = additions
        .keys()
        .filter(|key| !target.contains_key(key))
        .count();
    if target.len() + new_keys > MAX_TASKS {
        return Err(ActingdError::process(
            "policy_recompute_wake_budget_exceeded",
        ));
    }
    target.extend(additions);
    Ok(())
}

struct DeferredPolicyTrigger {
    trigger: PolicyTrigger,
    recomputes: BTreeMap<PolicyRecomputeKey, PolicyRecomputeWake>,
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
            self.merge_deferred_work(trigger, Vec::new(), cycle.directive.eligible_at_unix_ms)?;
        } else {
            if self
                .deferred
                .as_ref()
                .is_none_or(|deferred| deferred.recomputes.is_empty())
            {
                self.deferred = None;
            }
            if cycle.directive.kind == PolicyRecomputeKind::Full {
                self.next_reconciliation_unix_ms =
                    checked_deadline(now_unix_ms, self.cadence.reconciliation_interval_ms)?;
            }
        }
        Ok(())
    }

    fn coalesce_deferred_work(
        &mut self,
        trigger: PolicyTrigger,
        recomputes: Vec<PolicyRecomputeWake>,
    ) -> Result<bool, ActingdError> {
        let Some(deferred) = self.deferred.as_mut() else {
            return Ok(false);
        };
        if trigger == PolicyTrigger::CatalogChanged {
            return Ok(false);
        }
        merge_recompute_wakes(&mut deferred.recomputes, recomputes)?;
        deferred.trigger = coalesce_policy_trigger(deferred.trigger, trigger);
        Ok(true)
    }

    fn defer_projection(
        &mut self,
        trigger: PolicyTrigger,
        recomputes: Vec<PolicyRecomputeWake>,
        now_unix_ms: u64,
    ) -> Result<(), ActingdError> {
        let eligible_at_unix_ms = checked_deadline(now_unix_ms, self.cadence.debounce_ms.max(1))?;
        self.merge_deferred_work(trigger, recomputes, eligible_at_unix_ms)
    }

    fn merge_deferred_work(
        &mut self,
        trigger: PolicyTrigger,
        recomputes: Vec<PolicyRecomputeWake>,
        eligible_at_unix_ms: u64,
    ) -> Result<(), ActingdError> {
        let pending = recompute_wakes_by_key(recomputes)?;
        match self.deferred.as_mut() {
            Some(existing) => {
                merge_recompute_wakes(&mut existing.recomputes, pending.into_values())?;
                existing.trigger = coalesce_policy_trigger(existing.trigger, trigger);
                existing.eligible_at_unix_ms =
                    existing.eligible_at_unix_ms.max(eligible_at_unix_ms);
            }
            None => {
                self.deferred = Some(DeferredPolicyTrigger {
                    trigger,
                    recomputes: pending,
                    eligible_at_unix_ms,
                });
            }
        }
        Ok(())
    }

    fn take_due_work(
        &mut self,
        now_unix_ms: u64,
    ) -> Result<Option<(PolicyTrigger, Vec<PolicyRecomputeWake>)>, ActingdError> {
        let previous = self.last_clock_observation_unix_ms;
        let delta = now_unix_ms.saturating_sub(previous);
        if now_unix_ms < previous || delta > self.cadence.clock_jump_threshold_ms {
            self.record_clock_observation(now_unix_ms)?;
            return Ok(Some((
                PolicyTrigger::ClockObserved {
                    previous_unix_ms: previous,
                },
                Vec::new(),
            )));
        }
        if now_unix_ms >= self.next_clock_observation_unix_ms {
            self.record_clock_observation(now_unix_ms)?;
        }
        if now_unix_ms >= self.next_reconciliation_unix_ms {
            return Ok(Some((PolicyTrigger::Reconciliation, Vec::new())));
        }
        if self
            .deferred
            .as_ref()
            .is_some_and(|deferred| now_unix_ms >= deferred.eligible_at_unix_ms)
        {
            return Ok(self.deferred.take().map(|deferred| {
                (
                    deferred.trigger,
                    deferred.recomputes.into_values().collect(),
                )
            }));
        }
        if self
            .next_wake_unix_ms
            .is_some_and(|next_wake| now_unix_ms >= next_wake)
        {
            self.next_wake_unix_ms = None;
            let previous_unix_ms = self.last_clock_observation_unix_ms;
            self.record_clock_observation(now_unix_ms)?;
            return Ok(Some((
                PolicyTrigger::ClockObserved { previous_unix_ms },
                Vec::new(),
            )));
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
    initial_cycle: PolicyCycleExecution,
) -> Result<(), ActingdError> {
    let mut schedule = PolicyDriverSchedule::new(
        policy.cadence.clone(),
        &initial_cycle.cycle,
        system_unix_ms()?,
    )?;
    for wake in initial_cycle.recompute_wakes {
        control.notify_recompute(wake)?;
    }
    loop {
        match control.wait(Duration::ZERO)? {
            PolicyDriverWake::Shutdown => return Ok(()),
            PolicyDriverWake::Trigger(trigger, recomputes) => {
                handle_policy_driver_trigger(
                    &host,
                    &policy,
                    &control,
                    &mut schedule,
                    trigger,
                    recomputes,
                )?;
                continue;
            }
            PolicyDriverWake::TimedOut => {}
        }

        let now = system_unix_ms()?;
        if let Some((trigger, recomputes)) = schedule.take_due_work(now)? {
            execute_ready_policy_trigger(
                &host,
                &policy,
                &control,
                &mut schedule,
                trigger,
                recomputes,
                now,
            )?;
            continue;
        }
        match control.wait(schedule.wait_duration(now))? {
            PolicyDriverWake::Shutdown => return Ok(()),
            PolicyDriverWake::Trigger(trigger, recomputes) => {
                handle_policy_driver_trigger(
                    &host,
                    &policy,
                    &control,
                    &mut schedule,
                    trigger,
                    recomputes,
                )?;
            }
            PolicyDriverWake::TimedOut => {}
        }
    }
}

fn handle_policy_driver_trigger(
    host: &RuntimeHost,
    policy: &PolicyBootstrap,
    control: &PolicyDriverControl,
    schedule: &mut PolicyDriverSchedule,
    mut trigger: PolicyTrigger,
    mut recomputes: Vec<PolicyRecomputeWake>,
) -> Result<(), ActingdError> {
    if schedule.coalesce_deferred_work(trigger, recomputes.clone())? {
        return Ok(());
    }
    let now = system_unix_ms()?;
    if let Some((due, due_recomputes)) = schedule.take_due_work(now)? {
        trigger = coalesce_policy_trigger(due, trigger);
        let mut keyed = recompute_wakes_by_key(recomputes)?;
        merge_recompute_wakes(&mut keyed, due_recomputes)?;
        recomputes = keyed.into_values().collect();
    }
    execute_ready_policy_trigger(host, policy, control, schedule, trigger, recomputes, now)
}

fn execute_ready_policy_trigger(
    host: &RuntimeHost,
    policy: &PolicyBootstrap,
    control: &PolicyDriverControl,
    schedule: &mut PolicyDriverSchedule,
    trigger: PolicyTrigger,
    recomputes: Vec<PolicyRecomputeWake>,
    now_unix_ms: u64,
) -> Result<(), ActingdError> {
    apply_policy_cycle_result(
        control,
        schedule,
        trigger,
        recomputes,
        now_unix_ms,
        execute_policy_cycle(host, policy, trigger),
    )
}

fn apply_policy_cycle_result(
    control: &PolicyDriverControl,
    schedule: &mut PolicyDriverSchedule,
    trigger: PolicyTrigger,
    recomputes: Vec<PolicyRecomputeWake>,
    now_unix_ms: u64,
    result: Result<PolicyCycleExecution, ActingdError>,
) -> Result<(), ActingdError> {
    match result {
        Ok(execution) => {
            let policy_consumed = execution.cycle.directive.kind != PolicyRecomputeKind::Deferred;
            let apply = (|| {
                schedule.apply_cycle(trigger, &execution.cycle, now_unix_ms)?;
                if !policy_consumed {
                    schedule.merge_deferred_work(
                        trigger,
                        recomputes.clone(),
                        execution.cycle.directive.eligible_at_unix_ms,
                    )?;
                }
                for wake in execution.recompute_wakes {
                    control.notify_recompute(wake)?;
                }
                Ok(())
            })();
            match apply {
                Ok(()) if policy_consumed => control.complete_recomputes(&recomputes),
                Ok(()) => Ok(()),
                Err(error) => Err(error),
            }
        }
        Err(error) if !recomputes.is_empty() && error.code == "outcome_projection_not_ready" => {
            schedule.defer_projection(trigger, recomputes, now_unix_ms)
        }
        Err(error) => Err(error),
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
    fn record_lifecycle_failure(
        &self,
        host: &RuntimeHost,
        stage: RuntimeLifecycleFailureStage,
    ) -> Result<(), ActingdError> {
        let failure = if let Some(error) = &self.runtime {
            RuntimeLifecycleFailure::Host(error)
        } else if let Some(error) = &self.client {
            RuntimeLifecycleFailure::Client {
                code: error.code(),
                operation: error.operation(),
                fatal: error.is_fatal(),
                runtime_code: error.projection().map(|projection| projection.code),
            }
        } else {
            RuntimeLifecycleFailure::Process { code: self.code }
        };
        host.record_lifecycle_failure(stage, failure)
            .map_err(ActingdError::runtime)
    }

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
        ApplicationLifecycleAction, ContainedTaskRequest, IdentifierIssuer, PolicyPayload,
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
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
            PolicyDriverWake::Trigger(PolicyTrigger::CatalogChanged, recomputes)
                if recomputes.is_empty()
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
    fn resident_driver_ignores_spurious_notification_until_a_real_wake() {
        let control = Arc::new(PolicyDriverControl::default());
        let finished = Arc::new(AtomicBool::new(false));
        let waiter = thread::spawn({
            let control = Arc::clone(&control);
            let finished = Arc::clone(&finished);
            move || {
                let result = control.wait(Duration::from_secs(30));
                finished.store(true, Ordering::SeqCst);
                control.changed.notify_all();
                result
            }
        });

        let state = control.state.lock().expect("driver signal");
        let (state, wait) = control
            .changed
            .wait_timeout_while(state, Duration::from_secs(5), |state| {
                state.wait_predicate_checks == 0 && !finished.load(Ordering::SeqCst)
            })
            .expect("observe waiter entering its predicate wait");
        assert!(!wait.timed_out(), "waiter did not enter its predicate wait");
        assert!(!finished.load(Ordering::SeqCst));
        let predicate_checks_before_notification = state.wait_predicate_checks;
        assert!(predicate_checks_before_notification > 0);

        control.changed.notify_all();
        let (state, wait) = control
            .changed
            .wait_timeout_while(state, Duration::from_secs(5), |state| {
                state.wait_predicate_checks <= predicate_checks_before_notification
                    && !finished.load(Ordering::SeqCst)
            })
            .expect("observe predicate recheck after spurious notification");
        assert!(
            !wait.timed_out(),
            "waiter did not recheck its predicate after notification"
        );
        let predicate_checks = state.wait_predicate_checks;
        let finished_early = finished.load(Ordering::SeqCst);
        drop(state);

        if finished_early {
            let result = waiter.join().expect("join early waiter");
            let code = match result {
                Ok(_) => "unexpected_policy_driver_wake",
                Err(error) => error.code,
            };
            panic!("spurious notification completed the wait early: {}", code);
        }
        assert!(predicate_checks > predicate_checks_before_notification);

        control.shutdown().expect("request driver shutdown");
        assert!(matches!(
            waiter.join().expect("join waiter").expect("wait result"),
            PolicyDriverWake::Shutdown
        ));
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
            ProcedureBinding::new(
                "procedure.followup",
                format!("sha256:{package_sha256}"),
                "operation.observe",
                vec!["after_observation".to_string()],
            )
            .expect("followup procedure binding"),
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
        let execution = initialize_policy(&host, &policy).expect("formal scheduled Recovery cycle");
        assert_eq!(execution.cycle.pending_dispatch_intents.len(), 1);
        let [wake] = execution.recompute_wakes.as_slice() else {
            panic!("one exact mapped-outcome wake")
        };
        assert_eq!(
            wake.projection.outcome().identity().catalog_task_id(),
            "fixture.observe"
        );
        assert_eq!(
            wake.projection.outcome().disposition().outcome_key(),
            "resident-completed"
        );
        let control = PolicyDriverControl::default();
        control
            .notify_recompute(wake.clone())
            .expect("first mapped-outcome wake");
        control
            .notify_recompute(wake.clone())
            .expect("idempotent mapped-outcome wake");
        let PolicyDriverWake::Trigger(trigger, recomputes) =
            control.wait(Duration::ZERO).expect("mapped-outcome work")
        else {
            panic!("mapped outcome must wake the resident driver")
        };
        assert_eq!(trigger, PolicyTrigger::FactsChanged);
        assert_eq!(recomputes, vec![wake.clone()]);
        let signal = control.state.lock().expect("driver signal");
        assert!(
            signal.completed_recomputes.is_empty(),
            "taking exact work must not mark it completed before policy consumption"
        );
        assert_eq!(
            signal.active_recomputes.get(&wake.key()),
            Some(wake),
            "the exact obligation must remain active until policy consumption succeeds"
        );
        drop(signal);
        let client = RuntimeClient::connect(
            RuntimeClientConfig::new(root.path(), EventActor::Agent, EventSource::Adapter)
                .with_io_timeout(Duration::from_secs(1)),
        )
        .expect("connect formal-provider runtime");
        let defer_now = 10_000;
        let mut projection_schedule = PolicyDriverSchedule::new(
            policy.cadence.clone(),
            &policy_cycle(PolicyRecomputeKind::Full, defer_now),
            defer_now,
        )
        .expect("projection schedule");
        apply_policy_cycle_result(
            &control,
            &mut projection_schedule,
            trigger,
            recomputes,
            defer_now,
            Err(ActingdError::process("outcome_projection_not_ready")),
        )
        .expect("first bounded not-ready defer");
        assert!(
            projection_schedule
                .take_due_work(defer_now)
                .expect("pre-deadline projection work")
                .is_none()
        );
        let (first_deferred_trigger, first_deferred_recomputes) = projection_schedule
            .take_due_work(defer_now + policy.cadence.debounce_ms)
            .expect("first projection deadline")
            .expect("first retained exact projection work");
        assert_eq!(first_deferred_trigger, PolicyTrigger::FactsChanged);
        assert_eq!(first_deferred_recomputes, vec![wake.clone()]);
        assert_eq!(
            client
                .query_events(
                    EventQuery {
                        event_type: Some(EventType::PolicyDispatchIntent),
                        ..EventQuery::default()
                    },
                    ProjectionProfile::Forensic,
                )
                .expect("query intents before projection readiness")
                .len(),
            1,
            "not-ready projection attempts must not dispatch a successor"
        );
        apply_policy_cycle_result(
            &control,
            &mut projection_schedule,
            first_deferred_trigger,
            first_deferred_recomputes,
            defer_now + policy.cadence.debounce_ms,
            Err(ActingdError::process("outcome_projection_not_ready")),
        )
        .expect("second bounded not-ready defer");
        let (second_deferred_trigger, second_deferred_recomputes) = projection_schedule
            .take_due_work(defer_now + (policy.cadence.debounce_ms * 2))
            .expect("second projection deadline")
            .expect("second retained exact projection work");
        assert_eq!(second_deferred_trigger, PolicyTrigger::FactsChanged);
        assert_eq!(second_deferred_recomputes, vec![wake.clone()]);
        assert_eq!(
            client
                .query_events(
                    EventQuery {
                        event_type: Some(EventType::PolicyDispatchIntent),
                        ..EventQuery::default()
                    },
                    ProjectionProfile::Forensic,
                )
                .expect("query intents after second not-ready result")
                .len(),
            1,
            "a second not-ready projection must still produce zero successor dispatches"
        );
        thread::sleep(Duration::from_millis(policy.cadence.cooldown_ms + 10));
        let ready = execute_policy_cycle(&host, &policy, second_deferred_trigger);
        assert_eq!(
            ready
                .as_ref()
                .expect("third projection attempt is ready")
                .cycle
                .pending_dispatch_intents
                .iter()
                .filter(|intent| intent.task_id == "fixture.followup")
                .count(),
            1,
            "the evaluator, not the wake, must decide exactly one successor"
        );
        apply_policy_cycle_result(
            &control,
            &mut projection_schedule,
            second_deferred_trigger,
            second_deferred_recomputes,
            defer_now + (policy.cadence.debounce_ms * 2),
            ready,
        )
        .expect("third cycle consumes the ready exact projection");
        control
            .notify_recompute(wake.clone())
            .expect("replayed completed exact wake");
        assert!(matches!(
            control.wait(Duration::ZERO).expect("deduplicated replay"),
            PolicyDriverWake::TimedOut
        ));
        let conflict = PolicyRecomputeWake {
            projection: SchedulingOutcomeProjection::new(
                wake.projection.ledger_position(),
                actingcommand_contract::AuthoritativeSchedulingOutcome::new(
                    wake.projection.outcome().identity().clone(),
                    actingcommand_contract::SchedulingDisposition::new(
                        "resident-conflict",
                        wake.projection.outcome().disposition().effect().clone(),
                    )
                    .expect("conflicting disposition"),
                    wake.projection.outcome().terminal_timestamp_unix_ms(),
                )
                .expect("conflicting outcome"),
            )
            .expect("conflicting projection"),
        };
        assert_eq!(
            control
                .notify_recompute(conflict)
                .expect_err("same terminal identity cannot change outcome")
                .code,
            "policy_recompute_wake_identity_conflict"
        );
        assert_eq!(input_count.load(Ordering::SeqCst), 1);
        assert_eq!(capture_count.load(Ordering::SeqCst), 2);

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
            let expected = if event_type == EventType::PolicyDispatchIntent {
                2
            } else {
                1
            };
            assert_eq!(
                events
                    .iter()
                    .filter(|event| event.event_type == event_type)
                    .count(),
                expected,
                "{event_type:?}"
            );
        }
        let followup_intents = events
            .iter()
            .filter(|event| {
                matches!(
                    &event.payload,
                    ProjectionPayload::Full(payload)
                        if matches!(
                            payload.as_ref(),
                            EventPayload::Policy(PolicyPayload::DispatchIntent(payload))
                                if payload.task_id() == "fixture.followup"
                        )
                )
            })
            .count();
        assert_eq!(followup_intents, 1);
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
                .take_due_work(now + 1_000)
                .expect("reconciliation deadline"),
            Some((PolicyTrigger::Reconciliation, Vec::new()))
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
            schedule.take_due_work(now + 1_001).expect("clock jump"),
            Some((
                PolicyTrigger::ClockObserved {
                    previous_unix_ms: now
                },
                Vec::new()
            ))
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
        let mut followup = tasks["tasks"][0].clone();
        followup["id"] = json!("fixture.followup");
        followup["procedure_ref"] = json!("procedure.followup");
        followup["priority"] = json!(200);
        followup["trigger"] = json!({
            "kind": "any",
            "predicates": [
                {
                    "kind": "outcome",
                    "task_id": "fixture.observe",
                    "outcome_key": "resident-completed",
                    "comparison": "eq",
                    "value": {"type": "boolean", "value": true}
                },
                {
                    "kind": "outcome",
                    "task_id": "fixture.observe",
                    "outcome_key": "resident-no-effect",
                    "comparison": "eq",
                    "value": {"type": "boolean", "value": true}
                }
            ]
        });
        followup["feedback_stop"] = json!({
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
        followup["produces"] = json!([]);
        followup["instance_overrides"] = json!([]);
        tasks["tasks"]
            .as_array_mut()
            .expect("resident task array")
            .push(followup);
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
                    "scheduling_outcome":{
                        "designated_operation":"open_terminal",
                        "mappings":[
                            {
                                "outcome_key":"resident-completed",
                                "effect":"designated_effect_completed",
                                "terminal_pages":["terminal"]
                            },
                            {
                                "outcome_key":"resident-no-effect",
                                "effect":"no_designated_effect",
                                "terminal_pages":["terminal"]
                            }
                        ]
                    },
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
