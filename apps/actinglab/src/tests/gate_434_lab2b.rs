// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-2b (register §1.287, "Slice LAB-2b"; contracts/lab-two-entries.md §8.2, §9.3
// and open point 10; coordinator work order rd-1108/arch-013/brief-lab2b.md and the coordinator's
// ruling of 2026-11-10 on the two results that name `outcome`): actinglab's own commands print the
// CLI module's line.
// - `own_commands_print_the_cli_line`: with `--json` an own command prints one line: its result's
//   members flat beside `outcome` (`lab_operation_completed`, success), or `{outcome (its own code,
//   error, the values the catalog registers for it), message, details (whole)}`; warnings stay a
//   member of the result; the global options before or after the words print the same line; the
//   exit codes are 0, 1 and 2 only.
// - `own_commands_print_for_people_into_a_pipe`: without `--json`, with stdout redirected (the
//   hotplace kit's `standard-pack check`), the text for people, never JSON.
// - `mcp_child_lines_keep_the_envelope`: a `--json --via mcp …` line prints through `main`'s path
//   the envelope the parse-and-run path prints, byte for byte, with its exit codes 2 to 6 (the
//   consumer goldens pin it against main's code, `tests/consumer_goldens.rs`).
// - `status_and_run_show_take_their_core_rows`: the two words reach the CLI module's core rows on
//   the command line, actinglab's old implementations are gone, and the command line answers what
//   `serve --stdio` answers, `status` with its `watchdog` block.
// - `own_results_never_name_outcome`: no own command's line carries a top-level `outcome` other
//   than its own; `resource verify` and `run summary` carry their documents whole under `report`
//   and `summary`; a result that names `outcome` without a declared member is refused, never
//   flattened.
// - `do_check_failure_exits_one`: `do`'s `lab_operation_check_failed` exits 1 on the command line
//   and 5 with `--via mcp`.
// Fakes only: an empty configuration in a temporary folder, offline fixtures; no Runtime, no
// device, no clock, no sleep.

use super::*;
use crate::cli_module::{self, own_line, serve};
use actingcommand_contract::codes::RuntimeContractCode;
use actingcommand_contract::outcome::OUTCOME_SCHEMA_VERSION;
use actingcommand_contract::{CorrelationId, EffectDisposition, RuntimeReceiptState};
use actingcommand_lab_ops::cli_line::Exit;
use actingcommand_lab_ops::initiator::ENVIRONMENT_VARIABLE;
use actingcommand_runtime_client::LabOperationAnswer;
use actingcommand_runtime_client::test_support::lab_operation_check_failure;

const REQUEST: &str = "request_0123456789abcdef0123456789abcdef";
const CORRELATION: &str = "correlation_fedcba9876543210fedcba9876543210";
const COMPLETED: &str = "lab_operation_completed";

fn words_of(line: &[&str]) -> Vec<String> {
    line.iter().map(|word| (*word).to_owned()).collect()
}

/// The paths of the offline fixture, as command-line words.
struct Fixture {
    repo: String,
    root: String,
    not_a_root: String,
    missing: String,
    runs: String,
}

/// A resource repository with one declaration and a changed-path list, NAV-T's installed game
/// root, a file that is no root, a package that is not there and a run root.
fn fixture(temp: &TempDir) -> Fixture {
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    fs::write(
        repo.join("declaration-exclusions.json"),
        br#"{"schema_version":"actingcommand.declaration-exclusions.v1","directories":["regression"]}"#,
    )
    .unwrap();
    let root = crate::standard_pack_cli::gate_373_navt::game_root(
        temp,
        crate::standard_pack_cli::gate_373_navt::HOME_TO_SHOP,
    );
    let not_a_root = temp.path().join("not-a-root");
    fs::write(&not_a_root, b"no game root\n").unwrap();
    let runs = temp.path().join("runs");
    fs::create_dir_all(&runs).unwrap();
    // The packs directory is there; its package `missing` is not.
    fs::create_dir_all(temp.path().join("packs")).unwrap();
    let text = |path: &Path| path.to_str().expect("a UTF-8 path").to_owned();
    Fixture {
        repo: text(repo.as_path()),
        root: text(root.as_path()),
        not_a_root: text(not_a_root.as_path()),
        missing: text(temp.path().join("packs").join("missing").as_path()),
        runs: text(runs.as_path()),
    }
}

/// What `main` prints for actinglab's own command line `line` (no `--via`), stdout redirected:
/// its exit and its stdout text.
fn printed_own(line: &[String]) -> (Exit, String) {
    assert!(!own_line::via_line(line), "{line:?}");
    let invocation = match entry(line.to_vec(), own_line::envelope_by_default(line, true)) {
        Ok(invocation) => invocation,
        Err(module) => {
            panic!("{line:?} is actinglab's own command line, not the module's {module:?}")
        }
    };
    let (exit, json) = own_line::answer(invocation);
    let text = cli_module::stdout_text(&exit, json)
        .unwrap_or_else(|text| panic!("{line:?} printed no line: {text}"));
    (exit, text)
}

/// The one JSON line `text` is.
fn one_line(text: &str) -> Value {
    assert!(!text.contains('\n'), "one line: {text}");
    serde_json::from_str(text).unwrap_or_else(|error| panic!("no JSON line ({error}): {text}"))
}

/// The members of `line`, in order.
fn names(line: &Value) -> Vec<String> {
    line.as_object()
        .expect("a line is an object")
        .keys()
        .cloned()
        .collect()
}

#[test]
fn own_commands_print_the_cli_line() {
    let _guard = env_lock();
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    let temp = TempDir::new().unwrap();
    set_config_env(temp.path().join("config.json"));
    let paths = fixture(&temp);
    let mut codes = BTreeSet::new();

    // A success, the global options before and after the words: the result's members flat
    // beside the line's own outcome, as the result's data holds them.
    let validate = words_of(&[
        "--json",
        "resource",
        "validate",
        "--repo",
        paths.repo.as_str(),
        "--changed-path",
        "declaration-exclusions.json",
    ]);
    let validate_after = words_of(&[
        "resource",
        "validate",
        "--repo",
        paths.repo.as_str(),
        "--changed-path",
        "declaration-exclusions.json",
        "--json",
    ]);
    let (exit, text) = printed_own(&validate);
    assert_eq!(printed_own(&validate_after).1, text);
    codes.insert(exit.code);
    assert_eq!(exit.code, 0, "{text}");
    assert!(exit.developer.is_none(), "{text}");
    let line = one_line(&text);
    assert_eq!(line["outcome"]["schema_version"], OUTCOME_SCHEMA_VERSION);
    assert_eq!(line["outcome"]["code"], COMPLETED);
    assert_eq!(line["outcome"]["category"], "success");
    assert_eq!(line["entries"][0]["status"], "valid", "{line}");
    let data = run_invocation(parse_invocation(validate.iter().cloned(), false))
        .envelope
        .data
        .expect("the result");
    let mut flat = data.as_object().expect("an object result").clone();
    flat.insert("outcome".to_owned(), line["outcome"].clone());
    assert_eq!(line, Value::Object(flat));

    // A failure, `--run-root` before and after the words: its outcome, its message and its
    // details nested whole; exit 1 and the developer line.
    let preflight = words_of(&[
        "--run-root",
        paths.runs.as_str(),
        "--json",
        "package",
        "preflight",
        "--package",
        paths.missing.as_str(),
    ]);
    let preflight_after = words_of(&[
        "package",
        "preflight",
        "--package",
        paths.missing.as_str(),
        "--run-root",
        paths.runs.as_str(),
        "--json",
    ]);
    let (exit, text) = printed_own(&preflight);
    assert_eq!(printed_own(&preflight_after).1, text);
    codes.insert(exit.code);
    assert_eq!(exit.code, 1, "{text}");
    let line = one_line(&text);
    let error = run_invocation(parse_invocation(preflight.iter().cloned(), false))
        .envelope
        .error
        .expect("the refusal");
    assert_eq!(names(&line), ["message", "details", "outcome"]);
    assert_eq!(line["outcome"]["code"], json!(error.code));
    assert_eq!(line["outcome"]["code"], "package_invalid");
    assert_eq!(line["outcome"]["category"], "error");
    assert_eq!(line["message"], json!(error.message));
    assert_eq!(Some(&line["details"]), error.details.as_ref());
    let details = &line["details"];
    for (name, value) in line["outcome"]["values"].as_object().expect("values") {
        assert_eq!(&details[name.as_str()], value, "{name}: {line}");
    }
    let developer = exit.developer.expect("the failure's developer line");
    assert!(
        developer
            .line(cli_module::PROGRAM)
            .starts_with("ERROR actinglab: package_invalid"),
        "{}",
        developer.line(cli_module::PROGRAM)
    );

    // An unknown word and refused short words: actinglab's own usage error, exit 1.
    for words in [
        &["--json", "frobnicate"][..],
        &["--json", "app", "launch"][..],
    ] {
        let (exit, text) = printed_own(&words_of(words));
        codes.insert(exit.code);
        assert_eq!(exit.code, 1, "{words:?}: {text}");
        let line = one_line(&text);
        assert_eq!(line["outcome"]["code"], "validation_failed", "{words:?}");
        assert_eq!(line["outcome"]["values"], json!({}), "{words:?}");
        assert!(line["message"].is_string(), "{words:?}");
    }
    let (_, refused) = printed_own(&words_of(&["--json", "app", "launch"]));
    assert_eq!(
        one_line(&refused)["details"],
        json!({"argument": "<instance>", "argument_problem": "required"})
    );

    // Warnings a result carries stay its member in their own shape; the category stays success
    // and no developer line is written. A result that is no object goes under `result`.
    let warned = json!({
        "status": "written",
        "warnings": [{"code": "resource_pool_producer_missing", "values": {"path": "pools.json"}}]
    });
    let exit = own_line::success("package bundle", warned.clone());
    codes.insert(exit.code);
    let line = exit.line.clone().expect("the line");
    assert_eq!(line["warnings"], warned["warnings"]);
    assert_eq!(line["outcome"]["category"], "success");
    assert!(exit.developer.is_none());
    let text = own_line::success("schema", json!("neutral text"));
    assert_eq!(
        names(text.line.as_ref().expect("the line")),
        ["result", "outcome"]
    );
    assert_eq!(text.line.unwrap()["result"], "neutral text");

    assert!(codes.iter().all(|code| *code <= 2), "{codes:?}");
    set_missing_config_env();
}

#[test]
fn own_commands_print_for_people_into_a_pipe() {
    let _guard = env_lock();
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    let temp = TempDir::new().unwrap();
    set_config_env(temp.path().join("config.json"));
    let paths = fixture(&temp);

    // The hotplace kit's line: no `--json`, stdout redirected. No envelope by default any more.
    let check = words_of(&["standard-pack", "check", "--root", paths.root.as_str()]);
    assert!(!own_line::envelope_by_default(&check, true));
    let (exit, text) = printed_own(&check);
    assert_eq!(exit.code, 0, "{text}");
    assert_eq!(text, cli_module::people(exit.line.as_ref().unwrap()));
    assert!(text.starts_with("lab_operation_completed\n"), "{text}");
    assert!(
        text.lines()
            .any(|row| row.trim_start().starts_with("status") && row.ends_with("checked")),
        "{text}"
    );
    assert!(serde_json::from_str::<Value>(&text).is_err(), "{text}");

    // A failure for people: the code, then its message.
    let failing = words_of(&[
        "standard-pack",
        "check",
        "--root",
        paths.not_a_root.as_str(),
    ]);
    let (exit, text) = printed_own(&failing);
    assert_eq!(exit.code, 1, "{text}");
    assert_eq!(
        text,
        format!(
            "validation_failed\n  message  --root {} is no directory",
            paths.not_a_root
        )
    );
    assert!(exit.developer.is_some());

    // A `--via` line keeps its envelope by default when stdout is no terminal.
    let via = words_of(&[
        "--via",
        "mcp",
        "standard-pack",
        "check",
        "--root",
        paths.root.as_str(),
    ]);
    assert!(own_line::envelope_by_default(&via, true));
    assert!(!own_line::envelope_by_default(&via, false));
    set_missing_config_env();
}

#[test]
fn mcp_child_lines_keep_the_envelope() {
    let _guard = env_lock();
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    let temp = TempDir::new().unwrap();
    set_config_env(temp.path().join("config.json"));
    let paths = fixture(&temp);
    // `record mark`'s answer through `--json --via mcp` (its `initiator`) is pinned by
    // `gate_434_lab2::recording_steps_and_marks_carry_the_initiator`, `do`'s exit 5 by
    // `do_check_failure_exits_one`.
    let lines: [(&[&str], i32); 3] = [
        (
            &[
                "--json",
                "--via",
                "mcp",
                "resource",
                "validate",
                "--repo",
                paths.repo.as_str(),
                "--changed-path",
                "declaration-exclusions.json",
            ],
            0,
        ),
        (
            &[
                "--json",
                "--via",
                "mcp",
                "package",
                "check",
                "--package",
                paths.missing.as_str(),
            ],
            2,
        ),
        // Without `--json` into a pipe, the envelope as before.
        (
            &[
                "--via",
                "mcp",
                "package",
                "check",
                "--package",
                paths.missing.as_str(),
            ],
            2,
        ),
    ];
    for (line, code) in lines {
        let line = words_of(line);
        assert!(own_line::via_line(&line), "{line:?}");
        let piped = own_line::envelope_by_default(&line, true);
        assert!(piped, "{line:?}");
        let through_main = match entry(line.clone(), piped) {
            Ok(invocation) => enveloped(&run_invocation(invocation)),
            Err(module) => panic!("{line:?} is actinglab's own command line: {module:?}"),
        };
        let without_route = enveloped(&run_invocation(parse_invocation(
            line.iter().cloned(),
            true,
        )));
        assert_eq!(through_main, without_route, "{line:?}");
        assert_eq!(
            i32::from(through_main.1),
            code,
            "{line:?}: {}",
            through_main.0
        );
        let envelope: Value = serde_json::from_str(&through_main.0)
            .unwrap_or_else(|error| panic!("{line:?}: no envelope ({error}): {}", through_main.0));
        assert_eq!(envelope["schema_version"], CLI_SCHEMA_VERSION, "{envelope}");
        let ok = envelope["ok"].as_bool().expect("ok");
        assert_eq!(ok, code == 0, "{envelope}");
        assert!(
            envelope.get(if ok { "data" } else { "error" }).is_some(),
            "{envelope}"
        );
        assert!(envelope.get("outcome").is_none(), "{envelope}");
    }
    set_missing_config_env();
}

#[test]
fn status_and_run_show_take_their_core_rows() {
    let _guard = env_lock();
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    // `main`'s route gives both words to the CLI module, `--json` anywhere.
    for line in [
        &["status"][..],
        &["--json", "status"][..],
        &["status", "--json"][..],
        &["run", "show", REQUEST][..],
        &["--json", "run", "show", REQUEST][..],
    ] {
        match entry(words_of(line), false) {
            Err(module) => assert_eq!(module, words_of(line)),
            Ok(own) => panic!("{line:?} is the CLI module's core row, not {own:?}"),
        }
    }
    assert!(!own_command(&words_of(&["status"])));
    assert!(!own_command(&words_of(&["run", "show"])));
    assert!(own_command(&words_of(&["run", "summary"])));
    // actinglab's old implementations are gone: its own dispatch knows neither word.
    let temp = TempDir::new().unwrap();
    set_config_env(temp.path().join("config.json"));
    for (line, message) in [
        (
            &["--json", "status"][..],
            "unknown actinglab command: status",
        ),
        (
            &["--json", "run", "show", REQUEST][..],
            "unknown run command: show",
        ),
    ] {
        let old = run_cli(line.iter().copied(), false);
        let error = old.envelope.error.as_ref().expect("no own command");
        assert_eq!(
            (error.code.as_str(), error.message.as_str()),
            ("validation_failed", message)
        );
    }

    // The command line answers what `serve --stdio` answers for the same row, `status` with its
    // `watchdog` block (null without an install root). No Runtime runs on the state root.
    let state_root = temp.path().join("state");
    fs::create_dir_all(&state_root).unwrap();
    unsafe {
        env::set_var(RUNTIME_STATE_ROOT_ENV, &state_root);
    }
    let table = cli_module::table().expect("the operation table");
    let input = format!(
        "{}\n{}\n",
        json!({"op": "status", "params": {}}),
        json!({"op": "run show", "params": {"id": REQUEST}})
    );
    let mut output = Vec::new();
    let mut developer = Vec::new();
    serve::serve(table, input.as_bytes(), &mut output, &mut developer).expect("the loop ends");
    let output = String::from_utf8(output).unwrap();
    let served = output.lines().collect::<Vec<_>>();
    assert_eq!(served.len(), 2, "{output}");
    let status = cli_module::run_line(&words_of(&["status"]));
    assert_eq!(
        cli_module::stdout_text(&status, true).expect("the status line"),
        served[0]
    );
    let line: Value = serde_json::from_str(served[0]).unwrap();
    assert_eq!(
        line["outcome"]["code"], "runtime_overview_reported",
        "{line}"
    );
    assert!(line.get("watchdog").is_some(), "{line}");
    let shown = cli_module::run_line(&words_of(&["run", "show", REQUEST]));
    assert_eq!(
        cli_module::stdout_text(&shown, true).expect("the run show line"),
        served[1]
    );
    let line: Value = serde_json::from_str(served[1]).unwrap();
    assert!(line["outcome"]["code"].is_string(), "{line}");
    unsafe {
        env::remove_var(RUNTIME_STATE_ROOT_ENV);
    }
    set_missing_config_env();
}

#[test]
fn own_results_never_name_outcome() {
    let _guard = env_lock();
    let snapshot: Value = serde_json::from_slice(
        &fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ratchet/actinglab_commands.json"),
        )
        .expect("the command snapshot"),
    )
    .expect("the command snapshot parses");
    let commands = snapshot["commands"]
        .as_array()
        .expect("its commands")
        .iter()
        .map(|command| command.as_str().expect("a command").to_owned())
        .collect::<Vec<_>>();
    assert!(!commands.is_empty());
    // The two declared commands are commands of actinglab's dispatch.
    assert_eq!(
        own_line::WRAPPED,
        [("resource verify", "report"), ("run summary", "summary")]
    );
    for (command, _) in own_line::WRAPPED {
        assert!(commands.iter().any(|own| own == command), "{command}");
    }

    // Every own command: a result is flattened beside the line's own outcome only, and a result
    // that names `outcome` is never flattened: its declared member carries it whole, and without
    // one the line is refused (nothing on stdout, exit 1).
    let ordinary = json!({"status": "neutral"});
    let colliding = json!({"status": "neutral", "outcome": {"kind": "neutral"}});
    for command in &commands {
        let exit = own_line::success(command, ordinary.clone());
        let line = exit.line.expect("an ordinary result's line");
        match own_line::wrapping_member(command) {
            Some(member) => assert_eq!(line[member], ordinary, "{command}"),
            None => assert_eq!(names(&line), ["status", "outcome"], "{command}"),
        }
        assert_eq!(line["outcome"]["code"], COMPLETED, "{command}");
        let exit = own_line::success(command, colliding.clone());
        match own_line::wrapping_member(command) {
            Some(member) => {
                let line = exit.line.expect("a declared result's line");
                assert_eq!(names(&line), [member, "outcome"], "{command}");
                assert_eq!(line[member], colliding, "{command}");
                assert_eq!(line["outcome"]["code"], COMPLETED, "{command}");
            }
            None => {
                assert!(
                    exit.line.is_err(),
                    "{command}: a second outcome was flattened"
                );
                assert!(cli_module::stdout_text(&exit, true).is_err(), "{command}");
                assert_eq!(exit.code, 1, "{command}");
            }
        }
    }

    // `resource verify` and `run summary` carry their documents unchanged under `report` and
    // `summary`, whose own `outcome` stays inside.
    let report = json!({
        "schema_version": "actinglab.resource-verify.v1",
        "result": "passed",
        "outcome": {
            "schema_version": OUTCOME_SCHEMA_VERSION,
            "code": "resource_verify_passed",
            "category": "success",
            "values": {"step_count": 7}
        },
        "steps": [],
        "failed_steps": []
    });
    let summary = json!({
        "schema_version": "actingcommand.run-summary.v1",
        "status": "succeeded",
        "outcome": {"kind": "fixture_simulation", "policy": "completed", "result": "no_op"},
        "effect": "no_op"
    });
    for (command, member, document) in [
        ("resource verify", "report", report),
        ("run summary", "summary", summary),
    ] {
        let result = CliResult::ok(command.to_owned(), document.clone(), true, String::new());
        let exit = own_line::exit(&result);
        assert_eq!(exit.code, 0, "{command}");
        let text = cli_module::stdout_text(&exit, true).expect("the line");
        let line = one_line(&text);
        assert_eq!(names(&line), [member, "outcome"], "{command}");
        assert_eq!(line[member], document, "{command}");
        assert_eq!(line["outcome"]["code"], COMPLETED, "{command}");
    }

    // The own commands that run offline with nothing to read answer no `outcome` of their own.
    let temp = TempDir::new().unwrap();
    set_config_env(temp.path().join("config.json"));
    for command in [
        "help",
        "version",
        "paths",
        "schema",
        "list",
        "capabilities",
        "doctor",
    ] {
        let result = run_cli(["--json", command], false);
        let data = result
            .envelope
            .data
            .clone()
            .unwrap_or_else(|| panic!("{command}: {}", result.envelope_json()));
        assert!(data.get("outcome").is_none(), "{command}");
        let line = own_line::exit(&result).line.expect("the line");
        assert_eq!(line["outcome"]["code"], COMPLETED, "{command}");
    }
    set_missing_config_env();
}

#[test]
fn do_check_failure_exits_one() {
    let answer = LabOperationAnswer {
        request_id: serde_json::from_value(json!(REQUEST)).expect("a canonical request id"),
        correlation_id: serde_json::from_value::<CorrelationId>(json!(CORRELATION))
            .expect("a canonical correlation id"),
        receipt_state: RuntimeReceiptState::Completed,
        effect: Some(EffectDisposition::Performed),
        failure: None,
    };
    let client_code = RuntimeContractCode::RuntimeLabDiagnosticBudgetMismatch.as_str();
    let error = crate::lab2_cli::lab_operation_call_error(lab_operation_check_failure(
        RuntimeContractCode::RuntimeLabDiagnosticBudgetMismatch,
        answer,
    ));
    assert_eq!(error.code, "lab_operation_check_failed");
    let details = error.details.clone().expect("the details");

    // The command line: the CLI module's line, exit 1, the catalog's values of the code beside
    // the message and the details whole.
    let result = CliResult::err("do".to_owned(), error.clone(), true);
    let exit = own_line::exit(&result);
    assert_eq!(exit.code, 1);
    let line = one_line(&cli_module::stdout_text(&exit, true).expect("the line"));
    assert_eq!(names(&line), ["message", "details", "outcome"]);
    assert_eq!(line["outcome"]["code"], "lab_operation_check_failed");
    assert_eq!(line["outcome"]["category"], "error");
    assert_eq!(
        line["outcome"]["values"],
        json!({"req_id": REQUEST, "receipt_state": "completed", "client_code": client_code})
    );
    assert_eq!(line["message"], json!(error.message));
    assert_eq!(line["details"], details);
    assert_eq!(line["details"]["effect"], "performed");
    assert_eq!(line["details"]["executed"], true);

    // MCP's actinglab child (`--json --via mcp … do …`): the envelope, exit 5.
    let (text, code) = enveloped(&CliResult::err("do".to_owned(), error, true));
    assert_eq!(code, 5);
    let envelope: Value = serde_json::from_str(&text).expect("the envelope");
    assert_eq!(envelope["ok"], false);
    assert_eq!(envelope["error"]["code"], "lab_operation_check_failed");
    assert_eq!(envelope["error"]["details"], details);
}
