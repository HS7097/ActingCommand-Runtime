// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-5G4c (register §1.287, "Slice LAB-5G4c"; coordinator work order
// rd-1113/brief-lab5g4c.md; the coordinator's ruling 3 of 6775192250 on board #434, not the
// owner's words): `record draft` is a command of actinglab, MCP's `ac_binding_draft` ported.
// - `record_draft_answers_the_binding_parts`: a made-up `record stop` answer given inline; the four
//   binding parts come back verbatim; with `binding_requires` a list, `manual_steps` is that list
//   and the manual step, and no `binding_requires_unavailable`; absent or null, `binding_requires`
//   is null, the warning is there and `manual_steps` is the manual step alone.
// - `record_draft_refuses_what_the_tool_refuses`: neither or both of the two options
//   (`--record-stop` `conflicting`); an answer without `lab`, without one of its three parts, with a
//   package path that begins with `--`, or with a `binding_requires` that is no list
//   (`--record-stop` `unusable`); text that is not JSON (`wrong_type`); an export that is not
//   `{path, sha256, size?}` (`--record-stop-export` `wrong_type`) or names no file (`unusable`).
//   Each `actingctl_usage_invalid` with its argument and problem, exit 1. Nothing is written: the
//   place is outside any installation, where arguments that pass are refused
//   `lab_tool_unavailable` (`mcp_blocker` `install_root`); each refusal comes before it, so
//   nothing is located, run or exported.
// - `record_draft_reads_only_checked_exports`: an export in the export directory, read with its
//   sha256 (with or without the `sha256:` prefix); the same bytes with a wrong sha256, in a file
//   outside the directory and in a subdirectory of it are `--record-stop-export` `unusable`. The
//   export directory is today's, as today's tool's tests use it (`gate_mcpt1`).
// - `record_draft_without_daemon_program_says_so`: in a made-up installation whose program root has
//   no daemon program, `check_config` is `{error}` (`check_config_failed` with the configuration's
//   path, class runtime), the warning `check_config_unavailable` names the configuration, and the
//   rest of the answer stands.
// - `serve_answers_record_draft_as_the_command_line`: the `serve --stdio` loop answers the line
//   the command line prints for the same parameters, a draft and a refusal.
// Fakes only: made-up installation roots and exports in temporary folders; no Runtime, no device,
// no clock, no sleep.

use super::env_lock;
use super::gate_434_lab5b4::{Isolated, printed};
use crate::cli_module::{self, authoring, serve};
use crate::record_draft;
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_lab_ops::cli_line;
use actingcommand_lab_ops::codes::ActingctlCode;
use actingcommand_lab_ops::material_export;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

/// The three parts of a `record stop` answer's `lab` the draft requires, beside `binding_requires`.
const PARTS: [&str; 3] = [
    "binding_example",
    "prerequisite_entry_example",
    "catalog_on_failure_example",
];
/// The manual step, today's sentence.
const MANUAL_STEP: &str = "edit the configuration, approve in the UI, restart the daemon";

fn text_of(path: &Path) -> &str {
    path.to_str().expect("a UTF-8 path")
}

/// A made-up single-location installation root under `base` (`runtime\BUILD-MANIFEST.json`, the
/// root's `actingd.config.json` naming an absolute state root) without a daemon program: the
/// program placed as its `runtime\actinglab.exe`, and the configuration.
fn made_up_root(base: &Path) -> (PathBuf, PathBuf) {
    let root = base.join("install-root");
    let runtime = root.join("runtime");
    fs::create_dir_all(&runtime).unwrap();
    fs::write(runtime.join("BUILD-MANIFEST.json"), b"{}\n").unwrap();
    let config = root.join("actingd.config.json");
    let document = json!({"state_root": text_of(&base.join("state"))});
    fs::write(&config, document.to_string()).unwrap();
    (runtime.join("actinglab.exe"), config)
}

/// A program outside any installation root.
fn unrooted(base: &Path) -> PathBuf {
    let unpacked = base.join("unpacked-tools");
    fs::create_dir_all(&unpacked).unwrap();
    unpacked.join("actinglab.exe")
}

/// The `lab` parts of a made-up `record stop` answer of the package `package`.
fn lab_parts(package: &str) -> Value {
    json!({
        "binding_example": {
            "instance": "neutral.a",
            "scheduled_execution": {"package_path": package, "task": "neutral.task"},
        },
        "prerequisite_entry_example": {"entry": "neutral.entry"},
        "catalog_on_failure_example": {"on_failure": "neutral.fallback"},
    })
}

/// A made-up `record stop` answer of `package`, its `binding_requires` `requires` when given.
fn record_stop(package: &str, requires: Option<Value>) -> Value {
    let mut lab = lab_parts(package);
    if let Some(requires) = requires {
        lab["binding_requires"] = requires;
    }
    json!({"status": "stopped", "lab": lab})
}

/// What `actinglab --json record draft <words>` prints: its exit code and its line.
fn drafted(words: &[&str]) -> (u8, Value) {
    let line = [&["--json", "record", "draft"][..], words].concat();
    let exit = printed(&line);
    let answered = exit
        .line
        .clone()
        .unwrap_or_else(|text| panic!("{line:?} printed no line: {text}"));
    (exit.code, answered)
}

/// The draft of the answer `record_stop` given inline, which must succeed.
fn draft_of(record_stop: &Value) -> Value {
    let text = record_stop.to_string();
    let (code, line) = drafted(&["--record-stop", text.as_str()]);
    assert_eq!(code, 0, "{line}");
    assert_eq!(line["outcome"]["code"], "lab_operation_completed", "{line}");
    line
}

/// The codes of the warnings a line carries, in order.
fn warning_codes(line: &Value) -> Vec<String> {
    line["warnings"]
        .as_array()
        .map(|warnings| {
            warnings
                .iter()
                .filter_map(|warning| warning["outcome"]["code"].as_str())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// The refusal a line answers: `actingctl_usage_invalid`, exit 1, its argument and problem.
fn refusal(words: &[&str]) -> (Value, Value) {
    let (code, line) = drafted(words);
    assert_eq!(code, 1, "{words:?}: {line}");
    assert_eq!(
        line["outcome"]["code"], "actingctl_usage_invalid",
        "{words:?}: {line}"
    );
    (
        line["outcome"]["values"]["argument"].clone(),
        line["outcome"]["values"]["argument_problem"].clone(),
    )
}

/// The `path` value an outcome gives `path`.
fn path_value(path: &Path) -> Value {
    let outcome = Outcome::new(ActingctlCode::CheckConfigUnavailable).path(path);
    cli_line::outcome_value(&outcome, Detail::Full)["values"]["path"].clone()
}

#[test]
fn record_draft_answers_the_binding_parts() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let (program, _config) = made_up_root(isolated.temp.path());
    let _place = authoring::test_place::set(program);
    let package = isolated.path("neutral.package");
    let parts = lab_parts(&package);

    // `binding_requires` a list: the list, then the manual step; no warning about it.
    let requires = json!(["neutral.prerequisite.a", "neutral.prerequisite.b"]);
    let line = draft_of(&record_stop(&package, Some(requires.clone())));
    for part in PARTS {
        assert_eq!(line[part], parts[part], "{part} verbatim: {line}");
    }
    assert_eq!(line["binding_requires"], requires, "{line}");
    assert_eq!(
        line["manual_steps"],
        json!([
            "neutral.prerequisite.a",
            "neutral.prerequisite.b",
            MANUAL_STEP
        ]),
        "{line}"
    );
    assert_eq!(line["admission"]["package"], json!(package), "{line}");
    assert!(
        !warning_codes(&line)
            .iter()
            .any(|code| code == "binding_requires_unavailable"),
        "{line}"
    );

    // Absent, or null: null, the warning first, and the manual step alone.
    for requires in [None, Some(Value::Null)] {
        let line = draft_of(&record_stop(&package, requires.clone()));
        for part in PARTS {
            assert_eq!(line[part], parts[part], "{part} verbatim: {line}");
        }
        assert_eq!(
            line["binding_requires"],
            Value::Null,
            "{requires:?}: {line}"
        );
        assert_eq!(line["manual_steps"], json!([MANUAL_STEP]), "{line}");
        assert_eq!(
            warning_codes(&line).first().map(String::as_str),
            Some("binding_requires_unavailable"),
            "{requires:?}: {line}"
        );
    }
}

#[test]
fn record_draft_refuses_what_the_tool_refuses() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let _place = authoring::test_place::set(unrooted(isolated.temp.path()));
    let package = isolated.path("neutral.package");
    let valid = record_stop(&package, None).to_string();

    // Arguments that pass are answered outside any installation as today's tool answers where it
    // locates none: so a refusal below came before anything was located, run or written.
    let (code, line) = drafted(&["--record-stop", valid.as_str()]);
    assert_eq!(code, 1, "{line}");
    assert_eq!(line["outcome"]["code"], "lab_tool_unavailable", "{line}");
    assert_eq!(
        line["outcome"]["values"]["mcp_blocker"], "install_root",
        "{line}"
    );

    let lab_without = |part: &str| {
        let mut answer = record_stop(&package, None);
        if let Some(lab) = answer["lab"].as_object_mut() {
            lab.remove(part);
        }
        answer.to_string()
    };
    let with_package = |path: &str| record_stop(path, None).to_string();
    let with_requires = |requires: Value| record_stop(&package, Some(requires)).to_string();
    let missing = isolated.path("neutral-missing-export.json");
    let export = |value: Value| value.to_string();
    // Each command line after `record draft`, and the argument and problem it is refused with.
    let cases: [(Vec<String>, &str, &str); 15] = [
        (Vec::new(), "--record-stop", "conflicting"),
        (
            vec![
                "--record-stop".to_owned(),
                valid.clone(),
                "--record-stop-export".to_owned(),
                export(json!({"path": missing, "sha256": "sha256:00"})),
            ],
            "--record-stop",
            "conflicting",
        ),
        (
            vec![
                "--record-stop".to_owned(),
                json!({"status": "stopped"}).to_string(),
            ],
            "--record-stop",
            "unusable",
        ),
        (
            vec!["--record-stop".to_owned(), lab_without(PARTS[0])],
            "--record-stop",
            "unusable",
        ),
        (
            vec!["--record-stop".to_owned(), lab_without(PARTS[1])],
            "--record-stop",
            "unusable",
        ),
        (
            vec!["--record-stop".to_owned(), lab_without(PARTS[2])],
            "--record-stop",
            "unusable",
        ),
        (
            vec![
                "--record-stop".to_owned(),
                with_package("--neutral.package"),
            ],
            "--record-stop",
            "unusable",
        ),
        (
            vec!["--record-stop".to_owned(), with_requires(json!("neutral"))],
            "--record-stop",
            "unusable",
        ),
        (
            vec!["--record-stop".to_owned(), with_requires(json!({}))],
            "--record-stop",
            "unusable",
        ),
        (
            vec!["--record-stop".to_owned(), "not json".to_owned()],
            "--record-stop",
            "wrong_type",
        ),
        (
            vec!["--record-stop-export".to_owned(), export(json!([]))],
            "--record-stop-export",
            "wrong_type",
        ),
        (
            vec![
                "--record-stop-export".to_owned(),
                export(json!({"path": missing, "sha256": "sha256:00", "kind": "json"})),
            ],
            "--record-stop-export",
            "wrong_type",
        ),
        (
            vec![
                "--record-stop-export".to_owned(),
                export(json!({"path": missing})),
            ],
            "--record-stop-export",
            "wrong_type",
        ),
        (
            vec![
                "--record-stop-export".to_owned(),
                export(json!({"path": "", "sha256": "sha256:00"})),
            ],
            "--record-stop-export",
            "wrong_type",
        ),
        (
            vec![
                "--record-stop-export".to_owned(),
                export(json!({"path": missing, "sha256": "sha256:00"})),
            ],
            "--record-stop-export",
            "unusable",
        ),
    ];
    for (words, argument, problem) in &cases {
        let words = words.iter().map(String::as_str).collect::<Vec<_>>();
        assert_eq!(
            refusal(&words),
            (json!(argument), json!(problem)),
            "{words:?}"
        );
    }
}

#[test]
fn record_draft_reads_only_checked_exports() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let (program, _config) = made_up_root(isolated.temp.path());
    let _place = authoring::test_place::set(program);
    // The temporary folder's package path makes these bytes, and so the export's name, this run's.
    let package = isolated.path("neutral.package");
    let answer = record_stop(&package, Some(json!(["neutral.exported"])));
    let bytes = serde_json::to_vec(&answer).unwrap();
    let hex = format!("{:x}", Sha256::digest(&bytes));
    let sha256 = format!("sha256:{hex}");
    let directory = record_draft::export_directory();
    let export = material_export::write(&directory, &sha256, "json", &bytes).expect("the export");
    let path = export["path"]
        .as_str()
        .expect("the export's path")
        .to_owned();
    let read = |export: Value| {
        let text = export.to_string();
        drafted(&["--record-stop-export", text.as_str()])
    };

    // In the export directory, with its sha256, with or without the prefix: read.
    for named in [sha256.as_str(), hex.as_str()] {
        let (code, line) = read(json!({"path": path, "sha256": named, "size": bytes.len()}));
        assert_eq!(code, 0, "{named}: {line}");
        for part in PARTS {
            assert_eq!(line[part], answer["lab"][part], "{part}: {line}");
        }
        assert_eq!(
            line["manual_steps"],
            json!(["neutral.exported", MANUAL_STEP]),
            "{line}"
        );
    }

    // A wrong sha256, a file outside the directory and a file in a subdirectory of it: unusable.
    let outside = isolated.temp.path().join(format!("{hex}.json"));
    fs::write(&outside, &bytes).unwrap();
    let subdirectory = tempfile::Builder::new()
        .prefix("neutral-record-draft-")
        .tempdir_in(&directory)
        .unwrap();
    let inside_subdirectory = subdirectory.path().join(format!("{hex}.json"));
    fs::write(&inside_subdirectory, &bytes).unwrap();
    let wrong = format!("sha256:{}", "0".repeat(64));
    for refused in [
        json!({"path": path, "sha256": wrong}),
        json!({"path": text_of(&outside), "sha256": sha256}),
        json!({"path": text_of(&inside_subdirectory), "sha256": sha256}),
    ] {
        let (code, line) = read(refused.clone());
        assert_eq!(code, 1, "{refused}: {line}");
        assert_eq!(
            (
                &line["outcome"]["code"],
                &line["outcome"]["values"]["argument"],
                &line["outcome"]["values"]["argument_problem"],
            ),
            (
                &json!("actingctl_usage_invalid"),
                &json!("--record-stop-export"),
                &json!("unusable"),
            ),
            "{refused}: {line}"
        );
    }
}

#[test]
fn record_draft_without_daemon_program_says_so() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let (program, config) = made_up_root(isolated.temp.path());
    let daemon = isolated
        .temp
        .path()
        .join("install-root")
        .join("runtime")
        .join("actingcommand-actingd.exe");
    assert!(!daemon.exists(), "setup: the root holds no daemon program");
    let _place = authoring::test_place::set(program);
    let package = isolated.path("neutral.package");
    let parts = lab_parts(&package);
    let line = draft_of(&record_stop(&package, Some(json!([]))));

    // check-config could not be started: its error, class runtime, and the warning naming the
    // configuration in force.
    let check_config = &line["check_config"];
    assert_eq!(
        check_config.as_object().map(|members| members.len()),
        Some(1),
        "{line}"
    );
    assert_eq!(check_config["error"]["class"], "runtime", "{line}");
    assert_eq!(
        check_config["error"]["outcome"]["code"], "check_config_failed",
        "{line}"
    );
    assert_eq!(
        check_config["error"]["outcome"]["values"]["path"],
        path_value(&config),
        "{line}"
    );
    let warning = line["warnings"]
        .as_array()
        .and_then(|warnings| {
            warnings
                .iter()
                .find(|warning| warning["outcome"]["code"] == "check_config_unavailable")
        })
        .unwrap_or_else(|| panic!("no check_config_unavailable: {line}"));
    assert_eq!(
        warning["outcome"]["values"]["path"],
        path_value(&config),
        "{line}"
    );

    // The rest of the answer stands.
    for part in PARTS {
        assert_eq!(line[part], parts[part], "{part}: {line}");
    }
    assert_eq!(line["binding_requires"], json!([]), "{line}");
    assert_eq!(line["manual_steps"], json!([MANUAL_STEP]), "{line}");
    assert_eq!(line["admission"]["package"], json!(package), "{line}");
}

#[test]
fn serve_answers_record_draft_as_the_command_line() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let (program, _config) = made_up_root(isolated.temp.path());
    let _place = authoring::test_place::set(program);
    let package = isolated.path("neutral.package");
    let answer = record_stop(&package, Some(json!(["neutral.served"])));
    let table = cli_module::table().expect("the operation table");
    let input = [
        json!({"op": "record draft", "params": {"record_stop": answer}}).to_string(),
        json!({"op": "record draft"}).to_string(),
    ]
    .join("\n");
    let mut output = Vec::new();
    let mut developer = Vec::new();
    serve::serve(table, input.as_bytes(), &mut output, &mut developer).expect("the loop ends");
    let output = String::from_utf8(output).unwrap();
    let served = output
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("one JSON answer per line"))
        .collect::<Vec<_>>();
    assert_eq!(served.len(), 2, "one answer per request: {output}");

    // The same parameters on the command line: the same line.
    let text = answer.to_string();
    let lines = [drafted(&["--record-stop", text.as_str()]).1, drafted(&[]).1];
    assert_eq!(served, lines);
    assert_eq!(served[0]["outcome"]["code"], "lab_operation_completed");
    assert_eq!(
        served[0]["manual_steps"],
        json!(["neutral.served", MANUAL_STEP])
    );
    assert_eq!(
        (
            &served[1]["outcome"]["code"],
            &served[1]["outcome"]["values"]["argument"],
            &served[1]["outcome"]["values"]["argument_problem"],
        ),
        (
            &json!("actingctl_usage_invalid"),
            &json!("--record-stop"),
            &json!("conflicting"),
        )
    );
}
