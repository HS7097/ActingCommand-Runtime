// SPDX-License-Identifier: AGPL-3.0-only

use std::collections::HashSet;

use crate::source::SourceMap;
use crate::{
    ActivityProfile, CatalogBundle, CatalogDiagnostic, CatalogDiagnosticCode, ClockSchedule,
    ClockSource, Comparison, EffectDirection, FactValue, LoadProfile, MAX_ACTIVITY_PROFILES,
    MAX_APPROVAL_REFS, MAX_BUDGET_COUNT, MAX_CLOCK_DRIFT_MS, MAX_DST_OFFSET_MINUTES,
    MAX_EFFECTS_PER_TASK, MAX_FACT_MAX_AGE_MS, MAX_GOALS_PER_PROFILE, MAX_ID_BYTES,
    MAX_INSTANCE_OVERRIDES_PER_TASK, MAX_POOLS, MAX_PREDICATE_DEPTH, MAX_PREDICATE_NODES,
    MAX_REFERENCES_PER_TASK, MAX_TASKS, MAX_TEXT_BYTES, MAX_TIMELINE_EVENTS,
    MAX_UTC_OFFSET_MINUTES, MAX_WINDOWS_PER_PROFILE, MIN_DST_OFFSET_MINUTES,
    MIN_UTC_OFFSET_MINUTES, MetricRef, ObservationRef, PoolSpec, PredicateSpec, ResourceEffectSpec,
    SCHEDULING_SCHEMA_VERSION, ScopeSelector, TaskSpec,
};

pub(crate) struct CatalogSourceMaps<'a> {
    pub tasks: &'a SourceMap,
    pub pools: &'a SourceMap,
    pub activity: &'a SourceMap,
    pub timeline: &'a SourceMap,
}

pub(crate) fn validate_catalog(
    bundle: &CatalogBundle,
    maps: CatalogSourceMaps<'_>,
) -> Vec<CatalogDiagnostic> {
    let mut diagnostics = Vec::new();
    validate_descriptors(bundle, &maps, &mut diagnostics);

    let task_ids: HashSet<&str> = bundle
        .tasks
        .tasks
        .iter()
        .map(|task| task.id.as_str())
        .collect();
    let pool_ids: HashSet<&str> = bundle
        .pools
        .pools
        .iter()
        .map(|pool| pool.id.as_str())
        .collect();
    let descriptor = Some((
        bundle.tasks.catalog.catalog_id.as_str(),
        bundle.tasks.catalog.catalog_version,
    ));

    validate_tasks(
        bundle,
        maps.tasks,
        &task_ids,
        &pool_ids,
        descriptor,
        &mut diagnostics,
    );
    validate_pools(bundle, maps.pools, &task_ids, descriptor, &mut diagnostics);
    validate_activity(
        bundle,
        maps.activity,
        &task_ids,
        &pool_ids,
        descriptor,
        &mut diagnostics,
    );
    validate_timeline(bundle, maps.timeline, descriptor, &mut diagnostics);
    sort_diagnostics(&mut diagnostics);
    diagnostics
}

pub(crate) fn sort_diagnostics(diagnostics: &mut [CatalogDiagnostic]) {
    diagnostics.sort_by(|left, right| {
        (
            left.source.document,
            left.source.line,
            left.source.column,
            left.code,
            left.json_path.as_str(),
            left.reason.as_str(),
        )
            .cmp(&(
                right.source.document,
                right.source.line,
                right.source.column,
                right.code,
                right.json_path.as_str(),
                right.reason.as_str(),
            ))
    });
}

fn validate_descriptors(
    bundle: &CatalogBundle,
    maps: &CatalogSourceMaps<'_>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    let documents = [
        (
            bundle.tasks.schema_version.as_str(),
            &bundle.tasks.catalog,
            maps.tasks,
        ),
        (
            bundle.pools.schema_version.as_str(),
            &bundle.pools.catalog,
            maps.pools,
        ),
        (
            bundle.activity.schema_version.as_str(),
            &bundle.activity.catalog,
            maps.activity,
        ),
        (
            bundle.timeline.schema_version.as_str(),
            &bundle.timeline.catalog,
            maps.timeline,
        ),
    ];

    for (schema_version, catalog, map) in documents {
        let descriptor = Some((catalog.catalog_id.as_str(), catalog.catalog_version));
        if schema_version != SCHEDULING_SCHEMA_VERSION {
            let mut diagnostic = map.diagnostic(
                CatalogDiagnosticCode::UnsupportedSchemaVersion,
                "/schema_version",
                format!("unsupported scheduling schema version `{schema_version}`"),
                descriptor,
            );
            diagnostic.schema_version.0 = Some(schema_version.to_owned());
            diagnostics.push(diagnostic);
        }
        validate_identifier(
            map,
            "/catalog/catalog_id",
            &catalog.catalog_id,
            descriptor,
            diagnostics,
        );
        if catalog.catalog_version == 0 {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::LimitExceeded,
                "/catalog/catalog_version",
                "catalog_version must be greater than zero",
                descriptor,
            ));
        }
        if catalog.approval_refs.is_empty() {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::ApprovalMissing,
                "/catalog/approval_refs",
                "at least one approval reference is required",
                descriptor,
            ));
        }
        if catalog.approval_refs.len() > MAX_APPROVAL_REFS {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::LimitExceeded,
                "/catalog/approval_refs",
                format!("approval reference count exceeds {MAX_APPROVAL_REFS}"),
                descriptor,
            ));
        }
        let mut approvals = HashSet::new();
        for (index, approval) in catalog.approval_refs.iter().enumerate() {
            validate_reference(
                map,
                &format!("/catalog/approval_refs/{index}"),
                approval,
                descriptor,
                diagnostics,
            );
            if !approvals.insert(approval) {
                diagnostics.push(map.diagnostic(
                    CatalogDiagnosticCode::DuplicateId,
                    format!("/catalog/approval_refs/{index}"),
                    format!("duplicate approval reference `{approval}`"),
                    descriptor,
                ));
            }
        }
    }

    if !bundle.descriptors_match() {
        diagnostics.push(maps.tasks.diagnostic(
            CatalogDiagnosticCode::DescriptorMismatch,
            "/catalog",
            "all four documents must share one schema version and catalog descriptor",
            Some((
                bundle.tasks.catalog.catalog_id.as_str(),
                bundle.tasks.catalog.catalog_version,
            )),
        ));
    }
}

fn validate_tasks(
    bundle: &CatalogBundle,
    map: &SourceMap,
    task_ids: &HashSet<&str>,
    pool_ids: &HashSet<&str>,
    descriptor: Option<(&str, u64)>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    if bundle.tasks.tasks.is_empty() || bundle.tasks.tasks.len() > MAX_TASKS {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::LimitExceeded,
            "/tasks",
            format!("task count must be between 1 and {MAX_TASKS}"),
            descriptor,
        ));
    }

    let mut ids = HashSet::new();
    for (index, task) in bundle.tasks.tasks.iter().enumerate() {
        let path = format!("/tasks/{index}");
        validate_identifier(
            map,
            &format!("{path}/id"),
            &task.id,
            descriptor,
            diagnostics,
        );
        if !ids.insert(task.id.as_str()) {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::DuplicateId,
                format!("{path}/id"),
                format!("duplicate task id `{}`", task.id),
                descriptor,
            ));
        }
        validate_task(
            task,
            &path,
            map,
            task_ids,
            pool_ids,
            &bundle.pools.pools,
            descriptor,
            diagnostics,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_task(
    task: &TaskSpec,
    path: &str,
    map: &SourceMap,
    task_ids: &HashSet<&str>,
    pool_ids: &HashSet<&str>,
    pools: &[PoolSpec],
    descriptor: Option<(&str, u64)>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    validate_scope(
        map,
        &format!("{path}/scope"),
        &task.scope,
        descriptor,
        diagnostics,
    );
    validate_identifier(
        map,
        &format!("{path}/entrypoint/operation_id"),
        &task.entrypoint.operation_id,
        descriptor,
        diagnostics,
    );
    validate_identifier(
        map,
        &format!("{path}/procedure_ref"),
        &task.procedure_ref,
        descriptor,
        diagnostics,
    );
    validate_predicate(
        &task.trigger,
        &format!("{path}/trigger"),
        map,
        task_ids,
        pool_ids,
        descriptor,
        diagnostics,
    );
    validate_predicate(
        &task.feedback_stop,
        &format!("{path}/feedback_stop"),
        map,
        task_ids,
        pool_ids,
        descriptor,
        diagnostics,
    );
    validate_effects(
        &task.consumes,
        EffectDirection::Consume,
        &format!("{path}/consumes"),
        map,
        pool_ids,
        pools,
        descriptor,
        diagnostics,
    );
    validate_effects(
        &task.produces,
        EffectDirection::Produce,
        &format!("{path}/produces"),
        map,
        pool_ids,
        pools,
        descriptor,
        diagnostics,
    );
    if task.on_failure.retry_limit > 1000
        || task.on_failure.escalation_threshold == 0
        || task.on_failure.escalation_threshold > 1000
    {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::LimitExceeded,
            format!("{path}/on_failure"),
            "failure policy exceeds the frozen V1 bounds",
            descriptor,
        ));
    }
    if task.next_run_clamp_ms == 0
        || task.expected_duration_ms == 0
        || task.loop_budget.daily_limit == 0
        || task.loop_budget.window_iteration_limit == 0
        || task.loop_budget.max_runtime_ms == 0
    {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::LoopBudgetMissing,
            format!("{path}/loop_budget"),
            "task loops and runtime must have nonzero explicit bounds",
            descriptor,
        ));
    }
    if task.loop_budget.daily_limit > MAX_BUDGET_COUNT
        || task.loop_budget.window_iteration_limit > MAX_BUDGET_COUNT
    {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::LimitExceeded,
            format!("{path}/loop_budget"),
            format!("task loop counts cannot exceed {MAX_BUDGET_COUNT}"),
            descriptor,
        ));
    }
    if task.yield_points.len() > MAX_REFERENCES_PER_TASK {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::LimitExceeded,
            format!("{path}/yield_points"),
            format!("yield point count exceeds {MAX_REFERENCES_PER_TASK}"),
            descriptor,
        ));
    }
    let mut yield_points = HashSet::new();
    for (index, yield_point) in task.yield_points.iter().enumerate() {
        validate_identifier(
            map,
            &format!("{path}/yield_points/{index}"),
            yield_point,
            descriptor,
            diagnostics,
        );
        if !yield_points.insert(yield_point) {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::DuplicateId,
                format!("{path}/yield_points/{index}"),
                format!("duplicate yield point `{yield_point}`"),
                descriptor,
            ));
        }
    }
    validate_load_profile(
        &task.load_profile,
        &format!("{path}/load_profile"),
        map,
        descriptor,
        diagnostics,
    );
    if task.strategic_weight_milli > 10_000 {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::LimitExceeded,
            format!("{path}/strategic_weight_milli"),
            "strategic weight exceeds 10000",
            descriptor,
        ));
    }
    if task.instance_overrides.len() > MAX_INSTANCE_OVERRIDES_PER_TASK {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::LimitExceeded,
            format!("{path}/instance_overrides"),
            format!("instance override count exceeds {MAX_INSTANCE_OVERRIDES_PER_TASK}"),
            descriptor,
        ));
    }
    let mut instances = HashSet::new();
    for (index, override_spec) in task.instance_overrides.iter().enumerate() {
        let override_path = format!("{path}/instance_overrides/{index}");
        validate_identifier(
            map,
            &format!("{override_path}/instance_id"),
            &override_spec.instance_id,
            descriptor,
            diagnostics,
        );
        if !instances.insert(override_spec.instance_id.as_str()) {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::DuplicateId,
                format!("{override_path}/instance_id"),
                format!(
                    "duplicate instance override `{}`",
                    override_spec.instance_id
                ),
                descriptor,
            ));
        }
        if let Some(weight) = override_spec.strategic_weight_milli.0
            && weight > 10_000
        {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::LimitExceeded,
                format!("{override_path}/strategic_weight_milli"),
                "override strategic weight exceeds 10000",
                descriptor,
            ));
        }
        if let Some(profile) = &override_spec.load_profile.0 {
            validate_load_profile(
                profile,
                &format!("{override_path}/load_profile"),
                map,
                descriptor,
                diagnostics,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_effects(
    effects: &[ResourceEffectSpec],
    expected_direction: EffectDirection,
    path: &str,
    map: &SourceMap,
    pool_ids: &HashSet<&str>,
    pools: &[PoolSpec],
    descriptor: Option<(&str, u64)>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    if effects.len() > MAX_EFFECTS_PER_TASK {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::LimitExceeded,
            path,
            format!("effect count exceeds {MAX_EFFECTS_PER_TASK}"),
            descriptor,
        ));
    }
    for (index, effect) in effects.iter().enumerate() {
        let effect_path = format!("{path}/{index}");
        if effect.direction != expected_direction {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::EffectIncompatible,
                format!("{effect_path}/direction"),
                format!("effect direction must be {expected_direction:?}"),
                descriptor,
            ));
        }
        if !pool_ids.contains(effect.pool_id.as_str()) {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::DanglingReference,
                format!("{effect_path}/pool_id"),
                format!("referenced pool `{}` does not exist", effect.pool_id),
                descriptor,
            ));
        }
        if effect.amount == 0 || effect.confidence_milli > 1000 {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::LimitExceeded,
                effect_path.clone(),
                "effect amount and confidence are outside V1 bounds",
                descriptor,
            ));
        }
        if let Some(pool) = pools.iter().find(|pool| pool.id == effect.pool_id)
            && effect.amount > pool.capacity
        {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::EffectIncompatible,
                format!("{effect_path}/amount"),
                format!(
                    "effect amount {} exceeds pool capacity {}",
                    effect.amount, pool.capacity
                ),
                descriptor,
            ));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_predicate(
    predicate: &PredicateSpec,
    path: &str,
    map: &SourceMap,
    task_ids: &HashSet<&str>,
    pool_ids: &HashSet<&str>,
    descriptor: Option<(&str, u64)>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    let mut node_count = 0_usize;
    validate_predicate_node(
        predicate,
        path,
        1,
        &mut node_count,
        map,
        task_ids,
        pool_ids,
        descriptor,
        diagnostics,
    );
}

#[allow(clippy::too_many_arguments)]
fn validate_predicate_node(
    predicate: &PredicateSpec,
    path: &str,
    depth: usize,
    node_count: &mut usize,
    map: &SourceMap,
    task_ids: &HashSet<&str>,
    pool_ids: &HashSet<&str>,
    descriptor: Option<(&str, u64)>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    *node_count += 1;
    if depth > MAX_PREDICATE_DEPTH || *node_count > MAX_PREDICATE_NODES {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::LimitExceeded,
            path,
            format!(
                "predicate exceeds depth {MAX_PREDICATE_DEPTH} or node count {MAX_PREDICATE_NODES}"
            ),
            descriptor,
        ));
        return;
    }

    match predicate {
        PredicateSpec::All { predicates } | PredicateSpec::Any { predicates } => {
            if predicates.is_empty() || predicates.len() > MAX_REFERENCES_PER_TASK {
                diagnostics.push(map.diagnostic(
                    CatalogDiagnosticCode::PredicateUnreachable,
                    format!("{path}/predicates"),
                    "boolean predicate group must contain a bounded nonempty list",
                    descriptor,
                ));
            }
            if matches!(predicate, PredicateSpec::All { .. })
                && has_conflicting_equalities(predicates)
            {
                diagnostics.push(map.diagnostic(
                    CatalogDiagnosticCode::PredicateUnreachable,
                    path,
                    "all-group contains conflicting equality predicates",
                    descriptor,
                ));
            }
            for (index, child) in predicates.iter().enumerate() {
                validate_predicate_node(
                    child,
                    &format!("{path}/predicates/{index}"),
                    depth + 1,
                    node_count,
                    map,
                    task_ids,
                    pool_ids,
                    descriptor,
                    diagnostics,
                );
            }
        }
        PredicateSpec::Not { predicate } => validate_predicate_node(
            predicate,
            &format!("{path}/predicate"),
            depth + 1,
            node_count,
            map,
            task_ids,
            pool_ids,
            descriptor,
            diagnostics,
        ),
        PredicateSpec::Clock { schedule } => {
            validate_schedule(schedule, path, map, descriptor, diagnostics)
        }
        PredicateSpec::ResourceProjection {
            pool_id,
            comparison,
            ..
        } => {
            if !pool_ids.contains(pool_id.as_str()) {
                diagnostics.push(map.diagnostic(
                    CatalogDiagnosticCode::DanglingReference,
                    format!("{path}/pool_id"),
                    format!("referenced pool `{pool_id}` does not exist"),
                    descriptor,
                ));
            }
            if *comparison == Comparison::Contains {
                diagnostics.push(map.diagnostic(
                    CatalogDiagnosticCode::PredicateUncomputable,
                    format!("{path}/comparison"),
                    "resource projection cannot use string containment",
                    descriptor,
                ));
            }
        }
        PredicateSpec::Fact {
            scope,
            fact_key,
            comparison,
            value,
            max_age_ms,
        } => {
            validate_scope(
                map,
                &format!("{path}/scope"),
                scope,
                descriptor,
                diagnostics,
            );
            validate_identifier(
                map,
                &format!("{path}/fact_key"),
                fact_key,
                descriptor,
                diagnostics,
            );
            validate_comparison_value(*comparison, value, path, map, descriptor, diagnostics);
            if max_age_ms.is_some_and(|value| value == 0 || value > MAX_FACT_MAX_AGE_MS) {
                diagnostics.push(map.diagnostic(
                    CatalogDiagnosticCode::LimitExceeded,
                    format!("{path}/max_age_ms"),
                    format!("fact max_age_ms must be null or within 1..={MAX_FACT_MAX_AGE_MS}"),
                    descriptor,
                ));
            }
        }
        PredicateSpec::RecordDeadline {
            scope,
            fact_key,
            timestamp_field,
            within_ms,
            max_age_ms,
        } => {
            validate_scope(
                map,
                &format!("{path}/scope"),
                scope,
                descriptor,
                diagnostics,
            );
            validate_identifier(
                map,
                &format!("{path}/fact_key"),
                fact_key,
                descriptor,
                diagnostics,
            );
            validate_identifier(
                map,
                &format!("{path}/timestamp_field"),
                timestamp_field,
                descriptor,
                diagnostics,
            );
            if *within_ms == 0 || *within_ms > MAX_FACT_MAX_AGE_MS {
                diagnostics.push(map.diagnostic(
                    CatalogDiagnosticCode::LimitExceeded,
                    format!("{path}/within_ms"),
                    format!("record deadline within_ms must be within 1..={MAX_FACT_MAX_AGE_MS}"),
                    descriptor,
                ));
            }
            if max_age_ms.is_some_and(|value| value == 0 || value > MAX_FACT_MAX_AGE_MS) {
                diagnostics.push(map.diagnostic(
                    CatalogDiagnosticCode::LimitExceeded,
                    format!("{path}/max_age_ms"),
                    format!("fact max_age_ms must be null or within 1..={MAX_FACT_MAX_AGE_MS}"),
                    descriptor,
                ));
            }
        }
        PredicateSpec::DependencyCompleted {
            task_id,
            terminal_states,
        } => {
            if !task_ids.contains(task_id.as_str()) {
                diagnostics.push(map.diagnostic(
                    CatalogDiagnosticCode::DanglingReference,
                    format!("{path}/task_id"),
                    format!("referenced task `{task_id}` does not exist"),
                    descriptor,
                ));
            }
            let unique: HashSet<_> = terminal_states.iter().collect();
            if terminal_states.is_empty() || unique.len() != terminal_states.len() {
                diagnostics.push(map.diagnostic(
                    CatalogDiagnosticCode::PredicateUncomputable,
                    format!("{path}/terminal_states"),
                    "terminal state list must be nonempty and unique",
                    descriptor,
                ));
            }
        }
        PredicateSpec::Outcome {
            task_id,
            outcome_key,
            comparison,
            value,
        } => {
            if !task_ids.contains(task_id.as_str()) {
                diagnostics.push(map.diagnostic(
                    CatalogDiagnosticCode::DanglingReference,
                    format!("{path}/task_id"),
                    format!("referenced task `{task_id}` does not exist"),
                    descriptor,
                ));
            }
            validate_identifier(
                map,
                &format!("{path}/outcome_key"),
                outcome_key,
                descriptor,
                diagnostics,
            );
            validate_comparison_value(*comparison, value, path, map, descriptor, diagnostics);
        }
    }
}

fn has_conflicting_equalities(predicates: &[PredicateSpec]) -> bool {
    for (index, left) in predicates.iter().enumerate() {
        let PredicateSpec::Fact {
            scope: left_scope,
            fact_key: left_key,
            comparison: Comparison::Eq,
            value: left_value,
            ..
        } = left
        else {
            continue;
        };
        for right in &predicates[index + 1..] {
            if let PredicateSpec::Fact {
                scope: right_scope,
                fact_key: right_key,
                comparison: Comparison::Eq,
                value: right_value,
                ..
            } = right
                && left_scope == right_scope
                && left_key == right_key
                && left_value != right_value
            {
                return true;
            }
        }
    }
    false
}

fn validate_comparison_value(
    comparison: Comparison,
    value: &FactValue,
    path: &str,
    map: &SourceMap,
    descriptor: Option<(&str, u64)>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    let computable = match comparison {
        Comparison::Eq | Comparison::NotEq => true,
        Comparison::Contains => {
            matches!(value, FactValue::String(_))
                || matches!(value, FactValue::RecordList(records) if !records.is_empty())
        }
        Comparison::LessThan
        | Comparison::LessThanOrEqual
        | Comparison::GreaterThan
        | Comparison::GreaterThanOrEqual => matches!(
            value,
            FactValue::Integer(_) | FactValue::TimestampMs(_) | FactValue::DurationMs(_)
        ),
    };
    if !computable {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::PredicateUncomputable,
            format!("{path}/comparison"),
            "comparison is incompatible with the typed fact value",
            descriptor,
        ));
    }
    if let FactValue::String(text) = value
        && text.len() > MAX_TEXT_BYTES
    {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::LimitExceeded,
            format!("{path}/value/value"),
            format!("fact string exceeds {MAX_TEXT_BYTES} UTF-8 bytes"),
            descriptor,
        ));
    }
    if let FactValue::RecordList(records) = value {
        if records.len() > 256
            || records.iter().any(|record| record.len() > 64)
            || records.iter().flat_map(|record| record.iter()).any(|(key, value)| {
                key.is_empty()
                    || key.len() > MAX_TEXT_BYTES
                    || matches!(value, crate::FactScalar::String(text) if text.len() > MAX_TEXT_BYTES)
            })
        {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::LimitExceeded,
                format!("{path}/value/value"),
                "fact record list exceeds the bounded predicate surface",
                descriptor,
            ));
        }
        if comparison == Comparison::Contains && records.len() != 1 {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::PredicateUncomputable,
                format!("{path}/value/value"),
                "record-list contains requires exactly one expected record",
                descriptor,
            ));
        }
    }
}

fn validate_schedule(
    schedule: &ClockSchedule,
    path: &str,
    map: &SourceMap,
    descriptor: Option<(&str, u64)>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    let valid = match schedule {
        ClockSchedule::Interval {
            clock_source,
            every_ms,
            ..
        } => {
            validate_clock_source(clock_source, path, map, descriptor, diagnostics);
            *every_ms > 0
        }
        ClockSchedule::At { clock_source, .. } => {
            validate_clock_source(clock_source, path, map, descriptor, diagnostics);
            !matches!(clock_source, ClockSource::Local)
        }
        ClockSchedule::Daily {
            clock_source,
            minutes_of_day,
        } => {
            validate_clock_source(clock_source, path, map, descriptor, diagnostics);
            let unique: HashSet<_> = minutes_of_day.iter().collect();
            !matches!(clock_source, ClockSource::Local)
                && !minutes_of_day.is_empty()
                && minutes_of_day.len() <= 32
                && unique.len() == minutes_of_day.len()
                && minutes_of_day.iter().all(|minute| *minute <= 1439)
        }
        ClockSchedule::Weekly {
            clock_source,
            weekday,
            minute_of_day,
        } => {
            validate_clock_source(clock_source, path, map, descriptor, diagnostics);
            !matches!(clock_source, ClockSource::Local)
                && (1..=7).contains(weekday)
                && *minute_of_day <= 1439
        }
    };
    if !valid {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::PredicateUncomputable,
            path,
            "clock schedule is outside the frozen V1 bounds",
            descriptor,
        ));
    }
}

fn validate_clock_source(
    source: &ClockSource,
    path: &str,
    map: &SourceMap,
    descriptor: Option<(&str, u64)>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    let (timezone_id, reveal_source, utc_offset, dst_offset, drift) = match source {
        ClockSource::Local => return,
        ClockSource::Server {
            timezone_id,
            utc_offset_minutes,
            dst_offset_minutes,
            maintenance_drift_ms,
        } => (
            timezone_id,
            None,
            *utc_offset_minutes,
            *dst_offset_minutes,
            *maintenance_drift_ms,
        ),
        ClockSource::Reveal {
            reveal_source,
            timezone_id,
            utc_offset_minutes,
            dst_offset_minutes,
            maintenance_drift_ms,
        } => (
            timezone_id,
            Some(reveal_source),
            *utc_offset_minutes,
            *dst_offset_minutes,
            *maintenance_drift_ms,
        ),
    };
    validate_reference(
        map,
        &format!("{path}/clock_source/timezone_id"),
        timezone_id,
        descriptor,
        diagnostics,
    );
    if let Some(reveal_source) = reveal_source {
        validate_identifier(
            map,
            &format!("{path}/clock_source/reveal_source"),
            reveal_source,
            descriptor,
            diagnostics,
        );
    }
    if !(MIN_UTC_OFFSET_MINUTES..=MAX_UTC_OFFSET_MINUTES).contains(&utc_offset)
        || !(MIN_DST_OFFSET_MINUTES..=MAX_DST_OFFSET_MINUTES).contains(&dst_offset)
        || !(-MAX_CLOCK_DRIFT_MS..=MAX_CLOCK_DRIFT_MS).contains(&drift)
    {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::LimitExceeded,
            format!("{path}/clock_source"),
            "clock source offset or maintenance drift exceeds the frozen V1 bounds",
            descriptor,
        ));
    }
}

fn validate_pools(
    bundle: &CatalogBundle,
    map: &SourceMap,
    task_ids: &HashSet<&str>,
    descriptor: Option<(&str, u64)>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    if bundle.pools.pools.len() > MAX_POOLS {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::LimitExceeded,
            "/pools",
            format!("pool count exceeds {MAX_POOLS}"),
            descriptor,
        ));
    }
    let mut ids = HashSet::new();
    for (index, pool) in bundle.pools.pools.iter().enumerate() {
        let path = format!("/pools/{index}");
        validate_identifier(
            map,
            &format!("{path}/id"),
            &pool.id,
            descriptor,
            diagnostics,
        );
        if !ids.insert(pool.id.as_str()) {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::DuplicateId,
                format!("{path}/id"),
                format!("duplicate pool id `{}`", pool.id),
                descriptor,
            ));
        }
        validate_scope(
            map,
            &format!("{path}/scope"),
            &pool.scope,
            descriptor,
            diagnostics,
        );
        if pool.capacity == 0 || pool.projection.amount == 0 || pool.projection.per_ms == 0 {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::LimitExceeded,
                path.clone(),
                "pool capacity and regeneration projection must be nonzero",
                descriptor,
            ));
        }
        match &pool.observation {
            ObservationRef::Fact { fact_key } => validate_identifier(
                map,
                &format!("{path}/observation/fact_key"),
                fact_key,
                descriptor,
                diagnostics,
            ),
            ObservationRef::Outcome {
                task_id,
                outcome_key,
            } => {
                if !task_ids.contains(task_id.as_str()) {
                    diagnostics.push(map.diagnostic(
                        CatalogDiagnosticCode::DanglingReference,
                        format!("{path}/observation/task_id"),
                        format!("referenced task `{task_id}` does not exist"),
                        descriptor,
                    ));
                }
                validate_identifier(
                    map,
                    &format!("{path}/observation/outcome_key"),
                    outcome_key,
                    descriptor,
                    diagnostics,
                );
            }
        }
        if let Some(delay) = &pool.group_delay
            && delay.minimum_delay_ms > delay.maximum_delay_ms
        {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::LimitExceeded,
                format!("{path}/group_delay"),
                "minimum group delay cannot exceed maximum group delay",
                descriptor,
            ));
        }
    }
}

fn validate_activity(
    bundle: &CatalogBundle,
    map: &SourceMap,
    task_ids: &HashSet<&str>,
    pool_ids: &HashSet<&str>,
    descriptor: Option<(&str, u64)>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    if bundle.activity.profiles.len() > MAX_ACTIVITY_PROFILES {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::LimitExceeded,
            "/profiles",
            format!("activity profile count exceeds {MAX_ACTIVITY_PROFILES}"),
            descriptor,
        ));
    }
    let mut ids = HashSet::new();
    for (index, profile) in bundle.activity.profiles.iter().enumerate() {
        let path = format!("/profiles/{index}");
        validate_identifier(
            map,
            &format!("{path}/id"),
            &profile.id,
            descriptor,
            diagnostics,
        );
        if !ids.insert(profile.id.as_str()) {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::DuplicateId,
                format!("{path}/id"),
                format!("duplicate activity profile id `{}`", profile.id),
                descriptor,
            ));
        }
        validate_activity_profile(
            profile,
            &path,
            map,
            task_ids,
            pool_ids,
            descriptor,
            diagnostics,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_activity_profile(
    profile: &ActivityProfile,
    path: &str,
    map: &SourceMap,
    task_ids: &HashSet<&str>,
    pool_ids: &HashSet<&str>,
    descriptor: Option<(&str, u64)>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    validate_scope(
        map,
        &format!("{path}/scope"),
        &profile.scope,
        descriptor,
        diagnostics,
    );
    if profile.windows.is_empty() || profile.windows.len() > MAX_WINDOWS_PER_PROFILE {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::LimitExceeded,
            format!("{path}/windows"),
            format!("window count must be between 1 and {MAX_WINDOWS_PER_PROFILE}"),
            descriptor,
        ));
    }
    for (index, window) in profile.windows.iter().enumerate() {
        let unique: HashSet<_> = window.weekdays.iter().collect();
        if window.weekdays.is_empty()
            || window.weekdays.len() > 7
            || unique.len() != window.weekdays.len()
            || window.weekdays.iter().any(|day| !(1..=7).contains(day))
            || !(-840..=840).contains(&window.utc_offset_minutes)
            || window.start_minute_of_day > 1439
            || window.end_minute_of_day > 1439
        {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::LimitExceeded,
                format!("{path}/windows/{index}"),
                "activity window is outside the frozen V1 bounds",
                descriptor,
            ));
        }
    }
    if profile.daily_budget == 0
        || profile.max_window_iterations == 0
        || profile.session_max_ms == 0
        || profile.detection_budget.window_dispatch_limit == 0
        || profile.detection_budget.window_runtime_ms == 0
        || profile.detection_budget.expected_duration_ms == 0
        || profile.detection_budget.expected_duration_ms
            > profile.detection_budget.window_runtime_ms
        || profile.minimum_interval_ms > profile.maximum_interval_ms
    {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::LoopBudgetMissing,
            path,
            "activity budgets and intervals must be explicit and internally consistent",
            descriptor,
        ));
    }
    if profile.daily_budget > MAX_BUDGET_COUNT || profile.max_window_iterations > MAX_BUDGET_COUNT {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::LimitExceeded,
            path,
            format!("activity budget counts cannot exceed {MAX_BUDGET_COUNT}"),
            descriptor,
        ));
    }
    if profile.detection_budget.window_dispatch_limit > MAX_BUDGET_COUNT {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::LimitExceeded,
            format!("{path}/detection_budget/window_dispatch_limit"),
            format!("detection dispatch count cannot exceed {MAX_BUDGET_COUNT}"),
            descriptor,
        ));
    }
    if profile.importance_milli > 10_000 {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::LimitExceeded,
            format!("{path}/importance_milli"),
            "instance importance exceeds 10000",
            descriptor,
        ));
    }
    if profile.goals.len() > MAX_GOALS_PER_PROFILE {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::LimitExceeded,
            format!("{path}/goals"),
            format!("goal count exceeds {MAX_GOALS_PER_PROFILE}"),
            descriptor,
        ));
    }
    let mut goal_ids = HashSet::new();
    for (index, goal) in profile.goals.iter().enumerate() {
        let goal_path = format!("{path}/goals/{index}");
        validate_identifier(
            map,
            &format!("{goal_path}/id"),
            &goal.id,
            descriptor,
            diagnostics,
        );
        if !goal_ids.insert(goal.id.as_str()) {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::DuplicateId,
                format!("{goal_path}/id"),
                format!("duplicate goal id `{}`", goal.id),
                descriptor,
            ));
        }
        if goal.strategic_weight_milli > 10_000 {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::LimitExceeded,
                format!("{goal_path}/strategic_weight_milli"),
                "goal strategic weight exceeds 10000",
                descriptor,
            ));
        }
        validate_metric(
            &goal.metric,
            &format!("{goal_path}/metric"),
            map,
            task_ids,
            pool_ids,
            descriptor,
            diagnostics,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_metric(
    metric: &MetricRef,
    path: &str,
    map: &SourceMap,
    task_ids: &HashSet<&str>,
    pool_ids: &HashSet<&str>,
    descriptor: Option<(&str, u64)>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    match metric {
        MetricRef::Fact { fact_key } => {
            validate_identifier(map, path, fact_key, descriptor, diagnostics)
        }
        MetricRef::Pool { pool_id } => {
            if !pool_ids.contains(pool_id.as_str()) {
                diagnostics.push(map.diagnostic(
                    CatalogDiagnosticCode::DanglingReference,
                    format!("{path}/pool_id"),
                    format!("referenced pool `{pool_id}` does not exist"),
                    descriptor,
                ));
            }
        }
        MetricRef::Outcome {
            task_id,
            outcome_key,
        } => {
            if !task_ids.contains(task_id.as_str()) {
                diagnostics.push(map.diagnostic(
                    CatalogDiagnosticCode::DanglingReference,
                    format!("{path}/task_id"),
                    format!("referenced task `{task_id}` does not exist"),
                    descriptor,
                ));
            }
            validate_identifier(
                map,
                &format!("{path}/outcome_key"),
                outcome_key,
                descriptor,
                diagnostics,
            );
        }
    }
}

fn validate_timeline(
    bundle: &CatalogBundle,
    map: &SourceMap,
    descriptor: Option<(&str, u64)>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    if bundle.timeline.events.len() > MAX_TIMELINE_EVENTS {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::LimitExceeded,
            "/events",
            format!("timeline event count exceeds {MAX_TIMELINE_EVENTS}"),
            descriptor,
        ));
    }
    let mut ids = HashSet::new();
    for (index, event) in bundle.timeline.events.iter().enumerate() {
        let path = format!("/events/{index}");
        validate_identifier(
            map,
            &format!("{path}/id"),
            &event.id,
            descriptor,
            diagnostics,
        );
        if !ids.insert(event.id.as_str()) {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::DuplicateId,
                format!("{path}/id"),
                format!("duplicate timeline event id `{}`", event.id),
                descriptor,
            ));
        }
        validate_scope(
            map,
            &format!("{path}/scope"),
            &event.scope,
            descriptor,
            diagnostics,
        );
        validate_schedule(
            &event.schedule,
            &format!("{path}/schedule"),
            map,
            descriptor,
            diagnostics,
        );
        if event.invalidates_fact_prefixes.len() > MAX_REFERENCES_PER_TASK {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::LimitExceeded,
                format!("{path}/invalidates_fact_prefixes"),
                format!("fact prefix count exceeds {MAX_REFERENCES_PER_TASK}"),
                descriptor,
            ));
        }
        for (prefix_index, prefix) in event.invalidates_fact_prefixes.iter().enumerate() {
            validate_identifier(
                map,
                &format!("{path}/invalidates_fact_prefixes/{prefix_index}"),
                prefix,
                descriptor,
                diagnostics,
            );
        }
    }
}

fn validate_scope(
    map: &SourceMap,
    path: &str,
    scope: &ScopeSelector,
    descriptor: Option<(&str, u64)>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    let (suffix, id) = match scope {
        ScopeSelector::Instance { instance_id } => ("instance_id", instance_id),
        ScopeSelector::Server { server_id } => ("server_id", server_id),
        ScopeSelector::Game { game_id } => ("game_id", game_id),
    };
    validate_identifier(
        map,
        &format!("{path}/{suffix}"),
        id,
        descriptor,
        diagnostics,
    );
}

fn validate_load_profile(
    profile: &LoadProfile,
    path: &str,
    map: &SourceMap,
    descriptor: Option<(&str, u64)>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    if let LoadProfile::Weighted {
        cpu_milli,
        gpu_milli,
        io_milli,
    } = profile
        && (*cpu_milli > 1000 || *gpu_milli > 1000 || *io_milli > 1000)
    {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::LimitExceeded,
            path,
            "weighted load components cannot exceed 1000",
            descriptor,
        ));
    }
}

fn validate_identifier(
    map: &SourceMap,
    path: &str,
    value: &str,
    descriptor: Option<(&str, u64)>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    if value.is_empty()
        || value.len() > MAX_ID_BYTES
        || !value
            .bytes()
            .enumerate()
            .all(|(index, byte)| is_identifier_byte(index, byte))
    {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::TypeMismatch,
            path,
            format!("`{value}` is not a valid bounded identifier"),
            descriptor,
        ));
    }
}

fn validate_reference(
    map: &SourceMap,
    path: &str,
    value: &str,
    descriptor: Option<(&str, u64)>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    if value.is_empty()
        || value.len() > MAX_ID_BYTES
        || !value.bytes().enumerate().all(|(index, byte)| {
            is_identifier_byte(index, byte) || (index > 0 && matches!(byte, b'/' | b'#'))
        })
    {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::TypeMismatch,
            path,
            format!("`{value}` is not a valid bounded reference"),
            descriptor,
        ));
    }
}

fn is_identifier_byte(index: usize, byte: u8) -> bool {
    byte.is_ascii_lowercase()
        || byte.is_ascii_digit()
        || (index > 0 && matches!(byte, b'.' | b'_' | b':' | b'-'))
}
