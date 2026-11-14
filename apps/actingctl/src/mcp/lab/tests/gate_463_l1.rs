// SPDX-License-Identifier: AGPL-3.0-only

// #463 L1 (contracts/lab-recording.md, "One call per step"; contracts/lab-operations.json, the rows
// of the recording family; register §1.370): whatever the Lab has is a command and a tool.
// - `l1_each_new_record_tool_reaches_its_command_and_answers_its_envelope`: `ac_record_candidates`,
//   `ac_record_amend`, `ac_record_step`, `ac_record_archive` and `ac_record_reopen` are author tools
//   whose input schema is their command's flags; each runs `actinglab --json --via mcp record
//   <action>` with its arguments as that command's flags, and its job answers the envelope's data
//   verbatim.
// - `l1_ac_lab_do_hands_the_one_call_form_to_actinglab`: `mark`, `next_frame_after_ms` and
//   `next_frame_out` are `--mark-request-json` (the request unchanged), `--next-frame-after-ms` and
//   `--next-frame-out`; MCP gives the one-call form no refusal of its own, `dry_run` included, which
//   reaches actinglab beside `--record`, where the command refuses it as `do --capture --record
//   --dry-run` (`record_flag_unsupported`, actinglab's gate_463_l1); with `operation` each of the
//   three is refused as conflicting before any child; a recorded answer comes back verbatim.
// - `l1_fix1_an_exported_ac_lab_do_answer_keeps_its_next_frame_inline` (fix round 1, item 3): a
//   recorded ac_lab_do answer over the output budget is exported whole, and its `next_frame` (the
//   next screen) stays inline verbatim beside `after_attachment`, the rows in the export only, the
//   answer within the budget.
// The actinglab child is never started: the tests read the command a tool would run, the answer a
// job ends with, and the refusals given before the install is located.

use super::super::super::protocol::Era;
use super::super::super::tools::{Tier, definition, find};
use super::super::{
    DO, LabCommand, Program, RECORD_AMEND, RECORD_ARCHIVE, RECORD_CANDIDATES, RECORD_REOPEN,
    RECORD_STEP, act, command_end, command_line, do_input, fits_as_job, input_schema, interpret,
};
use crate::child::Captured;
use crate::codes::{ActingctlCode, ArgumentProblem};
use crate::mcp::jobs::Jobs;
use crate::mcp::runtime::RuntimeAccess;
use crate::mcp::tests::gate_mcpt1::unreached_deadline;
use crate::mcp::tools::{ToolContext, ToolOutcome};
use serde_json::{Map, Value, json};
use std::path::PathBuf;
use std::process::ExitStatus;
use std::sync::atomic::AtomicBool;

const ALIAS: &str = "neutral.instance";

fn arguments(members: Value) -> Map<String, Value> {
    members.as_object().cloned().unwrap_or_default()
}

fn words(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

fn program() -> Program {
    Program {
        actinglab: PathBuf::from("neutral-install")
            .join("tools")
            .join("actinglab.exe"),
        root: PathBuf::from("neutral-install"),
        config: PathBuf::from("neutral-install").join("actingd.config.json"),
        installation: None,
        state_root: Ok(PathBuf::from("neutral-state")),
    }
}

#[cfg(windows)]
fn exited(code: u8) -> ExitStatus {
    use std::os::windows::process::ExitStatusExt;
    ExitStatus::from_raw(u32::from(code))
}

#[cfg(unix)]
fn exited(code: u8) -> ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    ExitStatus::from_raw(i32::from(code) << 8)
}

/// `ac_lab_do` with `arguments`, in a context whose install cannot be located.
fn call(arguments: &Map<String, Value>) -> ToolOutcome {
    let runtime = RuntimeAccess::new(None, None);
    let cancelled = AtomicBool::new(false);
    let jobs = Jobs::new();
    let context = ToolContext {
        runtime: &runtime,
        cancelled: &cancelled,
        deadline: unreached_deadline(),
        era: Era::Modern,
        session: 1,
        jobs: &jobs,
    };
    act(&context, arguments)
}

#[test]
fn l1_each_new_record_tool_reaches_its_command_and_answers_its_envelope() {
    let cases: [(&str, &LabCommand, Value, Vec<String>, bool); 5] = [
        (
            "ac_record_candidates",
            &RECORD_CANDIDATES,
            json!({"instance": ALIAS, "step_id": "neutral_step"}),
            words(&[
                "record",
                "candidates",
                "--instance",
                ALIAS,
                "--step-id",
                "neutral_step",
            ]),
            true,
        ),
        (
            "ac_record_amend",
            &RECORD_AMEND,
            json!({"instance": ALIAS, "step_id": "neutral_step", "threshold": 0.5,
                   "color_check": true}),
            words(&[
                "record",
                "amend",
                "--instance",
                ALIAS,
                "--step-id",
                "neutral_step",
                "--threshold",
                "0.5",
                "--color-check",
            ]),
            false,
        ),
        (
            "ac_record_step",
            &RECORD_STEP,
            json!({"instance": ALIAS, "kind": "anchor", "frame": "neutral-frame.png",
                   "capture": false}),
            words(&[
                "record",
                "step",
                "--instance",
                ALIAS,
                "--kind",
                "anchor",
                "--frame",
                "neutral-frame.png",
            ]),
            false,
        ),
        (
            "ac_record_archive",
            &RECORD_ARCHIVE,
            json!({"instance": ALIAS, "record_id": "record_a", "copy_to": "neutral-copy"}),
            words(&[
                "record",
                "archive",
                "--instance",
                ALIAS,
                "--record-id",
                "record_a",
                "--copy-to",
                "neutral-copy",
            ]),
            false,
        ),
        (
            "ac_record_reopen",
            &RECORD_REOPEN,
            json!({"instance": ALIAS, "record_id": "record_a"}),
            words(&[
                "record",
                "reopen",
                "--instance",
                ALIAS,
                "--record-id",
                "record_a",
            ]),
            false,
        ),
    ];
    for (tool, command, members, line, read_only) in cases {
        // A tool of the table, author tier, whose schema is its command's flags.
        let found = find(tool).unwrap_or_else(|| panic!("{tool} is in the tool table"));
        assert_eq!(found.tier, Tier::Author, "{tool}");
        let described = definition(found, Era::Modern);
        assert_eq!(described["inputSchema"], input_schema(command), "{tool}");
        assert_eq!(
            described["annotations"]["readOnlyHint"], read_only,
            "{tool}: {described}"
        );

        // The command it runs: actinglab with the command's words and its arguments as flags.
        let built = command_line(command, &arguments(members))
            .unwrap_or_else(|error| panic!("{tool} was refused: {}", error.to_value()));
        assert_eq!(built, line, "{tool}");
        let started = program()
            .command(&built)
            .get_args()
            .map(|word| word.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            started[..3],
            ["--json", "--via", "mcp"],
            "{tool}: {started:?}"
        );
        assert_eq!(started[3..], line[..], "{tool}: {started:?}");

        // Its answer: the envelope's data, verbatim.
        let data = json!({"req_id": format!("{tool}-req"), "status": "neutral_status"});
        let envelope = json!({"ok": true, "data": data.clone()});
        let answer = interpret(
            &Captured {
                status: exited(0),
                stdout: envelope.to_string().into_bytes(),
                stderr: Vec::new(),
            },
            tool,
        );
        assert_eq!(
            answer.req_id.as_deref(),
            Some(format!("{tool}-req").as_str()),
            "{tool}"
        );
        let (result, warnings) = command_end(
            command,
            "lab_job_gate_463",
            answer.result,
            answer.req_id.as_deref(),
            Vec::new(),
        );
        match result {
            Ok(result) => assert_eq!(result, data, "{tool}"),
            Err(error) => panic!("{tool} answered an error: {}", error.to_value()),
        }
        assert!(warnings.is_empty(), "{tool}: {warnings:?}");
    }
}

#[test]
fn l1_ac_lab_do_hands_the_one_call_form_to_actinglab() {
    // One rectangle given once: the template's crop and, by `click.from`, the click.
    let request = json!({
        "schema_version": "actingcommand.lab-record-mark.v1",
        "add": [{"id": "ui/start", "family": "template",
                 "region": {"x": 20, "y": 20, "width": 12, "height": 12}}],
        "click": {"from": "ui/start"}
    });
    let members = json!({
        "instance": ALIAS,
        "capture": true,
        "record": true,
        "package": "neutral-package",
        "mark": request.clone(),
        "next_frame_after_ms": 800,
        "next_frame_out": "neutral-next.png",
    });

    // The schema takes the three, and the line carries them as the command's flags.
    let schema = do_input();
    assert_eq!(schema["properties"]["mark"]["type"], "object", "{schema}");
    assert_eq!(
        schema["properties"]["next_frame_after_ms"]["type"], "integer",
        "{schema}"
    );
    assert_eq!(
        schema["properties"]["next_frame_out"]["type"], "string",
        "{schema}"
    );
    let text = request.to_string();
    let line = command_line(&DO, &arguments(members.clone()))
        .unwrap_or_else(|error| panic!("the one call was refused: {}", error.to_value()));
    assert_eq!(
        line,
        words(&[
            "do",
            "--instance",
            ALIAS,
            "--package",
            "neutral-package",
            "--mark-request-json",
            text.as_str(),
            "--next-frame-after-ms",
            "800",
            "--next-frame-out",
            "neutral-next.png",
            "--capture",
            "--record",
        ])
    );
    let passed: Value = serde_json::from_str(&line[6]).expect("the request is JSON");
    assert_eq!(passed, request);

    // MCP adds no refusal of its own: the call stops where the install is located, with dry_run
    // too, whose line carries --dry-run beside --record for the command to refuse.
    let mut dry = arguments(members.clone());
    dry.insert("dry_run".to_owned(), json!(true));
    for (case, members) in [
        ("the one call", arguments(members)),
        ("with dry_run", dry.clone()),
    ] {
        if let Err(error) = call(&members) {
            assert!(
                !error.outcome_ref().is(ActingctlCode::ArgumentsInvalid),
                "{case} was refused by MCP's checks: {}",
                error.to_value()
            );
        }
    }
    let dry_line = match command_line(&DO, &dry) {
        Ok(line) => line,
        Err(error) => panic!("the dry line was refused: {}", error.to_value()),
    };
    assert!(
        dry_line.iter().any(|word| word == "--dry-run")
            && dry_line.iter().any(|word| word == "--record"),
        "{dry_line:?}"
    );

    // With operation each of the three conflicts, before any child.
    for (argument, value) in [
        ("mark", request),
        ("next_frame_after_ms", json!(800)),
        ("next_frame_out", json!("neutral-next.png")),
    ] {
        let mut members = arguments(json!({
            "operation": "op_a",
            "capture": true,
            "zip": "neutral.zip",
        }));
        members.insert(argument.to_owned(), value);
        let Err(error) = call(&members) else {
            panic!("{argument} with operation was not refused");
        };
        let outcome = error.outcome_ref();
        assert!(
            outcome.is(ActingctlCode::ArgumentsInvalid),
            "{argument}: {}",
            error.to_value()
        );
        assert_eq!(outcome.value("argument"), Some(&json!(argument)));
        assert_eq!(
            outcome.value("argument_problem"),
            Some(&json!(ArgumentProblem::Conflicting.as_str()))
        );
    }

    // A recorded answer, its record.mark and its next frame, comes back verbatim.
    let data = json!({
        "req_id": "req-463",
        "executed": true,
        "record": {"status": "click_recorded", "step": 1, "step_closed": true,
                   "mark": {"status": "marks_recorded", "step": 1}},
        "next_frame": {"after_ms": 800, "out": "neutral-next.png",
                       "record": {"status": "frame_recorded", "step": 2, "step_opened": true}}
    });
    let (result, warnings) = command_end(
        &DO,
        "lab_job_gate_463_do",
        Ok(data.clone()),
        Some("req-463"),
        Vec::new(),
    );
    match result {
        Ok(result) => assert_eq!(result, data),
        Err(error) => panic!("the recorded answer became an error: {}", error.to_value()),
    }
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn l1_fix1_an_exported_ac_lab_do_answer_keeps_its_next_frame_inline() {
    // A recorded step's answer in one call, with rows far over the 11 KiB payload budget beside it.
    let rows = (0..800)
        .map(|index| format!("neutral-ledger-row-{index:04}"))
        .collect::<Vec<_>>();
    let next_frame = json!({
        "width": 96,
        "height": 64,
        "capture_backend_used": "neutral_backend",
        "after_ms": 800,
        "out": "neutral-next.png",
        "record": {"status": "frame_recorded", "step": 2, "step_opened": true},
    });
    let data = json!({
        "req_id": "req-463-fix1",
        "executed": true,
        "frame_coordinates": {
            "device_pixels": [96, 64],
            "orientation": "landscape",
            "applies_to": ["tap", "tap_rect", "swipe"],
        },
        "after_attachment": {"kind": "frame", "frame_path": "neutral-after.png"},
        "record": {"status": "click_recorded", "step": 1, "step_closed": true},
        "next_frame": next_frame.clone(),
        "ledger": {"events": rows},
    });
    let handle = "lab_job_gate_463_fix1";
    let (result, warnings) = command_end(
        &DO,
        handle,
        Ok(data.clone()),
        Some("req-463-fix1"),
        Vec::new(),
    );
    let answer = match result {
        Ok(answer) => answer,
        Err(error) => panic!("the exported answer became an error: {}", error.to_value()),
    };
    assert_eq!(answer["overflowed"], json!(true), "{answer}");
    assert!(answer["export"]["path"].is_string(), "no export: {answer}");
    assert_eq!(
        answer["next_frame"], next_frame,
        "the next screen inline: {answer}"
    );
    assert_eq!(
        answer["after_attachment"], data["after_attachment"],
        "{answer}"
    );
    assert!(
        answer.get("ledger").is_none(),
        "the rows stay in the export: {answer}"
    );
    assert!(
        fits_as_job(handle, &answer, &warnings),
        "the answer is over the budget"
    );
}
