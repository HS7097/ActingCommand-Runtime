// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 B8 gate test, host half (plan `rd-1021/train-0129/b-plan/plan.md` §2.8 and §5.3
// BT-B8 T2; `contracts/remedies.md` section 4; contracts/instance-fact-store.md, "Run
// parameters"; host gate key 128; register §1.146): the host overlays the schedule values a
// policy dispatch carries as `session.dispatch.schedule.<key>` on the instance fact snapshots of
// that run, so its select step reads them; a direct run has none, and the package policy's
// `on_unknown` decides. Every expected value comes from §2.8.
//
// A neutral fixture, the frames and select step of `gate_411_b15r`: frames are 2x1, the first
// pixel the page (red `home`, white `result`), the second green. The package has one select step
// from `home` to `result` over two fixed slots, whose feature `first` passes on slot 0 only. Its
// policy declares the five schedule values as facts; one gate holds when all five are known, the
// other admits slot 0 when `last_in_window` is false and slot 1 when it is true, and both abort
// the evaluation on an unknown value. A policy run is the catalog's one task `t_rounds`, opted in
// with `dispatch_parameters: true`, on an hourly clock, evaluated at 12:00 UTC inside the fixture
// instance's activity window of 08:00-22:00 UTC: its next occurrence, 13:00, is inside the window.

use super::*;
use actingcommand_contract::{
    TaskSelectionOutcome, TaskSelectionRecord, TaskSelectionUnknownReason,
};
use actingcommand_policy::PolicyEvaluation;
use serde_json::{Value, json};

// The fixture instance's configured game and server, which a policy run's package declares.
const GAME: &str = "fixture-game-a";
const SERVER: &str = "fixture-server-a";
// A direct run's instance, which no policy input names, and its package's game and server.
const DIRECT_ALIAS: &str = "neutral.instance";
const DIRECT_GAME: &str = "neutral";
const DIRECT_SERVER: &str = "test";
const TASK: &str = "t_rounds";
const PROCEDURE: &str = "procedure.rounds";
const PACKAGE_ID: &str = "neutral.test.rounds";
const SCHEDULE_PREFIX: &str = "session.dispatch.schedule.";
const HOME: [u8; 3] = [255, 0, 0];
const RESULT: [u8; 3] = [255, 255, 255];
const GREEN: [u8; 3] = [0, 255, 0];
const BLUE: [u8; 3] = [0, 0, 255];
// The candidate a run whose round is not the window's last chooses.
const FIRST: &str = "choices#00";
const PARAMETERS: &str = "policy_dispatch_parameters";
const HOUR_MS: u64 = 3_600_000;

// ---- The package.

// The package of `game` and `server` as an archive.
fn package_bytes(game: &str, server: &str) -> Vec<u8> {
    let page = |name: &str| format!("{game}/{name}");
    let space = json!({"width": 2, "height": 1});
    let slots = [(0, "slot/first"), (1, "slot/second")]
        .into_iter()
        .map(|(x, target)| {
            json!({
                "rect": {"x": x, "y": 0, "width": 1, "height": 1},
                "click": {"x": x, "y": 0, "width": 1, "height": 1},
                "targets": {"first": target}
            })
        })
        .collect::<Vec<_>>();
    let layout = json!({
        "id": "choices",
        "page_id": page("home"),
        "kind": "fixed_slots",
        "features": [{"name": "first", "value": "passed"}],
        "slots": slots
    });
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": PACKAGE_ID,
        "execution_mode": "navigable_route",
        "game": game,
        "server": server,
        "resolution": space.clone(),
        "entry_task_id": "task",
        "capture_interval_ms": 10,
        "step_timeout_ms": 600,
        "timeout_ms": 8000,
        "max_steps": 2
    });
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": game,
        "server_scope": [server],
        "coordinate_space": space.clone(),
        "target_page": "result",
        "candidate_layouts": [layout.clone()],
        "operations": [{
            "id": "pick",
            "from": "home",
            "to": "result",
            "expect_after": {"page_id": "result"},
            "guard": {
                "page_id": "home",
                "target_id": "guard/ready",
                "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                "color_probe": "guard/ready"
            },
            "select": {"layout_id": "choices", "policy": {"path": "policies/by_round.json"}},
            "retryable": false
        }]
    });
    let fact = |key: &str| json!({"source": "fact", "fact_key": format!("{SCHEDULE_PREFIX}{key}")});
    let declared = |key: &str, value_type: &str| {
        json!({"fact_key": format!("{SCHEDULE_PREFIX}{key}"),
               "value_type": {"type": value_type},
               "max_age_ms": 86_400_000, "minimum_confidence_milli": 1000})
    };
    let known_integer =
        |key: &str| json!({"kind": "integer_at_least", "value": fact(key), "threshold": 0});
    let known_boolean = |key: &str| {
        json!({"kind": "any", "of": [
            {"kind": "boolean_equals", "value": fact(key), "expected": true},
            {"kind": "boolean_equals", "value": fact(key), "expected": false}
        ]})
    };
    let reads = |last: bool, first: bool| {
        json!({"kind": "all", "of": [
            {"kind": "boolean_equals", "value": fact("last_in_window"), "expected": last},
            {"kind": "boolean_equals", "value": {"source": "field", "field": "first"},
             "expected": first}
        ]})
    };
    let policy = json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": "by-round",
        "applies_to": {
            "candidate_layout_id": "choices",
            "outcome_keys": {"selected": "slot_selected", "empty": "slot_empty",
                             "insufficient": "slot_insufficient",
                             "ambiguous": "slot_ambiguous", "unknown": "slot_unknown"}
        },
        "fields": [{"name": "first", "value_type": {"type": "boolean"}}],
        "facts": [
            declared("next_occurrence_unix_ms", "integer"),
            declared("window_end_unix_ms", "integer"),
            declared("last_in_window", "boolean"),
            declared("last_daily_slot", "boolean"),
            declared("daily_remaining", "integer")
        ],
        "gates": [
            {"gate_id": "schedule_known",
             "predicate": {"kind": "all", "of": [
                 known_integer("next_occurrence_unix_ms"),
                 known_integer("window_end_unix_ms"),
                 known_boolean("last_in_window"),
                 known_boolean("last_daily_slot"),
                 known_integer("daily_remaining")
             ]},
             "on_unknown": {"kind": "abort_evaluation"}},
            {"gate_id": "by_round",
             "predicate": {"kind": "any", "of": [reads(false, true), reads(true, false)]},
             "on_unknown": {"kind": "abort_evaluation"}}
        ],
        "scoring": [],
        "selection": {"mode": "exactly_one", "required_count": 1},
        "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
    });
    let color = |id: &str, x: u32, expected: [u8; 3]| {
        json!({"type": "color", "id": id,
               "region": {"x": x, "y": 0, "width": 1, "height": 1}, "expected": expected})
    };
    let pack = json!({
        "schema_version": "0.7",
        "game": game,
        "server": server,
        "coordinate_space": space,
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            color("page/home", 0, HOME),
            color("page/result", 0, RESULT),
            color("guard/ready", 1, GREEN),
            color("slot/first", 1, GREEN),
            color("slot/second", 1, BLUE)
        ],
        "candidate_layouts": [layout]
    });
    let pages = json!({"schema_version": "0.3", "pages": [
        {"id": page("home"), "required": ["page/home"], "optional": [], "forbidden": []},
        {"id": page("result"), "required": ["page/result"], "optional": [], "forbidden": []}
    ]});
    let bytes = |value: Value| serde_json::to_vec(&value).expect("package JSON");
    let entries = [
        ("control.json".to_owned(), bytes(control)),
        (
            "resources/manifest.json".to_owned(),
            bytes(json!({"schema_version": "0.3", "entry_task_id": "task"})),
        ),
        (
            "resources/operations/task/task.json".to_owned(),
            bytes(task),
        ),
        (
            "resources/operations/task/policies/by_round.json".to_owned(),
            bytes(policy),
        ),
        (
            format!("resources/recognition/{game}.{server}.pack.json"),
            bytes(pack),
        ),
        (
            format!("resources/recognition/{game}.{server}.pages.json"),
            bytes(pages),
        ),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, content) in entries {
        zip.start_file(path, options).expect("package entry");
        zip.write_all(&content).expect("package content");
    }
    zip.finish().expect("finish package").into_inner()
}

// The device: `home` until the run's tap, then `result`.
fn show_home_until_the_tap(state: &FakeState) {
    *state.pixel_script.lock().expect("pixel script") =
        Some(Arc::new(|_: usize, inputs: usize| {
            vec![(0, if inputs == 0 { HOME } else { RESULT })]
        }));
}

// ---- The policy run.

// The fixture catalog declared as `actingcommand.scheduling.v2` (each timeline event with its
// explicit unbounded validity), with the one task `t_rounds`: an hourly clock, opted in.
fn catalog() -> CatalogSources {
    let mut sources = policy_sources(1);
    for source in [
        &mut sources.tasks,
        &mut sources.pools,
        &mut sources.activity,
        &mut sources.timeline,
    ] {
        let mut document: Value = serde_json::from_slice(&source.bytes).expect("fixture document");
        document["schema_version"] = json!(actingcommand_policy::SCHEDULING_SCHEMA_VERSION_V2);
        if let Some(events) = document.get_mut("events").and_then(Value::as_array_mut) {
            for event in events {
                event["validity"] = json!({"from_unix_ms": 0, "until_unix_ms": null});
            }
        }
        source.bytes = serde_json::to_vec_pretty(&document).expect("V2 fixture bytes");
    }
    let mut tasks: Value = serde_json::from_slice(&sources.tasks.bytes).expect("fixture tasks");
    let mut task = tasks["tasks"][0].clone();
    task["id"] = json!(TASK);
    task["procedure_ref"] = json!(PROCEDURE);
    task["trigger"] = json!({
        "kind": "clock",
        "schedule": {"kind": "interval", "clock_source": {"kind": "local"}, "every_ms": HOUR_MS,
                     "anchor_ms": 0}
    });
    task["dispatch_parameters"] = json!(true);
    tasks["tasks"] = json!([task]);
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("fixture task bytes");
    sources
}

// The fixture's facts without a scheduling outcome (the catalog references none).
fn facts() -> EvaluationFacts {
    let mut facts = policy_facts();
    facts.outcomes.clear();
    facts
}

// A host on the fixture instance with the package written and bound at its location, as actingd
// binds a scheduled procedure, the catalog active and the clock at `at`; evaluates there, admits
// the dispatch of `t_rounds` as the scheduler does (capacity sampled, its approval recorded) and
// runs it: the dispatch's reason chain, the run's select records and its receipt state.
fn policy_run(
    at: u64,
) -> (
    DecisionReasonChain,
    Vec<TaskSelectionRecord>,
    RuntimeReceiptState,
) {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("rounds.zip");
    fs::write(&package, package_bytes(GAME, SERVER)).expect("write the package");
    let state = Arc::new(FakeState::default());
    show_home_until_the_tap(&state);
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let manifest = ProcedureManifest::new([ProcedureBinding::new(
        PROCEDURE,
        "operation.observe",
        vec!["after_observation".to_owned()],
    )
    .expect("procedure binding")
    .with_package_path(package.to_string_lossy())])
    .expect("procedure manifest");
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(clock.clone())
            .with_procedure_manifest(manifest),
        Arc::new(
            FakeProvider::one(POLICY_INSTANCE_ALIAS, instance_id(), Arc::clone(&state))
                .fixture_simulation(),
        ),
    )
    .expect("runtime host");
    host.activate_policy_catalog(&catalog())
        .expect("activate the catalog");

    advance_manual_clock_to(&clock, at);
    let evaluation: PolicyEvaluation = host
        .evaluate_policy_cycle_with_test_inputs(
            &facts(),
            &policy_resources(),
            EvaluationTime {
                unix_ms: at,
                monotonic_ms: at,
            },
            7,
            PolicyTrigger::FactsChanged,
        )
        .expect("evaluate the policy")
        .evaluation
        .expect("a full evaluation");
    let intent = evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.task_id == TASK && intent.instance_id == POLICY_INSTANCE_ALIAS)
        .unwrap_or_else(|| panic!("a dispatch of {TASK}: {evaluation:#?}"))
        .clone();
    let chain = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .expect("the intent's reason chain")
        .clone();
    host.capacity_sampler_for_test().expect("capacity owner")()
        .expect("capacity sample at the admission time");
    record_policy_approval(&host, &intent);
    let request = ContainedTaskRequest::new(package.to_string_lossy().into_owned())
        .expect("contained task request");
    let PolicyDispatchAdmission::Granted { context } = host
        .admit_scheduled_policy_dispatch(&intent, &chain, &policy_context(&host, &intent), &request)
        .expect("policy admission")
    else {
        panic!("expected one policy run context")
    };
    let receipt = host
        .run_scheduled_contained_task(&context, &request)
        .expect("the scheduled run");
    let records = selections(
        &host,
        EventQuery {
            run_id: Some(context.run_id()),
            ..EventQuery::default()
        },
    );
    let receipt_state = receipt.state();
    host.complete_scheduled_policy_run(&context, &receipt)
        .expect("the run settles");
    host.close().expect("close the runtime host");
    (chain, records, receipt_state)
}

// The select records among the records `query` selects, in ledger order.
fn selections(host: &RuntimeHost, query: EventQuery) -> Vec<TaskSelectionRecord> {
    let mut events = host
        .query_persisted_events_for_test(query)
        .expect("the run's records");
    events.sort_by_key(PersistedEvent::sequence);
    events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
                TaskSemanticFact::SelectionEvaluated { selection, .. } => {
                    Some((**selection).clone())
                }
                _ => None,
            },
            _ => None,
        })
        .collect()
}

// ---- Test.

// §2.8 and T2. A policy run of the opted-in task: its dispatch carries
// `policy_dispatch_parameters`, and its select step's snapshot holds the five values as
// `session.dispatch.schedule.*` (the gate that needs all five known passes) and chooses by
// `last_in_window`, false at 12:00 with the next occurrence at 13:00 inside the window: slot 0.
// A direct run of the same select step has no dispatch and holds none of them: the policy's
// `on_unknown` (`abort_evaluation`) decides, and the select record is `unknown` with
// `fact_missing`.
#[test]
fn b8_t2_a_policy_run_reads_the_schedule_values_and_a_direct_run_has_none() {
    let (chain, records, receipt_state) = policy_run(POLICY_NOW_UNIX_MS);
    assert!(
        chain
            .reasons
            .iter()
            .any(|reason| reason.outcome.code().as_str() == PARAMETERS),
        "{chain:?}"
    );
    let [record] = records.as_slice() else {
        panic!("one select record: {records:?}");
    };
    assert_eq!(
        record.selection_result,
        TaskSelectionOutcome::Selected { count: 1 },
        "{record:?}"
    );
    assert_eq!(record.selected, [FIRST], "{record:?}");
    assert_eq!(receipt_state, RuntimeReceiptState::Completed);

    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("rounds.zip");
    fs::write(&package, package_bytes(DIRECT_GAME, DIRECT_SERVER)).expect("write the package");
    let state = Arc::new(FakeState::default());
    // A direct run needs a device instance whose display is observed (Workflow #392 DA-A).
    state.physical_task_geometry.store(true, Ordering::Release);
    *state
        .physical_task_extent
        .lock()
        .expect("fake physical task extent") = Some((2, 1));
    show_home_until_the_tap(&state);
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            DIRECT_ALIAS,
            instance_id(),
            Arc::clone(&state),
        )),
    )
    .expect("runtime host");
    let mut client = TestClient::connect(&host);
    client.set_receipt_read_timeout();
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::run_contained_task(
            DIRECT_ALIAS,
            client.ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package.display().to_string()).expect("task request"),
        ),
    );
    let receipt = client.send(&request);
    let records = selections(
        &host,
        EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        },
    );
    assert!(!records.is_empty(), "a select record: {receipt:?}");
    for record in &records {
        assert_eq!(
            record.selection_result,
            TaskSelectionOutcome::Unknown {
                reason: TaskSelectionUnknownReason::FactMissing
            },
            "{record:?}"
        );
        assert!(record.selected.is_empty(), "{record:?}");
    }
    drop(client);
    host.close().expect("close the runtime host");
}
