// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_policy::{
    CatalogDocumentSource, CatalogSources, CompiledCatalog, EffectDirection, EvaluationFacts,
    EvaluationResources, EvaluationTime, HostResourceSnapshot, InstanceSnapshot, PoolValueSnapshot,
    ScopeSelector, TaskRuntimeSnapshot, compile_catalog, evaluate,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const HOUR_MS: u64 = 3_600_000;
const DAY_MS: u64 = 86_400_000;

#[test]
fn neutral_activity_diff_compiles_and_expresses_both_requirement_shapes() {
    let first = compile_catalog(&neutral_sources()).expect("neutral activity catalog");
    let second = compile_catalog(&neutral_sources()).expect("repeat neutral activity catalog");

    assert_eq!(first.catalog_hash(), second.catalog_hash());
    assert_eq!(
        first.dry_run_json().expect("first dry-run"),
        second.dry_run_json().expect("second dry-run")
    );
    assert_eq!(first.summary().counts.tasks, 2);
    assert_eq!(first.summary().counts.pools, 3);

    let catalog = first.catalog();
    let regeneration = catalog
        .tasks
        .tasks
        .iter()
        .find(|task| task.id == "neutral.consume-regeneration")
        .expect("regeneration task");
    assert_eq!(regeneration.consumes.len(), 1);
    assert!(regeneration.produces.is_empty());

    let balance = catalog
        .tasks
        .tasks
        .iter()
        .find(|task| task.id == "neutral.balance-material")
        .expect("balance task");
    assert_eq!(balance.consumes.len(), 1);
    assert_eq!(balance.produces.len(), 1);

    let evaluation = evaluate(
        &first,
        &SimulationState::initial().facts(&first, 8 * HOUR_MS),
        &SimulationState::initial().resources(8 * HOUR_MS),
        EvaluationTime {
            unix_ms: 8 * HOUR_MS,
        },
        44,
    )
    .expect("neutral requirement evaluation");
    let task_ids = evaluation
        .dispatch_intents
        .iter()
        .map(|intent| intent.task_id.as_str())
        .collect::<Vec<_>>();
    assert!(task_ids.contains(&"neutral.consume-regeneration"));
    assert!(task_ids.contains(&"neutral.balance-material"));
    assert_complete_reason_chains(&evaluation);
}

#[test]
fn accelerated_48h_replay_is_deterministic_bounded_and_recoverable() {
    let catalog = compile_catalog(&neutral_sources()).expect("neutral activity catalog");
    let mut uninterrupted = SimulationState::initial();
    let transcript = run_hours(&catalog, &mut uninterrupted, 0, 48);

    let mut first_half = SimulationState::initial();
    let mut recovered_transcript = run_hours(&catalog, &mut first_half, 0, 24);
    let checkpoint = serde_json::to_vec(&first_half).expect("serialize midpoint state");
    let mut recovered: SimulationState =
        serde_json::from_slice(&checkpoint).expect("recover midpoint state");
    recovered_transcript.extend(run_hours(&catalog, &mut recovered, 24, 48));

    assert_eq!(transcript, recovered_transcript);
    assert_eq!(uninterrupted, recovered);
    assert_eq!(uninterrupted.elapsed_hours, 48);
    assert!(
        uninterrupted
            .dispatch_totals
            .get("neutral.consume-regeneration")
            .copied()
            .unwrap_or_default()
            > 0
    );
    assert!(
        uninterrupted
            .dispatch_totals
            .get("neutral.balance-material")
            .copied()
            .unwrap_or_default()
            > 0
    );
    assert!(uninterrupted.pools["neutral-refined-material"] >= 80);
    assert!(uninterrupted.pools["neutral-energy"] <= 120);
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SimulationState {
    pools: BTreeMap<String, u64>,
    last_dispatched: BTreeMap<String, u64>,
    dispatches_by_day: BTreeMap<String, u32>,
    dispatch_totals: BTreeMap<String, u32>,
    ledger_position: u64,
    elapsed_hours: u64,
}

impl SimulationState {
    fn initial() -> Self {
        Self {
            pools: BTreeMap::from([
                ("neutral-energy".to_string(), 120),
                ("neutral-raw-material".to_string(), 80),
                ("neutral-refined-material".to_string(), 10),
            ]),
            last_dispatched: BTreeMap::new(),
            dispatches_by_day: BTreeMap::new(),
            dispatch_totals: BTreeMap::new(),
            ledger_position: 1,
            elapsed_hours: 0,
        }
    }

    fn facts(&self, catalog: &CompiledCatalog, now: u64) -> EvaluationFacts {
        let tasks = catalog
            .catalog()
            .tasks
            .tasks
            .iter()
            .map(|task| TaskRuntimeSnapshot {
                task_id: task.id.clone(),
                instance_id: instance_id(&task.scope).to_string(),
                last_dispatched_unix_ms: self.last_dispatched.get(&task.id).copied(),
                eligible_since_unix_ms: Some(0),
                terminal_state: None,
            })
            .collect();
        EvaluationFacts {
            ledger_position: self.ledger_position,
            fact_snapshot_id: format!("snapshot:{now}:{}", self.ledger_position),
            facts: Vec::new(),
            outcomes: Vec::new(),
            tasks,
            instances: vec![
                InstanceSnapshot {
                    instance_id: "neutral-instance-a".to_string(),
                    server_id: "neutral-server".to_string(),
                    game_id: "neutral-game".to_string(),
                    host_id: "neutral-host-a".to_string(),
                    available: true,
                    capability_operation_ids: vec!["operation.consume".to_string()],
                    preferred_task_ids: vec!["neutral.consume-regeneration".to_string()],
                },
                InstanceSnapshot {
                    instance_id: "neutral-instance-b".to_string(),
                    server_id: "neutral-server".to_string(),
                    game_id: "neutral-game".to_string(),
                    host_id: "neutral-host-b".to_string(),
                    available: true,
                    capability_operation_ids: vec!["operation.balance".to_string()],
                    preferred_task_ids: vec!["neutral.balance-material".to_string()],
                },
            ],
        }
    }

    fn resources(&self, now: u64) -> EvaluationResources {
        EvaluationResources {
            pools: self
                .pools
                .iter()
                .map(|(pool_id, value)| PoolValueSnapshot {
                    pool_id: pool_id.clone(),
                    value: *value,
                    observed_at_unix_ms: now,
                })
                .collect(),
            hosts: ["neutral-host-a", "neutral-host-b"]
                .into_iter()
                .map(|host_id| HostResourceSnapshot {
                    host_id: host_id.to_string(),
                    cpu_available_milli: 1_000,
                    gpu_available_milli: 1_000,
                    io_available_milli: 1_000,
                    host_responsiveness_basis_points: 10_000,
                    third_party_pressure_basis_points: 0,
                    heavy_dispatch_limit: 1,
                    active_heavy_dispatches: 0,
                })
                .collect(),
        }
    }

    fn regenerate_for_hour(&mut self, hour: u64) {
        if hour == 0 {
            return;
        }
        let energy = self.pools.get_mut("neutral-energy").expect("energy pool");
        *energy = energy.saturating_add(6).min(120);
        if hour.is_multiple_of(24) {
            for pool_id in ["neutral-raw-material", "neutral-refined-material"] {
                let value = self.pools.get_mut(pool_id).expect("material pool");
                *value = value.saturating_add(1).min(100);
            }
        }
    }
}

fn run_hours(
    catalog: &CompiledCatalog,
    state: &mut SimulationState,
    start_hour: u64,
    end_hour: u64,
) -> Vec<Vec<u8>> {
    let mut transcript = Vec::new();
    for hour in start_hour..end_hour {
        state.regenerate_for_hour(hour);
        let now = hour * HOUR_MS;
        let evaluation = evaluate(
            catalog,
            &state.facts(catalog, now),
            &state.resources(now),
            EvaluationTime { unix_ms: now },
            44,
        )
        .expect("accelerated policy evaluation");
        assert_complete_reason_chains(&evaluation);

        for intent in &evaluation.dispatch_intents {
            let day = now / DAY_MS;
            let day_key = format!("{day}:{}", intent.task_id);
            let daily = state.dispatches_by_day.entry(day_key).or_default();
            *daily += 1;
            assert!(*daily <= intent.prerequisites.daily_limit);
            assert!(intent.prerequisites.window_iteration_limit > 0);
            assert!(intent.prerequisites.max_runtime_ms >= intent.expected_duration_ms);
            apply_declared_effects(catalog, state, &intent.task_id);
            state.last_dispatched.insert(intent.task_id.clone(), now);
            *state
                .dispatch_totals
                .entry(intent.task_id.clone())
                .or_default() += 1;
        }
        state.ledger_position += evaluation.dispatch_intents.len() as u64 + 1;
        state.elapsed_hours = hour + 1;
        transcript.push(serde_json::to_vec(&evaluation).expect("stable evaluation JSON"));
    }
    transcript
}

fn apply_declared_effects(catalog: &CompiledCatalog, state: &mut SimulationState, task_id: &str) {
    let task = catalog
        .catalog()
        .tasks
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .expect("dispatched task exists");
    let capacities = catalog
        .catalog()
        .pools
        .pools
        .iter()
        .map(|pool| (pool.id.as_str(), pool.capacity))
        .collect::<BTreeMap<_, _>>();
    for effect in task.consumes.iter().chain(&task.produces) {
        let value = state
            .pools
            .get_mut(&effect.pool_id)
            .expect("effect pool exists");
        match effect.direction {
            EffectDirection::Consume => {
                assert!(*value >= effect.amount);
                *value -= effect.amount;
            }
            EffectDirection::Produce => {
                *value = value
                    .checked_add(effect.amount)
                    .expect("bounded resource addition")
                    .min(capacities[effect.pool_id.as_str()]);
            }
        }
    }
}

fn assert_complete_reason_chains(evaluation: &actingcommand_policy::PolicyEvaluation) {
    assert_eq!(
        evaluation.dispatch_intents.len(),
        evaluation.reason_chains.len()
    );
    for intent in &evaluation.dispatch_intents {
        let chain = evaluation
            .reason_chains
            .iter()
            .find(|chain| chain.id == intent.reason_chain_id)
            .expect("intent reason chain");
        assert_eq!(chain.decision_id, intent.decision_id);
        assert!(!chain.reasons.is_empty());
    }
}

fn instance_id(scope: &ScopeSelector) -> &str {
    match scope {
        ScopeSelector::Instance { instance_id } => instance_id,
        ScopeSelector::Server { .. } | ScopeSelector::Game { .. } => {
            panic!("H1 fixture tasks must remain instance scoped")
        }
    }
}

fn neutral_sources() -> CatalogSources {
    CatalogSources {
        tasks: source(
            "tasks.json",
            include_bytes!("../../../contracts/scheduling/examples/h1-neutral-activity/tasks.json"),
        ),
        pools: source(
            "pools.json",
            include_bytes!("../../../contracts/scheduling/examples/h1-neutral-activity/pools.json"),
        ),
        activity: source(
            "activity.json",
            include_bytes!(
                "../../../contracts/scheduling/examples/h1-neutral-activity/activity.json"
            ),
        ),
        timeline: source(
            "timeline.json",
            include_bytes!(
                "../../../contracts/scheduling/examples/h1-neutral-activity/timeline.json"
            ),
        ),
    }
}

fn source(name: &str, bytes: &[u8]) -> CatalogDocumentSource {
    CatalogDocumentSource::new(
        format!("memory://h1-neutral-activity/{name}"),
        bytes.to_vec(),
    )
}
