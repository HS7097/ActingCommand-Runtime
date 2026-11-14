// SPDX-License-Identifier: AGPL-3.0-only

// ROUT-L RL-1b (frozen model v1.0 §3.3–§3.6, §3.8, §3.9, §6.1 row RL-1b, §7.1 A2–A6, A8, A9;
// Alice's ruling 51): the scheduler workbench of the host. Each test runs the host in composer
// mode (no configured catalog) over neutral fixture game roots on disk, a manual Runtime clock
// and fake instances; the rules are catalog-a's task, pool, profiles and event in the
// `schedule.json` shape. A check runs at the start of each cycle at most once per
// `standard_packs.check_interval_ms` (default 60 000), which each test passes by moving the
// monotonic clock. Every expected state comes from the model rows named at each test.

use super::*;
use crate::codes::{HostCode, HostLocation};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::{Code, Outcome};
use actingcommand_contract::{
    CONFIG_CATALOG_PROVENANCE_FACT_KEY, CONFIG_CATALOG_WARNINGS_FACT_KEY, CatalogCompileWarning,
    CatalogProvenanceCount, CatalogProvenanceEntry, CatalogValueKind, CatalogValueOrigin,
    ConfigCatalogProvenance, ConfigCatalogWarnings, ConfigParameter, ConfigParameterSource,
    FactScalar, RuntimeConfigManifest, RuntimeFactRecord, RuntimeFactScope, RuntimeLifecyclePhase,
    RuntimePayload,
};
use actingcommand_policy::{
    STANDARD_PACKS_CATALOG_ID, StandardPackBinding, StandardPackComposeInput,
    StandardPackGameFiles, compose_standard_pack_rules,
};
use serde_json::{Value, json};

pub(super) const GAME_A: &str = "fixture-game-a";
const GAME_B: &str = "fixture-game-b";
pub(super) const SERVER_A: &str = "fixture-server-a";
const SERVER_B: &str = "fixture-server-b";
const INSTANCE_A: &str = POLICY_INSTANCE_ALIAS;
const INSTANCE_B: &str = POLICY_INSTANCE_ALIAS_B;
const RULE_1: &str = "fixture.observe";
const RULE_2: &str = "fixture.second";
const RULE_B: &str = "fixture.other";
const PACKAGE_1: &str = "fixture.observe";
const PACKAGE_2: &str = "fixture.second";
const PACKAGE_B: &str = "fixture.other";
pub(super) const POOL_A: &str = "fixture-pool-a";
const POOL_B: &str = "fixture-pool-b";
/// The default `standard_packs.check_interval_ms`.
const INTERVAL_MS: u64 = 60_000;
/// The least `standard_packs.check_interval_ms`: within the fixture scheduler's lease TTL
/// (5 000 ms), so a dispatch admitted before a check still holds its lease after it.
const SHORT_INTERVAL_MS: u64 = 1_000;
const HOUR_MS: u64 = 3_600_000;

// ---------------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------------

fn parse(text: &str) -> Value {
    serde_json::from_str(text).expect("fixture JSON")
}

/// Removes every `scope` member at any depth: `schedule.json` names no instance (§3.1).
fn strip_scopes(value: &mut Value) {
    match value {
        Value::Object(members) => {
            members.shift_remove("scope");
            for (_, member) in members.iter_mut() {
                strip_scopes(member);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(strip_scopes),
        _ => {}
    }
}

/// catalog-a's task as the rule `id` of `package_id` at `priority`: no scope, entrypoint,
/// procedure or instance override; no yield point (the package operation has none); it never
/// stops by feedback; it produces into `pool` when given.
pub(super) fn rule(id: &str, package_id: &str, priority: u64, pool: Option<&str>) -> Value {
    let tasks = parse(include_str!(
        "../../../../contracts/scheduling/examples/catalog-a/tasks.json"
    ));
    let mut rule = tasks["tasks"][0].clone();
    strip_scopes(&mut rule);
    let fields = rule.as_object_mut().expect("a task object");
    for member in ["entrypoint", "procedure_ref", "instance_overrides"] {
        fields.shift_remove(member);
    }
    fields.insert("id".to_owned(), json!(id));
    fields.insert("package_id".to_owned(), json!(package_id));
    fields.insert("priority".to_owned(), json!(priority));
    fields.insert("yield_points".to_owned(), json!([]));
    fields.insert(
        "feedback_stop".to_owned(),
        json!({
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
        }),
    );
    let produces = match pool {
        Some(pool) => {
            let mut produced = fields["produces"].clone();
            produced[0]["pool_id"] = json!(pool);
            produced
        }
        None => json!([]),
    };
    fields.insert("produces".to_owned(), produces);
    rule
}

/// catalog-a's pool as the pool `id`, with no scope. SS-2b (register §1.225): with a valuation,
/// so a composed rule that produces into it has a known positive net benefit and is dispatched as
/// before; the composer judges every rule here, none of the fixture packages being
/// maintenance-role.
pub(super) fn pool(id: &str) -> Value {
    let pools = parse(include_str!(
        "../../../../contracts/scheduling/examples/catalog-a/pools.json"
    ));
    let mut pool = pools["pools"][0].clone();
    strip_scopes(&mut pool);
    pool["id"] = json!(id);
    pool["valuation"] =
        json!({"name": "Fixture", "unit": "unit", "scale": 1, "base_weight_milli": 1000});
    pool
}

/// catalog-a's profiles with no scope, their ids suffixed by `suffix`; a profile's goals name
/// `pool`, or none without one.
pub(super) fn profiles(suffix: &str, pool: Option<&str>) -> Vec<Value> {
    let activity = parse(include_str!(
        "../../../../contracts/scheduling/examples/catalog-a/activity.json"
    ));
    activity["profiles"]
        .as_array()
        .expect("profiles")
        .iter()
        .map(|profile| {
            let mut profile = profile.clone();
            strip_scopes(&mut profile);
            let id = profile["id"].as_str().expect("profile id").to_owned();
            profile["id"] = json!(format!("{id}{suffix}"));
            match pool {
                Some(pool) => {
                    if let Some(goals) = profile["goals"].as_array_mut() {
                        for goal in goals {
                            goal["metric"]["pool_id"] = json!(pool);
                            let goal_id = goal["id"].as_str().expect("goal id").to_owned();
                            goal["id"] = json!(format!("{goal_id}{suffix}"));
                        }
                    }
                }
                None => profile["goals"] = json!([]),
            }
            profile
        })
        .collect()
}

/// catalog-a's event with no scope, open-ended from the epoch (the V2 validity).
fn event(id: &str) -> Value {
    let timeline = parse(include_str!(
        "../../../../contracts/scheduling/examples/catalog-a/timeline.json"
    ));
    let mut event = timeline["events"][0].clone();
    strip_scopes(&mut event);
    event["id"] = json!(id);
    event["validity"] = json!({"from_unix_ms": 0, "until_unix_ms": null});
    event
}

/// Game A's `schedule.json`: `rules` (id, package, priority), the first producing into pool A,
/// with pool A, catalog-a's two profiles and its event.
pub(super) fn schedule_a(rules: &[(&str, &str, u64)]) -> Value {
    json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": [],
        "rules": rules
            .iter()
            .enumerate()
            .map(|(index, (id, package, priority))| {
                rule(id, package, *priority, (index == 0).then_some(POOL_A))
            })
            .collect::<Vec<_>>(),
        "pools": [pool(POOL_A)],
        "activity": profiles("", Some(POOL_A)),
        "timeline": [event("fixture-reset-a")],
        "held": []
    })
}

/// Game B's `schedule.json`: rule B at `priority`, its own profiles, and pool B when asked.
fn schedule_b(priority: u64, with_pool: bool) -> Value {
    let pools = if with_pool {
        json!([pool(POOL_B)])
    } else {
        json!([])
    };
    json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": [],
        "rules": [rule(RULE_B, PACKAGE_B, priority, with_pool.then_some(POOL_B))],
        "pools": pools,
        "activity": profiles("-b", with_pool.then_some(POOL_B)),
        "timeline": [],
        "held": []
    })
}

/// A bundle index v4 of `game` registering `packages` on `server`.
pub(super) fn bundle(game: &str, server: &str, packages: &[&str]) -> Value {
    json!({
        "schema_version": "actingcommand.bundle.v4",
        "game": game,
        "packs": packages
            .iter()
            .map(|package_id| json!({
                "package_id": package_id,
                "server": server,
                "entry_task_id": "task",
                "path": format!("packs/{package_id}"),
                "file_count": 1,
                "byte_count": 64
            }))
            .collect::<Vec<_>>()
    })
}

/// Workflow #455 G-R2s: the composer's input for `root` when every package its registry lists
/// reads and provides nothing.
pub(super) fn readable_root(
    root: &actingcommand_contract::StandardPackRoot,
) -> actingcommand_contract::IrreversiblePackages {
    actingcommand_contract::IrreversiblePackages::new(
        root.registry
            .package_ids()
            .map(|package_id| (package_id.to_owned(), Some(BTreeMap::new())))
            .collect(),
    )
}

/// Workflow #455 G-R2s: the composer's input for `root` when no package its registry lists reads,
/// as the workbench reads a root whose packages have no files.
pub(super) fn unread_root(
    root: &actingcommand_contract::StandardPackRoot,
) -> actingcommand_contract::IrreversiblePackages {
    actingcommand_contract::IrreversiblePackages::new(
        root.registry
            .package_ids()
            .map(|package_id| (package_id.to_owned(), None))
            .collect(),
    )
}

pub(super) fn write_json(path: &Path, value: &Value) {
    fs::create_dir_all(path.parent().expect("a parent directory")).expect("create the game root");
    fs::write(
        path,
        serde_json::to_vec_pretty(value).expect("fixture bytes"),
    )
    .expect("write a root file");
}

/// A configuration that sets each `(key, value)` and nothing else.
fn manifest(values: &[(&str, FactScalar)]) -> RuntimeConfigManifest {
    let mut parameters = values
        .iter()
        .map(|(key, value)| ConfigParameter {
            key: (*key).to_owned(),
            value: value.clone(),
            source: ConfigParameterSource::Explicit,
            settable: false,
        })
        .collect::<Vec<_>>();
    actingcommand_runtime_config::annotate_settable(&mut parameters)
        .expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

/// `standard_packs.cached_generations = 0`.
fn no_cache() -> RuntimeConfigManifest {
    manifest(&[(
        actingcommand_runtime_config::STANDARD_PACKS_CACHED_GENERATIONS,
        FactScalar::Integer(0),
    )])
}

/// `standard_packs.cached_generations = 0` and `standard_packs.check_interval_ms` at its least,
/// `SHORT_INTERVAL_MS`.
fn no_cache_short_interval() -> RuntimeConfigManifest {
    manifest(&[
        (
            actingcommand_runtime_config::STANDARD_PACKS_CACHED_GENERATIONS,
            FactScalar::Integer(0),
        ),
        (
            actingcommand_runtime_config::STANDARD_PACKS_CHECK_INTERVAL_MS,
            FactScalar::DurationMs(SHORT_INTERVAL_MS),
        ),
    ])
}

/// One bound instance: alias, game and server.
type Bound = (&'static str, &'static str, &'static str);

const BOUND_A: Bound = (INSTANCE_A, GAME_A, SERVER_A);
const BOUND_B: Bound = (INSTANCE_B, GAME_B, SERVER_B);

fn facts(bound: &[Bound]) -> EvaluationFacts {
    let mut facts = policy_facts();
    facts.outcomes.clear();
    facts.instances = bound
        .iter()
        .map(|(alias, game, server)| InstanceSnapshot {
            instance_id: (*alias).to_owned(),
            server_id: (*server).to_owned(),
            game_id: (*game).to_owned(),
            host_id: "fixture-host-a".to_owned(),
            available: true,
            capability_operation_ids: vec![
                actingcommand_policy::STANDARD_PACK_OPERATION_ID.to_owned(),
            ],
            preferred_task_ids: Vec::new(),
        })
        .collect();
    facts
}

/// The fixture's resources with a static reading for each of `pools`.
pub(super) fn resources(pools: &[&str]) -> EvaluationResources {
    let mut resources = policy_resources();
    resources.pools = pools
        .iter()
        .map(|pool_id| PoolValueSnapshot {
            pool_id: (*pool_id).to_owned(),
            value: 10,
            observed_at_unix_ms: POLICY_NOW_UNIX_MS,
        })
        .collect();
    resources
}

/// One host, its game roots and its manual clock.
struct Bench {
    host: RuntimeHost,
    clock: Arc<ManualRuntimeClock>,
    state: TempDir,
    packs: TempDir,
    bound: Vec<Bound>,
    instances: Vec<InstanceId>,
    facts: EvaluationFacts,
    resources: EvaluationResources,
    manifest: Option<RuntimeConfigManifest>,
}

impl Bench {
    /// A host for the instances `bound` over an empty standard-pack root: in composer mode
    /// when `composer`, else with the fixture's configured procedure manifest.
    fn new(
        bound: &[Bound],
        pools: &[&str],
        composer: bool,
        manifest: Option<RuntimeConfigManifest>,
    ) -> Self {
        Self::with_facts(bound, pools, composer, manifest, facts(bound))
    }

    /// As `new`, with `facts` as the configured instance inputs from the start: the start seeds
    /// the instance facts from them, so a cycle at the start instant seeds nothing new.
    fn with_facts(
        bound: &[Bound],
        pools: &[&str],
        composer: bool,
        manifest: Option<RuntimeConfigManifest>,
        facts: EvaluationFacts,
    ) -> Self {
        let state = TempDir::new().expect("state root");
        let packs = TempDir::new().expect("standard-pack root");
        let instances = bound.iter().map(|_| instance_id()).collect::<Vec<_>>();
        let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
        let host = Self::start_host(
            &state,
            &packs,
            &facts,
            bound,
            &instances,
            &clock,
            composer,
            manifest.clone(),
        );
        Self {
            host,
            clock,
            state,
            packs,
            bound: bound.to_vec(),
            instances,
            facts,
            resources: resources(pools),
            manifest,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn start_host(
        state: &TempDir,
        packs: &TempDir,
        facts: &EvaluationFacts,
        bound: &[Bound],
        instances: &[InstanceId],
        clock: &Arc<ManualRuntimeClock>,
        composer: bool,
        manifest: Option<RuntimeConfigManifest>,
    ) -> RuntimeHost {
        let mut host_config = config(state)
            .with_policy_inputs(PolicyInputSnapshot::new(facts.clone(), policy_resources()))
            .with_runtime_clock(clock.clone())
            .with_standard_packs_root(Some(packs.path().to_owned()))
            .with_standard_pack_schedule(composer);
        if let Some(manifest) = manifest {
            host_config = host_config.with_config_manifest(manifest);
        }
        let provider = FakeProvider::from_entries(bound.iter().zip(instances).map(
            |((alias, _, _), instance)| {
                (
                    (*alias).to_owned(),
                    *instance,
                    Arc::new(FakeState::default()),
                )
            },
        ));
        RuntimeHost::start(host_config, Arc::new(provider)).expect("start the workbench host")
    }

    /// A clean stop and a start on the same state root, in composer mode when `composer`.
    fn restart(self, composer: bool) -> Self {
        let Self {
            host,
            clock,
            state,
            packs,
            bound,
            instances,
            facts,
            resources,
            manifest,
        } = self;
        host.close().expect("close the workbench host");
        let host = Self::start_host(
            &state,
            &packs,
            &facts,
            &bound,
            &instances,
            &clock,
            composer,
            manifest.clone(),
        );
        Self {
            host,
            clock,
            state,
            packs,
            bound,
            instances,
            facts,
            resources,
            manifest,
        }
    }

    fn path(&self, game: &str, file: &str) -> PathBuf {
        self.packs.path().join(game).join(file)
    }

    fn place(&self, game: &str, server: &str, packages: &[&str], schedule: Option<&Value>) {
        write_json(
            &self.path(game, "bundle.json"),
            &bundle(game, server, packages),
        );
        match schedule {
            Some(schedule) => write_json(&self.path(game, "schedule.json"), schedule),
            None => match fs::remove_file(self.path(game, "schedule.json")) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => panic!("remove schedule.json: {error}"),
            },
        }
    }

    /// One cycle at `unix_ms` with the monotonic clock at `monotonic_ms`.
    fn cycle(&self, unix_ms: u64, monotonic_ms: u64) -> PolicyCycle {
        self.clock.set_unix_ms(unix_ms);
        self.clock.set_monotonic_ms(monotonic_ms);
        // The host's capacity at the new time, as its capacity monitor samples it while time
        // passes (`gate_372k2`'s `Slots`).
        let sample_capacity = self
            .host
            .capacity_sampler_for_test()
            .expect("capacity owner");
        sample_capacity().expect("capacity sample");
        self.host
            .evaluate_policy_cycle_with_test_inputs(
                &self.facts,
                &self.resources,
                EvaluationTime {
                    unix_ms,
                    monotonic_ms: unix_ms,
                },
                7,
                PolicyTrigger::Recovery,
            )
            .unwrap_or_else(|error| panic!("cycle at {unix_ms}/{monotonic_ms}: {error}"))
    }

    fn active(&self) -> CatalogGeneration {
        self.host
            .active_policy_catalog()
            .expect("read the active catalog")
            .expect("an active catalog")
    }

    fn events(&self, event_type: EventType) -> Vec<PersistedEvent> {
        self.host
            .query_persisted_events_for_test(EventQuery {
                event_type: Some(event_type),
                ..EventQuery::default()
            })
            .expect("ledger events")
    }

    /// The outcomes of the `standard_pack_read` lifecycle records, in ledger order.
    fn standard_pack_reads(&self) -> Vec<Outcome> {
        self.events(EventType::RuntimeLifecycleObserved)
            .iter()
            .filter_map(|event| match event.payload() {
                EventPayload::Runtime(RuntimePayload::LifecycleObserved(value)) => {
                    match value.phase() {
                        RuntimeLifecyclePhase::StandardPackRead { outcome } => {
                            Some(outcome.clone())
                        }
                        _ => None,
                    }
                }
                _ => None,
            })
            .collect()
    }

    /// The `standard_pack_read` outcomes of `code` for `game` (any game when `None`).
    fn reads_of(&self, code: impl Into<Code>, game: Option<&str>) -> Vec<Outcome> {
        let code = code.into();
        self.standard_pack_reads()
            .into_iter()
            .filter(|outcome| outcome.is(code))
            .filter(|outcome| {
                game.is_none_or(|game| outcome.value("game").and_then(Value::as_str) == Some(game))
            })
            .collect()
    }

    fn close(self) {
        self.host.close().expect("close the workbench host");
    }
}

fn decided(cycle: &PolicyCycle, task_id: &str, instance_id: &str) -> bool {
    cycle.evaluation.as_ref().is_some_and(|evaluation| {
        evaluation.decisions.iter().any(|decision| {
            decision.task_id == task_id && decision.instance_id.as_deref() == Some(instance_id)
        })
    })
}

fn intent<'a>(cycle: &'a PolicyCycle, task_id: &str) -> Option<&'a DispatchIntent> {
    cycle.evaluation.as_ref().and_then(|evaluation| {
        evaluation
            .dispatch_intents
            .iter()
            .find(|intent| intent.task_id == task_id)
    })
}

/// Admits the dispatch `cycle` selected for `task_id` with no approval fact.
fn admit(bench: &Bench, cycle: &PolicyCycle, task_id: &str) -> (DispatchIntent, PolicyRunContext) {
    let evaluation = cycle.evaluation.as_ref().expect("an evaluation");
    let intent = intent(cycle, task_id)
        .unwrap_or_else(|| {
            panic!(
                "{task_id} is selected: {:#?} {evaluation:#?}",
                bench.standard_pack_reads()
            )
        })
        .clone();
    let reasons = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .expect("the intent's reason chain")
        .clone();
    let context = PolicyAdmissionContext {
        fact_ledger_position: intent.input_ledger_position,
        fact_snapshot_id: intent.fact_snapshot_id.clone(),
        approval_fact_ids: BTreeSet::new(),
        fencing_owner_epoch: bench.host.runtime_info().owner_epoch(),
        now_unix_ms: intent.prerequisites.evaluated_at_unix_ms,
    };
    let PolicyDispatchAdmission::Granted { context } = bench
        .host
        .admit_policy_dispatch(&intent, &reasons, &context)
        .unwrap_or_else(|error| panic!("admit {task_id}: {error}"))
    else {
        panic!("{task_id} is granted");
    };
    (intent, *context)
}

// ---------------------------------------------------------------------------------------------
// A2, A3, A4, A5, A6, A8, A9 and item 1 of the RL-1a review
// ---------------------------------------------------------------------------------------------

/// A2 (§7.1): rule R1 of a placed pack is due. Package 2, its bundle entry and rule R2 are
/// written and the check interval passes: the next cycle gives a `catalog.activated` of a higher
/// version, no approval record, and a decision for R2, with no restart. A truncated
/// `schedule.json` then gives `standard_pack_root_invalid` exactly once and R1 still runs; the
/// complete file is then composed.
#[test]
fn rl1b_a2_a_placed_pack_is_scheduled_without_a_restart() {
    let bench = Bench::new(&[BOUND_A], &[POOL_A], true, None);
    bench.place(
        GAME_A,
        SERVER_A,
        &[PACKAGE_1],
        Some(&schedule_a(&[(RULE_1, PACKAGE_1, 100)])),
    );
    let first = bench.cycle(POLICY_NOW_UNIX_MS, 0);
    assert!(
        intent(&first, RULE_1).is_some(),
        "R1 is due: {:#?} {first:#?}",
        bench.standard_pack_reads()
    );
    let one = bench.active();
    assert_eq!(one.catalog_id(), STANDARD_PACKS_CATALOG_ID);
    assert_eq!(bench.events(EventType::CatalogActivated).len(), 1);
    let epoch = bench.host.runtime_info().owner_epoch();

    let both = schedule_a(&[(RULE_1, PACKAGE_1, 100), (RULE_2, PACKAGE_2, 90)]);
    bench.place(GAME_A, SERVER_A, &[PACKAGE_1, PACKAGE_2], Some(&both));
    // Within the interval nothing is read.
    let early = bench.cycle(POLICY_NOW_UNIX_MS, INTERVAL_MS - 1);
    assert!(!decided(&early, RULE_2, INSTANCE_A));
    assert_eq!(bench.active(), one);
    let second = bench.cycle(POLICY_NOW_UNIX_MS, INTERVAL_MS);
    let two = bench.active();
    assert!(two.catalog_version() > one.catalog_version());
    assert_eq!(bench.events(EventType::CatalogActivated).len(), 2);
    assert!(bench.events(EventType::ApprovalDecision).is_empty());
    assert!(
        decided(&second, RULE_2, INSTANCE_A),
        "R2 is decided: {second:#?}"
    );
    assert_eq!(bench.host.runtime_info().owner_epoch(), epoch, "no restart");

    let bytes = serde_json::to_vec_pretty(&both).expect("schedule bytes");
    fs::write(
        bench.path(GAME_A, "schedule.json"),
        &bytes[..bytes.len() / 2],
    )
    .expect("truncate schedule.json");
    for step in 2..4 {
        let cycle = bench.cycle(POLICY_NOW_UNIX_MS, step * INTERVAL_MS);
        assert!(
            intent(&cycle, RULE_1).is_some(),
            "R1 still runs: {cycle:#?}"
        );
    }
    assert_eq!(
        bench
            .reads_of(SharedCode::StandardPackRootInvalid, Some(GAME_A))
            .len(),
        1,
        "the truncated file is recorded exactly once"
    );
    assert_eq!(bench.active(), two, "the last good rules stay active");

    // A changed priority of another length: the file's size changes with its content, whatever
    // the file system's time resolution.
    let fixed = schedule_a(&[(RULE_1, PACKAGE_1, 99), (RULE_2, PACKAGE_2, 90)]);
    bench.place(GAME_A, SERVER_A, &[PACKAGE_1, PACKAGE_2], Some(&fixed));
    let after = bench.cycle(POLICY_NOW_UNIX_MS, 4 * INTERVAL_MS);
    assert!(bench.active().catalog_version() > two.catalog_version());
    assert!(decided(&after, RULE_1, INSTANCE_A) && decided(&after, RULE_2, INSTANCE_A));
    assert_eq!(
        bench
            .reads_of(SharedCode::StandardPackRootInvalid, Some(GAME_A))
            .len(),
        1
    );
    assert_eq!(bench.host.runtime_info().owner_epoch(), epoch, "no restart");
    bench.close();
}

/// A3 (§7.1): two bound games; game B's `schedule.json` is removed. The next composition leaves
/// game B out, and with `cached_generations = 0` a `Weak` handle to the old generation's compiled
/// catalog no longer upgrades; the memory holds at most 1 + 0 generations and no source byte.
#[test]
fn rl1b_a3_a_game_left_out_frees_its_generation() {
    let bench = Bench::new(&[BOUND_A, BOUND_B], &[POOL_A], true, Some(no_cache()));
    bench.place(
        GAME_A,
        SERVER_A,
        &[PACKAGE_1],
        Some(&schedule_a(&[(RULE_1, PACKAGE_1, 100)])),
    );
    bench.place(
        GAME_B,
        SERVER_B,
        &[PACKAGE_B],
        Some(&schedule_b(100, false)),
    );
    let first = bench.cycle(POLICY_NOW_UNIX_MS, 0);
    assert!(
        decided(&first, RULE_1, INSTANCE_A) && decided(&first, RULE_B, INSTANCE_B),
        "both games are composed: {:#?} {first:#?}",
        bench.standard_pack_reads()
    );
    let (_, old) = bench
        .host
        .policy_catalog_memory_for_test()
        .expect("catalog memory");
    let old = old.expect("an active compiled catalog");
    assert!(old.upgrade().is_some());

    bench.place(GAME_B, SERVER_B, &[PACKAGE_B], None);
    let second = bench.cycle(POLICY_NOW_UNIX_MS, INTERVAL_MS);
    assert!(decided(&second, RULE_1, INSTANCE_A));
    assert!(
        !decided(&second, RULE_B, INSTANCE_B),
        "game B is left out: {second:#?}"
    );
    assert!(
        old.upgrade().is_none(),
        "no reference holds the unloaded generation"
    );
    let (residency, _) = bench
        .host
        .policy_catalog_memory_for_test()
        .expect("catalog memory");
    assert!(residency.memo_generations <= 1, "{residency:?}");
    assert_eq!(residency.source_bytes, 0, "{residency:?}");
    bench.close();
}

/// A4 (§7.1): a game root installed without a bound instance: its `schedule.json` is never
/// opened, and no `standard_pack_registered` record carries schedule values for it.
#[test]
fn rl1b_a4_a_root_without_a_bound_instance_is_never_read() {
    let bench = Bench::new(&[BOUND_A], &[POOL_A], true, None);
    bench.place(
        GAME_A,
        SERVER_A,
        &[PACKAGE_1],
        Some(&schedule_a(&[(RULE_1, PACKAGE_1, 100)])),
    );
    bench.place(
        GAME_B,
        SERVER_B,
        &[PACKAGE_B],
        Some(&schedule_b(100, false)),
    );
    for step in 0..3 {
        bench.cycle(POLICY_NOW_UNIX_MS, step * INTERVAL_MS);
    }
    assert_eq!(
        bench
            .host
            .standard_pack_schedule_reads_for_test(GAME_B)
            .expect("reads"),
        0
    );
    assert!(
        bench
            .host
            .standard_pack_schedule_reads_for_test(GAME_A)
            .expect("reads")
            >= 1
    );
    let registered = bench.reads_of(HostCode::StandardPackRegistered, None);
    assert!(
        registered
            .iter()
            .filter(|outcome| outcome.value("game").and_then(Value::as_str) == Some(GAME_B))
            .all(|outcome| outcome.value("schedule_sha256").is_none()),
        "{registered:#?}"
    );
    assert!(
        registered
            .iter()
            .any(
                |outcome| outcome.value("game").and_then(Value::as_str) == Some(GAME_A)
                    && outcome.value("schedule_sha256").is_some()
            ),
        "{registered:#?}"
    );
    bench.close();
}

/// A5 (§7.1, §3.5): a composed generation is activated and runs a dispatch with no approval
/// decision in the ledger.
#[test]
fn rl1b_a5_a_composed_generation_runs_dispatches_without_any_approval() {
    let bench = Bench::new(&[BOUND_A], &[POOL_A], true, None);
    bench.place(
        GAME_A,
        SERVER_A,
        &[PACKAGE_1],
        Some(&schedule_a(&[(RULE_1, PACKAGE_1, 100)])),
    );
    let cycle = bench.cycle(POLICY_NOW_UNIX_MS, 0);
    let (intent, context) = admit(&bench, &cycle, RULE_1);
    assert!(intent.approval_refs.is_empty());
    assert_eq!(intent.catalog_hash, bench.active().catalog_hash());
    bench
        .host
        .complete_policy_dispatch(&intent.decision_id)
        .expect("complete the dispatch");
    bench
        .host
        .release_policy_dispatch_lease_for_test(&context)
        .expect("end the dispatch's lease");
    assert!(bench.events(EventType::ApprovalDecision).is_empty());
    assert_eq!(bench.events(EventType::CatalogActivated).len(), 1);
    bench.close();
}

/// A6 (§7.1, §3.6): a broken `schedule.json` of game A keeps game A's previous rules, its error
/// is recorded once per sha256 (also when game B's change makes the workbench compose again),
/// and game B is unaffected.
#[test]
fn rl1b_a6_a_broken_file_keeps_that_games_last_good_rules_and_spares_the_others() {
    let bench = Bench::new(&[BOUND_A, BOUND_B], &[POOL_A], true, None);
    bench.place(
        GAME_A,
        SERVER_A,
        &[PACKAGE_1],
        Some(&schedule_a(&[(RULE_1, PACKAGE_1, 100)])),
    );
    bench.place(
        GAME_B,
        SERVER_B,
        &[PACKAGE_B],
        Some(&schedule_b(100, false)),
    );
    bench.cycle(POLICY_NOW_UNIX_MS, 0);

    fs::write(
        bench.path(GAME_A, "schedule.json"),
        b"{\"schema_version\": ",
    )
    .expect("break schedule.json");
    let broken = bench.cycle(POLICY_NOW_UNIX_MS, INTERVAL_MS);
    assert!(
        decided(&broken, RULE_1, INSTANCE_A),
        "game A keeps R1: {broken:#?}"
    );
    assert!(decided(&broken, RULE_B, INSTANCE_B));
    // Game B changes: the workbench composes again, game A still from its last good file.
    bench.place(GAME_B, SERVER_B, &[PACKAGE_B], Some(&schedule_b(99, false)));
    let changed = bench.cycle(POLICY_NOW_UNIX_MS, 2 * INTERVAL_MS);
    assert!(
        decided(&changed, RULE_1, INSTANCE_A),
        "game A keeps R1: {changed:#?}"
    );
    assert!(decided(&changed, RULE_B, INSTANCE_B));
    bench.cycle(POLICY_NOW_UNIX_MS, 3 * INTERVAL_MS);
    let refusals = bench.reads_of(SharedCode::StandardPackRootInvalid, None);
    assert_eq!(refusals.len(), 1, "{refusals:#?}");
    assert_eq!(
        refusals[0].value("game").and_then(Value::as_str),
        Some(GAME_A)
    );
    assert!(
        bench
            .reads_of(SharedCode::StandardPackRootInvalid, Some(GAME_B))
            .is_empty()
    );
    bench.close();
}

/// A8 (§7.1): with a configured catalog the dual source is inert: a host whose standard-pack
/// root holds `schedule.json` evaluates as one whose root holds none, never composes and never
/// records schedule values.
#[test]
fn rl1b_a8_a_configured_catalog_evaluates_as_before() {
    let decisions = |with_schedule: bool| {
        // The configured fixture's own instance inputs, from the start.
        let bench = Bench::with_facts(&[BOUND_A], &[POOL_A], false, None, policy_facts());
        let schedule = schedule_a(&[(RULE_2, PACKAGE_1, 100)]);
        bench.place(
            GAME_A,
            SERVER_A,
            &[PACKAGE_1],
            with_schedule.then_some(&schedule),
        );
        bench
            .host
            .activate_policy_catalog(&policy_sources(1))
            .expect("activate the configured catalog");
        let configured = bench.active();
        let mut seen = Vec::new();
        for step in 0..3 {
            let cycle = bench.cycle(POLICY_NOW_UNIX_MS + step * HOUR_MS, step * INTERVAL_MS);
            let evaluation = cycle.evaluation.as_ref().expect("an evaluation");
            seen.push(format!(
                "{:?}|{:?}",
                evaluation
                    .decisions
                    .iter()
                    .map(|decision| (
                        decision.task_id.clone(),
                        decision.instance_id.clone(),
                        format!("{:?}", decision.state),
                        format!("{:?}", decision.eligibility),
                    ))
                    .collect::<Vec<_>>(),
                evaluation
                    .dispatch_intents
                    .iter()
                    .map(|intent| (
                        intent.task_id.clone(),
                        intent.instance_id.clone(),
                        intent.catalog_hash.clone(),
                        intent.procedure_ref.clone(),
                        intent.approval_refs.clone(),
                    ))
                    .collect::<Vec<_>>()
            ));
        }
        assert_eq!(bench.active(), configured, "nothing is composed");
        assert!(
            bench
                .reads_of(HostCode::StandardPackRegistered, None)
                .iter()
                .all(|outcome| outcome.value("schedule_sha256").is_none())
        );
        bench.close();
        seen
    };
    assert_eq!(decisions(true), decisions(false));
}

/// A9 (§7.1): a dispatch admitted under generation N settles after N+1 is active, with
/// `cached_generations = 0` (N is read back from disk). The check interval is the least one, so
/// N+1 is composed while the dispatch still holds its lease (no agent renews it here).
#[test]
fn rl1b_a9_a_dispatch_admitted_under_one_generation_settles_after_the_next() {
    let bench = Bench::new(&[BOUND_A], &[POOL_A], true, Some(no_cache_short_interval()));
    bench.place(
        GAME_A,
        SERVER_A,
        &[PACKAGE_1],
        Some(&schedule_a(&[(RULE_1, PACKAGE_1, 100)])),
    );
    let cycle = bench.cycle(POLICY_NOW_UNIX_MS, 0);
    let generation_n = bench.active();
    let (intent, context) = admit(&bench, &cycle, RULE_1);
    assert_eq!(intent.catalog_hash, generation_n.catalog_hash());

    bench.place(
        GAME_A,
        SERVER_A,
        &[PACKAGE_1],
        Some(&schedule_a(&[(RULE_1, PACKAGE_1, 99)])),
    );
    bench.cycle(POLICY_NOW_UNIX_MS, SHORT_INTERVAL_MS);
    assert!(bench.active().catalog_version() > generation_n.catalog_version());
    let (residency, _) = bench
        .host
        .policy_catalog_memory_for_test()
        .expect("catalog memory");
    assert!(residency.memo_generations <= 1, "{residency:?}");

    bench
        .host
        .complete_policy_dispatch(&intent.decision_id)
        .expect("the dispatch of generation N settles after N+1 is active");
    bench
        .host
        .release_policy_dispatch_lease_for_test(&context)
        .expect("end the dispatch's lease");
    bench.close();
}

/// A9, second half (§6.1 row RL-1b): the first composition switches from a configured catalog
/// with the same task ids.
#[test]
fn rl1b_a9_the_first_composition_switches_from_a_configured_catalog_with_the_same_task_ids() {
    let bench = Bench::new(&[BOUND_A], &[POOL_A], false, None);
    let schedule = schedule_a(&[(RULE_1, PACKAGE_1, 100), (RULE_2, PACKAGE_2, 90)]);
    bench.place(GAME_A, SERVER_A, &[PACKAGE_1, PACKAGE_2], Some(&schedule));
    // The configured catalog: the same documents as configured today, under its own id.
    let instances = BTreeMap::from([(
        INSTANCE_A.to_owned(),
        StandardPackBinding {
            game_id: GAME_A.to_owned(),
            server_id: SERVER_A.to_owned(),
        },
    )]);
    let bundle_bytes = fs::read(bench.path(GAME_A, "bundle.json")).expect("bundle bytes");
    let root = actingcommand_contract::read_standard_pack_root(
        actingcommand_contract::StandardPackRootBytes {
            bundle: Some(&bundle_bytes),
            pages: None,
            interrupts: None,
            navigation: None,
            tables: false,
            display_layout: None,
        },
        GAME_A,
    )
    .expect("a sound root")
    .expect("a game root");
    let schedule_bytes = serde_json::to_vec_pretty(&schedule).expect("schedule bytes");
    // Workflow #455 G-R2s: every package of the root reads, none irreversible.
    let irreversible = readable_root(&root);
    let games = BTreeMap::from([(
        GAME_A.to_owned(),
        StandardPackGameFiles {
            schedule: &schedule_bytes,
            root: &root,
            maintenance: &[],
            irreversible: &irreversible,
        },
    )]);
    let composed = compose_standard_pack_rules(&StandardPackComposeInput {
        instances: &instances,
        games: &games,
        subsystems: None,
        catalog_version: 1,
    })
    .expect("compose the configured catalog");
    let mut configured = composed.sources().clone();
    for source in [
        &mut configured.tasks,
        &mut configured.pools,
        &mut configured.activity,
        &mut configured.timeline,
    ] {
        let mut document: Value = serde_json::from_slice(&source.bytes).expect("a document");
        document["catalog"]["catalog_id"] = json!("fixture.catalog-a");
        source.bytes = serde_json::to_vec_pretty(&document).expect("document bytes");
    }
    bench
        .host
        .activate_policy_catalog(&configured)
        .expect("activate the configured catalog");
    assert_eq!(bench.active().catalog_id(), "fixture.catalog-a");

    // The restarted host evaluates at the fixture's restart instant, after every record of the
    // first start (board #71, TCLOCK).
    let bench = bench.restart(true);
    let cycle = bench.cycle(POLICY_RESTART_UNIX_MS, 0);
    let active = bench.active();
    assert_eq!(active.catalog_id(), STANDARD_PACKS_CATALOG_ID);
    let activated = bench.events(EventType::CatalogActivated);
    assert_eq!(
        activated.len(),
        2,
        "the configured catalog, then the composition"
    );
    let mut decided_ids = cycle
        .evaluation
        .as_ref()
        .expect("an evaluation")
        .decisions
        .iter()
        .map(|decision| decision.task_id.clone())
        .collect::<BTreeSet<_>>();
    decided_ids.remove(actingcommand_policy::FACT_REFRESH_TASK_ID);
    let configured_tasks = composed
        .compiled()
        .catalog()
        .tasks
        .tasks
        .iter()
        .map(|task| task.id.clone())
        .collect::<BTreeSet<_>>();
    assert_eq!(decided_ids, configured_tasks);
    bench.close();
}

/// Item 1 of the RL-1a review (#386 6399836824; #419; §3.6 "no rules is a valid state"): game B,
/// whose `schedule.json` declared pool B, is removed while the configuration still gives pool B's
/// reading. The stale reading never stops the evaluation of game A's instance; it is recorded
/// once as `policy_pool_reading_unused`.
#[test]
fn rl1b_item1_a_stale_pool_reading_never_stops_the_other_games() {
    let bench = Bench::new(&[BOUND_A, BOUND_B], &[POOL_A, POOL_B], true, None);
    bench.place(
        GAME_A,
        SERVER_A,
        &[PACKAGE_1],
        Some(&schedule_a(&[(RULE_1, PACKAGE_1, 100)])),
    );
    bench.place(GAME_B, SERVER_B, &[PACKAGE_B], Some(&schedule_b(100, true)));
    let first = bench.cycle(POLICY_NOW_UNIX_MS, 0);
    assert!(
        decided(&first, RULE_1, INSTANCE_A) && decided(&first, RULE_B, INSTANCE_B),
        "both games are composed: {:#?} {first:#?}",
        bench.standard_pack_reads()
    );
    assert!(
        bench
            .reads_of(HostCode::PolicyPoolReadingUnused, None)
            .is_empty()
    );

    bench.place(GAME_B, SERVER_B, &[PACKAGE_B], None);
    for step in 1..3 {
        let cycle = bench.cycle(POLICY_NOW_UNIX_MS, step * INTERVAL_MS);
        assert!(
            decided(&cycle, RULE_1, INSTANCE_A),
            "game A is evaluated: {cycle:#?}"
        );
        assert!(
            intent(&cycle, RULE_1).is_some(),
            "R1 is still due: {cycle:#?}"
        );
    }
    let notes = bench.reads_of(HostCode::PolicyPoolReadingUnused, None);
    assert_eq!(notes.len(), 1, "{notes:#?}");
    assert_eq!(
        notes[0].value("pool_id").and_then(Value::as_str),
        Some(POOL_B)
    );
    bench.close();
}

// ---------------------------------------------------------------------------------------------
// #428 CW-1 (register §1.290; coordinator work order rd-1110/brief-cw1.md): the composed
// catalog's compile warnings are recorded with the activation and held for the overview
// ---------------------------------------------------------------------------------------------

/// The warning a pool that writes both water lines and declares no valuation compiles with
/// (SS-2h fix 1, register §1.289).
const LINES_UNVALUED: &str = "pool_lines_unvalued";

/// [`schedule_a`] whose pool A writes both water lines (constants below its capacity 120) and
/// declares no valuation, with `pool_lines` in `requires`: it composes, and its compiled catalog
/// carries `pool_lines_unvalued` at the pool's `stop_line`.
fn schedule_a_lines_unvalued(rules: &[(&str, &str, u64)]) -> Value {
    let mut schedule = schedule_a(rules);
    schedule["requires"] = json!(["pool_lines"]);
    let pool = schedule["pools"][0].as_object_mut().expect("pool A");
    pool.shift_remove("valuation");
    pool.insert("stop_line".to_owned(), json!(100));
    pool.insert("hard_line".to_owned(), json!(50));
    schedule
}

/// The workbench's `standard_pack_registered` records of `game` (those with `schedule_sha256`),
/// in ledger order.
fn registrations(bench: &Bench, game: &str) -> Vec<Outcome> {
    bench
        .reads_of(HostCode::StandardPackRegistered, Some(game))
        .into_iter()
        .filter(|outcome| outcome.value("schedule_sha256").is_some())
        .collect()
}

/// The `config.catalog_warnings` fact the host holds now.
fn catalog_warnings_fact(bench: &Bench) -> Option<RuntimeFactRecord> {
    bench
        .host
        .runtime_fact_snapshot()
        .expect("the runtime fact snapshot")
        .records
        .into_iter()
        .find(|record| {
            record.scope == RuntimeFactScope::Runtime
                && record.key == CONFIG_CATALOG_WARNINGS_FACT_KEY
        })
}

/// CW-1 test 1 (fails before: the warning was made and dropped): a composed root whose pool
/// writes both lines and declares no valuation is composed and activated as before; the
/// activation's `standard_pack_registered` record of the game carries `compile_warnings` with
/// `pool_lines_unvalued` at `/pools/0/stop_line` of that game's `schedule.json`, and
/// `config.catalog_warnings`, which the overview's catalog block reads, holds it with its game and
/// pointer. The file fixed with a valuation, the next activation records the game with no
/// warning and the fact empty.
#[test]
fn cw1_a_lines_without_a_valuation_are_recorded_with_the_activation_and_held_for_the_overview() {
    let bench = Bench::new(&[BOUND_A], &[POOL_A], true, None);
    bench.place(
        GAME_A,
        SERVER_A,
        &[PACKAGE_1],
        Some(&schedule_a_lines_unvalued(&[(RULE_1, PACKAGE_1, 100)])),
    );
    bench.cycle(POLICY_NOW_UNIX_MS, 0);
    assert_eq!(bench.active().catalog_id(), STANDARD_PACKS_CATALOG_ID);
    assert_eq!(bench.events(EventType::CatalogActivated).len(), 1);
    let registered = registrations(&bench, GAME_A);
    assert_eq!(registered.len(), 1, "{registered:#?}");
    assert_eq!(
        registered[0].value("compile_warnings"),
        Some(&json!([{"warning_code": LINES_UNVALUED, "json_pointer": "/pools/0/stop_line"}])),
        "{registered:#?}"
    );
    let expected = ConfigCatalogWarnings::of(&[CatalogCompileWarning {
        warning: LINES_UNVALUED.to_owned(),
        game: Some(GAME_A.to_owned()),
        pointer: "/pools/0/stop_line".to_owned(),
    }]);
    assert_eq!(expected.total(), 1);
    let fact = catalog_warnings_fact(&bench).expect("config.catalog_warnings is recorded");
    assert_eq!(
        fact.value,
        expected.to_fact_record(0, OriginModule::Policy).value
    );
    assert_eq!(
        ConfigCatalogWarnings::from_fact_record(&fact).expect("the fact reads back"),
        expected
    );

    bench.place(
        GAME_A,
        SERVER_A,
        &[PACKAGE_1],
        Some(&schedule_a(&[(RULE_1, PACKAGE_1, 100)])),
    );
    bench.cycle(POLICY_NOW_UNIX_MS, INTERVAL_MS);
    assert_eq!(bench.events(EventType::CatalogActivated).len(), 2);
    let registered = registrations(&bench, GAME_A);
    assert_eq!(registered.len(), 2, "{registered:#?}");
    assert_eq!(registered[1].value("compile_warnings"), None);
    let fact = catalog_warnings_fact(&bench).expect("config.catalog_warnings stays recorded");
    assert!(
        ConfigCatalogWarnings::from_fact_record(&fact)
            .expect("the fact reads back")
            .is_empty(),
        "{fact:#?}"
    );
    bench.close();
}

/// CW-1 test 2: a composition without compile warnings records as before: each activation's
/// `standard_pack_registered` record of the game is RL-1b's, member for member, and no
/// `config.catalog_warnings` fact is recorded, at the first activation, at the next one and at
/// the first check after a restart.
#[test]
fn cw1_b_a_composition_without_warnings_records_as_before() {
    let bench = Bench::new(&[BOUND_A], &[POOL_A], true, None);
    let expected = |sha: &Value| {
        Outcome::new(HostCode::StandardPackRegistered)
            .operation(HostLocation::StandardPackRegister)
            .game(GAME_A)
            .count(1_usize)
            .schedule_sha256(sha.as_str().expect("schedule_sha256"))
            .rules(1_usize)
            .held(0_usize)
            .unclassified(0_usize)
    };
    for (step, priority) in [(0, 100), (1, 99)] {
        bench.place(
            GAME_A,
            SERVER_A,
            &[PACKAGE_1],
            Some(&schedule_a(&[(RULE_1, PACKAGE_1, priority)])),
        );
        bench.cycle(POLICY_NOW_UNIX_MS, step * INTERVAL_MS);
        let registered = registrations(&bench, GAME_A);
        assert_eq!(registered.len(), step as usize + 1, "{registered:#?}");
        let last = registered.last().expect("a registration");
        let sha = last.value("schedule_sha256").expect("schedule_sha256");
        assert_eq!(*last, expected(sha));
        assert!(catalog_warnings_fact(&bench).is_none());
    }
    assert_eq!(bench.events(EventType::CatalogActivated).len(), 2);

    // The restarted host evaluates at the fixture's restart instant, after every record of the
    // first start (board #71, TCLOCK). Its monotonic origin is the shared clock's reading when it
    // starts, where the last cycle above left it (`INTERVAL_MS`), so its first check reads the
    // clock there: a monotonic clock never goes back.
    let bench = bench.restart(true);
    bench.cycle(POLICY_RESTART_UNIX_MS, INTERVAL_MS);
    assert_eq!(bench.events(EventType::CatalogActivated).len(), 2);
    assert_eq!(registrations(&bench, GAME_A).len(), 2);
    assert!(catalog_warnings_fact(&bench).is_none());
    bench.close();
}

// ---------------------------------------------------------------------------------------------
// #428 CW-1 fix 1 (coordinator work order rd-1110/brief-cw1-fix1.md items 2 and 3; the
// coordinator's ruling: no new registration record): the cases the review found untested
// ---------------------------------------------------------------------------------------------

/// A second instance of game A on server A: every item of game A's file is composed for both.
const BOUND_A_TWICE: Bound = (INSTANCE_B, GAME_A, SERVER_A);

/// The one record `pool_lines_unvalued` of [`schedule_a_lines_unvalued`] gives in the
/// `compile_warnings` of game A's `standard_pack_registered`.
fn lines_unvalued_record() -> Value {
    json!([{"warning_code": LINES_UNVALUED, "json_pointer": "/pools/0/stop_line"}])
}

/// What `config.catalog_warnings` holds for one `pool_lines_unvalued` at pool A's `stop_line` in
/// game A's `schedule.json`.
fn lines_unvalued_in_a() -> ConfigCatalogWarnings {
    ConfigCatalogWarnings::of(&[CatalogCompileWarning {
        warning: LINES_UNVALUED.to_owned(),
        game: Some(GAME_A.to_owned()),
        pointer: "/pools/0/stop_line".to_owned(),
    }])
}

/// The warnings `config.catalog_warnings` holds now, read back.
fn held_warnings(bench: &Bench) -> ConfigCatalogWarnings {
    let fact = catalog_warnings_fact(bench).expect("config.catalog_warnings is held");
    ConfigCatalogWarnings::from_fact_record(&fact).expect("the fact reads back")
}

/// Game A's files as the bench placed them, composed for the instances `bound`.
fn compose_a(bench: &Bench, bound: &[Bound]) -> actingcommand_policy::StandardPackComposition {
    let instances = bound
        .iter()
        .map(|(alias, game, server)| {
            (
                (*alias).to_owned(),
                StandardPackBinding {
                    game_id: (*game).to_owned(),
                    server_id: (*server).to_owned(),
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    let bundle_bytes = fs::read(bench.path(GAME_A, "bundle.json")).expect("bundle bytes");
    let root = actingcommand_contract::read_standard_pack_root(
        actingcommand_contract::StandardPackRootBytes {
            bundle: Some(&bundle_bytes),
            pages: None,
            interrupts: None,
            navigation: None,
            tables: false,
            display_layout: None,
        },
        GAME_A,
    )
    .expect("a sound root")
    .expect("a game root");
    let schedule_bytes = fs::read(bench.path(GAME_A, "schedule.json")).expect("schedule bytes");
    // Workflow #455 G-R2s: the bench places no package files, so the workbench reads none of
    // them and marks every task; the same input here.
    let irreversible = unread_root(&root);
    let games = BTreeMap::from([(
        GAME_A.to_owned(),
        StandardPackGameFiles {
            schedule: &schedule_bytes,
            root: &root,
            maintenance: &[],
            irreversible: &irreversible,
        },
    )]);
    compose_standard_pack_rules(&StandardPackComposeInput {
        instances: &instances,
        games: &games,
        subsystems: None,
        catalog_version: 1,
    })
    .expect("compose game A")
}

/// CW-1 fix 1, case 1: two games, game A's pool with lines and no valuation. The file of game B
/// changes and game A's does not: the next activation records game B again, with no warning, and
/// writes no new `standard_pack_registered` record of game A; `config.catalog_warnings` still
/// holds game A's warning, as the active generation has it.
#[test]
fn cw1_c_a_game_whose_file_did_not_change_is_not_registered_again_and_its_warnings_stay_held() {
    let bench = Bench::new(&[BOUND_A, BOUND_B], &[POOL_A], true, None);
    bench.place(
        GAME_A,
        SERVER_A,
        &[PACKAGE_1],
        Some(&schedule_a_lines_unvalued(&[(RULE_1, PACKAGE_1, 100)])),
    );
    bench.place(
        GAME_B,
        SERVER_B,
        &[PACKAGE_B],
        Some(&schedule_b(100, false)),
    );
    bench.cycle(POLICY_NOW_UNIX_MS, 0);
    assert_eq!(bench.events(EventType::CatalogActivated).len(), 1);
    let registered_a = registrations(&bench, GAME_A);
    assert_eq!(registered_a.len(), 1, "{registered_a:#?}");
    assert_eq!(
        registered_a[0].value("compile_warnings"),
        Some(&lines_unvalued_record()),
        "{registered_a:#?}"
    );
    assert_eq!(registrations(&bench, GAME_B).len(), 1);
    assert_eq!(held_warnings(&bench), lines_unvalued_in_a());

    // Game B's file changes (a priority of another length); game A's does not.
    bench.place(GAME_B, SERVER_B, &[PACKAGE_B], Some(&schedule_b(99, false)));
    bench.cycle(POLICY_NOW_UNIX_MS, INTERVAL_MS);
    assert_eq!(bench.events(EventType::CatalogActivated).len(), 2);
    let registered_b = registrations(&bench, GAME_B);
    assert_eq!(registered_b.len(), 2, "{registered_b:#?}");
    assert_eq!(registered_b[1].value("compile_warnings"), None);
    assert_eq!(
        registrations(&bench, GAME_A),
        registered_a,
        "game A is not registered again"
    );
    assert_eq!(held_warnings(&bench), lines_unvalued_in_a());
    bench.close();
}

/// CW-1 fix 1, case 2: game A bound to two instances. Its pool is composed once for each, and
/// the compiled catalog warns at both copies; the activation's record of game A lists the warning
/// once, at its one place in game A's `schedule.json`, and `config.catalog_warnings` counts it once.
#[test]
fn cw1_d_the_same_warning_from_two_instances_is_listed_once() {
    let bound = [BOUND_A, BOUND_A_TWICE];
    let bench = Bench::new(&bound, &[POOL_A], true, None);
    bench.place(
        GAME_A,
        SERVER_A,
        &[PACKAGE_1],
        Some(&schedule_a_lines_unvalued(&[(RULE_1, PACKAGE_1, 100)])),
    );
    let composed = compose_a(&bench, &bound);
    assert_eq!(
        composed.compiled().warnings().len(),
        2,
        "one warning at each copy of the pool: {:#?}",
        composed.compiled().warnings()
    );
    assert_eq!(composed.compile_warnings().len(), 1);

    bench.cycle(POLICY_NOW_UNIX_MS, 0);
    assert_eq!(bench.events(EventType::CatalogActivated).len(), 1);
    let registered = registrations(&bench, GAME_A);
    assert_eq!(registered.len(), 1, "{registered:#?}");
    assert_eq!(
        registered[0].value("compile_warnings"),
        Some(&lines_unvalued_record()),
        "{registered:#?}"
    );
    let held = held_warnings(&bench);
    assert_eq!(held.total(), 1);
    assert_eq!(held, lines_unvalued_in_a());
    bench.close();
}

/// CW-1 fix 1, case 3: after a restart, at the first check of the restarted host, the active
/// generation (restored, not activated again, and its game not registered again) has its
/// compile warnings in `config.catalog_warnings` again.
#[test]
fn cw1_e_after_a_restart_the_fact_holds_the_active_generation_s_warnings_again() {
    let bench = Bench::new(&[BOUND_A], &[POOL_A], true, None);
    bench.place(
        GAME_A,
        SERVER_A,
        &[PACKAGE_1],
        Some(&schedule_a_lines_unvalued(&[(RULE_1, PACKAGE_1, 100)])),
    );
    bench.cycle(POLICY_NOW_UNIX_MS, 0);
    assert_eq!(bench.events(EventType::CatalogActivated).len(), 1);
    assert_eq!(held_warnings(&bench), lines_unvalued_in_a());

    // The restarted host evaluates at the fixture's restart instant, after every record of the
    // first start (board #71, TCLOCK).
    let bench = bench.restart(true);
    bench.cycle(POLICY_RESTART_UNIX_MS, 0);
    assert_eq!(bench.events(EventType::CatalogActivated).len(), 1);
    assert_eq!(registrations(&bench, GAME_A).len(), 1);
    assert_eq!(held_warnings(&bench), lines_unvalued_in_a());
    bench.close();
}

// ---------------------------------------------------------------------------------------------
// #428 SCHED-C1b (register §1.299; coordinator work order rd-1111/brief-sched-c1b.md;
// contracts/standard-pack-schedule.md, "Provenance"): the workbench records who wrote the active
// generation's fixed values and valuations where and when it records its compile warnings
// ---------------------------------------------------------------------------------------------

/// A third rule of game A, on its own package.
const RULE_3: &str = "fixture.third";
const PACKAGE_3: &str = "fixture.third";

/// [`schedule_a`] with three rules: rule 1 produces into pool A, whose valuation block the pack
/// declared (no `valuation_origin`); rule 2 runs on a fixed value the pack wrote and rule 3 on
/// one the model's export wrote, marked `"fixed_value_origin": "model"` when `marked`, neither
/// producing anything; `requires` names `benefit_fixed_value`.
fn schedule_a_fixed_values(marked: bool) -> Value {
    let mut schedule = schedule_a(&[
        (RULE_1, PACKAGE_1, 100),
        (RULE_2, PACKAGE_2, 100),
        (RULE_3, PACKAGE_3, 100),
    ]);
    schedule["requires"] = json!([actingcommand_policy::BENEFIT_FIXED_VALUE_SUBSYSTEM]);
    schedule["rules"][1]["fixed_value_milli"] = json!(12_000);
    schedule["rules"][2]["fixed_value_milli"] = json!(9_000);
    if marked {
        schedule["rules"][2]["fixed_value_origin"] = json!("model");
    }
    schedule
}

/// [`schedule_a`] with rule 1 alone and pool A without its valuation block: no fixed value and no
/// declared valuation.
fn schedule_a_without_values() -> Value {
    let mut schedule = schedule_a(&[(RULE_1, PACKAGE_1, 100)]);
    let pool = schedule["pools"][0].as_object_mut().expect("pool A");
    pool.shift_remove("valuation");
    schedule
}

/// The `config.catalog_provenance` fact the host holds now.
fn catalog_provenance_fact(bench: &Bench) -> Option<RuntimeFactRecord> {
    bench
        .host
        .runtime_fact_snapshot()
        .expect("the runtime fact snapshot")
        .records
        .into_iter()
        .find(|record| {
            record.scope == RuntimeFactScope::Runtime
                && record.key == CONFIG_CATALOG_PROVENANCE_FACT_KEY
        })
}

/// The values `config.catalog_provenance` holds now, read back.
fn held_provenance(bench: &Bench) -> ConfigCatalogProvenance {
    let fact = catalog_provenance_fact(bench).expect("config.catalog_provenance is held");
    ConfigCatalogProvenance::from_fact_record(&fact).expect("the fact reads back")
}

/// One value of game A: its kind, who wrote it and its rule's or pool's id.
fn value_of_a(
    kind: CatalogValueKind,
    origin: CatalogValueOrigin,
    id: &str,
) -> CatalogProvenanceEntry {
    CatalogProvenanceEntry {
        game: GAME_A.to_owned(),
        kind,
        origin,
        id: id.to_owned(),
    }
}

/// The count row of game A's values of `kind` from `origin`.
fn count_of_a(
    kind: CatalogValueKind,
    origin: CatalogValueOrigin,
    count: u64,
) -> CatalogProvenanceCount {
    CatalogProvenanceCount {
        game: GAME_A.to_owned(),
        kind,
        origin,
        count,
    }
}

/// SCHED-C1b test 1: a generation whose catalog holds a fixed value the pack wrote, one the
/// model's export marked and a valuation block the pack declared. `config.catalog_provenance`
/// holds each under its origin, one count row per kind and origin, then the ids in file order;
/// the activation's `standard_pack_registered` record is RL-1b's, member for member (the fact
/// has no record of its own). The model's mark taken out, only an origin changes, which changes
/// no digest: no new activation, no new registration, and the fact follows, both fixed values
/// now the pack's. A file without fixed values and valuation blocks: the next activation records
/// the fact empty, as CW-1 records its fact once the active generation has no warning.
#[test]
fn sched_c1b_a_the_fact_holds_each_value_under_its_origin_and_follows_an_origin_change() {
    use CatalogValueKind::{FixedValue, Valuation};
    use CatalogValueOrigin::{Model, Pack};

    let bench = Bench::new(&[BOUND_A], &[POOL_A], true, None);
    let packages = [PACKAGE_1, PACKAGE_2, PACKAGE_3];
    let marked = schedule_a_fixed_values(true);
    bench.place(GAME_A, SERVER_A, &packages, Some(&marked));
    bench.cycle(POLICY_NOW_UNIX_MS, 0);
    assert_eq!(bench.active().catalog_id(), STANDARD_PACKS_CATALOG_ID);
    assert_eq!(bench.events(EventType::CatalogActivated).len(), 1);
    let held = held_provenance(&bench);
    assert_eq!(
        held.counts,
        [
            count_of_a(FixedValue, Pack, 1),
            count_of_a(FixedValue, Model, 1),
            count_of_a(Valuation, Pack, 1),
        ]
    );
    assert_eq!(
        held.ids,
        [
            value_of_a(FixedValue, Pack, RULE_2),
            value_of_a(FixedValue, Model, RULE_3),
            value_of_a(Valuation, Pack, POOL_A),
        ]
    );
    assert!(!held.is_cut(), "{held:?}");
    let registered = registrations(&bench, GAME_A);
    assert_eq!(registered.len(), 1, "{registered:#?}");
    let sha = registered[0]
        .value("schedule_sha256")
        .and_then(Value::as_str)
        .expect("schedule_sha256");
    assert_eq!(
        registered[0],
        Outcome::new(HostCode::StandardPackRegistered)
            .operation(HostLocation::StandardPackRegister)
            .game(GAME_A)
            .count(3_usize)
            .schedule_sha256(sha)
            .rules(3_usize)
            .held(0_usize)
            .unclassified(0_usize)
    );

    // Only an origin changes: the same catalog hash and rules digest.
    let unmarked = schedule_a_fixed_values(false);
    bench.place(GAME_A, SERVER_A, &packages, Some(&unmarked));
    bench.cycle(POLICY_NOW_UNIX_MS, INTERVAL_MS);
    assert_eq!(bench.events(EventType::CatalogActivated).len(), 1);
    assert_eq!(registrations(&bench, GAME_A).len(), 1);
    let held = held_provenance(&bench);
    assert_eq!(
        held.counts,
        [
            count_of_a(FixedValue, Pack, 2),
            count_of_a(Valuation, Pack, 1),
        ]
    );
    assert_eq!(
        held.ids,
        [
            value_of_a(FixedValue, Pack, RULE_2),
            value_of_a(FixedValue, Pack, RULE_3),
            value_of_a(Valuation, Pack, POOL_A),
        ]
    );

    // Neither fixed values nor a valuation block: the fact is recorded empty.
    let plain = schedule_a_without_values();
    bench.place(GAME_A, SERVER_A, &[PACKAGE_1], Some(&plain));
    bench.cycle(POLICY_NOW_UNIX_MS, 2 * INTERVAL_MS);
    assert_eq!(bench.events(EventType::CatalogActivated).len(), 2);
    let fact = catalog_provenance_fact(&bench).expect("config.catalog_provenance stays recorded");
    assert_eq!(fact.value, ContractFactValue::RecordList(Vec::new()));
    assert!(held_provenance(&bench).is_empty());
    bench.close();
}

/// SCHED-C1b test 2 (CW-1 test 2's rule for a composition without warnings): a composition
/// without fixed values and valuation blocks records no `config.catalog_provenance`, at its
/// activation and at the first check after a restart, so a Runtime whose catalogs never held
/// such a value records nothing new.
#[test]
fn sched_c1b_b_a_generation_without_values_records_no_fact() {
    let bench = Bench::new(&[BOUND_A], &[POOL_A], true, None);
    let plain = schedule_a_without_values();
    bench.place(GAME_A, SERVER_A, &[PACKAGE_1], Some(&plain));
    bench.cycle(POLICY_NOW_UNIX_MS, 0);
    assert_eq!(bench.events(EventType::CatalogActivated).len(), 1);
    assert!(catalog_provenance_fact(&bench).is_none());

    // The restarted host evaluates at the fixture's restart instant, after every record of the
    // first start (board #71, TCLOCK).
    let bench = bench.restart(true);
    bench.cycle(POLICY_RESTART_UNIX_MS, 0);
    assert_eq!(bench.events(EventType::CatalogActivated).len(), 1);
    assert!(catalog_provenance_fact(&bench).is_none());
    bench.close();
}
