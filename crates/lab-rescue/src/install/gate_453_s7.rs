// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #453 S7 (register §1.350; contracts/vision-model-folders.md, "The manifest"; coordinator
// work order rd-1114/brief-453-s7s8-vision-manifest.md): the rescue rows `vision manifest` and
// `vision check` run the live build's `actingd vision manifest|check` (and `vision check` the
// Tools' `standard-pack check` of each game root), each child ended at its bound, and write
// nothing in the root or its state root:
// - `vision_manifest_answers_the_live_builds_manifest`: `install_command_done` of
//   `install_vision_manifest` with actingd's manifest as `report` and its outcome as the link;
// - `vision_manifest_refuses_with_actingds_outcome_as_its_cause`: `install_command_refused`,
//   `install_refusal` `vision_report_unanswered` (fix 1: the row resolves no root), actingd's
//   outcome its `caused_by` link, no report;
// - `vision_manifest_a_child_that_never_ends_is_ended_at_its_bound`: ended once at 30 000 ms,
//   `vision_report_unanswered` with `timeout_ms` the bound and no cause;
// - `vision_check_answers_a_complete_root`: `install_command_done` of `install_vision_check`
//   with actingd's report, no standard-pack root and no required model;
// - `vision_check_refuses_with_actingds_outcome_as_its_cause`: `vision_root_unresolved` caused by
//   actingd's `vision_root_unavailable`, no report, no standard-pack check run;
// - `vision_check_a_child_that_never_ends_is_ended_at_its_bound`: ended once at 160 000 ms (fix
//   1), `vision_report_unanswered` with `timeout_ms` the bound;
// - `vision_check_joins_the_required_models_of_two_game_roots`: each game root's check first, in
//   name order, then actingd with `--model` for each model they need; two game roots' models
//   joined, a model both name once with both games, a model no folder holds counted in
//   `install_caveat_noted` `vision_incomplete`; the folder without `bundle.json` and the file
//   named in `skipped` (fix 1).
// Fix 1 (review; coordinator work order rd-1114/brief-1693-fix1.md):
// - `vision_check_refuses_when_the_rows_limit_runs_out`: three game roots whose checks take
//   290 s each leave actingd less than its own bound; it is ended there and the refusal
//   (`vision_report_unanswered`) has `timeout_ms` the row's limit, 900 000;
// - `vision_check_names_a_game_root_it_skips_and_why` (Windows): a game root that is a junction
//   is not checked and is named in `skipped` as `link`.
// Fix 2 (review; coordinator work order rd-1114/brief-1693-fix2.md):
// - `vision_check_a_refusal_where_actingd_loads_the_configuration_is_a_root_not_resolved`:
//   actingd's refusal at stage `config_load` is `vision_root_unresolved`, its outcome the cause;
// - `vision_check_a_refusal_before_the_configuration_names_no_root`: actingd's own installation
//   check's refusal (`install_executable_unavailable`, no stage) is `vision_report_unanswered`;
// - `vision_check_refuses_when_the_rows_limit_runs_out_during_a_game_roots_check`: four game
//   roots whose checks take 290 s each; the fourth is ended at what the row's limit left, and the
//   refusal is `pack_invalid` naming it with `timeout_ms` 900 000; actingd is not started;
// - `vision_check_starts_nothing_when_no_time_is_left`: three game roots whose checks take
//   299 999 ms each use the row's limit up; the fourth is not started, and the refusal is
//   `pack_invalid` naming it with `timeout_ms` 900 000; actingd is not started.
// `vision_check_refuses_with_actingds_outcome_as_its_cause` gives actingd's refusal its stage
// `config_vision`, as actingd names it (fix 2: the stage decides), and
// `vision_check_joins_the_required_models_of_two_game_roots` actingd's `absent` entry for the
// classifier it was asked about (fix 2: actingd names it; it is not present).
// The machine is the fake host of `fixture.rs`: its actingd and its Tools' Lab program print the
// lines the test sets; no program is spawned (but `mklink` to make the junction) and no file of a
// vision root exists.

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

/// The line `actingd vision check` prints when it refuses with `code` and `values` (fix 2: a
/// stage among them where it names one).
fn vision_refusal(code: &str, values: Value) -> Value {
    let mut line = vision_line(code, "error", None);
    line["outcome"]["values"] = values;
    line
}

/// The command line of the live build's `actingd vision check` of `root`, asked about `models`.
fn check_command(root: &Path, models: &[&str]) -> String {
    let mut line = format!(
        "live actingcommand-actingd.exe vision check --config {} --install-root {}",
        root.join("actingd.config.json").to_string_lossy(),
        root.to_string_lossy()
    );
    for model in models {
        line.push_str(" --model ");
        line.push_str(model);
    }
    line
}

/// The line of the Tools' `standard-pack check` that passed with `models`.
fn pack_checked(models: Value) -> Value {
    json!({"status": "checked", "pack_count": 2, "models": models,
           "outcome": {"schema_version": "actingcommand.outcome.v1",
               "code": "lab_operation_completed", "category": "success", "values": {}}})
}

/// Names `packs` as the root's `standard_packs.root`.
fn configure_packs(root: &Path, packs: &Path) {
    let config_path = root.join("actingd.config.json");
    let mut config: Value =
        serde_json::from_slice(&fs::read(&config_path).expect("the configuration"))
            .expect("the configuration JSON");
    config["standard_packs"] = json!({"root": packs});
    write_json(&config_path, &config);
}

/// A game root `game` below `packs`, with its bundle index.
fn game_root(packs: &Path, game: &str) -> PathBuf {
    let root = packs.join(game);
    fs::create_dir_all(&root).expect("a game root");
    fs::write(root.join("bundle.json"), b"{}").expect("its bundle index");
    root
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
        line["outcome"]["values"]["install_refusal"], "vision_report_unanswered",
        "the row resolves no root, so its failure names none: {line}"
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
        outcome["values"]["install_refusal"], "vision_report_unanswered",
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
        json!({"root": null, "games": [], "skipped": []}),
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
        [check_command(&root, &[])],
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
    // Fix 2: at the stage actingd names for it, where it resolves the vision root.
    host.programs.vision.borrow_mut().insert(
        "check".to_owned(),
        (
            1,
            vision_refusal("vision_root_unavailable", json!({"stage": "config_vision"})),
        ),
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
    assert_eq!(*host.log.borrow(), [check_command(&root, &[])]);
    assert_eq!(tree(&root), root_before, "nothing written in the root");
    assert_eq!(tree(&state_root), state_before, "the state root untouched");
}

#[test]
fn vision_check_a_refusal_where_actingd_loads_the_configuration_is_a_root_not_resolved() {
    let Prepared {
        _temp,
        host,
        root,
        state_root,
    } = prepared();
    // A configuration file actingd cannot decode, refused where it loads it.
    host.programs.vision.borrow_mut().insert(
        "check".to_owned(),
        (
            1,
            vision_refusal("config_decode_failed", json!({"stage": "config_load"})),
        ),
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
        "a refusal at a stage where actingd loads, assembles or resolves the vision \
         configuration: {line}"
    );
    assert_eq!(
        links(&line),
        [("caused_by".to_owned(), "config_decode_failed".to_owned())],
        "{line}"
    );
    assert_eq!(
        line["outcome"]["causes"][0]["values"]["stage"], "config_load",
        "actingd's outcome as it came: {line}"
    );
    assert!(line.get("report").is_none(), "{line}");
    assert_eq!(*host.log.borrow(), [check_command(&root, &[])]);
    assert_eq!(tree(&root), root_before, "nothing written in the root");
    assert_eq!(tree(&state_root), state_before, "the state root untouched");
}

#[test]
fn vision_check_a_refusal_before_the_configuration_names_no_root() {
    let Prepared {
        _temp,
        host,
        root,
        state_root,
    } = prepared();
    // actingd's own installation check refuses before it reads the configuration: no stage.
    host.programs.vision.borrow_mut().insert(
        "check".to_owned(),
        (
            1,
            vision_refusal("install_executable_unavailable", json!({})),
        ),
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
        line["outcome"]["values"]["install_refusal"], "vision_report_unanswered",
        "a refusal with no stage says nothing of the vision root: {line}"
    );
    assert_eq!(
        links(&line),
        [(
            "caused_by".to_owned(),
            "install_executable_unavailable".to_owned()
        )],
        "{line}"
    );
    assert!(line.get("report").is_none(), "{line}");
    assert_eq!(*host.log.borrow(), [check_command(&root, &[])]);
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
        outcome["values"]["install_refusal"], "vision_report_unanswered",
        "an ended child says nothing of the root: {line}"
    );
    assert_eq!(
        outcome["values"]["timeout_ms"], 160_000,
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
        [("vision".to_owned(), 160_000)],
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
    // which hold no game and are named as skipped (fix 1).
    let packs = _temp.path().join("packs");
    let alpha = game_root(&packs, "neutral.alpha");
    let beta = game_root(&packs, "neutral.beta");
    fs::create_dir_all(packs.join("neutral.loose")).expect("a folder without a bundle");
    fs::write(packs.join("notes.txt"), b"neutral").expect("a plain file");
    configure_packs(&root, &packs);
    // The OCR model's folder is there with its identity; the classifier's is not, and actingd,
    // asked about it, names it absent (fix 2).
    let report = check_report(json!([
        {"model_ref": CLASSIFIER.0, "asked": true, "absent": true},
        {"model_ref": OCR.0, "family": "ppocr-ctc", "layout": "flat", "asked": true,
         "identity": OCR.1}
    ]));
    host.programs.vision.borrow_mut().insert(
        "check".to_owned(),
        (
            0,
            vision_line("vision_check_completed", "success", Some(report.clone())),
        ),
    );
    host.programs.standard_pack_checks.borrow_mut().insert(
        alpha.to_string_lossy().into_owned(),
        (
            0,
            pack_checked(json!([
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
            pack_checked(json!([
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
               "games": ["neutral.alpha", "neutral.beta"],
               "skipped": [{"name": "neutral.loose", "why": "no_bundle"},
                           {"name": "notes.txt", "why": "not_a_folder"}]}),
        "nothing below the standard-pack root is skipped silently: {line}"
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
            format!(
                "live actinglab.exe --json standard-pack check --root {}",
                alpha.to_string_lossy()
            ),
            format!(
                "live actinglab.exe --json standard-pack check --root {}",
                beta.to_string_lossy()
            ),
            check_command(&root, &[CLASSIFIER.0, OCR.0]),
        ],
        "each game root's check once, in name order, then actingd once, asked about each model \
         the packs need"
    );
    assert_eq!(
        host.programs
            .deadlines
            .borrow()
            .iter()
            .map(|(_, ms)| *ms)
            .collect::<Vec<_>>(),
        [300_000, 300_000, 160_000],
        "each child within its own bound, the row's limit far off"
    );
    assert_eq!(tree(&root), root_before, "nothing written in the root");
    assert_eq!(tree(&state_root), state_before, "the state root untouched");
    assert_eq!(tree(&packs), packs_before, "nothing written in the packs");
}

#[test]
fn vision_check_refuses_when_the_rows_limit_runs_out() {
    let Prepared {
        _temp,
        host,
        root,
        state_root,
    } = prepared();
    // Three game roots whose checks pass after 290 s each, within their own 300 s: 870 s of the
    // row's 900 s are gone when actingd starts, which never ends.
    let packs = _temp.path().join("packs");
    let games =
        ["neutral.alpha", "neutral.beta", "neutral.gamma"].map(|game| game_root(&packs, game));
    configure_packs(&root, &packs);
    for game in &games {
        host.programs.standard_pack_checks.borrow_mut().insert(
            game.to_string_lossy().into_owned(),
            (0, pack_checked(json!([]))),
        );
    }
    host.programs.run_ms.set(290_000);
    host.programs.stall.borrow_mut().push("vision");
    let root_before = tree(&root);
    let state_before = tree(&state_root);
    let packs_before = tree(&packs);

    let (line, exit) = row_line(&host, &root, "vision check", &Map::new());

    assert_eq!(exit, 1, "{line}");
    let outcome = &line["outcome"];
    assert_eq!(outcome["code"], "install_command_refused", "{line}");
    assert_eq!(
        outcome["values"]["operation"], "install_vision_check",
        "{line}"
    );
    assert_eq!(
        outcome["values"]["install_refusal"], "vision_report_unanswered",
        "{line}"
    );
    assert_eq!(
        outcome["values"]["timeout_ms"], 900_000,
        "the row's limit ran out, not actingd's own 160 s: {line}"
    );
    assert!(
        links(&line).is_empty(),
        "an ended child is no cause: {line}"
    );
    assert!(line.get("report").is_none(), "{line}");
    let deadlines = host
        .programs
        .deadlines
        .borrow()
        .iter()
        .map(|(_, ms)| *ms)
        .collect::<Vec<_>>();
    assert_eq!(
        deadlines[..3],
        [300_000, 300_000, 300_000],
        "each game root's check within its own bound"
    );
    let ended = host.programs.ended.borrow().clone();
    assert_eq!(ended.len(), 1, "{ended:?}");
    assert_eq!(ended[0].0, "vision", "{ended:?}");
    assert!(
        (29_000..=30_000).contains(&ended[0].1),
        "actingd ended at what the row's limit left, about 30 s: {ended:?}"
    );
    assert_eq!(deadlines.len(), 4, "{deadlines:?}");
    assert_eq!(tree(&root), root_before, "nothing written in the root");
    assert_eq!(tree(&state_root), state_before, "the state root untouched");
    assert_eq!(tree(&packs), packs_before, "nothing written in the packs");
}

/// Fix 2: four game roots below a standard-pack root of `root`, each of whose checks passes with
/// no model and takes `run_ms` on the fake clock (one that would take longer than its bound is
/// ended there); the packs, and the four roots in name order.
fn four_game_roots(
    host: &FakeHost,
    temp: &Path,
    root: &Path,
    run_ms: u64,
) -> (PathBuf, [PathBuf; 4]) {
    let packs = temp.join("packs");
    let games = [
        "neutral.alpha",
        "neutral.beta",
        "neutral.delta",
        "neutral.gamma",
    ]
    .map(|game| game_root(&packs, game));
    configure_packs(root, &packs);
    for game in &games {
        host.programs.standard_pack_checks.borrow_mut().insert(
            game.to_string_lossy().into_owned(),
            (0, pack_checked(json!([]))),
        );
    }
    host.programs.run_ms.set(run_ms);
    (packs, games)
}

/// The refusal of a game root that ran out of the row's limit: `pack_invalid` naming `game` at
/// `game_root`, `timeout_ms` the row's limit, no cause, no report.
fn assert_ran_out_at(line: &Value, game: &str, game_root: &Path) {
    let outcome = &line["outcome"];
    assert_eq!(outcome["code"], "install_command_refused", "{line}");
    assert_eq!(
        outcome["values"]["operation"], "install_vision_check",
        "{line}"
    );
    assert_eq!(
        outcome["values"]["install_refusal"], "pack_invalid",
        "{line}"
    );
    assert_eq!(outcome["values"]["game"], game, "the root it names: {line}");
    let named = PathBuf::from(outcome["values"]["path"].as_str().expect("a path"));
    assert_eq!(
        named.file_name(),
        game_root.file_name(),
        "the path of the root it names: {line}"
    );
    assert_eq!(
        outcome["values"]["timeout_ms"], 900_000,
        "the row's limit ran out: {line}"
    );
    assert!(links(line).is_empty(), "no cause: {line}");
    assert!(line.get("report").is_none(), "{line}");
}

#[test]
fn vision_check_refuses_when_the_rows_limit_runs_out_during_a_game_roots_check() {
    let Prepared {
        _temp,
        host,
        root,
        state_root,
    } = prepared();
    // Checks of 290 s each: three take 870 s of the row's 900 s (and a millisecond of the fake
    // clock per reading), the fourth gets what is left, about 30 s, and is ended there.
    let (packs, games) = four_game_roots(&host, _temp.path(), &root, 290_000);
    let root_before = tree(&root);
    let state_before = tree(&state_root);
    let packs_before = tree(&packs);

    let (line, exit) = row_line(&host, &root, "vision check", &Map::new());

    assert_eq!(exit, 1, "{line}");
    assert_ran_out_at(&line, "neutral.gamma", &games[3]);
    let deadlines = host
        .programs
        .deadlines
        .borrow()
        .iter()
        .map(|(_, ms)| *ms)
        .collect::<Vec<_>>();
    assert_eq!(
        deadlines[..3],
        [300_000, 300_000, 300_000],
        "the first three within their own bound: {deadlines:?}"
    );
    assert_eq!(deadlines.len(), 4, "{deadlines:?}");
    assert!(
        (29_000..=30_000).contains(&deadlines[3]),
        "the fourth within what the row's limit left: {deadlines:?}"
    );
    assert_eq!(
        *host.programs.ended.borrow(),
        [("--json".to_owned(), deadlines[3])],
        "the fourth check ended there, once"
    );
    assert_eq!(
        *host.log.borrow(),
        games
            .iter()
            .map(|game| format!(
                "live actinglab.exe --json standard-pack check --root {}",
                game.to_string_lossy()
            ))
            .collect::<Vec<_>>(),
        "every game root checked in name order, actingd not started"
    );
    assert_eq!(tree(&root), root_before, "nothing written in the root");
    assert_eq!(tree(&state_root), state_before, "the state root untouched");
    assert_eq!(tree(&packs), packs_before, "nothing written in the packs");
}

#[test]
fn vision_check_starts_nothing_when_no_time_is_left() {
    let Prepared {
        _temp,
        host,
        root,
        state_root,
    } = prepared();
    // Checks of 299 999 ms each, on a fake clock that also moves 1 ms after each reading: the row
    // reads its start s; the first check's bound at s + 1 (its own 300 000; it ends at
    // s + 300 001), the second's at s + 300 001 (its own; it ends at s + 600 001), the third's at
    // s + 600 001, when 299 999 ms of the limit are left, all of which it takes and no more (it
    // ends at s + 900 001); the fourth's bound, read at s + 900 001, finds nothing left, so it is
    // not started.
    let (packs, games) = four_game_roots(&host, _temp.path(), &root, 299_999);
    let root_before = tree(&root);
    let state_before = tree(&state_root);
    let packs_before = tree(&packs);

    let (line, exit) = row_line(&host, &root, "vision check", &Map::new());

    assert_eq!(exit, 1, "{line}");
    assert_ran_out_at(&line, "neutral.gamma", &games[3]);
    assert_eq!(
        host.programs
            .deadlines
            .borrow()
            .iter()
            .map(|(_, ms)| *ms)
            .collect::<Vec<_>>(),
        [300_000, 300_000, 299_999],
        "three checks, the third within what the limit left; none for the fourth"
    );
    assert!(
        host.programs.ended.borrow().is_empty(),
        "none ended: each took no longer than its bound"
    );
    assert_eq!(
        *host.log.borrow(),
        games[..3]
            .iter()
            .map(|game| format!(
                "live actinglab.exe --json standard-pack check --root {}",
                game.to_string_lossy()
            ))
            .collect::<Vec<_>>(),
        "the fourth game root not started, nor actingd"
    );
    assert_eq!(tree(&root), root_before, "nothing written in the root");
    assert_eq!(tree(&state_root), state_before, "the state root untouched");
    assert_eq!(tree(&packs), packs_before, "nothing written in the packs");
}

#[cfg(windows)]
#[test]
fn vision_check_names_a_game_root_it_skips_and_why() {
    let Prepared {
        _temp,
        host,
        root,
        state_root,
    } = prepared();
    // A game root, and one more that is a junction to a folder elsewhere that holds bundle.json:
    // the Runtime lists no game root through a junction, so the check does not check it either,
    // and says so.
    let packs = _temp.path().join("packs");
    let alpha = game_root(&packs, "neutral.alpha");
    let elsewhere = game_root(&_temp.path().join("elsewhere"), "neutral.linked");
    // mklink takes plain paths, not the canonical `\\?\` form.
    let made = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(packs.join("neutral.linked"))
        .arg(&elsewhere)
        .output()
        .expect("run mklink");
    assert!(
        made.status.success(),
        "make the junction: {}",
        String::from_utf8_lossy(&made.stderr)
    );
    configure_packs(&root, &packs);
    host.programs.standard_pack_checks.borrow_mut().insert(
        alpha.to_string_lossy().into_owned(),
        (0, pack_checked(json!([]))),
    );
    let report = check_report(json!([{"model_ref": OCR.0, "family": "ppocr-ctc",
                                      "layout": "flat", "asked": true, "identity": OCR.1}]));
    host.programs.vision.borrow_mut().insert(
        "check".to_owned(),
        (
            0,
            vision_line("vision_check_completed", "success", Some(report)),
        ),
    );
    let root_before = tree(&root);
    let state_before = tree(&state_root);

    let (line, exit) = row_line(&host, &root, "vision check", &Map::new());

    assert_eq!(exit, 0, "{line}");
    assert_eq!(line["outcome"]["code"], "install_command_done", "{line}");
    assert_eq!(
        line["standard_packs"],
        json!({"root": super::files::plain(&packs), "games": ["neutral.alpha"],
               "skipped": [{"name": "neutral.linked", "why": "link"}]}),
        "the junction is named, with why it is not checked: {line}"
    );
    assert_eq!(
        *host.log.borrow(),
        [
            format!(
                "live actinglab.exe --json standard-pack check --root {}",
                alpha.to_string_lossy()
            ),
            check_command(&root, &[]),
        ],
        "only the game root that is no junction is checked"
    );
    assert_eq!(tree(&root), root_before, "nothing written in the root");
    assert_eq!(tree(&state_root), state_before, "the state root untouched");
}
