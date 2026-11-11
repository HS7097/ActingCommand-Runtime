// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #429 R-8 (register §1.308; contracts/instance-fact-store.md, "Pack schedule facts";
// contracts/runtime-interface.md, "Workbench request kinds", the rows EditPackSchedule and
// PackScheduleView; contracts/scheduling-blackout.md, "Order of the deferrals"; coordinator work
// order rd-1111/brief-r8.md): a task package switched on or off for one instance. Through the
// host, on a manual clock and a fake device:
// - the three counters may write `session.pack.schedule`, each keeping its origin on the
//   `fact.published` event, and an observation that mixes the key with another family's is
//   refused `fact_origin_mixed`;
// - every refusal of a list or of an edit is `session_pack_schedule_invalid` with `fact_key` and
//   `field`, and nothing is appended;
// - an instance record that names only one package leaves another to the server and game layers;
// - `EditPackSchedule.expected_version` compares the scope's list version (0: no record yet);
// - a closed package's candidates are deferred `pack_schedule_closed` after an active blackout's
//   deferral, waking at the covering row's end; an open-ended closed row names no wake;
// - a record written after an evaluation refuses its admission as `policy_facts_stale`;
// - without a record nothing of the pack schedule enters an evaluation;
// - `PackScheduleView` lists every package of the instance's tasks with its source, row, next
//   change and layer below, and the scopes' versions; an unknown package's row is not listed and
//   is reported once per cycle;
// - "close this task": a closing row, then `StopInstanceRun` stops a client's direct run of the
//   package and refuses a scheduler's run, whose package the next cycle does not select.
// Every alias, package and id is neutral.

use super::gate_369s5::direct_run_package;
use super::gate_383_ao4q::ledger_head;
use super::*;
use crate::host::ContainedTaskCheckpointPoint;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    ConfigParameter, ConfigParameterSource, ContainedTaskCancellationReason, EventId,
    FactKeyFamily, FactObservation, FactPayload, FactScalar, InstanceRunReset, InstanceRunStop,
    PACK_SCHEDULE_FACT_KEY, PackScheduleBelow, PackScheduleEditRow, PackScheduleEdited,
    PackSchedulePackage, PackScheduleScopeVersion, PackScheduleSource, PackScheduleView, RequestId,
    RuntimeConfigManifest, fact_key_family,
};
use actingcommand_policy::{PolicyEvaluation, SchedulingDecisionState, TaskDecision};
use actingcommand_runtime_config::scheduling::BLACKOUT_HOST;
use serde_json::{Value, json};
use std::net::SocketAddr;

const ALIAS: &str = POLICY_INSTANCE_ALIAS;
const OTHER_ALIAS: &str = POLICY_INSTANCE_ALIAS_B;
const SERVER: &str = "fixture-server-a";
const GAME: &str = "fixture-game-a";
const PACK_A: &str = "neutral.pack.a";
const PACK_B: &str = "neutral.pack.b";
const PACK_C: &str = "neutral.pack.c";
const UNKNOWN: &str = "neutral.pack.unknown";
const TASK_A: &str = "t_alpha";
const TASK_B: &str = "t_beta";
const TASK_C: &str = "t_gamma";
const START: u64 = POLICY_NOW_UNIX_MS;
const MINUTE_MS: u64 = 60_000;
const HOUR_MS: u64 = 60 * MINUTE_MS;
const SEED: u64 = 29;
const INVALID: &str = "session_pack_schedule_invalid";
const CLOSED: &str = "pack_schedule_closed";
const UNKNOWN_PACKAGE: &str = "pack_schedule_unknown_package";
/// How long a test waits for something the host does on another thread.
const GATE_WAIT: Duration = Duration::from_secs(30);

// ---- Packages, catalog and host.

/// A package directory with its control declaration (`package_id`), its resource list and one
/// task, as `crate::remedy::build_table` reads a bound package.
fn write_package(root: &TempDir, directory: &str, package_id: &str) -> PathBuf {
    let path = root.path().join(directory);
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": package_id,
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 16, "height": 9},
        "entry_task_id": "task",
        "capture_interval_ms": 10,
        "step_timeout_ms": 600,
        "timeout_ms": 8000,
        "max_steps": 2
    });
    let probe = |page: &str, expected: [u8; 3]| {
        json!({
            "id": format!("{page}_colour"),
            "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
            "expected": expected
        })
    };
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "locale": "en-US",
        "goal": "R-8 fixture",
        "coordinate_space": {"width": 16, "height": 9},
        "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
        "color_probes": [probe("home", [0, 0, 255]), probe("page_b", [255, 0, 0])],
        "page_rules": {
            "home": {"required": ["home_colour"]},
            "page_b": {"required": ["page_b_colour"]}
        },
        "entry_page": "home",
        "target_page": "page_b",
        "operations": [{
            "id": "go",
            "from": "home",
            "to": "page_b",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true,
            "retryable": false
        }]
    });
    for (file, value) in [
        ("control.json", control),
        (
            "resources/operations/resources.json",
            json!({"schema_version": "1.0", "resources": [], "resource_count": 0}),
        ),
        ("resources/operations/task/task.json", task),
    ] {
        let file = path.join(file);
        fs::create_dir_all(file.parent().expect("a package file's directory"))
            .expect("package directory");
        fs::write(
            file,
            serde_json::to_vec_pretty(&value).expect("package JSON"),
        )
        .expect("package file");
    }
    path
}

fn procedure(task_id: &str) -> String {
    format!("procedure.{task_id}")
}

/// The fixture catalog with one task per `(task id, instance)`, each due at every evaluation and
/// bound to its own procedure, in the order given (the first ranks highest).
fn catalog(tasks: &[(&str, &str)]) -> CatalogSources {
    let mut sources = policy_sources(1);
    let mut document: Value = serde_json::from_slice(&sources.tasks.bytes).expect("fixture tasks");
    let base = document["tasks"][0].clone();
    let count = tasks.len();
    document["tasks"] = tasks
        .iter()
        .enumerate()
        .map(|(index, (task_id, alias))| {
            let mut task = base.clone();
            task["id"] = json!(task_id);
            task["procedure_ref"] = json!(procedure(task_id));
            task["scope"] = json!({"kind": "instance", "instance_id": alias});
            task["priority"] = json!(100 * (count - index));
            task["trigger"] = json!({
                "kind": "clock",
                "schedule": {"kind": "interval", "clock_source": {"kind": "local"},
                             "every_ms": 1, "anchor_ms": 0}
            });
            task["cooldown_ms"] = json!(0);
            task["instance_overrides"] = json!([]);
            task["produces"] = json!([]);
            task
        })
        .collect::<Vec<_>>()
        .into();
    sources.tasks.bytes = serde_json::to_vec_pretty(&document).expect("fixture task bytes");
    sources
}

/// Each task's procedure, bound at its package's location (`None`: bound with no location), as
/// actingd binds a scheduled procedure.
fn manifest(tasks: &[(&str, Option<&Path>)]) -> ProcedureManifest {
    ProcedureManifest::new(tasks.iter().map(|(task_id, path)| {
        let binding = ProcedureBinding::new(
            procedure(task_id),
            "operation.observe",
            vec!["after_observation".to_owned()],
        )
        .expect("procedure binding");
        match path {
            Some(path) => binding.with_package_path(path.to_string_lossy()),
            None => binding,
        }
    }))
    .expect("procedure manifest")
}

/// The configuration whose one setting is the host blackout `[from, until)`.
fn host_blackout(from: u64, until: u64) -> RuntimeConfigManifest {
    let entries = json!([{"kind": "once", "id": "neutral.quiet", "from_unix_ms": from,
                          "until_unix_ms": until}]);
    let mut parameters = vec![ConfigParameter {
        key: BLACKOUT_HOST.to_owned(),
        value: FactScalar::String(entries.to_string()),
        source: ConfigParameterSource::Explicit,
        settable: false,
    }];
    actingcommand_runtime_config::annotate_settable(&mut parameters)
        .expect("the blackout key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

/// The fixture's facts without a scheduling outcome (the catalog references none).
fn facts() -> EvaluationFacts {
    let mut facts = policy_facts();
    facts.outcomes.clear();
    facts
}

struct Fixture {
    _root: TempDir,
    clock: Arc<ManualRuntimeClock>,
    host: RuntimeHost,
    instance: InstanceId,
}

impl Fixture {
    /// A host on the fixture instance (`state` its device) with the procedures of `manifest`, the
    /// catalog of `tasks` active and the clock at the fixture's time; `config` its settings.
    fn start(
        root: TempDir,
        state: Arc<FakeState>,
        manifest: ProcedureManifest,
        tasks: &[(&str, &str)],
        config_manifest: Option<RuntimeConfigManifest>,
    ) -> Self {
        let clock = Arc::new(ManualRuntimeClock::new(START, 0));
        let instance = instance_id();
        let mut host_config = config(&root)
            .with_runtime_clock(clock.clone())
            .with_procedure_manifest(manifest);
        if let Some(config_manifest) = config_manifest {
            host_config = host_config.with_config_manifest(config_manifest);
        }
        let host = RuntimeHost::start(
            host_config,
            Arc::new(FakeProvider::one(ALIAS, instance, state)),
        )
        .expect("runtime host");
        host.activate_policy_catalog(&catalog(tasks))
            .expect("activate the catalog");
        Self {
            _root: root,
            clock,
            host,
            instance,
        }
    }

    /// The host of `tasks`, each bound to a package directory of its own package id.
    fn with_packages(tasks: &[(&str, &str, &str)], config: Option<RuntimeConfigManifest>) -> Self {
        let root = TempDir::new().expect("tempdir");
        let paths = tasks
            .iter()
            .map(|(task_id, _, package_id)| write_package(&root, task_id, package_id))
            .collect::<Vec<_>>();
        let bound = tasks
            .iter()
            .zip(&paths)
            .map(|((task_id, _, _), path)| (*task_id, Some(path.as_path())))
            .collect::<Vec<_>>();
        let catalog_tasks = tasks
            .iter()
            .map(|(task_id, alias, _)| (*task_id, *alias))
            .collect::<Vec<_>>();
        Self::start(
            root,
            Arc::new(FakeState::default()),
            manifest(&bound),
            &catalog_tasks,
            config,
        )
    }

    /// Moves the Runtime clock (both clocks) to `at` and samples the host's capacity there.
    fn set_time(&self, at: u64) {
        advance_manual_clock_to(&self.clock, at);
        let sample_capacity = self
            .host
            .capacity_sampler_for_test()
            .expect("capacity owner");
        sample_capacity().expect("capacity sample");
    }

    /// One full evaluation at `at`.
    fn evaluate(&self, at: u64) -> PolicyEvaluation {
        self.set_time(at);
        self.host
            .evaluate_policy_cycle_with_test_inputs(
                &facts(),
                &policy_resources(),
                EvaluationTime {
                    unix_ms: at,
                    monotonic_ms: at,
                },
                SEED,
                PolicyTrigger::FactsChanged,
            )
            .unwrap_or_else(|error| panic!("evaluate at {at}: {error}"))
            .evaluation
            .expect("a full evaluation")
    }

    /// `EditPackSchedule` from the operator, its receipt. The clock moves one millisecond first,
    /// so two edits of one scope are two observations, each strictly newer.
    fn edit(
        &self,
        scope: FactScope,
        package_id: &str,
        rows: Vec<PackScheduleEditRow>,
        expected_version: Option<u64>,
    ) -> RuntimeReceipt {
        self.clock.advance(1);
        let mut client = TestClient::connect(&self.host);
        let request = client.request(RuntimeOperation::EditPackSchedule {
            scope,
            package_id: package_id.to_owned(),
            rows,
            expected_version,
        });
        client.send(&request)
    }

    /// The view of `alias`.
    fn view(&self, alias: &str) -> PackScheduleView {
        let mut client = TestClient::connect(&self.host);
        let request = client.request(RuntimeOperation::PackScheduleView {
            instance_alias: alias.to_owned(),
        });
        let receipt = client.send(&request);
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "{receipt:?}"
        );
        assert_eq!(
            receipt.outcome().code().as_str(),
            "request_pack_schedule_view_completed"
        );
        assert!(receipt.terminal().is_none(), "a read records nothing");
        let Some(RuntimeResult::PackScheduleViewed { view }) = receipt.result() else {
            panic!("a pack schedule view: {receipt:?}");
        };
        (**view).clone()
    }

    /// Every `fact.published` event of the key, in ledger order, with its record.
    fn schedule_events(&self) -> Vec<(PersistedEvent, FactRecord)> {
        let mut events = self
            .host
            .query_persisted_events_for_test(EventQuery {
                event_type: Some(EventType::FactPublished),
                ..EventQuery::default()
            })
            .expect("ledger records")
            .into_iter()
            .filter_map(|event| {
                let record = match event.payload() {
                    EventPayload::Fact(FactPayload::Published(payload))
                        if payload.record().key == PACK_SCHEDULE_FACT_KEY =>
                    {
                        payload.record().clone()
                    }
                    _ => return None,
                };
                Some((event, record))
            })
            .collect::<Vec<_>>();
        events.sort_by_key(|(event, _)| event.sequence());
        events
    }

    fn head(&self) -> u64 {
        ledger_head(&mut TestClient::connect(&self.host))
    }

    fn close(self) {
        assert!(self.host.fatal_error().expect("runtime health").is_none());
        self.host.close().expect("close host");
    }
}

// ---- Rows, records and answers.

fn row(from: u64, until: Option<u64>, schedulable: bool) -> PackScheduleEditRow {
    PackScheduleEditRow {
        from_unix_ms: from,
        until_unix_ms: until,
        schedulable,
    }
}

fn instance_scope() -> FactScope {
    FactScope::Instance {
        instance_id: ALIAS.to_owned(),
    }
}

fn server_scope() -> FactScope {
    FactScope::Server {
        server_id: SERVER.to_owned(),
    }
}

fn game_scope() -> FactScope {
    FactScope::Game {
        game_id: GAME.to_owned(),
    }
}

/// One stored row as a published list holds it.
fn stored_row(
    package_id: &str,
    from: u64,
    until: Option<u64>,
    schedulable: bool,
) -> BTreeMap<String, FactScalar> {
    let mut row = BTreeMap::from([
        (
            "package_id".to_owned(),
            FactScalar::String(package_id.to_owned()),
        ),
        ("from_unix_ms".to_owned(), FactScalar::TimestampMs(from)),
        ("schedulable".to_owned(), FactScalar::Boolean(schedulable)),
    ]);
    if let Some(until) = until {
        row.insert("until_unix_ms".to_owned(), FactScalar::TimestampMs(until));
    }
    row
}

/// A published list of `scope` with `rows`, observed at `observed_at_unix_ms`.
fn schedule_record(
    scope: FactScope,
    rows: Vec<BTreeMap<String, FactScalar>>,
    observed_at_unix_ms: u64,
) -> FactRecord {
    let identity = serde_json::to_vec(&(&scope, &rows, observed_at_unix_ms)).expect("identity");
    FactRecord {
        scope,
        key: PACK_SCHEDULE_FACT_KEY.to_owned(),
        content: FactContent::Inline {
            value: ContractFactValue::RecordList(rows),
        },
        observed_at_unix_ms,
        expires_at_unix_ms: None,
        ttl_policy: None,
        confidence_milli: 1_000,
        source_detector: "neutral.test.writer".to_owned(),
        source_snapshot_id: format!("snapshot:pack-schedule:{:x}", Sha256::digest(identity)),
        schema_version: "fact.v1".to_owned(),
        resource_bundle_hash: "b".repeat(64),
        invalidate_on: Vec::new(),
    }
}

fn publish(record: FactRecord) -> RuntimeOperation {
    RuntimeOperation::PublishFacts {
        observation: FactObservation {
            records: vec![record],
        },
    }
}

/// The edit a completed receipt answers, its code the kind's success code and its terminal the
/// event that holds the list.
#[track_caller]
fn edited(receipt: &RuntimeReceipt) -> PackScheduleEdited {
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "the edit is accepted: {receipt:?}"
    );
    assert_eq!(
        receipt.outcome().code().as_str(),
        "request_edit_pack_schedule_completed"
    );
    let Some(RuntimeResult::PackScheduleEdited { edited }) = receipt.result() else {
        panic!("an edit: {receipt:?}");
    };
    let terminal = receipt.terminal().expect("the edit's terminal");
    assert_eq!(
        (terminal.sequence, terminal.event_id),
        (edited.version, edited.event_id)
    );
    *edited
}

/// A refusal as `session_pack_schedule_invalid`: Denied, no result, no terminal, the key and
/// the `field` it names.
#[track_caller]
fn refused_at(receipt: &RuntimeReceipt, field: &str) {
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied, "{receipt:?}");
    assert_eq!(receipt.outcome().code().as_str(), INVALID, "{receipt:?}");
    assert_eq!(
        receipt.outcome().value("fact_key"),
        Some(&json!(PACK_SCHEDULE_FACT_KEY)),
        "{receipt:?}"
    );
    assert_eq!(
        receipt.outcome().value("field"),
        Some(&json!(field)),
        "{receipt:?}"
    );
    assert!(receipt.result().is_none(), "{receipt:?}");
    assert!(receipt.terminal().is_none(), "{receipt:?}");
}

fn decision<'a>(evaluation: &'a PolicyEvaluation, task_id: &str) -> &'a TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|decision| {
            decision.task_id == task_id && decision.instance_id.as_deref() == Some(ALIAS)
        })
        .unwrap_or_else(|| panic!("a decision of {task_id}: {evaluation:#?}"))
}

fn dispatched(
    evaluation: &PolicyEvaluation,
    task_id: &str,
) -> Option<(DispatchIntent, DecisionReasonChain)> {
    let intent = evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.task_id == task_id && intent.instance_id == ALIAS)?
        .clone();
    let chain = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .expect("the intent's reason chain")
        .clone();
    Some((intent, chain))
}

/// The one reason that defers `task_id`, which must be `code`, and its wake; the pair is not
/// dispatched.
#[track_caller]
fn deferred_by(evaluation: &PolicyEvaluation, task_id: &str, code: &str) -> (Outcome, Option<u64>) {
    let decision = decision(evaluation, task_id);
    assert_eq!(
        decision.state,
        SchedulingDecisionState::Deferred,
        "{decision:#?}"
    );
    let reason = decision
        .reasons
        .iter()
        .find(|reason| reason.outcome.code().as_str() == code)
        .unwrap_or_else(|| panic!("{task_id} is deferred {code}: {decision:#?}"));
    assert!(
        dispatched(evaluation, task_id).is_none(),
        "a deferred pair is not dispatched: {evaluation:#?}"
    );
    (reason.outcome.clone(), decision.next_wake_unix_ms)
}

/// Whether any reason of the evaluation names one of the pack schedule's codes, or any decision
/// is the pack schedule's report.
fn names_the_pack_schedule(evaluation: &PolicyEvaluation) -> bool {
    evaluation.decisions.iter().any(|decision| {
        decision.task_id == PACK_SCHEDULE_FACT_KEY
            || decision.reasons.iter().any(|reason| {
                matches!(
                    reason.outcome.code().as_str(),
                    CLOSED | UNKNOWN_PACKAGE | INVALID
                )
            })
    })
}

// ---- Families and origins.

// The key is its own family with the origin exception: the operator (cli, cli), the person
// (user, ui) and the agent (agent, adapter) each publish a valid list as one `fact.published` of
// the key that keeps the writer's origin and the record as written. An observation that mixes the
// key with another family's key (instance information, a priority offset) is refused
// `fact_origin_mixed`, by the contract when built and by the host when it arrives, and nothing is
// appended.
#[test]
fn r8_the_three_origins_may_write_and_a_mixed_observation_is_refused() {
    assert_eq!(
        fact_key_family(PACK_SCHEDULE_FACT_KEY),
        FactKeyFamily::PackSchedule
    );
    assert!(FactKeyFamily::PackSchedule.takes_origin_exception());
    let fixture = Fixture::with_packages(&[(TASK_A, ALIAS, PACK_A)], None);
    let mut client = TestClient::connect(&fixture.host);
    type Writer = fn(&TestClient, RuntimeOperation) -> RuntimeRequest;
    let writers: [(Writer, EventActor, EventSource); 3] = [
        (TestClient::request, EventActor::Cli, EventSource::Cli),
        (
            TestClient::governance_request,
            EventActor::User,
            EventSource::Ui,
        ),
        (
            TestClient::agent_request,
            EventActor::Agent,
            EventSource::Adapter,
        ),
    ];
    for (offset, (writer, actor, source)) in (1_u64..).zip(writers) {
        let at = START + offset * MINUTE_MS;
        fixture.set_time(at);
        let record = schedule_record(
            instance_scope(),
            vec![stored_row(PACK_A, at, None, false)],
            at,
        );
        let receipt = client.send(&writer(&client, publish(record.clone())));
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "the {actor:?} write is accepted: {receipt:?}"
        );
        let Some(RuntimeResult::FactPublished { event_id }) = receipt.result() else {
            panic!("a fact publication: {receipt:?}");
        };
        let events = fixture.schedule_events();
        let (event, written) = events
            .iter()
            .find(|(event, _)| event.event_id() == event_id)
            .unwrap_or_else(|| panic!("the published event of the {actor:?} write"));
        assert_eq!(
            (event.origin().source(), event.origin().actor()),
            (source, actor),
            "the event keeps the writer's origin"
        );
        assert_eq!(
            (
                &written.scope,
                &written.content,
                written.observed_at_unix_ms,
                &written.source_snapshot_id
            ),
            (
                &record.scope,
                &record.content,
                record.observed_at_unix_ms,
                &record.source_snapshot_id
            ),
            "the record as written"
        );
    }
    assert_eq!(fixture.schedule_events().len(), 3);

    let schedule = schedule_record(instance_scope(), Vec::new(), START);
    let mut information = schedule.clone();
    information.key = "display.neutral_label".to_owned();
    information.content = FactContent::Inline {
        value: ContractFactValue::String("neutral".to_owned()),
    };
    let mut offset = schedule.clone();
    offset.key = actingcommand_contract::priority_offset_fact_key(TASK_A);
    offset.content = FactContent::Inline {
        value: ContractFactValue::Integer(250),
    };
    let head = fixture.head();
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    for other in [&information, &offset] {
        for (actor, source) in [
            (EventActor::Agent, EventSource::Adapter),
            (EventActor::Cli, EventSource::Cli),
            (EventActor::User, EventSource::Ui),
        ] {
            let built = RuntimeRequest::new(
                ids.mint_request_id().expect("request id"),
                ids.mint_correlation_id().expect("correlation id"),
                None,
                actor,
                source,
                START,
                RuntimeOperation::PublishFacts {
                    observation: FactObservation {
                        records: vec![schedule.clone(), other.clone()],
                    },
                },
            );
            let Err(error) = built else {
                panic!("a mix with {} from {actor:?} was built", other.key);
            };
            assert_eq!(error.code().as_str(), "fact_origin_mixed");
        }
        // The same mix as it would arrive on the wire.
        let request = client.agent_request(publish(schedule.clone()));
        let mut wire = serde_json::to_value(&request).expect("request JSON");
        wire["operation"]["observation"]["records"]
            .as_array_mut()
            .expect("the observation's records")
            .push(serde_json::to_value(other).expect("record JSON"));
        let mixed: RuntimeRequest = serde_json::from_value(wire).expect("the request's shape");
        let receipt = client.send(&mixed);
        assert_eq!(receipt.state(), RuntimeReceiptState::Denied, "{receipt:?}");
        assert_eq!(receipt.outcome().code().as_str(), "fact_origin_mixed");
    }
    assert_eq!(fixture.head(), head, "nothing is appended");
    drop(client);
    fixture.close();
}

// ---- The rows' rule.

// Each rule refused alone, as `session_pack_schedule_invalid` (non-fatal) with `fact_key` and
// `field`, and nothing appended. A published list: a value that is not a record list (`""`), a
// row that ends at or before it starts (`/0/until_unix_ms`), two rows of one package that overlap
// (`/1`) and an open-ended row before a later one of its package (`/1`), a member of another type
// (`/0/from_unix_ms`), a member the row does not have (`/0/note`), a row without its package
// (`/0/package_id`). An edit: a row that ends at or before it starts (`/rows/0/until_unix_ms`),
// overlapping rows (`/rows/1`, an open-ended one first), no package (`/package_id`), and more rows
// than the record list's bound (`/rows`).
#[test]
fn r8_each_refused_row_appends_nothing() {
    let fixture = Fixture::with_packages(&[(TASK_A, ALIAS, PACK_A)], None);
    let at = START + MINUTE_MS;
    fixture.set_time(at);
    let mut client = TestClient::connect(&fixture.host);
    let head = fixture.head();

    let mut not_a_list = schedule_record(instance_scope(), Vec::new(), at);
    not_a_list.content = FactContent::Inline {
        value: ContractFactValue::String("closed".to_owned()),
    };
    let mut wrong_type = stored_row(PACK_A, at, None, false);
    wrong_type.insert("from_unix_ms".to_owned(), FactScalar::Integer(1));
    let mut unknown_member = stored_row(PACK_A, at, None, false);
    unknown_member.insert("note".to_owned(), FactScalar::String("x".to_owned()));
    let mut no_package = stored_row(PACK_A, at, None, false);
    no_package.remove("package_id");
    let published = [
        (not_a_list, ""),
        (
            schedule_record(
                instance_scope(),
                vec![stored_row(PACK_A, at, Some(at), false)],
                at,
            ),
            "/0/until_unix_ms",
        ),
        (
            schedule_record(
                instance_scope(),
                vec![
                    stored_row(PACK_A, at, Some(at + HOUR_MS), false),
                    stored_row(PACK_B, at, Some(at + HOUR_MS), false),
                    stored_row(PACK_A, at + MINUTE_MS, Some(at + 2 * HOUR_MS), true),
                ],
                at,
            ),
            "/2",
        ),
        (
            schedule_record(
                instance_scope(),
                vec![
                    stored_row(PACK_A, at, None, false),
                    stored_row(PACK_A, at + HOUR_MS, Some(at + 2 * HOUR_MS), true),
                ],
                at,
            ),
            "/1",
        ),
        (
            schedule_record(instance_scope(), vec![wrong_type], at),
            "/0/from_unix_ms",
        ),
        (
            schedule_record(instance_scope(), vec![unknown_member], at),
            "/0/note",
        ),
        (
            schedule_record(instance_scope(), vec![no_package], at),
            "/0/package_id",
        ),
    ];
    for (record, field) in published {
        let receipt = client.send(&client.request(publish(record)));
        refused_at(&receipt, field);
    }

    let edits = [
        (
            PACK_A,
            vec![row(at, Some(at - 1), false)],
            "/rows/0/until_unix_ms",
        ),
        (
            PACK_A,
            vec![
                row(at, None, false),
                row(at + HOUR_MS, Some(at + 2 * HOUR_MS), true),
            ],
            "/rows/1",
        ),
        ("", vec![row(at, None, false)], "/package_id"),
        (
            PACK_A,
            (0..257_u64)
                .map(|index| row(at + index * 10, Some(at + index * 10 + 5), false))
                .collect::<Vec<_>>(),
            "/rows",
        ),
    ];
    for (package_id, rows, field) in edits {
        refused_at(
            &fixture.edit(instance_scope(), package_id, rows, None),
            field,
        );
    }

    assert_eq!(fixture.head(), head, "nothing is appended");
    assert!(fixture.schedule_events().is_empty());
    drop(client);
    fixture.close();
}

// ---- Which row applies.

// Each package is resolved on its own, layer by layer: an instance record that names only package
// A leaves package B to the server and game layers. The game closes A and B, the server opens B,
// the instance opens A for an hour: A is decided by the instance's row (its end the next change,
// and below it the game's closed row), B by the server's open row (below it the game's).
#[test]
fn r8_an_instance_record_of_one_package_leaves_another_to_the_server_and_game_layers() {
    let fixture = Fixture::with_packages(&[(TASK_A, ALIAS, PACK_A), (TASK_B, ALIAS, PACK_B)], None);
    let at = START + MINUTE_MS;
    fixture.set_time(at);
    for package_id in [PACK_A, PACK_B] {
        edited(&fixture.edit(
            game_scope(),
            package_id,
            vec![row(START, None, false)],
            None,
        ));
    }
    edited(&fixture.edit(server_scope(), PACK_B, vec![row(START, None, true)], None));
    let instance_row = row(START, Some(at + HOUR_MS), true);
    edited(&fixture.edit(instance_scope(), PACK_A, vec![instance_row], None));

    let view = fixture.view(ALIAS);
    assert_eq!(
        view.packages,
        vec![
            PackSchedulePackage {
                package_id: PACK_A.to_owned(),
                schedulable: true,
                source: PackScheduleSource::Instance,
                row: Some(instance_row),
                next_change_unix_ms: Some(at + HOUR_MS),
                below: Some(PackScheduleBelow {
                    schedulable: false,
                    source: PackScheduleSource::Game,
                }),
            },
            PackSchedulePackage {
                package_id: PACK_B.to_owned(),
                schedulable: true,
                source: PackScheduleSource::Server,
                row: Some(row(START, None, true)),
                next_change_unix_ms: None,
                below: Some(PackScheduleBelow {
                    schedulable: false,
                    source: PackScheduleSource::Game,
                }),
            },
        ]
    );

    // After the instance's row ends, the instance layer still holds A's rows, so it decides, and
    // none covers: A is schedulable with no row (`none`), the server's and game's rows unread.
    fixture.set_time(at + HOUR_MS);
    let later = fixture.view(ALIAS);
    assert_eq!(
        later.packages[0],
        PackSchedulePackage {
            package_id: PACK_A.to_owned(),
            schedulable: true,
            source: PackScheduleSource::None,
            row: None,
            next_change_unix_ms: None,
            below: None,
        }
    );
    assert_eq!(later.packages[1], view.packages[1]);
    fixture.close();
}

// ---- Versions.

// `expected_version` absent compares nothing, 0 means the scope has no record yet, any other value
// must be the list's version: 0 on an empty scope writes; 0 while the scope has a record is
// `fact_observation_version_conflict` with `current_version` and appends nothing; the current
// version writes; none writes. Each accepted edit is one `fact.published` of the scope's whole
// list (the other packages' rows kept), its version that event's sequence; `rows: []` withdraws
// the package's rows. The view names each scope's version, 0 without a record.
#[test]
fn r8_an_edit_compares_the_scope_list_version() {
    let fixture = Fixture::with_packages(&[(TASK_A, ALIAS, PACK_A), (TASK_B, ALIAS, PACK_B)], None);
    fixture.set_time(START + MINUTE_MS);
    let closed = vec![row(START, None, false)];
    let first = edited(&fixture.edit(instance_scope(), PACK_A, closed.clone(), Some(0)));

    fixture.set_time(START + 2 * MINUTE_MS);
    let head = fixture.head();
    let conflict = fixture.edit(instance_scope(), PACK_B, closed.clone(), Some(0));
    assert_eq!(
        conflict.state(),
        RuntimeReceiptState::Denied,
        "{conflict:?}"
    );
    assert_eq!(
        conflict.outcome().code().as_str(),
        "fact_observation_version_conflict"
    );
    assert_eq!(
        conflict.outcome().value("current_version"),
        Some(&json!(first.version))
    );
    assert_eq!(
        conflict.outcome().value("fact_key"),
        Some(&json!(PACK_SCHEDULE_FACT_KEY))
    );
    assert_eq!(fixture.head(), head, "a conflict appends nothing");

    let second = edited(&fixture.edit(
        instance_scope(),
        PACK_B,
        closed.clone(),
        Some(first.version),
    ));
    assert!(second.version > first.version);

    fixture.set_time(START + 3 * MINUTE_MS);
    let third = edited(&fixture.edit(instance_scope(), PACK_A, Vec::new(), None));

    let events = fixture.schedule_events();
    let ids = events
        .iter()
        .map(|(event, _)| (event.sequence(), *event.event_id()))
        .collect::<Vec<(u64, EventId)>>();
    assert_eq!(
        ids,
        vec![
            (first.version, first.event_id),
            (second.version, second.event_id),
            (third.version, third.event_id),
        ],
        "one fact.published per accepted edit"
    );
    let lists = events
        .iter()
        .map(|(_, record)| record.content.clone())
        .collect::<Vec<_>>();
    let list = |rows: Vec<BTreeMap<String, FactScalar>>| FactContent::Inline {
        value: ContractFactValue::RecordList(rows),
    };
    assert_eq!(
        lists,
        vec![
            list(vec![stored_row(PACK_A, START, None, false)]),
            list(vec![
                stored_row(PACK_A, START, None, false),
                stored_row(PACK_B, START, None, false),
            ]),
            list(vec![stored_row(PACK_B, START, None, false)]),
        ]
    );

    let view = fixture.view(ALIAS);
    assert_eq!(
        view.scopes,
        vec![
            PackScheduleScopeVersion {
                scope: instance_scope(),
                version: third.version,
            },
            PackScheduleScopeVersion {
                scope: server_scope(),
                version: 0,
            },
            PackScheduleScopeVersion {
                scope: game_scope(),
                version: 0,
            },
        ]
    );
    fixture.close();
}

// ---- The evaluation.

// A closed package's candidates are deferred `pack_schedule_closed`, after an active blackout's
// deferral (position 4 before position 5): inside the blackout the candidate waits
// `blackout_active`; after it, `pack_schedule_closed` with `package_id` and `next_eligible_unix_ms`
// the covering row's end, which is the decision's wake; at that end the package is dispatched
// again. An open-ended closed row gives no such time and no wake.
#[test]
fn r8_a_closed_package_is_deferred_after_a_blackout_and_wakes_at_the_row_end() {
    let blackout_start = START + HOUR_MS;
    let blackout_end = START + 2 * HOUR_MS;
    let row_end = START + 3 * HOUR_MS;
    let fixture = Fixture::with_packages(
        &[(TASK_A, ALIAS, PACK_A)],
        Some(host_blackout(blackout_start, blackout_end)),
    );
    fixture.set_time(START + MINUTE_MS);
    edited(&fixture.edit(
        instance_scope(),
        PACK_A,
        vec![row(START, Some(row_end), false)],
        None,
    ));

    let inside = fixture.evaluate(blackout_start + MINUTE_MS);
    let (_, wake) = deferred_by(&inside, TASK_A, "blackout_active");
    assert_eq!(wake, Some(blackout_end));
    assert!(
        decision(&inside, TASK_A)
            .reasons
            .iter()
            .all(|reason| reason.outcome.code().as_str() != CLOSED),
        "the blackout's deferral comes first"
    );

    let after = fixture.evaluate(blackout_end + MINUTE_MS);
    let (outcome, wake) = deferred_by(&after, TASK_A, CLOSED);
    assert_eq!(outcome.value("package_id"), Some(&json!(PACK_A)));
    assert_eq!(
        outcome.value("next_eligible_unix_ms"),
        Some(&json!(row_end))
    );
    assert_eq!(wake, Some(row_end), "the evaluation wakes at the row's end");

    let reopened = fixture.evaluate(row_end);
    assert!(dispatched(&reopened, TASK_A).is_some(), "{reopened:#?}");

    fixture.set_time(row_end + MINUTE_MS);
    edited(&fixture.edit(
        instance_scope(),
        PACK_A,
        vec![row(row_end + MINUTE_MS, None, false)],
        None,
    ));
    let open_ended = fixture.evaluate(row_end + 2 * MINUTE_MS);
    let (outcome, wake) = deferred_by(&open_ended, TASK_A, CLOSED);
    assert_eq!(outcome.value("package_id"), Some(&json!(PACK_A)));
    assert!(
        outcome.value("next_eligible_unix_ms").is_none(),
        "{outcome:?}"
    );
    assert_eq!(wake, None, "an open-ended row names no wake");
    fixture.close();
}

// A record written after an evaluation and before its admission refuses the intent as
// `policy_facts_stale`: the closing row's record is in the snapshot identity of the instance's
// view, so the package is not dispatched; the next evaluation defers it `pack_schedule_closed`.
#[test]
fn r8_a_record_written_after_an_evaluation_refuses_its_admission_as_stale() {
    let fixture = Fixture::with_packages(&[(TASK_A, ALIAS, PACK_A)], None);
    let at = START + MINUTE_MS;
    let evaluation = fixture.evaluate(at);
    let (intent, chain) = dispatched(&evaluation, TASK_A)
        .unwrap_or_else(|| panic!("{TASK_A} is dispatched: {evaluation:#?}"));
    record_policy_approval(&fixture.host, &intent);
    edited(&fixture.edit(instance_scope(), PACK_A, vec![row(at, None, false)], None));

    let error = fixture
        .host
        .admit_policy_dispatch(&intent, &chain, &policy_context(&fixture.host, &intent))
        .expect_err("the admission is refused");
    assert_eq!(error.code(), "policy_facts_stale", "{error:?}");
    assert!(!error.is_fatal(), "{error:?}");
    assert!(
        fixture
            .host
            .query_persisted_events_for_test(EventQuery {
                event_type: Some(EventType::PolicyDispatchAdmitted),
                ..EventQuery::default()
            })
            .expect("admitted records")
            .iter()
            .all(|event| !matches!(
                event.payload(),
                EventPayload::Policy(PolicyPayload::DispatchAdmitted(payload))
                    if payload.decision_id() == intent.decision_id
            )),
        "a refused intent is not admitted"
    );

    let next = fixture.evaluate(at + MINUTE_MS);
    deferred_by(&next, TASK_A, CLOSED);
    fixture.close();
}

// Without any `session.pack.schedule` record nothing of the pack schedule enters an evaluation:
// on a host whose catalog tasks are bound to their packages (so the Runtime knows each task's
// package) and on one whose bindings name no package, the same inputs give the same snapshot
// identity and every decision the same, none of them naming a pack schedule code, and the
// candidate is dispatched.
#[test]
fn r8_without_a_record_the_identity_and_every_decision_are_unchanged() {
    let bound = Fixture::with_packages(&[(TASK_A, ALIAS, PACK_A), (TASK_B, ALIAS, PACK_B)], None);
    let unbound = Fixture::start(
        TempDir::new().expect("tempdir"),
        Arc::new(FakeState::default()),
        manifest(&[(TASK_A, None), (TASK_B, None)]),
        &[(TASK_A, ALIAS), (TASK_B, ALIAS)],
        None,
    );
    let at = START + MINUTE_MS;
    let with_packages = bound.evaluate(at);
    let without = unbound.evaluate(at);
    assert!(
        !names_the_pack_schedule(&with_packages),
        "{with_packages:#?}"
    );
    // Each decision's pair, state, eligibility, reasons and wake (its developer text aside).
    let decided = |evaluation: &PolicyEvaluation| {
        evaluation
            .decisions
            .iter()
            .map(|decision| {
                (
                    decision.task_id.clone(),
                    decision.instance_id.clone(),
                    format!("{:?} {:?}", decision.state, decision.eligibility),
                    decision
                        .reasons
                        .iter()
                        .map(|reason| reason.outcome.clone())
                        .collect::<Vec<_>>(),
                    decision.next_wake_unix_ms,
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(decided(&with_packages), decided(&without));
    let (intent, _) = dispatched(&with_packages, TASK_A)
        .unwrap_or_else(|| panic!("{TASK_A} is dispatched: {with_packages:#?}"));
    let (other, _) = dispatched(&without, TASK_A).expect("the same dispatch");
    assert_eq!(intent.fact_snapshot_id, other.fact_snapshot_id);
    bound.close();
    unbound.close();
}

// ---- The view.

// `PackScheduleView` lists every package a task of the active catalog that covers the instance
// runs, in package id order, each with its decision now, its source, its row, its next change and
// the layer below; a package only another instance's task runs, and one only a row names, are not
// listed; each scope's version is named (0 without a record). The read records nothing. An
// unknown package's rows, in two scopes, are reported once per evaluated cycle as
// `pack_schedule_unknown_package` (a decision of the key with no instance) and have no effect.
#[test]
fn r8_the_view_lists_the_instance_packages_and_an_unknown_package_is_reported_once() {
    let fixture = Fixture::with_packages(
        &[
            (TASK_A, ALIAS, PACK_A),
            (TASK_B, ALIAS, PACK_B),
            (TASK_C, OTHER_ALIAS, PACK_C),
        ],
        None,
    );
    let at = START + MINUTE_MS;
    fixture.set_time(at);
    let closed_row = row(START, Some(at + HOUR_MS), false);
    edited(&fixture.edit(instance_scope(), PACK_A, vec![closed_row], None));
    let instance = edited(&fixture.edit(
        instance_scope(),
        UNKNOWN,
        vec![row(START, None, false)],
        None,
    ));
    edited(&fixture.edit(game_scope(), UNKNOWN, vec![row(START, None, false)], None));
    let game = edited(&fixture.edit(
        game_scope(),
        PACK_B,
        vec![row(at + 2 * HOUR_MS, None, false)],
        None,
    ));

    let head = fixture.head();
    let expected = PackScheduleView {
        instance_alias: ALIAS.to_owned(),
        scopes: vec![
            PackScheduleScopeVersion {
                scope: instance_scope(),
                version: instance.version,
            },
            PackScheduleScopeVersion {
                scope: server_scope(),
                version: 0,
            },
            PackScheduleScopeVersion {
                scope: game_scope(),
                version: game.version,
            },
        ],
        packages: vec![
            PackSchedulePackage {
                package_id: PACK_A.to_owned(),
                schedulable: false,
                source: PackScheduleSource::Instance,
                row: Some(closed_row),
                next_change_unix_ms: Some(at + HOUR_MS),
                below: Some(PackScheduleBelow {
                    schedulable: true,
                    source: PackScheduleSource::None,
                }),
            },
            PackSchedulePackage {
                package_id: PACK_B.to_owned(),
                schedulable: true,
                source: PackScheduleSource::None,
                row: None,
                next_change_unix_ms: Some(at + 2 * HOUR_MS),
                below: None,
            },
        ],
    };
    // Before any evaluation the packages are read from the bound packages themselves.
    assert_eq!(fixture.view(ALIAS), expected);
    assert_eq!(fixture.head(), head, "the read records nothing");

    let evaluation = fixture.evaluate(at + MINUTE_MS);
    let reports = evaluation
        .decisions
        .iter()
        .filter(|decision| decision.task_id == PACK_SCHEDULE_FACT_KEY)
        .collect::<Vec<_>>();
    let [report] = reports.as_slice() else {
        panic!("one report: {reports:#?}");
    };
    assert_eq!(report.instance_id, None);
    let [reason] = report.reasons.as_slice() else {
        panic!("one reason: {report:#?}");
    };
    assert_eq!(reason.outcome.code().as_str(), UNKNOWN_PACKAGE);
    assert_eq!(reason.outcome.value("package_id"), Some(&json!(UNKNOWN)));
    deferred_by(&evaluation, TASK_A, CLOSED);
    assert!(dispatched(&evaluation, TASK_B).is_some(), "{evaluation:#?}");

    // After it, the same view from the table the evaluation built.
    fixture.set_time(at + 2 * MINUTE_MS);
    assert_eq!(fixture.view(ALIAS), expected);

    let mut client = TestClient::connect(&fixture.host);
    let request = client.request(RuntimeOperation::PackScheduleView {
        instance_alias: "neutral.instance.none".to_owned(),
    });
    let unknown = client.send(&request);
    assert_eq!(unknown.state(), RuntimeReceiptState::Denied, "{unknown:?}");
    assert_eq!(unknown.outcome().code().as_str(), "instance_unknown");
    drop(client);
    fixture.close();
}

// ---- "Close this task".

/// `StopInstanceRun` of `run` on `alias` with `wait_ms`, on a new connection.
fn stop(address: SocketAddr, alias: &str, run: RequestId, wait_ms: u64) -> RuntimeReceipt {
    let mut client = TestClient::connect_address(address);
    client.set_receipt_read_timeout();
    let request = client.request(RuntimeOperation::StopInstanceRun {
        instance_alias: alias.to_owned(),
        request_id: run,
        wait_ms,
    });
    client.send(&request)
}

/// `EditPackSchedule` closing `package_id` on `alias` from now on, open-ended, on a new connection.
fn close_package(address: SocketAddr, alias: &str, package_id: &str, from: u64) -> RuntimeReceipt {
    let mut client = TestClient::connect_address(address);
    let request = client.request(RuntimeOperation::EditPackSchedule {
        scope: FactScope::Instance {
            instance_id: alias.to_owned(),
        },
        package_id: package_id.to_owned(),
        rows: vec![row(from, None, false)],
        expected_version: None,
    });
    client.send(&request)
}

fn stopped(receipt: &RuntimeReceipt) -> InstanceRunStop {
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    let Some(RuntimeResult::InstanceRunStopped { stopped }) = receipt.result() else {
        panic!("a stop: {receipt:?}");
    };
    *stopped
}

// The workbench's "close this task" is two steps: a closing row of the task's package on the
// instance, then `StopInstanceRun` of its run. A client's direct run of the package is stopped
// (`contained_task_cancelled`, the Runtime's reset done); a run the scheduler started is refused
// `scheduled_contained_task_not_client_cancellable` with `run_origin` `policy` and nothing is
// cancelled, while the closed row keeps the package from the next cycle's selection.
#[test]
fn r8_closing_a_task_then_stopping_its_run() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let package = PathBuf::from(direct_run_package(&root, &state));
    let fixture = Fixture::start(
        root,
        Arc::clone(&state),
        manifest(&[(TASK_A, Some(package.as_path()))]),
        &[(TASK_A, ALIAS)],
        None,
    );
    let address = fixture.host.runtime_info().socket_addr().expect("address");
    // The direct run's package, as its control declares it.
    let package_id = "neutral.semantic.task";
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let request = runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            ALIAS,
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package.to_string_lossy().into_owned())
                .expect("task request"),
        ),
    );
    let run = request.request_id();
    let (sent, answered) = mpsc::channel();
    let checkpoint = fixture
        .host
        .run_at_request_checkpoint_for_test(
            ContainedTaskCheckpointPoint::PackageAdmitted,
            run,
            fixture.instance,
            move |_| {
                let closed = close_package(address, ALIAS, package_id, START);
                let stopping = stop(address, ALIAS, run, 0);
                sent.send((closed, stopping)).expect("the two answers");
            },
        )
        .expect("arm the run's checkpoint");
    let receipt = fixture
        .host
        .process_request_for_test(&request, ConnectionId::new(1_801).expect("connection"))
        .expect("the run's receipt");
    assert_eq!(checkpoint.consumed(), 1, "the run reached its checkpoint");
    let (closed, first) = answered.recv_timeout(GATE_WAIT).expect("the two answers");
    edited(&closed);
    assert!(!stopped(&first).settled, "{first:?}");
    assert!(
        matches!(
            receipt.result(),
            Some(RuntimeResult::ContainedTaskCancelled {
                reason: ContainedTaskCancellationReason::ClientRequested,
                ..
            })
        ),
        "{receipt:?}"
    );
    assert_eq!(
        stopped(&stop(address, ALIAS, run, 0)),
        InstanceRunStop {
            request_id: run,
            settled: true,
            reset: Some(InstanceRunReset::Done),
        }
    );

    // A run the scheduler started: refused with its origin, nothing asked of it.
    let policy_run = *ids.mint_request_id().expect("request id").transport();
    fixture
        .host
        .begin_unstoppable_run_for_test(policy_run, fixture.instance, false)
        .expect("a scheduler's run");
    let refused = stop(address, ALIAS, policy_run, 1_000);
    assert_eq!(refused.state(), RuntimeReceiptState::Denied, "{refused:?}");
    assert_eq!(
        refused.outcome().code().as_str(),
        "scheduled_contained_task_not_client_cancellable"
    );
    assert_eq!(
        refused.outcome().value("run_origin"),
        Some(&json!("policy"))
    );
    assert_eq!(
        fixture
            .host
            .end_unstoppable_run_for_test(policy_run)
            .expect("the run registry"),
        Some(false),
        "nothing asked the scheduler's run to stop"
    );

    // The closed row keeps the package from the next cycle's selection.
    let next = fixture.evaluate(START + HOUR_MS);
    let (outcome, wake) = deferred_by(&next, TASK_A, CLOSED);
    assert_eq!(outcome.value("package_id"), Some(&json!(package_id)));
    assert_eq!(wake, None);
    fixture.close();
}
