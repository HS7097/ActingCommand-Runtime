// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-5C (register §1.287, "Slice LAB-5C"; contracts/lab-two-entries.md §9.1;
// coordinator work order rd-1108/arch-013/brief-lab5c.md): the arguments only today's MCP tools
// took become parameters of their rows; every command line that worked before works the same.
// - `lab5c_new_parameters_reach_their_function`: for every parameter the slice adds, a command
//   line that gives it reaches the function that serves its row with that value: the words the
//   row gives its command in `lab-ops` (`core_ops::offline_words`, `core_ops::handed_words`) and,
//   where `lab-ops` builds it before any connection, the query (`events`, through
//   `events::cli_request`), the window (`diagnose`, whose start too far back is refused) and the
//   page (`codes`, offline); `material`'s read (`core_ops::material_read`). For each row, a
//   command line without the new options gives what it gave at the base. (Fix 1: `app
//   launch|restart|stop` gain no `record`; their rows are main's.)
// - `remedy_set_takes_a_rows_file_or_rows_inline`: `file` alone goes on as before; `rows` alone
//   goes beside the words, and `lab-ops` reads it (another shape, and fix 1: rows larger than a
//   rows file may be, refused as a file is, before any connection); both are refused (`--rows`
//   `conflicting`); neither is refused as before (`<file>` `required`), on the command line and
//   in `serve --stdio` alike.
// - `material_out_writes_the_whole_material`: a made-up material is written into a temporary
//   directory and the line names the file.
// Fakes only: a state root in a temporary folder on which no Runtime runs; no device, no sleep.
// One case reads the system clock: `diagnose --since-unix-ms 1`, whose start `lab-ops` measures
// back from now; 1 ms after the epoch is more than seven days back from any clock later than
// seven days after the epoch, so the case is `since_out_of_range` whatever the clock reads.

use super::*;
use crate::cli_module::core_ops::{self, MaterialRead};
use crate::cli_module::{self, words};
use actingcommand_contract::EventQuery;
use actingcommand_contract::MAX_FACT_OBSERVATION_BYTES;
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_lab_ops::cli_line;
use actingcommand_lab_ops::events;
use actingcommand_lab_ops::initiator::ENVIRONMENT_VARIABLE;
use actingcommand_lab_ops::table::{Operation, OperationTable};
use serde_json::{Map, Value, json};
use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

/// The state root the words name; nothing reads it.
const STATE: &str = "state";
/// Typed identifiers, as the Runtime spells them.
const TASK: &str = "task_0123456789abcdef0123456789abcdef";
const CORRELATION: &str = "correlation_0123456789abcdef0123456789abcdef";
const REQUEST: &str = "request_0123456789abcdef0123456789abcdef";
const RUN: &str = "run_0123456789abcdef0123456789abcdef";
/// A material ref; only the Runtime would resolve it.
const MATERIAL: &str = "1:2:3";

fn words_of(line: &[&str]) -> Vec<String> {
    line.iter().map(|word| (*word).to_owned()).collect()
}

fn table() -> &'static OperationTable {
    cli_module::table().expect("the operation table")
}

/// The row and the parameters the words give `line`.
fn call(line: &[&str]) -> (&'static Operation, Map<String, Value>) {
    let parsed = words::parse(table(), &words_of(line)).unwrap_or_else(|outcome| {
        panic!(
            "{line:?}: {}",
            cli_line::outcome_value(&outcome, Detail::Full)
        )
    });
    let operation = table().operation(&parsed.name).expect("the call's row");
    (operation, parsed.params)
}

/// The refusal's `argument` and `argument_problem`.
fn refusal(outcome: &Outcome) -> (Value, Value) {
    let value = cli_line::outcome_value(outcome, Detail::Full);
    assert_eq!(value["code"], "actingctl_usage_invalid", "{value}");
    (
        value["values"]["argument"].clone(),
        value["values"]["argument_problem"].clone(),
    )
}

/// The words' refusal of `line`.
fn refused(line: &[&str]) -> (Value, Value) {
    match words::parse(table(), &words_of(line)) {
        Ok(call) => panic!("{line:?} must be refused, not read as {call:?}"),
        Err(outcome) => refusal(&outcome),
    }
}

/// The `serve --stdio` parameters of `request`, or its refusal.
fn served(request: &Value) -> Result<Value, (Value, Value)> {
    words::from_json(table(), &request.to_string())
        .map(|call| Value::Object(call.params))
        .map_err(|outcome| refusal(&outcome))
}

/// The words the row of `line` gives its command in `lab-ops` (the state root [`STATE`], no
/// install root).
fn handed(line: &[&str]) -> Vec<String> {
    let (operation, params) = call(line);
    let argv = match core_ops::offline_words(operation, &params, None) {
        Some(argv) => argv,
        None => match core_ops::handed_words(operation, &params, None, Path::new(STATE)) {
            Some(Ok(argv)) => argv,
            Some(Err(exit)) => panic!("{line:?} is refused: {:?}", exit.line),
            None => panic!("{line:?} gives lab-ops no words"),
        },
    };
    argv.into_iter()
        .map(|word| word.into_string().expect("UTF-8 words"))
        .collect()
}

/// The read `events` builds from the words the row of `line` gives it, before any connection.
fn events_request(line: &[&str]) -> events::Request {
    let argv = handed(line);
    assert!(
        argv.starts_with(&words_of(&["events", "--state-root", STATE])),
        "{line:?}: {argv:?}"
    );
    let after = argv[1..].iter().map(OsString::from).collect::<Vec<_>>();
    events::cli_request(&after)
        .unwrap_or_else(|error| panic!("{line:?}: {error}"))
        .1
}

/// The module's exit code and line for `line`.
fn module_line(line: &[&str]) -> (u8, Value) {
    let exit = cli_module::run_line(&words_of(line));
    (exit.code, exit.line.expect("the module's line"))
}

#[test]
fn lab5c_new_parameters_reach_their_function() {
    let _guard = env_lock();
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }

    // `events`: each filter MCP `ac_events` took goes on under its flag and is the query's.
    let filters: [(&[&str], &str, Value); 12] = [
        (&["events", "--view", "errors"], "view", json!("errors")),
        (
            &["events", "--since-unix-ms", "1000"],
            "from_timestamp_unix_ms",
            json!(1000),
        ),
        (
            &["events", "--until-unix-ms", "2000"],
            "to_timestamp_unix_ms",
            json!(2000),
        ),
        (
            &["events", "--through-sequence", "9"],
            "to_sequence",
            json!(9),
        ),
        (
            &["events", "--event-type", "runtime.failed"],
            "event_type",
            json!("runtime.failed"),
        ),
        (
            &[
                "events",
                "--exclude-event-types",
                "runtime.failed,fact.published",
            ],
            "exclude_event_types",
            json!(["runtime.failed", "fact.published"]),
        ),
        (
            &["events", "--fact-key", "application.foreground"],
            "fact_key",
            json!("application.foreground"),
        ),
        (
            &["events", "--min-severity", "warning"],
            "minimum_severity",
            json!("warning"),
        ),
        (
            &["events", "--max-severity", "error"],
            "maximum_severity",
            json!("error"),
        ),
        (
            &["events", "--origin-module", "actingctl"],
            "origin_module",
            json!("actingctl"),
        ),
        (&["events", "--task-id", TASK], "task_id", json!(TASK)),
        (
            &["events", "--correlation-id", CORRELATION],
            "correlation_id",
            json!(CORRELATION),
        ),
    ];
    for (line, member, value) in filters {
        assert_eq!(
            handed(line),
            words_of(&["events", "--state-root", STATE, line[1], line[2]]),
            "{line:?}"
        );
        let request = events_request(line);
        let query = serde_json::to_value(request.filters()).expect("the query");
        assert_eq!(query[member], value, "{line:?}: {query}");
    }
    // The detail and the wait go on too, and are the read's.
    assert_eq!(
        handed(&["events", "--detail", "summary", "--wait-s", "3"]),
        words_of(&[
            "events",
            "--state-root",
            STATE,
            "--detail",
            "summary",
            "--wait-s",
            "3"
        ])
    );
    let request = events_request(&["events", "--detail", "summary", "--wait-s", "3"]);
    assert_eq!(
        (request.detail, request.wait),
        (events::Detail::Summary, Duration::from_secs(3))
    );
    // Without them: the words the base gave, and the base's read (brief rows, no wait, no
    // filter).
    assert_eq!(
        handed(&[
            "events",
            "neutral.a",
            "--type",
            "a.b,c.d",
            "--run",
            RUN,
            "--request",
            REQUEST,
            "--after",
            "5",
            "--limit",
            "7"
        ]),
        words_of(&[
            "events",
            "--state-root",
            STATE,
            "--instance",
            "neutral.a",
            "--event-types",
            "a.b,c.d",
            "--run-id",
            RUN,
            "--request-id",
            REQUEST,
            "--after-sequence",
            "5",
            "--limit",
            "7"
        ])
    );
    let plain = events_request(&["events"]);
    assert_eq!(
        (plain.detail, plain.wait, plain.filters()),
        (
            events::Detail::Brief,
            Duration::ZERO,
            &EventQuery::default()
        )
    );

    // `diagnose`: the window's start in unix ms goes on under its flag; without it, as before.
    assert_eq!(
        handed(&[
            "diagnose",
            "neutral.a",
            "--task",
            TASK,
            "--since-unix-ms",
            "1"
        ]),
        words_of(&[
            "diagnose",
            "--state-root",
            STATE,
            "--instance",
            "neutral.a",
            "--task-id",
            TASK,
            "--since-unix-ms",
            "1"
        ])
    );
    assert_eq!(
        handed(&["diagnose", "neutral.a", "--task", TASK]),
        words_of(&[
            "diagnose",
            "--state-root",
            STATE,
            "--instance",
            "neutral.a",
            "--task-id",
            TASK
        ])
    );

    // `codes`: the list's filters and page go on under their flags, after the code; without
    // them, as before.
    assert_eq!(
        handed(&[
            "codes",
            "--prefix",
            "actingctl_",
            "--owner",
            "neutral.owner",
            "--limit",
            "2"
        ]),
        words_of(&[
            "codes",
            "--prefix",
            "actingctl_",
            "--owner",
            "neutral.owner",
            "--limit",
            "2"
        ])
    );
    assert_eq!(
        handed(&["codes", "actingctl_usage_invalid"]),
        words_of(&["codes", "actingctl_usage_invalid"])
    );
    assert_eq!(handed(&["codes"]), words_of(&["codes"]));

    // `packs`: one role's rows, after the instance and the install root; without it, as before.
    let (operation, params) = call(&["packs", "neutral.a", "--role", "startup"]);
    assert_eq!(
        core_ops::offline_words(operation, &params, Some(Path::new("root"))),
        Some(
            [
                "packs",
                "--instance",
                "neutral.a",
                "--root",
                "root",
                "--role",
                "startup"
            ]
            .map(OsString::from)
            .to_vec()
        )
    );
    assert_eq!(
        handed(&["packs", "neutral.a"]),
        words_of(&["packs", "--instance", "neutral.a"])
    );
    // A role the operation layer does not know is refused by name, as MCP refuses it.
    assert_eq!(
        refused(&["packs", "--role", "neutral"]),
        (json!("--role"), json!("not_recognised"))
    );

    // `material`: `out` asks for the whole material; without it, the slice as before.
    let (_, params) = call(&["material", MATERIAL]);
    assert!(matches!(
        core_ops::material_read(&params),
        Ok(MaterialRead::Text {
            reference: MATERIAL,
            offset: 0,
            max_bytes: 4096
        })
    ));
    let (_, params) = call(&["material", MATERIAL, "--offset", "5", "--max-bytes", "10"]);
    assert!(matches!(
        core_ops::material_read(&params),
        Ok(MaterialRead::Text {
            reference: MATERIAL,
            offset: 5,
            max_bytes: 10
        })
    ));
    let temp = TempDir::new().unwrap();
    let out = temp.path().join("exports");
    let out_word = out.to_str().expect("a UTF-8 path");
    let (_, params) = call(&["material", MATERIAL, "--out", out_word]);
    match core_ops::material_read(&params) {
        Ok(read) => assert_eq!(
            read,
            MaterialRead::Whole {
                reference: MATERIAL,
                out: out.clone()
            }
        ),
        Err(exit) => panic!("--out is refused: {:?}", exit.line),
    }
    // The slice's offset and size are refused beside `out` (`conflicting`). The row reads them
    // in `material_read`, after the connection is made, as the base checked `--max-bytes`'
    // range: with no Runtime running, the line is the connection's failure.
    for (flag, value) in [("--offset", "5"), ("--max-bytes", "10")] {
        let (_, params) = call(&["material", MATERIAL, "--out", out_word, flag, value]);
        match core_ops::material_read(&params) {
            Ok(read) => panic!("{flag} beside --out is read as {read:?}"),
            Err(exit) => {
                let line = exit.line.expect("the refusal's line");
                assert_eq!(exit.code, 1, "{line}");
                assert_eq!(
                    (
                        line["outcome"]["values"]["argument"].clone(),
                        line["outcome"]["values"]["argument_problem"].clone()
                    ),
                    (json!(flag), json!("conflicting")),
                    "{line}"
                );
            }
        }
    }

    // Through the module, on a state root no Runtime runs on: `lab-ops` reads the values and
    // refuses them by its own rules before any connection.
    let state_root = temp.path().join("state");
    fs::create_dir_all(&state_root).unwrap();
    let _state = use_runtime_state_root(&state_root);
    // `diagnose`'s window starts more than seven days back.
    let (code, line) = module_line(&["diagnose", "--since-unix-ms", "1"]);
    assert_eq!(
        (code, line["outcome"]["code"].clone()),
        (1, json!("since_out_of_range")),
        "{line}"
    );
    // `codes`, offline: the page the filters and the size ask for.
    let (code, line) = module_line(&["codes", "--prefix", "actingctl_", "--limit", "2"]);
    assert_eq!(code, 0, "{line}");
    let page = line["codes"].as_array().expect("the page");
    assert_eq!(page.len(), 2, "{line}");
    assert!(
        page.iter().all(|row| row["code"]
            .as_str()
            .is_some_and(|code| code.starts_with("actingctl_"))),
        "{line}"
    );
    assert!(line["next_cursor"].is_string(), "{line}");
    let (code, line) = module_line(&["codes", "--owner", "neutral.owner"]);
    assert_eq!((code, line["codes"].clone()), (0, json!([])), "{line}");
    // A list flag beside a code is the operation layer's `conflicting`.
    let (code, line) = module_line(&["codes", "actingctl_usage_invalid", "--prefix", "a"]);
    assert_eq!(code, 1, "{line}");
    assert_eq!(
        (
            line["outcome"]["values"]["argument"].clone(),
            line["outcome"]["values"]["argument_problem"].clone()
        ),
        (json!("--prefix"), json!("conflicting")),
        "{line}"
    );
}

#[test]
fn remedy_set_takes_a_rows_file_or_rows_inline() {
    let _guard = env_lock();
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    let rows = r#"[{"row":"none","need":"neutral.need"}]"#;

    // `file` alone: the words and the line `lab-ops` gets, as before.
    let (_, params) = call(&["remedy", "set", "--instance", "neutral.a", "rows.json"]);
    assert_eq!(
        Value::Object(params),
        json!({"instance": "neutral.a", "file": "rows.json"})
    );
    assert_eq!(
        handed(&["remedy", "set", "--instance", "neutral.a", "rows.json"]),
        words_of(&[
            "remedy",
            "set",
            "--state-root",
            STATE,
            "--instance",
            "neutral.a",
            "--rows",
            "rows.json"
        ])
    );
    // `rows` alone: the JSON goes beside the words, which carry no `--rows`.
    let (_, params) = call(&["remedy", "set", "--game", "neutral", "--rows", rows]);
    assert_eq!(
        Value::Object(params),
        json!({"game": "neutral", "rows": [{"row": "none", "need": "neutral.need"}]})
    );
    assert_eq!(
        handed(&["remedy", "set", "--game", "neutral", "--rows", rows]),
        words_of(&["remedy", "set", "--state-root", STATE, "--game", "neutral"])
    );
    // Both are refused; neither is refused as before; `serve --stdio` alike.
    assert_eq!(
        refused(&[
            "remedy",
            "set",
            "--game",
            "neutral",
            "rows.json",
            "--rows",
            rows
        ]),
        (json!("--rows"), json!("conflicting"))
    );
    assert_eq!(
        refused(&["remedy", "set", "--game", "neutral"]),
        (json!("<file>"), json!("required"))
    );
    assert_eq!(
        served(
            &json!({"op": "remedy set", "params": {"game": "neutral", "file": "rows.json", "rows": []}})
        ),
        Err((json!("--rows"), json!("conflicting")))
    );
    assert_eq!(
        served(&json!({"op": "remedy set", "params": {"game": "neutral"}})),
        Err((json!("<file>"), json!("required")))
    );
    assert_eq!(
        served(&json!({"op": "remedy set", "params": {"game": "neutral", "rows": []}})),
        Ok(json!({"game": "neutral", "rows": []}))
    );

    // Through the module, on a state root no Runtime runs on.
    let temp = TempDir::new().unwrap();
    let state_root = temp.path().join("state");
    fs::create_dir_all(&state_root).unwrap();
    let _state = use_runtime_state_root(&state_root);
    let usage = |line: &[&str]| {
        let (code, answered) = module_line(line);
        assert_eq!(code, 1, "{line:?}: {answered}");
        assert_eq!(
            answered["outcome"]["code"], "actingctl_usage_invalid",
            "{answered}"
        );
        (
            answered["outcome"]["values"]["argument"].clone(),
            answered["outcome"]["values"]["argument_problem"].clone(),
        )
    };
    // A rows file `lab-ops` cannot read: its refusal, as before.
    let missing = temp.path().join("missing.json");
    let missing = missing.to_str().expect("a UTF-8 path");
    assert_eq!(
        usage(&["remedy", "set", "--game", "neutral", missing]),
        (json!("--rows"), json!("unusable"))
    );
    // Rows inline of another shape: `lab-ops` refuses them as a rows file holding them.
    assert_eq!(
        usage(&["remedy", "set", "--game", "neutral", "--rows", "[1]"]),
        (json!("--rows"), json!("wrong_type"))
    );
    // Fix 1: rows inline larger than a rows file may be are refused as a larger file is, with
    // its bound in bytes, before anything is sent.
    let large = json!([{ "row": "x".repeat(MAX_FACT_OBSERVATION_BYTES) }]).to_string();
    let (code, line) = module_line(&[
        "remedy",
        "set",
        "--game",
        "neutral",
        "--rows",
        large.as_str(),
    ]);
    let outcome = &line["outcome"];
    assert_eq!(
        (
            code,
            outcome["code"].clone(),
            outcome["values"]["argument"].clone(),
            outcome["values"]["argument_problem"].clone(),
            outcome["values"]["range_max"].clone()
        ),
        (
            1,
            json!("actingctl_usage_invalid"),
            json!("--rows"),
            json!("out_of_range"),
            json!(MAX_FACT_OBSERVATION_BYTES)
        ),
        "{outcome}"
    );
    // Rows inline of the rows' shape are taken: the line goes on to the Runtime, whose
    // `runtime-info.json` the state root does not hold.
    let (code, line) = module_line(&["remedy", "set", "--game", "neutral", "--rows", rows]);
    assert_eq!(code, 1, "{line}");
    assert_ne!(line["outcome"]["code"], "actingctl_usage_invalid", "{line}");
    assert!(
        line["outcome"]
            .to_string()
            .contains("\"runtime_info_unavailable\""),
        "{line}"
    );
}

#[test]
fn material_out_writes_the_whole_material() {
    let _guard = env_lock();
    let temp = TempDir::new().unwrap();
    let out = temp.path().join("exports");
    let bytes = br#"{"neutral":"material"}"#;
    let hex = "ab".repeat(32);
    let sha256 = format!("sha256:{hex}");
    let exit = core_ops::material_written(&out, &sha256, "json", bytes);
    let line = exit.line.expect("the line");
    assert_eq!(exit.code, 0, "{line}");
    let written = out.join(format!("{hex}.json"));
    assert_eq!(
        (
            line["outcome"]["code"].clone(),
            line["path"].clone(),
            line["sha256"].clone(),
            line["size"].clone()
        ),
        (
            json!("lab_operation_completed"),
            json!(written.display().to_string()),
            json!(sha256),
            json!(bytes.len())
        ),
        "{line}"
    );
    assert_eq!(fs::read(&written).unwrap(), bytes.to_vec());
}
