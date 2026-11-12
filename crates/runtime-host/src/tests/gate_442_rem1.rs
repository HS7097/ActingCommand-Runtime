// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #442 REM-1 (register §1.326; contracts/remedies.md section 7.1; coordinator work order
// rd-1112/brief-442-rem-1.md): in composer mode the remedy table the start builds reads the
// active composed generation's packages, so a blocked result after a restart opens its round and
// a round opened before it stays open; every table says in the log what it found, and names the
// catalog tasks without a package location.
//
// A neutral fixture. The host runs in composer mode (no configured catalog) over one game root on
// disk, a manual Runtime clock and one fake instance. The root's `schedule.json` holds two rules
// in catalog-a's task shape: `fixture.blocked`, due at every evaluation, and `fixture.sink`, whose
// trigger is never true by itself; each rule's task and package share its id. The frames are 2x1
// and their first pixel is the screen: red `home`, blue `page_b`, white `result`. Each run starts
// on `home` and its one tap shows the page the test chose for that run. Package `fixture.blocked`
// goes from `home` to `page_b` or `result` and maps `page_b` to the key `blocked_k`, whose remedy
// serves the need `need_a` with the capability `cap_x` (its package default); package
// `fixture.sink` goes from `home` to `result` and provides `cap_x`, irreversible, with one
// parameter. Every expected value is the work order's requirement or `contracts/remedies.md`'s.

use super::gate_rl1b::{
    GAME_A, POOL_A, SERVER_A, bundle, pool, profiles, resources, rule, write_json,
};
use super::*;
use actingcommand_contract::outcome::Outcome;
use actingcommand_policy::{PolicyEvaluation, STANDARD_PACKS_CATALOG_ID};
use serde_json::{Value, json};

/// The blocked rule: its task id and its package id.
const BLOCKED_TASK: &str = "fixture.blocked";
/// The provider rule: its task id and its package id.
const SINK_TASK: &str = "fixture.sink";
const INSTANCE: &str = POLICY_INSTANCE_ALIAS;
const BLOCKED_KEY: &str = "blocked_k";
const DONE_KEY: &str = "done";
const NEED: &str = "need_a";
const CAPABILITY: &str = "cap_x";
const HOME: [u8; 3] = [255, 0, 0];
const PAGE_B: [u8; 3] = [0, 0, 255];
const RESULT: [u8; 3] = [255, 255, 255];

const WAITING: &str = "policy_remedy_waiting";
const REQUESTED: &str = "policy_remedy_requested";

// ---- The game root.

// One package: from `home` to `page_b` or `result` when it declares `remedy` on the mapping of
// `page_b`, otherwise from `home` to `result` with no outcome declaration; with `provides` when
// one is given.
fn write_package(
    directory: &Path,
    package_id: &str,
    remedy: Option<Value>,
    provides: Option<Value>,
) {
    let blocking = remedy.is_some();
    let pages: &[(&str, [u8; 3])] = if blocking {
        &[("home", HOME), ("page_b", PAGE_B), ("result", RESULT)]
    } else {
        &[("home", HOME), ("result", RESULT)]
    };
    let mut control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": package_id,
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 10,
        "step_timeout_ms": 600,
        "timeout_ms": 8000,
        "max_steps": 2
    });
    if let Some(provides) = provides {
        control["provides"] = provides;
    }
    let ends = if blocking {
        json!(["page_b", "result"])
    } else {
        json!("result")
    };
    let mut task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "locale": "en-US",
        "goal": "REM-1 fixture",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
        "color_probes": pages
            .iter()
            .map(|(page, colour)| json!({
                "id": format!("{page}_colour"),
                "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
                "expected": colour
            }))
            .collect::<Vec<_>>(),
        "page_rules": pages
            .iter()
            .map(|(page, _)| ((*page).to_owned(), json!({"required": [format!("{page}_colour")]})))
            .collect::<serde_json::Map<_, _>>(),
        "entry_page": "home",
        "target_page": ends.clone(),
        "operations": [{
            "id": "go",
            "from": "home",
            "to": ends,
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true,
            "retryable": false
        }]
    });
    if let Some(remedy) = remedy {
        task["scheduling_outcome"] = json!({"mappings": [
            {"outcome_key": BLOCKED_KEY, "effect": "no_designated_effect",
             "terminal_pages": ["page_b"], "remedy": remedy},
            {"outcome_key": DONE_KEY, "effect": "no_designated_effect",
             "terminal_pages": ["result"]}
        ]});
    }
    for (file, value) in [
        ("control.json", control),
        (
            "resources/operations/resources.json",
            json!({"schema_version": "1.0", "resources": [], "resource_count": 0}),
        ),
        ("resources/operations/task/task.json", task),
    ] {
        write_json(&directory.join(file), &value);
    }
}

// The game root's `schedule.json`: the blocked rule, due at every evaluation, and the provider
// rule, never due by itself, both producing into pool A, with room in the activity window for
// every run of a test.
fn schedule() -> Value {
    let mut blocked = rule(BLOCKED_TASK, BLOCKED_TASK, 100, Some(POOL_A));
    blocked["trigger"] = json!({
        "kind": "clock",
        "schedule": {"kind": "interval", "clock_source": {"kind": "local"}, "every_ms": 1,
                     "anchor_ms": 0}
    });
    let mut sink = rule(SINK_TASK, SINK_TASK, 50, Some(POOL_A));
    sink["trigger"] = sink["feedback_stop"].clone();
    for spec in [&mut blocked, &mut sink] {
        spec["loop_budget"]["window_iteration_limit"] = json!(8);
        // SCH-M ED-2: after its first run a task reserves its expected duration E, its run plus
        // the 300 000 ms margin; one hour holds every run of a test (as `gate_411_b15h`).
        spec["loop_budget"]["max_runtime_ms"] = json!(3_600_000);
    }
    let mut activity = profiles("", Some(POOL_A));
    for profile in &mut activity {
        profile["max_window_iterations"] = json!(8);
    }
    json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": [],
        "rules": [blocked, sink],
        "pools": [pool(POOL_A)],
        "activity": activity,
        "timeline": [],
        "held": []
    })
}

// Places the game root below `packs`: its bundle, its `schedule.json` and both packages, each in
// the directory the bundle names for it.
fn place_game_root(packs: &Path) {
    let game = packs.join(GAME_A);
    write_json(
        &game.join("bundle.json"),
        &bundle(GAME_A, SERVER_A, &[BLOCKED_TASK, SINK_TASK]),
    );
    write_json(&game.join("schedule.json"), &schedule());
    write_package(
        &game.join(BLOCKED_TASK),
        BLOCKED_TASK,
        Some(json!({"need": NEED, "capabilities": [CAPABILITY], "default": [CAPABILITY]})),
        None,
    );
    write_package(
        &game.join(SINK_TASK),
        SINK_TASK,
        None,
        Some(
            json!([{"capability": CAPABILITY, "irreversible": true, "parameters": [
                {"name": "param_p", "options": ["opt_1", "opt_2"], "default": "opt_1"}
            ]}]),
        ),
    );
}

// The fixture's facts: the one instance bound to game A on server A, able to run the Runtime's
// package operation, and no recorded outcome.
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

// ---- The device.

// The device: `home` until the run's tap, then the page the test chose for the run.
struct Screen {
    taps_before: Arc<AtomicUsize>,
    after_tap: Arc<std::sync::Mutex<[u8; 3]>>,
}

impl Screen {
    fn install(state: &FakeState) -> Self {
        let taps_before = Arc::new(AtomicUsize::new(0));
        let after_tap = Arc::new(std::sync::Mutex::new(RESULT));
        let (before, after) = (Arc::clone(&taps_before), Arc::clone(&after_tap));
        *state.pixel_script.lock().expect("pixel script") =
            Some(Arc::new(move |_: usize, inputs: usize| {
                let page = if inputs <= before.load(Ordering::Acquire) {
                    HOME
                } else {
                    *after.lock().expect("the page after the tap")
                };
                vec![(0, page)]
            }));
        Self {
            taps_before,
            after_tap,
        }
    }

    // The next run starts on `home` and its tap shows `page`.
    fn next(&self, state: &FakeState, page: [u8; 3]) {
        self.taps_before
            .store(state.input_count.load(Ordering::Acquire), Ordering::Release);
        *self.after_tap.lock().expect("the page after the tap") = page;
    }
}

// ---- The fixture.

struct Scene {
    state_root: TempDir,
    packs: TempDir,
    instance: InstanceId,
    device: Arc<FakeState>,
    screen: Screen,
    clock: Arc<ManualRuntimeClock>,
    host: RuntimeHost,
}

fn start_host(
    state_root: &TempDir,
    packs: &TempDir,
    clock: &Arc<ManualRuntimeClock>,
    device: &Arc<FakeState>,
    instance: InstanceId,
) -> RuntimeHost {
    RuntimeHost::start(
        config(state_root)
            .with_policy_inputs(PolicyInputSnapshot::new(facts(), policy_resources()))
            .with_runtime_clock(clock.clone())
            .with_standard_packs_root(Some(packs.path().to_owned()))
            .with_standard_pack_schedule(true),
        Arc::new(FakeProvider::one(INSTANCE, instance, Arc::clone(device)).fixture_simulation()),
    )
    .expect("start the composer-mode host")
}

impl Scene {
    // A composer-mode host over the placed game root, its clock at the fixture's time; nothing
    // is composed before its first cycle.
    fn new() -> Self {
        let state_root = TempDir::new().expect("state root");
        let packs = TempDir::new().expect("standard-pack root");
        place_game_root(packs.path());
        let device = Arc::new(FakeState::default());
        let screen = Screen::install(&device);
        let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
        let instance = instance_id();
        let host = start_host(&state_root, &packs, &clock, &device, instance);
        Self {
            state_root,
            packs,
            instance,
            device,
            screen,
            clock,
            host,
        }
    }

    // The host closed and started again on the same files, state root and instance, its clock at
    // `at`.
    fn restart(self, at: u64) -> Self {
        let Self {
            state_root,
            packs,
            instance,
            device,
            screen,
            host,
            ..
        } = self;
        host.close().expect("close the composer-mode host");
        let clock = Arc::new(ManualRuntimeClock::new(at, 0));
        let host = start_host(&state_root, &packs, &clock, &device, instance);
        Self {
            state_root,
            packs,
            instance,
            device,
            screen,
            clock,
            host,
        }
    }

    // The host's capacity at the clock's time, as its capacity monitor samples it.
    fn sample_capacity(&self) {
        self.host
            .capacity_sampler_for_test()
            .expect("capacity owner")()
        .expect("capacity sample");
    }

    // One full evaluation at `at`, the Runtime clock moved there first; its cycle runs the
    // scheduler workbench's check first.
    fn evaluate(&self, at: u64, seed: u64) -> PolicyEvaluation {
        advance_manual_clock_to(&self.clock, at);
        self.sample_capacity();
        self.host
            .evaluate_policy_cycle_with_test_inputs(
                &facts(),
                &resources(&[POOL_A]),
                EvaluationTime {
                    unix_ms: at,
                    monotonic_ms: at,
                },
                seed,
                PolicyTrigger::FactsChanged,
            )
            .unwrap_or_else(|error| panic!("evaluate the policy at {at}: {error}"))
            .evaluation
            .expect("a full evaluation")
    }

    // The request of `task_id`'s package, at the place the composed procedure binds it.
    fn request(&self, task_id: &str) -> ContainedTaskRequest {
        ContainedTaskRequest::new(
            self.packs
                .path()
                .join(GAME_A)
                .join(task_id)
                .to_string_lossy()
                .into_owned(),
        )
        .expect("contained task request")
    }

    // Admits `intent` with its package's request, runs the package with the device showing
    // `page` after the tap, and settles the successful run.
    fn run(
        &self,
        intent: &DispatchIntent,
        chain: &DecisionReasonChain,
        page: [u8; 3],
    ) -> Box<PolicyRunContext> {
        self.sample_capacity();
        let admission = PolicyAdmissionContext {
            fact_ledger_position: intent.input_ledger_position,
            fact_snapshot_id: intent.fact_snapshot_id.clone(),
            approval_fact_ids: BTreeSet::new(),
            fencing_owner_epoch: self.host.runtime_info().owner_epoch(),
            now_unix_ms: intent.prerequisites.evaluated_at_unix_ms,
        };
        let PolicyDispatchAdmission::Granted { context } = self
            .host
            .admit_scheduled_policy_dispatch(
                intent,
                chain,
                &admission,
                &self.request(&intent.task_id),
            )
            .unwrap_or_else(|error| panic!("admit {}: {error}", intent.task_id))
        else {
            panic!("expected one policy run context")
        };
        self.screen.next(&self.device, page);
        let receipt = self
            .host
            .run_scheduled_contained_task(&context, &self.request(&intent.task_id))
            .expect("the scheduled run");
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "{receipt:?}"
        );
        self.host
            .complete_scheduled_policy_run(&context, &receipt)
            .expect("the run settles");
        context
    }

    // Evaluates at `at`, then runs the dispatch of `task_id` it must select, the device showing
    // `page` after the tap.
    fn evaluate_and_run(
        &self,
        at: u64,
        seed: u64,
        task_id: &str,
        page: [u8; 3],
    ) -> Box<PolicyRunContext> {
        let evaluation = self.evaluate(at, seed);
        let (intent, chain) = dispatched(&evaluation, task_id)
            .unwrap_or_else(|| panic!("a dispatch of {task_id}: {evaluation:#?}"));
        self.run(&intent, &chain, page)
    }

    // The remedy table in effect.
    fn table(&self) -> crate::remedy::BuiltTable {
        self.host
            .remedy_table_for_test()
            .expect("the policy host")
            .expect("the remedy table")
    }

    fn active(&self) -> CatalogGeneration {
        self.host
            .active_policy_catalog()
            .expect("read the active catalog")
            .expect("an active catalog")
    }

    // The persisted records of `event_type`, in ledger order.
    fn records(&self, event_type: EventType) -> Vec<PersistedEvent> {
        self.host
            .query_persisted_events_for_test(EventQuery {
                event_type: Some(event_type),
                ..EventQuery::default()
            })
            .expect("ledger records")
    }

    // The outcome keys the terminal of the run of `context` names.
    fn terminal_keys(&self, context: &PolicyRunContext) -> Vec<String> {
        self.host
            .query_persisted_events_for_test(EventQuery {
                run_id: Some(context.run_id()),
                event_type: Some(EventType::TaskCompleted),
                ..EventQuery::default()
            })
            .expect("the run's terminal")
            .iter()
            .filter_map(|event| match event.payload() {
                EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
                    TaskSemanticFact::TerminalCommitted {
                        scheduling_disposition: Some(disposition),
                        ..
                    } => Some(disposition.outcome_key().to_owned()),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    fn close(self) {
        self.host.close().expect("close the composer-mode host");
    }
}

// The evaluation's dispatch of `task_id` with its reason chain, if it has one.
fn dispatched(
    evaluation: &PolicyEvaluation,
    task_id: &str,
) -> Option<(DispatchIntent, DecisionReasonChain)> {
    let intent = evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.task_id == task_id && intent.instance_id == INSTANCE)?
        .clone();
    let chain = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .expect("the intent's reason chain")
        .clone();
    Some((intent, chain))
}

// The reasons of the evaluation's decision of `task_id` on the fixture instance.
fn decision_reasons(evaluation: &PolicyEvaluation, task_id: &str) -> Vec<Outcome> {
    evaluation
        .decisions
        .iter()
        .filter(|decision| {
            decision.task_id == task_id && decision.instance_id.as_deref() == Some(INSTANCE)
        })
        .flat_map(|decision| decision.reasons.iter().map(|reason| reason.outcome.clone()))
        .collect()
}

// The outcomes of `code` among the reasons of `chain`.
fn chain_reasons_of(chain: &DecisionReasonChain, code: &str) -> Vec<Outcome> {
    chain
        .reasons
        .iter()
        .filter(|reason| reason.outcome.code().as_str() == code)
        .map(|reason| reason.outcome.clone())
        .collect()
}

// The outcome's value of `key` as text.
fn text<'a>(outcome: &'a Outcome, key: &str) -> Option<&'a str> {
    outcome.value(key).and_then(Value::as_str)
}

// When the instance may run its next dispatch after the run of `context`.
fn next_eligible(context: &PolicyRunContext) -> u64 {
    context.admission().activity.next_eligible_unix_ms
}

// The table declares the blocked task's remedy and lists its provider, as the packages declare
// them, and its summary line says so: two catalog tasks, both with a package location, one that
// declares a remedy, one provider, none without a location, built from a manifest.
fn assert_declares_remedies(table: &crate::remedy::BuiltTable, catalog_hash: &str, when: &str) {
    assert!(table.declares_remedies(BLOCKED_TASK), "{when}: {table:#?}");
    assert!(
        table.remedy_key(BLOCKED_TASK, BLOCKED_KEY),
        "{when}: {table:#?}"
    );
    let remedies = table
        .table
        .remedies
        .iter()
        .map(|remedy| {
            (
                remedy.task_id.as_str(),
                remedy.outcome_key.as_str(),
                remedy.need.as_str(),
                remedy.capabilities.clone(),
                remedy.default.clone(),
                remedy.problem.is_none(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        remedies,
        [(
            BLOCKED_TASK,
            BLOCKED_KEY,
            NEED,
            vec![CAPABILITY.to_owned()],
            vec![CAPABILITY.to_owned()],
            true,
        )],
        "{when}"
    );
    let providers = table
        .table
        .providers
        .iter()
        .map(|provider| {
            (
                provider.task_id.as_str(),
                provider
                    .provides
                    .iter()
                    .map(|provided| (provided.capability.as_str(), provided.irreversible))
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(providers, [(SINK_TASK, vec![(CAPABILITY, true)])], "{when}");
    assert!(table.unbound_tasks().is_empty(), "{when}: {table:#?}");
    let summary = table.summary_line();
    assert!(
        summary.starts_with(&format!(
            "actingd remedy_table built catalog_hash={catalog_hash} manifest=sha256:"
        )),
        "{when}: {summary}"
    );
    assert!(
        summary.ends_with(" tasks=2 located=2 remedy_tasks=1 providers=1 unbound=0"),
        "{when}: {summary}"
    );
    assert_eq!(table.unbound_line(), None, "{when}");
}

// ---- Tests.

// Work order, test 1: in composer mode, started twice on the same files, the table the second
// start builds declares the blocked task's remedy and lists the provider before the first cycle
// and still after it; the restart activates nothing.
#[test]
fn composer_restart_table_declares_remedies() {
    let scene = Scene::new();
    // The first start composes the rules at its first cycle and activates them.
    scene.evaluate(POLICY_NOW_UNIX_MS, 7);
    let composed = scene.active();
    assert_eq!(composed.catalog_id(), STANDARD_PACKS_CATALOG_ID);
    assert_declares_remedies(&scene.table(), composed.catalog_hash(), "first start");

    let scene = scene.restart(POLICY_RESTART_UNIX_MS);
    assert_eq!(
        scene.active().catalog_hash(),
        composed.catalog_hash(),
        "the restart loads the same generation"
    );
    assert_declares_remedies(
        &scene.table(),
        composed.catalog_hash(),
        "second start, before its first cycle",
    );
    scene.evaluate(POLICY_RESTART_UNIX_MS, 8);
    assert_declares_remedies(
        &scene.table(),
        composed.catalog_hash(),
        "second start, after its first cycle",
    );
    assert_eq!(scene.active().catalog_hash(), composed.catalog_hash());
    assert_eq!(
        scene.records(EventType::CatalogActivated).len(),
        1,
        "nothing is activated again"
    );
    scene.close();
}

// Work order, test 2: after a restart the blocked task succeeds with the remedy key. In the next
// cycle, inside the instance's activity interval, the blocked task waits with
// `policy_remedy_waiting` for its round; once the instance may run again, the provider's intent
// carries `policy_remedy_requested` (source `package`, `irreversible` true) and the blocked task is
// not dispatched.
#[test]
fn composer_restart_blocked_run_opens_round() {
    let scene = Scene::new();
    scene.evaluate(POLICY_NOW_UNIX_MS, 7);
    assert_eq!(scene.active().catalog_id(), STANDARD_PACKS_CATALOG_ID);

    let scene = scene.restart(POLICY_RESTART_UNIX_MS);
    let blocked = scene.evaluate_and_run(POLICY_RESTART_UNIX_MS, 8, BLOCKED_TASK, PAGE_B);
    assert_eq!(scene.terminal_keys(&blocked), [BLOCKED_KEY]);
    let round = blocked.decision_id().to_owned();

    let waiting_at = POLICY_RESTART_UNIX_MS + 2_000;
    assert!(waiting_at < next_eligible(&blocked), "inside the interval");
    let waiting = scene.evaluate(waiting_at, 9);
    assert!(dispatched(&waiting, BLOCKED_TASK).is_none(), "{waiting:#?}");
    assert!(dispatched(&waiting, SINK_TASK).is_none(), "{waiting:#?}");
    let reasons = decision_reasons(&waiting, BLOCKED_TASK);
    let hold = reasons
        .iter()
        .find(|reason| reason.code().as_str() == WAITING)
        .unwrap_or_else(|| panic!("the blocked task waits for its remedy: {reasons:?}"));
    assert_eq!(text(hold, "task_id"), Some(BLOCKED_TASK));
    assert_eq!(text(hold, "instance_id"), Some(INSTANCE));
    assert_eq!(text(hold, "decision_id"), Some(round.as_str()));
    assert_eq!(text(hold, "outcome_key"), Some(BLOCKED_KEY));
    assert_eq!(text(hold, "need"), Some(NEED));

    let requesting = scene.evaluate(next_eligible(&blocked), 10);
    assert!(
        dispatched(&requesting, BLOCKED_TASK).is_none(),
        "the blocked task is held: {requesting:#?}"
    );
    let (_, chain) = dispatched(&requesting, SINK_TASK)
        .unwrap_or_else(|| panic!("the round requests its remedy: {requesting:#?}"));
    let requested = chain_reasons_of(&chain, REQUESTED);
    assert_eq!(requested.len(), 1, "{chain:?}");
    assert_eq!(text(&requested[0], "task_id"), Some(BLOCKED_TASK));
    assert_eq!(text(&requested[0], "decision_id"), Some(round.as_str()));
    assert_eq!(text(&requested[0], "outcome_key"), Some(BLOCKED_KEY));
    assert_eq!(text(&requested[0], "need"), Some(NEED));
    assert_eq!(text(&requested[0], "capability"), Some(CAPABILITY));
    assert_eq!(text(&requested[0], "remedy_source"), Some("package"));
    assert_eq!(requested[0].value("irreversible"), Some(&json!(true)));
    scene.close();
}

// Work order, test 3: a blocking run from before the restart still opens its round after it: the
// start reads that run's key from its terminal, and the first cycle after the restart requests
// the provider for that round instead of dispatching the blocked task again.
#[test]
fn composer_restart_keeps_round_opened_before() {
    let scene = Scene::new();
    let blocked = scene.evaluate_and_run(POLICY_NOW_UNIX_MS, 7, BLOCKED_TASK, PAGE_B);
    assert_eq!(scene.active().catalog_id(), STANDARD_PACKS_CATALOG_ID);
    assert_eq!(scene.terminal_keys(&blocked), [BLOCKED_KEY]);
    let round = blocked.decision_id().to_owned();

    let restart_at = next_eligible(&blocked);
    let scene = scene.restart(restart_at);
    let requesting = scene.evaluate(restart_at, 8);
    assert!(
        dispatched(&requesting, BLOCKED_TASK).is_none(),
        "the blocked task is held: {requesting:#?}"
    );
    let (_, chain) = dispatched(&requesting, SINK_TASK)
        .unwrap_or_else(|| panic!("the round opened before the restart: {requesting:#?}"));
    let requested = chain_reasons_of(&chain, REQUESTED);
    assert_eq!(requested.len(), 1, "{chain:?}");
    assert_eq!(text(&requested[0], "task_id"), Some(BLOCKED_TASK));
    assert_eq!(text(&requested[0], "decision_id"), Some(round.as_str()));
    assert_eq!(text(&requested[0], "outcome_key"), Some(BLOCKED_KEY));
    assert_eq!(text(&requested[0], "remedy_source"), Some("package"));
    scene.close();
}

// Work order, test 4: the catalog tasks whose procedure has no package location (a binding
// without one, no binding, no manifest) are listed in catalog order; the summary line gives
// their number, and the second line names the first ten and counts the rest. A located package
// that cannot be read is unreadable, not unbound.
#[test]
fn remedy_table_names_unbound_tasks() {
    // Twelve tasks of the fixture catalog, each with its own procedure.
    let mut sources = policy_sources(1);
    let mut tasks: Value = serde_json::from_slice(&sources.tasks.bytes).expect("fixture tasks");
    let template = tasks["tasks"][0].clone();
    tasks["tasks"] = Value::Array(
        (0..12)
            .map(|index| {
                let mut task = template.clone();
                task["id"] = json!(format!("fixture.task-{index:02}"));
                task["procedure_ref"] = json!(format!("procedure.task-{index:02}"));
                task
            })
            .collect(),
    );
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("fixture task bytes");
    let compiled = compile_catalog(&sources).expect("the catalog compiles");
    let ids = compiled
        .catalog()
        .tasks
        .tasks
        .iter()
        .map(|task| task.id.clone())
        .collect::<Vec<_>>();
    assert_eq!(ids.len(), 12);

    // The manifest locates the first task's package (a place that holds none) and binds the
    // second task's procedure without a location; it binds no other task's procedure.
    let root = TempDir::new().expect("tempdir");
    let manifest = ProcedureManifest::new([
        ProcedureBinding::new(
            "procedure.task-00",
            "operation.observe",
            vec!["after_observation".to_owned()],
        )
        .expect("procedure binding")
        .with_package_path(root.path().join("absent").to_string_lossy()),
        ProcedureBinding::new(
            "procedure.task-01",
            "operation.observe",
            vec!["after_observation".to_owned()],
        )
        .expect("procedure binding"),
    ])
    .expect("procedure manifest");
    let built = crate::remedy::build_table(&compiled, Some(&manifest), &[], 0);
    let unbound = ids
        .iter()
        .filter(|id| id.as_str() != "fixture.task-00")
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(built.unbound_tasks(), unbound.as_slice());
    assert_eq!(
        built.summary_line(),
        format!(
            "actingd remedy_table built catalog_hash={} manifest={} tasks=12 located=1 remedy_tasks=0 providers=0 unbound=11",
            compiled.catalog_hash(),
            manifest.digest(),
        )
    );
    assert_eq!(
        built.unbound_line(),
        Some(format!(
            "actingd remedy_table unbound tasks={} more=1",
            unbound[..10].join(",")
        ))
    );
    assert_eq!(built.unreadable.len(), 1, "{:?}", built.unreadable);
    assert_eq!(built.unreadable[0].tasks, ["fixture.task-00"]);

    // Without a manifest every task is without a package location, and the line says so.
    let bare = crate::remedy::build_table(&compiled, None, &[], 0);
    assert_eq!(bare.unbound_tasks(), ids.as_slice());
    assert_eq!(
        bare.summary_line(),
        format!(
            "actingd remedy_table built catalog_hash={} manifest=none tasks=12 located=0 remedy_tasks=0 providers=0 unbound=12",
            compiled.catalog_hash(),
        )
    );
    assert_eq!(
        bare.unbound_line(),
        Some(format!(
            "actingd remedy_table unbound tasks={} more=2",
            ids[..10].join(",")
        ))
    );
    assert!(bare.unreadable.is_empty());
}
