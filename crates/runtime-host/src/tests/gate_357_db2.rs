// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #357 DB-2 (#430 S4a-1; model M-357a v1.1 §2.4, §3.5 K1, K2, K4, §4.3; register
//! §1.244; coordinator decision 6, #419): shared tables through the host's run preparation. A
//! table failure denies that one run with `package_invalid` and the failure's own code, before any
//! lease and any task record, and nothing else stops: a package that reads no table still runs on
//! the same instance and the Runtime stays up. A table replaced in place is the next run's. A
//! scan reads the bound table on every viewport: a static table drop is a candidate with its
//! verdict and no unknown instance, a candidate not of the class is gated out, a pinned fact
//! missing stays an unknown instance. A single-frame select reads the same table. A run that binds
//! a table records it once with its file's digest; another instance is untouched by a failure. A
//! synthetic `neutral` game and neutral tables only; the list screen and the package are the
//! SEL-L4b gate's. The layer package's failure is `gate_373_nav6`'s, on its interrupt fixture.

use super::gate_308_l4b::{
    ALIAS, CardNames, HEIGHT, WIDTH, list_screen, member, scan_select_package,
    scan_select_package_with, single_frame_select_package, table_read_identities,
};
use super::*;
use actingcommand_contract::outcome::{Category, Outcome};
use actingcommand_contract::{RuntimeLifecyclePhase, RuntimePayload};
use serde_json::{Value, json};

/// The other instance of the two-instance test.
const OTHER_ALIAS: &str = "neutral.instance.other";

/// A table of the game, `unit` then `group` as its row key, with a nullable score.
fn table(rows: Value) -> Value {
    json!({
        "schema_version": "actingcommand.table.v1",
        "game": "neutral",
        "table_id": "unit_scores",
        "row_key": ["unit", "group"],
        "columns": [
            {"name": "unit", "type": "key"},
            {"name": "group", "type": "enum_string", "allowed": ["alpha", "beta"]},
            {"name": "listed", "type": "boolean"},
            {"name": "score_milli", "type": "integer", "min": 0, "max": 100000, "nullable": true}
        ],
        "rows": rows
    })
}

/// A document that gates on the class and scores by the table, pinned as `pin` says.
pub(super) fn table_policy(mode: &str, pin: Value) -> Value {
    let read = |column: &str| {
        json!({"source": "table", "table_id": "unit_scores", "key_field": "member",
               "pins": [pin.clone()], "value_column": column})
    };
    let mut class = read("listed");
    class["absent"] = json!({"type": "boolean", "value": false});
    let identities = table_read_identities();
    let mut facts = Vec::new();
    if pin.get("equals_fact").is_some() {
        facts.push(json!({"fact_key": "session.example.group",
                          "value_type": {"type": "enum_string", "allowed": ["alpha", "beta"]},
                          "max_age_ms": 3_600_000, "minimum_confidence_milli": 1000}));
    }
    json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": "cards-by-table",
        "applies_to": {
            "candidate_layout_id": "layout/cards",
            "outcome_keys": {"selected": "card_selected", "empty": "card_empty",
                             "insufficient": "card_insufficient",
                             "ambiguous": "card_ambiguous", "unknown": "card_unknown"}
        },
        "fields": [{"name": "member",
                    "value_type": {"type": "enum_string", "allowed": identities}}],
        "facts": facts,
        "tables": [{"table_id": "unit_scores", "path": "tables/unit_scores.json",
                    "row_key": ["unit", "group"],
                    "columns": {"group": {"type": "enum_string", "allowed": ["alpha", "beta"]},
                                "listed": {"type": "boolean"},
                                "score_milli": {"type": "integer"}}}],
        "gates": [{"gate_id": "gate-listed",
                   "predicate": {"kind": "boolean_equals", "value": class, "expected": true},
                   "on_unknown": {"kind": "drop_candidate"}}],
        "scoring": [{"term_id": "term-score", "value": read("score_milli"),
                     "transform": {"kind": "identity"}, "weight_milli": 1000,
                     "on_unknown": {"kind": "drop_candidate"}}],
        "selection": {"mode": mode, "required_count": 1},
        "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
    })
}

pub(super) fn literal_pin() -> Value {
    json!({"column": "group", "equals": {"type": "string", "value": "alpha"}})
}

/// The game root `neutral`, without tables, below a new standard-pack root: (root, game root).
fn game_root(temp: &TempDir) -> (PathBuf, PathBuf) {
    let root = temp.path().join("packs");
    let game = root.join("neutral");
    fs::create_dir_all(&game).expect("game root");
    fs::write(
        game.join("bundle.json"),
        serde_json::to_vec(&json!({
            "schema_version": "actingcommand.bundle.v4",
            "game": "neutral",
            "packs": [{"package_id": "neutral.test.listed", "server": "test",
                       "entry_task_id": "task", "path": "packs/neutral.test.listed",
                       "file_count": 3, "byte_count": 64}]
        }))
        .expect("bundle JSON"),
    )
    .expect("write bundle.json");
    (root, game)
}

fn write_table_at(game: &Path, bytes: &[u8]) {
    let directory = game.join("tables");
    fs::create_dir_all(&directory).expect("tables directory");
    fs::write(directory.join("unit_scores.json"), bytes).expect("write the table");
}

struct Fixture {
    temp: TempDir,
    game: PathBuf,
    host: RuntimeHost,
    state: Arc<FakeState>,
}

impl Fixture {
    /// A host whose standard-pack root holds the game root `neutral`, without tables, and whose
    /// one device instance shows the list screen.
    fn new() -> Self {
        let temp = TempDir::new().expect("tempdir");
        let (root, game) = game_root(&temp);
        let (host, state) = start(&temp, Some(root));
        Self {
            temp,
            game,
            host,
            state,
        }
    }

    fn write_table(&self, bytes: &[u8]) {
        write_table_at(&self.game, bytes);
    }

    fn write_table_value(&self, value: &Value) {
        self.write_table(&serde_json::to_vec(value).expect("table JSON"));
    }
}

/// A fake device whose display is the package's and which shows the list screen.
fn list_device() -> Arc<FakeState> {
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    *state
        .physical_task_extent
        .lock()
        .expect("fake physical task extent") = Some((WIDTH, HEIGHT));
    let screen = list_screen();
    *state.pixel_script.lock().expect("pixel script") =
        Some(Arc::new(move |_: usize, _: usize| screen.clone()));
    state
}

fn start(temp: &TempDir, root: Option<PathBuf>) -> (RuntimeHost, Arc<FakeState>) {
    let state = list_device();
    let host = RuntimeHost::start(
        config(temp).with_standard_packs_root(root),
        Arc::new(
            FakeProvider::one(ALIAS, instance_id(), Arc::clone(&state))
                .with_vision_provider(Arc::new(CardNames)),
        ),
    )
    .expect("runtime host");
    (host, state)
}

struct Run {
    receipt: RuntimeReceipt,
    events: Vec<PersistedEvent>,
}

impl Run {
    fn task_records(&self) -> usize {
        self.events
            .iter()
            .filter(|event| matches!(event.payload(), EventPayload::Task(_)))
            .count()
    }

    /// The select step's one record: its verdicts and its unknown instances.
    fn selection(&self) -> (usize, u32) {
        self.events
            .iter()
            .find_map(|event| match event.payload() {
                EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
                    TaskSemanticFact::SelectionEvaluated { selection, .. } => Some((
                        selection.verdicts.len(),
                        selection
                            .unknown_check
                            .map_or(0, |check| check.unknown_instances),
                    )),
                    _ => None,
                },
                _ => None,
            })
            .expect("the step's selection record")
    }

    /// The candidates the select step's one record selected.
    fn selected(&self) -> Vec<String> {
        self.events
            .iter()
            .find_map(|event| match event.payload() {
                EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
                    TaskSemanticFact::SelectionEvaluated { selection, .. } => {
                        Some(selection.selected.clone())
                    }
                    _ => None,
                },
                _ => None,
            })
            .expect("the step's selection record")
    }
}

fn run(host: &RuntimeHost, temp: &TempDir, package: &[u8]) -> Run {
    run_on(host, temp, ALIAS, package)
}

fn run_on(host: &RuntimeHost, temp: &TempDir, alias: &str, package: &[u8]) -> Run {
    let path = temp.path().join("run.zip");
    fs::write(&path, package).expect("write the package");
    let mut client = TestClient::connect(host);
    client.set_receipt_read_timeout();
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::run_contained_task(
            alias,
            client.ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(path.display().to_string()).expect("task request"),
        ),
    );
    let receipt = client.send(&request);
    drop(client);
    let events = host
        .query_persisted_events_for_test(EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        })
        .expect("the run's records");
    Run { receipt, events }
}

fn text<'a>(outcome: &'a Outcome, key: &str) -> Option<&'a str> {
    outcome.value(key).and_then(Value::as_str)
}

/// The `standard_pack_table_read` records so far, in ledger order.
fn table_reads(host: &RuntimeHost) -> Vec<Outcome> {
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(EventType::RuntimeLifecycleObserved),
        ..EventQuery::default()
    })
    .expect("lifecycle events")
    .iter()
    .filter_map(|event| match event.payload() {
        EventPayload::Runtime(RuntimePayload::LifecycleObserved(value)) => match value.phase() {
            RuntimeLifecyclePhase::StandardPackRead { outcome }
                if outcome.code().as_str() == "standard_pack_table_read" =>
            {
                Some(outcome.clone())
            }
            _ => None,
        },
        _ => None,
    })
    .collect()
}

/// The run was denied with `code`, `package_invalid`, before any task record and any input, and
/// nothing else stopped: the Runtime is up, and a package that reads no table runs next on the
/// same instance (coordinator decision 6, #419).
fn assert_only_this_run_denied(fixture: &Fixture, run_of: &Run, code: &str) {
    let outcome = run_of.receipt.outcome();
    assert_eq!(
        run_of.receipt.state(),
        RuntimeReceiptState::Denied,
        "{outcome:?}"
    );
    assert_eq!(outcome.code().as_str(), code, "{outcome:?}");
    assert_eq!(text(outcome, "runtime_code"), Some("package_invalid"));
    assert_eq!(run_of.task_records(), 0, "no task record");
    assert_eq!(fixture.state.input_count.load(Ordering::Acquire), 0);
    assert!(
        fixture
            .host
            .fatal_error()
            .expect("runtime health")
            .is_none()
    );
    let plain = run(&fixture.host, &fixture.temp, &scan_select_package());
    assert_eq!(
        plain.receipt.state(),
        RuntimeReceiptState::Completed,
        "a package that reads no table: {:?}",
        plain.receipt.outcome()
    );
}

/// K1 and K2 refuse the one run: a missing table, one that cannot be read, a refused one and each
/// of the mismatches; repaired in place, the next run reads it, and broken again, the next run
/// after that is refused again.
#[test]
fn db2_a_table_failure_denies_that_run_and_nothing_else() {
    let fixture = Fixture::new();
    let package = scan_select_package_with(table_policy("none_allowed", literal_pin()), true);
    let denied = |code: &str| {
        let refused = run(&fixture.host, &fixture.temp, &package);
        assert_only_this_run_denied(&fixture, &refused, code);
        refused
    };

    let missing = denied("standard_pack_table_missing");
    assert_eq!(text(missing.receipt.outcome(), "game"), Some("neutral"));

    fixture.write_table(b"{\"not\": \"a table\"}");
    let invalid = denied("standard_pack_table_invalid");
    assert_eq!(
        text(invalid.receipt.outcome(), "standard_pack_problem"),
        Some("malformed")
    );

    let rows = json!([["member-00", "alpha", true, 5]]);
    let mut other_key = table(rows.clone());
    other_key["row_key"] = json!(["unit"]);
    fixture.write_table_value(&other_key);
    let row_key = denied("standard_pack_table_mismatch");
    assert_eq!(
        text(row_key.receipt.outcome(), "table_id"),
        Some("unit_scores")
    );
    assert_eq!(
        text(row_key.receipt.outcome(), "table_mismatch"),
        Some("row_key")
    );

    let mut no_column = table(rows.clone());
    no_column["columns"][3]["name"] = json!("other_milli");
    fixture.write_table_value(&no_column);
    let column_absent = denied("standard_pack_table_mismatch");
    assert_eq!(
        text(column_absent.receipt.outcome(), "table_mismatch"),
        Some("column_absent")
    );
    assert_eq!(
        text(column_absent.receipt.outcome(), "table_column"),
        Some("score_milli")
    );

    let mut other_type = table(json!([["member-00", "alpha", 1, 5]]));
    other_type["columns"][2] = json!({"name": "listed", "type": "integer", "min": 0, "max": 1});
    fixture.write_table_value(&other_type);
    let column_type = denied("standard_pack_table_mismatch");
    assert_eq!(
        text(column_type.receipt.outcome(), "table_mismatch"),
        Some("column_type")
    );

    let mut fewer = table(rows.clone());
    fewer["columns"][1]["allowed"] = json!(["alpha"]);
    fixture.write_table_value(&fewer);
    let not_subset = denied("standard_pack_table_mismatch");
    assert_eq!(
        text(not_subset.receipt.outcome(), "table_mismatch"),
        Some("allowed_not_subset")
    );

    // Repaired in place, read by the next run: nobody holds a row, every candidate is dropped.
    fixture.write_table_value(&table(json!([["member-99", "alpha", true, 5]])));
    let repaired = run(&fixture.host, &fixture.temp, &package);
    assert_eq!(
        repaired.receipt.state(),
        RuntimeReceiptState::Completed,
        "{:?}",
        repaired.receipt.outcome()
    );

    fixture.write_table(b"[1.5]");
    denied("standard_pack_table_invalid");
    fixture.host.close().expect("close runtime host");
}

/// A table that cannot be read, and a run with no root configured at all, deny that run the same
/// way; a broken game root denies it with the root's own code.
#[test]
fn db2_an_unreadable_table_no_root_and_a_broken_root_deny_the_run() {
    let fixture = Fixture::new();
    let package = scan_select_package_with(table_policy("none_allowed", literal_pin()), true);
    fs::create_dir_all(fixture.game.join("tables/unit_scores.json")).expect("a directory");
    let unreadable = run(&fixture.host, &fixture.temp, &package);
    assert_only_this_run_denied(&fixture, &unreadable, "standard_pack_table_unreadable");

    fs::write(fixture.game.join("bundle.json"), b"{}").expect("break the root");
    let broken = run(&fixture.host, &fixture.temp, &package);
    assert_only_this_run_denied(&fixture, &broken, "standard_pack_root_invalid");
    fixture.host.close().expect("close runtime host");

    let temp = TempDir::new().expect("tempdir");
    let (host, state) = start(&temp, None);
    let unconfigured = run(&host, &temp, &package);
    assert_eq!(unconfigured.receipt.state(), RuntimeReceiptState::Denied);
    assert_eq!(
        unconfigured.receipt.outcome().code().as_str(),
        "standard_pack_table_missing"
    );
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    host.close().expect("close runtime host");
}

/// §4.3: a scan over a table that holds no row for anyone. Under `none_allowed` with `on_empty`
/// each is a candidate with its `unknown_dropped` verdict and no unknown instance, so the run ends
/// normally (P4). Under `exactly_one` the same scan is `insufficient`, `selection_not_selected`
/// (P5), never blocked.
#[test]
fn db2_a_scan_lists_static_table_drops_as_candidates_and_never_as_unknown() {
    let fixture = Fixture::new();
    fixture.write_table_value(&table(json!([["member-99", "alpha", true, 5]])));
    let empty = run(
        &fixture.host,
        &fixture.temp,
        &scan_select_package_with(table_policy("none_allowed", literal_pin()), true),
    );
    assert_eq!(
        empty.receipt.state(),
        RuntimeReceiptState::Completed,
        "{:?}",
        empty.receipt.outcome()
    );
    assert_eq!(empty.selection(), (6, 0));

    let insufficient = run(
        &fixture.host,
        &fixture.temp,
        &scan_select_package_with(table_policy("exactly_one", literal_pin()), false),
    );
    assert_eq!(
        insufficient.receipt.outcome().code().as_str(),
        "selection_not_selected"
    );
    assert_eq!(insufficient.selection(), (6, 0));
    assert_eq!(fixture.state.input_count.load(Ordering::Acquire), 0);
    fixture.host.close().expect("close runtime host");
}

/// §4.2, §4.3: a key with rows, none of this class, is gated out and no candidate; a pinned fact
/// the snapshot does not hold is an unknown instance, which blocks the empty answer (P6).
#[test]
fn db2_a_scan_gates_out_another_class_and_keeps_a_missing_pinned_fact_unknown() {
    let fixture = Fixture::new();
    let rows = (0..=6)
        .map(|card| json!([member(card), "beta", true, 5]))
        .collect::<Vec<_>>();
    fixture.write_table_value(&table(Value::Array(rows)));
    let gated = run(
        &fixture.host,
        &fixture.temp,
        &scan_select_package_with(table_policy("none_allowed", literal_pin()), true),
    );
    assert_eq!(
        gated.receipt.state(),
        RuntimeReceiptState::Completed,
        "{:?}",
        gated.receipt.outcome()
    );
    assert_eq!(gated.selection(), (0, 0));

    let unknown = run(
        &fixture.host,
        &fixture.temp,
        &scan_select_package_with(
            table_policy(
                "none_allowed",
                json!({"column": "group", "equals_fact": "session.example.group"}),
            ),
            true,
        ),
    );
    assert_eq!(
        unknown.receipt.outcome().code().as_str(),
        "selection_result_blocked"
    );
    assert_eq!(unknown.selection(), (0, 6));
    fixture.host.close().expect("close runtime host");
}

/// Coordinator item 2 (#430 §8.2; register §1.244): a run that binds a table writes one
/// `standard_pack_read` record for it, with the file's digest, on the success path; a run that
/// reads none writes none, and a failed bind writes none and keeps its refusal.
#[test]
fn db2_a_run_records_the_table_it_reads_with_its_digest_and_a_run_that_reads_none_records_none() {
    let fixture = Fixture::new();
    let bytes =
        serde_json::to_vec(&table(json!([["member-99", "alpha", true, 5]]))).expect("table JSON");
    fixture.write_table(&bytes);

    let plain = run(&fixture.host, &fixture.temp, &scan_select_package());
    assert_eq!(plain.receipt.state(), RuntimeReceiptState::Completed);
    assert!(table_reads(&fixture.host).is_empty(), "no table was read");

    let package = scan_select_package_with(table_policy("none_allowed", literal_pin()), true);
    let first = run(&fixture.host, &fixture.temp, &package);
    assert_eq!(
        first.receipt.state(),
        RuntimeReceiptState::Completed,
        "{:?}",
        first.receipt.outcome()
    );
    let digest = format!("sha256:{:x}", Sha256::digest(bytes));
    let reads = table_reads(&fixture.host);
    let [record] = reads.as_slice() else {
        panic!("one record of the table: {reads:?}");
    };
    assert_eq!(record.category(), Category::Info);
    assert_eq!(text(record, "game"), Some("neutral"));
    assert!(text(record, "path").is_some_and(|path| path.ends_with("unit_scores.json")));
    assert_eq!(text(record, "actual_sha256"), Some(digest.as_str()));

    // Once per run: the next run writes its own.
    run(&fixture.host, &fixture.temp, &package);
    assert_eq!(table_reads(&fixture.host).len(), 2);

    // A failed bind keeps its refusal and writes none.
    fixture.write_table(b"[1.5]");
    let refused = run(&fixture.host, &fixture.temp, &package);
    assert_eq!(refused.receipt.state(), RuntimeReceiptState::Denied);
    assert_eq!(table_reads(&fixture.host).len(), 2);
    fixture.host.close().expect("close runtime host");
}

/// A single-frame select reads the bound table as a scan does (TB7): the table's highest value is
/// the one chosen, confirmed on a fresh frame and tapped, and the run ends on the target page.
#[test]
fn db2_a_single_frame_select_chooses_by_the_bound_table() {
    let fixture = Fixture::new();
    let rows = (0..6)
        .map(|card| {
            json!([
                member(card),
                "alpha",
                true,
                if card == 3 { 900 } else { 100 }
            ])
        })
        .collect::<Vec<_>>();
    fixture.write_table_value(&table(Value::Array(rows)));
    // The target page shows once the tap is made: pixel (0, 0) blue.
    let screen = list_screen();
    *fixture.state.pixel_script.lock().expect("pixel script") =
        Some(Arc::new(move |_: usize, inputs: usize| {
            let mut pixels = screen.clone();
            if inputs > 0 {
                pixels[0].1 = [0, 0, 255];
            }
            pixels
        }));
    let chosen = run(
        &fixture.host,
        &fixture.temp,
        &single_frame_select_package(table_policy("exactly_one", literal_pin()), false),
    );
    assert_eq!(
        chosen.receipt.state(),
        RuntimeReceiptState::Completed,
        "{:?}",
        chosen.receipt.outcome()
    );
    assert_eq!(chosen.selected(), ["layout/cards#03"]);
    assert_eq!(fixture.state.input_count.load(Ordering::Acquire), 1);
    fixture.host.close().expect("close runtime host");
}

/// §4.3 for a single frame: nobody has a row, so each is dropped, none is an unknown instance, and
/// the step ends on its `on_empty` (P4), over the `fixed_slots` layout such a step needs; without
/// `on_empty` under `exactly_one` it is `selection_not_selected` (P5).
#[test]
fn db2_a_single_frame_select_drops_what_the_table_lacks_without_an_unknown_instance() {
    let fixture = Fixture::new();
    fixture.write_table_value(&table(json!([["member-99", "alpha", true, 5]])));
    let empty = run(
        &fixture.host,
        &fixture.temp,
        &single_frame_select_package(table_policy("none_allowed", literal_pin()), true),
    );
    assert_eq!(
        empty.receipt.state(),
        RuntimeReceiptState::Completed,
        "{:?}",
        empty.receipt.outcome()
    );
    assert_eq!(empty.selection(), (6, 0));

    let insufficient = run(
        &fixture.host,
        &fixture.temp,
        &single_frame_select_package(table_policy("exactly_one", literal_pin()), false),
    );
    assert_eq!(
        insufficient.receipt.outcome().code().as_str(),
        "selection_not_selected"
    );
    assert_eq!(fixture.state.input_count.load(Ordering::Acquire), 0);
    fixture.host.close().expect("close runtime host");
}

/// Coordinator decision 6 (#419): one instance's table failure denies its one run; the other
/// instance runs, a package that reads no table and, once the table is there, one that does.
#[test]
fn db2_a_table_failure_on_one_instance_leaves_the_other_instance_running() {
    let temp = TempDir::new().expect("tempdir");
    let (root, game) = game_root(&temp);
    let (failing, other) = (list_device(), list_device());
    let host = RuntimeHost::start(
        config(&temp).with_standard_packs_root(Some(root)),
        Arc::new(
            FakeProvider::from_entries([
                (ALIAS.to_owned(), instance_id(), Arc::clone(&failing)),
                (OTHER_ALIAS.to_owned(), instance_id(), Arc::clone(&other)),
            ])
            .with_vision_provider(Arc::new(CardNames)),
        ),
    )
    .expect("runtime host");
    let package = scan_select_package_with(table_policy("none_allowed", literal_pin()), true);

    let refused = run_on(&host, &temp, ALIAS, &package);
    assert_eq!(refused.receipt.state(), RuntimeReceiptState::Denied);
    assert_eq!(
        refused.receipt.outcome().code().as_str(),
        "standard_pack_table_missing"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());

    let plain = run_on(&host, &temp, OTHER_ALIAS, &scan_select_package());
    assert_eq!(
        plain.receipt.state(),
        RuntimeReceiptState::Completed,
        "{:?}",
        plain.receipt.outcome()
    );

    let bytes =
        serde_json::to_vec(&table(json!([["member-99", "alpha", true, 5]]))).expect("table JSON");
    write_table_at(&game, &bytes);
    for alias in [OTHER_ALIAS, ALIAS] {
        let reads = run_on(&host, &temp, alias, &package);
        assert_eq!(
            reads.receipt.state(),
            RuntimeReceiptState::Completed,
            "{alias}: {:?}",
            reads.receipt.outcome()
        );
    }
    assert_eq!(failing.input_count.load(Ordering::Acquire), 0);
    assert_eq!(other.input_count.load(Ordering::Acquire), 0);
    host.close().expect("close runtime host");
}
