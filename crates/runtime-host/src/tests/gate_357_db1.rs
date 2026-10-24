// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #357 DB-1 (model M-357a v1.1 §2.1, §2.3, §2.4; §8, the DB-1 test list; register
//! §1.65 TB2, TB3): the shared tables of a standard-pack game root as the Runtime's startup and
//! check-config's report read them. A table that cannot be read is
//! `standard_pack_table_unreadable`; one bad table leaves its root registered, adds one record
//! and the Runtime starts; a directory with only `tables/` is `bundle_missing`; a broken root's
//! tables are neither read nor listed; the 65th table by file name is `table_too_large` and the
//! first 64 are read. Synthetic `neutral` games and neutral tables only.

use super::*;
use actingcommand_contract::outcome::{Category, Outcome};
use actingcommand_contract::{RuntimeLifecyclePhase, RuntimePayload};
use serde_json::{Value, json};

const ALIAS: &str = "neutral.instance";
/// The instant the reports judge column expiries at (2026-01-01T00:00:00Z); no wall clock.
const NOW_UNIX_MS: u64 = 1_767_225_600_000;

/// A bundle index v4 of `game` with one pack.
fn bundle(game: &str) -> Value {
    json!({
        "schema_version": "actingcommand.bundle.v4",
        "game": game,
        "packs": [{"package_id": format!("{game}.test.task"), "server": "test",
                   "entry_task_id": "task", "path": format!("packs/{game}.test.task"),
                   "file_count": 1, "byte_count": 1}]
    })
}

/// A valid table `table_id` of `game`: a key column and a nullable score column, `rows` rows.
fn table(game: &str, table_id: &str, rows: usize) -> Value {
    json!({
        "schema_version": "actingcommand.table.v1",
        "game": game,
        "table_id": table_id,
        "row_key": ["unit"],
        "columns": [
            {"name": "unit", "type": "key"},
            {"name": "score_milli", "type": "integer", "min": 0, "max": 100000, "nullable": true}
        ],
        "rows": (0..rows)
            .map(|index| json!([format!("unit.{index:03}"), if index == 0 { Value::Null } else { json!(1000) }]))
            .collect::<Vec<_>>()
    })
}

fn write_json(path: &Path, value: &Value) {
    fs::create_dir_all(path.parent().expect("a parent directory")).expect("create directory");
    fs::write(path, serde_json::to_vec(value).expect("fixture JSON")).expect("write fixture");
}

/// The standard-pack root below `temp` with three game roots:
/// - `broken`: `bundle.json` and `interrupts.json` without `pages.json`, and a bad table;
/// - `neutral`: `bundle.json` and three tables: `unit_ranks` (valid), `unit_scores` (two rows
///   with one row key) and `zone_marks.json`, a directory, which cannot be read as a file;
/// - `orphan`: only `tables/` with a valid table.
fn three_game_roots(temp: &TempDir) -> PathBuf {
    let root = temp.path().join("packages");
    let broken = root.join("broken");
    write_json(&broken.join("bundle.json"), &bundle("broken"));
    write_json(
        &broken.join("interrupts.json"),
        &json!({"schema_version": "actingcommand.interrupts.v1", "game": "broken", "interrupts": []}),
    );
    let mut duplicate = table("broken", "unit_scores", 2);
    duplicate["rows"][1][0] = json!("unit.000");
    write_json(&broken.join("tables/unit_scores.json"), &duplicate);

    let neutral = root.join("neutral");
    write_json(&neutral.join("bundle.json"), &bundle("neutral"));
    write_json(
        &neutral.join("tables/unit_ranks.json"),
        &table("neutral", "unit_ranks", 3),
    );
    let mut duplicate = table("neutral", "unit_scores", 2);
    duplicate["rows"][1][0] = json!("unit.000");
    write_json(&neutral.join("tables/unit_scores.json"), &duplicate);
    fs::create_dir_all(neutral.join("tables/zone_marks.json")).expect("a directory in tables/");

    write_json(
        &root.join("orphan/tables/unit_ranks.json"),
        &table("orphan", "unit_ranks", 1),
    );
    root
}

fn start(temp: &TempDir, root: &Path) -> RuntimeHost {
    RuntimeHost::start(
        config(temp).with_standard_packs_root(Some(root.to_owned())),
        Arc::new(FakeProvider::one(
            ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("the Runtime starts")
}

/// The outcomes of the `standard_pack_read` lifecycle records, in ledger order.
fn standard_pack_reads(host: &RuntimeHost) -> Vec<Outcome> {
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(EventType::RuntimeLifecycleObserved),
        ..EventQuery::default()
    })
    .expect("lifecycle events")
    .iter()
    .filter_map(|event| match event.payload() {
        EventPayload::Runtime(RuntimePayload::LifecycleObserved(value)) => match value.phase() {
            RuntimeLifecyclePhase::StandardPackRead { outcome } => Some(outcome.clone()),
            _ => None,
        },
        _ => None,
    })
    .collect()
}

fn text<'a>(outcome: &'a Outcome, key: &str) -> Option<&'a str> {
    outcome.value(key).and_then(Value::as_str)
}

fn ends_with(outcome: &Outcome, suffix: &[&str]) -> bool {
    let expected = suffix.iter().collect::<PathBuf>();
    text(outcome, "path").is_some_and(|path| Path::new(path).ends_with(&expected))
}

fn cause_codes(outcome: &Outcome) -> Vec<&str> {
    outcome
        .causes()
        .iter()
        .map(|link| link.outcome().code().as_str())
        .collect()
}

/// One bad table: its root is registered as usual, the table adds one record and the Runtime
/// starts; a table that cannot be read is `standard_pack_table_unreadable`; a directory with
/// only `tables/` is `bundle_missing`; the broken root's table adds no record.
#[test]
fn db1_startup_records_each_refused_table_and_the_runtime_starts() {
    let temp = TempDir::new().expect("tempdir");
    let root = three_game_roots(&temp);
    let host = start(&temp, &root);
    let reads = standard_pack_reads(&host);
    let [broken, registered, invalid, unreadable, orphan] = reads.as_slice() else {
        panic!("one record per game root and per refused table of a sound root: {reads:?}");
    };

    assert_eq!(broken.code().as_str(), "standard_pack_root_invalid");
    assert_eq!(text(broken, "game"), Some("broken"));
    assert_eq!(text(broken, "standard_pack_problem"), Some("pages_missing"));

    assert_eq!(registered.code().as_str(), "standard_pack_registered");
    assert_eq!(text(registered, "game"), Some("neutral"));

    assert_eq!(invalid.code().as_str(), "standard_pack_table_invalid");
    assert_eq!(invalid.category(), Category::Error);
    assert_eq!(text(invalid, "game"), Some("neutral"));
    assert!(
        ends_with(invalid, &["tables", "unit_scores.json"]),
        "{invalid:?}"
    );
    assert_eq!(
        text(invalid, "standard_pack_problem"),
        Some("table_row_duplicate")
    );
    assert_eq!(text(invalid, "json_pointer"), Some("/rows/1"));

    assert_eq!(unreadable.code().as_str(), "standard_pack_table_unreadable");
    assert_eq!(unreadable.category(), Category::Error);
    assert_eq!(text(unreadable, "game"), Some("neutral"));
    assert!(
        ends_with(unreadable, &["tables", "zone_marks.json"]),
        "{unreadable:?}"
    );
    assert_eq!(cause_codes(unreadable), ["foreign_os_error"]);

    assert_eq!(orphan.code().as_str(), "standard_pack_root_invalid");
    assert_eq!(text(orphan, "game"), Some("orphan"));
    assert_eq!(
        text(orphan, "standard_pack_problem"),
        Some("bundle_missing")
    );
    assert!(ends_with(orphan, &["orphan", "bundle.json"]), "{orphan:?}");
    host.close().expect("close runtime host");
}

/// check-config's report of the same root: the sound root lists its tables in file name order
/// (row count, or the refusal), the broken root and the directory with only `tables/` list
/// none, and only the sound root's refused tables are notes.
#[test]
fn db1_check_config_lists_only_a_sound_roots_tables() {
    let temp = TempDir::new().expect("tempdir");
    let root = three_game_roots(&temp);
    let report = inspect_standard_packs(Some(root.as_path()), NOW_UNIX_MS);
    let games = report
        .games
        .iter()
        .map(|game| game.game.as_str())
        .collect::<Vec<_>>();
    assert_eq!(games, ["broken", "neutral", "orphan"]);
    let [broken, neutral, orphan] = report.games.as_slice() else {
        panic!("three game roots: {games:?}");
    };
    assert!(broken.problem.is_some());
    assert!(
        broken.tables.is_none(),
        "a broken root's tables are not listed"
    );
    assert!(orphan.problem.is_some());
    assert!(
        orphan.tables.is_none(),
        "a broken root's tables are not listed"
    );

    let tables = neutral
        .tables
        .as_ref()
        .expect("a sound root lists its tables");
    let ids = tables
        .iter()
        .map(|table| table.table_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, ["unit_ranks", "unit_scores", "zone_marks"]);
    assert_eq!(tables[0].row_count, Some(3));
    assert_eq!(tables[0].problem, None);
    assert_eq!(tables[0].expired_columns, None);
    assert_eq!(tables[1].row_count, None);
    let invalid = tables[1]
        .problem
        .as_ref()
        .expect("the refused table's problem");
    assert_eq!(invalid.code().as_str(), "standard_pack_table_invalid");
    assert_eq!(
        text(invalid, "standard_pack_problem"),
        Some("table_row_duplicate")
    );
    let unreadable = tables[2]
        .problem
        .as_ref()
        .expect("the unreadable table's problem");
    assert_eq!(unreadable.code().as_str(), "standard_pack_table_unreadable");

    let notes = report
        .table_notes()
        .iter()
        .map(|note| {
            (
                note.code().as_str().to_owned(),
                text(note, "game").map(str::to_owned),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        notes,
        [
            (
                "standard_pack_table_invalid".to_owned(),
                Some("neutral".to_owned())
            ),
            (
                "standard_pack_table_unreadable".to_owned(),
                Some("neutral".to_owned())
            ),
        ]
    );
}

/// A game root with 65 tables: the first 64 by file name are read, the 65th is
/// `table_too_large`.
#[test]
fn db1_the_65th_table_is_too_large_and_the_first_64_are_read() {
    let temp = TempDir::new().expect("tempdir");
    let root = temp.path().join("packages");
    let game = root.join("neutral");
    write_json(&game.join("bundle.json"), &bundle("neutral"));
    for index in 0..=64 {
        let table_id = format!("t{index:02}");
        write_json(
            &game.join("tables").join(format!("{table_id}.json")),
            &table("neutral", &table_id, 1),
        );
    }
    let report = inspect_standard_packs(Some(root.as_path()), NOW_UNIX_MS);
    let [neutral] = report.games.as_slice() else {
        panic!("one game root: {:?}", report.games);
    };
    let tables = neutral
        .tables
        .as_ref()
        .expect("a sound root lists its tables");
    assert_eq!(tables.len(), 65);
    for table in &tables[..64] {
        assert_eq!(table.row_count, Some(1), "{table:?}");
        assert_eq!(table.problem, None, "{table:?}");
    }
    assert_eq!(tables[64].table_id, "t64");
    assert_eq!(tables[64].row_count, None);
    let too_large = tables[64]
        .problem
        .as_ref()
        .expect("the 65th table's problem");
    assert_eq!(too_large.code().as_str(), "standard_pack_table_invalid");
    assert_eq!(
        text(too_large, "standard_pack_problem"),
        Some("table_too_large")
    );
}
