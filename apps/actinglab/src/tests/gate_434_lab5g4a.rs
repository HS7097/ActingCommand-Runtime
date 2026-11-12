// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-5G4a (register §1.287, "Slice LAB-5G4a"; contracts/lab-two-entries.md §4.2,
// §4.3 and §8.2; coordinator work order rd-1108/arch-013/brief-lab5g4a.md): `serve --stdio`
// takes `--front mcp`, and refuses a value that begins with `--`.
// - `serve_front_mcp_records_lab_mcp`: `serve --stdio --front mcp` is taken, in either order;
//   another value, the option twice, the option without `--stdio` and the option with
//   `ACTINGCOMMAND_INITIATOR` set (to anything) are refused, each with its code and its argument
//   or variable; without the option the words are checked as before. A recording step added
//   through the loop under the option carries `Lab-MCP` and is answered with the loop's line,
//   never the envelope of a `--via` line (Workflow #434 LAB-5G4b, adapted: that line inside the
//   envelope `{exit_code, line}`, exit 0); without it, `Lab-CLI` and the two words the variable
//   may name. The
//   connection configuration under the option equals MCP's `--via mcp` child's (the Lab channel,
//   `via` `mcp`); without it there is no `via`.
// - `serve_refuses_values_that_begin_with_two_dashes`: a string value beginning with `--`, alone
//   and inside a list, is refused (`actingctl_usage_invalid`, the parameter as `argument`,
//   `not_recognised`) on a row actinglab's own command serves (`resource validate`) and on a row
//   the CLI module serves (`status`), and the next line is still answered; a string beginning with
//   one dash (an own row and a module row) and a negative number are taken as before: the loop
//   answers what the command line answers for the same words.
// Fakes only: an empty configuration and a recording folder in a temporary folder; no Runtime, no
// device, no clock, no sleep.

use super::*;
use crate::cli_module::{self, own_line, serve};
use actingcommand_lab_ops::cli_line::Exit;
use actingcommand_lab_ops::initiator::ENVIRONMENT_VARIABLE;
use actingcommand_runtime_client::{ChannelVia, RuntimeChannel};
use std::ffi::OsStr;

/// A made-up instance alias.
const INSTANCE: &str = "neutral.instance";

fn words_of(line: &[&str]) -> Vec<String> {
    line.iter().map(|word| (*word).to_owned()).collect()
}

/// The lines the loop answers for `requests` under the front end `front`, one per request.
fn served(front: Option<serve::Front>, requests: &[Value]) -> Vec<Value> {
    let table = cli_module::table().expect("the operation table");
    let input = requests
        .iter()
        .map(|request| format!("{request}\n"))
        .collect::<String>();
    let mut output = Vec::new();
    let mut developer = Vec::new();
    serve::serve_as(table, front, input.as_bytes(), &mut output, &mut developer)
        .expect("the loop ends");
    let output = String::from_utf8(output).expect("UTF-8 lines");
    let lines = output
        .lines()
        .map(|line| {
            serde_json::from_str::<Value>(line)
                .unwrap_or_else(|error| panic!("one JSON line ({error}): {line}"))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        lines.len(),
        requests.len(),
        "one line per request: {output}"
    );
    lines
}

/// The refusal `serve` prints for the words after it, the variable set to `variable`: the values
/// of its `actingctl_usage_invalid`, exit 1.
fn front_refused(words: &[&str], variable: Option<&str>) -> Value {
    let exit = match serve::front(&words_of(words), variable.map(OsStr::new)) {
        Ok(front) => panic!("{words:?} must be refused, not read as {front:?}"),
        Err(exit) => exit,
    };
    assert_eq!(exit.code, 1, "{words:?}");
    let line = exit.line.expect("the refusal's line");
    assert_eq!(line["outcome"]["code"], "actingctl_usage_invalid", "{line}");
    line["outcome"]["values"].clone()
}

/// The line of `exit`.
fn line_of(exit: &Exit) -> Value {
    exit.line.clone().expect("a line")
}

#[test]
fn serve_front_mcp_records_lab_mcp() {
    let _guard = env_lock();
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    let mcp = Some(serve::Front::Mcp);

    // The option: `--front mcp` once beside `--stdio`, in either order.
    for words in [
        &["--stdio", "--front", "mcp"][..],
        &["--front", "mcp", "--stdio"][..],
    ] {
        assert_eq!(
            serve::front(&words_of(words), None).ok(),
            Some(mcp),
            "{words:?}"
        );
    }
    // Another value, the option twice, the option without `--stdio`: refused as an unknown word.
    for (words, argument) in [
        (&["--stdio", "--front", "ui"][..], "ui"),
        (
            &["--stdio", "--front", "mcp", "--front", "mcp"][..],
            "--front",
        ),
        (&["--front", "mcp"][..], "--front"),
    ] {
        assert_eq!(
            front_refused(words, None),
            json!({"argument": argument, "argument_problem": "unknown"}),
            "{words:?}"
        );
    }
    // The option with the variable set, to anything: refused before the loop, naming the variable.
    for value in ["Lab-制包", "Lab-救急", "Lab-MCP", ""] {
        assert_eq!(
            front_refused(&["--stdio", "--front", "mcp"], Some(value)),
            json!({"variable": ENVIRONMENT_VARIABLE}),
            "{value:?}"
        );
    }
    // Without the option, as before: only `--stdio`, and the variable is left to the initiator's
    // own check (§4.2).
    assert_eq!(serve::front(&words_of(&["--stdio"]), None).ok(), Some(None));
    assert_eq!(
        serve::front(&words_of(&["--stdio"]), Some(OsStr::new("Lab-制包"))).ok(),
        Some(None)
    );
    assert_eq!(
        front_refused(&[], None),
        json!({"argument": "--stdio", "argument_problem": "required"})
    );
    assert_eq!(
        front_refused(&["--stdio", "--frobnicate"], None),
        json!({"argument": "--frobnicate", "argument_problem": "unknown"})
    );

    // The connection: under the option, the configuration of MCP's `--via mcp` child (the Lab
    // channel and `via` `mcp`; `lab_client_config_via` gives each the envelope (Lab, Lab));
    // without it, the Lab channel and no `via`, as on main.
    let temp = TempDir::new().unwrap();
    let Ok(child) = parse_invocation(["--json", "--via", "mcp", "record", "status"], true) else {
        panic!("the line of MCP's actinglab child parses");
    };
    assert_eq!(child.global.via, Some(ChannelVia::Mcp));
    let binding = |front: Option<serve::Front>| {
        lab_client_config_via(temp.path(), front.map(serve::Front::via)).channel()
    };
    assert_eq!(
        binding(mcp),
        lab_client_config_via(temp.path(), child.global.via).channel()
    );
    assert_eq!(
        (binding(mcp).channel, binding(mcp).via),
        (RuntimeChannel::Lab, Some(ChannelVia::Mcp))
    );
    assert_eq!(
        (binding(None).channel, binding(None).via),
        (RuntimeChannel::Lab, None)
    );

    // The recording: a step added through the loop carries the process's word.
    set_config_env(temp.path().join("config.json"));
    let frame = temp.path().join("frame.png");
    fs::write(&frame, test_record_frame_png(12, 10)).unwrap();
    let frame = frame.to_str().expect("a UTF-8 path").to_owned();
    let state = temp.path().join("session");
    let state = state.to_str().expect("a UTF-8 path").to_owned();
    let step = |step_id: &str| {
        json!({"op": "record step", "params": {
            "instance": INSTANCE,
            "kind": "anchor",
            "step_id": step_id,
            "id": "page/home",
            "region": "2,3,4,5",
            "frame": frame,
            "state_dir": state,
        }})
    };
    let started = served(
        None,
        &[json!({"op": "record start", "params": {
            "instance": INSTANCE,
            "task_id": "neutral-task",
            "state_dir": state,
        }})],
    );
    assert_eq!(
        started[0]["outcome"]["code"], "lab_operation_completed",
        "{}",
        started[0]
    );
    let mut lines = served(None, &[step("cli-anchor")]);
    // Workflow #434 LAB-5G4b (adapted): under the option the loop's line is the envelope's `line`,
    // the command line's exit code beside it.
    let enveloped = served(mcp, &[step("mcp-anchor")]);
    assert_eq!(enveloped[0]["exit_code"], json!(0), "{}", enveloped[0]);
    lines.push(enveloped[0]["line"].clone());
    for (word, step_id) in [
        ("Lab-制包", "workbench-anchor"),
        ("Lab-救急", "rescue-anchor"),
    ] {
        unsafe {
            env::set_var(ENVIRONMENT_VARIABLE, word);
        }
        lines.extend(served(None, &[step(step_id)]));
    }
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    let expected = [
        ("cli-anchor", "Lab-CLI"),
        ("mcp-anchor", "Lab-MCP"),
        ("workbench-anchor", "Lab-制包"),
        ("rescue-anchor", "Lab-救急"),
    ];
    for (line, (step_id, word)) in lines.iter().zip(expected) {
        // The loop's line (the result's members beside `outcome`), never the envelope.
        assert_eq!(line["outcome"]["code"], "lab_operation_completed", "{line}");
        assert!(
            line.get("ok").is_none() && line.get("data").is_none(),
            "{line}"
        );
        assert_eq!(
            (
                line["step"]["step_id"].clone(),
                line["step"]["initiator"].clone()
            ),
            (json!(step_id), json!(word)),
            "{line}"
        );
    }
    let recorded = lines.last().expect("four lines")["record"]["steps"]
        .as_array()
        .expect("the recording's steps")
        .iter()
        .map(|step| (step["step_id"].clone(), step["initiator"].clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        recorded,
        expected
            .iter()
            .map(|(step_id, word)| (json!(step_id), json!(word)))
            .collect::<Vec<_>>()
    );
    set_missing_config_env();
}

#[test]
fn serve_refuses_values_that_begin_with_two_dashes() {
    let _guard = env_lock();
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    let temp = TempDir::new().unwrap();
    set_config_env(temp.path().join("config.json"));
    let repo = temp.path().join("repo");
    let repo = repo.to_str().expect("a UTF-8 path").to_owned();
    let codes = json!({"op": "codes", "params": {"code": "actingctl_usage_invalid"}});
    let requests = [
        // A row actinglab's own command serves: a value alone, then one in a list.
        json!({"op": "resource validate", "params": {"repo": "--changed-path"}}),
        codes.clone(),
        json!({"op": "resource validate", "params": {
            "repo": repo,
            "changed_path": ["declarations.json", "--repo"],
        }}),
        codes.clone(),
        // A row the CLI module serves: a value alone, then one in a list.
        json!({"op": "status", "params": {"instance": "--sections"}}),
        codes.clone(),
        json!({"op": "status", "params": {"sections": ["daemon", "--instance"]}}),
        codes.clone(),
        // One dash, on a module row and an own row, and a negative number: taken as before.
        json!({"op": "codes", "params": {"prefix": "-x"}}),
        json!({"op": "user-config", "params": {"key": "-x"}}),
        json!({"op": "codes", "params": {"limit": -1}}),
    ];
    let lines = served(None, &requests);
    let answered = line_of(&cli_module::run_line(&words_of(&[
        "codes",
        "actingctl_usage_invalid",
    ])));
    for (index, argument) in [
        (0, "repo"),
        (2, "changed_path"),
        (4, "instance"),
        (6, "sections"),
    ] {
        assert_eq!(
            lines[index]["outcome"]["code"], "actingctl_usage_invalid",
            "{}",
            lines[index]
        );
        assert_eq!(
            lines[index]["outcome"]["values"],
            json!({"argument": argument, "argument_problem": "not_recognised"}),
            "{}",
            lines[index]
        );
        // The loop goes on: the next line is answered.
        assert_eq!(lines[index + 1], answered, "after {}", requests[index]);
    }
    // One dash and a negative number reach the row's command as the command line's words do.
    assert_eq!(
        lines[8],
        line_of(&cli_module::run_line(&words_of(&[
            "codes", "--prefix", "-x"
        ])))
    );
    let (own, json) = own_line::answer(parse_invocation(
        ["--json", "user-config", "get", "-x"],
        false,
    ));
    assert!(json);
    assert_eq!(lines[9], line_of(&own));
    assert_eq!(
        lines[10],
        line_of(&cli_module::run_line(&words_of(&[
            "codes", "--limit", "-1"
        ])))
    );
    set_missing_config_env();
}
