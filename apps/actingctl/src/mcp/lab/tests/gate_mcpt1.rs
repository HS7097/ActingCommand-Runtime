// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #382 MCP-T1 (test plan A-5a, inventory key 30): the one overflow rule for every
//! Lab answer (#338 S3 review F2, items 1-3). An answer over the output budget goes whole to
//! `%TEMP%\actingcommand-mcp\materials\<sha256>.json`, with the warning `answer_exported`
//! naming that file. Workflow #378 A5: a tool error is `{class, outcome}` and is never exported;
//! the warnings found before it become its `note` links. Also A5: how `interpret` turns
//! actinglab's exit code and envelope into an answer or a tool error.

use super::super::{Overflow, finish, interpret};
use crate::child::Captured;
use crate::codes::ActingctlCode;
use crate::mcp::tools::{self, ToolError};
use actingcommand_contract::codes::{ContractCode, RuntimeErrorCode, SharedCode};
use actingcommand_contract::outcome::{Category, Outcome};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitStatus;

/// The ac_record_stop parts an exported answer keeps inline (F2 item 2).
const BINDING_PARTS: [&str; 5] = [
    "binding_example",
    "binding_requires",
    "prerequisite_entry_example",
    "catalog_on_failure_example",
    "package_ref",
];

/// Far over the 11 KiB payload budget; every item names itself.
fn oversized(label: &str) -> Value {
    json!(
        (0..800)
            .map(|index| format!("{label}-oversized-{index:04}"))
            .collect::<Vec<_>>()
    )
}

fn members(value: &Value) -> BTreeSet<&str> {
    value
        .as_object()
        .map(|members| members.keys().map(String::as_str).collect())
        .unwrap_or_default()
}

/// `(relation, code)` of each link of `outcome`, in order.
fn links(outcome: &Outcome) -> Vec<(String, String)> {
    outcome
        .causes()
        .iter()
        .map(|link| {
            (
                link.relation().as_str().to_owned(),
                link.outcome().code().as_str().to_owned(),
            )
        })
        .collect()
}

/// The content of the file an export `{path, sha256, size}` names and that file's
/// `sha256:<hex>`, after checking the export against it: the file is in
/// `%TEMP%\actingcommand-mcp\materials`, named `<sha256 hex>.json`; `size` is its byte length
/// and `sha256` names the SHA-256 of its bytes.
#[track_caller]
fn exported(export: &Value) -> (Value, String) {
    assert_eq!(
        members(export),
        BTreeSet::from(["path", "sha256", "size"]),
        "export: {export}"
    );
    let path = PathBuf::from(
        export["path"]
            .as_str()
            .unwrap_or_else(|| panic!("an export without a path: {export}")),
    );
    let bytes = fs::read(&path).unwrap_or_else(|error| {
        panic!("cannot read the exported file {}: {error}", path.display())
    });
    let digest = format!("{:x}", Sha256::digest(&bytes));
    let directory = env::temp_dir().join("actingcommand-mcp").join("materials");
    assert_eq!(
        path.parent(),
        Some(directory.as_path()),
        "the export is not in the export directory: {export}"
    );
    assert_eq!(
        path.file_name().and_then(|name| name.to_str()),
        Some(format!("{digest}.json").as_str()),
        "the export is not named by its SHA-256: {export}"
    );
    assert_eq!(export["size"], json!(bytes.len()), "export: {export}");
    let named = export["sha256"].as_str().unwrap_or_default();
    assert!(
        named == digest || named == format!("sha256:{digest}"),
        "the export's sha256 does not name the file's SHA-256 {digest}: {export}"
    );
    let content = serde_json::from_slice(&bytes)
        .unwrap_or_else(|error| panic!("the exported file is not JSON ({error}): {export}"));
    (content, format!("sha256:{digest}"))
}

#[test]
fn gate_a_lab_answer_over_the_budget_is_exported_whole_and_one_inside_comes_back_unchanged() {
    // (a) An answer over the budget: {req_id, export, overflowed: true}, the file the answer,
    // and the one warning answer_exported {path, actual_sha256, actual_bytes} naming that file.
    let data = json!({"req_id": "gate-req-a", "observation": {"rows": oversized("a")}});
    let overflow = Overflow {
        handle: "lab_job_gate_a",
        keeps_binding: false,
    };
    let (result, warnings) = finish(&overflow, Ok(data.clone()), Some("gate-req-a"), Vec::new());
    let answer =
        result.unwrap_or_else(|error| panic!("(a) answered an error: {}", error.to_value()));
    assert_eq!(
        members(&answer),
        BTreeSet::from(["export", "overflowed", "req_id"]),
        "(a) {answer}"
    );
    assert_eq!(answer["req_id"], json!("gate-req-a"), "(a) {answer}");
    assert_eq!(answer["overflowed"], json!(true), "(a) {answer}");
    let (content, sha256) = exported(&answer["export"]);
    assert_eq!(content, data, "(a) the exported file is not the answer");
    assert_eq!(warnings.len(), 1, "(a) warnings: {warnings:?}");
    assert_eq!(
        members(&warnings[0]),
        BTreeSet::from(["outcome"]),
        "(a) {}",
        warnings[0]
    );
    let warning = &warnings[0]["outcome"];
    assert_eq!(
        warning["code"],
        json!(ActingctlCode::AnswerExported.as_str()),
        "(a) {warning}"
    );
    assert_eq!(warning["category"], json!("warning"), "(a) {warning}");
    assert_eq!(
        warning["values"],
        json!({
            "path": answer["export"]["path"],
            "actual_sha256": sha256,
            "actual_bytes": answer["export"]["size"],
        }),
        "(a) {warning}"
    );

    // (b) An ac_record_stop answer over the budget keeps its binding parts inline, verbatim.
    let parts = json!({
        "binding_example": {"instance": "gate-instance", "task": "gate-task"},
        "binding_requires": ["gate-requirement"],
        "prerequisite_entry_example": {"entry": "gate-entry"},
        "catalog_on_failure_example": {"on_failure": "gate-fallback"},
        "package_ref": "gate-package-ref",
    });
    let mut lab = parts.as_object().cloned().unwrap_or_default();
    lab.insert("steps".to_owned(), oversized("b"));
    let data = json!({"req_id": "gate-req-b", "lab": lab});
    let overflow = Overflow {
        handle: "lab_job_gate_b",
        keeps_binding: true,
    };
    let (result, _) = finish(&overflow, Ok(data.clone()), Some("gate-req-b"), Vec::new());
    let answer =
        result.unwrap_or_else(|error| panic!("(b) answered an error: {}", error.to_value()));
    assert_eq!(
        members(&answer["lab"]),
        BINDING_PARTS.into_iter().collect::<BTreeSet<_>>(),
        "(b) {answer}"
    );
    for part in BINDING_PARTS {
        assert_eq!(
            answer["lab"][part], parts[part],
            "(b) part {part}: {answer}"
        );
    }
    assert_eq!(answer["overflowed"], json!(true), "(b) {answer}");
    assert_eq!(
        exported(&answer["export"]).0,
        data,
        "(b) the exported file is not the answer"
    );

    // (c) An error, as a Lab failure arrives (actinglab's exit 4, its code as the cause), with a
    // warning found before it: the error comes back as it is, {class, outcome} and nothing else,
    // the warning as its note link, and the job keeps no warning of its own.
    let error = ToolError::outcome(
        "device",
        Outcome::new(ActingctlCode::LabToolFailed)
            .exit_code(4)
            .req_id("gate-req-c")
            .caused_by(Outcome::relayed(RuntimeErrorCode::InstanceUnknown.as_str())),
    );
    let note = tools::warning(&Outcome::new(ActingctlCode::ClientActionWithoutReqId));
    let overflow = Overflow {
        handle: "lab_job_gate_c",
        keeps_binding: false,
    };
    let (result, warnings) = finish(&overflow, Err(error), Some("gate-req-c"), vec![note]);
    let error = match result {
        Ok(answer) => panic!("(c) an error answered {answer}"),
        Err(error) => error,
    };
    assert_eq!(
        members(&error.to_value()),
        BTreeSet::from(["class", "outcome"]),
        "(c) {}",
        error.to_value()
    );
    assert_eq!(error.class(), "device", "(c) {}", error.to_value());
    let outcome = error.outcome_ref();
    assert_eq!(
        outcome.code().as_str(),
        ActingctlCode::LabToolFailed.as_str(),
        "(c) {outcome:?}"
    );
    assert_eq!(
        Some(outcome.values()),
        json!({"exit_code": 4, "req_id": "gate-req-c"}).as_object(),
        "(c) {outcome:?}"
    );
    assert_eq!(
        links(outcome),
        vec![
            (
                "caused_by".to_owned(),
                RuntimeErrorCode::InstanceUnknown.as_str().to_owned()
            ),
            (
                "note".to_owned(),
                ActingctlCode::ClientActionWithoutReqId.as_str().to_owned()
            ),
        ],
        "(c) {outcome:?}"
    );
    assert!(warnings.is_empty(), "(c) job warnings: {warnings:?}");

    // (d) An answer inside the budget comes back unchanged, with no export.
    let data = json!({"req_id": "gate-req-d", "status": "inside"});
    let overflow = Overflow {
        handle: "lab_job_gate_d",
        keeps_binding: false,
    };
    let (result, _) = finish(&overflow, Ok(data.clone()), Some("gate-req-d"), Vec::new());
    let answer =
        result.unwrap_or_else(|error| panic!("(d) answered an error: {}", error.to_value()));
    assert_eq!(answer, data, "(d) an answer inside the budget changed");
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

/// What an actinglab child that exited with `code` printed.
fn captured(code: u8, stdout: &str, stderr: &str) -> Captured {
    Captured {
        status: exited(code),
        stdout: stdout.as_bytes().to_vec(),
        stderr: stderr.as_bytes().to_vec(),
    }
}

#[test]
fn gate_interpret_answers_each_actinglab_exit_by_its_class_with_actinglab_s_code_as_the_cause() {
    // Exit 0 with ok: the envelope's data, unchanged, and its req_id.
    let data = json!({"req_id": "gate-req-0", "observation": {"rows": []}});
    let envelope = json!({"ok": true, "data": data.clone()});
    let answer = interpret(&captured(0, &envelope.to_string(), ""), "gate");
    assert!(answer.returned, "exit 0");
    assert_eq!(answer.req_id.as_deref(), Some("gate-req-0"), "exit 0");
    match answer.result {
        Ok(result) => assert_eq!(result, data, "exit 0"),
        Err(error) => panic!("exit 0 answered an error: {}", error.to_value()),
    }

    // Exits 2..6 with an error envelope: lab_tool_failed {exit_code, req_id} in the class the
    // exit code names, caused by actinglab's code (not_implemented for exit 6). actinglab's
    // message and its other details are not carried.
    let rows = [
        (
            2,
            "observe_identity_limit_exceeded",
            "usage",
            "observe_identity_limit_exceeded",
        ),
        (
            3,
            RuntimeErrorCode::LeaseBusy.as_str(),
            "safety",
            RuntimeErrorCode::LeaseBusy.as_str(),
        ),
        (
            4,
            RuntimeErrorCode::InstanceUnknown.as_str(),
            "device",
            RuntimeErrorCode::InstanceUnknown.as_str(),
        ),
        (
            5,
            RuntimeErrorCode::RuntimeUnavailable.as_str(),
            "runtime",
            RuntimeErrorCode::RuntimeUnavailable.as_str(),
        ),
        (
            6,
            "lab_command_not_implemented",
            "usage",
            SharedCode::NotImplemented.as_str(),
        ),
    ];
    for (code, spelling, class, cause) in rows {
        let req_id = format!("gate-req-{code}");
        let envelope = json!({
            "ok": false,
            "error": {
                "code": spelling,
                "message": "gate lab message",
                "details": {"req_id": req_id, "step": 3},
            },
        });
        let answer = interpret(
            &captured(code, &envelope.to_string(), "gate stderr"),
            "gate",
        );
        assert!(answer.returned, "exit {code}");
        assert_eq!(
            answer.req_id.as_deref(),
            Some(req_id.as_str()),
            "exit {code}"
        );
        let error = match answer.result {
            Ok(result) => panic!("exit {code} answered {result}"),
            Err(error) => error,
        };
        assert_eq!(error.class(), class, "exit {code}: {}", error.to_value());
        let outcome = error.outcome_ref();
        assert_eq!(
            outcome.code().as_str(),
            ActingctlCode::LabToolFailed.as_str(),
            "exit {code}: {outcome:?}"
        );
        assert_eq!(outcome.category(), Category::Error, "exit {code}");
        assert_eq!(
            Some(outcome.values()),
            json!({"exit_code": code, "req_id": req_id}).as_object(),
            "exit {code}: {outcome:?}"
        );
        assert_eq!(
            links(outcome),
            vec![("caused_by".to_owned(), cause.to_owned())],
            "exit {code}: {outcome:?}"
        );
        assert!(
            !error.to_value().to_string().contains("gate lab message"),
            "exit {code} carries actinglab's message: {}",
            error.to_value()
        );
    }

    // No envelope: runtime lab_process_failed {exit_code}, caused by foreign_other_error with
    // actinglab's stderr as raw_text.
    let answer = interpret(
        &captured(1, "gate output that is not JSON", "gate stderr tail"),
        "gate",
    );
    assert!(answer.returned, "no envelope");
    let error = match answer.result {
        Ok(result) => panic!("no envelope answered {result}"),
        Err(error) => error,
    };
    assert_eq!(
        error.class(),
        "runtime",
        "no envelope: {}",
        error.to_value()
    );
    let outcome = error.outcome_ref();
    assert_eq!(
        outcome.code().as_str(),
        ActingctlCode::LabProcessFailed.as_str(),
        "no envelope: {outcome:?}"
    );
    assert_eq!(
        Some(outcome.values()),
        json!({"exit_code": 1}).as_object(),
        "no envelope: {outcome:?}"
    );
    assert_eq!(
        links(outcome),
        vec![(
            "caused_by".to_owned(),
            ContractCode::ForeignOtherError.as_str().to_owned()
        )],
        "no envelope: {outcome:?}"
    );
    let cause = outcome.causes()[0].outcome();
    assert_eq!(
        Some(cause.values()),
        json!({"raw_source": "process", "raw_text": "gate stderr tail", "exit_code": 1})
            .as_object(),
        "no envelope: {cause:?}"
    );
}
