// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 B15, slice R6 (host gate key 115; register §1.116; model-b15 §5.1;
// contracts/instance-fact-store.md, "Remedy choice facts"): a person or an agent writes an
// instance's remedy choices, the instance fact `session.remedy.choices`. The key is a family of
// its own, which an observation never mixes with another and which the three counters may write
// until U-6, each keeping its origin on the `fact.published` event. Every write is checked before
// the fact write gate, its shape and then its names against the packages the active catalog
// binds on every instance its scope covers, and a refusal is `session_remedy_choices_invalid`
// (`fact_key`, `field`, `instance_id` for a meaning error) with nothing appended; its scope is
// an instance or a game, never a server (coordinator ruling on #993). Its snapshot identity
// covers its observation, so the same rows written twice are two versions.
//
// A neutral fixture: the policy fixture's one instance (its server and game); package `main`
// declares on its mapping `blocked_k` the remedy of the need `need_a` with the capabilities
// `cap_x` and `cap_y`; package `sink` provides `cap_x` with the parameter `param_p` (options
// `opt_1`, `opt_2`). The active catalog binds both to that instance (tasks `t_main`, `t_sink`).
// Nothing provides `cap_y`. Every expected value is the brief's requirement or model-b15's.

use super::*;
use actingcommand_contract::{
    EventId, FactKeyFamily, FactObservation, FactPayload, FactScalar, REMEDY_CHOICES_FACT_KEY,
    fact_key_family, remedy_choices_snapshot_id,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const MAIN_TASK: &str = "t_main";
const SINK_TASK: &str = "t_sink";
const MAIN_PROCEDURE: &str = "procedure.main";
const SINK_PROCEDURE: &str = "procedure.sink";
const MAIN_PACKAGE: &str = "neutral.test.main";
const SINK_PACKAGE: &str = "neutral.test.sink";
const SERVER: &str = "fixture-server-a";
const GAME: &str = "fixture-game-a";
const DETECTOR: &str = "neutral.test.writer";
const INVALID: &str = "session_remedy_choices_invalid";
const MIXED: &str = "fact_origin_mixed";

// ---- Packages, catalog and host.

// A package's control declaration, with `provides` when one is given.
fn control(package_id: &str, provides: Option<Value>) -> Value {
    let mut control = json!({
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
    if let Some(provides) = provides {
        control["provides"] = provides;
    }
    control
}

// A package's one task: from `home` to `page_b`, which its mapping `blocked_k` maps, with `remedy`
// on that mapping when one is given.
fn task(remedy: Option<Value>) -> Value {
    let mut mapping = json!({
        "outcome_key": "blocked_k",
        "effect": "no_designated_effect",
        "terminal_pages": ["page_b"]
    });
    if let Some(remedy) = remedy {
        mapping["remedy"] = remedy;
    }
    let probe = |page: &str, expected: [u8; 3]| {
        json!({
            "id": format!("{page}_colour"),
            "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
            "expected": expected
        })
    };
    json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "locale": "en-US",
        "goal": "B15 R6 fixture",
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
        }],
        "scheduling_outcome": {"mappings": [mapping]}
    })
}

// The package's source directory at `path`: its control declaration, its resource list and its
// one task.
fn write_package(path: &Path, control: &Value, task: &Value) {
    for (file, value) in [
        ("control.json", control.clone()),
        (
            "resources/operations/resources.json",
            json!({"schema_version": "1.0", "resources": [], "resource_count": 0}),
        ),
        ("resources/operations/task/task.json", task.clone()),
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
}

// The policy fixture's catalog with its one task twice: `t_main` bound to `main`'s procedure and
// `t_sink` to `sink`'s, both on the fixture instance.
fn catalog() -> CatalogSources {
    let mut sources = policy_sources(1);
    let mut tasks: Value = serde_json::from_slice(&sources.tasks.bytes).expect("fixture tasks");
    let mut main = tasks["tasks"][0].clone();
    main["id"] = json!(MAIN_TASK);
    main["procedure_ref"] = json!(MAIN_PROCEDURE);
    let mut sink = main.clone();
    sink["id"] = json!(SINK_TASK);
    sink["procedure_ref"] = json!(SINK_PROCEDURE);
    tasks["tasks"] = json!([main, sink]);
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("fixture task bytes");
    sources
}

// Each task's procedure bound at its package's location, as actingd binds a scheduled procedure.
fn manifest(main: &Path, sink: &Path) -> ProcedureManifest {
    ProcedureManifest::new([(MAIN_PROCEDURE, main), (SINK_PROCEDURE, sink)].map(
        |(procedure_ref, path)| {
            ProcedureBinding::new(
                procedure_ref,
                "operation.observe",
                vec!["after_observation".to_owned()],
            )
            .expect("procedure binding")
            .with_package_path(path.to_string_lossy())
        },
    ))
    .expect("procedure manifest")
}

struct Fixture {
    _root: TempDir,
    host: RuntimeHost,
}

impl Fixture {
    // A host with `main` and `sink` written and bound and the catalog active.
    fn new() -> Self {
        let root = TempDir::new().expect("tempdir");
        let (main, sink) = (root.path().join("main"), root.path().join("sink"));
        write_package(
            &main,
            &control(MAIN_PACKAGE, None),
            &task(Some(
                json!({"need": "need_a", "capabilities": ["cap_x", "cap_y"]}),
            )),
        );
        write_package(
            &sink,
            &control(
                SINK_PACKAGE,
                Some(json!([{"capability": "cap_x", "parameters": [
                    {"name": "param_p", "options": ["opt_1", "opt_2"], "default": "opt_1"}
                ]}])),
            ),
            &task(None),
        );
        let host = RuntimeHost::start(
            config(&root).with_procedure_manifest(manifest(&main, &sink)),
            Arc::new(FakeProvider::one(
                POLICY_INSTANCE_ALIAS,
                instance_id(),
                Arc::new(FakeState::default()),
            )),
        )
        .expect("runtime host");
        host.activate_policy_catalog(&catalog())
            .expect("activate the catalog");
        Self { _root: root, host }
    }

    // Every `fact.published` event of `session.remedy.choices`, in ledger order, with its record.
    fn choice_events(&self) -> Vec<(PersistedEvent, FactRecord)> {
        self.host
            .query_persisted_events_for_test(EventQuery {
                event_type: Some(EventType::FactPublished),
                ..EventQuery::default()
            })
            .expect("ledger records")
            .into_iter()
            .filter_map(|event| {
                let record = match event.payload() {
                    EventPayload::Fact(FactPayload::Published(payload))
                        if payload.record().key == REMEDY_CHOICES_FACT_KEY =>
                    {
                        payload.record().clone()
                    }
                    _ => return None,
                };
                Some((event, record))
            })
            .collect()
    }

    // The instance's active `session.remedy.choices` records.
    fn active_choices(&self) -> Vec<FactRecord> {
        self.host
            .instance_fact_snapshot(InstanceFactContext {
                instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
                server_id: SERVER.to_owned(),
                game_id: GAME.to_owned(),
            })
            .expect("the instance's facts")
            .records
            .into_iter()
            .filter(|record| record.key == REMEDY_CHOICES_FACT_KEY)
            .collect()
    }
}

// ---- Records and requests.

fn row(members: &[(&str, &str)]) -> BTreeMap<String, FactScalar> {
    members
        .iter()
        .map(|(member, value)| {
            (
                (*member).to_owned(),
                FactScalar::String((*value).to_owned()),
            )
        })
        .collect()
}

fn allow(need: &str, capability: &str) -> BTreeMap<String, FactScalar> {
    row(&[("row", "allow"), ("need", need), ("capability", capability)])
}

fn no_remedy(need: &str) -> BTreeMap<String, FactScalar> {
    row(&[("row", "none"), ("need", need)])
}

fn parameter(capability: &str, parameter: &str, value: &str) -> BTreeMap<String, FactScalar> {
    row(&[
        ("row", "parameter"),
        ("capability", capability),
        ("parameter", parameter),
        ("value", value),
    ])
}

fn instance_scope() -> FactScope {
    FactScope::Instance {
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
    }
}

// The remedy choice record of `scope` with `rows`, observed `offset` ms after the fixture's time.
fn choices(scope: FactScope, rows: Vec<BTreeMap<String, FactScalar>>, offset: u64) -> FactRecord {
    FactRecord::remedy_choices(scope, rows, POLICY_NOW_UNIX_MS + offset, DETECTOR)
        .expect("a remedy choice record")
}

fn publish(record: FactRecord) -> RuntimeOperation {
    RuntimeOperation::PublishFacts {
        observation: FactObservation {
            records: vec![record],
        },
    }
}

// The event id of an accepted publication.
#[track_caller]
fn published(receipt: &RuntimeReceipt) -> EventId {
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "the write is accepted: {receipt:?}"
    );
    let Some(RuntimeResult::FactPublished { event_id }) = receipt.result() else {
        panic!("a fact publication result: {receipt:?}");
    };
    *event_id
}

// A refusal's outcome: `(code, fact_key, field, instance_id)`.
fn refusal(receipt: &RuntimeReceipt) -> (String, Option<Value>, Option<Value>, Option<Value>) {
    let outcome = receipt.outcome();
    (
        outcome.code().as_str().to_owned(),
        outcome.value("fact_key").cloned(),
        outcome.value("field").cloned(),
        outcome.value("instance_id").cloned(),
    )
}

// ---- Requirements 1 and 3.

// A valid record written by each of the three counters, the operator (cli, cli), the person
// (user, ui) and the agent (agent, adapter), is published as one `fact.published` event of the
// key that carries the writer's origin and the record as written; its `source_snapshot_id` is
// `remedy-choices:` and the SHA-256 hex of its scope, rows and observation time. The key's family
// is the remedy choices family.
#[test]
fn b15w_a_valid_record_is_published_with_the_writers_origin() {
    assert_eq!(
        fact_key_family(REMEDY_CHOICES_FACT_KEY),
        FactKeyFamily::RemedyChoices
    );
    let fixture = Fixture::new();
    let mut client = TestClient::connect(&fixture.host);
    let rows = vec![
        allow("need_a", "cap_x"),
        parameter("cap_x", "param_p", "opt_2"),
    ];
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
        let record = choices(instance_scope(), rows.clone(), offset);
        let request = writer(&client, publish(record.clone()));
        let event_id = published(&client.send(&request));

        let events = fixture.choice_events();
        let (event, written) = events
            .iter()
            .find(|(event, _)| *event.event_id() == event_id)
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
        let hex = written
            .source_snapshot_id
            .strip_prefix("remedy-choices:")
            .unwrap_or_else(|| panic!("a remedy-choices snapshot id: {written:?}"));
        assert!(
            hex.len() == 64
                && hex
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "a lowercase SHA-256 hex: {hex}"
        );
        assert_eq!(
            written.source_snapshot_id,
            remedy_choices_snapshot_id(&instance_scope(), &rows, POLICY_NOW_UNIX_MS + offset)
                .expect("the snapshot id"),
            "the identity of (scope, rows, observed_at)"
        );
    }
    assert_eq!(fixture.choice_events().len(), 3);
}

// ---- Requirement 2: shape.

// Each shape rule refused alone, as `session_remedy_choices_invalid` with `fact_key` and `field`
// the pointer to the row (and member), and no `instance_id`; nothing is appended.
#[test]
fn b15w_shape_violations_are_refused_with_their_pointer() {
    let fixture = Fixture::new();
    let mut client = TestClient::connect(&fixture.host);
    let scope = instance_scope;
    let mut not_a_list = choices(scope(), Vec::new(), 1);
    not_a_list.content = FactContent::Inline {
        value: ContractFactValue::String("allow".to_owned()),
    };
    let mut with_invalidation = choices(scope(), vec![no_remedy("need_a")], 2);
    with_invalidation.invalidate_on = vec![EventType::PolicyPlanningSignalObserved];
    let mut not_a_string = allow("need_a", "cap_x");
    not_a_string.insert("need".to_owned(), FactScalar::Integer(1));
    let mut without_capability = allow("need_a", "cap_x");
    without_capability.remove("capability");
    let mut extra_member = allow("need_a", "cap_x");
    extra_member.insert(
        "parameter".to_owned(),
        FactScalar::String("param_p".to_owned()),
    );
    let cases = [
        ("not a record list", not_a_list, ""),
        ("invalidating events", with_invalidation, "/invalidate_on"),
        (
            "no row kind",
            choices(scope(), vec![row(&[("need", "need_a")])], 3),
            "/0/row",
        ),
        (
            "an unknown row kind",
            choices(
                scope(),
                vec![row(&[("row", "deny"), ("need", "need_a")])],
                4,
            ),
            "/0/row",
        ),
        (
            "a member its kind lacks",
            choices(scope(), vec![no_remedy("need_b"), extra_member], 5),
            "/1/parameter",
        ),
        (
            "a missing field",
            choices(scope(), vec![without_capability], 6),
            "/0/capability",
        ),
        (
            "a field that is not a string",
            choices(scope(), vec![not_a_string], 7),
            "/0/need",
        ),
        (
            "a name outside the charset",
            choices(scope(), vec![allow("Need_A", "cap_x")], 8),
            "/0/need",
        ),
        (
            "none as a capability",
            choices(scope(), vec![allow("need_a", "none")], 9),
            "/0/capability",
        ),
        (
            "a row twice",
            choices(
                scope(),
                vec![allow("need_a", "cap_x"), allow("need_a", "cap_x")],
                10,
            ),
            "/1",
        ),
        (
            "none with an allow of the same need",
            choices(
                scope(),
                vec![allow("need_a", "cap_x"), no_remedy("need_a")],
                11,
            ),
            "/1",
        ),
        (
            "two values of one parameter",
            choices(
                scope(),
                vec![
                    parameter("cap_x", "param_p", "opt_1"),
                    parameter("cap_x", "param_p", "opt_2"),
                ],
                12,
            ),
            "/1",
        ),
    ];
    for (case, record, field) in cases {
        let request = client.request(publish(record));
        let receipt = client.send(&request);
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Denied,
            "{case}: {receipt:?}"
        );
        assert_eq!(
            refusal(&receipt),
            (
                INVALID.to_owned(),
                Some(json!(REMEDY_CHOICES_FACT_KEY)),
                Some(json!(field)),
                None
            ),
            "{case}"
        );
    }
    assert!(
        fixture.choice_events().is_empty(),
        "a refused write appends nothing"
    );
}

// ---- Requirement 2: meaning.

// Names the packages bound on a covered instance do not declare are refused with the pointer of
// the row and member and the instance they are unknown on: a need no bound remedy declares, a
// capability the need's remedy does not list, a parameter row of a capability no bound provider
// declares (`cap_y` is listed but provided by nothing), of a parameter the capability does not
// declare, or of a value that is not one of its options; a game record covers the game's
// instance. Nothing is appended; rows naming only declared names are taken.
#[test]
fn b15w_names_the_bound_packages_do_not_declare_are_refused() {
    let fixture = Fixture::new();
    let mut client = TestClient::connect(&fixture.host);
    let instance = instance_scope;
    let game = || FactScope::Game {
        game_id: GAME.to_owned(),
    };
    let cases = [
        (
            "an unknown need",
            choices(
                instance(),
                vec![allow("need_a", "cap_x"), allow("need_z", "cap_x")],
                1,
            ),
            "/1/need",
        ),
        (
            "a capability the need's remedy does not list",
            choices(instance(), vec![allow("need_a", "cap_z")], 2),
            "/0/capability",
        ),
        (
            "a capability no bound provider declares",
            choices(instance(), vec![parameter("cap_y", "param_p", "opt_1")], 3),
            "/0/capability",
        ),
        (
            "a parameter the capability does not declare",
            choices(instance(), vec![parameter("cap_x", "param_q", "opt_1")], 4),
            "/0/parameter",
        ),
        (
            "a value that is not an option",
            choices(instance(), vec![parameter("cap_x", "param_p", "opt_9")], 5),
            "/0/value",
        ),
        (
            "an unknown need in a game record",
            choices(game(), vec![allow("need_z", "cap_x")], 6),
            "/0/need",
        ),
    ];
    for (case, record, field) in cases {
        let request = client.request(publish(record));
        let receipt = client.send(&request);
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Denied,
            "{case}: {receipt:?}"
        );
        assert_eq!(
            refusal(&receipt),
            (
                INVALID.to_owned(),
                Some(json!(REMEDY_CHOICES_FACT_KEY)),
                Some(json!(field)),
                Some(json!(POLICY_INSTANCE_ALIAS))
            ),
            "{case}"
        );
    }
    assert!(
        fixture.choice_events().is_empty(),
        "a refused write appends nothing"
    );

    let declared = choices(
        instance(),
        vec![
            allow("need_a", "cap_y"),
            parameter("cap_x", "param_p", "opt_1"),
            no_remedy("need_z"),
        ],
        7,
    );
    let request = client.request(publish(declared));
    published(&client.send(&request));
    assert_eq!(fixture.choice_events().len(), 1);
}

// ---- Requirement 1: families.

// An observation that mixes the remedy choices with a key of another family (an instance
// information key or a priority offset) is refused as `fact_origin_mixed`: the contract refuses
// to build it from the agent and from the operator, and the host refuses it when it arrives;
// nothing is appended.
#[test]
fn b15w_a_write_mixing_families_is_refused() {
    let fixture = Fixture::new();
    let mut client = TestClient::connect(&fixture.host);
    let remedy = choices(instance_scope(), vec![no_remedy("need_a")], 1);
    let mut information = remedy.clone();
    information.key = "display.neutral_label".to_owned();
    information.content = FactContent::Inline {
        value: ContractFactValue::String("neutral".to_owned()),
    };
    let mut offset = remedy.clone();
    offset.key = actingcommand_contract::priority_offset_fact_key(MAIN_TASK);
    offset.content = FactContent::Inline {
        value: ContractFactValue::Integer(250),
    };
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    for other in [&information, &offset] {
        for (actor, source) in [
            (EventActor::Agent, EventSource::Adapter),
            (EventActor::Cli, EventSource::Cli),
        ] {
            let built = RuntimeRequest::new(
                ids.mint_request_id().expect("request id"),
                ids.mint_correlation_id().expect("correlation id"),
                None,
                actor,
                source,
                POLICY_NOW_UNIX_MS,
                RuntimeOperation::PublishFacts {
                    observation: FactObservation {
                        records: vec![remedy.clone(), other.clone()],
                    },
                },
            );
            let Err(error) = built else {
                panic!("a mix with {} from {actor:?} was built", other.key);
            };
            assert_eq!(error.code().as_str(), MIXED, "{} from {actor:?}", other.key);
        }

        // The same mix as it would arrive on the wire.
        let request = client.agent_request(publish(remedy.clone()));
        let mut wire = serde_json::to_value(&request).expect("request JSON");
        wire["operation"]["observation"]["records"]
            .as_array_mut()
            .expect("the observation's records")
            .push(serde_json::to_value(other).expect("record JSON"));
        let mixed: RuntimeRequest = serde_json::from_value(wire).expect("the request's shape");
        let receipt = client.send(&mixed);
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Denied,
            "{}: {receipt:?}",
            other.key
        );
        assert_eq!(receipt.outcome().code().as_str(), MIXED, "{}", other.key);
    }
    assert!(fixture.choice_events().is_empty());
    assert!(
        !fixture
            .host
            .query_persisted_events_for_test(EventQuery {
                event_type: Some(EventType::FactPublished),
                ..EventQuery::default()
            })
            .expect("ledger records")
            .iter()
            .any(|event| matches!(
                event.payload(),
                EventPayload::Fact(FactPayload::Published(payload))
                    if payload.record().key == information.key
                        || payload.record().key == offset.key
            )),
        "neither mixed key is appended"
    );
}

// ---- Requirement 3: identity.

// The same rows written twice, the second observed later, are two versions: two snapshot ids,
// two `fact.published` events in order, and the instance's active record is the second.
#[test]
fn b15w_two_writes_of_the_same_rows_are_two_versions() {
    let fixture = Fixture::new();
    let mut client = TestClient::connect(&fixture.host);
    let rows = vec![allow("need_a", "cap_x")];
    let first = choices(instance_scope(), rows.clone(), 1);
    let second = choices(instance_scope(), rows, 2);
    assert_ne!(
        first.source_snapshot_id, second.source_snapshot_id,
        "a later observation is a new snapshot id"
    );
    let request = client.request(publish(first.clone()));
    let first_event = published(&client.send(&request));
    let request = client.request(publish(second.clone()));
    let second_event = published(&client.send(&request));
    assert_ne!(first_event, second_event);

    let events = fixture.choice_events();
    assert_eq!(
        events
            .iter()
            .map(|(event, record)| (*event.event_id(), record.source_snapshot_id.clone()))
            .collect::<Vec<_>>(),
        vec![
            (first_event, first.source_snapshot_id),
            (second_event, second.source_snapshot_id.clone()),
        ]
    );
    let active = fixture.active_choices();
    assert_eq!(
        active
            .iter()
            .map(|record| record.source_snapshot_id.clone())
            .collect::<Vec<_>>(),
        vec![second.source_snapshot_id]
    );
}

// ---- Requirement 2: scope (coordinator ruling on #993).

// The store's server scope names a server without its game, so a record there would apply to the
// instances of every game whose server id is equal: remedy choices are written at instance or
// game scope only. A server-scoped record is refused from every writer, the operator (cli, cli),
// the person (user, ui) and the agent (agent, adapter), as a `PublishFacts` observation or a
// `PublishFact`, whatever its rows (names the bound packages declare, a `none` row, or the empty
// list that would withdraw it): `session_remedy_choices_invalid` with `fact_key`, `field`
// `/scope` and no `instance_id`; nothing is appended. The same declared rows at the game scope of
// the fixture's instance are taken.
#[test]
fn b15w_a_server_scoped_record_is_refused() {
    let fixture = Fixture::new();
    let mut client = TestClient::connect(&fixture.host);
    let server = || FactScope::Server {
        server_id: SERVER.to_owned(),
    };
    let game = || FactScope::Game {
        game_id: GAME.to_owned(),
    };
    let declared = vec![
        allow("need_a", "cap_x"),
        parameter("cap_x", "param_p", "opt_2"),
    ];
    type Writer = fn(&TestClient, RuntimeOperation) -> RuntimeRequest;
    let writers: [(Writer, EventActor); 3] = [
        (TestClient::request, EventActor::Cli),
        (TestClient::governance_request, EventActor::User),
        (TestClient::agent_request, EventActor::Agent),
    ];
    let mut offset = 0_u64;
    for (writer, actor) in writers {
        for rows in [declared.clone(), vec![no_remedy("need_a")], Vec::new()] {
            offset += 1;
            let case = format!("{actor:?}, {} rows", rows.len());
            let record = choices(server(), rows, offset);
            for operation in [
                publish(record.clone()),
                RuntimeOperation::PublishFact { record },
            ] {
                let request = writer(&client, operation);
                let receipt = client.send(&request);
                assert_eq!(
                    receipt.state(),
                    RuntimeReceiptState::Denied,
                    "{case}: {receipt:?}"
                );
                assert_eq!(
                    refusal(&receipt),
                    (
                        INVALID.to_owned(),
                        Some(json!(REMEDY_CHOICES_FACT_KEY)),
                        Some(json!("/scope")),
                        None
                    ),
                    "{case}"
                );
            }
        }
    }
    assert!(
        fixture.choice_events().is_empty(),
        "a refused write appends nothing"
    );

    let request = client.request(publish(choices(game(), declared, offset + 1)));
    published(&client.send(&request));
    assert_eq!(
        fixture
            .choice_events()
            .into_iter()
            .map(|(_, record)| record.scope)
            .collect::<Vec<_>>(),
        vec![game()],
        "the game scope is taken"
    );
}
