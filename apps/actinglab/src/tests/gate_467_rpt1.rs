// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #467 RPT-1 (register §1.379; contracts/problem-report.md; the coordinator's ruling Q6):
// actinglab's placeholder `report export --last-error --out <path>`, which wrote an archive that
// held no error data, is gone, and its words are the operation table's row, which the CLI module
// serves.
// - `rpt1_report_export_is_the_tables_row_and_the_placeholder_is_gone`: the words are no command
//   of actinglab's own (not a group of its dispatch, not in the inventory
//   `ratchet/actinglab_commands.json`, not among the help's own commands) and the row's slice is
//   built; `main`'s route gives them to the module, whose words read the row's parameters; the
//   placeholder's `--last-error` is refused by name before anything is read or written; an output
//   inside the state root is refused before the Runtime is reached; without a Runtime the row
//   answers the client's failure and writes nothing.
// No Runtime, no device: an isolated environment whose Runtime state root does not exist.

use super::env_lock;
use super::gate_434_lab5b4::Isolated;
use crate::cli_help::OWN_WITHOUT_ROW;
use crate::cli_module::{self, Route, words};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

fn owned(line: &[&str]) -> Vec<String> {
    line.iter().map(|word| (*word).to_owned()).collect()
}

/// What the CLI module answers the command line `line`: its line and its exit code.
fn module_answer(line: &[&str]) -> (Value, u8) {
    let exit = cli_module::run_line(&owned(line));
    let printed = exit
        .line
        .unwrap_or_else(|text| panic!("{line:?} printed no line: {text}"));
    (printed, exit.code)
}

#[test]
fn rpt1_report_export_is_the_tables_row_and_the_placeholder_is_gone() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let out = isolated.path("reports");

    // No command of actinglab's own: not its dispatch, not its inventory, not its help's own
    // commands; the row's slice is one actinglab builds.
    assert!(!crate::own_command(&owned(&["report", "export"])));
    let inventory: Value = serde_json::from_slice(
        &fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ratchet/actinglab_commands.json"),
        )
        .expect("the command inventory"),
    )
    .expect("the command inventory parses");
    assert!(
        !inventory["commands"]
            .as_array()
            .expect("its commands")
            .contains(&json!("report export"))
    );
    assert!(
        !inventory["dispatch_arms"]
            .as_array()
            .expect("its dispatch arms")
            .contains(&json!("report <subcommand>"))
    );
    assert!(
        OWN_WITHOUT_ROW
            .iter()
            .all(|own| own.words != "report export")
    );
    assert!(words::BUILT_SLICES.contains(&"RPT-1"));

    // `main`'s route gives the words to the module, which reads the row's parameters.
    match cli_module::route(owned(&["report", "export", "--out", out.as_str()])) {
        Route::Module(_) => {}
        other => panic!("report export is the module's: {other:?}"),
    }
    let table = cli_module::table().expect("the operation table");
    let line = [
        "report",
        "export",
        "alpha",
        "--out",
        out.as_str(),
        "--no-frames",
        "--max-runs",
        "3",
        "--since",
        "48h",
    ];
    let call = words::parse(table, &owned(&line))
        .unwrap_or_else(|outcome| panic!("{line:?} is read: {outcome}"));
    assert_eq!(call.name, "report export");
    assert_eq!(
        Value::Object(call.params),
        json!({
            "instance": "alpha",
            "out": out,
            "no_frames": true,
            "max_runs": 3,
            "since": "48h",
        })
    );

    // The placeholder's switch is refused by name, before anything is read or written.
    let (answer, code) =
        module_answer(&["report", "export", "--last-error", "--out", out.as_str()]);
    assert_eq!(code, 1, "{answer}");
    assert_eq!(
        answer["outcome"]["code"], "actingctl_usage_invalid",
        "{answer}"
    );
    assert_eq!(
        answer["outcome"]["values"]["argument"], "--last-error",
        "{answer}"
    );
    assert_eq!(
        answer["outcome"]["values"]["argument_problem"], "unknown",
        "{answer}"
    );
    assert!(!Path::new(&out).exists());

    // An output inside the state root is refused before the Runtime is reached.
    let inside = isolated.runtime.join("reports").display().to_string();
    let (answer, code) = module_answer(&["report", "export", "--out", inside.as_str()]);
    assert_eq!(code, 1, "{answer}");
    assert_eq!(
        answer["outcome"]["code"], "report_output_refused",
        "{answer}"
    );
    assert!(
        answer["outcome"]["values"].get("state_root").is_some(),
        "{answer}"
    );

    // Without a Runtime the row answers the client's failure; nothing is written anywhere.
    let (answer, code) = module_answer(&["report", "export", "--out", out.as_str()]);
    assert_ne!(code, 0, "{answer}");
    assert_ne!(answer["outcome"]["code"], "report_exported", "{answer}");
    assert!(!Path::new(&out).exists());
    assert!(!isolated.runtime.exists());
}
