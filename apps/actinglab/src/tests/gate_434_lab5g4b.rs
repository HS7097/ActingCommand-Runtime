// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-5G4b (register §1.287, "Slice LAB-5G4b"; contracts/lab-two-entries.md §8.2;
// coordinator work order rd-1113/brief-lab5g4b.md, point 3): `serve --stdio --front mcp` answers
// each line as the envelope `{exit_code, req_id?, line}`; without the option every line is as
// before.
// - `serve_front_mcp_answers_an_envelope`: the same requests through the loop without and with the
//   option. Without it the loop prints, byte for byte, what the command line prints with `--json`
//   for the same words (a CLI module row that answers, the same row refused, a row actinglab's own
//   command serves). With it each line is `{exit_code, line}`: `line` that same line, `exit_code`
//   the command line's exit for it (0 and non-zero), and no `req_id`, as none of those results
//   carries one. An own command's result that carries a request id gives it as `req_id` beside
//   its line and its exit: from its result on a success (exit 0), from its details on a failure
//   (exit 1); one without a request id gives none.
// Fakes only: an empty configuration in a temporary folder; no Runtime, no device, no clock, no
// sleep.

use super::*;
use crate::cli_module::{self, own_line, serve};
use actingcommand_lab_ops::cli_line::Exit;
use actingcommand_lab_ops::initiator::ENVIRONMENT_VARIABLE;

/// A made-up request id.
const REQUEST: &str = "neutral-request-0001";

fn words_of(line: &[&str]) -> Vec<String> {
    line.iter().map(|word| (*word).to_owned()).collect()
}

/// What the loop prints on stdout for `requests` under the front end `front`.
fn printed(front: Option<serve::Front>, requests: &[Value]) -> String {
    let table = cli_module::table().expect("the operation table");
    let input = requests
        .iter()
        .map(|request| format!("{request}\n"))
        .collect::<String>();
    let mut output = Vec::new();
    let mut developer = Vec::new();
    serve::serve_as(table, front, input.as_bytes(), &mut output, &mut developer)
        .expect("the loop ends");
    String::from_utf8(output).expect("UTF-8 lines")
}

/// The line of `exit`, and the text the command line prints for it with `--json`.
fn line_and_text(exit: &Exit) -> (Value, String) {
    let line = exit.line.clone().expect("the line encodes");
    let text = cli_module::stdout_text(exit, true).expect("the command line prints the line");
    (line, text)
}

#[test]
fn serve_front_mcp_answers_an_envelope() {
    let _guard = env_lock();
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    let temp = TempDir::new().unwrap();
    set_config_env(temp.path().join("config.json"));

    // Each request, and what the command line gives for the same words: a CLI module row that
    // answers, the same row refused (a code the catalog does not list), and a row actinglab's own
    // command serves.
    let requests = [
        json!({"op": "codes", "params": {"code": "actingctl_usage_invalid"}}),
        json!({"op": "codes", "params": {"code": "neutral_no_such_code"}}),
        json!({"op": "user-config", "params": {"key": "-x"}}),
    ];
    let (own, json) = own_line::answer(parse_invocation(
        ["--json", "user-config", "get", "-x"],
        false,
    ));
    assert!(json);
    let command_line = [
        cli_module::run_line(&words_of(&["codes", "actingctl_usage_invalid"])),
        cli_module::run_line(&words_of(&["codes", "neutral_no_such_code"])),
        own,
    ];
    assert_eq!(command_line[0].code, 0, "setup: a row that answers");
    assert_ne!(command_line[1].code, 0, "setup: a row refused");

    // Without the option: byte for byte the command line's text, one line each.
    let expected = command_line
        .iter()
        .map(|exit| format!("{}\n", line_and_text(exit).1))
        .collect::<String>();
    assert_eq!(printed(None, &requests), expected, "without --front mcp");

    // With it: the envelope of each line, its exit code beside it; no request id.
    let enveloped = printed(Some(serve::Front::Mcp), &requests);
    let lines = enveloped.lines().collect::<Vec<_>>();
    assert_eq!(
        lines.len(),
        requests.len(),
        "one line per request: {enveloped}"
    );
    for ((text, exit), request) in lines.iter().zip(&command_line).zip(&requests) {
        let envelope = serde_json::from_str::<Value>(text)
            .unwrap_or_else(|error| panic!("one JSON line ({error}): {text}"));
        let (line, _) = line_and_text(exit);
        assert_eq!(
            envelope,
            json!({"exit_code": exit.code, "line": line}),
            "{request}"
        );
        assert!(envelope.get("req_id").is_none(), "{request}: {envelope}");
    }

    // An own command's result that carries a request id: in its result on a success, in its
    // details on a failure; and one that carries none.
    let carried = CliResult::ok(
        "observe".to_owned(),
        json!({"req_id": REQUEST, "state": "observed"}),
        true,
        String::new(),
    );
    let failed = CliResult::err(
        "do".to_owned(),
        CliError::usage("neutral check failed").with_details(json!({"req_id": REQUEST})),
        true,
    );
    let without = CliResult::ok(
        "observe".to_owned(),
        json!({"state": "observed"}),
        true,
        String::new(),
    );
    for (result, code, request_id) in [
        (&carried, 0_u8, Some(REQUEST)),
        (&failed, 1, Some(REQUEST)),
        (&without, 0, None),
    ] {
        let exit = own_line::exit(result);
        assert_eq!(exit.code, code, "{}", result.envelope_json());
        let found = own_line::request_id(result);
        assert_eq!(found.as_deref(), request_id, "{}", result.envelope_json());
        let (line, _) = line_and_text(&exit);
        let mut expected = json!({"exit_code": code, "line": line.clone()});
        if let Some(request_id) = request_id {
            expected["req_id"] = json!(request_id);
        }
        assert_eq!(
            serve::enveloped(exit.code, found.as_deref(), line),
            expected,
            "{}",
            result.envelope_json()
        );
    }
    set_missing_config_env();
}
