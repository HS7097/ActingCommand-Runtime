// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #453 S7 (register §1.350; contracts/vision-model-folders.md, "The manifest"; coordinator
// work order rd-1114/brief-453-s7s8-vision-manifest.md): the rescue rows `vision manifest` and
// `vision check` run the live build's `actingd vision manifest|check` (and `vision check` the
// Tools' `standard-pack check` of each game root), each child ended at its bound, and write
// nothing in the root or its state root:
// - `vision_manifest_answers_the_live_builds_manifest`: `install_command_done` of
//   `install_vision_manifest` with actingd's manifest as `report` and its outcome as the link;
// - `vision_manifest_refuses_with_actingds_outcome_as_its_cause`: `install_command_refused`,
//   `install_refusal` `vision_root_unresolved`, actingd's outcome its `caused_by` link, no report;
// - `vision_manifest_a_child_that_never_ends_is_ended_at_its_bound`: ended once at 30 000 ms,
//   the refusal with `timeout_ms` the bound and no cause;
// - `vision_check_answers_a_complete_root`: `install_command_done` of `install_vision_check`
//   with actingd's report, no standard-pack root and no required model;
// - `vision_check_refuses_with_actingds_outcome_as_its_cause`: `vision_root_unresolved` caused by
//   actingd's `vision_root_unavailable`, no report, no standard-pack check run;
// - `vision_check_a_child_that_never_ends_is_ended_at_its_bound`: ended once at 120 000 ms;
// - `vision_check_joins_the_required_models_of_two_game_roots`: two game roots' models joined,
//   a model both name once with both games, a model no folder holds counted in
//   `install_caveat_noted` `vision_incomplete`.
// The machine is the fake host of `fixture.rs`: its actingd and its Tools' Lab program print the
// lines the test sets; no program is spawned and no file of a vision root exists.

use super::fixture::{BuildSpec, C1, FakeHost, live_root, tree, write_json};
use crate::agent_channel::Answer as Answered;
use crate::rescue::{self, Context};
use actingcommand_lab_ops::initiator::Initiator;
use serde_json::{Map, Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const OCR: (&str, &str) = (
    "neutral.ocr",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
);
const CLASSIFIER: (&str, &str) = (
    "neutral.classifier",
    "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
);

/// The rescue row `row` with `params` on `root`, as a Lab process the UI started; its one line
/// and exit code.
fn row_line(host: &FakeHost, root: &Path, row: &str, params: &Map<String, Value>) -> (Value, u8) {
    let config = root.join("actingd.config.json");
    let state_root = actingcommand_lab_ops::locate::configured_state_root(&config)
        .expect("the configured state root");
    let location = rescue::location_of(root, &config, &state_root);
    let context = Context {
        host,
        location: &location,
        initiator: Initiator::LabRescue,
    };
    match rescue::answer_with(&context, row, params) {
        Answered::Line(exit) => (exit.line.expect("an encoded line"), exit.code),
        other => panic!("{row}: a line, not {other:?}"),
    }
}

/// `{"provider": provider}`.
fn provider(provider: &str) -> Map<String, Value> {
    let mut params = Map::new();
    params.insert("provider".to_owned(), Value::from(provider));
    params
}

/// The line `actingd vision` prints: the schema, `outcome` and, when given, `report`.
fn vision_line(code: &str, category: &str, report: Option<Value>) -> Value {
    let mut line = json!({
        "schema_version": "actingcommand.actingd.vision.v1",
        "outcome": {"schema_version": "actingcommand.outcome.v1",
            "code": code, "category": category, "values": {}},
    });
    if let Some(report) = report {
        line["report"] = report;
    }
    line
}

/// The command line of the live build's `actingd vision check` of `root`.
fn check_command(root: &Path) -> String {
    format!(
        "live actingcommand-actingd.exe vision check --config {} --install-root {}",
        root.join("actingd.config.json").to_string_lossy(),
        root.to_string_lossy()
    )
}

/// A report of `actingd vision check` whose files are all `ok`, with `folders` and its one
/// manifest model, the OCR model, matching.
fn check_report(folders: Value) -> Value {
    json!({
        "provider": "cpu",
        "vision_root": "C:\\ActingCommand\\vision",
        "files": [
            {"vision_root_path": "ort/onnxruntime.dll", "size": 1, "sha256": "c".repeat(64),
             "component": "neutral-runtime", "archive_path": "native/onnxruntime.dll",
             "license": false, "pinned": true, "state": "ok"},
            {"vision_root_path": "models/neutral.ocr/rec.onnx", "size": 1,
             "sha256": "d".repeat(64), "component": "neutral-model", "archive_path": null,
             "license": false, "pinned": true, "state": "ok"}
        ],
        "folders": folders,
        "models": [{"model_ref": OCR.0, "family": "ppocr-ctc", "model_sha256": OCR.1,
                    "present": true, "identity": OCR.1, "identity_matches": true}]
    })
}

/// A root and its state root, with what each holds before a row runs.
struct Prepared {
    _temp: TempDir,
    host: FakeHost,
    root: PathBuf,
    state_root: PathBuf,
}

fn prepared() -> Prepared {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = live_root(temp.path(), "vision", &BuildSpec::new(C1));
    let state_root = temp.path().join("vision-state");
    Prepared {
        _temp: temp,
        host,
        root,
        state_root,
    }
}

/// The outcome's links, each `(relation, code)`.
fn links(line: &Value) -> Vec<(String, String)> {
    line["outcome"]["causes"]
        .as_array()
        .map(|causes| {
            causes
                .iter()
                .map(|cause| {
                    (
                        cause["relation"].as_str().unwrap_or_default().to_owned(),
                        cause["code"].as_str().unwrap_or_default().to_owned(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn vision_manifest_answers_the_live_builds_manifest() {
    let Prepared {
        _temp,
        host,
        root,
        state_root,
    } = prepared();
    let report = json!({
        "provider": "directml",
        "files": [{"vision_root_path": "ort/DirectML.dll", "size": 1, "sha256": "e".repeat(64),
                   "component": "neutral-gpu", "archive_path": "native/DirectML.dll",
                   "license": false, "pinned": true}],
        "models": []
    });
    host.programs.vision.borrow_mut().insert(
        "manifest".to_owned(),
        (
            0,
            vision_line("vision_manifest_reported", "success", Some(report.clone())),
        ),
    );
    let root_before = tree(&root);
    let state_before = tree(&state_root);

    let (line, exit) = row_line(&host, &root, "vision manifest", &provider("directml"));

    assert_eq!(exit, 0, "{line}");
    assert_eq!(line["outcome"]["code"], "install_command_done", "{line}");
    assert_eq!(
        line["outcome"]["values"]["operation"], "install_vision_manifest",
        "{line}"
    );
    assert_eq!(line["report"], report, "actingd's manifest is the answer");
    assert_eq!(
        links(&line),
        [("related".to_owned(), "vision_manifest_reported".to_owned())],
        "{line}"
    );
    assert_eq!(
        *host.log.borrow(),
        ["live actingcommand-actingd.exe vision manifest --provider directml".to_owned()],
        "the row runs actingd vision manifest once and nothing else"
    );
    assert_eq!(tree(&root), root_before, "nothing written in the root");
    assert_eq!(tree(&state_root), state_before, "the state root untouched");
}

#[test]
fn vision_manifest_refuses_with_actingds_outcome_as_its_cause() {
    let Prepared {
        _temp,
        host,
        root,
        state_root,
    } = prepared();
    host.programs.vision.borrow_mut().insert(
        "manifest".to_owned(),
        (1, vision_line("invalid_arguments", "error", None)),
    );
    let root_before = tree(&root);
    let state_before = tree(&state_root);

    let (line, exit) = row_line(&host, &root, "vision manifest", &provider("cpu"));

    assert_eq!(exit, 1, "{line}");
    assert_eq!(line["outcome"]["code"], "install_command_refused", "{line}");
    assert_eq!(
        line["outcome"]["values"]["operation"], "install_vision_manifest",
        "{line}"
    );
    assert_eq!(
        line["outcome"]["values"]["install_refusal"], "vision_root_unresolved",
        "{line}"
    );
    assert_eq!(
        links(&line),
        [("caused_by".to_owned(), "invalid_arguments".to_owned())],
        "{line}"
    );
    assert!(
        line.get("report").is_none(),
        "a refusal carries no report: {line}"
    );
    assert_eq!(
        *host.log.borrow(),
        ["live actingcommand-actingd.exe vision manifest --provider cpu".to_owned()]
    );
    assert_eq!(tree(&root), root_before, "nothing written in the root");
    assert_eq!(tree(&state_root), state_before, "the state root untouched");
}

#[test]
fn vision_manifest_a_child_that_never_ends_is_ended_at_its_bound() {
    let Prepared {
        _temp,
        host,
        root,
        state_root,
    } = prepared();
    host.programs.stall.borrow_mut().push("vision");
    let root_before = tree(&root);
    let state_before = tree(&state_root);

    let (line, exit) = row_line(&host, &root, "vision manifest", &provider("directml"));

    assert_eq!(exit, 1, "{line}");
    let outcome = &line["outcome"];
    assert_eq!(outcome["code"], "install_command_refused", "{line}");
    assert_eq!(
        outcome["values"]["install_refusal"], "vision_root_unresolved",
        "{line}"
    );
    assert_eq!(
        outcome["values"]["timeout_ms"], 30_000,
        "the child ran out its bound: {line}"
    );
    assert!(
        outcome["values"].get("pid").is_none() && outcome["values"].get("config_key").is_none(),
        "no child is left running to name: {line}"
    );
    assert!(
        links(&line).is_empty(),
        "an ended child is no cause: {line}"
    );
    assert!(line.get("report").is_none(), "{line}");
    assert_eq!(
        *host.programs.ended.borrow(),
        [("vision".to_owned(), 30_000)],
        "the child is ended at the bound, once"
    );
    assert_eq!(tree(&root), root_before, "nothing written in the root");
    assert_eq!(tree(&state_root), state_before, "the state root untouched");
}

#[test]
fn vision_check_answers_a_complete_root() {
    let Prepared {
        _temp,
        host,
        root,
        state_root,
    } = prepared();
    let report = check_report(json!([{"model_ref": OCR.0, "family": "ppocr-ctc",
                                      "layout": "flat", "identity": OCR.1}]));
    host.programs.vision.borrow_mut().insert(
        "check".to_owned(),
        (
            0,
            vision_line("vision_check_completed", "success", Some(report.clone())),
        ),
    );
    let root_before = tree(&root);
    let state_before = tree(&state_root);

    let (line, exit) = row_line(&host, &root, "vision check", &Map::new());

    assert_eq!(exit, 0, "{line}");
    assert_eq!(line["outcome"]["code"], "install_command_done", "{line}");
    assert_eq!(
        line["outcome"]["values"]["operation"], "install_vision_check",
        "{line}"
    );
    assert_eq!(line["report"], report, "actingd's report is the answer's");
    assert_eq!(
        line["standard_packs"],
        json!({"root": null, "games": []}),
        "{line}"
    );
    assert_eq!(line["required_models"], json!([]), "{line}");
    assert_eq!(
        links(&line),
        [("related".to_owned(), "vision_check_completed".to_owned())],
        "{line}"
    );
    assert_eq!(
        *host.log.borrow(),
        [check_command(&root)],
        "no standard-pack root: actingd vision check alone"
    );
    assert_eq!(tree(&root), root_before, "nothing written in the root");
    assert_eq!(tree(&state_root), state_before, "the state root untouched");
}

#[test]
fn vision_check_refuses_with_actingds_outcome_as_its_cause() {
    let Prepared {
        _temp,
        host,
        root,
        state_root,
    } = prepared();
    host.programs.vision.borrow_mut().insert(
        "check".to_owned(),
        (1, vision_line("vision_root_unavailable", "error", None)),
    );
    let root_before = tree(&root);
    let state_before = tree(&state_root);

    let (line, exit) = row_line(&host, &root, "vision check", &Map::new());

    assert_eq!(exit, 1, "{line}");
    assert_eq!(line["outcome"]["code"], "install_command_refused", "{line}");
    assert_eq!(
        line["outcome"]["values"]["operation"], "install_vision_check",
        "{line}"
    );
    assert_eq!(
        line["outcome"]["values"]["install_refusal"], "vision_root_unresolved",
        "{line}"
    );
    assert_eq!(
        links(&line),
        [("caused_by".to_owned(), "vision_root_unavailable".to_owned())],
        "{line}"
    );
    assert!(
        line.get("report").is_none(),
        "a refusal carries no report: {line}"
    );
    assert_eq!(*host.log.borrow(), [check_command(&root)]);
    assert_eq!(tree(&root), root_before, "nothing written in the root");
    assert_eq!(tree(&state_root), state_before, "the state root untouched");
}

#[test]
fn vision_check_a_child_that_never_ends_is_ended_at_its_bound() {
    let Prepared {
        _temp,
        host,
        root,
        state_root,
    } = prepared();
    host.programs.stall.borrow_mut().push("vision");
    let root_before = tree(&root);
    let state_before = tree(&state_root);

    let (line, exit) = row_line(&host, &root, "vision check", &Map::new());

    assert_eq!(exit, 1, "{line}");
    let outcome = &line["outcome"];
    assert_eq!(
        outcome["values"]["operation"], "install_vision_check",
        "{line}"
    );
    assert_eq!(
        outcome["values"]["install_refusal"], "vision_root_unresolved",
        "{line}"
    );
    assert_eq!(
        outcome["values"]["timeout_ms"], 120_000,
        "the child ran out its bound: {line}"
    );
    assert!(
        outcome["values"].get("pid").is_none() && outcome["values"].get("config_key").is_none(),
        "no child is left running to name: {line}"
    );
    assert!(
        links(&line).is_empty(),
        "an ended child is no cause: {line}"
    );
    assert_eq!(
        *host.programs.ended.borrow(),
        [("vision".to_owned(), 120_000)],
        "the child is ended at the bound, once"
    );
    assert_eq!(tree(&root), root_before, "nothing written in the root");
    assert_eq!(tree(&state_root), state_before, "the state root untouched");
}

#[test]
fn vision_check_joins_the_required_models_of_two_game_roots() {
    let Prepared {
        _temp,
        host,
        root,
        state_root,
    } = prepared();
    // The standard-pack root: two game roots, a folder without bundle.json and a plain file,
    // which hold no game.
    let packs = _temp.path().join("packs");
    for game in ["neutral.alpha", "neutral.beta"] {
        fs::create_dir_all(packs.join(game)).expect("a game root");
        fs::write(packs.join(game).join("bundle.json"), b"{}").expect("its bundle index");
    }
    fs::create_dir_all(packs.join("neutral.loose")).expect("a folder without a bundle");
    fs::write(packs.join("notes.txt"), b"neutral").expect("a plain file");
    let config_path = root.join("actingd.config.json");
    let mut config: Value =
        serde_json::from_slice(&fs::read(&config_path).expect("the configuration"))
            .expect("the configuration JSON");
    config["standard_packs"] = json!({"root": packs});
    write_json(&config_path, &config);
    // The OCR model's folder is there with its identity; the classifier's is not.
    let report = check_report(json!([{"model_ref": OCR.0, "family": "ppocr-ctc",
                                      "layout": "flat", "identity": OCR.1}]));
    host.programs.vision.borrow_mut().insert(
        "check".to_owned(),
        (
            0,
            vision_line("vision_check_completed", "success", Some(report.clone())),
        ),
    );
    let checked = |models: Value| {
        json!({"status": "checked", "pack_count": 2, "models": models,
               "outcome": {"schema_version": "actingcommand.outcome.v1",
                   "code": "lab_operation_completed", "category": "success", "values": {}}})
    };
    let alpha = packs.join("neutral.alpha");
    let beta = packs.join("neutral.beta");
    host.programs.standard_pack_checks.borrow_mut().insert(
        alpha.to_string_lossy().into_owned(),
        (
            0,
            checked(json!([
                {"model_ref": CLASSIFIER.0, "model_sha256": CLASSIFIER.1,
                 "packages": ["neutral.alpha.two"]},
                {"model_ref": OCR.0, "model_sha256": OCR.1, "packages": ["neutral.alpha.one"]}
            ])),
        ),
    );
    host.programs.standard_pack_checks.borrow_mut().insert(
        beta.to_string_lossy().into_owned(),
        (
            0,
            checked(json!([
                {"model_ref": OCR.0, "model_sha256": OCR.1,
                 "packages": ["neutral.beta.one", "neutral.beta.two"]}
            ])),
        ),
    );
    let root_before = tree(&root);
    let state_before = tree(&state_root);
    let packs_before = tree(&packs);

    let (line, exit) = row_line(&host, &root, "vision check", &Map::new());

    assert_eq!(exit, 0, "a caveat is no failure: {line}");
    let outcome = &line["outcome"];
    assert_eq!(outcome["code"], "install_caveat_noted", "{line}");
    assert_eq!(
        outcome["values"]["operation"], "install_vision_check",
        "{line}"
    );
    assert_eq!(
        outcome["values"]["install_caveat"], "vision_incomplete",
        "{line}"
    );
    assert_eq!(
        outcome["values"]["count"], 1,
        "the classifier no folder holds: {line}"
    );
    assert_eq!(
        line["standard_packs"],
        json!({"root": super::files::plain(&packs),
               "games": ["neutral.alpha", "neutral.beta"]}),
        "{line}"
    );
    assert_eq!(
        line["required_models"],
        json!([
            {"model_ref": CLASSIFIER.0, "model_sha256": CLASSIFIER.1,
             "games": [{"game": "neutral.alpha", "packages": ["neutral.alpha.two"]}],
             "present": false, "identity_matches": false},
            {"model_ref": OCR.0, "model_sha256": OCR.1,
             "games": [{"game": "neutral.alpha", "packages": ["neutral.alpha.one"]},
                       {"game": "neutral.beta",
                        "packages": ["neutral.beta.one", "neutral.beta.two"]}],
             "present": true, "identity_matches": true}
        ]),
        "each model once, with the games and packages that name it: {line}"
    );
    assert_eq!(line["report"], report, "{line}");
    assert_eq!(
        *host.log.borrow(),
        [
            check_command(&root),
            format!(
                "live actinglab.exe --json standard-pack check --root {}",
                alpha.to_string_lossy()
            ),
            format!(
                "live actinglab.exe --json standard-pack check --root {}",
                beta.to_string_lossy()
            ),
        ],
        "actingd once, then each game root's check once, in name order"
    );
    assert_eq!(
        host.programs
            .deadlines
            .borrow()
            .iter()
            .map(|(_, ms)| *ms)
            .collect::<Vec<_>>(),
        [120_000, 300_000, 300_000],
        "each child within its bound"
    );
    assert_eq!(tree(&root), root_before, "nothing written in the root");
    assert_eq!(tree(&state_root), state_before, "the state root untouched");
    assert_eq!(tree(&packs), packs_before, "nothing written in the packs");
}
