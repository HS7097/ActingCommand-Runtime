// SPDX-License-Identifier: AGPL-3.0-only

use std::collections::{HashMap, HashSet};

use actingcommand_contract::MAX_STRATEGIC_WEIGHT_MILLI;

use crate::source::SourceMap;
use crate::{
    ActivityProfile, CatalogBundle, CatalogDiagnostic, CatalogDiagnosticCode, ClockSchedule,
    ClockSource, Comparison, DiagnosticSeverity, EffectAmount, EffectDirection, FactComparand,
    FactReference, FactValue, LoadProfile, MAX_ACTIVITY_PROFILES, MAX_APPROVAL_REFS,
    MAX_BUDGET_COUNT, MAX_CANONICAL_INTEGER, MAX_CLOCK_DRIFT_MS, MAX_CONTINUED_TASK_IDS,
    MAX_DEFER_AGING_CAP_MS, MAX_DEFER_FOR_MS, MAX_DST_OFFSET_MINUTES, MAX_EFFECTS_PER_TASK,
    MAX_FACT_MAX_AGE_MS, MAX_FACT_TIME_OFFSET_MS, MAX_GOALS_PER_PROFILE, MAX_ID_BYTES,
    MAX_INSTANCE_OVERRIDES_PER_TASK, MAX_MISSED_GRACE_MS, MAX_POOLS, MAX_PREDICATE_DEPTH,
    MAX_PREDICATE_NODES, MAX_PRIORITY_PERCENTILE, MAX_QUEUE_WAIT_MS, MAX_REFERENCES_PER_TASK,
    MAX_TASKS, MAX_TEXT_BYTES, MAX_TIMELINE_EVENTS, MAX_UTC_OFFSET_MINUTES,
    MAX_VALUATION_BASE_WEIGHT_MILLI, MAX_VALUATION_GAP_WEIGHT_MILLI, MAX_VALUATION_NAME_BYTES,
    MAX_VALUATION_UNIT_BYTES, MAX_VALUE_MILLI, MAX_WINDOWS_PER_PROFILE, MIN_CANONICAL_INTEGER,
    MIN_DST_OFFSET_MINUTES, MIN_FACT_TIME_OFFSET_MS, MIN_MISSED_GRACE_MS, MIN_QUEUE_WAIT_MS,
    MIN_UTC_OFFSET_MINUTES, MetricRef, MissedSlotRule, ObservationRef, PoolCapacity, PoolLine,
    PoolProjection, PoolSpec, PoolValuation, PredicateSpec, ProjectionComparand,
    ResourceEffectSpec, SCHEDULING_SCHEMA_VERSION, SCHEDULING_SCHEMA_VERSION_V2, ScopeSelector,
    TaskSpec, TimelineDocument, TimelineEvent,
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
    has_selection_document: bool,
) -> Vec<CatalogDiagnostic> {
    let mut diagnostics = Vec::new();
    validate_descriptors(bundle, &maps, &mut diagnostics);
    validate_priority_selection(bundle, maps.tasks, has_selection_document, &mut diagnostics);

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

pub(crate) fn validate_schema_version(
    schema_version: &str,
    descriptor: Option<(&str, u64)>,
    map: &SourceMap,
) -> Option<CatalogDiagnostic> {
    if matches!(
        schema_version,
        SCHEDULING_SCHEMA_VERSION | SCHEDULING_SCHEMA_VERSION_V2
    ) {
        return None;
    }
    let mut diagnostic = map.diagnostic(
        CatalogDiagnosticCode::UnsupportedSchemaVersion,
        "/schema_version",
        format!("unsupported scheduling schema version `{schema_version}`"),
        descriptor,
    );
    diagnostic.schema_version.0 = Some(schema_version.to_owned());
    Some(diagnostic)
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
        if let Some(diagnostic) = validate_schema_version(schema_version, descriptor, map) {
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
        // ROUT-L RL-1a (model §3.5, R1): a V2 catalog may carry no approval; the standard-pack
        // composer writes none. V1 keeps its rule.
        if catalog.approval_refs.is_empty() && schema_version != SCHEDULING_SCHEMA_VERSION_V2 {
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

fn validate_priority_selection(
    bundle: &CatalogBundle,
    map: &SourceMap,
    has_selection_document: bool,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    let Some(selection) = &bundle.tasks.priority_selection else {
        return;
    };
    let descriptor = Some((
        bundle.tasks.catalog.catalog_id.as_str(),
        bundle.tasks.catalog.catalog_version,
    ));
    if !has_selection_document {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::PrioritySelectionWithoutDocument,
            "/priority_selection",
            "priority_selection requires a selection document in the same catalog",
            descriptor,
        ));
    }
    if selection.defer_below_milli >= selection.promote_above_milli {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::LimitExceeded,
            "/priority_selection",
            "defer_below_milli must be less than promote_above_milli",
            descriptor,
        ));
    }
    if selection.defer_for_ms == 0 || selection.defer_for_ms > MAX_DEFER_FOR_MS {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::LimitExceeded,
            "/priority_selection/defer_for_ms",
            format!("defer_for_ms must be within 1..={MAX_DEFER_FOR_MS}"),
            descriptor,
        ));
    }
    for (field, value) in [
        ("defer_below_milli", selection.defer_below_milli),
        ("promote_above_milli", selection.promote_above_milli),
    ] {
        if !(MIN_CANONICAL_INTEGER..=MAX_CANONICAL_INTEGER).contains(&value) {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::LimitExceeded,
                format!("/priority_selection/{field}"),
                "priority selection thresholds must be canonical safe integers",
                descriptor,
            ));
        }
    }
    if selection.aging_ms_per_milli == Some(0) {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::LimitExceeded,
            "/priority_selection/aging_ms_per_milli",
            "aging_ms_per_milli must be at least 1",
            descriptor,
        ));
    }
    if selection
        .defer_aging_cap_ms
        .is_some_and(|cap| cap == 0 || cap > MAX_DEFER_AGING_CAP_MS)
    {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::LimitExceeded,
            "/priority_selection/defer_aging_cap_ms",
            format!("defer_aging_cap_ms must be within 1..={MAX_DEFER_AGING_CAP_MS}"),
            descriptor,
        ));
    }
    let percentiles_valid = match (
        selection.defer_below_percentile,
        selection.promote_above_percentile,
    ) {
        (None, None) => true,
        (Some(defer), Some(promote)) => defer < promote && promote <= MAX_PRIORITY_PERCENTILE,
        _ => false,
    };
    if !percentiles_valid {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::PrioritySelectionPercentileInvalid,
            "/priority_selection",
            format!(
                "defer_below_percentile and promote_above_percentile must be declared together within 0..={MAX_PRIORITY_PERCENTILE} with defer_below_percentile less than promote_above_percentile"
            ),
            descriptor,
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
    // ROUT-L RL-1a (model §3.6): a V2 catalog may hold no task, the composition of standard
    // packs without rules. V1 keeps its rule.
    let v2 = map.schema_version.as_deref() == Some(SCHEDULING_SCHEMA_VERSION_V2);
    let minimum = usize::from(!v2);
    if bundle.tasks.tasks.len() < minimum || bundle.tasks.tasks.len() > MAX_TASKS {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::LimitExceeded,
            "/tasks",
            format!("task count must be between {minimum} and {MAX_TASKS}"),
            descriptor,
        ));
    }

    let mut ids = HashSet::new();
    for (index, task) in bundle.tasks.tasks.iter().enumerate() {
        let path = format!("/tasks/{index}");
        validate_item_identifier(
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
            &bundle.timeline,
            descriptor,
            diagnostics,
        );
        // Workflow #411 B8: `dispatch_parameters` is a V2 member; a V1 document that names it is
        // refused as an unknown member.
        if task.dispatch_parameters.is_some()
            && map.schema_version.as_deref() != Some(SCHEDULING_SCHEMA_VERSION_V2)
        {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::UnknownField,
                format!("{path}/dispatch_parameters"),
                "dispatch_parameters requires actingcommand.scheduling.v2",
                descriptor,
            ));
        }
        // SS-2b (register §1.225): `benefit_judged` is a V2 member; a V1 document that names it
        // is refused as an unknown member.
        if task.benefit_judged.is_some()
            && map.schema_version.as_deref() != Some(SCHEDULING_SCHEMA_VERSION_V2)
        {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::UnknownField,
                format!("{path}/benefit_judged"),
                "benefit_judged requires actingcommand.scheduling.v2",
                descriptor,
            ));
        }
        validate_reserves(
            task,
            &path,
            map,
            &bundle.pools.pools,
            descriptor,
            diagnostics,
        );
        validate_realization(
            task,
            &path,
            map,
            &bundle.pools.pools,
            descriptor,
            diagnostics,
        );
        validate_limited(task, &path, map, descriptor, diagnostics);
        validate_fixed_value(
            task,
            &path,
            map,
            &bundle.pools.pools,
            descriptor,
            diagnostics,
        );
        validate_queue_wait(task, &path, map, descriptor, diagnostics);
        // Workflow #455 G-R2s (register §1.369): the composer's irreversible mark is a V2
        // member; a V1 document that names it is refused as an unknown member.
        if task.irreversible_reach.is_some()
            && map.schema_version.as_deref() != Some(SCHEDULING_SCHEMA_VERSION_V2)
        {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::UnknownField,
                format!("{path}/{}", crate::IRREVERSIBLE_REACH_MEMBER),
                format!(
                    "{} requires actingcommand.scheduling.v2",
                    crate::IRREVERSIBLE_REACH_MEMBER
                ),
                descriptor,
            ));
        }
    }
    validate_continuations(bundle, map, task_ids, descriptor, diagnostics);
    validate_realization_cycles(bundle, map, descriptor, diagnostics);
}

/// SS-2d (register §1.235; design `rd-1031/design-acm-price-addendum2.1.md` §7.3): a task's
/// `benefit_realization`. It is a V2 member (a V1 document that names it is refused as an
/// unknown member). `true` requires, each refused at its own member:
/// 1. `benefit_judged: true` (`missing_required_field` at `benefit_judged`): only a judged
///    candidate can be within its rounding error;
/// 2. at least one `consumes` and one `produces` entry (`missing_required_field` at the empty
///    list);
/// 3. a reserve on every pool it consumes (`missing_required_field` at `reserves`), so each run
///    lowers a pool with a reserve line and stops there;
/// 4. a fact reading of every pool it consumes (`effect_incompatible` at the entry), so its
///    progress is visible to the no-progress guard and the reserve gate reads it.
///
/// `false` is as without the member and requires nothing.
fn validate_realization(
    task: &TaskSpec,
    path: &str,
    map: &SourceMap,
    pools: &[PoolSpec],
    descriptor: Option<(&str, u64)>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    if task.benefit_realization.is_none() {
        return;
    }
    if map.schema_version.as_deref() != Some(SCHEDULING_SCHEMA_VERSION_V2) {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::UnknownField,
            format!("{path}/benefit_realization"),
            "benefit_realization requires actingcommand.scheduling.v2",
            descriptor,
        ));
        return;
    }
    if !task.is_benefit_realization() {
        return;
    }
    if !task.is_benefit_judged() {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::MissingRequiredField,
            format!("{path}/benefit_judged"),
            "benefit_realization requires benefit_judged: true",
            descriptor,
        ));
    }
    for (member, effects) in [("consumes", &task.consumes), ("produces", &task.produces)] {
        if effects.is_empty() {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::MissingRequiredField,
                format!("{path}/{member}"),
                format!("benefit_realization requires at least one {member} entry"),
                descriptor,
            ));
        }
    }
    let mut reported = HashSet::new();
    for (index, effect) in task.consumes.iter().enumerate() {
        let reserved = task
            .reserves
            .iter()
            .flatten()
            .any(|reserve| reserve.pool_id == effect.pool_id);
        if !reserved && reported.insert(effect.pool_id.as_str()) {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::MissingRequiredField,
                format!("{path}/reserves"),
                format!(
                    "benefit_realization requires a reserve on the consumed pool `{}`",
                    effect.pool_id
                ),
                descriptor,
            ));
        }
        let read_from_fact = pools
            .iter()
            .find(|pool| pool.id == effect.pool_id)
            .is_none_or(|pool| matches!(pool.observation, ObservationRef::Fact { .. }));
        if !read_from_fact {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::EffectIncompatible,
                format!("{path}/consumes/{index}"),
                format!(
                    "benefit_realization requires the consumed pool `{}` to be observed by a fact reading",
                    effect.pool_id
                ),
                descriptor,
            ));
        }
    }
}

/// SS-2h (register §1.289): a task's `benefit_limited`. It is a V2 member (a V1 document that
/// names it is refused as an unknown member); `true` requires `benefit_judged: true`
/// (`missing_required_field` at `benefit_judged`), since only a judged candidate takes part in a
/// capped spending order. `false` is as without the member.
fn validate_limited(
    task: &TaskSpec,
    path: &str,
    map: &SourceMap,
    descriptor: Option<(&str, u64)>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    if task.benefit_limited.is_none() {
        return;
    }
    if map.schema_version.as_deref() != Some(SCHEDULING_SCHEMA_VERSION_V2) {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::UnknownField,
            format!("{path}/benefit_limited"),
            "benefit_limited requires actingcommand.scheduling.v2",
            descriptor,
        ));
        return;
    }
    if task.is_benefit_limited() && !task.is_benefit_judged() {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::MissingRequiredField,
            format!("{path}/benefit_judged"),
            "benefit_limited requires benefit_judged: true",
            descriptor,
        ));
    }
}

/// SCHED-C1a (register §1.299; `contracts/scheduling/README.md`, "Fixed values written in the
/// pack", "Refused at compile time"): a task's `benefit_fixed_value_milli`. It is a V2 member (a
/// V1 document that names it is refused as an unknown member). Its value lies in
/// `1..=MAX_CANONICAL_INTEGER` (`limit_exceeded` at the member; a value of 0 is no value); it
/// requires `benefit_judged: true` (`missing_required_field` at `benefit_judged`). Beside it,
/// each a `fixed_value_conflict` at the member, since the task would then have two values and
/// neither may silently win: (a) a `produces` entry on a pool whose `valuation` is a declared
/// block or the pending marker, (b) `benefit_realization: true`, whose zero tier reads a modelled
/// value's rounding.
fn validate_fixed_value(
    task: &TaskSpec,
    path: &str,
    map: &SourceMap,
    pools: &[PoolSpec],
    descriptor: Option<(&str, u64)>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    let Some(value) = task.benefit_fixed_value_milli else {
        return;
    };
    let member = format!("{path}/benefit_fixed_value_milli");
    if map.schema_version.as_deref() != Some(SCHEDULING_SCHEMA_VERSION_V2) {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::UnknownField,
            member,
            "benefit_fixed_value_milli requires actingcommand.scheduling.v2",
            descriptor,
        ));
        return;
    }
    if !(1..=MAX_CANONICAL_INTEGER).contains(&value) {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::LimitExceeded,
            &member,
            format!("benefit_fixed_value_milli must be within 1..={MAX_CANONICAL_INTEGER}"),
            descriptor,
        ));
    }
    if !task.is_benefit_judged() {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::MissingRequiredField,
            format!("{path}/benefit_judged"),
            "benefit_fixed_value_milli requires benefit_judged: true",
            descriptor,
        ));
    }
    if let Some(effect) = task.produces.iter().find(|effect| {
        pools
            .iter()
            .any(|pool| pool.id == effect.pool_id && pool.valuation.is_some())
    }) {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::FixedValueConflict,
            &member,
            format!(
                "benefit_fixed_value_milli beside a production of the priced pool `{}`: remove the \
                 fixed value or the pool's valuation",
                effect.pool_id
            ),
            descriptor,
        ));
    }
    if task.is_benefit_realization() {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::FixedValueConflict,
            &member,
            "benefit_fixed_value_milli beside benefit_realization: true",
            descriptor,
        ));
    }
}

/// Workflow #437 ORD-1a (register §1.352; `contracts/scheduling/v2/README.md`): a task's
/// `max_queue_wait_ms`. It is a V2 member (a V1 document that names it is refused as an unknown
/// member); its value lies in `MIN_QUEUE_WAIT_MS..=MAX_QUEUE_WAIT_MS` (`limit_exceeded` at the
/// member). A value that is no unsigned integer, an explicit `null` included, is refused when the
/// document is read (`type_mismatch`).
fn validate_queue_wait(
    task: &TaskSpec,
    path: &str,
    map: &SourceMap,
    descriptor: Option<(&str, u64)>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    let Some(limit) = task.max_queue_wait_ms else {
        return;
    };
    let member = format!("{path}/max_queue_wait_ms");
    if map.schema_version.as_deref() != Some(SCHEDULING_SCHEMA_VERSION_V2) {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::UnknownField,
            member,
            "max_queue_wait_ms requires actingcommand.scheduling.v2",
            descriptor,
        ));
        return;
    }
    if !(MIN_QUEUE_WAIT_MS..=MAX_QUEUE_WAIT_MS).contains(&limit) {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::LimitExceeded,
            member,
            format!("max_queue_wait_ms must be within {MIN_QUEUE_WAIT_MS}..={MAX_QUEUE_WAIT_MS}"),
            descriptor,
        ));
    }
}

/// SS-2d (register §1.235; design §7.3 check 5): the tasks marked `benefit_realization` link
/// every pool each consumes to every pool it produces, and that graph has no cycle (a task that
/// consumes and produces one pool is a cycle of its own), so no zero-benefit loop can run. The
/// compiler binds no task to an instance, so the marked tasks of the whole catalog form one graph
/// by pool id: a composed catalog's pool ids are per instance already once a rule is copied to
/// two instances. Each marked task with a link on a cycle is one `realization_cycle` at its
/// `benefit_realization`, in task order.
fn validate_realization_cycles(
    bundle: &CatalogBundle,
    map: &SourceMap,
    descriptor: Option<(&str, u64)>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    if map.schema_version.as_deref() != Some(SCHEDULING_SCHEMA_VERSION_V2) {
        return;
    }
    let marked = bundle
        .tasks
        .tasks
        .iter()
        .enumerate()
        .filter(|(_, task)| task.is_benefit_realization())
        .collect::<Vec<_>>();
    if marked.is_empty() {
        return;
    }
    let mut links = HashMap::<&str, HashSet<&str>>::new();
    for (_, task) in &marked {
        for consumed in &task.consumes {
            let next = links.entry(consumed.pool_id.as_str()).or_default();
            next.extend(task.produces.iter().map(|effect| effect.pool_id.as_str()));
        }
    }
    for (index, task) in marked.iter().copied() {
        // The pools reachable from what the task produces; a cycle reaches what it consumes.
        let mut reached = HashSet::new();
        let mut frontier = task
            .produces
            .iter()
            .map(|effect| effect.pool_id.as_str())
            .collect::<Vec<_>>();
        while let Some(pool) = frontier.pop() {
            if reached.insert(pool) {
                frontier.extend(links.get(pool).into_iter().flatten().copied());
            }
        }
        if let Some(consumed) = task
            .consumes
            .iter()
            .find(|effect| reached.contains(effect.pool_id.as_str()))
        {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::RealizationCycle,
                format!("/tasks/{index}/benefit_realization"),
                format!(
                    "the realization tasks form a cycle: task `{}` consumes `{}`, which its own production reaches again",
                    task.id, consumed.pool_id
                ),
                descriptor,
            ));
        }
    }
}

/// Workflow #411 B16 part 2: a task's `reserves`. It is a V2 member (a V1 document that names it
/// is refused as an unknown member) of 1..=`MAX_EFFECTS_PER_TASK` entries; each names a pool of
/// the catalog (`dangling_reference`) once (`duplicate_id`), and its `at_least` is at most that
/// pool's capacity (`limit_exceeded`).
fn validate_reserves(
    task: &TaskSpec,
    path: &str,
    map: &SourceMap,
    pools: &[PoolSpec],
    descriptor: Option<(&str, u64)>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    let Some(reserves) = &task.reserves else {
        return;
    };
    let path = format!("{path}/reserves");
    if map.schema_version.as_deref() != Some(SCHEDULING_SCHEMA_VERSION_V2) {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::UnknownField,
            path,
            "reserves requires actingcommand.scheduling.v2",
            descriptor,
        ));
        return;
    }
    if reserves.is_empty() || reserves.len() > MAX_EFFECTS_PER_TASK {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::LimitExceeded,
            &path,
            format!("reserves must list 1..={MAX_EFFECTS_PER_TASK} pools"),
            descriptor,
        ));
    }
    let mut listed = HashSet::new();
    for (index, reserve) in reserves.iter().enumerate() {
        let item_path = format!("{path}/{index}");
        let Some(pool) = pools.iter().find(|pool| pool.id == reserve.pool_id) else {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::DanglingReference,
                format!("{item_path}/pool_id"),
                format!("referenced pool `{}` does not exist", reserve.pool_id),
                descriptor,
            ));
            continue;
        };
        if !listed.insert(reserve.pool_id.as_str()) {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::DuplicateId,
                format!("{item_path}/pool_id"),
                format!("pool `{}` is reserved twice", reserve.pool_id),
                descriptor,
            ));
        }
        // #386 CF-5: a capacity read from a fact is known only at evaluation.
        if let Some(capacity) = pool.capacity.constant()
            && reserve.at_least > capacity
        {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::LimitExceeded,
                format!("{item_path}/at_least"),
                format!(
                    "a reserve of {} exceeds pool capacity {capacity}",
                    reserve.at_least
                ),
                descriptor,
            ));
        }
    }
}

/// Workflow #372 K5: each task's `continues_from`. It is a V2 member (a V1 document that names
/// it is refused as an unknown member) of 1..=`MAX_CONTINUED_TASK_IDS` ids, each by the rule of
/// a task id. A continued id is never a task id of the same catalog, is listed once in its task,
/// and is claimed by one task only (a split is refused): each is `duplicate_id` at the item.
/// Nothing is checked against the ledger.
fn validate_continuations(
    bundle: &CatalogBundle,
    map: &SourceMap,
    task_ids: &HashSet<&str>,
    descriptor: Option<(&str, u64)>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    // ROUT-L RL-1a (model §3.2 step 3): the standard-pack composer's copies of one rule on two
    // or more instances each continue the rule's own id, so an id may be claimed by several tasks
    // when each is scoped to another instance; any other second claim is a split.
    let mut claimed = HashMap::<&str, Vec<Option<&str>>>::new();
    for (index, task) in bundle.tasks.tasks.iter().enumerate() {
        let Some(continued) = &task.continues_from else {
            continue;
        };
        let instance = match &task.scope {
            ScopeSelector::Instance { instance_id } => Some(instance_id.as_str()),
            ScopeSelector::Server { .. } | ScopeSelector::Game { .. } => None,
        };
        let path = format!("/tasks/{index}/continues_from");
        if map.schema_version.as_deref() != Some(SCHEDULING_SCHEMA_VERSION_V2) {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::UnknownField,
                path,
                "continues_from requires actingcommand.scheduling.v2",
                descriptor,
            ));
            continue;
        }
        if continued.is_empty() || continued.len() > MAX_CONTINUED_TASK_IDS {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::LimitExceeded,
                &path,
                format!("continues_from must list 1..={MAX_CONTINUED_TASK_IDS} task ids"),
                descriptor,
            ));
        }
        let mut listed = HashSet::new();
        for (item, id) in continued.iter().enumerate() {
            let item_path = format!("{path}/{item}");
            validate_identifier(map, &item_path, id, descriptor, diagnostics);
            let reason = if task_ids.contains(id.as_str()) {
                Some(format!(
                    "continued task id `{id}` is a task of this catalog"
                ))
            } else if !listed.insert(id.as_str()) {
                Some(format!("continued task id `{id}` is listed twice"))
            } else if !claim(&mut claimed, id, instance) {
                Some(format!("continued task id `{id}` is claimed by two tasks"))
            } else {
                None
            };
            if let Some(reason) = reason {
                diagnostics.push(map.diagnostic(
                    CatalogDiagnosticCode::DuplicateId,
                    item_path,
                    reason,
                    descriptor,
                ));
            }
        }
    }
}

/// Records a claim of the continued id `id` by a task scoped to `instance` (`None` for a server or
/// game scope): `false` when a task scoped to the same instance, or either claim not scoped to an
/// instance, claimed it before.
fn claim<'a>(
    claimed: &mut HashMap<&'a str, Vec<Option<&'a str>>>,
    id: &'a str,
    instance: Option<&'a str>,
) -> bool {
    let claimants = claimed.entry(id).or_default();
    let free = instance.is_some()
        && claimants
            .iter()
            .all(|claimant| claimant.is_some() && *claimant != instance);
    if free || claimants.is_empty() {
        claimants.push(instance);
        true
    } else {
        false
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
    timeline: &TimelineDocument,
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
        timeline,
        descriptor,
        diagnostics,
    );
    validate_predicate(
        &task.feedback_stop,
        &format!("{path}/feedback_stop"),
        map,
        task_ids,
        pool_ids,
        timeline,
        descriptor,
        diagnostics,
    );
    // SS-5: a probe admits its task, so it stands in the trigger alone, outside every `not`. A
    // V1 document is refused at the member itself (`validate_predicate_node`).
    if map.schema_version.as_deref() == Some(SCHEDULING_SCHEMA_VERSION_V2) {
        refuse_misplaced_probe(
            &task.trigger,
            &format!("{path}/trigger"),
            true,
            map,
            descriptor,
            diagnostics,
        );
        refuse_misplaced_probe(
            &task.feedback_stop,
            &format!("{path}/feedback_stop"),
            false,
            map,
            descriptor,
            diagnostics,
        );
    }
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
    // SS-3 (register §1.212): V1 requires `loop_budget` and its three items; V2 makes the member
    // and each item optional, and an item that is written must still be nonzero.
    if map.schema_version.as_deref() != Some(SCHEDULING_SCHEMA_VERSION_V2) {
        let missing = match &task.loop_budget {
            None => Some(format!("{path}/loop_budget")),
            Some(budget) => [
                ("daily_limit", budget.daily_limit.is_none()),
                (
                    "window_iteration_limit",
                    budget.window_iteration_limit.is_none(),
                ),
                ("max_runtime_ms", budget.max_runtime_ms.is_none()),
            ]
            .into_iter()
            .find(|(_, absent)| *absent)
            .map(|(item, _)| format!("{path}/loop_budget/{item}")),
        };
        if let Some(missing) = missing {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::MissingRequiredField,
                missing,
                "an actingcommand.scheduling.v1 task requires loop_budget with daily_limit, window_iteration_limit and max_runtime_ms",
                descriptor,
            ));
        }
    }
    if task.next_run_clamp_ms == 0
        || task.expected_duration_ms == 0
        || task.daily_limit() == Some(0)
        || task.window_iteration_limit() == Some(0)
        || task.max_runtime_ms() == Some(0)
    {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::LoopBudgetMissing,
            format!("{path}/loop_budget"),
            "task loops and runtime must have nonzero explicit bounds",
            descriptor,
        ));
    }
    if task
        .daily_limit()
        .is_some_and(|limit| limit > MAX_BUDGET_COUNT)
        || task
            .window_iteration_limit()
            .is_some_and(|limit| limit > MAX_BUDGET_COUNT)
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
    if task.strategic_weight_milli > MAX_STRATEGIC_WEIGHT_MILLI {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::LimitExceeded,
            format!("{path}/strategic_weight_milli"),
            format!("strategic weight exceeds {MAX_STRATEGIC_WEIGHT_MILLI}"),
            descriptor,
        ));
    }
    if task
        .value_milli
        .is_some_and(|value| value > MAX_VALUE_MILLI)
    {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::LimitExceeded,
            format!("{path}/value_milli"),
            format!("declared value exceeds {MAX_VALUE_MILLI}"),
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
        validate_instance_alias(
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
            && weight > MAX_STRATEGIC_WEIGHT_MILLI
        {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::LimitExceeded,
                format!("{override_path}/strategic_weight_milli"),
                format!("override strategic weight exceeds {MAX_STRATEGIC_WEIGHT_MILLI}"),
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
    // SCHED-C1f1 (register §1.303 SC1F-1, question 7): the pools of the entries that read their
    // amount from a fact; a second such entry of one pool is refused.
    let mut read_from_facts = HashSet::new();
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
        // SS-2b (register §1.225): `amount` may be the pending marker in V2, in either direction,
        // as the effect's one quantity. It has no quantity to bound against the pool's capacity.
        let v2 = map.schema_version.as_deref() == Some(SCHEDULING_SCHEMA_VERSION_V2);
        // SCHED-C1f1 (register §1.303 SC1F-1): an object `amount` is a consumption read from a
        // fact, checked by its own rules.
        if matches!(
            effect.amount,
            Some(EffectAmount::Fact(_) | EffectAmount::Malformed(_))
        ) {
            let first = read_from_facts.insert(effect.pool_id.as_str());
            validate_consumption_fact(
                effect,
                (expected_direction, first),
                &effect_path,
                map,
                pools,
                descriptor,
                diagnostics,
            );
            continue;
        }
        if effect.is_pending() {
            if !v2 {
                diagnostics.push(map.diagnostic(
                    CatalogDiagnosticCode::TypeMismatch,
                    format!("{effect_path}/amount"),
                    "the pending amount requires actingcommand.scheduling.v2",
                    descriptor,
                ));
            } else if effect.expected_amount_milli.is_some() || effect.confidence_milli > 1000 {
                diagnostics.push(map.diagnostic(
                    CatalogDiagnosticCode::LimitExceeded,
                    effect_path.clone(),
                    "effect requires exactly one positive amount or bounded v2 expected_amount_milli; confidence must be in 0..=1000",
                    descriptor,
                ));
            }
            continue;
        }
        // SS-2g (register §1.232): a V2 effect of either direction may declare its quantity as
        // `expected_amount_milli`, a consumption as a production may; V1 requires `amount`.
        let quantity_milli = match (effect.units(), effect.expected_amount_milli) {
            (Some(amount), None) if amount > 0 => Some(u128::from(amount) * 1_000),
            (None, Some(amount)) if v2 && u128::from(amount) <= MAX_CANONICAL_INTEGER as u128 => {
                Some(u128::from(amount))
            }
            _ => None,
        };
        if quantity_milli.is_none() || effect.confidence_milli > 1000 {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::LimitExceeded,
                effect_path.clone(),
                "effect requires exactly one positive amount or bounded v2 expected_amount_milli; confidence must be in 0..=1000",
                descriptor,
            ));
        }
        // #386 CF-5: a capacity read from a fact is known only at evaluation.
        if let Some(capacity) = pools
            .iter()
            .find(|pool| pool.id == effect.pool_id)
            .and_then(|pool| pool.capacity.constant())
            && quantity_milli.is_some_and(|amount| amount > u128::from(capacity) * 1_000)
        {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::EffectIncompatible,
                effect_path.clone(),
                format!("effect quantity exceeds pool capacity {capacity} real units"),
                descriptor,
            ));
        }
    }
}

/// SCHED-C1f1 (register §1.303 SC1F-1; `contracts/scheduling/README.md`, "A run's consumption
/// read from a fact", *Refused*): the entry at `effect_path` whose `amount` is an object, the
/// form `{"type": "fact", "fact_key": B, "fallback": F}` or something written in its place. Each
/// refusal at its pointer:
/// - in a V1 document, `unknown_field` at `amount/type`, and nothing else is checked;
/// - on a `produces` entry (`direction` the list's), `effect_incompatible` at `amount`, and
///   nothing else is checked;
/// - beside `expected_amount_milli`, or with a confidence above 1000, `limit_exceeded` at the
///   entry, as two quantities are;
/// - a `type` other than `fact`, a `fact_key` that is no identifier, a `fallback` that is no
///   integer (`null` included), `type_mismatch` at the member;
/// - a `fallback` outside `1..=2^53−1`, `limit_exceeded` at `amount/fallback`; one above a constant
///   capacity, `effect_incompatible` at the entry, as a constant `amount` above it;
/// - a member missing, `missing_required_field` at `amount` (the reason names it); any other
///   member (`offset`, `scope` among them), `unknown_field` at it;
/// - with `first` false (another entry of the task's list on the same pool writes the form
///   before it), `effect_incompatible` at `amount`.
fn validate_consumption_fact(
    effect: &ResourceEffectSpec,
    (direction, first): (EffectDirection, bool),
    effect_path: &str,
    map: &SourceMap,
    pools: &[PoolSpec],
    descriptor: Option<(&str, u64)>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    let amount = format!("{effect_path}/amount");
    let [kind, fact_key, fallback] = crate::schema::CONSUMPTION_FACT_MEMBERS;
    if map.schema_version.as_deref() != Some(SCHEDULING_SCHEMA_VERSION_V2) {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::UnknownField,
            format!("{amount}/{kind}"),
            "a consumption read from a fact requires actingcommand.scheduling.v2",
            descriptor,
        ));
        return;
    }
    if direction != EffectDirection::Consume {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::EffectIncompatible,
            amount,
            "only a consumes entry reads its amount from a fact",
            descriptor,
        ));
        return;
    }
    if effect.expected_amount_milli.is_some() || effect.confidence_milli > 1000 {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::LimitExceeded,
            effect_path,
            "effect requires exactly one positive amount or bounded v2 expected_amount_milli; confidence must be in 0..=1000",
            descriptor,
        ));
    }
    if !first {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::EffectIncompatible,
            &amount,
            format!(
                "a task reads one run's amount of pool `{}` from one fact: another of its entries \
                 on the pool writes the form before this one",
                effect.pool_id
            ),
            descriptor,
        ));
    }
    let fallback_at = format!("{amount}/{fallback}");
    match &effect.amount {
        Some(EffectAmount::Fact(form)) => {
            validate_read_fact_key(
                map,
                &format!("{amount}/{fact_key}"),
                &form.fact_key,
                descriptor,
                diagnostics,
            );
            validate_fallback(
                i128::from(form.fallback),
                (effect, effect_path, &fallback_at),
                map,
                pools,
                descriptor,
                diagnostics,
            );
        }
        Some(EffectAmount::Malformed(members)) => {
            for (name, value) in members {
                let at = format!("{amount}/{}", pointer_token(name));
                match name.as_str() {
                    member if member == kind => {
                        if value.as_str() != Some(crate::FACT_REFERENCE_TYPE) {
                            diagnostics.push(map.diagnostic(
                                CatalogDiagnosticCode::TypeMismatch,
                                at,
                                format!(
                                    "a consumption read from a fact has the type `{}`",
                                    crate::FACT_REFERENCE_TYPE
                                ),
                                descriptor,
                            ));
                        }
                    }
                    member if member == fact_key => match value.as_str() {
                        Some(key) => validate_read_fact_key(map, &at, key, descriptor, diagnostics),
                        None => diagnostics.push(map.diagnostic(
                            CatalogDiagnosticCode::TypeMismatch,
                            at,
                            "a consumption's fact_key is an identifier",
                            descriptor,
                        )),
                    },
                    member if member == fallback => {
                        let integer = value
                            .as_u64()
                            .map(i128::from)
                            .or_else(|| value.as_i64().map(i128::from));
                        match integer {
                            Some(integer) => validate_fallback(
                                integer,
                                (effect, effect_path, &fallback_at),
                                map,
                                pools,
                                descriptor,
                                diagnostics,
                            ),
                            None => diagnostics.push(map.diagnostic(
                                CatalogDiagnosticCode::TypeMismatch,
                                at,
                                "a consumption's fallback is an integer",
                                descriptor,
                            )),
                        }
                    }
                    _ => diagnostics.push(map.diagnostic(
                        CatalogDiagnosticCode::UnknownField,
                        at,
                        format!(
                            "a consumption read from a fact names `{name}`: it has only type, \
                             fact_key and fallback, and reads the instance's own scope"
                        ),
                        descriptor,
                    )),
                }
            }
            for member in crate::schema::CONSUMPTION_FACT_MEMBERS {
                if !members.contains_key(member) {
                    diagnostics.push(map.diagnostic(
                        CatalogDiagnosticCode::MissingRequiredField,
                        &amount,
                        format!("a consumption read from a fact requires its member `{member}`"),
                        descriptor,
                    ));
                }
            }
        }
        Some(EffectAmount::Units(_) | EffectAmount::Pending) | None => {}
    }
}

/// SCHED-C1f1: a consumption's fallback `value` (an integer as written) of `effect` at
/// `effect_path`, whose `fallback` member is at `fallback_at`: within `1..=2^53−1`
/// (`limit_exceeded` at the member), then at most a constant capacity of its pool
/// (`effect_incompatible` at the entry, as a constant `amount` above it).
fn validate_fallback(
    value: i128,
    (effect, effect_path, fallback_at): (&ResourceEffectSpec, &str, &str),
    map: &SourceMap,
    pools: &[PoolSpec],
    descriptor: Option<(&str, u64)>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    if !(1..=i128::from(MAX_CANONICAL_INTEGER)).contains(&value) {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::LimitExceeded,
            fallback_at,
            format!("a consumption's fallback must be within 1..={MAX_CANONICAL_INTEGER}"),
            descriptor,
        ));
        return;
    }
    if let Some(capacity) = pools
        .iter()
        .find(|pool| pool.id == effect.pool_id)
        .and_then(|pool| pool.capacity.constant())
        && value > i128::from(capacity)
    {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::EffectIncompatible,
            effect_path,
            format!("effect quantity exceeds pool capacity {capacity} real units"),
            descriptor,
        ));
    }
}

/// One JSON Pointer reference token for the member `name` (RFC 6901: `~` as `~0`, `/` as `~1`).
fn pointer_token(name: &str) -> String {
    name.replace('~', "~0").replace('/', "~1")
}

#[allow(clippy::too_many_arguments)]
fn validate_predicate(
    predicate: &PredicateSpec,
    path: &str,
    map: &SourceMap,
    task_ids: &HashSet<&str>,
    pool_ids: &HashSet<&str>,
    timeline: &TimelineDocument,
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
        timeline,
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
    timeline: &TimelineDocument,
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
                    timeline,
                    descriptor,
                    diagnostics,
                );
            }
        }
        PredicateSpec::Not { predicate } => {
            let child_path = format!("{path}/predicate");
            refuse_missed_under_not(predicate, &child_path, map, descriptor, diagnostics);
            validate_predicate_node(
                predicate,
                &child_path,
                depth + 1,
                node_count,
                map,
                task_ids,
                pool_ids,
                timeline,
                descriptor,
                diagnostics,
            )
        }
        PredicateSpec::Clock { schedule, missed } => {
            validate_schedule(schedule, path, map, descriptor, diagnostics);
            if let Some(missed) = missed {
                validate_missed(missed, path, map, descriptor, diagnostics);
            }
        }
        PredicateSpec::TimelineActive { event_id } => {
            if map.schema_version.as_deref() != Some(SCHEDULING_SCHEMA_VERSION_V2) {
                diagnostics.push(map.diagnostic(
                    CatalogDiagnosticCode::TypeMismatch,
                    path,
                    "timeline_active requires actingcommand.scheduling.v2",
                    descriptor,
                ));
            }
            validate_item_identifier(
                map,
                &format!("{path}/event_id"),
                event_id,
                descriptor,
                diagnostics,
            );
            if !timeline.events.iter().any(|event| event.id == *event_id) {
                diagnostics.push(map.diagnostic(
                    CatalogDiagnosticCode::DanglingReference,
                    format!("{path}/event_id"),
                    format!("referenced timeline event `{event_id}` does not exist"),
                    descriptor,
                ));
            }
        }
        PredicateSpec::ResourceProjection {
            pool_id,
            comparison,
            value,
        } => {
            // #386 CF-5: a fact reference reads a fact of the pool's scope.
            if let ProjectionComparand::Fact(reference) = value {
                validate_reference_members(
                    reference,
                    &format!("{path}/value"),
                    "pool",
                    map,
                    descriptor,
                    diagnostics,
                );
            }
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
            on_unknown,
        } => {
            validate_scope(
                map,
                &format!("{path}/scope"),
                scope,
                descriptor,
                diagnostics,
            );
            validate_read_fact_key(
                map,
                &format!("{path}/fact_key"),
                fact_key,
                descriptor,
                diagnostics,
            );
            if on_unknown.is_some() {
                validate_probe(scope, fact_key, path, map, descriptor, diagnostics);
            }
            match value {
                FactComparand::Value(value) => {
                    validate_comparison_value(
                        *comparison,
                        value,
                        path,
                        map,
                        descriptor,
                        diagnostics,
                    );
                }
                FactComparand::Fact(reference) => {
                    validate_fact_reference(
                        *comparison,
                        reference,
                        path,
                        map,
                        descriptor,
                        diagnostics,
                    );
                }
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
            validate_read_fact_key(
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
        // Workflow #405 W1: a V2 predicate; `scope` and `fact_key` follow `fact`'s rules.
        PredicateSpec::FactTimeReached {
            scope,
            fact_key,
            offset_ms,
            max_age_ms,
            ..
        } => {
            if map.schema_version.as_deref() != Some(SCHEDULING_SCHEMA_VERSION_V2) {
                diagnostics.push(map.diagnostic(
                    CatalogDiagnosticCode::UnknownField,
                    format!("{path}/kind"),
                    "fact_time_reached requires actingcommand.scheduling.v2",
                    descriptor,
                ));
                return;
            }
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
            if !(MIN_FACT_TIME_OFFSET_MS..=MAX_FACT_TIME_OFFSET_MS).contains(offset_ms) {
                diagnostics.push(map.diagnostic(
                    CatalogDiagnosticCode::LimitExceeded,
                    format!("{path}/offset_ms"),
                    format!(
                        "fact_time_reached offset_ms must be within {MIN_FACT_TIME_OFFSET_MS}..={MAX_FACT_TIME_OFFSET_MS}"
                    ),
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
            since_own_run,
        } => {
            // SCH-M CF-1: the edge member is V2 only; a V1 document that writes it, with either
            // value, is refused at the member.
            if since_own_run.is_some()
                && map.schema_version.as_deref() != Some(SCHEDULING_SCHEMA_VERSION_V2)
            {
                diagnostics.push(map.diagnostic(
                    CatalogDiagnosticCode::UnknownField,
                    format!("{path}/since_own_run"),
                    "dependency_completed since_own_run requires actingcommand.scheduling.v2",
                    descriptor,
                ));
            }
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

/// Two `fact` equalities on one fact with two different typed values (#386 CF-4: a fact
/// reference's value is known only at evaluation, so it conflicts with nothing here).
fn has_conflicting_equalities(predicates: &[PredicateSpec]) -> bool {
    for (index, left) in predicates.iter().enumerate() {
        let PredicateSpec::Fact {
            scope: left_scope,
            fact_key: left_key,
            comparison: Comparison::Eq,
            value: FactComparand::Value(left_value),
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
                value: FactComparand::Value(right_value),
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

/// #386 CF-4: a fact reference is V2 only (a V1 document is refused as `unknown_field` at
/// `value/type`); it names a bounded identifier, an offset within the canonical integers and no
/// scope (a reference always reads the predicate's own scope, so a `scope` member is refused as
/// a cross-scope reference at `value/scope`); and it compares integers, so `contains` is
/// uncomputable.
fn validate_fact_reference(
    comparison: Comparison,
    reference: &FactReference,
    path: &str,
    map: &SourceMap,
    descriptor: Option<(&str, u64)>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    if !validate_reference_members(
        reference,
        &format!("{path}/value"),
        "predicate",
        map,
        descriptor,
        diagnostics,
    ) {
        return;
    }
    if comparison == Comparison::Contains {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::PredicateUncomputable,
            format!("{path}/comparison"),
            "a fact reference compares integers; contains is uncomputable",
            descriptor,
        ));
    }
}

/// #386 CF-4, CF-5: the members of the fact reference at `member` (a `fact` predicate's or a
/// `resource_projection`'s `value`, or a pool's `capacity`). It is V2 only: a V1 document is
/// refused as `unknown_field` at `<member>/type`, and nothing else is checked (`false`). It names
/// a bounded identifier, an offset within the canonical integers and no scope: it always reads a
/// fact of its `owner`'s own scope (the predicate's, or the pool's), so a `scope` member is
/// refused as a cross-scope reference at `<member>/scope`.
fn validate_reference_members(
    reference: &FactReference,
    member: &str,
    owner: &str,
    map: &SourceMap,
    descriptor: Option<(&str, u64)>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) -> bool {
    if map.schema_version.as_deref() != Some(SCHEDULING_SCHEMA_VERSION_V2) {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::UnknownField,
            format!("{member}/type"),
            "a fact reference requires actingcommand.scheduling.v2",
            descriptor,
        ));
        return false;
    }
    if reference.scope.is_some() {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::UnknownField,
            format!("{member}/scope"),
            format!("a fact reference reads a fact of the {owner}'s own scope and names no scope"),
            descriptor,
        ));
    }
    validate_read_fact_key(
        map,
        &format!("{member}/fact_key"),
        &reference.fact_key,
        descriptor,
        diagnostics,
    );
    if !(MIN_CANONICAL_INTEGER..=MAX_CANONICAL_INTEGER).contains(&reference.offset) {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::LimitExceeded,
            format!("{member}/offset"),
            format!(
                "a fact reference offset must be within {MIN_CANONICAL_INTEGER}..={MAX_CANONICAL_INTEGER}"
            ),
            descriptor,
        ));
    }
    true
}

/// Workflow #361 R7: `missed` is a V2 member, and its grace is one minute to seven days.
fn validate_missed(
    missed: &MissedSlotRule,
    path: &str,
    map: &SourceMap,
    descriptor: Option<(&str, u64)>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    if map.schema_version.as_deref() != Some(SCHEDULING_SCHEMA_VERSION_V2) {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::UnknownField,
            format!("{path}/missed"),
            "clock missed requires actingcommand.scheduling.v2",
            descriptor,
        ));
        return;
    }
    let MissedSlotRule::SkipAfter { grace_ms } = missed;
    if !(MIN_MISSED_GRACE_MS..=MAX_MISSED_GRACE_MS).contains(grace_ms) {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::LimitExceeded,
            format!("{path}/missed/grace_ms"),
            format!(
                "clock missed grace_ms must be within {MIN_MISSED_GRACE_MS}..={MAX_MISSED_GRACE_MS}"
            ),
            descriptor,
        ));
    }
}

/// Workflow #361 R7: a clock under a `not` cannot carry `missed`, because the rule only skips
/// occurrences of a clock that makes its task due. `predicate` is the child of a `not` at
/// `path`; a nested `not` refuses its own subtree when it is validated.
fn refuse_missed_under_not(
    predicate: &PredicateSpec,
    path: &str,
    map: &SourceMap,
    descriptor: Option<(&str, u64)>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    match predicate {
        PredicateSpec::All { predicates } | PredicateSpec::Any { predicates } => {
            for (index, child) in predicates.iter().enumerate() {
                refuse_missed_under_not(
                    child,
                    &format!("{path}/predicates/{index}"),
                    map,
                    descriptor,
                    diagnostics,
                );
            }
        }
        PredicateSpec::Clock {
            missed: Some(_), ..
        } => diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::PredicateUncomputable,
            path,
            "a clock under not cannot carry missed",
            descriptor,
        )),
        PredicateSpec::Not { .. }
        | PredicateSpec::Clock { missed: None, .. }
        | PredicateSpec::TimelineActive { .. }
        | PredicateSpec::ResourceProjection { .. }
        | PredicateSpec::Fact { .. }
        | PredicateSpec::RecordDeadline { .. }
        | PredicateSpec::FactTimeReached { .. }
        | PredicateSpec::DependencyCompleted { .. }
        | PredicateSpec::Outcome { .. } => {}
    }
}

/// SS-5: `on_unknown` is a V2 member (a V1 document is refused as `unknown_field` at the member),
/// and a probe names a fact only the task's own package reads: a reading of the instance's own
/// scope (a reading writes nothing else), under a key a reading mark may write (`resource.` or
/// `inventory.`). Any other is `predicate_uncomputable` at the member: no run of the task can make
/// it known. Whether the key is one of the marks of the task's package is checked where the rule
/// meets its package (`actinglab standard-pack check`, `package bundle --schedule`).
fn validate_probe(
    scope: &ScopeSelector,
    fact_key: &str,
    path: &str,
    map: &SourceMap,
    descriptor: Option<(&str, u64)>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    let member = format!("{path}/on_unknown");
    if map.schema_version.as_deref() != Some(SCHEDULING_SCHEMA_VERSION_V2) {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::UnknownField,
            member,
            "fact on_unknown requires actingcommand.scheduling.v2",
            descriptor,
        ));
        return;
    }
    if !matches!(scope, ScopeSelector::Instance { .. }) {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::PredicateUncomputable,
            member,
            "a probe reads a fact of an instance's own scope, the only scope a reading writes",
            descriptor,
        ));
        return;
    }
    if !crate::evaluator::REALISTIC_FACT_PREFIXES
        .iter()
        .any(|prefix| fact_key.starts_with(prefix))
    {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::PredicateUncomputable,
            member,
            "a probe names a key a reading mark writes (resource. or inventory.)",
            descriptor,
        ));
    }
}

/// SS-5: a probe admits its task, so a `fact` predicate with `on_unknown` stands only where its
/// truth makes the task due: in the `trigger`, reached through `all` and `any`. Under a `not` (its
/// true would block the task) or in the `feedback_stop` (its true would stop it) it is
/// `predicate_uncomputable` at the member. `admits` says whether `predicate` at `path` is such a
/// place.
fn refuse_misplaced_probe(
    predicate: &PredicateSpec,
    path: &str,
    admits: bool,
    map: &SourceMap,
    descriptor: Option<(&str, u64)>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    match predicate {
        PredicateSpec::All { predicates } | PredicateSpec::Any { predicates } => {
            for (index, child) in predicates.iter().enumerate() {
                refuse_misplaced_probe(
                    child,
                    &format!("{path}/predicates/{index}"),
                    admits,
                    map,
                    descriptor,
                    diagnostics,
                );
            }
        }
        PredicateSpec::Not { predicate } => refuse_misplaced_probe(
            predicate,
            &format!("{path}/predicate"),
            false,
            map,
            descriptor,
            diagnostics,
        ),
        PredicateSpec::Fact {
            on_unknown: Some(_),
            ..
        } if !admits => diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::PredicateUncomputable,
            format!("{path}/on_unknown"),
            "a probe stands only in a trigger, outside every not",
            descriptor,
        )),
        PredicateSpec::Fact { .. }
        | PredicateSpec::Clock { .. }
        | PredicateSpec::TimelineActive { .. }
        | PredicateSpec::ResourceProjection { .. }
        | PredicateSpec::RecordDeadline { .. }
        | PredicateSpec::FactTimeReached { .. }
        | PredicateSpec::DependencyCompleted { .. }
        | PredicateSpec::Outcome { .. } => {}
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
        validate_item_identifier(
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
        // #386 CF-5: a capacity read from a fact is checked as a reference here; a reading that
        // gives no positive capacity leaves the pool without a value at evaluation.
        if let PoolCapacity::Fact(reference) = &pool.capacity {
            validate_reference_members(
                reference,
                &format!("{path}/capacity"),
                "pool",
                map,
                descriptor,
                diagnostics,
            );
        }
        if pool.capacity.constant() == Some(0)
            || matches!(
                &pool.projection,
                PoolProjection::Regenerating(projection)
                    if projection.amount == 0 || projection.per_ms == 0
            )
        {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::LimitExceeded,
                path.clone(),
                "pool capacity and regeneration projection must be nonzero",
                descriptor,
            ));
        }
        if let crate::PoolValueSource::LedgerFact {
            minimum_confidence_milli,
        } = pool.value_source
            && (!(1..=1_000).contains(&minimum_confidence_milli)
                || !matches!(pool.observation, ObservationRef::Fact { .. }))
        {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::LimitExceeded,
                path.clone(),
                "ledger fact pools require a fact observation and confidence in 1..=1000",
                descriptor,
            ));
        }
        match &pool.observation {
            ObservationRef::Fact { fact_key } => validate_read_fact_key(
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
        // SS-2b (register §1.225): the pending marker in place of the block is V2's.
        if pool.valuation_pending()
            && map.schema_version.as_deref() != Some(SCHEDULING_SCHEMA_VERSION_V2)
        {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::TypeMismatch,
                format!("{path}/valuation"),
                "the pending valuation requires actingcommand.scheduling.v2",
                descriptor,
            ));
        }
        if let Some(valuation) = pool.declared_valuation() {
            validate_valuation(
                valuation,
                &format!("{path}/valuation"),
                map,
                descriptor,
                diagnostics,
            );
        }
        // SS-2h (register §1.289): the stop line and the hard line are V2's; SCHED-C1c (register
        // §1.299): so is the trigger line, and the consumers tell whether the lines can spend.
        validate_pool_lines(
            pool,
            &bundle.tasks.tasks,
            &path,
            map,
            descriptor,
            diagnostics,
        );
    }
}

/// SS-2h (register §1.289; `contracts/scheduling/README.md`, "Pool lines"): a pool's `stop_line`
/// and `hard_line` at `<path>/stop_line` and `<path>/hard_line`. They are V2 members: a V1
/// document is refused as `unknown_field` at each member written, and nothing else is checked.
/// Each line follows its form's rules: a reference a capacity reference's
/// ([`validate_reference_members`]), a constant a canonical safe integer (`limit_exceeded` at the
/// member). Then, `pool_lines_invalid` at the member:
/// 1. one written without the other (at the one written);
/// 2. a line above the pool's capacity, when both are known to the compiler: a constant above a
///    constant capacity, or a reference to the capacity's own fact with a larger offset;
/// 3. the stop line not above the hard line, when both are known to the compiler: two constants,
///    or two references to one fact compared by their offsets (at `hard_line`).
///
/// SCHED-C1c (register §1.299; "The trigger line" there): a pool's `trigger_line` at
/// `<path>/trigger_line` is a V2 member of the same two forms and rules, and `pool_lines_invalid`
/// at it when it is written without both other lines (rule 1), above the capacity (rule 2), or
/// below the stop line where both are known to the compiler (rule 4: two constants, or two
/// references to one fact compared by their offsets).
///
/// Lines the compiler cannot compare are checked at each evaluation (`lines_invalid`). SCHED-C1c:
/// a valued pool whose lines are valid and cannot spend it ([`unspendable_lines`]) compiles with
/// the warning `pool_lines_unspendable` at its `hard_line`.
fn validate_pool_lines(
    pool: &PoolSpec,
    tasks: &[TaskSpec],
    path: &str,
    map: &SourceMap,
    descriptor: Option<(&str, u64)>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    let written = [
        ("stop_line", &pool.stop_line),
        ("hard_line", &pool.hard_line),
        ("trigger_line", &pool.trigger_line),
    ];
    if written.iter().all(|(_, line)| line.is_none()) {
        return;
    }
    if map.schema_version.as_deref() != Some(SCHEDULING_SCHEMA_VERSION_V2) {
        for (member, line) in written {
            if line.is_some() {
                diagnostics.push(map.diagnostic(
                    CatalogDiagnosticCode::UnknownField,
                    format!("{path}/{member}"),
                    format!("a pool's {member} requires actingcommand.scheduling.v2"),
                    descriptor,
                ));
            }
        }
        return;
    }
    let mut well_formed = true;
    for (member, line) in written {
        let Some(line) = line else {
            continue;
        };
        let at = format!("{path}/{member}");
        match line {
            PoolLine::Fact(reference) => {
                let count = diagnostics.len();
                validate_reference_members(reference, &at, "pool", map, descriptor, diagnostics);
                well_formed &= diagnostics.len() == count;
            }
            PoolLine::Constant(value) => {
                if *value > MAX_CANONICAL_INTEGER as u64 {
                    diagnostics.push(map.diagnostic(
                        CatalogDiagnosticCode::LimitExceeded,
                        at,
                        format!("a constant {member} must be within 0..={MAX_CANONICAL_INTEGER}"),
                        descriptor,
                    ));
                    well_formed = false;
                }
            }
        }
    }
    let (Some(stop), Some(hard)) = (&pool.stop_line, &pool.hard_line) else {
        if pool.stop_line.is_some() || pool.hard_line.is_some() {
            let member = if pool.stop_line.is_some() {
                "stop_line"
            } else {
                "hard_line"
            };
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::PoolLinesInvalid,
                format!("{path}/{member}"),
                "a pool writes stop_line and hard_line together, or neither",
                descriptor,
            ));
        }
        // SCHED-C1c, rule 1: a trigger line only beside both lines.
        if pool.trigger_line.is_some() {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::PoolLinesInvalid,
                format!("{path}/trigger_line"),
                "a pool writes trigger_line only beside stop_line and hard_line",
                descriptor,
            ));
        }
        return;
    };
    // SS-2h fix 1: only a valued pool's lines take part; a pool without a declared valuation
    // carries them unread, which the compiler says (a warning at `stop_line`; the catalog compiles).
    if pool.declared_valuation().is_none() {
        let mut warning = map.diagnostic(
            CatalogDiagnosticCode::PoolLinesUnvalued,
            format!("{path}/stop_line"),
            "a pool's stop_line and hard_line take part only with a declared valuation",
            descriptor,
        );
        warning.severity = DiagnosticSeverity::Warning;
        diagnostics.push(warning);
    }
    if !well_formed {
        return;
    }
    let invalid = diagnostics.len();
    // Rule 2: a line above the capacity, where both are known here; SCHED-C1c: the trigger line
    // too.
    let lines = [
        ("stop_line", Some(stop)),
        ("hard_line", Some(hard)),
        ("trigger_line", pool.trigger_line.as_ref()),
    ];
    for (member, line) in lines {
        let Some(line) = line else {
            continue;
        };
        let above = match (line, &pool.capacity) {
            (PoolLine::Constant(value), PoolCapacity::Constant(capacity)) => value > capacity,
            (PoolLine::Fact(reference), PoolCapacity::Fact(capacity)) => {
                reference.fact_key == capacity.fact_key && reference.offset > capacity.offset
            }
            _ => false,
        };
        if above {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::PoolLinesInvalid,
                format!("{path}/{member}"),
                format!("a pool's {member} may not be above its capacity"),
                descriptor,
            ));
        }
    }
    // Rule 3: the stop line above the hard line, where both are known here.
    let not_above = match (stop, hard) {
        (PoolLine::Constant(stop), PoolLine::Constant(hard)) => stop <= hard,
        (PoolLine::Fact(stop), PoolLine::Fact(hard)) => {
            stop.fact_key == hard.fact_key && stop.offset <= hard.offset
        }
        _ => false,
    };
    if not_above {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::PoolLinesInvalid,
            format!("{path}/hard_line"),
            "a pool's stop_line must be above its hard_line",
            descriptor,
        ));
    }
    // SCHED-C1c, rule 4: the trigger line at or above the stop line, where both are known here.
    let below = match (&pool.trigger_line, stop) {
        (Some(PoolLine::Constant(trigger)), PoolLine::Constant(stop)) => trigger < stop,
        (Some(PoolLine::Fact(trigger)), PoolLine::Fact(stop)) => {
            trigger.fact_key == stop.fact_key && trigger.offset < stop.offset
        }
        _ => false,
    };
    if below {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::PoolLinesInvalid,
            format!("{path}/trigger_line"),
            "a pool's trigger_line may not be below its stop_line",
            descriptor,
        ));
    }
    // SCHED-C1c ("A pool its lines cannot spend"): valid lines of a valued pool that no judged
    // consumer's run fits under, where the compiler can tell (a warning; the catalog compiles).
    if diagnostics.len() == invalid
        && pool.declared_valuation().is_some()
        && let Some((room, least)) = unspendable_lines(pool, hard, tasks)
    {
        let mut warning = map.diagnostic(
            CatalogDiagnosticCode::PoolLinesUnspendable,
            format!("{path}/hard_line"),
            format!(
                "every judged consumer's run consumes more than the capacity less the hard_line: \
                 pool_id={} capacity_less_hard_line={room} least_consumption={least}",
                pool.id
            ),
            descriptor,
        );
        warning.severity = DiagnosticSeverity::Warning;
        diagnostics.push(warning);
    }
}

/// SCHED-C1c (register §1.299; `contracts/scheduling/README.md`, "Pool lines", "A pool its lines
/// cannot spend"): where the compiler can tell, the pool's capacity less its hard line `hard` and
/// the least consumption `⌈C/1000⌉` of one run among the pool's judged consumers, when every one
/// consumes more than that room: no run can start from the capacity without leaving the pool below
/// its hard line. The compiler can tell for a constant hard line against a constant capacity, and
/// for a hard line that references the capacity's own fact (the capacity's offset less the
/// line's), against every consumer's consumption: a judged task that is no manual request and
/// declares a consumption of the pool, the evaluator's consumers, one of whose entries of the pool
/// is pending leaving it untold. SCHED-C1f1: a consumption read from a fact is read at its
/// fallback. `None` otherwise, and for a pool without a consumer.
fn unspendable_lines(pool: &PoolSpec, hard: &PoolLine, tasks: &[TaskSpec]) -> Option<(u64, u64)> {
    let room = match (&pool.capacity, hard) {
        (PoolCapacity::Constant(capacity), PoolLine::Constant(hard)) => {
            capacity.checked_sub(*hard)?
        }
        (PoolCapacity::Fact(capacity), PoolLine::Fact(hard)) => {
            if capacity.fact_key != hard.fact_key {
                return None;
            }
            u64::try_from(i128::from(capacity.offset) - i128::from(hard.offset)).ok()?
        }
        _ => return None,
    };
    let mut least = None::<u64>;
    for task in tasks {
        if !task.is_benefit_judged() || crate::evaluator::manual_request(task) {
            continue;
        }
        let mut entries = task
            .consumes
            .iter()
            .filter(|effect| effect.pool_id == pool.id)
            .peekable();
        if entries.peek().is_none() {
            continue;
        }
        if entries.any(|effect| effect.is_pending()) {
            return None;
        }
        // SCHED-C1f1 (register §1.303, question 8): each consumer's declared consumption, a
        // fact-driven amount at its fallback, the amount an evaluation reads while the fact gives
        // no usable reading.
        let consumed = crate::resource_targets::consumption(
            task,
            &pool.id,
            crate::evaluator::Amounts::Declared,
        )
        .ok()?;
        if consumed > 0 {
            least = Some(least.map_or(consumed, |least| least.min(consumed)));
        }
    }
    least
        .filter(|least| *least > room)
        .map(|least| (room, least))
}

fn validate_valuation(
    valuation: &PoolValuation,
    path: &str,
    map: &SourceMap,
    descriptor: Option<(&str, u64)>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    for (field, text, max_bytes) in [
        ("name", &valuation.name, MAX_VALUATION_NAME_BYTES),
        ("unit", &valuation.unit, MAX_VALUATION_UNIT_BYTES),
    ] {
        if text.len() > max_bytes {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::LimitExceeded,
                format!("{path}/{field}"),
                format!("valuation {field} exceeds {max_bytes} UTF-8 bytes"),
                descriptor,
            ));
        }
        if text.trim().is_empty() || text.chars().any(char::is_control) {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::TypeMismatch,
                format!("{path}/{field}"),
                format!("valuation {field} must be nonblank text without control characters"),
                descriptor,
            ));
        }
    }
    if valuation.scale == 0 || valuation.scale > MAX_CANONICAL_INTEGER as u64 {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::LimitExceeded,
            format!("{path}/scale"),
            format!("valuation scale must be within 1..={MAX_CANONICAL_INTEGER}"),
            descriptor,
        ));
    }
    if valuation.base_weight_milli > MAX_VALUATION_BASE_WEIGHT_MILLI {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::LimitExceeded,
            format!("{path}/base_weight_milli"),
            format!(
                "valuation base_weight_milli must be within 0..={MAX_VALUATION_BASE_WEIGHT_MILLI}"
            ),
            descriptor,
        ));
    }
    if let Some(gap) = &valuation.gap
        && !(1..=MAX_VALUATION_GAP_WEIGHT_MILLI).contains(&gap.weight_milli)
    {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::LimitExceeded,
            format!("{path}/gap/weight_milli"),
            format!(
                "valuation gap weight_milli must be within 1..={MAX_VALUATION_GAP_WEIGHT_MILLI}"
            ),
            descriptor,
        ));
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
        validate_item_identifier(
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
    // SS-3 (register §1.212): V1 requires the three activity run-count bounds; V2 makes each
    // optional, and one that is written must still be nonzero.
    if map.schema_version.as_deref() != Some(SCHEDULING_SCHEMA_VERSION_V2)
        && let Some((item, _)) = [
            ("daily_budget", profile.daily_budget.is_none()),
            (
                "max_window_iterations",
                profile.max_window_iterations.is_none(),
            ),
            ("session_max_ms", profile.session_max_ms.is_none()),
        ]
        .into_iter()
        .find(|(_, absent)| *absent)
    {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::MissingRequiredField,
            format!("{path}/{item}"),
            "an actingcommand.scheduling.v1 activity profile requires daily_budget, max_window_iterations and session_max_ms",
            descriptor,
        ));
    }
    if profile.daily_budget == Some(0)
        || profile.max_window_iterations == Some(0)
        || profile.session_max_ms == Some(0)
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
    if profile
        .daily_budget
        .is_some_and(|budget| budget > MAX_BUDGET_COUNT)
        || profile
            .max_window_iterations
            .is_some_and(|budget| budget > MAX_BUDGET_COUNT)
    {
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
        if goal.strategic_weight_milli > MAX_STRATEGIC_WEIGHT_MILLI {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::LimitExceeded,
                format!("{goal_path}/strategic_weight_milli"),
                format!("goal strategic weight exceeds {MAX_STRATEGIC_WEIGHT_MILLI}"),
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
        match (bundle.timeline.schema_version.as_str(), &event.validity) {
            (SCHEDULING_SCHEMA_VERSION_V2, Some(validity)) => {
                if validity
                    .until_unix_ms
                    .0
                    .is_some_and(|until| until <= validity.from_unix_ms)
                {
                    diagnostics.push(map.diagnostic(
                        CatalogDiagnosticCode::PredicateUnreachable,
                        format!("{path}/validity"),
                        "timeline validity must be a nonempty half-open Unix interval",
                        descriptor,
                    ));
                }
            }
            (SCHEDULING_SCHEMA_VERSION_V2, None) => diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::MissingRequiredField,
                format!("{path}/validity"),
                "V2 timeline events require explicit validity",
                descriptor,
            )),
            (_, Some(_)) => diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::UnknownField,
                format!("{path}/validity"),
                "timeline validity requires actingcommand.scheduling.v2",
                descriptor,
            )),
            (_, None) => {}
        }
        validate_item_identifier(
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
        validate_excluded_prefixes(
            event,
            &bundle.timeline.schema_version,
            &path,
            map,
            descriptor,
            diagnostics,
        );
    }
}

/// Workflow #411 P19: a timeline event's `excludes_fact_prefixes` is V2 only (`unknown_field`
/// at the member in a V1 document); it holds at most as many prefixes as
/// `invalidates_fact_prefixes` may, each an identifier below one of the event's invalidated
/// prefixes (`dangling_reference` at the entry otherwise: it would exclude nothing).
fn validate_excluded_prefixes(
    event: &TimelineEvent,
    schema_version: &str,
    path: &str,
    map: &SourceMap,
    descriptor: Option<(&str, u64)>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    let Some(excluded) = &event.excludes_fact_prefixes else {
        return;
    };
    let member = format!("{path}/excludes_fact_prefixes");
    if schema_version != SCHEDULING_SCHEMA_VERSION_V2 {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::UnknownField,
            member,
            "timeline excludes_fact_prefixes requires actingcommand.scheduling.v2",
            descriptor,
        ));
        return;
    }
    if excluded.len() > MAX_REFERENCES_PER_TASK {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::LimitExceeded,
            member.clone(),
            format!("excluded fact prefix count exceeds {MAX_REFERENCES_PER_TASK}"),
            descriptor,
        ));
    }
    for (prefix_index, prefix) in excluded.iter().enumerate() {
        let entry = format!("{member}/{prefix_index}");
        validate_identifier(map, &entry, prefix, descriptor, diagnostics);
        if !event
            .invalidates_fact_prefixes
            .iter()
            .any(|invalidated| prefix.starts_with(invalidated.as_str()))
        {
            diagnostics.push(map.diagnostic(
                CatalogDiagnosticCode::DanglingReference,
                entry,
                format!(
                    "excluded fact prefix `{prefix}` lies below none of the event's invalidated prefixes"
                ),
                descriptor,
            ));
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
        ScopeSelector::Instance { instance_id } => {
            validate_instance_alias(
                map,
                &format!("{path}/instance_id"),
                instance_id,
                descriptor,
                diagnostics,
            );
            return;
        }
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

/// #428 S168-LB1a (since 0.12.55; `contracts/scheduling/README.md`, "Record lists and the
/// projection"): a member that names a fact key to read, a bounded identifier never under
/// `inventory.list.`. That prefix holds list readings, a select or judge step's input, which the
/// scheduler's projection leaves out; a member that names one is `type_mismatch` at its key, so no
/// predicate meets the gap at evaluation.
fn validate_read_fact_key(
    map: &SourceMap,
    path: &str,
    value: &str,
    descriptor: Option<(&str, u64)>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    validate_identifier(map, path, value, descriptor, diagnostics);
    if actingcommand_contract::is_list_reading_key(value) {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::TypeMismatch,
            path,
            format!(
                "`{value}` holds a list reading, a selection's input the scheduler never reads"
            ),
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

/// The id of a task, pool, activity profile or timeline event, and a reference to an event. ROUT-L
/// RL-1a (model §3.2 step 3): in V2 it may also be an instance copy's id `<id>@<alias>`, an
/// identifier, `@` and the copy's instance alias without whitespace, all of it at most 128 bytes
/// (the id bound every reader of a task id keeps). V1 keeps the identifier rule.
fn validate_item_identifier(
    map: &SourceMap,
    path: &str,
    value: &str,
    descriptor: Option<(&str, u64)>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    let copy = map.schema_version.as_deref() == Some(SCHEDULING_SCHEMA_VERSION_V2)
        && value.len() <= MAX_ID_BYTES
        && value.split_once('@').is_some_and(|(base, alias)| {
            !base.is_empty()
                && base
                    .bytes()
                    .enumerate()
                    .all(|(index, byte)| is_identifier_byte(index, byte))
                && !alias.is_empty()
                && actingcommand_contract::validate_instance_alias(alias).is_ok()
                && !alias.chars().any(char::is_whitespace)
        });
    if !copy {
        validate_identifier(map, path, value, descriptor, diagnostics);
    }
}

fn validate_instance_alias(
    map: &SourceMap,
    path: &str,
    value: &str,
    descriptor: Option<(&str, u64)>,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) {
    if actingcommand_contract::validate_instance_alias(value).is_err() {
        diagnostics.push(map.diagnostic(
            CatalogDiagnosticCode::TypeMismatch,
            path,
            "instance alias must contain 1 to 256 UTF-8 bytes without control characters",
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
