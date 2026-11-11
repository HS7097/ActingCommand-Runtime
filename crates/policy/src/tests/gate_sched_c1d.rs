// SPDX-License-Identifier: AGPL-3.0-only

// #428 SCHED-C1d (register §1.299 SC1-11; the coordinator's ruling 9 of 11-11 and its correction
// of 11-11 13:4x, not Alice's words: the episodes are kept as a tagged program fact under iron
// rule 13; coordinator work order `rd-1111/brief-sched-c1d.md`; contracts/scheduling/README.md,
// "Capped spending order" with "Episodes survive"): the capped spending order's episodes
// - outlive a start: what the Runtime keeps of them (`CappedMemory::kept`) builds the same memory
//   again (`CappedMemory::restore`), and the next cycle goes on in the same round, with the same
//   frozen order and the same members passed, as the uninterrupted run;
// - outlive a new catalog generation that leaves the pool's consumers unchanged (each by its
//   accounting identity, its consumption and its limited mark); any other change of the consumers
//   ends the episode;
// - wait through an unknown reading: nothing of the episode is dispatched while it lasts, its
//   consumers are held with `capped_cap_unknown`, and the first known reading goes on by the same
//   rules (or ends it at the stop line);
// - are restored only from kept episodes that keep the shape's rules; an episode whose pool the
//   catalog does not give a declared valuation is dropped alone;
// - and a memory without an episode keeps nothing.
//
// The catalog is neutral and V2, as SCHED-C1c's: tasks on one instance, the capped pool
// `pool.neutral` read from `resource.neutral`, its capacity read from `resource.neutral.capacity`
// (180), valued 1000 milli a unit (Q = 1), no regeneration, its stop line the capacity less 40
// (140) and its hard line the capacity less 76 (104), no trigger line (the cap); `pool.out` valued
// 1000 milli a unit, so a consumer's value `V` without the pool's cost is its production in milli.
// A run is simulated: the pool is read again half a minute after the dispatch, and the next round
// is a minute after it.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, fact, instance};
use crate::{
    CandidateEligibility, CappedMemory, CappedStateProblem, CatalogDocumentSource, CatalogSources,
    ChainHandoverInputs, CompiledCatalog, DecisionReason, DispatchIntent, EvaluationFacts,
    EvaluationResources, EvaluationTime, FactRefreshInputs, FactRefreshSettings,
    HostResourceSnapshot, InstanceHolds, KeptConsumer, KeptEpisode, KeptMember, KeptRound,
    NoProgressGuards, OwedReruns, PolicyCode, PolicyEvaluation, PolicyEvaluationError, RemedyPlan,
    RunSlots, SCHEDULING_SCHEMA_VERSION_V2, TaskDecision, TaskRuntimeSnapshot, TaskTerminalState,
    compile_catalog, evaluate_with_capped_spending, project_fact_pools,
};
use actingcommand_contract::outcome::{Code, Outcome};

const A: &str = "instance-a";
const GAME: &str = "game-a";
const OPERATION: &str = "operation.run";
const MINUTE_MS: u64 = 60_000;
/// The capped pool, read from `READING_KEY`.
const POOL: &str = "pool.neutral";
const READING_KEY: &str = "resource.neutral";
/// The capacity fact of the capped pool, in its scope.
const CAPACITY_KEY: &str = "resource.neutral.capacity";
/// A fact of the instance test 3's consumers are due on, whatever the pool's reading.
const READY_KEY: &str = "resource.neutral.ready";
/// The produced pool, 1000 milli a unit.
const OUT: &str = "pool.out";
/// The Runtime's deferral of a used-up daily count, which passes a member for its round.
const BUDGET: &str = "policy_budget_exhausted";

// Test 1, a restart in an episode's second round. Read at 240 (cap 180), A, B and C (10 units each,
// `V ÷ c` 3000, 2500 and 2000) are frozen A, B, C and dispatched in turn: 240 → 230 → 220 → 210.
// Round 2 begins at 210 with the same order; its head A is refused by a gate of its own (a used-up
// daily count), so the round passes A (`capped_not_dispatchable`, `gate_code` the count's code)
// and dispatches B as rank 2: 210 → 200. What the Runtime keeps of the memory is episode 1, round
// 2 begun at the round's start from 210, the consumers A, B and C, and the order A (passed, with
// the count's code), B, C; it builds the same memory again. The next cycle after that restart goes
// on in round 2: C is dispatched as rank 3, A stays passed and B waits, exactly as the
// uninterrupted run decides. A restart that kept nothing begins a new episode at 200 instead, whose
// first round dispatches A.
#[test]
fn sched_c1d_t1_a_restart_continues_the_round_from_the_kept_episodes() {
    let mut world = World::new(
        catalog(
            vec![
                judged("task_a", 10, 30),
                judged("task_b", 10, 25),
                judged("task_c", 10, 20),
            ],
            pools(),
        ),
        240,
    );
    for (rank, task_id) in ["task_a", "task_b", "task_c"].into_iter().enumerate() {
        let evaluation = world.evaluate();
        assert_eq!(dispatched(&evaluation), [task_id], "{evaluation:#?}");
        let spend = spend(&evaluation, task_id);
        assert_eq!(spend.value("capped_round"), Some(&json!(1)));
        assert_eq!(spend.value("capped_rank"), Some(&json!(rank + 1)));
        world.run(&evaluation, 10);
    }
    assert_eq!(world.reading, 210);
    let round_start = world.now;
    let refused = world.evaluate_with(|intent| {
        if intent.task_id == "task_a" {
            deferred(BUDGET)
        } else {
            CandidateEligibility::Eligible
        }
    });
    assert_eq!(dispatched(&refused), ["task_b"], "{refused:#?}");
    let second = spend(&refused, "task_b");
    assert_eq!(second.value("capped_episode"), Some(&json!(1)));
    assert_eq!(second.value("capped_round"), Some(&json!(2)));
    assert_eq!(second.value("capped_rank"), Some(&json!(2)));
    world.run(&refused, 10);
    assert_eq!(world.reading, 200);

    let kept = world.memory.kept();
    let consumer = |task_id: &str| KeptConsumer {
        task_id: task_id.to_owned(),
        consumed: 10,
        limited: false,
    };
    let member = |task_id: &str, gate: Option<&str>| KeptMember {
        task_id: task_id.to_owned(),
        consumed: 10,
        passed: gate.is_some(),
        gate_code: None,
    };
    assert_eq!(kept.len(), 1, "{kept:#?}");
    let episode = &kept[0];
    assert_eq!(
        (
            episode.instance_id.as_str(),
            episode.pool_id.as_str(),
            episode.number,
            episode.rounds
        ),
        (A, POOL, 1, 2)
    );
    assert_eq!(
        episode.consumers,
        [consumer("task_a"), consumer("task_b"), consumer("task_c")]
    );
    let round = episode.round.as_ref().expect("round 2 is under way");
    assert_eq!(round.started_at_unix_ms, round_start);
    assert_eq!(round.reading, 210);
    let gates = round
        .order
        .iter()
        .map(|member| member.gate_code.map(Code::as_str))
        .collect::<Vec<_>>();
    assert_eq!(gates, [Some(BUDGET), None, None], "{round:#?}");
    let without_gates = round
        .order
        .iter()
        .map(|member| KeptMember {
            gate_code: None,
            ..member.clone()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        without_gates,
        [
            member("task_a", Some(BUDGET)),
            member("task_b", None),
            member("task_c", None)
        ]
    );

    let restored = CappedMemory::restore(kept, Some(&world.catalog)).expect("the kept episodes");
    assert!(restored.dropped.is_empty(), "{:?}", restored.dropped);
    assert_eq!(restored.memory, world.memory, "the same memory again");
    let mut restarted = world.clone_world();
    restarted.memory = restored.memory;
    let mut forgetful = world.clone_world();
    forgetful.memory = CappedMemory::default();

    let uninterrupted = world.evaluate();
    let after_restart = restarted.evaluate();
    assert_eq!(dispatched(&after_restart), ["task_c"], "{after_restart:#?}");
    let third = spend(&after_restart, "task_c");
    assert_eq!(third.value("capped_episode"), Some(&json!(1)));
    assert_eq!(third.value("capped_round"), Some(&json!(2)));
    assert_eq!(third.value("capped_rank"), Some(&json!(3)));
    let passed = decision(&after_restart, "task_a");
    assert_eq!(hold(passed).as_deref(), Some("capped_not_dispatchable"));
    assert_eq!(
        last(passed).outcome.value("gate_code"),
        Some(&json!(BUDGET))
    );
    assert_eq!(
        hold(decision(&after_restart, "task_b")).as_deref(),
        Some("capped_not_head")
    );
    assert_eq!(
        serde_json::to_vec(&after_restart.decisions).expect("decisions"),
        serde_json::to_vec(&uninterrupted.decisions).expect("decisions"),
        "the restart decides as the uninterrupted run"
    );
    assert_eq!(dispatched(&uninterrupted), dispatched(&after_restart));
    assert_eq!(restarted.memory.episode(A, POOL), Some((1, 2)));

    let forgot = forgetful.evaluate();
    assert_eq!(dispatched(&forgot), ["task_a"], "{forgot:#?}");
    assert_eq!(
        spend(&forgot, "task_a").value("capped_round"),
        Some(&json!(1)),
        "a restart that kept nothing begins a new episode"
    );
}

// Test 2, a new generation. A (30 units, produces 30) and B (25, 20) at 200: A runs as the head of
// episode 1 and the pool reads 170, below the cap and above the stop line. A generation that
// changes the consumers' values and adds a pool of no consumer keeps the episode: B runs as rank 2
// of the same round. So does one whose B now runs as `task_b2`, which continues `task_b`. A
// generation that adds a consumer, removes one, changes one's consumption or marks one limited ends
// the episode: at 170 none begins, and nothing is dispatched (at full price A is worth 0 and B is
// not positive).
#[test]
fn sched_c1d_t2_a_generation_keeps_the_episode_while_its_consumers_are_the_same() {
    let base = || vec![judged("task_a", 30, 30), judged("task_b", 25, 20)];
    let mut world = World::new(catalog(base(), pools()), 200);
    let first = world.evaluate();
    assert_eq!(dispatched(&first), ["task_a"], "{first:#?}");
    world.run(&first, 30);
    assert_eq!(world.reading, 170);
    assert_eq!(world.memory.episode(A, POOL), Some((1, 1)));

    let mut other = pools();
    other
        .as_array_mut()
        .expect("the pools")
        .push(valued("pool.other", 1000, 50));
    let mut renamed = judged("task_b2", 25, 20);
    renamed["continues_from"] = json!(["task_b"]);
    for (case, tasks, pools, head) in [
        (
            "other values and a pool of no consumer",
            vec![judged("task_a", 30, 40), judged("task_b", 25, 22)],
            other,
            "task_b",
        ),
        (
            "B continued as task_b2",
            vec![judged("task_a", 30, 30), renamed],
            pools(),
            "task_b2",
        ),
    ] {
        let mut generation = world.clone_world();
        generation.catalog = catalog(tasks, pools);
        assert_ne!(
            generation.catalog.catalog_hash(),
            world.catalog.catalog_hash(),
            "{case}: a new generation"
        );
        let evaluation = generation.evaluate();
        assert_eq!(dispatched(&evaluation), [head], "{case}: {evaluation:#?}");
        let next = spend(&evaluation, head);
        assert_eq!(next.value("capped_episode"), Some(&json!(1)), "{case}");
        assert_eq!(next.value("capped_round"), Some(&json!(1)), "{case}");
        assert_eq!(next.value("capped_rank"), Some(&json!(2)), "{case}: frozen");
        assert_eq!(generation.memory.episode(A, POOL), Some((1, 1)), "{case}");
    }

    for (case, tasks) in [
        (
            "a consumer added",
            vec![
                judged("task_a", 30, 30),
                judged("task_b", 25, 20),
                judged("task_c", 10, 5),
            ],
        ),
        ("a consumer removed", vec![judged("task_a", 30, 30)]),
        (
            "a consumption changed",
            vec![judged("task_a", 30, 30), judged("task_b", 20, 20)],
        ),
        (
            "a consumer marked limited",
            vec![judged("task_a", 30, 30), limited(judged("task_b", 25, 20))],
        ),
    ] {
        let mut generation = world.clone_world();
        generation.catalog = catalog(tasks, pools());
        let evaluation = generation.evaluate();
        assert!(
            dispatched(&evaluation).is_empty(),
            "{case}: {evaluation:#?}"
        );
        assert_eq!(generation.memory.episode(A, POOL), None, "{case}: ended");
    }
}

// Test 3, an unknown reading in the middle. A (30 units, produces 30) and B (25, 20), each due
// while `resource.neutral.ready` is 1, at 200: A runs as the head of episode 1, then the pool's
// reading is missing. The episode waits: nothing is dispatched, both consumers are held with
// `capped_cap_unknown` naming the reading's key, and the episode stays in round 1. Read again at
// 170, the episode goes on: B runs as rank 2 of the same round, from the round's starting reading.
// At 145 the reading is missing again, the episode waits again, and read at 140 (the stop line) it
// ends by its rule: nothing is dispatched.
#[test]
fn sched_c1d_t3_an_unknown_reading_waits_and_the_next_known_one_goes_on() {
    let ready = |task: Value| with(task, "trigger", at_least(READY_KEY, 1));
    let mut world = World::new(
        catalog(
            vec![
                ready(judged("task_a", 30, 30)),
                ready(judged("task_b", 25, 20)),
            ],
            pools(),
        ),
        200,
    );
    world.extra.insert(READY_KEY.to_owned(), 1);
    let first = world.evaluate();
    assert_eq!(dispatched(&first), ["task_a"], "{first:#?}");
    world.run(&first, 30);

    let wait = |world: &mut World| {
        world.reading_known = false;
        let evaluation = world.evaluate();
        assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
        for task_id in ["task_a", "task_b"] {
            let held = decision(&evaluation, task_id);
            assert_eq!(
                hold(held).as_deref(),
                Some("capped_cap_unknown"),
                "{task_id}: {held:#?}"
            );
            assert_eq!(
                last(held).outcome.value("fact_key"),
                Some(&json!(READING_KEY)),
                "{task_id}"
            );
        }
        assert_eq!(world.memory.episode(A, POOL), Some((1, 1)), "it waits");
        world.reading_known = true;
    };
    wait(&mut world);

    world.set_reading(170);
    let resumed = world.evaluate();
    assert_eq!(dispatched(&resumed), ["task_b"], "{resumed:#?}");
    let next = spend(&resumed, "task_b");
    assert_eq!(next.value("capped_episode"), Some(&json!(1)));
    assert_eq!(next.value("capped_round"), Some(&json!(1)));
    assert_eq!(next.value("capped_rank"), Some(&json!(2)));
    assert_eq!(next.value("pool_reading"), Some(&json!(170)));
    world.run(&resumed, 25);
    assert_eq!(world.reading, 145);

    wait(&mut world);
    world.set_reading(140);
    let ended = world.evaluate();
    assert!(dispatched(&ended).is_empty(), "{ended:#?}");
    assert_eq!(
        world.memory.episode(A, POOL),
        None,
        "spent at the stop line"
    );
}

// Test 4, the restore's rules. Kept episodes that name one pair twice, an episode number 0, no
// consumer, a member that is no consumer, a gate code on a member not passed, or a round without a
// round number are refused whole. An episode of a pool the catalog does not give a declared
// valuation (one it does not have, or without a catalog at all) is dropped alone, and the others
// are restored, each counting on from its own number.
#[test]
fn sched_c1d_t4_the_restore_refuses_broken_episodes_and_drops_an_unvalued_pool() {
    let compiled = catalog(vec![judged("task_a", 30, 30)], pools());
    let episode = |pool_id: &str| KeptEpisode {
        instance_id: A.to_owned(),
        pool_id: pool_id.to_owned(),
        number: 3,
        rounds: 1,
        consumers: vec![KeptConsumer {
            task_id: "task_a".to_owned(),
            consumed: 30,
            limited: false,
        }],
        round: Some(KeptRound {
            started_at_unix_ms: NOW,
            reading: 200,
            order: vec![KeptMember {
                task_id: "task_a".to_owned(),
                consumed: 30,
                passed: false,
                gate_code: None,
            }],
        }),
    };
    let invalid = |pool_id: &str| CappedStateProblem::Invalid {
        instance_id: A.to_owned(),
        pool_id: pool_id.to_owned(),
    };
    assert_eq!(
        CappedMemory::restore(vec![episode(POOL), episode(POOL)], Some(&compiled)),
        Err(CappedStateProblem::Repeated {
            instance_id: A.to_owned(),
            pool_id: POOL.to_owned(),
        })
    );
    let gate = PolicyCode::PoolLineHeld.code();
    for (case, broken) in [
        ("number 0", {
            let mut broken = episode(POOL);
            broken.number = 0;
            broken
        }),
        ("no consumer", {
            let mut broken = episode(POOL);
            broken.consumers.clear();
            broken
        }),
        ("a member that is no consumer", {
            let mut broken = episode(POOL);
            if let Some(round) = broken.round.as_mut() {
                round.order[0].task_id = "task_x".to_owned();
            }
            broken
        }),
        ("a gate code on a member not passed", {
            let mut broken = episode(POOL);
            if let Some(round) = broken.round.as_mut() {
                round.order[0].gate_code = Some(gate);
            }
            broken
        }),
        ("a round without a round number", {
            let mut broken = episode(POOL);
            broken.rounds = 0;
            broken
        }),
    ] {
        assert_eq!(
            CappedMemory::restore(vec![broken], Some(&compiled)),
            Err(invalid(POOL)),
            "{case}"
        );
    }

    let restored =
        CappedMemory::restore(vec![episode("pool.gone"), episode(POOL)], Some(&compiled))
            .expect("the valid episodes");
    assert_eq!(
        restored.dropped,
        [(A.to_owned(), "pool.gone".to_owned())],
        "dropped alone"
    );
    assert_eq!(restored.memory.episode(A, POOL), Some((3, 1)));
    assert_eq!(restored.memory.episode(A, "pool.gone"), None);
    assert_eq!(restored.memory.kept(), [episode(POOL)], "kept as it came");

    let without = CappedMemory::restore(vec![episode(POOL)], None).expect("no catalog");
    assert!(without.memory.is_empty());
    assert_eq!(without.dropped, [(A.to_owned(), POOL.to_owned())]);
}

// Test 5, identity: a memory without an episode keeps nothing, whether no pool is valued or the
// valued pool stays below its cap (170 of 180: A at full price is worth 0 and is not dispatched).
#[test]
fn sched_c1d_t5_without_an_episode_nothing_is_kept() {
    assert!(CappedMemory::default().kept().is_empty());
    let mut below = World::new(catalog(vec![judged("task_a", 30, 30)], pools()), 170);
    let evaluation = below.evaluate();
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    assert!(below.memory.kept().is_empty());
    let mut bare = pools();
    for pool in bare.as_array_mut().expect("the pools") {
        pool.as_object_mut()
            .expect("a pool object")
            .shift_remove("valuation");
    }
    let mut unvalued = World::new(catalog(vec![task_body("task_u", 100)], bare), 200);
    let _evaluation = unvalued.evaluate();
    assert!(unvalued.memory.kept().is_empty());
}

// ---- The world a test drives ----

/// The catalog, the Runtime's memory of the capped episodes, the capped pool's reading (missing
/// while `reading_known` is false), other facts of its scope and the tasks' snapshots.
struct World {
    catalog: CompiledCatalog,
    memory: CappedMemory,
    reading: i64,
    reading_known: bool,
    read_at: u64,
    now: u64,
    extra: BTreeMap<String, i64>,
    states: Vec<TaskRuntimeSnapshot>,
}

impl World {
    /// The world at NOW, the capped pool read at `reading` a minute before.
    fn new(catalog: CompiledCatalog, reading: i64) -> Self {
        Self {
            catalog,
            memory: CappedMemory::default(),
            reading,
            reading_known: true,
            read_at: NOW - MINUTE_MS,
            now: NOW,
            extra: BTreeMap::new(),
            states: Vec::new(),
        }
    }

    fn clone_world(&self) -> Self {
        Self {
            catalog: self.catalog.clone(),
            memory: self.memory.clone(),
            reading: self.reading,
            reading_known: self.reading_known,
            read_at: self.read_at,
            now: self.now,
            extra: self.extra.clone(),
            states: self.states.clone(),
        }
    }

    /// A new reading of the pool, observed now; the next round is a minute later.
    fn set_reading(&mut self, reading: i64) {
        self.reading = reading;
        self.read_at = self.now;
        self.now += MINUTE_MS;
    }

    fn facts(&self) -> EvaluationFacts {
        let mut facts = vec![fact(A, CAPACITY_KEY, 180, NOW - HOUR_MS, None)];
        if self.reading_known {
            facts.push(fact(A, READING_KEY, self.reading, self.read_at, None));
        }
        for (fact_key, value) in &self.extra {
            facts.push(fact(A, fact_key, *value, NOW - HOUR_MS, None));
        }
        EvaluationFacts {
            ledger_position: 7,
            fact_snapshot_id: "snapshot:fixture-sched-c1d".to_owned(),
            facts,
            outcomes: Vec::new(),
            tasks: self.states.clone(),
            instances: vec![instance(A, &[])],
            priority_offsets: Vec::new(),
        }
    }

    /// One round by the Runtime's entry, with its memory, every candidate admitted.
    fn evaluate(&mut self) -> PolicyEvaluation {
        self.evaluate_with(|_| CandidateEligibility::Eligible)
    }

    /// One round by the Runtime's entry, with its memory and `admission`'s answers.
    fn evaluate_with(
        &mut self,
        admission: impl Fn(&DispatchIntent) -> CandidateEligibility,
    ) -> PolicyEvaluation {
        let facts = self.facts();
        let resources = resources(&self.catalog, &facts);
        evaluate_with_capped_spending::<PolicyEvaluationError>(
            &self.catalog,
            &facts,
            &resources,
            time(self.now),
            11,
            &BTreeSet::new(),
            &refresh_inputs(),
            &RemedyPlan::default(),
            &ChainHandoverInputs::default(),
            &RunSlots::default(),
            &OwedReruns::default(),
            &InstanceHolds::default(),
            &NoProgressGuards::default(),
            &mut self.memory,
            |_, _, _| Ok(false),
            |intent: &DispatchIntent| -> Result<CandidateEligibility, PolicyEvaluationError> {
                Ok(admission(intent))
            },
        )
        .unwrap_or_else(|error| panic!("the round evaluates: {error}"))
    }

    /// Runs the round's one dispatch: it succeeds having consumed `spent`; the pool is read half a
    /// minute after the dispatch and the next round is a minute after it.
    fn run(&mut self, evaluation: &PolicyEvaluation, spent: i64) {
        let [intent] = evaluation.dispatch_intents.as_slice() else {
            panic!("one dispatch: {evaluation:#?}");
        };
        self.states.retain(|state| state.task_id != intent.task_id);
        self.states.push(TaskRuntimeSnapshot {
            task_id: intent.task_id.clone(),
            instance_id: A.to_owned(),
            last_dispatched_unix_ms: Some(self.now),
            eligible_since_unix_ms: None,
            terminal_state: Some(TaskTerminalState::Succeeded),
            completed_window: None,
            last_duration_ms: None,
            failure_streak: None,
            expected_runtime_ms: None,
            expected_runtime_runs: None,
        });
        self.reading -= spent;
        self.read_at = self.now + 30_000;
        self.now += MINUTE_MS;
    }
}

fn time(at: u64) -> EvaluationTime {
    EvaluationTime {
        unix_ms: at,
        monotonic_ms: at,
    }
}

fn resources(catalog: &CompiledCatalog, facts: &EvaluationFacts) -> EvaluationResources {
    project_fact_pools(
        catalog,
        facts,
        &EvaluationResources {
            pools: Vec::new(),
            hosts: vec![HostResourceSnapshot {
                host_id: format!("host.{A}"),
                cpu_available_milli: 1_000,
                gpu_available_milli: 1_000,
                io_available_milli: 1_000,
                host_responsiveness_basis_points: 10_000,
                third_party_pressure_basis_points: 0,
                heavy_dispatch_limit: 1,
                active_heavy_dispatches: 0,
                heavy_threshold_milli: crate::DEFAULT_HEAVY_THRESHOLD_MILLI,
            }],
        },
    )
}

/// The fact refresh off: no reader is bound.
fn refresh_inputs() -> FactRefreshInputs {
    FactRefreshInputs {
        settings: FactRefreshSettings {
            enabled: false,
            min_interval_ms: 300_000,
            daily_limit: 96,
            failure_backoff_ms: 600_000,
            max_consecutive_failures: 3,
            expected_duration_ms: crate::FACT_REFRESH_EXPECTED_DURATION_MS,
        },
        readers: Vec::new(),
        instance_facts_loaded_at_unix_ms: Some(NOW - DAY_MS),
        instance_holds: BTreeMap::new(),
        idle_closed: BTreeMap::new(),
    }
}

/// The admission's deferral by the registered code `code`.
fn deferred(code: &str) -> CandidateEligibility {
    CandidateEligibility::Deferred {
        reason: DecisionReason {
            outcome: Outcome::relayed(code),
            detail: format!("fixture admission: {code}"),
        },
        next_wake_unix_ms: None,
    }
}

// ---- Assertions ----

fn dispatched(evaluation: &PolicyEvaluation) -> Vec<&str> {
    evaluation
        .dispatch_intents
        .iter()
        .map(|intent| intent.task_id.as_str())
        .collect()
}

fn decision<'a>(evaluation: &'a PolicyEvaluation, task_id: &str) -> &'a TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|decision| decision.task_id == task_id && decision.instance_id.as_deref() == Some(A))
        .unwrap_or_else(|| panic!("a decision for {task_id} on {A}: {evaluation:#?}"))
}

fn reason(decision: &TaskDecision, code: PolicyCode) -> &DecisionReason {
    decision
        .reasons
        .iter()
        .find(|reason| reason.outcome.is(code))
        .unwrap_or_else(|| panic!("a {} reason on {decision:#?}", code.as_str()))
}

fn last(decision: &TaskDecision) -> &DecisionReason {
    decision
        .reasons
        .last()
        .unwrap_or_else(|| panic!("a reason on {decision:#?}"))
}

/// The `pool_line_hold` of the decision's last reason, when it is `pool_line_held`.
fn hold(decision: &TaskDecision) -> Option<String> {
    let last = last(decision);
    if !last.outcome.is(PolicyCode::PoolLineHeld) {
        return None;
    }
    last.outcome
        .value("pool_line_hold")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// The `capped_spend` reason of `task_id`'s dispatch.
fn spend<'a>(evaluation: &'a PolicyEvaluation, task_id: &str) -> &'a Outcome {
    &reason(decision(evaluation, task_id), PolicyCode::CappedSpend).outcome
}

// ---- The catalog ----

fn effect(pool: &str, direction: &str, amount: u64) -> Value {
    json!({
        "pool_id": pool,
        "direction": direction,
        "amount": amount,
        "observation_source": "scan_verified",
        "confidence_milli": 1000
    })
}

/// Due while the capped pool holds `amount`.
fn enough(amount: u64) -> Value {
    json!({
        "kind": "resource_projection",
        "pool_id": POOL,
        "comparison": "greater_than_or_equal",
        "value": amount
    })
}

fn at_least(fact_key: &str, threshold: i64) -> Value {
    json!({
        "kind": "fact",
        "scope": {"kind": "instance", "instance_id": A},
        "fact_key": fact_key,
        "comparison": "greater_than_or_equal",
        "value": {"type": "integer", "value": threshold},
        "max_age_ms": DAY_MS
    })
}

fn hourly() -> Value {
    json!({
        "kind": "clock",
        "schedule": {
            "kind": "interval",
            "clock_source": {"kind": "local"},
            "every_ms": HOUR_MS,
            "anchor_ms": 0
        }
    })
}

/// An unmarked task of the game on its hourly clock, of `priority`, with no effect.
fn task_body(id: &str, priority: i16) -> Value {
    json!({
        "id": id,
        "scope": {"kind": "game", "game_id": GAME},
        "entrypoint": {"operation_id": OPERATION},
        "procedure_ref": format!("procedure.{id}"),
        "priority": priority,
        "trigger": hourly(),
        "feedback_stop": {
            "kind": "outcome",
            "task_id": id,
            "outcome_key": "completed",
            "comparison": "eq",
            "value": {"type": "boolean", "value": true}
        },
        "consumes": [],
        "produces": [],
        "on_failure": {
            "action": "continue",
            "retry_limit": 2,
            "retry_backoff_ms": 30000,
            "escalation_threshold": 3
        },
        "sensitive": false,
        "next_run_clamp_ms": DAY_MS,
        "yield_points": [],
        "expected_duration_ms": 60_000,
        "cooldown_ms": 1000,
        "load_profile": {"kind": "weighted", "cpu_milli": 200, "gpu_milli": 100, "io_milli": 300},
        "strategic_weight_milli": 1000,
        "instance_overrides": []
    })
}

/// A judged task: `consumed` units of the capped pool a run, due while the pool holds them, and
/// `produced` units of `pool.out`.
fn judged(id: &str, consumed: u64, produced: u64) -> Value {
    let mut task = task_body(id, 100);
    task["consumes"] = json!([effect(POOL, "consume", consumed)]);
    task["produces"] = json!([effect(OUT, "produce", produced)]);
    task["benefit_judged"] = json!(true);
    task["trigger"] = enough(consumed);
    task
}

fn limited(mut task: Value) -> Value {
    task["benefit_limited"] = json!(true);
    task
}

fn with(mut task: Value, name: &str, value: Value) -> Value {
    task[name] = value;
    task
}

/// A pool of the instance that does not regenerate, read from `resource.<the id without
/// "pool.">`, of constant `capacity`, weighed `weight` milli a unit.
fn valued(id: &str, weight: u32, capacity: u64) -> Value {
    json!({
        "id": id,
        "scope": {"kind": "instance", "instance_id": A},
        "capacity": capacity,
        "projection": {"kind": "none"},
        "observation": {"kind": "fact", "fact_key": format!("resource.{}", &id["pool.".len()..])},
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000},
        "valuation": {"name": "Fixture", "unit": "unit", "scale": 1, "base_weight_milli": weight}
    })
}

/// The capacity fact plus `offset`.
fn line(offset: i64) -> Value {
    json!({"type": "fact", "fact_key": CAPACITY_KEY, "offset": offset})
}

/// The produced pool and the capped pool: its capacity read from its fact, its stop line 40 below
/// the cap and its hard line 76 below it.
fn pools() -> Value {
    let mut capped = valued(POOL, 1000, 1);
    capped["capacity"] = line(0);
    capped["stop_line"] = line(-40);
    capped["hard_line"] = line(-76);
    json!([valued(OUT, 1000, 1_000_000), capped])
}

fn catalog(tasks: Vec<Value>, pools: Value) -> CompiledCatalog {
    let header = json!({
        "catalog_id": "fixture.sched-c1d",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-sched-c1d"]
    });
    let document = |member: &str, entries: Value| {
        let mut document = json!({
            "schema_version": SCHEDULING_SCHEMA_VERSION_V2,
            "catalog": header,
        });
        document[member] = entries;
        document
    };
    let profile = json!({
        "id": "fixture-activity",
        "scope": {"kind": "game", "game_id": GAME},
        "windows": [{
            "weekdays": [1, 2, 3, 4, 5, 6, 7],
            "utc_offset_minutes": 0,
            "start_minute_of_day": 0,
            "end_minute_of_day": 0
        }],
        "daily_budget": 24,
        "max_window_iterations": 4,
        "session_max_ms": 7200000,
        "detection_budget": {
            "window_dispatch_limit": 4,
            "window_runtime_ms": 300000,
            "expected_duration_ms": 60000
        },
        "minimum_interval_ms": 60000,
        "maximum_interval_ms": 300000,
        "seed_source": "ledger",
        "resample_policy": "same_round_stable",
        "importance_milli": 1000,
        "goals": []
    });
    compile_catalog(&CatalogSources {
        tasks: source("tasks", &document("tasks", Value::Array(tasks))),
        pools: source("pools", &document("pools", pools)),
        activity: source("activity", &document("profiles", json!([profile]))),
        timeline: source("timeline", &document("events", json!([]))),
        selection: None,
    })
    .unwrap_or_else(|failure| panic!("the fixture catalog compiles: {:?}", failure.diagnostics()))
}

fn source(name: &str, document: &Value) -> CatalogDocumentSource {
    CatalogDocumentSource::new(
        format!("memory://fixture-sched-c1d/{name}.json"),
        serde_json::to_vec(document).expect("fixture document bytes"),
    )
}
