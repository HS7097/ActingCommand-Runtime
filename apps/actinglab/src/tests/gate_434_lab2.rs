// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-2 (register §1.287, "Slice LAB-2"; contracts/lab-two-entries.md §4.2, §8.2,
// §9.3 and open points 6, 7 and 10; coordinator work orders rd-1108/arch-013/brief-lab2.md and
// brief-lab2-fix1.md): the CLI module of actinglab and `actinglab serve --stdio`. LAB-2 fix 1 (the
// coordinator's ruling of 2026-11-10): the slice changes nothing any current consumer of actinglab
// gets; the module's output is that of the operations it serves itself, those from actingctl.
// - `cli_short_words_parse`: the short words of the operation table name the operation and its
//   parameters: the longest name, the instance and the other positionals in order, options and
//   switches, a source command's further flags and words; words and flags the row does not take,
//   a reserved row, a row not served (the rescue module's `clean`, LAB-3) and actinglab's old
//   `config get` are refused with `actingctl_usage_invalid` naming the word. `main`'s route: the
//   module's operations and `serve` go to the module, the short words of a row actinglab's own
//   command serves go to that command in its words.
// - `cli_json_matches_op_output`: through `main`'s entry, a core word prints its line for people
//   by default (into a pipe too) and with `--json` the line `lab-ops` builds for it; a row
//   actinglab's own command serves answers that command's line, as its own words do (LAB-2b: the
//   CLI module's line, no longer the envelope).
// - `serve_answers_same_as_cli`: one answer line per request line, the line the operation's
//   `--json` prints (the module's line, for actinglab's own commands too since LAB-2b); a request
//   that is no operation is refused and the loop goes on.
// - `recording_steps_and_marks_carry_the_initiator` (the work order's fourth test): a recording's
//   steps and marks carry the process's initiator word (§4.2), `ACTINGCOMMAND_INITIATOR` changes
//   it, a word the variable may not name is refused and nothing is written, and through MCP's
//   actinglab child (`--via mcp`) the word is `Lab-MCP` (§4.1).
// - `consumer_command_lines_take_the_old_path_unchanged` (fix 1; renamed in fix 2): every command
//   of actinglab's dispatch is actinglab's own on `main`'s route, its words unchanged. Since
//   Workflow #434 LAB-2b only MCP's `--via mcp` child keeps the old path: each of its command lines
//   prints through `main`'s entry exactly the envelope the same line prints through the
//   parse-and-run path without the route, with the same exit code (0, or 2 to 6), against the
//   same fixture, the request id each run issues masked. That this output is main's is pinned by
//   the integration goldens' MCP cases (`tests/consumer_goldens.rs`); the other consumers' lines
//   are `gate_434_lab2b`'s.
// Fakes only: an empty configuration in a temporary folder, offline frames and fixtures; no
// Runtime, no device, no clock, no sleep.

use super::*;
use crate::cli_module::{self, Route, serve, words};
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

/// The command line `main`'s route gives actinglab's own command, in that command's words.
fn own_route(line: &[&str]) -> Vec<String> {
    match cli_module::route(words_of(line)) {
        Route::Own(own) => own,
        other => panic!("{line:?} is actinglab's own command line: {other:?}"),
    }
}

#[test]
fn cli_short_words_parse() {
    // The instance is a positional; an option keeps its value as written.
    assert_eq!(
        parsed(&[
            "pause",
            "neutral.a",
            "--reason",
            "hotplace-x",
            "--drain",
            "300s"
        ]),
        (
            "pause".to_owned(),
            json!({"instance": "neutral.a", "reason": "hotplace-x", "drain": "300s"})
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
    // A reserved row (`batch`) and a row of a source not built (the rescue module's `clean`;
    // Workflow #434 LAB-3 serves `deploy`, which this line named before).
    assert_eq!(
        refused(&["batch", "neutral.a", "steps.json"]).1,
        json!("batch")
    );
    assert_eq!(refused(&["clean"]).1, json!("clean"));
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

    // LAB-2 fix 1, `main`'s route. `serve` and the operations the module serves itself, those
    // from actingctl, go to the module, `--json` anywhere.
    let module_lines: [&[&str]; 6] = [
        &["serve", "--stdio"],
        &["codes", "actingctl_usage_invalid"],
        &["--json", "config", "set", "a.b=1"],
        &["devices"],
        &["run", "evidence", "req-neutral-0001", "--json"],
        &["drill", "neutral.a"],
    ];
    for line in module_lines {
        match cli_module::route(words_of(line)) {
            Route::Module(module) => assert_eq!(module, words_of(line)),
            other => panic!("{line:?} is the module's: {other:?}"),
        }
    }
    // The short words of a row actinglab's own command serves name that command in its words,
    // the instance as `--instance`, `--json` kept.
    assert_eq!(own_route(&["watch"]), words_of(&["lab", "watch"]));
    assert_eq!(
        own_route(&["--json", "recover", "neutral.a"]),
        words_of(&["--json", "session", "recover", "--instance", "neutral.a"])
    );
    assert_eq!(
        own_route(&["user-config", "instance.neutral.serial"]),
        words_of(&["user-config", "get", "instance.neutral.serial"])
    );
    assert_eq!(
        own_route(&["app", "launch", "16384"]),
        words_of(&["session", "app", "launch", "--instance", "16384"])
    );
    // Refused, they are actinglab's own usage error, naming the operation and the word.
    match cli_module::route(words_of(&["app", "launch"])) {
        Route::Refused {
            command,
            json,
            error,
        } => {
            assert_eq!((command.as_str(), json), ("app launch", false));
            assert_eq!(error.code, "validation_failed");
            assert_eq!(
                error.details,
                Some(json!({"argument": "<instance>", "argument_problem": "required"}))
            );
        }
        other => panic!("app launch without its instance is refused: {other:?}"),
    }
}

#[test]
fn cli_json_matches_op_output() {
    let _guard = env_lock();
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    // A core row through `main`'s entry: the CLI module, the line `lab-ops` builds for the
    // operation (actingctl's `codes`), unchanged.
    let served = actingcommand_lab_ops::code_lookup::dispatch(&[
        OsString::from("codes"),
        OsString::from("actingctl_usage_invalid"),
    ])
    .expect("codes is served");
    let served_line = served.line.clone().expect("the served line");
    assert_eq!(served_line["outcome"]["code"], "outcome_codes_reported");
    for (line, json) in [
        (&["--json", "codes", "actingctl_usage_invalid"][..], true),
        (&["codes", "actingctl_usage_invalid"][..], false),
    ] {
        // Stdout is no terminal here, as in a pipe.
        let module = match entry(words_of(line), true) {
            Err(module) => module,
            Ok(own) => panic!("{line:?} is the CLI module's, not {own:?}"),
        };
        let (asked, words) = cli_module::front_end(module);
        assert_eq!(asked, json, "{line:?}");
        let cli = cli_module::run_line(&words);
        assert_eq!((cli.code, served.code), (0, 0));
        assert_eq!(cli.line.as_ref().ok(), Some(&served_line));
        let printed = cli_module::stdout_text(&cli, asked).expect("the printed text");
        if json {
            // `--json`: the one line `cli_line` builds.
            assert_eq!(printed, served_line.to_string());
        } else {
            // For people by default, into a pipe too: the code with its values and the result's
            // members.
            assert_eq!(printed, cli_module::people(&served_line));
            assert!(printed.starts_with("outcome_codes_reported"), "{printed}");
            assert!(
                serde_json::from_str::<Value>(&printed).is_err(),
                "{printed}"
            );
        }
    }

    // A row actinglab's own command serves, named by its short words: the line that command
    // prints for its own words (Workflow #434 LAB-2b: the CLI module's line), and its exit code.
    let temp = TempDir::new().unwrap();
    set_config_env(temp.path().join("config.json"));
    let set = run_cli(
        [
            "--json",
            "user-config",
            "set",
            "instance.neutral.serial",
            "TESTSERIAL0001",
        ],
        true,
    );
    assert_eq!(set.exit_code(), 0, "{}", set.envelope_json());
    let short = match entry(
        words_of(&["--json", "user-config", "instance.neutral.serial"]),
        false,
    ) {
        Ok(invocation) => cli_module::own_line::answer(invocation),
        Err(module) => {
            panic!("user-config is actinglab's own command, not the module's {module:?}")
        }
    };
    let own = cli_module::own_line::answer(parse_invocation(
        ["--json", "user-config", "get", "instance.neutral.serial"],
        false,
    ));
    assert_eq!(
        cli_module::stdout_text(&short.0, short.1),
        cli_module::stdout_text(&own.0, own.1)
    );
    assert_eq!((short.0.code, own.0.code, short.1), (0, 0, true));
    let line = short.0.line.expect("the line");
    assert_eq!(line["value"], json!("TESTSERIAL0001"));
    assert_eq!(line["outcome"]["code"], "lab_operation_completed");
    set_missing_config_env();
}

#[test]
fn serve_answers_same_as_cli() {
    let _guard = env_lock();
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    let temp = TempDir::new().unwrap();
    set_config_env(temp.path().join("config.json"));
    let set = run_cli(
        [
            "--json",
            "user-config",
            "set",
            "instance.neutral.serial",
            "TESTSERIAL0001",
        ],
        true,
    );
    assert_eq!(set.exit_code(), 0, "{}", set.envelope_json());
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
    let output = String::from_utf8(output).unwrap();
    let lines = output.lines().collect::<Vec<_>>();
    assert_eq!(
        lines.len(),
        5,
        "one answer per request, none for the blank line"
    );
    // An operation of the module: the CLI's `--json` line.
    let cli = cli_module::run_line(&words_of(&["codes", "actingctl_usage_invalid"]));
    assert_eq!(
        lines[0],
        cli_module::stdout_text(&cli, true).expect("the CLI's line")
    );
    // An operation actinglab's own command serves: the line `actinglab --json` prints for it
    // (Workflow #434 LAB-2b: the CLI module's line).
    let (own, json) = cli_module::own_line::answer(parse_invocation(
        ["--json", "user-config", "get", "instance.neutral.serial"],
        false,
    ));
    assert!(json);
    assert_eq!(
        lines[1],
        cli_module::stdout_text(&own, true).expect("the command's line")
    );
    let answers = lines
        .iter()
        .map(|line| serde_json::from_str::<Value>(line).expect("one JSON answer per line"))
        .collect::<Vec<_>>();
    assert_eq!(answers[1]["outcome"]["code"], "lab_operation_completed");
    assert_eq!(answers[1]["value"], "TESTSERIAL0001");
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
/// the step's click; the answer `record mark` prints as its `data`.
fn mark(lock: &RecordingLock, ocr: &LabOcr, frame: &Path, page: &str, click: bool) -> Value {
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
    let outcome = record_mark(lock, &request, false, ocr)
        .unwrap_or_else(|error| panic!("record mark {page}: {}: {}", error.code, error.message));
    serde_json::to_value(outcome).expect("the answer of record mark")
}

/// The `initiator` of each mark an answer of `record mark` lists.
fn answered_words(answer: &Value) -> Vec<Value> {
    answer["marks"]
        .as_array()
        .expect("the answer's marks")
        .iter()
        .map(|mark| mark["initiator"].clone())
        .collect()
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
    // §4.1 (fix 1): through MCP's actinglab child the word is `Lab-MCP`, whatever the variable
    // says; `--via ui` is no MCP child and keeps §4.2's word.
    unsafe {
        env::set_var(ENVIRONMENT_VARIABLE, "Lab-制包");
    }
    assert_eq!(
        cli_module::recording_initiator(Some(ChannelVia::Mcp)).ok(),
        Some("Lab-MCP")
    );
    assert_eq!(
        cli_module::recording_initiator(Some(ChannelVia::Ui)).ok(),
        Some("Lab-制包")
    );
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    assert_eq!(cli_module::recording_initiator(None).ok(), Some("Lab-CLI"));

    let temp = TempDir::new().unwrap();
    set_config_env(temp.path().join("config.json"));
    let state_dir = temp.path().join("session");
    let state = state_dir.to_str().unwrap().to_owned();
    let frame = temp.path().join("frame.png");
    fs::write(&frame, test_record_frame_png(12, 10)).unwrap();
    // One session step, through the command line's own parsing; `--via mcp` as MCP's child
    // sends it.
    let step = |step_id: &str, via_mcp: bool| {
        let mut line = vec!["--json"];
        if via_mcp {
            line.extend(["--via", "mcp"]);
        }
        line.extend([
            "--instance",
            "neutral.instance",
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
        ]);
        run_cli(line, true)
    };
    unsafe {
        env::set_var(ENVIRONMENT_VARIABLE, "Lab-制包");
    }
    let start = run_cli(
        [
            "--json",
            "--instance",
            "neutral.instance",
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
    let workbench = step("home-anchor", false);
    assert_eq!(workbench.exit_code(), 0, "{}", workbench.envelope_json());
    let through_mcp = step("mcp-anchor", true);
    assert_eq!(
        through_mcp.exit_code(),
        0,
        "{}",
        through_mcp.envelope_json()
    );
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    let command_line = step("mail-anchor", false);
    assert_eq!(
        command_line.exit_code(),
        0,
        "{}",
        command_line.envelope_json()
    );
    unsafe {
        env::set_var(ENVIRONMENT_VARIABLE, "Lab-MCP");
    }
    let refused = step("other-anchor", false);
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    assert_eq!(refused.exit_code(), 2, "{}", refused.envelope_json());
    assert_eq!(
        refused.envelope.error.as_ref().unwrap().code,
        "actingctl_usage_invalid"
    );
    let last = step("last-anchor", false);
    assert_eq!(last.exit_code(), 0, "{}", last.envelope_json());
    let steps = last.envelope.data.as_ref().unwrap()["record"]["steps"].clone();
    assert_eq!(
        steps
            .as_array()
            .unwrap()
            .iter()
            .map(|step| (step["step_id"].clone(), step["initiator"].clone()))
            .collect::<Vec<_>>(),
        [
            (json!("home-anchor"), json!("Lab-制包")),
            (json!("mcp-anchor"), json!("Lab-MCP")),
            (json!("mail-anchor"), json!("Lab-CLI")),
            (json!("last-anchor"), json!("Lab-CLI")),
        ],
        "nothing was written for the refused word"
    );

    // The Lab recording (`record mark`): each step and mark carries the word of the process that
    // held the lock when it was added, and (fix 2) the answer of `record mark` carries the word
    // the file does.
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
    let word = cli_module::recording_initiator(None).expect("Lab-救急");
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
        let answer = mark(&lock, &ocr, &red, "home", true);
        assert_eq!(answered_words(&answer), [json!("Lab-救急")], "{answer}");
    }
    // MCP's actinglab child (`ac_record_mark`): `--json --via mcp … record mark --request-json …`
    // through `main`'s entry; its answer is the envelope's `data`.
    let request = json!({
        "schema_version": LAB_RECORD_MARK_SCHEMA,
        "frame": blue.to_str().unwrap(),
        "add": [{
            "id": "list_color",
            "family": "color",
            "region": {"x": 0, "y": 0, "width": 4, "height": 4}
        }]
    })
    .to_string();
    let through_mcp = words_of(&[
        "--json",
        "--via",
        "mcp",
        "--instance",
        "neutral.instance",
        "record",
        "mark",
        "--state-dir",
        lab_state.to_str().unwrap(),
        "--request-json",
        request.as_str(),
    ]);
    let answered = match entry(through_mcp.clone(), true) {
        Ok(invocation) => run_invocation(invocation),
        Err(module) => panic!("{through_mcp:?} is actinglab's own command line: {module:?}"),
    };
    assert_eq!(answered.exit_code(), 0, "{}", answered.envelope_json());
    let answer = answered.envelope.data.clone().expect("the answer's data");
    assert_eq!(answered_words(&answer), [json!("Lab-MCP")], "{answer}");
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
            (json!("Lab-MCP"), vec![json!("Lab-MCP")]),
        ]
    );
    set_missing_config_env();
}

/// What `line` prints through the parse-and-run path without `main`'s route: the command line
/// parsed (`json_default` when stdout is no terminal), run, and printed as the envelope or as the
/// text for people, by this build's code (the steps `main` took before LAB-2); the printed line
/// and the exit code. This build's, not main's: the integration goldens compare with main.
fn without_the_route(line: &[String], json_default: bool) -> (String, i32) {
    printed(&run_invocation(parse_invocation(
        line.iter().cloned(),
        json_default,
    )))
}

/// What `main` prints for the `--via` line `line` now (Workflow #434 LAB-2b: the envelope path),
/// stdout a terminal or not (`piped`): its entry, then the same run and print.
fn through_main(line: &[String], piped: bool) -> (String, i32) {
    assert!(cli_module::own_line::via_line(line), "{line:?}");
    match entry(
        line.to_vec(),
        cli_module::own_line::envelope_by_default(line, piped),
    ) {
        Ok(invocation) => {
            let (text, code) = enveloped(&run_invocation(invocation));
            (text, i32::from(code))
        }
        Err(module) => {
            panic!("{line:?} is actinglab's own command line, not the module's {module:?}")
        }
    }
}

fn printed(result: &CliResult) -> (String, i32) {
    let text = if result.print_json {
        result.envelope_json()
    } else {
        result.human_text()
    };
    (text, result.exit_code())
}

/// `printed` with each request id its run issued (`req-<hex>-<n>`, new in every run, as
/// `detect-page`'s refusal details carry it) as `<REQ_ID>`: the one member two runs of the same
/// line never share (fix 2; the protocol goldens mask it the same way). Everything else stays.
fn run_ids_masked((text, code): (String, i32)) -> (String, i32) {
    let mut masked = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(start) = rest.find("req-") {
        masked.push_str(&rest[..start]);
        let after = &rest[start + "req-".len()..];
        let hex = after
            .find(|character: char| !character.is_ascii_hexdigit())
            .unwrap_or(after.len());
        let tail = &after[hex..];
        let digits = tail.strip_prefix('-').map(|number| {
            number
                .find(|character: char| !character.is_ascii_digit())
                .unwrap_or(number.len())
        });
        match digits {
            Some(digits) if hex > 0 && digits > 0 => {
                masked.push_str("<REQ_ID>");
                rest = &tail[1 + digits..];
            }
            _ => {
                masked.push_str("req-");
                rest = after;
            }
        }
    }
    masked.push_str(rest);
    (masked, code)
}

#[test]
fn consumer_command_lines_take_the_old_path_unchanged() {
    let _guard = env_lock();
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    // Every command of actinglab's dispatch (the snapshot the architecture guard keeps equal to
    // `execute`) is actinglab's own on `main`'s route, unchanged, with leading global options too.
    let snapshot: Value = serde_json::from_slice(
        &fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ratchet/actinglab_commands.json"),
        )
        .expect("the command snapshot"),
    )
    .expect("the command snapshot parses");
    let commands = snapshot["commands"].as_array().expect("its commands");
    assert!(!commands.is_empty());
    for command in commands {
        let command = command.as_str().expect("a command");
        for prefix in [
            &[][..],
            &["--json"][..],
            &["--json", "--run-root", "runs"][..],
        ] {
            let line = prefix
                .iter()
                .copied()
                .chain(command.split(' '))
                .collect::<Vec<_>>();
            assert_eq!(own_route(&line), words_of(&line), "{line:?}");
        }
    }

    let temp = TempDir::new().unwrap();
    set_config_env(temp.path().join("config.json"));
    // A resource repository with one declaration.
    let repo_path = temp.path().join("repo");
    fs::create_dir_all(&repo_path).unwrap();
    fs::write(
        repo_path.join("declaration-exclusions.json"),
        br#"{"schema_version":"actingcommand.declaration-exclusions.v1","directories":["regression"]}"#,
    )
    .unwrap();
    let repo = repo_path.to_str().unwrap();
    // A package and a scene that do not exist: the commands' own refusals.
    let package_path = temp.path().join("packs").join("neutral.pack");
    let package = package_path.to_str().unwrap();
    let scene_path = temp.path().join("scene.png");
    let scene = scene_path.to_str().unwrap();

    // Workflow #434 LAB-2b: only MCP's actinglab child (`lab_ops::lab_child`, `--json --via mcp
    // …`) keeps the old path, the envelope, until LAB-5; every other consumer's line prints the
    // CLI module's line (`gate_434_lab2b`). Stdout redirected, and the same at a terminal.
    let lines: [(&[&str], bool); 8] = [
        (
            &[
                "--json",
                "--via",
                "mcp",
                "resource",
                "validate",
                "--repo",
                repo,
                "--changed-path",
                "declaration-exclusions.json",
            ],
            true,
        ),
        (
            &[
                "--json",
                "--via",
                "mcp",
                "resource",
                "validate",
                "--repo",
                repo,
                "--changed-path",
                "declaration-exclusions.json",
            ],
            false,
        ),
        (
            &[
                "--json",
                "--via",
                "mcp",
                "package",
                "check",
                "--package",
                package,
            ],
            true,
        ),
        (
            &[
                "--json",
                "--via",
                "mcp",
                "package",
                "check",
                "--package",
                package,
            ],
            false,
        ),
        (
            &[
                "--json",
                "--via",
                "mcp",
                "package",
                "preflight",
                "--package",
                package,
            ],
            true,
        ),
        (
            &[
                "--json",
                "--via",
                "mcp",
                "observe",
                "--package",
                package,
                "--scene",
                scene,
            ],
            true,
        ),
        (
            &[
                "--json",
                "--via",
                "mcp",
                "detect-page",
                "--package",
                package,
                "--scene",
                scene,
            ],
            true,
        ),
        (
            &["--via", "mcp", "package", "check", "--package", package],
            true,
        ),
    ];
    for (line, piped) in lines {
        let line = words_of(line);
        let now = run_ids_masked(through_main(&line, piped));
        assert_eq!(
            now,
            run_ids_masked(without_the_route(&line, piped)),
            "{line:?}"
        );
        // The envelope MCP's child reads: `ok`, and `data` or `error`; exit 0 or 2 to 6.
        let envelope: Value = serde_json::from_str(&now.0).expect("the envelope");
        let ok = envelope["ok"].as_bool().expect("ok");
        assert_eq!(ok, now.1 == 0, "{line:?}: {}", now.0);
        assert!(
            now.1 == 0 || (2..=6).contains(&now.1),
            "{line:?}: {}",
            now.1
        );
        assert!(
            envelope.get(if ok { "data" } else { "error" }).is_some(),
            "{line:?}: {}",
            now.0
        );
    }
    // What MCP reads of a success.
    let validated = through_main(&words_of(lines[0].0), true);
    let validated: Value = serde_json::from_str(&validated.0).unwrap();
    assert_eq!(
        validated["data"]["entries"][0]["status"], "valid",
        "{validated}"
    );

    // The hotplace kit's offline composition writes its `--out`: `main` parses it as before.
    let compose = words_of(&[
        "--json",
        "standard-pack",
        "compose",
        "--root",
        temp.path().join("packages").to_str().unwrap(),
        "--instance",
        "neutral.a=neutral/test",
        "--out",
        temp.path().join("composed").to_str().unwrap(),
    ]);
    match entry(compose.clone(), true) {
        Ok(invocation) => assert_eq!(
            format!("{invocation:?}"),
            format!("{:?}", parse_invocation(compose.iter().cloned(), true))
        ),
        Err(module) => panic!("compose is actinglab's own command, not the module's {module:?}"),
    }
    set_missing_config_env();
}
