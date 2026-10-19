// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 B15, slice R5 (host gate key 114; register §1.115; contracts/remedies.md section
// 4; contracts/instance-fact-store.md, "Run parameters"; model-b15 §6): a run reads its
// parameters as read-only facts. A policy run whose dispatch chain carries
// `policy_dispatch_parameters` holds, in every instance fact snapshot it takes, one record
// `session.dispatch.parameter.<name>` = `<value>` per entry; a run without a dispatch holds the
// package's declared defaults. The records are never stored, nothing may publish under
// `session.dispatch.`, and a select step's `input_sha256` covers the values.
//
// A neutral fixture: frames are 2x1, the first pixel the page (red `home`, white `result`), the
// second green. The provider package provides `cap_x` with the parameter `param_p` (options
// `opt_1` and `opt_2`, its default per test) and has one select step from `home` to `result`
// over two fixed slots, whose feature `first` passes on slot 0 only. Its policy reads
// `session.dispatch.parameter.param_p` as an `enum_string`, admits slot 0 for `opt_1` and slot 1
// for `opt_2`, and aborts the evaluation when the value is unknown. A policy run is the catalog's
// one task `t_param`, due at every evaluation, on the fixture instance. Every expected value is
// the brief's requirement or model-b15's.

use super::*;
use actingcommand_contract::{FactObservation, FactPayload, FactScalar, TaskSelectionRecord};
use actingcommand_policy::PolicyEvaluation;
use serde_json::{Value, json};

// The fixture instance's configured game and server, which a policy run's package declares.
const GAME: &str = "fixture-game-a";
const SERVER: &str = "fixture-server-a";
// A direct run's instance, which no policy input names, and its package's game and server.
const DIRECT_ALIAS: &str = "neutral.instance";
const DIRECT_GAME: &str = "neutral";
const DIRECT_SERVER: &str = "test";
const TASK: &str = "t_param";
const PROCEDURE: &str = "procedure.param";
const PACKAGE_ID: &str = "neutral.test.param";
const PARAMETER_KEY: &str = "session.dispatch.parameter.param_p";
const RESERVED_PREFIX: &str = "session.dispatch.";
const HOME: [u8; 3] = [255, 0, 0];
const RESULT: [u8; 3] = [255, 255, 255];
const GREEN: [u8; 3] = [0, 255, 0];
const BLUE: [u8; 3] = [0, 0, 255];
// The candidate `opt_1` admits and the one `opt_2` admits.
const FIRST: &str = "choices#00";
const SECOND: &str = "choices#01";
const PARAMETERS: &str = "policy_dispatch_parameters";
const RESERVED: &str = "session_dispatch_key_reserved";

// ---- The package.

// The provider package of `game` and `server` as an archive, `param_p`'s default `default`.
fn package_bytes(game: &str, server: &str, default: &str) -> Vec<u8> {
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
        "max_steps": 2,
        "provides": [{"capability": "cap_x", "parameters": [
            {"name": "param_p", "options": ["opt_1", "opt_2"], "default": default}
        ]}]
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
            "select": {"layout_id": "choices", "policy": {"path": "policies/by_parameter.json"}},
            "retryable": false
        }]
    });
    let reads = |option: &str, first: bool| {
        json!({"kind": "all", "of": [
            {"kind": "string_in", "value": {"source": "fact", "fact_key": PARAMETER_KEY},
             "allowed": [option]},
            {"kind": "boolean_equals", "value": {"source": "field", "field": "first"},
             "expected": first}
        ]})
    };
    let policy = json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": "by-parameter",
        "applies_to": {
            "candidate_layout_id": "choices",
            "outcome_keys": {"selected": "slot_selected", "empty": "slot_empty",
                             "insufficient": "slot_insufficient",
                             "ambiguous": "slot_ambiguous", "unknown": "slot_unknown"}
        },
        "fields": [{"name": "first", "value_type": {"type": "boolean"}}],
        "facts": [{"fact_key": PARAMETER_KEY,
                   "value_type": {"type": "enum_string", "allowed": ["opt_1", "opt_2"]},
                   "max_age_ms": 86_400_000, "minimum_confidence_milli": 1000}],
        "gates": [{"gate_id": "by_parameter",
                   "predicate": {"kind": "any", "of": [reads("opt_1", true), reads("opt_2", false)]},
                   "on_unknown": {"kind": "abort_evaluation"}}],
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
            "resources/operations/task/policies/by_parameter.json".to_owned(),
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

// ---- Policy runs.

// The fixture catalog with the one task `t_param`, due at every evaluation.
fn catalog() -> CatalogSources {
    let mut sources = policy_sources(1);
    let mut tasks: Value = serde_json::from_slice(&sources.tasks.bytes).expect("fixture tasks");
    let mut task = tasks["tasks"][0].clone();
    task["id"] = json!(TASK);
    task["procedure_ref"] = json!(PROCEDURE);
    task["trigger"] = json!({
        "kind": "clock",
        "schedule": {"kind": "interval", "clock_source": {"kind": "local"}, "every_ms": 1,
                     "anchor_ms": 0}
    });
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

struct Fixture {
    _root: TempDir,
    package: PathBuf,
    clock: Arc<ManualRuntimeClock>,
    host: RuntimeHost,
}

impl Fixture {
    // A host on `instance` with the package (`param_p`'s default `default`) written and bound at
    // its location, as actingd binds a scheduled procedure, the catalog active and the clock at
    // the fixture's time.
    fn new(default: &str, instance: InstanceId) -> Self {
        let root = TempDir::new().expect("tempdir");
        let package = root.path().join("param.zip");
        fs::write(&package, package_bytes(GAME, SERVER, default)).expect("write the package");
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
                FakeProvider::one(POLICY_INSTANCE_ALIAS, instance, Arc::clone(&state))
                    .fixture_simulation(),
            ),
        )
        .expect("runtime host");
        host.activate_policy_catalog(&catalog())
            .expect("activate the catalog");
        Self {
            _root: root,
            package,
            clock,
            host,
        }
    }

    // One full evaluation at `at`, the Runtime clock moved there first.
    fn evaluate(&self, at: u64, seed: u64) -> PolicyEvaluation {
        advance_manual_clock_to(&self.clock, at);
        self.host
            .evaluate_policy_cycle_with_test_inputs(
                &facts(),
                &policy_resources(),
                EvaluationTime {
                    unix_ms: at,
                    monotonic_ms: at,
                },
                seed,
                PolicyTrigger::FactsChanged,
            )
            .expect("evaluate the policy")
            .evaluation
            .expect("a full evaluation")
    }

    // Evaluates at `at`, admits the dispatch of `t_param` as the scheduler does (capacity sampled,
    // its approval recorded), runs it and settles it: its reason chain and its run.
    fn run(&self, at: u64, seed: u64) -> (DecisionReasonChain, Box<PolicyRunContext>) {
        let evaluation = self.evaluate(at, seed);
        let (intent, chain) = dispatched(&evaluation)
            .unwrap_or_else(|| panic!("a dispatch of {TASK}: {evaluation:#?}"));
        self.host
            .capacity_sampler_for_test()
            .expect("capacity owner")()
        .expect("capacity sample at the admission time");
        record_policy_approval(&self.host, &intent);
        let request = ContainedTaskRequest::new(self.package.to_string_lossy().into_owned())
            .expect("contained task request");
        let PolicyDispatchAdmission::Granted { context } = self
            .host
            .admit_scheduled_policy_dispatch(
                &intent,
                &chain,
                &policy_context(&self.host, &intent),
                &request,
            )
            .expect("policy admission")
        else {
            panic!("expected one policy run context")
        };
        let receipt = self
            .host
            .run_scheduled_contained_task(&context, &request)
            .expect("the scheduled run");
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "{receipt:?}"
        );
        self.host
            .complete_scheduled_policy_run(&context, &receipt)
            .expect("the run settles");
        (chain, context)
    }

    // The one select record of the run `run_id`.
    fn selection(&self, run_id: RunId) -> TaskSelectionRecord {
        let records = selections(
            &self.host,
            EventQuery {
                run_id: Some(run_id),
                ..EventQuery::default()
            },
        );
        let [record] = records.as_slice() else {
            panic!("one select record: {records:?}");
        };
        record.clone()
    }
}

// The evaluation's dispatch of `t_param` with its reason chain, if it has one.
fn dispatched(evaluation: &PolicyEvaluation) -> Option<(DispatchIntent, DecisionReasonChain)> {
    let intent = evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.task_id == TASK && intent.instance_id == POLICY_INSTANCE_ALIAS)?
        .clone();
    let chain = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .expect("the intent's reason chain")
        .clone();
    Some((intent, chain))
}

// The `run_parameters` of the chain's `policy_dispatch_parameters`, if it carries one.
fn run_parameters(chain: &DecisionReasonChain) -> Option<Value> {
    chain
        .reasons
        .iter()
        .find(|reason| reason.outcome.code().as_str() == PARAMETERS)
        .and_then(|reason| reason.outcome.value("run_parameters").cloned())
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

// An agent's target record for the fixture instance that chooses `opt_2` for `param_p` of
// `cap_x`, observed at `observed_at_unix_ms`.
fn target_record(observed_at_unix_ms: u64) -> FactRecord {
    let text = |value: &str| FactScalar::String(value.to_owned());
    FactRecord {
        scope: FactScope::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        },
        key: "session.remedy.choices".to_owned(),
        content: FactContent::Inline {
            value: ContractFactValue::RecordList(vec![BTreeMap::from([
                ("row".to_owned(), text("parameter")),
                ("capability".to_owned(), text("cap_x")),
                ("parameter".to_owned(), text("param_p")),
                ("value".to_owned(), text("opt_2")),
            ])]),
        },
        observed_at_unix_ms,
        expires_at_unix_ms: None,
        ttl_policy: None,
        confidence_milli: 1_000,
        source_detector: "agent.fixture".to_owned(),
        source_snapshot_id: format!("remedy-choices:{:x}", Sha256::digest(b"b15r target record")),
        schema_version: "fact.v1".to_owned(),
        resource_bundle_hash: "a".repeat(64),
        invalidate_on: Vec::new(),
    }
}

// ---- Tests.

// Requirement 1: with the target record's value `opt_2` and the package default `opt_1`, the
// policy dispatch of `t_param` carries `opt_2` in `policy_dispatch_parameters`, and the run's
// select step reads it as `session.dispatch.parameter.param_p`: the `opt_2` candidate is chosen.
#[test]
fn b15r_a_policy_run_selects_by_the_value_its_dispatch_carries() {
    let fixture = Fixture::new("opt_1", instance_id());
    // The first evaluation under the catalog reads the bound package's declarations.
    fixture.evaluate(POLICY_NOW_UNIX_MS, 7);
    let written_at = POLICY_NOW_UNIX_MS + 1_000;
    advance_manual_clock_to(&fixture.clock, written_at);
    let mut client = TestClient::connect(&fixture.host);
    let publish = client.agent_request(RuntimeOperation::PublishFact {
        record: target_record(written_at),
    });
    let receipt = client.send(&publish);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );

    let (chain, context) = fixture.run(POLICY_NOW_UNIX_MS + 2_000, 8);
    assert_eq!(
        run_parameters(&chain),
        Some(
            json!([{"parameter_name": "param_p", "parameter_value": "opt_2",
                     "remedy_source": "target"}])
        ),
        "{chain:?}"
    );
    let selection = fixture.selection(context.run_id());
    assert_eq!(selection.selected, [SECOND], "{selection:?}");
    drop(client);
    fixture.host.close().expect("close the runtime host");
}

// Requirement 2: a direct run has no dispatch; its select step reads the package's declared
// default (`opt_2` here), and the `opt_2` candidate is chosen.
#[test]
fn b15r_a_direct_run_selects_by_the_declared_default() {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("param.zip");
    fs::write(&package, package_bytes(DIRECT_GAME, DIRECT_SERVER, "opt_2"))
        .expect("write the package");
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
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "the run's outcome: {:?}",
        receipt.outcome()
    );
    let records = selections(
        &host,
        EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        },
    );
    let [record] = records.as_slice() else {
        panic!("one select record: {records:?}");
    };
    assert_eq!(record.selected, [SECOND], "{record:?}");
    drop(client);
    host.close().expect("close the runtime host");
}

// Requirement 3: the overlay is never stored. After a policy run whose select step chose by its
// parameter, no `fact.published` record has a key under `session.dispatch.`, and the instance's
// fact snapshot holds none.
#[test]
fn b15r_the_overlay_writes_no_fact_record() {
    let fixture = Fixture::new("opt_1", instance_id());
    let (chain, context) = fixture.run(POLICY_NOW_UNIX_MS, 7);
    assert!(run_parameters(&chain).is_some(), "{chain:?}");
    assert_eq!(fixture.selection(context.run_id()).selected, [FIRST]);

    let published = fixture
        .host
        .query_persisted_events_for_test(EventQuery {
            event_type: Some(EventType::FactPublished),
            ..EventQuery::default()
        })
        .expect("the published facts");
    let keys = published
        .iter()
        .flat_map(|event| match event.payload() {
            EventPayload::Fact(FactPayload::Published(payload)) => payload
                .records()
                .map(|record| record.key.clone())
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        })
        .collect::<Vec<_>>();
    assert!(
        keys.iter().all(|key| !key.starts_with(RESERVED_PREFIX)),
        "{keys:?}"
    );
    let snapshot = fixture
        .host
        .instance_fact_snapshot(InstanceFactContext {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
            server_id: SERVER.to_owned(),
            game_id: GAME.to_owned(),
        })
        .expect("the instance's facts");
    assert!(
        snapshot
            .records
            .iter()
            .all(|record| !record.key.starts_with(RESERVED_PREFIX)),
        "{:?}",
        snapshot.records
    );
    fixture.host.close().expect("close the runtime host");
}

// Requirement 4: a publication of a key under `session.dispatch.` is refused
// `session_dispatch_key_reserved` with the key, a non-fatal request error with nothing appended:
// an agent's PublishFact, an agent's PublishFacts whose other record is an ordinary one, and the
// Runtime's own publication path.
#[test]
fn b15r_publishing_a_dispatch_key_is_refused() {
    let root = TempDir::new().expect("tempdir");
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    let published = |host: &RuntimeHost| {
        host.query_persisted_events_for_test(EventQuery {
            event_type: Some(EventType::FactPublished),
            ..EventQuery::default()
        })
        .expect("the published facts")
        .len()
    };
    let before = published(&host);
    let instance = FactScope::Instance {
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
    };
    let parameter = stored_fact(
        instance.clone(),
        PARAMETER_KEY,
        ContractFactValue::String("opt_2".to_owned()),
        "snapshot:b15r-parameter",
        Vec::new(),
    );
    // The batch is one consistent observation (one source snapshot), so the request is valid
    // and reaches the reserved-prefix rule.
    let other = stored_fact(
        instance.clone(),
        "session.dispatch.other",
        ContractFactValue::String("opt_1".to_owned()),
        "snapshot:b15r-batch",
        Vec::new(),
    );
    let ordinary = stored_fact(
        instance,
        "session.example.flag",
        ContractFactValue::Boolean(true),
        "snapshot:b15r-batch",
        Vec::new(),
    );

    let mut client = TestClient::connect(&host);
    let requests = [
        (
            client.agent_request(RuntimeOperation::PublishFact {
                record: parameter.clone(),
            }),
            PARAMETER_KEY,
        ),
        (
            client.agent_request(RuntimeOperation::PublishFacts {
                observation: FactObservation {
                    records: vec![ordinary, other],
                },
            }),
            "session.dispatch.other",
        ),
    ];
    for (request, key) in requests {
        let receipt = client.send(&request);
        assert_eq!(receipt.state(), RuntimeReceiptState::Denied, "{receipt:?}");
        assert_eq!(receipt.outcome().code().as_str(), RESERVED, "{receipt:?}");
        assert_eq!(
            receipt.outcome().value("fact_key"),
            Some(&json!(key)),
            "{receipt:?}"
        );
    }
    let error = host
        .publish_fact(parameter)
        .expect_err("a run's parameter is never stored");
    assert_eq!(error.code(), RESERVED, "{error:?}");
    assert!(!error.is_fatal(), "{error:?}");

    assert_eq!(published(&host), before, "nothing is appended");
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(client);
    host.close().expect("close the runtime host");
}

// Requirement 5: two policy runs that differ only in the value of `param_p` (each the package
// default of its own host, everything else equal: the stored snapshot, the instant and the
// candidates) choose different candidates and record different `input_sha256`.
#[test]
fn b15r_the_selection_input_hash_differs_between_two_values() {
    let instance = instance_id();
    let selection_with = |default: &str| {
        let fixture = Fixture::new(default, instance);
        let (chain, context) = fixture.run(POLICY_NOW_UNIX_MS, 7);
        assert!(run_parameters(&chain).is_some(), "{chain:?}");
        let selection = fixture.selection(context.run_id());
        fixture.host.close().expect("close the runtime host");
        selection
    };
    let first = selection_with("opt_1");
    let second = selection_with("opt_2");
    assert_eq!(first.fact_snapshot_id, second.fact_snapshot_id);
    assert_eq!(first.now_unix_ms, second.now_unix_ms);
    assert_eq!(
        first.projection.candidate_set_sha256(),
        second.projection.candidate_set_sha256()
    );
    assert_eq!(first.selected, [FIRST], "{first:?}");
    assert_eq!(second.selected, [SECOND], "{second:?}");
    assert_ne!(first.input_sha256, second.input_sha256);
}
