// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #477 CV-2 (0.12.80; register §1.508; contracts/standard-pack-schedule.md, "`requires`",
// "The scheduler workbench"; coordinator work order `rd-1122/brief-477-cv2.md`, its rulings 3 to 6
// and its tests 4 and 5), the Runtime's half. The scheduler workbench reads a game root whose
// `schedule.json` names `catalog_business` with its `pack-catalog.json`, composes it by the one
// composer, keeps the catalog beside the game's fragment, and watches it:
// - a switched root is composed and activated, each rule with the business part its entry gives,
//   who wrote each value is held in `config.catalog_provenance` as for a rule that wrote it,
//   check-config's preview composes it to the digest the workbench records, and a placement that
//   changes only the catalog is composed again (h1);
// - a Runtime whose subsystems do not name `catalog_business`, the stand-in for an older one,
//   refuses the file at `requires` (`schedule_requires_missing`) (h2);
// - a catalog the composer refuses, and then none at all, are recorded naming `pack-catalog.json`,
//   and the game keeps its last good rules (h3).
// Game A, its packages and its pool are the fixtures of `gate_rl1b`; times are a manual Runtime
// clock, and nothing sleeps.

use super::gate_442_rem1::write_package;
use super::gate_rl1b::{
    GAME_A, POOL_A, SERVER_A, bundle, pool, profiles, resources, rule, write_json,
};
use super::*;
use crate::PolicyTrigger;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    CATALOG_GENERATIONS_DIRECTORY, CATALOG_STORE_PATH, CONFIG_CATALOG_PROVENANCE_FACT_KEY,
    CatalogProvenanceEntry, CatalogValueKind, CatalogValueOrigin, ConfigCatalogProvenance,
    RuntimeConfigManifest, RuntimeFactScope, RuntimeLifecyclePhase, RuntimePayload,
    STANDARD_PACK_COMPOSITION_RECORD_FILE,
};
use actingcommand_policy::{
    BENEFIT_FIXED_VALUE_SUBSYSTEM, CATALOG_BUSINESS_SUBSYSTEM, STANDARD_PACK_OPERATION_ID,
    STANDARD_PACKS_CATALOG_ID,
};
use serde_json::{Value, json};

/// The rule, and its package, whose part produces into [`POOL_A`].
const RULE: &str = "fixture.observe";
/// The rule, and its package, whose part is a fixed value the model's export wrote.
const VALUED: &str = "fixture.valued";
/// The default `standard_packs.check_interval_ms`.
const INTERVAL_MS: u64 = 60_000;

/// One host in composer mode over a standard-pack root on disk, its manual clock, and the policy
/// instance on game A.
struct Bench {
    host: RuntimeHost,
    clock: Arc<ManualRuntimeClock>,
    state: TempDir,
    packs: TempDir,
    facts: EvaluationFacts,
    resources: EvaluationResources,
}

impl Bench {
    /// The host, with `manifest` as its configuration manifest when given (none: `requires` is not
    /// checked).
    fn new(manifest: Option<RuntimeConfigManifest>) -> Self {
        let state = TempDir::new().expect("state root");
        let packs = TempDir::new().expect("standard-pack root");
        let mut facts = policy_facts();
        facts.outcomes.clear();
        facts.instances = vec![InstanceSnapshot {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
            server_id: SERVER_A.to_owned(),
            game_id: GAME_A.to_owned(),
            host_id: "fixture-host-a".to_owned(),
            available: true,
            capability_operation_ids: vec![STANDARD_PACK_OPERATION_ID.to_owned()],
            preferred_task_ids: Vec::new(),
        }];
        let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
        let mut host_config = config(&state)
            .with_policy_inputs(PolicyInputSnapshot::new(facts.clone(), policy_resources()))
            .with_runtime_clock(clock.clone())
            .with_standard_packs_root(Some(packs.path().to_owned()))
            .with_standard_pack_schedule(true);
        if let Some(manifest) = manifest {
            host_config = host_config.with_config_manifest(manifest);
        }
        let host = RuntimeHost::start(
            host_config,
            Arc::new(FakeProvider::one(
                POLICY_INSTANCE_ALIAS,
                instance_id(),
                Arc::new(FakeState::default()),
            )),
        )
        .expect("start the composer-mode host");
        Self {
            host,
            clock,
            state,
            packs,
            facts,
            resources: resources(&[POOL_A]),
        }
    }

    /// A file of game A's root.
    fn path(&self, file: &str) -> PathBuf {
        self.packs.path().join(GAME_A).join(file)
    }

    /// Places game A's `bundle.json` registering `packages` (each a readable package) and
    /// `schedule.json`.
    fn place(&self, packages: &[&str], schedule: &Value) {
        write_json(
            &self.path("bundle.json"),
            &bundle(GAME_A, SERVER_A, packages),
        );
        for package_id in packages {
            let directory = self.path(package_id);
            if !directory.join("control.json").exists() {
                write_package(&directory, package_id, None, None);
            }
        }
        write_json(&self.path("schedule.json"), schedule);
    }

    /// One cycle with the monotonic clock at `monotonic_ms`; a check runs once `INTERVAL_MS` has
    /// passed since the last one.
    fn cycle(&self, monotonic_ms: u64) -> PolicyCycle {
        self.clock.set_unix_ms(POLICY_NOW_UNIX_MS);
        self.clock.set_monotonic_ms(monotonic_ms);
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
                    unix_ms: POLICY_NOW_UNIX_MS,
                    monotonic_ms: POLICY_NOW_UNIX_MS,
                },
                7,
                PolicyTrigger::Recovery,
            )
            .unwrap_or_else(|error| panic!("cycle at {monotonic_ms}: {error}"))
    }

    fn active(&self) -> CatalogGeneration {
        self.host
            .active_policy_catalog()
            .expect("read the active catalog")
            .expect("an active catalog")
    }

    fn activations(&self) -> usize {
        self.host
            .query_persisted_events_for_test(EventQuery {
                event_type: Some(EventType::CatalogActivated),
                ..EventQuery::default()
            })
            .expect("ledger events")
            .len()
    }

    /// The `standard_pack_root_invalid` outcomes of the `standard_pack_read` lifecycle records, in
    /// ledger order.
    fn refusals(&self) -> Vec<Outcome> {
        self.host
            .query_persisted_events_for_test(EventQuery {
                event_type: Some(EventType::RuntimeLifecycleObserved),
                ..EventQuery::default()
            })
            .expect("ledger events")
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
            .filter(|outcome| outcome.is(SharedCode::StandardPackRootInvalid))
            .collect()
    }

    /// The file `file` of the generation `generation` below the state root, where the catalog
    /// store keeps it.
    fn generation_file(&self, generation: &CatalogGeneration, file: &str) -> PathBuf {
        let hex = generation
            .catalog_hash()
            .strip_prefix("sha256:")
            .expect("a catalog hash");
        CATALOG_STORE_PATH
            .iter()
            .fold(self.state.path().to_path_buf(), |path, part| {
                path.join(part)
            })
            .join(CATALOG_GENERATIONS_DIRECTORY)
            .join(hex)
            .join(file)
    }

    /// The task `id` of the active generation's `tasks.json`, as staged.
    fn staged_task(&self, id: &str) -> Value {
        let path = self.generation_file(&self.active(), "tasks.json");
        let tasks: Value =
            serde_json::from_slice(&fs::read(&path).expect("the staged tasks")).expect("JSON");
        tasks["tasks"]
            .as_array()
            .expect("the tasks")
            .iter()
            .find(|task| task["id"] == id)
            .cloned()
            .unwrap_or_else(|| panic!("the staged task {id}: {tasks}"))
    }

    fn close(self) {
        self.host.close().expect("close the host");
    }
}

/// Game A's `schedule.json` cut over: [`RULE`] (and, with `valued`, [`VALUED`]) written without
/// a business member, `requires` naming `catalog_business` (and `benefit_fixed_value` for the
/// fixed value [`VALUED`]'s part holds), with [`POOL_A`] and the fixture's profiles.
fn switched_schedule(valued: bool) -> Value {
    let mut rules = vec![rule(RULE, RULE, 100, Some(POOL_A))];
    if valued {
        rules.push(rule(VALUED, VALUED, 100, None));
    }
    for written in &mut rules {
        let members = written.as_object_mut().expect("a rule object");
        members.shift_remove("consumes");
        members.shift_remove("produces");
    }
    let requires = if valued {
        json!([BENEFIT_FIXED_VALUE_SUBSYSTEM, CATALOG_BUSINESS_SUBSYSTEM])
    } else {
        json!([CATALOG_BUSINESS_SUBSYSTEM])
    };
    json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": requires,
        "rules": rules,
        "pools": [pool(POOL_A)],
        "activity": profiles("", Some(POOL_A)),
        "timeline": [],
        "held": []
    })
}

/// Game A's version 2 catalog: [`RULE`]'s entry produces `amount` of [`POOL_A`]; with `valued`,
/// [`VALUED`]'s holds a fixed value of 12 000 milli the model's export wrote.
fn catalog_document(amount: u64, valued: bool) -> Value {
    let mut packs = vec![json!({
        "package_id": RULE,
        "consumes": [],
        "produces": [{
            "pool_id": POOL_A,
            "direction": "produce",
            "amount": amount,
            "observation_source": "scan_verified",
            "confidence_milli": 1000
        }]
    })];
    if valued {
        packs.push(json!({
            "package_id": VALUED,
            "consumes": [],
            "produces": [],
            "value": {"milli": 12_000, "origin": "model"}
        }));
    }
    json!({"schema_version": "actingcommand.pack-catalog.v2", "game": GAME_A, "packs": packs})
}

/// Whether `outcome` is a refusal of `problem` at `pointer` of game A's file `file`.
fn refused_at(outcome: &Outcome, file: &str, problem: &str, pointer: &str) -> bool {
    outcome.value("game") == Some(&json!(GAME_A))
        && outcome
            .value("path")
            .and_then(Value::as_str)
            .is_some_and(|path| path.ends_with(file))
        && outcome.value("standard_pack_problem") == Some(&json!(problem))
        && outcome.value("json_pointer") == Some(&json!(pointer))
}

/// Tests 4 and 5 (rulings 3, 5 and 6): game A's switched root with its version 2 catalog is
/// composed at the first check and activated, without a manifest's subsystems in the way: the
/// staged tasks hold [`RULE`]'s production and [`VALUED`]'s fixed value as the catalog gives them,
/// the generation keeps the catalog beside the game's fragment, and `config.catalog_provenance`
/// holds [`VALUED`]'s value under the model, the origin its part names. check-config's preview of
/// the same root composes to the digest the activation records. A placement that changes only
/// `pack-catalog.json` (another amount, another length) is noticed at the next check and composed:
/// a second activation, whose staged task holds the new amount.
#[test]
fn cv2_h1_a_switched_root_is_composed_and_activated_and_its_catalog_alone_recomposes() {
    let bench = Bench::new(None);
    bench.place(&[RULE, VALUED], &switched_schedule(true));
    write_json(&bench.path("pack-catalog.json"), &catalog_document(1, true));
    bench.cycle(0);
    assert_eq!(bench.activations(), 1, "{:#?}", bench.refusals());
    let first = bench.active();
    assert_eq!(first.catalog_id(), STANDARD_PACKS_CATALOG_ID);
    assert_eq!(bench.staged_task(RULE)["produces"][0]["amount"], 1);
    assert_eq!(
        bench.staged_task(VALUED)["benefit_fixed_value_milli"],
        12_000
    );
    assert_eq!(
        fs::read(bench.generation_file(&first, &format!("fragment-catalogs/{GAME_A}.json")))
            .expect("the catalog kept with the fragment"),
        fs::read(bench.path("pack-catalog.json")).expect("the catalog")
    );
    let provenance = bench
        .host
        .runtime_fact_snapshot()
        .expect("the runtime fact snapshot")
        .records
        .into_iter()
        .find(|record| {
            record.scope == RuntimeFactScope::Runtime
                && record.key == CONFIG_CATALOG_PROVENANCE_FACT_KEY
        })
        .map(|record| ConfigCatalogProvenance::from_fact_record(&record).expect("the fact reads"))
        .expect("config.catalog_provenance is held");
    assert!(
        provenance.ids.contains(&CatalogProvenanceEntry {
            game: GAME_A.to_owned(),
            kind: CatalogValueKind::FixedValue,
            origin: CatalogValueOrigin::Model,
            id: VALUED.to_owned(),
        }),
        "{provenance:?}"
    );
    let record: Value = serde_json::from_slice(
        &fs::read(bench.generation_file(&first, STANDARD_PACK_COMPOSITION_RECORD_FILE))
            .expect("the composition record"),
    )
    .expect("JSON");
    let preview = crate::host::preview_standard_pack_schedule(
        Some(bench.packs.path()),
        &[(
            POLICY_INSTANCE_ALIAS.to_owned(),
            GAME_A.to_owned(),
            SERVER_A.to_owned(),
        )],
        None,
        true,
    );
    assert!(preview.problems.is_empty(), "{:?}", preview.problems);
    assert_eq!(
        preview.rules_digest.as_deref(),
        record["rules_digest"].as_str()
    );

    write_json(
        &bench.path("pack-catalog.json"),
        &catalog_document(10, true),
    );
    bench.cycle(INTERVAL_MS);
    assert_eq!(bench.activations(), 2, "{:#?}", bench.refusals());
    assert_ne!(bench.active(), first);
    assert_eq!(bench.staged_task(RULE)["produces"][0]["amount"], 10);
    bench.close();
}

/// Test 4 (ruling 4): a Runtime whose configuration manifest names no subsystem, the stand-in for
/// one of 0.12.79, which does not name `catalog_business`, refuses the switched file at
/// `requires`, `schedule_requires_missing` at `/requires/0`, by the rule that exists, and composes
/// no rule of the game.
#[test]
fn cv2_h2_a_runtime_that_does_not_name_the_subsystem_answers_requires_missing() {
    let bench = Bench::new(Some(RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters: Vec::new(),
    }));
    bench.place(&[RULE], &switched_schedule(false));
    write_json(
        &bench.path("pack-catalog.json"),
        &catalog_document(1, false),
    );
    bench.cycle(0);
    let refusals = bench.refusals();
    assert!(
        refusals.iter().any(|outcome| refused_at(
            outcome,
            "schedule.json",
            "schedule_requires_missing",
            "/requires/0"
        )),
        "{refusals:#?}"
    );
    let path = bench.generation_file(&bench.active(), "tasks.json");
    let tasks: Value =
        serde_json::from_slice(&fs::read(&path).expect("the staged tasks")).expect("JSON");
    assert_eq!(tasks["tasks"], json!([]), "{tasks}");
    bench.close();
}

/// Test 4 (ruling 5): game A's switched root is composed and activated; then its catalog is
/// replaced by one of version 1, which the composer refuses, and then removed. Each is recorded
/// once, naming `pack-catalog.json` (`catalog_version_1` at its `schema_version`, then
/// `catalog_missing`), and the game keeps its last good rules: the active generation, composed
/// again from its fragment and the catalog kept with it, is the one of before, and nothing is
/// activated.
#[test]
fn cv2_h3_a_refused_or_missing_catalog_keeps_the_games_last_good_rules() {
    let bench = Bench::new(None);
    bench.place(&[RULE, VALUED], &switched_schedule(true));
    write_json(&bench.path("pack-catalog.json"), &catalog_document(1, true));
    bench.cycle(0);
    assert_eq!(bench.activations(), 1, "{:#?}", bench.refusals());
    let good = bench.active();

    write_json(
        &bench.path("pack-catalog.json"),
        &json!({"schema_version": "actingcommand.pack-catalog.v1", "game": GAME_A, "packs": []}),
    );
    bench.cycle(INTERVAL_MS);
    let refusals = bench.refusals();
    assert_eq!(refusals.len(), 1, "{refusals:#?}");
    assert!(
        refused_at(
            &refusals[0],
            "pack-catalog.json",
            "catalog_version_1",
            "/schema_version"
        ),
        "{refusals:#?}"
    );
    assert_eq!(bench.active(), good);
    assert_eq!(bench.activations(), 1);

    fs::remove_file(bench.path("pack-catalog.json")).expect("remove the catalog");
    bench.cycle(2 * INTERVAL_MS);
    let refusals = bench.refusals();
    assert_eq!(refusals.len(), 2, "{refusals:#?}");
    assert!(
        refused_at(&refusals[1], "pack-catalog.json", "catalog_missing", ""),
        "{refusals:#?}"
    );
    assert_eq!(bench.active(), good);
    assert_eq!(bench.activations(), 1);
    bench.close();
}
