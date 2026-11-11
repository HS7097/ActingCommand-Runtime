// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #429 R-8 (register §1.306; contracts/lab-operations.json, the rows `pack-schedule` and
// `pack-schedule set`; coordinator work order rd-1111/brief-r8.md): the Lab's two core rows of the
// pack schedule reach their requests, as the neighbouring core rows reach theirs (`gate_429_r6`).
// - `r8_pack_schedule_rows_reach_their_requests`: both rows are offered with their one sentence
//   and name `PackScheduleView` and `EditPackSchedule`; their command lines are the CLI module's;
//   their short words and a `serve --stdio` line with the same parameters give the same call; the
//   scope word is `instance:<inst>`, `server:<id>` or `game:<id>` (another is refused), and the
//   rows are the edit's rows (an open-ended one included; another shape is refused).
// Reads only the table this build embeds: no state root, no Runtime, no device, no clock.

use crate::cli_module::{self, Route, words};
use actingcommand_contract::{PackScheduleEditRow, RuntimeOperationKind};
use actingcommand_lab_ops::core_ops::{self, ScheduleScope};
use serde_json::{Value, json};

fn words_of(line: &[&str]) -> Vec<String> {
    line.iter().map(|word| (*word).to_owned()).collect()
}

#[test]
fn r8_pack_schedule_rows_reach_their_requests() {
    let table = cli_module::table().expect("the operation table");
    for (name, kind) in [
        ("pack-schedule", RuntimeOperationKind::PackScheduleView),
        ("pack-schedule set", RuntimeOperationKind::EditPackSchedule),
    ] {
        let row = table
            .operation(name)
            .unwrap_or_else(|| panic!("the row {name}"));
        assert!(words::offered(row), "{name} is offered");
        let help = row.help.as_deref().unwrap_or_default();
        assert!(!help.trim().is_empty(), "{name} says what it does");
        assert!(row.requests.contains(&kind), "{name}: {:?}", row.requests);
    }

    // The read.
    let line = ["pack-schedule", "node.a"];
    match cli_module::route(words_of(&line)) {
        Route::Module(module) => assert_eq!(module, words_of(&line)),
        other => panic!("{line:?} is the CLI module's: {other:?}"),
    }
    let cli = words::parse(table, &words_of(&line)).expect("the words");
    let params = json!({"instance": "node.a"});
    let request = json!({"op": "pack-schedule", "params": params}).to_string();
    assert_eq!(cli.name, "pack-schedule");
    assert_eq!(
        words::from_json(table, &request).expect("the serve line"),
        cli
    );
    assert_eq!(Value::Object(cli.params), params);

    // The edit.
    let rows = r#"[{"from_unix_ms":1000,"until_unix_ms":2000,"schedulable":false},{"from_unix_ms":3000,"schedulable":true}]"#;
    let line = [
        "pack-schedule",
        "set",
        "instance:node.a",
        "neutral.pack.a",
        "--rows",
        rows,
        "--expected-version",
        "7",
    ];
    match cli_module::route(words_of(&line)) {
        Route::Module(module) => assert_eq!(module, words_of(&line)),
        other => panic!("{line:?} is the CLI module's: {other:?}"),
    }
    let cli = words::parse(table, &words_of(&line)).expect("the words");
    let params = json!({
        "scope": "instance:node.a",
        "package_id": "neutral.pack.a",
        "rows": [
            {"from_unix_ms": 1000, "until_unix_ms": 2000, "schedulable": false},
            {"from_unix_ms": 3000, "schedulable": true}
        ],
        "expected_version": 7
    });
    let request = json!({"op": "pack-schedule set", "params": params}).to_string();
    let served = words::from_json(table, &request).expect("the serve line");
    assert_eq!(cli.name, "pack-schedule set");
    assert_eq!(served, cli);
    assert_eq!(Value::Object(cli.params.clone()), params);
    assert_eq!(
        core_ops::pack_schedule_rows(&cli.params).expect("the rows"),
        vec![
            PackScheduleEditRow {
                from_unix_ms: 1000,
                until_unix_ms: Some(2000),
                schedulable: false,
            },
            PackScheduleEditRow {
                from_unix_ms: 3000,
                until_unix_ms: None,
                schedulable: true,
            },
        ]
    );
    let mut other_shape = cli.params.clone();
    other_shape.insert(
        "rows".to_owned(),
        json!([{"from_unix_ms": 1000, "schedulable": false, "note": "x"}]),
    );
    assert!(core_ops::pack_schedule_rows(&other_shape).is_err());

    // The scope word.
    for (word, scope) in [
        ("instance:node.a", ScheduleScope::Instance("node.a")),
        (
            "server:neutral-server",
            ScheduleScope::Server("neutral-server"),
        ),
        ("game:neutral-game", ScheduleScope::Game("neutral-game")),
    ] {
        assert_eq!(core_ops::pack_schedule_scope(word).expect(word), scope);
    }
    for word in ["node.a", "instance:", "region:neutral"] {
        assert!(core_ops::pack_schedule_scope(word).is_err(), "{word}");
    }
}
