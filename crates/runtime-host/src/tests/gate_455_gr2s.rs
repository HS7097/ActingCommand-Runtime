// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #455 G-R2x gate tests, host half (0.12.77; register §1.499; coordinator work order
// `rd-1121/brief-455-gr2x.md`, rulings 2 and 4; contracts/standard-pack-schedule.md, "The
// irreversible mark"). G-R2s (0.12.60, register §1.369) is withdrawn: the scheduler workbench
// still reads every package of a game root's `bundle.json` with the package reader the goal check
// uses, and reads the root again when a registered package's declaration files change, but the
// composition it activates marks no task, whatever a package declares; a package it cannot read
// is told by the one existing warning, `standard_pack_package_unreadable`, recorded once while it
// stays unreadable, with the reader's refusal as its cause. check-config's composition marks no
// task either.
//
// A neutral fixture: one game root on disk with two rules in catalog-a's task shape, each running
// the package of its own id, both real packages (`gate_442_rem1`'s); every declaration file's
// modification time is set by the test (no sleep, no clock read). The host runs in composer mode
// over a manual Runtime clock and one fake instance.

use super::gate_442_rem1::write_package;
use super::gate_rl1b::{GAME_A, POOL_A, SERVER_A, bundle, resources, schedule_a, write_json};
use super::*;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{RuntimeLifecyclePhase, RuntimePayload};
use serde_json::json;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The rule whose package keeps a capability nobody marks irreversible.
const KEEP: &str = "fixture.keep";
/// The rule whose package provides `cap_x`, marked irreversible or not by the test.
const SINK: &str = "fixture.sink";
const INSTANCE: &str = POLICY_INSTANCE_ALIAS;
/// The default `standard_packs.check_interval_ms`.
const INTERVAL_MS: u64 = 60_000;

/// The provides of SINK's control file: `cap_x`, marked irreversible when `marked`. The two
/// writings are the same length: `true` is written with a trailing space.
fn sink_control(marked: bool) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(&json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": SINK,
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 10,
        "step_timeout_ms": 600,
        "timeout_ms": 8000,
        "max_steps": 2,
        // An irreversible capability declares its parameters (the package reader refuses one
        // without); the unmarked writing declares the same.
        "provides": [{"capability": "cap_x", "irreversible": false, "parameters": [
            {"name": "param_p", "options": ["opt_1", "opt_2"], "default": "opt_1"}
        ]}]
    }))
    .expect("control bytes");
    if marked {
        let text = String::from_utf8(bytes).expect("UTF-8 control bytes");
        let replaced = text.replace("\"irreversible\": false", "\"irreversible\": true ");
        assert_ne!(replaced, text, "the mark is written");
        bytes = replaced.into_bytes();
    }
    bytes
}

/// A fixed modification time, `seconds` after a fixed instant: no clock is read.
fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_700_000_000 + seconds)
}

/// Writes `bytes` to `path` and sets its modification time to `when`.
fn write_at(path: &Path, bytes: &[u8], when: SystemTime) {
    fs::write(path, bytes).expect("write the file");
    set_time(path, when);
}

fn set_time(path: &Path, when: SystemTime) {
    fs::File::options()
        .write(true)
        .open(path)
        .and_then(|file| file.set_modified(when))
        .expect("set the file's modification time");
}

/// The game root below `packs`: the bundle registering KEEP and SINK, game A's `schedule.json`
/// with KEEP (producing pool A) and SINK, and both packages, every declaration file's time set to
/// `at(0)`; SINK's control file marks nothing.
fn place_root(packs: &Path) -> PathBuf {
    let game = packs.join(GAME_A);
    write_json(
        &game.join("bundle.json"),
        &bundle(GAME_A, SERVER_A, &[KEEP, SINK]),
    );
    write_json(
        &game.join("schedule.json"),
        &schedule_a(&[(KEEP, KEEP, 100), (SINK, SINK, 90)]),
    );
    write_package(
        &game.join(KEEP),
        KEEP,
        None,
        Some(json!([{"capability": "cap_k"}])),
    );
    write_package(&game.join(SINK), SINK, None, None);
    write_at(&control(&game, SINK), &sink_control(false), at(0));
    for package in [KEEP, SINK] {
        set_time(&control(&game, package), at(0));
        set_time(&manifest(&game, package), at(0));
    }
    game
}

fn control(game: &Path, package: &str) -> PathBuf {
    game.join(package).join("control.json")
}

fn manifest(game: &Path, package: &str) -> PathBuf {
    game.join(package)
        .join("resources")
        .join("operations")
        .join("resources.json")
}

/// The fixture's facts: the one instance bound to game A on server A.
fn facts() -> EvaluationFacts {
    let mut facts = policy_facts();
    facts.outcomes.clear();
    facts.instances = vec![InstanceSnapshot {
        instance_id: INSTANCE.to_owned(),
        server_id: SERVER_A.to_owned(),
        game_id: GAME_A.to_owned(),
        host_id: "fixture-host-a".to_owned(),
        available: true,
        capability_operation_ids: vec![actingcommand_policy::STANDARD_PACK_OPERATION_ID.to_owned()],
        preferred_task_ids: Vec::new(),
    }];
    facts
}

/// A composer-mode host over `packs`.
struct Scene {
    host: RuntimeHost,
    clock: Arc<ManualRuntimeClock>,
    _state: TempDir,
}

impl Scene {
    fn start(packs: &Path) -> Self {
        let state = TempDir::new().expect("state root");
        let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
        let host = RuntimeHost::start(
            config(&state)
                .with_policy_inputs(PolicyInputSnapshot::new(facts(), policy_resources()))
                .with_runtime_clock(clock.clone())
                .with_standard_packs_root(Some(packs.to_owned()))
                .with_standard_pack_schedule(true),
            Arc::new(FakeProvider::from_entries([(
                INSTANCE.to_owned(),
                instance_id(),
                Arc::new(FakeState::default()),
            )])),
        )
        .expect("start the composer-mode host");
        Self {
            host,
            clock,
            _state: state,
        }
    }

    /// One cycle with the monotonic clock at `monotonic_ms`; its check runs first.
    fn cycle(&self, monotonic_ms: u64) {
        self.clock.set_unix_ms(POLICY_NOW_UNIX_MS);
        self.clock.set_monotonic_ms(monotonic_ms);
        let sample_capacity = self
            .host
            .capacity_sampler_for_test()
            .expect("capacity owner");
        sample_capacity().expect("capacity sample");
        self.host
            .evaluate_policy_cycle_with_test_inputs(
                &facts(),
                &resources(&[POOL_A]),
                EvaluationTime {
                    unix_ms: POLICY_NOW_UNIX_MS,
                    monotonic_ms: POLICY_NOW_UNIX_MS,
                },
                7,
                PolicyTrigger::Recovery,
            )
            .unwrap_or_else(|error| panic!("cycle at {monotonic_ms}: {error}"));
    }

    /// The active composed generation's tasks by id, and those of them that carry the member
    /// `irreversible_reach`.
    fn tasks(&self) -> (BTreeSet<String>, BTreeSet<String>) {
        let (_, active) = self
            .host
            .policy_catalog_memory_for_test()
            .expect("catalog memory");
        let compiled = active
            .and_then(|handle| handle.upgrade())
            .expect("an active compiled catalog");
        let tasks = &compiled.catalog().tasks.tasks;
        (
            tasks.iter().map(|task| task.id.clone()).collect(),
            tasks
                .iter()
                .filter(|task| task.irreversible_reach.is_some())
                .map(|task| task.id.clone())
                .collect(),
        )
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

    fn schedule_reads(&self) -> u64 {
        self.host
            .standard_pack_schedule_reads_for_test(GAME_A)
            .expect("reads")
    }

    /// The `standard_pack_read` outcomes of `code`, in ledger order.
    fn reads_of(&self, code: SharedCode) -> Vec<Outcome> {
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
            .filter(|outcome| outcome.is(code))
            .collect()
    }

    fn close(self) {
        self.host.close().expect("close the composer-mode host");
    }
}

fn ids(tasks: &[&str]) -> BTreeSet<String> {
    tasks.iter().map(|task| (*task).to_owned()).collect()
}

// Host test 6 of the work order (rulings 2 and 4): KEEP's control file is broken before the
// start. The first check composes and activates the root, both rules, neither carrying the
// member, and records `standard_pack_package_unreadable` naming KEEP once, with the reader's
// refusal as its cause. SINK's control file is then replaced in place by one of the same length
// that marks `cap_x` irreversible, its time moved: the next check reads the root again and composes
// it, still without a member, so the rules digest is the active generation's and nothing is
// activated; KEEP, still unreadable, is not recorded again. KEEP readable again, the next check
// composes the same rules and records nothing more.
#[test]
fn gr2x_host_an_unreadable_package_is_warned_and_no_package_marks_a_task() {
    let packs = TempDir::new().expect("standard-pack root");
    let game = place_root(packs.path());
    let keep = fs::read(control(&game, KEEP)).expect("KEEP's control bytes");
    write_at(&control(&game, KEEP), b"{", at(5));
    let scene = Scene::start(packs.path());

    scene.cycle(0);
    assert_eq!(scene.activations(), 1, "composed and activated");
    assert_eq!(
        scene.tasks(),
        (ids(&[KEEP, SINK]), ids(&[])),
        "both rules, no member"
    );
    let unreadable = scene.reads_of(SharedCode::StandardPackPackageUnreadable);
    assert_eq!(unreadable.len(), 1, "{unreadable:#?}");
    assert_eq!(unreadable[0].value("game"), Some(&json!(GAME_A)));
    assert_eq!(unreadable[0].value("package_id"), Some(&json!(KEEP)));
    assert!(
        unreadable[0]
            .causes()
            .iter()
            .any(|link| link.relation().as_str() == "caused_by"),
        "the reader's refusal: {unreadable:#?}"
    );

    let marked = sink_control(true);
    assert_eq!(marked.len(), sink_control(false).len(), "the same length");
    let reads = scene.schedule_reads();
    write_at(&control(&game, SINK), &marked, at(10));
    scene.cycle(INTERVAL_MS);
    assert_eq!(scene.schedule_reads(), reads + 1, "the root is read again");
    assert_eq!(scene.tasks(), (ids(&[KEEP, SINK]), ids(&[])), "no member");
    assert_eq!(scene.activations(), 1, "the same rules, nothing activated");
    assert_eq!(
        scene
            .reads_of(SharedCode::StandardPackPackageUnreadable)
            .len(),
        1,
        "recorded once while it stays unreadable"
    );

    write_at(&control(&game, KEEP), &keep, at(20));
    scene.cycle(2 * INTERVAL_MS);
    assert_eq!(scene.tasks(), (ids(&[KEEP, SINK]), ids(&[])));
    assert_eq!(scene.activations(), 1);
    assert_eq!(
        scene
            .reads_of(SharedCode::StandardPackPackageUnreadable)
            .len(),
        1,
        "nothing more"
    );
    scene.close();
}

// G-R2s fix round 1, point 5, under G-R2x: a declaration file whose metadata cannot be read
// (another error than its absence; the gate tests' seam `DECLARATION_METADATA_FAILS_FOR_TEST`, for
// SINK's control file of this test's own root) is a mark of its own. The check goes on and reads
// the root again, SINK counts as unreadable without the reader running, and
// `standard_pack_package_unreadable` names SINK once, with no cause; the composition is the one of
// before, without a member, so nothing is activated. Once the metadata reads again nothing more is
// recorded.
#[test]
fn gr2x_host_a_declaration_file_whose_metadata_fails_is_warned_and_the_check_goes_on() {
    let packs = TempDir::new().expect("standard-pack root");
    let game = place_root(packs.path());
    let scene = Scene::start(packs.path());
    scene.cycle(0);
    assert_eq!(scene.tasks(), (ids(&[KEEP, SINK]), ids(&[])));
    let activations = scene.activations();
    let reads = scene.schedule_reads();

    let failing = control(&game, SINK);
    let seam = || {
        crate::host::DECLARATION_METADATA_FAILS_FOR_TEST
            .lock()
            .expect("the metadata seam")
    };
    seam().insert(failing.clone());
    scene.cycle(INTERVAL_MS);
    seam().remove(&failing);
    assert_eq!(
        scene.schedule_reads(),
        reads + 1,
        "the check went on and read the root"
    );
    assert_eq!(scene.tasks(), (ids(&[KEEP, SINK]), ids(&[])), "no member");
    assert_eq!(scene.activations(), activations, "nothing activated");
    let unreadable = scene.reads_of(SharedCode::StandardPackPackageUnreadable);
    assert_eq!(unreadable.len(), 1, "{unreadable:#?}");
    assert_eq!(unreadable[0].value("game"), Some(&json!(GAME_A)));
    assert_eq!(unreadable[0].value("package_id"), Some(&json!(SINK)));
    assert!(
        unreadable[0].causes().is_empty(),
        "the reader did not run: {unreadable:#?}"
    );

    scene.cycle(2 * INTERVAL_MS);
    assert_eq!(scene.tasks(), (ids(&[KEEP, SINK]), ids(&[])));
    assert_eq!(
        scene
            .reads_of(SharedCode::StandardPackPackageUnreadable)
            .len(),
        1,
        "the metadata reads again"
    );
    scene.close();
}

// check-config's composition (`compose_configured_standard_packs`) reads the packages as the
// workbench does and, SINK's control file marking `cap_x` irreversible, composes both rules
// without a member.
#[test]
fn gr2x_host_check_config_composes_no_mark() {
    let packs = TempDir::new().expect("standard-pack root");
    let game = place_root(packs.path());
    write_at(&control(&game, SINK), &sink_control(true), at(10));
    let state = TempDir::new().expect("state root");
    let host = config(&state)
        .with_policy_inputs(PolicyInputSnapshot::new(facts(), policy_resources()))
        .with_standard_packs_root(Some(packs.path().to_owned()))
        .with_standard_pack_schedule(true);
    let schedule = crate::compose_configured_standard_packs(&host)
        .unwrap_or_else(|error| panic!("compose the configuration's rules: {error}"));
    let compiled = schedule.compiled().expect("the rules compose");
    let tasks = &compiled.catalog().tasks.tasks;
    assert_eq!(
        tasks
            .iter()
            .map(|task| task.id.clone())
            .collect::<BTreeSet<_>>(),
        ids(&[KEEP, SINK])
    );
    assert!(
        tasks.iter().all(|task| task.irreversible_reach.is_none()),
        "{tasks:#?}"
    );
}
