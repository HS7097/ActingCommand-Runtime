// SPDX-License-Identifier: AGPL-3.0-only

//! Runtime-owned failure, activity-window, and loop-budget state.

use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::{
    PerformanceContext, PolicyActivitySample, PolicyAdmissionRecord, PolicyBudgetReceipt,
    PolicyExecutionEventData, PolicyExecutionOutcome, PolicyFailureClass, PolicyFailureDisposition,
    PolicyFailureRecord, RuntimeErrorCode,
};
use actingcommand_policy::{
    ActivityProfile, ActivityWindow, CompiledCatalog, DispatchIntent, FailureAction, TaskSpec,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const MILLIS_PER_MINUTE: i128 = 60_000;
const MILLIS_PER_DAY: i128 = 24 * 60 * MILLIS_PER_MINUTE;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyExecutionInput {
    Succeeded,
    Failed {
        error_code: String,
        class: PolicyFailureClass,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FailureStreak {
    error_code: String,
    class: PolicyFailureClass,
    count: u16,
    escalation_streak: u16,
}

#[derive(Default)]
pub(crate) struct PolicyControlState {
    task_daily: BTreeMap<(String, String, i64), u32>,
    task_window: BTreeMap<(String, String, String), u32>,
    task_runtime: BTreeMap<(String, String, String), u64>,
    activity_daily: BTreeMap<(String, String, i64), u32>,
    activity_window: BTreeMap<(String, String, String), u32>,
    activity_runtime: BTreeMap<(String, String, String), u64>,
    next_activity_eligible: BTreeMap<(String, String), u64>,
    failure_streaks: BTreeMap<(String, String), FailureStreak>,
}

impl PolicyControlState {
    pub(crate) fn preview_admission(
        &self,
        catalog: &CompiledCatalog,
        intent: &DispatchIntent,
        now_unix_ms: u64,
    ) -> RuntimeHostResult<PolicyAdmissionRecord> {
        let (task, profile) = task_and_profile(catalog, intent)?;
        let (local_day, window_index) = active_window(profile, now_unix_ms)?;
        let window_id = format!("{}:{local_day}:{window_index}", profile.id);
        let cadence_key = (intent.instance_id.clone(), profile.id.clone());
        if self
            .next_activity_eligible
            .get(&cadence_key)
            .is_some_and(|eligible| now_unix_ms < *eligible)
        {
            return Err(request(
                "policy_activity_interval_active",
                "reserve_policy_budget",
            ));
        }

        let seed = activity_seed(&intent.decision_id, &profile.id, &window_id);
        let interval_ms = sampled_interval(profile, seed)?;
        let next_eligible_unix_ms = now_unix_ms
            .checked_add(interval_ms)
            .ok_or_else(|| fatal("policy_activity_time_overflow", "reserve_policy_budget"))?;

        let task_daily_key = (
            intent.task_id.clone(),
            intent.instance_id.clone(),
            local_day,
        );
        let task_window_key = (
            intent.task_id.clone(),
            intent.instance_id.clone(),
            window_id.clone(),
        );
        let activity_daily_key = (profile.id.clone(), intent.instance_id.clone(), local_day);
        let activity_window_key = (
            profile.id.clone(),
            intent.instance_id.clone(),
            window_id.clone(),
        );

        let budget = PolicyBudgetReceipt {
            task_daily_used: next_count(
                self.task_daily.get(&task_daily_key).copied().unwrap_or(0),
                task.loop_budget.daily_limit,
            )?,
            task_daily_limit: task.loop_budget.daily_limit,
            task_window_used: next_count(
                self.task_window.get(&task_window_key).copied().unwrap_or(0),
                task.loop_budget.window_iteration_limit,
            )?,
            task_window_limit: task.loop_budget.window_iteration_limit,
            task_runtime_reserved_ms: next_runtime(
                self.task_runtime
                    .get(&task_window_key)
                    .copied()
                    .unwrap_or(0),
                intent.expected_duration_ms,
                task.loop_budget.max_runtime_ms,
            )?,
            task_runtime_limit_ms: task.loop_budget.max_runtime_ms,
            activity_daily_used: next_count(
                self.activity_daily
                    .get(&activity_daily_key)
                    .copied()
                    .unwrap_or(0),
                profile.daily_budget,
            )?,
            activity_daily_limit: profile.daily_budget,
            activity_window_used: next_count(
                self.activity_window
                    .get(&activity_window_key)
                    .copied()
                    .unwrap_or(0),
                profile.max_window_iterations,
            )?,
            activity_window_limit: profile.max_window_iterations,
            activity_runtime_reserved_ms: next_runtime(
                self.activity_runtime
                    .get(&activity_window_key)
                    .copied()
                    .unwrap_or(0),
                intent.expected_duration_ms,
                profile.session_max_ms,
            )?,
            activity_runtime_limit_ms: profile.session_max_ms,
        };
        Ok(PolicyAdmissionRecord {
            activity: PolicyActivitySample {
                profile_id: profile.id.clone(),
                local_day,
                window_id,
                admitted_at_unix_ms: now_unix_ms,
                seed,
                interval_ms,
                next_eligible_unix_ms,
            },
            budget,
        })
    }

    pub(crate) fn commit_admission(
        &mut self,
        catalog: &CompiledCatalog,
        intent: &DispatchIntent,
        admission: &PolicyAdmissionRecord,
    ) -> RuntimeHostResult<()> {
        let expected =
            self.preview_admission(catalog, intent, admission.activity.admitted_at_unix_ms)?;
        if &expected != admission {
            return Err(fatal(
                "policy_budget_receipt_mismatch",
                "commit_policy_budget",
            ));
        }
        let task_daily_key = (
            intent.task_id.clone(),
            intent.instance_id.clone(),
            admission.activity.local_day,
        );
        let task_window_key = (
            intent.task_id.clone(),
            intent.instance_id.clone(),
            admission.activity.window_id.clone(),
        );
        let activity_daily_key = (
            admission.activity.profile_id.clone(),
            intent.instance_id.clone(),
            admission.activity.local_day,
        );
        let activity_window_key = (
            admission.activity.profile_id.clone(),
            intent.instance_id.clone(),
            admission.activity.window_id.clone(),
        );
        self.task_daily
            .insert(task_daily_key, admission.budget.task_daily_used);
        self.task_window
            .insert(task_window_key.clone(), admission.budget.task_window_used);
        self.task_runtime
            .insert(task_window_key, admission.budget.task_runtime_reserved_ms);
        self.activity_daily
            .insert(activity_daily_key, admission.budget.activity_daily_used);
        self.activity_window.insert(
            activity_window_key.clone(),
            admission.budget.activity_window_used,
        );
        self.activity_runtime.insert(
            activity_window_key,
            admission.budget.activity_runtime_reserved_ms,
        );
        self.next_activity_eligible.insert(
            (
                intent.instance_id.clone(),
                admission.activity.profile_id.clone(),
            ),
            admission.activity.next_eligible_unix_ms,
        );
        Ok(())
    }

    pub(crate) fn preview_execution(
        &self,
        catalog: &CompiledCatalog,
        intent: &DispatchIntent,
        admission: &PolicyAdmissionRecord,
        observed_at_unix_ms: u64,
        input: &PolicyExecutionInput,
        perf_context: &PerformanceContext,
    ) -> RuntimeHostResult<PolicyExecutionEventData> {
        let (task, _) = task_and_profile(catalog, intent)?;
        let runtime_ms = observed_at_unix_ms
            .checked_sub(admission.activity.admitted_at_unix_ms)
            .ok_or_else(|| request("policy_execution_time_reversed", "classify_policy_outcome"))?;
        let (task_runtime_used_ms, activity_runtime_used_ms) =
            actual_runtime_totals(intent, admission, runtime_ms)?;
        let runtime_exceeded = task_runtime_used_ms > admission.budget.task_runtime_limit_ms
            || activity_runtime_used_ms > admission.budget.activity_runtime_limit_ms;
        let effective_input = if runtime_exceeded {
            PolicyExecutionInput::Failed {
                error_code: "policy_runtime_budget_exceeded".to_owned(),
                class: PolicyFailureClass::Severe,
            }
        } else {
            input.clone()
        };
        let outcome = match effective_input {
            PolicyExecutionInput::Succeeded => PolicyExecutionOutcome::Succeeded { runtime_ms },
            PolicyExecutionInput::Failed { error_code, class } => {
                let key = (intent.task_id.clone(), intent.instance_id.clone());
                let previous = self.failure_streaks.get(&key);
                let consecutive_same_error = match previous {
                    Some(previous)
                        if previous.error_code == error_code && previous.class == class =>
                    {
                        previous.count.checked_add(1).ok_or_else(|| {
                            fatal("policy_failure_count_overflow", "classify_policy_outcome")
                        })?
                    }
                    _ => 1,
                };
                let performance_tax_exempt = class == PolicyFailureClass::Recoverable
                    && !task.sensitive
                    && perf_context.pressure_observed();
                let escalation_streak = match previous {
                    Some(previous)
                        if previous.error_code == error_code && previous.class == class =>
                    {
                        if performance_tax_exempt {
                            previous.escalation_streak
                        } else {
                            previous.escalation_streak.checked_add(1).ok_or_else(|| {
                                fatal("policy_failure_count_overflow", "classify_policy_outcome")
                            })?
                        }
                    }
                    _ if performance_tax_exempt => 0,
                    _ => 1,
                };
                let effective_class = if class == PolicyFailureClass::Severe
                    || task.sensitive
                    || escalation_streak >= task.on_failure.escalation_threshold
                {
                    PolicyFailureClass::Severe
                } else {
                    PolicyFailureClass::Recoverable
                };
                let (disposition, retry_attempt, retry_at_unix_ms) =
                    if effective_class == PolicyFailureClass::Severe {
                        (PolicyFailureDisposition::PausedTask, 0, None)
                    } else if consecutive_same_error <= task.on_failure.retry_limit {
                        let backoff_ms = retry_backoff_ms(task, consecutive_same_error);
                        let retry_at =
                            observed_at_unix_ms.checked_add(backoff_ms).ok_or_else(|| {
                                fatal("policy_retry_time_overflow", "classify_policy_outcome")
                            })?;
                        (
                            PolicyFailureDisposition::RetryScheduled,
                            consecutive_same_error,
                            Some(retry_at),
                        )
                    } else {
                        let disposition = match task.on_failure.action {
                            FailureAction::Continue => PolicyFailureDisposition::Continue,
                            FailureAction::Pause => PolicyFailureDisposition::PausedTask,
                        };
                        (disposition, 0, None)
                    };
                PolicyExecutionOutcome::Failed {
                    failure: PolicyFailureRecord {
                        error_code,
                        reported_success: runtime_exceeded
                            && matches!(input, PolicyExecutionInput::Succeeded),
                        original_class: class,
                        effective_class,
                        consecutive_same_error,
                        escalation_streak,
                        performance_tax_exempt,
                        retry_attempt,
                        disposition,
                        retry_at_unix_ms,
                        runtime_ms,
                        sensitive: task.sensitive,
                        perf_context: Box::new(perf_context.clone()),
                    },
                }
            }
        };
        Ok(PolicyExecutionEventData {
            decision_id: intent.decision_id.clone(),
            task_id: intent.task_id.clone(),
            instance_id: intent.instance_id.clone(),
            observed_at_unix_ms,
            outcome,
        })
    }

    pub(crate) fn commit_execution(
        &mut self,
        catalog: &CompiledCatalog,
        intent: &DispatchIntent,
        admission: &PolicyAdmissionRecord,
        data: &PolicyExecutionEventData,
    ) -> RuntimeHostResult<()> {
        let input = match &data.outcome {
            PolicyExecutionOutcome::Succeeded { .. } => PolicyExecutionInput::Succeeded,
            PolicyExecutionOutcome::Failed { failure } if failure.reported_success => {
                PolicyExecutionInput::Succeeded
            }
            PolicyExecutionOutcome::Failed { failure } => PolicyExecutionInput::Failed {
                error_code: failure.error_code.clone(),
                class: failure.original_class,
            },
        };
        let perf_context = match &data.outcome {
            PolicyExecutionOutcome::Succeeded { .. } => PerformanceContext::unavailable(1),
            PolicyExecutionOutcome::Failed { failure } => failure.perf_context.as_ref().clone(),
        };
        let expected = self.preview_execution(
            catalog,
            intent,
            admission,
            data.observed_at_unix_ms,
            &input,
            &perf_context,
        )?;
        if &expected != data {
            return Err(fatal(
                "policy_execution_record_mismatch",
                "commit_policy_outcome",
            ));
        }
        let runtime_ms = match &data.outcome {
            PolicyExecutionOutcome::Succeeded { runtime_ms } => *runtime_ms,
            PolicyExecutionOutcome::Failed { failure } => failure.runtime_ms,
        };
        let (task_runtime_used_ms, activity_runtime_used_ms) =
            actual_runtime_totals(intent, admission, runtime_ms)?;
        let task_window_key = (
            intent.task_id.clone(),
            intent.instance_id.clone(),
            admission.activity.window_id.clone(),
        );
        let activity_window_key = (
            admission.activity.profile_id.clone(),
            intent.instance_id.clone(),
            admission.activity.window_id.clone(),
        );
        self.task_runtime
            .insert(task_window_key, task_runtime_used_ms);
        self.activity_runtime
            .insert(activity_window_key, activity_runtime_used_ms);
        let key = (intent.task_id.clone(), intent.instance_id.clone());
        match &data.outcome {
            PolicyExecutionOutcome::Succeeded { .. } => {
                self.failure_streaks.remove(&key);
            }
            PolicyExecutionOutcome::Failed { failure } => {
                self.failure_streaks.insert(
                    key,
                    FailureStreak {
                        error_code: failure.error_code.clone(),
                        class: failure.original_class,
                        count: failure.consecutive_same_error,
                        escalation_streak: failure.escalation_streak,
                    },
                );
            }
        }
        Ok(())
    }
}

fn task_and_profile<'a>(
    catalog: &'a CompiledCatalog,
    intent: &DispatchIntent,
) -> RuntimeHostResult<(&'a TaskSpec, &'a ActivityProfile)> {
    let task = catalog
        .catalog()
        .tasks
        .tasks
        .iter()
        .find(|task| task.id == intent.task_id)
        .ok_or_else(|| request("policy_task_missing", "resolve_policy_control"))?;
    let profile = catalog
        .catalog()
        .activity
        .profiles
        .iter()
        .find(|profile| profile.id == intent.prerequisites.activity_profile_id)
        .ok_or_else(|| request("policy_activity_profile_missing", "resolve_policy_control"))?;
    Ok((task, profile))
}

fn active_window(profile: &ActivityProfile, now_unix_ms: u64) -> RuntimeHostResult<(i64, usize)> {
    for (index, window) in profile.windows.iter().enumerate() {
        if let Some(local_day) = window_local_day(window, now_unix_ms)? {
            return Ok((local_day, index));
        }
    }
    Err(request(
        "policy_activity_window_closed",
        "reserve_policy_budget",
    ))
}

fn window_local_day(window: &ActivityWindow, now_unix_ms: u64) -> RuntimeHostResult<Option<i64>> {
    let local_ms = i128::from(now_unix_ms)
        .checked_add(i128::from(window.utc_offset_minutes) * MILLIS_PER_MINUTE)
        .ok_or_else(|| fatal("policy_activity_time_overflow", "resolve_activity_window"))?;
    let local_day = local_ms.div_euclid(MILLIS_PER_DAY);
    let minute_of_day = local_ms.rem_euclid(MILLIS_PER_DAY) / MILLIS_PER_MINUTE;
    let minute_of_day = u16::try_from(minute_of_day)
        .map_err(|_| fatal("policy_activity_time_invalid", "resolve_activity_window"))?;
    let current_weekday = weekday(local_day);
    let previous_day = local_day
        .checked_sub(1)
        .ok_or_else(|| fatal("policy_activity_day_overflow", "resolve_activity_window"))?;
    let active_day = if window.start_minute_of_day == window.end_minute_of_day {
        window
            .weekdays
            .contains(&current_weekday)
            .then_some(local_day)
    } else if window.start_minute_of_day < window.end_minute_of_day {
        (window.weekdays.contains(&current_weekday)
            && minute_of_day >= window.start_minute_of_day
            && minute_of_day < window.end_minute_of_day)
            .then_some(local_day)
    } else if minute_of_day >= window.start_minute_of_day
        && window.weekdays.contains(&current_weekday)
    {
        Some(local_day)
    } else if minute_of_day < window.end_minute_of_day
        && window.weekdays.contains(&weekday(previous_day))
    {
        Some(previous_day)
    } else {
        None
    };
    active_day
        .map(i64::try_from)
        .transpose()
        .map_err(|_| fatal("policy_activity_day_overflow", "resolve_activity_window"))
}

fn weekday(local_day: i128) -> u8 {
    ((local_day + 3).rem_euclid(7) + 1) as u8
}

fn activity_seed(decision_id: &str, profile_id: &str, window_id: &str) -> u64 {
    let digest = Sha256::digest(format!("{decision_id}\0{profile_id}\0{window_id}").as_bytes());
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    u64::from_be_bytes(bytes)
}

fn sampled_interval(profile: &ActivityProfile, seed: u64) -> RuntimeHostResult<u64> {
    let width = profile
        .maximum_interval_ms
        .checked_sub(profile.minimum_interval_ms)
        .and_then(|value| value.checked_add(1))
        .ok_or_else(|| {
            fatal(
                "policy_activity_interval_invalid",
                "sample_activity_interval",
            )
        })?;
    Ok(profile.minimum_interval_ms + seed % width)
}

fn next_count(current: u32, limit: u32) -> RuntimeHostResult<u32> {
    let next = current
        .checked_add(1)
        .ok_or_else(|| fatal("policy_budget_counter_overflow", "reserve_policy_budget"))?;
    if next > limit {
        return Err(request("policy_budget_exhausted", "reserve_policy_budget"));
    }
    Ok(next)
}

fn next_runtime(current: u64, reservation: u64, limit: u64) -> RuntimeHostResult<u64> {
    let next = current
        .checked_add(reservation)
        .ok_or_else(|| fatal("policy_budget_counter_overflow", "reserve_policy_budget"))?;
    if next > limit {
        return Err(request("policy_budget_exhausted", "reserve_policy_budget"));
    }
    Ok(next)
}

fn actual_runtime_totals(
    intent: &DispatchIntent,
    admission: &PolicyAdmissionRecord,
    runtime_ms: u64,
) -> RuntimeHostResult<(u64, u64)> {
    let task_before = admission
        .budget
        .task_runtime_reserved_ms
        .checked_sub(intent.expected_duration_ms)
        .ok_or_else(|| fatal("policy_budget_receipt_invalid", "classify_policy_outcome"))?;
    let activity_before = admission
        .budget
        .activity_runtime_reserved_ms
        .checked_sub(intent.expected_duration_ms)
        .ok_or_else(|| fatal("policy_budget_receipt_invalid", "classify_policy_outcome"))?;
    let task_total = task_before
        .checked_add(runtime_ms)
        .ok_or_else(|| fatal("policy_budget_counter_overflow", "classify_policy_outcome"))?;
    let activity_total = activity_before
        .checked_add(runtime_ms)
        .ok_or_else(|| fatal("policy_budget_counter_overflow", "classify_policy_outcome"))?;
    Ok((task_total, activity_total))
}

fn retry_backoff_ms(task: &TaskSpec, attempt: u16) -> u64 {
    let shift = u32::from(attempt.saturating_sub(1)).min(63);
    let multiplier = 1_u64.checked_shl(shift).unwrap_or(u64::MAX);
    task.on_failure
        .retry_backoff_ms
        .max(1)
        .saturating_mul(multiplier)
        .min(task.next_run_clamp_ms)
}

const fn request(code: &'static str, operation: &'static str) -> RuntimeHostError {
    RuntimeHostError::request(code, operation, RuntimeErrorCode::InvalidRequest)
}

const fn fatal(code: &'static str, operation: &'static str) -> RuntimeHostError {
    RuntimeHostError::fatal(code, operation, RuntimeErrorCode::RuntimeFatal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_contract::{
        PerformanceMonitorHealth, PerformancePressureKind, PerformancePressureRecord,
        PerformancePressureSeverity, PerformancePressureValue, PolicyExecutionOutcome,
        PolicyFailureDisposition,
    };
    use actingcommand_policy::{
        CatalogDocumentSource, CatalogSources, DispatchPrerequisites, LoadProfile, compile_catalog,
    };

    const NOW: u64 = 1_699_963_200_000;

    fn sources() -> CatalogSources {
        CatalogSources {
            tasks: CatalogDocumentSource::new(
                "memory://fixture/tasks.json",
                include_bytes!("../../../contracts/scheduling/examples/catalog-a/tasks.json")
                    .to_vec(),
            ),
            pools: CatalogDocumentSource::new(
                "memory://fixture/pools.json",
                include_bytes!("../../../contracts/scheduling/examples/catalog-a/pools.json")
                    .to_vec(),
            ),
            activity: CatalogDocumentSource::new(
                "memory://fixture/activity.json",
                include_bytes!("../../../contracts/scheduling/examples/catalog-a/activity.json")
                    .to_vec(),
            ),
            timeline: CatalogDocumentSource::new(
                "memory://fixture/timeline.json",
                include_bytes!("../../../contracts/scheduling/examples/catalog-a/timeline.json")
                    .to_vec(),
            ),
        }
    }

    fn catalog_with(
        task_mutator: impl FnOnce(&mut serde_json::Value),
        activity_mutator: impl FnOnce(&mut serde_json::Value),
    ) -> CompiledCatalog {
        let mut sources = sources();
        let mut tasks: serde_json::Value =
            serde_json::from_slice(&sources.tasks.bytes).expect("tasks JSON");
        task_mutator(&mut tasks);
        sources.tasks.bytes = serde_json::to_vec(&tasks).expect("tasks bytes");
        let mut activity: serde_json::Value =
            serde_json::from_slice(&sources.activity.bytes).expect("activity JSON");
        activity_mutator(&mut activity);
        sources.activity.bytes = serde_json::to_vec(&activity).expect("activity bytes");
        compile_catalog(&sources).expect("compiled catalog")
    }

    fn catalog() -> CompiledCatalog {
        catalog_with(|_| {}, |_| {})
    }

    fn unavailable(at_unix_ms: u64) -> PerformanceContext {
        PerformanceContext::unavailable(at_unix_ms)
    }

    fn pressured(at_unix_ms: u64) -> PerformanceContext {
        PerformanceContext {
            window_start_unix_ms: at_unix_ms.saturating_sub(30_000).max(1),
            window_end_unix_ms: at_unix_ms,
            health: PerformanceMonitorHealth::Healthy,
            sample_count: 1,
            unavailable_metrics: Vec::new(),
            pressures: vec![PerformancePressureRecord {
                kind: PerformancePressureKind::Cpu,
                severity: PerformancePressureSeverity::High,
                started_at_unix_ms: at_unix_ms.saturating_sub(1).max(1),
                last_observed_at_unix_ms: at_unix_ms,
                peak: PerformancePressureValue::Utilization {
                    basis_points: 9_500,
                },
            }],
            max_cpu_basis_points: Some(9_500),
            max_ram_basis_points: Some(4_000),
            disk_queue_depth_p95_milli: None,
            disk_latency_p95_micros: None,
            max_gpu_basis_points: None,
            max_frame_gap_ms: None,
            max_capture_latency_ms: None,
            max_recognition_latency_ms: None,
            max_action_effect_latency_ms: None,
            related_event_ids: Vec::new(),
        }
    }

    fn intent(catalog: &CompiledCatalog, suffix: u64) -> DispatchIntent {
        let task = &catalog.catalog().tasks.tasks[0];
        DispatchIntent {
            decision_id: format!("decision:fixture-{suffix}"),
            task_id: task.id.clone(),
            instance_id: "fixture-instance-a".to_owned(),
            operation_id: task.entrypoint.operation_id.clone(),
            procedure_ref: task.procedure_ref.clone(),
            catalog_hash: catalog.catalog_hash().to_owned(),
            catalog_version: catalog.catalog().tasks.catalog.catalog_version,
            input_ledger_position: 1,
            fact_snapshot_id: "snapshot:fixture-a".to_owned(),
            approval_refs: catalog.catalog().tasks.catalog.approval_refs.clone(),
            reason_chain_id: format!("reason:fixture-{suffix}"),
            expected_duration_ms: task.expected_duration_ms,
            load_profile: LoadProfile::Light,
            prerequisites: DispatchPrerequisites {
                fencing_required: true,
                evaluated_at_unix_ms: NOW,
                facts_fresh_until_unix_ms: None,
                activity_profile_id: "fixture-activity-a".to_owned(),
                daily_limit: task.loop_budget.daily_limit,
                window_iteration_limit: task.loop_budget.window_iteration_limit,
                max_runtime_ms: task.loop_budget.max_runtime_ms,
            },
        }
    }

    #[test]
    fn recoverable_failures_back_off_then_escalate_same_error() {
        let catalog = catalog();
        let mut state = PolicyControlState::default();
        for attempt in 1_u16..=3 {
            let intent = intent(&catalog, u64::from(attempt));
            let admission = state
                .preview_admission(&catalog, &intent, NOW)
                .expect("admission preview");
            let data = state
                .preview_execution(
                    &catalog,
                    &intent,
                    &admission,
                    NOW + 100,
                    &PolicyExecutionInput::Failed {
                        error_code: "transient.capture".to_owned(),
                        class: PolicyFailureClass::Recoverable,
                    },
                    &unavailable(NOW + 100),
                )
                .expect("failure classification");
            let PolicyExecutionOutcome::Failed { failure } = &data.outcome else {
                panic!("expected failure")
            };
            if attempt < 3 {
                assert_eq!(failure.effective_class, PolicyFailureClass::Recoverable);
                assert_eq!(
                    failure.disposition,
                    PolicyFailureDisposition::RetryScheduled
                );
                assert_eq!(failure.retry_attempt, attempt);
                assert!(failure.retry_at_unix_ms.is_some());
            } else {
                assert_eq!(failure.effective_class, PolicyFailureClass::Severe);
                assert_eq!(failure.disposition, PolicyFailureDisposition::PausedTask);
                assert_eq!(failure.retry_attempt, 0);
                assert!(failure.retry_at_unix_ms.is_none());
            }
            state
                .commit_execution(&catalog, &intent, &admission, &data)
                .expect("commit failure");
        }
    }

    #[test]
    fn performance_pressure_exempts_escalation_but_not_bounded_retry_count() {
        let catalog = catalog_with(
            |tasks| {
                tasks["tasks"][0]["on_failure"]["retry_limit"] = serde_json::json!(2);
                tasks["tasks"][0]["on_failure"]["escalation_threshold"] = serde_json::json!(2);
            },
            |_| {},
        );
        let mut state = PolicyControlState::default();
        for attempt in 1_u16..=3 {
            let intent = intent(&catalog, u64::from(attempt));
            let admission = state
                .preview_admission(&catalog, &intent, NOW)
                .expect("admission preview");
            let data = state
                .preview_execution(
                    &catalog,
                    &intent,
                    &admission,
                    NOW + 100,
                    &PolicyExecutionInput::Failed {
                        error_code: "transient.capture".to_owned(),
                        class: PolicyFailureClass::Recoverable,
                    },
                    &pressured(NOW + 100),
                )
                .expect("performance-associated failure");
            let PolicyExecutionOutcome::Failed { failure } = &data.outcome else {
                panic!("expected failure")
            };
            assert!(failure.performance_tax_exempt);
            assert_eq!(failure.consecutive_same_error, attempt);
            assert_eq!(failure.escalation_streak, 0);
            assert_eq!(failure.effective_class, PolicyFailureClass::Recoverable);
            if attempt <= 2 {
                assert_eq!(
                    failure.disposition,
                    PolicyFailureDisposition::RetryScheduled
                );
            } else {
                assert_eq!(failure.disposition, PolicyFailureDisposition::Continue);
                assert_eq!(failure.retry_attempt, 0);
                assert!(failure.retry_at_unix_ms.is_none());
            }
            state
                .commit_execution(&catalog, &intent, &admission, &data)
                .expect("commit failure");
        }
    }

    #[test]
    fn sensitive_failure_never_schedules_an_automatic_retry() {
        let catalog = catalog_with(
            |tasks| tasks["tasks"][0]["sensitive"] = serde_json::json!(true),
            |_| {},
        );
        let state = PolicyControlState::default();
        let intent = intent(&catalog, 1);
        let admission = state
            .preview_admission(&catalog, &intent, NOW)
            .expect("admission preview");
        let data = state
            .preview_execution(
                &catalog,
                &intent,
                &admission,
                NOW + 100,
                &PolicyExecutionInput::Failed {
                    error_code: "transient.capture".to_owned(),
                    class: PolicyFailureClass::Recoverable,
                },
                &unavailable(NOW + 100),
            )
            .expect("failure classification");
        let PolicyExecutionOutcome::Failed { failure } = data.outcome else {
            panic!("expected failure")
        };
        assert!(failure.sensitive);
        assert_eq!(failure.effective_class, PolicyFailureClass::Severe);
        assert_eq!(failure.disposition, PolicyFailureDisposition::PausedTask);
        assert!(failure.retry_at_unix_ms.is_none());
    }

    #[test]
    fn failure_streak_requires_the_same_class_and_error() {
        let catalog = catalog();
        let mut state = PolicyControlState::default();
        for (suffix, error_code, class) in [
            (1, "transient.capture", PolicyFailureClass::Recoverable),
            (2, "transient.network", PolicyFailureClass::Recoverable),
            (3, "transient.capture", PolicyFailureClass::Severe),
        ] {
            let intent = intent(&catalog, suffix);
            let admission = state
                .preview_admission(&catalog, &intent, NOW)
                .expect("admission preview");
            let data = state
                .preview_execution(
                    &catalog,
                    &intent,
                    &admission,
                    NOW + 100,
                    &PolicyExecutionInput::Failed {
                        error_code: error_code.to_owned(),
                        class,
                    },
                    &unavailable(NOW + 100),
                )
                .expect("failure classification");
            let PolicyExecutionOutcome::Failed { failure } = &data.outcome else {
                panic!("expected failure")
            };
            assert_eq!(failure.consecutive_same_error, 1);
            state
                .commit_execution(&catalog, &intent, &admission, &data)
                .expect("commit failure");
        }
    }

    #[test]
    fn zero_configured_backoff_still_schedules_a_bounded_delay() {
        let catalog = catalog_with(
            |tasks| {
                tasks["tasks"][0]["on_failure"]["retry_backoff_ms"] = serde_json::json!(0);
                tasks["tasks"][0]["next_run_clamp_ms"] = serde_json::json!(5);
            },
            |_| {},
        );
        let state = PolicyControlState::default();
        let intent = intent(&catalog, 1);
        let admission = state
            .preview_admission(&catalog, &intent, NOW)
            .expect("admission preview");
        let data = state
            .preview_execution(
                &catalog,
                &intent,
                &admission,
                NOW + 100,
                &PolicyExecutionInput::Failed {
                    error_code: "transient.capture".to_owned(),
                    class: PolicyFailureClass::Recoverable,
                },
                &unavailable(NOW + 100),
            )
            .expect("failure classification");
        let PolicyExecutionOutcome::Failed { failure } = data.outcome else {
            panic!("expected failure")
        };
        assert_eq!(failure.retry_at_unix_ms, Some(NOW + 101));
    }

    #[test]
    fn exhausted_retry_limit_uses_the_declared_continue_or_pause_action() {
        for (action, expected) in [
            ("continue", PolicyFailureDisposition::Continue),
            ("pause", PolicyFailureDisposition::PausedTask),
        ] {
            let catalog = catalog_with(
                |tasks| {
                    tasks["tasks"][0]["on_failure"]["action"] = serde_json::json!(action);
                    tasks["tasks"][0]["on_failure"]["retry_limit"] = serde_json::json!(1);
                    tasks["tasks"][0]["on_failure"]["escalation_threshold"] = serde_json::json!(5);
                },
                |_| {},
            );
            let mut state = PolicyControlState::default();
            for attempt in 1_u64..=2 {
                let intent = intent(&catalog, attempt);
                let admission = state
                    .preview_admission(&catalog, &intent, NOW)
                    .expect("admission preview");
                let data = state
                    .preview_execution(
                        &catalog,
                        &intent,
                        &admission,
                        NOW + 100,
                        &PolicyExecutionInput::Failed {
                            error_code: "transient.capture".to_owned(),
                            class: PolicyFailureClass::Recoverable,
                        },
                        &unavailable(NOW + 100),
                    )
                    .expect("failure classification");
                let PolicyExecutionOutcome::Failed { failure } = &data.outcome else {
                    panic!("expected failure")
                };
                if attempt == 1 {
                    assert_eq!(
                        failure.disposition,
                        PolicyFailureDisposition::RetryScheduled
                    );
                } else {
                    assert_eq!(failure.effective_class, PolicyFailureClass::Recoverable);
                    assert_eq!(failure.disposition, expected);
                    assert!(failure.retry_at_unix_ms.is_none());
                }
                state
                    .commit_execution(&catalog, &intent, &admission, &data)
                    .expect("commit failure");
            }
        }
    }

    #[test]
    fn runtime_owned_daily_budget_cannot_be_bypassed() {
        let catalog = catalog_with(
            |tasks| tasks["tasks"][0]["loop_budget"]["daily_limit"] = serde_json::json!(2),
            |activity| activity["profiles"][0]["daily_budget"] = serde_json::json!(2),
        );
        let mut state = PolicyControlState::default();
        for index in 0..2 {
            let now = NOW + index * 600_000;
            let intent = intent(&catalog, index + 1);
            let admission = state
                .preview_admission(&catalog, &intent, now)
                .expect("budget admission");
            state
                .commit_admission(&catalog, &intent, &admission)
                .expect("commit budget");
        }
        let error = state
            .preview_admission(&catalog, &intent(&catalog, 3), NOW + 1_200_000)
            .expect_err("third daily admission must fail");
        assert_eq!(error.code(), "policy_budget_exhausted");
    }

    #[test]
    fn runtime_owned_window_and_runtime_budgets_cannot_be_bypassed() {
        let window_catalog = catalog_with(
            |tasks| {
                tasks["tasks"][0]["loop_budget"]["daily_limit"] = serde_json::json!(10);
                tasks["tasks"][0]["loop_budget"]["window_iteration_limit"] = serde_json::json!(1);
            },
            |activity| {
                activity["profiles"][0]["daily_budget"] = serde_json::json!(10);
                activity["profiles"][0]["max_window_iterations"] = serde_json::json!(10);
            },
        );
        let mut window_state = PolicyControlState::default();
        let first = intent(&window_catalog, 1);
        let admission = window_state
            .preview_admission(&window_catalog, &first, NOW)
            .expect("first window admission");
        window_state
            .commit_admission(&window_catalog, &first, &admission)
            .expect("commit window budget");
        let error = window_state
            .preview_admission(&window_catalog, &intent(&window_catalog, 2), NOW + 600_000)
            .expect_err("second window iteration must fail");
        assert_eq!(error.code(), "policy_budget_exhausted");

        let runtime_catalog = catalog_with(
            |tasks| {
                tasks["tasks"][0]["loop_budget"]["daily_limit"] = serde_json::json!(10);
                tasks["tasks"][0]["loop_budget"]["window_iteration_limit"] = serde_json::json!(10);
                tasks["tasks"][0]["loop_budget"]["max_runtime_ms"] = serde_json::json!(100000);
            },
            |activity| {
                activity["profiles"][0]["daily_budget"] = serde_json::json!(10);
                activity["profiles"][0]["max_window_iterations"] = serde_json::json!(10);
            },
        );
        let mut runtime_state = PolicyControlState::default();
        let first = intent(&runtime_catalog, 1);
        let admission = runtime_state
            .preview_admission(&runtime_catalog, &first, NOW)
            .expect("first runtime admission");
        runtime_state
            .commit_admission(&runtime_catalog, &first, &admission)
            .expect("commit runtime budget");
        let error = runtime_state
            .preview_admission(
                &runtime_catalog,
                &intent(&runtime_catalog, 2),
                NOW + 600_000,
            )
            .expect_err("runtime reservation must remain bounded");
        assert_eq!(error.code(), "policy_budget_exhausted");
    }

    #[test]
    fn actual_runtime_replaces_the_reservation_before_the_next_admission() {
        let catalog = catalog_with(
            |tasks| {
                tasks["tasks"][0]["expected_duration_ms"] = serde_json::json!(30000);
                tasks["tasks"][0]["loop_budget"]["daily_limit"] = serde_json::json!(10);
                tasks["tasks"][0]["loop_budget"]["window_iteration_limit"] = serde_json::json!(10);
                tasks["tasks"][0]["loop_budget"]["max_runtime_ms"] = serde_json::json!(100000);
            },
            |activity| {
                activity["profiles"][0]["daily_budget"] = serde_json::json!(10);
                activity["profiles"][0]["max_window_iterations"] = serde_json::json!(10);
                activity["profiles"][0]["session_max_ms"] = serde_json::json!(100000);
            },
        );
        let mut state = PolicyControlState::default();
        let first = intent(&catalog, 1);
        let admission = state
            .preview_admission(&catalog, &first, NOW)
            .expect("first admission");
        state
            .commit_admission(&catalog, &first, &admission)
            .expect("commit first admission");
        let execution = state
            .preview_execution(
                &catalog,
                &first,
                &admission,
                NOW + 80000,
                &PolicyExecutionInput::Succeeded,
                &unavailable(NOW + 80000),
            )
            .expect("first execution");
        state
            .commit_execution(&catalog, &first, &admission, &execution)
            .expect("commit actual runtime");

        let error = state
            .preview_admission(&catalog, &intent(&catalog, 2), NOW + 600000)
            .expect_err("actual runtime must constrain the next admission");
        assert_eq!(error.code(), "policy_budget_exhausted");
    }

    #[test]
    fn activity_sample_is_stable_and_recorded_before_cadence_blocks_reentry() {
        let catalog = catalog();
        let mut state = PolicyControlState::default();
        let first_intent = intent(&catalog, 1);
        let first = state
            .preview_admission(&catalog, &first_intent, NOW)
            .expect("first sample");
        let replay = state
            .preview_admission(&catalog, &first_intent, NOW)
            .expect("same-round sample");
        assert_eq!(first, replay);
        assert!(first.activity.seed > 0);
        state
            .commit_admission(&catalog, &first_intent, &first)
            .expect("commit sample");
        let error = state
            .preview_admission(&catalog, &intent(&catalog, 2), NOW)
            .expect_err("cadence must block immediate resampling");
        assert_eq!(error.code(), "policy_activity_interval_active");
    }
}
