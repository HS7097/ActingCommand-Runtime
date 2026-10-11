// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #382 MCP-T1 (test plan A-5a, inventory key 30): the one overflow rule for every
//! Lab answer (#338 S3 review F2, items 1-3). An answer over the output budget goes whole to
//! `%TEMP%\actingcommand-mcp\materials\<sha256>.json`; an error that large keeps its class,
//! code and message with `lab_error` trimmed. Tool-error members are read only through
//! `error_parts`.

use super::super::{Effect, Overflow, finish};
use crate::codes::ActingctlCode;
use crate::mcp::tools::ToolError;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::PathBuf;

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

/// Class, code and message of a tool error.
#[track_caller]
fn error_parts(error: &Value) -> (&str, &str, &str) {
    (
        error_text(error, "class"),
        error_text(error, "code"),
        error_text(error, "message"),
    )
}

#[track_caller]
fn error_text<'a>(error: &'a Value, member: &str) -> &'a str {
    error[member]
        .as_str()
        .unwrap_or_else(|| panic!("a tool error without {member}: {error}"))
}

/// The content of the file an export `{path, sha256, size}` names, after checking the export
/// against it: the file is in `%TEMP%\actingcommand-mcp\materials`, named `<sha256 hex>.json`;
/// `size` is its byte length and `sha256` names the SHA-256 of its bytes.
#[track_caller]
fn exported(export: &Value) -> Value {
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
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|error| panic!("the exported file is not JSON ({error}): {export}"))
}

#[test]
fn gate_a_lab_answer_over_the_budget_is_exported_whole_and_one_inside_comes_back_unchanged() {
    // (a) An answer over the budget: {req_id, export, overflowed: true}, the file the answer.
    let data = json!({"req_id": "gate-req-a", "observation": {"rows": oversized("a")}});
    let overflow = Overflow {
        handle: "lab_job_gate_a",
        what: "record status",
        effect: Effect::Unchanged,
        keeps_binding: false,
    };
    let (result, _) = finish(&overflow, Ok(data.clone()), Some("gate-req-a"), Vec::new());
    let answer =
        result.unwrap_or_else(|error| panic!("(a) answered an error: {}", error.to_value()));
    assert_eq!(
        members(&answer),
        BTreeSet::from(["export", "overflowed", "req_id"]),
        "(a) {answer}"
    );
    assert_eq!(answer["req_id"], json!("gate-req-a"), "(a) {answer}");
    assert_eq!(answer["overflowed"], json!(true), "(a) {answer}");
    assert_eq!(
        exported(&answer["export"]),
        data,
        "(a) the exported file is not the answer"
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
        what: "record stop",
        effect: Effect::Changed,
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
        exported(&answer["export"]),
        data,
        "(b) the exported file is not the answer"
    );

    // (c) An error over the budget, as a Lab failure arrives: the class of exit code 5, code
    // and message verbatim from the envelope's error, which is lab_error.
    let scalars = json!({"req_id": "gate-req-c", "step": 3, "retryable": false, "note": null});
    let mut details = scalars.as_object().cloned().unwrap_or_default();
    details.insert("frames".to_owned(), oversized("c"));
    details.insert("context".to_owned(), json!({"nested": "gate-context"}));
    let lab_error = json!({
        "code": "lab_process_failed",
        "message": "gate lab failure",
        "details": Value::Object(details),
    });
    let error = ToolError::new(
        "runtime",
        ActingctlCode::LabProcessFailed,
        "gate lab failure",
    )
    .with_detail("lab_exit_code", json!(5))
    .with_detail("lab_error", lab_error.clone());
    let overflow = Overflow {
        handle: "lab_job_gate_c",
        what: "do",
        effect: Effect::Changed,
        keeps_binding: false,
    };
    let (result, _) = finish(&overflow, Err(error), Some("gate-req-c"), Vec::new());
    let error = match result {
        Ok(answer) => panic!("(c) an error over the budget answered {answer}"),
        Err(error) => error.to_value(),
    };
    assert_eq!(
        error_parts(&error),
        ("runtime", "lab_process_failed", "gate lab failure"),
        "(c) {error}"
    );
    let details = &error["details"];
    assert_eq!(
        details["lab_error"],
        json!({"code": "lab_process_failed", "message": "gate lab failure", "details": scalars}),
        "(c) {error}"
    );
    assert_eq!(details["lab_error_trimmed"], json!(true), "(c) {error}");
    assert_eq!(details["req_id"], json!("gate-req-c"), "(c) {error}");
    assert_eq!(
        exported(&details["export"]),
        lab_error,
        "(c) the exported file is not the untrimmed lab_error"
    );

    // (d) An answer inside the budget comes back unchanged, with no export.
    let data = json!({"req_id": "gate-req-d", "status": "inside"});
    let overflow = Overflow {
        handle: "lab_job_gate_d",
        what: "record status",
        effect: Effect::Unchanged,
        keeps_binding: false,
    };
    let (result, _) = finish(&overflow, Ok(data.clone()), Some("gate-req-d"), Vec::new());
    let answer =
        result.unwrap_or_else(|error| panic!("(d) answered an error: {}", error.to_value()));
    assert_eq!(answer, data, "(d) an answer inside the budget changed");
}
