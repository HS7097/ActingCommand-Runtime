// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-2 (register §1.287, "Slice LAB-2"; contracts/lab-two-entries.md §4.2, §8.2,
// §9.3 and open points 6 and 7; coordinator work order rd-1108/arch-013/brief-lab2.md): the CLI
// module of actinglab and `actinglab serve --stdio`.
// - `cli_short_words_parse`: the short words of the operation table name the operation and its
//   parameters: the longest name, the instance and the other positionals in order, options and
//   switches, a source command's further flags and words; words and flags the row does not take,
//   a reserved row, a row of a slice not built (the rescue module) and actinglab's old
//   `config get` are refused with `actingctl_usage_invalid` naming the word.
// - `cli_json_matches_op_output`: the `--json` line of an operation is the operation's output: for
//   a core row, the line `lab-ops` builds for it; for a row actinglab's own command serves, that
//   command's data beside `lab_operation_completed`; a failure keeps every member of its details
//   (`lab_operation_check_failed`: `req_id`, `effect`, `executed` and the rest) and exits 1.
// - `serve_answers_same_as_cli`: one answer line per request line, the CLI's `--json` line for
//   the same operation; a request that is no operation is refused and the loop goes on.
// - `recording_steps_and_marks_carry_the_initiator` (the work order's fourth test): a recording's
//   steps and marks carry the process's initiator word (§4.2), and `ACTINGCOMMAND_INITIATOR`
//   changes it; a word the variable may not name is refused and nothing is written.
// Fakes only: an empty configuration in a temporary folder, offline frames; no Runtime, no
// device, no clock, no sleep.

use super::*;
use crate::cli_module::{self, engine, serve, words};
use crate::codes::ActinglabCode;
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_lab::{
    ClickSpec, LAB_RECORD_MARK_SCHEMA, LabOcr, MarkFamily, MarkRequest, MarkSpec, RecordRect,
    RecordStartOptions, RecordingLock, record_mark, record_start,
};
use actingcommand_lab_ops::cli_line;
use actingcommand_lab_ops::initiator::{ENVIRONMENT_VARIABLE, Initiator};
use std::ffi::OsStr;

fn words_of(line: &[&str]) -> Vec<String> {
    line.iter().map(|word| (*word).to_owned()).collect()
}

/// The refusal's code, `argument` and `argument_problem`.
fn refusal(outcome: &Outcome) -> (Value, Value, Value) {
    let value = cli_line::outcome_value(outcome, Detail::Full);
    (
        value["code"].clone(),
        value["values"]["argument"].clone(),
        value["values"]["argument_problem"].clone(),
    )
}

fn parsed(line: &[&str]) -> (String, Value) {
    let table = cli_module::table().expect("the operation table");
    let call = words::parse(table, &words_of(line)).unwrap_or_else(|outcome| {
        panic!(
            "{line:?}: {}",
            cli_line::outcome_value(&outcome, Detail::Full)
        )
    });
    (call.name, Value::Object(call.params))
}

fn refused(line: &[&str]) -> (Value, Value, Value) {
    let table = cli_module::table().expect("the operation table");
    match words::parse(table, &words_of(line)) {
        Ok(call) => panic!("{line:?} must be refused, not read as {call:?}"),
        Err(outcome) => refusal(&outcome),
    }
}

#[test]
fn cli_short_words_parse() {
    // The instance is a positional; an option keeps its value as written.
    assert_eq!(
        parsed(&[
            "pause",
            "BlueArchiveJP",
            "--reason",
            "hotplace-x",
            "--drain",
            "300s"
        ]),
        (
            "pause".to_owned(),
            json!({"instance": "BlueArchiveJP", "reason": "hotplace-x", "drain": "300s"})
        )
    );
    // The longest name that begins the line; an ADB port names an instance too.
    assert_eq!(
        parsed(&["emu", "start", "16384", "--wait", "30s"]),
        (
            "emu start".to_owned(),
            json!({"instance": "16384", "wait": "30s"})
        )
    );
    // A repeated positional is a list; an integer option a number; a switch true.
    assert_eq!(
        parsed(&[
            "config",
            "set",
            "a.b=1",
            "c.d=\"x\"",
            "--expected-version",
            "7"
        ]),
        (
            "config set".to_owned(),
            json!({"changes": ["a.b=1", "c.d=\"x\""], "expected_version": 7})
        )
    );
    assert_eq!(
        parsed(&["drill", "TESTSERIAL0001", "--device"]),
        (
            "drill".to_owned(),
            json!({"instance": "TESTSERIAL0001", "device": true})
        )
    );
    // A list<string> option is comma-separated words.
    assert_eq!(
        parsed(&["events", "node.a", "--type", "a.b,c.d", "--limit", "5"]),
        (
            "events".to_owned(),
            json!({"instance": "node.a", "type": ["a.b", "c.d"], "limit": 5})
        )
    );
    // A row with `params_more` carries its source command's further flags and words over.
    assert_eq!(
        parsed(&["record", "start", "--task-id", "daily", "--dry-run"]),
        (
            "record start".to_owned(),
            json!({"task_id": "daily", "dry_run": true})
        )
    );
    assert_eq!(
        parsed(&["devices", "bind", "neutral.a", "TESTSERIAL0001"]),
        (
            "devices bind".to_owned(),
            json!({"instance": "neutral.a", "words": ["TESTSERIAL0001"]})
        )
    );
    assert_eq!(
        parsed(&[
            "regression",
            "--evidence",
            "page.json",
            "--package",
            "pack.zip"
        ]),
        (
            "regression".to_owned(),
            json!({"evidence": "page.json", "package": "pack.zip"})
        )
    );
    // Open point 7: actinglab's own user configuration is `user-config`.
    assert_eq!(
        parsed(&[
            "user-config",
            "set",
            "instance.neutral.serial",
            "TESTSERIAL0001"
        ]),
        (
            "user-config set".to_owned(),
            json!({"key": "instance.neutral.serial", "value": "TESTSERIAL0001"})
        )
    );

    let unknown = json!("unknown");
    // A word no offered operation begins with.
    assert_eq!(
        refused(&["frobnicate"]),
        (
            json!("actingctl_usage_invalid"),
            json!("frobnicate"),
            unknown.clone()
        )
    );
    // A reserved row (`batch`) and a row of a slice not built (the rescue module's `deploy`).
    assert_eq!(
        refused(&["batch", "neutral.a", "steps.json"]).1,
        json!("batch")
    );
    assert_eq!(refused(&["deploy", "v0.12.47"]).1, json!("deploy"));
    // The old `config get` is the table's `config` with a word it does not take.
    assert_eq!(
        refused(&["config", "get"]),
        (
            json!("actingctl_usage_invalid"),
            json!("get"),
            unknown.clone()
        )
    );
    // A flag a row without `params_more` does not take; a required parameter missing; a value of
    // the wrong type; an option given twice; an option without its value.
    assert_eq!(
        refused(&["pause", "--state-root", "x"]),
        (
            json!("actingctl_usage_invalid"),
            json!("--state-root"),
            unknown
        )
    );
    assert_eq!(
        refused(&["emu", "start"]),
        (
            json!("actingctl_usage_invalid"),
            json!("<instance>"),
            json!("required")
        )
    );
    assert_eq!(
        refused(&["material", "1:2:3", "--offset", "x"]).2,
        json!("wrong_type")
    );
    assert_eq!(
        refused(&["pause", "a", "--reason", "x", "--reason", "y"]).2,
        json!("repeated")
    );
    assert_eq!(
        refused(&["pause", "a", "--reason"]).2,
        json!("value_missing")
    );
}

#[test]
fn cli_json_matches_op_output() {
    let _guard = env_lock();
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    // A core row: the line `lab-ops` builds for the operation (actingctl's `codes`), unchanged.
    let cli = cli_module::run(&words_of(&["codes", "actingctl_usage_invalid"]));
    let served = actingcommand_lab_ops::code_lookup::dispatch(&[
        OsString::from("codes"),
        OsString::from("actingctl_usage_invalid"),
    ])
    .expect("codes is served");
    let line = cli.line.clone().expect("the CLI's line");
    assert_eq!(Some(&line), served.line.as_ref().ok());
    assert_eq!((cli.code, served.code), (0, 0));
    assert_eq!(line["outcome"]["code"], "outcome_codes_reported");

    // A row actinglab's own command serves: the command's data beside lab_operation_completed.
    let temp = TempDir::new().unwrap();
    set_config_env(temp.path().join("config.json"));
    let set = cli_module::run(&words_of(&[
        "user-config",
        "set",
        "instance.neutral.serial",
        "TESTSERIAL0001",
    ]));
    assert_eq!(set.code, 0, "{:?}", set.line);
    let cli = cli_module::run(&words_of(&["user-config", "instance.neutral.serial"]));
    let data = crate::execute(
        &parse_invocation(["user-config", "get", "instance.neutral.serial"], true)
            .expect("the command's words"),
    )
    .expect("the command's data");
    let Value::Object(members) = data else {
        panic!("user-config answers an object");
    };
    let expected = cli_line::success(members, &Outcome::new(ActinglabCode::LabOperationCompleted));
    assert_eq!(cli.line.as_ref().ok(), expected.line.as_ref().ok());
    assert_eq!(cli.code, 0);
    assert_eq!(cli.line.as_ref().unwrap()["value"], json!("TESTSERIAL0001"));
    set_missing_config_env();

    // A failure keeps every member of its details beside its code and exits 1 (§9.3).
    let details = json!({
        "req_id": "req-neutral-0001",
        "correlation_id": "corr-neutral-0001",
        "receipt_state": "completed",
        "effect": "performed",
        "executed": true,
        "failure": null,
        "client_code": "runtime_lab_diagnostic_budget_mismatch",
        "client_message": "the client's check failed"
    });
    let failed = engine::line(Err(CliError::new(
        ErrorKind::RuntimeUnavailable,
        ActinglabCode::LabOperationCheckFailed.as_str(),
        "the Runtime answered the Lab operation",
        &[],
    )
    .with_details(details.clone())));
    assert_eq!(failed.code, 1);
    let line = failed.line.expect("the failure's line");
    assert_eq!(line["outcome"]["code"], "lab_operation_check_failed");
    for (member, value) in details.as_object().unwrap() {
        assert_eq!(&line[member], value, "{member}");
    }
    assert!(
        failed.developer.is_some(),
        "a failure has its developer line"
    );
}

#[test]
fn serve_answers_same_as_cli() {
    let _guard = env_lock();
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    let temp = TempDir::new().unwrap();
    set_config_env(temp.path().join("config.json"));
    let table = cli_module::table().expect("the operation table");
    let input = [
        r#"{"op": "codes", "params": {"code": "actingctl_usage_invalid"}}"#,
        "",
        r#"{"op": "user-config", "params": {"key": "instance.neutral.serial"}}"#,
        r#"{"op": "frobnicate"}"#,
        "not JSON",
        r#"{"op": "pause", "params": {"instance": 16384}}"#,
    ]
    .join("\n");
    let mut output = Vec::new();
    let mut developer = Vec::new();
    serve::serve(table, input.as_bytes(), &mut output, &mut developer).expect("the loop ends");
    let answers = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("one JSON answer per line"))
        .collect::<Vec<_>>();
    assert_eq!(
        answers.len(),
        5,
        "one answer per request, none for the blank line"
    );
    for (answer, line) in [
        (&answers[0], &["codes", "actingctl_usage_invalid"][..]),
        (&answers[1], &["user-config", "instance.neutral.serial"][..]),
    ] {
        let cli = cli_module::run(&words_of(line));
        assert_eq!(Some(answer), cli.line.as_ref().ok(), "{line:?}");
    }
    assert_eq!(answers[2]["outcome"]["code"], "actingctl_usage_invalid");
    assert_eq!(answers[2]["outcome"]["values"]["argument"], "frobnicate");
    assert_eq!(answers[3]["outcome"]["code"], "actingctl_usage_invalid");
    assert_eq!(
        answers[3]["outcome"]["values"]["argument_problem"],
        "wrong_type"
    );
    // An instance is a string, as the CLI's words give it.
    assert_eq!(answers[4]["outcome"]["values"]["argument"], "instance");
    assert_eq!(
        answers[4]["outcome"]["values"]["argument_problem"],
        "wrong_type"
    );
    assert!(!developer.is_empty(), "the refusals' developer lines");
    set_missing_config_env();
}

/// A 16x9 frame: a 4x4 square of `color` at the top left on gray.
fn square_frame(color: [u8; 3]) -> Vec<u8> {
    let mut pixels = Vec::new();
    for y in 0..9 {
        for x in 0..16 {
            pixels.extend(if x < 4 && y < 4 { color } else { [60, 60, 60] });
        }
    }
    Frame::from_pixels(
        16,
        9,
        pixels,
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame")
    .encode_png_fast()
    .expect("fixture PNG")
}

/// One offline frame of the Lab recording under `lock`, with its colour mark and, with `click`,
/// the step's click.
fn mark(lock: &RecordingLock, ocr: &LabOcr, frame: &Path, page: &str, click: bool) {
    let request = MarkRequest {
        schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
        frame: Some(frame.display().to_string()),
        add: vec![MarkSpec::region_only(
            format!("{page}_color"),
            MarkFamily::Color,
            RecordRect {
                x: 0,
                y: 0,
                width: 4,
                height: 4,
            },
        )],
        click: click.then(|| {
            serde_json::from_value::<ClickSpec>(json!({
                "region": {"x": 8, "y": 2, "width": 4, "height": 4}
            }))
            .expect("a click")
        }),
        ..MarkRequest::default()
    };
    record_mark(lock, &request, false, ocr)
        .unwrap_or_else(|error| panic!("record mark {page}: {}: {}", error.code, error.message));
}

#[test]
fn recording_steps_and_marks_carry_the_initiator() {
    let _guard = env_lock();
    // §4.2: the word of an actinglab process.
    assert_eq!(
        Initiator::of_lab_process(None).ok(),
        Some(Initiator::LabCli)
    );
    assert_eq!(
        Initiator::of_lab_process(Some(OsStr::new("Lab-制包"))).ok(),
        Some(Initiator::LabAuthoring)
    );
    assert_eq!(
        Initiator::of_lab_process(Some(OsStr::new("Lab-救急"))).ok(),
        Some(Initiator::LabRescue)
    );
    for word in ["UI", "Lab-MCP", "Lab-看门狗", "Lab-CLI", "lab-制包", ""] {
        let outcome = match Initiator::of_lab_process(Some(OsStr::new(word))) {
            Ok(initiator) => panic!("{word:?} is no word of an actinglab process: {initiator}"),
            Err(outcome) => outcome,
        };
        let value = cli_line::outcome_value(&outcome, Detail::Full);
        assert_eq!(value["code"], "actingctl_usage_invalid", "{word:?}");
        assert_eq!(
            value["values"]["variable"], ENVIRONMENT_VARIABLE,
            "{word:?}"
        );
    }

    let temp = TempDir::new().unwrap();
    set_config_env(temp.path().join("config.json"));
    let state_dir = temp.path().join("session");
    let state = state_dir.to_str().unwrap().to_owned();
    let frame = temp.path().join("frame.png");
    fs::write(&frame, test_record_frame_png(12, 10)).unwrap();
    let step = |step_id: &str| {
        run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state.as_str(),
                "--kind",
                "anchor",
                "--step-id",
                step_id,
                "--id",
                "page/home",
                "--region",
                "2,3,4,5",
                "--frame",
                frame.to_str().unwrap(),
            ],
            true,
        )
    };
    unsafe {
        env::set_var(ENVIRONMENT_VARIABLE, "Lab-制包");
    }
    let start = run_cli(
        [
            "--json",
            "--instance",
            "ak",
            "session",
            "record",
            "start",
            "--state-dir",
            state.as_str(),
            "--task-id",
            "daily-check",
        ],
        true,
    );
    assert_eq!(start.exit_code(), 0, "{}", start.envelope_json());
    let workbench = step("home-anchor");
    assert_eq!(workbench.exit_code(), 0, "{}", workbench.envelope_json());
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    let command_line = step("mail-anchor");
    assert_eq!(
        command_line.exit_code(),
        0,
        "{}",
        command_line.envelope_json()
    );
    unsafe {
        env::set_var(ENVIRONMENT_VARIABLE, "Lab-MCP");
    }
    let refused = step("other-anchor");
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    assert_eq!(refused.exit_code(), 2, "{}", refused.envelope_json());
    assert_eq!(
        refused.envelope.error.as_ref().unwrap().code,
        "actingctl_usage_invalid"
    );
    let steps = command_line.envelope.data.as_ref().unwrap()["record"]["steps"].clone();
    assert_eq!(
        steps
            .as_array()
            .unwrap()
            .iter()
            .map(|step| (step["step_id"].clone(), step["initiator"].clone()))
            .collect::<Vec<_>>(),
        [
            (json!("home-anchor"), json!("Lab-制包")),
            (json!("mail-anchor"), json!("Lab-CLI")),
        ],
        "nothing was written for the refused word"
    );

    // The Lab recording (`record mark`): each step and mark carries the word of the process that
    // held the lock when it was added.
    let lab_state = temp.path().join("lab-state");
    fs::create_dir_all(&lab_state).unwrap();
    fs::write(
        lab_state.join("record-neutral.instance.json"),
        serde_json::to_vec(&json!({
            "record_id": "record_0001",
            "task_id": "neutral_task",
            "instance": "neutral.instance",
            "status": "active",
            "started_at_unix_ms": 1_700_000_000_000_u64
        }))
        .unwrap(),
    )
    .unwrap();
    let ocr = LabOcr::from_vision_root(&temp.path().join("no-vision"));
    let red = temp.path().join("red.png");
    let blue = temp.path().join("blue.png");
    fs::write(&red, square_frame([230, 20, 20])).unwrap();
    fs::write(&blue, square_frame([20, 20, 230])).unwrap();
    unsafe {
        env::set_var(ENVIRONMENT_VARIABLE, "Lab-救急");
    }
    let word = cli_module::recording_initiator().expect("Lab-救急");
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    {
        let lock = RecordingLock::acquire(&lab_state, "neutral.instance", "gate_434_lab2", word)
            .expect("recording lock");
        record_start(
            &lock,
            &RecordStartOptions {
                game: Some("neutral".to_owned()),
                server: Some("test".to_owned()),
                locale: Some("en-US".to_owned()),
                ..RecordStartOptions::default()
            },
        )
        .expect("record start");
        mark(&lock, &ocr, &red, "home", true);
    }
    {
        let word = cli_module::recording_initiator().expect("Lab-CLI");
        let lock = RecordingLock::acquire(&lab_state, "neutral.instance", "gate_434_lab2", word)
            .expect("recording lock");
        mark(&lock, &ocr, &blue, "list", false);
    }
    let recording: Value = serde_json::from_slice(
        &fs::read(
            lab_state
                .join("record-artifacts")
                .join("record_0001")
                .join("lab")
                .join("recording.json"),
        )
        .expect("the Lab recording"),
    )
    .unwrap();
    let words = recording["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|step| {
            (
                step["initiator"].clone(),
                step["marks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|mark| mark["initiator"].clone())
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        words,
        [
            (json!("Lab-救急"), vec![json!("Lab-救急")]),
            (json!("Lab-CLI"), vec![json!("Lab-CLI")]),
        ]
    );
    set_missing_config_env();
}
