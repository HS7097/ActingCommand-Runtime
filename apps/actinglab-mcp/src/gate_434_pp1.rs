// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #434 PP-1 (register §1.465; the coordinator's work order of 11-17, after `ac_place`
//! with `plan` answered a plan of 52 034 bytes `output_budget_exceeded` on production 0.12.70, with
//! no file written and `ac_run_show` naming its members only): a rescue row's success line over the
//! budget, scripted as the rescue module builds it (`rescue::place` and `composite_exit`,
//! `install::undo`'s plan form, `install::status`), answered as `calls::Service::rescue` answers the
//! module's line (`rescue_export::answered` within this server's budget). The server's own code, no
//! Runtime, no clock, no sleep; exports go to a temporary folder.
//!
//! - `pp1_a_plan_over_the_budget_is_exported_whole_beside_what_must_be_read` (test 1): a `place
//!   --plan` line that fits neither way is written whole, the file's bytes the line's; the warning
//!   `answer_exported` is first and names the file, `export` is its `{path, sha256, size}`, and the
//!   header holds the plan's `schedule_changes` whole under `plan` and the line's outcome, within
//!   the limit with its text. What the other rows' headers hold: `deploy --plan` the outcome alone,
//!   `op status` its `unfinished`, `last_operations` and `journal`.
//! - `pp1_a_larger_plan_keeps_the_four_lists_then_the_counts` (test 2): `packages` too large to fit:
//!   the four lists stay and `packages` is counted, `truncated` naming it; `enters` too large too:
//!   every list counted, `truncated` naming each; `undo --plan` by the same rule, its `reverses`
//!   whole.
//! - `pp1_run_show_of_the_job_answers_the_header_and_the_export` (test 3): the job made as every
//!   served call's is (`jobs::Jobs::start`) keeps the header, and `ac_run_show` of its handle
//!   (`server::job_answer`, read) answers it with its export, where the module's line as it came
//!   answered `output_budget_exceeded` and the names of its members.
//! - `pp1_an_answer_within_the_budget_and_a_refusal_are_as_before` (test 4): a line repeated as text,
//!   a line sent in `structuredContent` alone and a refusal over the budget are answered byte for
//!   byte as before, and nothing is written.
//! - `pp1_an_export_directory_that_cannot_be_written` (test 5): `material_export_failed` first, no
//!   `export`, the header's parts and outcome, nothing written; a header that does not fit even so
//!   answers `output_budget_exceeded` with its job's handle, and `ac_run_show` its summary.

use crate::budget;
use crate::calls::{self, Service};
use crate::jobs;
use crate::protocol::{self, Era, LegacyVersion};
use crate::rescue_export;
use crate::server;
use actingcommand_contract::outcome::Outcome;
use actingcommand_lab_ops::cli_line::{self, Exit};
use actingcommand_lab_ops::codes::{ActingctlCode, InstallLocation, InstallRefusal};
use serde_json::{Map, Value, json};
use std::fs;
use std::path::Path;

/// The era these calls are answered in: 2025-11-25, with `structuredContent`.
const ERA: Era = Era::Legacy(LegacyVersion::V20251125);
/// A plan id as the rescue module spells one; made up.
const PLAN_ID: &str = "plan-0123456789abcdef";
/// How many warnings of its own the line of test 5 carries, so that its header cannot fit.
const NOISY: usize = 300;

fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(members) => members,
        other => panic!("not a JSON object: {other}"),
    }
}

/// The names of `value`'s members, in order.
fn keys(value: &Value) -> Vec<&str> {
    value
        .as_object()
        .map(|members| members.keys().map(String::as_str).collect())
        .unwrap_or_default()
}

/// The line a `tools/call` result carries as its text, which its `structuredContent` repeats.
fn text_line(result: &Value) -> Value {
    let text = result["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("a result without its text: {result}"));
    let line = serde_json::from_str::<Value>(text)
        .unwrap_or_else(|error| panic!("the text is no JSON ({error}): {result}"));
    assert_eq!(
        result.get("structuredContent"),
        Some(&line),
        "structuredContent repeats the line"
    );
    line
}

/// The line of an exit, which encodes.
fn line_of(exit: &Exit) -> Value {
    exit.line.clone().expect("the line encodes")
}

/// A made-up package id.
fn package(index: usize) -> String {
    format!("neutral.pack.{index:04}")
}

/// `install_command_done` of the command `operation`, the installation not changed, as the rescue
/// module's plan forms end.
fn done(operation: InstallLocation) -> Outcome {
    Outcome::new(ActingctlCode::InstallCommandDone)
        .operation(operation)
        .installation_changed(false)
}

/// A plan's `schedule_changes` in the members and order the rescue module answers it
/// (`place::ScheduleChanges::value`): `rows` package rows, `entering` package ids in `enters`, one
/// in `leaves`, none in `content_only`, and one side that could not be read.
fn schedule_changes(rows: usize, entering: usize) -> Value {
    let packages = (0..rows)
        .map(|index| {
            json!({
                "package_id": package(index),
                "before": {"state": "unclassified"},
                "after": {"state": "scheduled", "roles": ["startup"]},
                "content": "replace",
            })
        })
        .collect::<Vec<_>>();
    json!({
        "packages": packages,
        "enters": (0..entering).map(package).collect::<Vec<_>>(),
        "leaves": [package(rows)],
        "content_only": [],
        "unread": [{
            "root": "installed",
            "failure": {"outcome": {"code": "standard_pack_root_invalid", "category": "error"}},
        }],
    })
}

/// A placement's plan as the rescue module lists it (`place::Placement::value`): `items` pack items
/// and `changes` as its `schedule_changes`.
fn placement_plan(items: usize, changes: &Value) -> Value {
    let items = (0..items)
        .map(|index| {
            json!({
                "item": package(index),
                "kind": "pack",
                "state": "replace",
                "dst": format!("packs/{}", package(index)),
            })
        })
        .collect::<Vec<_>>();
    json!({
        "plan_id": PLAN_ID,
        "game": "neutral",
        "game_root": "C:\\ActingCommand\\packs\\neutral",
        "zip": "C:\\ActingCommand\\neutral-bundle.zip",
        "zip_sha256": format!("sha256:{}", "0".repeat(64)),
        "items": items,
        "schedule_changes": changes,
    })
}

/// `place --plan`'s line as the rescue module answers it (`rescue::place`, `composite_exit`): the
/// plan, the instances bound to its game, the facts, the step `plan` repeating the plan, and
/// `install_command_done` of `install_pack_place`.
fn place_plan(items: usize, changes: &Value) -> Exit {
    let plan = placement_plan(items, changes);
    let members = object(json!({
        "plan": plan,
        "instances": ["neutral.a"],
        "facts": null,
        "steps": [{
            "step": "plan",
            "outcome": {"code": "install_command_done", "category": "success"},
            "result": plan,
        }],
    }));
    cli_line::success(members, &done(InstallLocation::PackPlace))
}

/// The line `exit` the rescue module gave for the row `row`, as this server answers it in [`ERA`],
/// its exports going to `exports` (`calls::Service::rescue`, once the module answered).
fn shaped(exports: &Path, row: &str, exit: &Exit) -> Exit {
    let service = Service::new(exports.to_path_buf());
    service.within_budget(ERA, |answer_budget| {
        rescue_export::answered(row, exit.clone(), ERA, answer_budget)
    })
}

/// The file the header's first warning names holds `whole`, byte for byte as the writer writes a
/// line, in `exports`; the header's `export` is its `{path, sha256, size}`.
#[track_caller]
fn assert_exported_whole(header: &Value, whole: &Value, exports: &Path) {
    use sha2::{Digest, Sha256};
    let warning = &header["warnings"][0]["outcome"];
    assert_eq!(warning["code"], "answer_exported", "first: {header}");
    let path = warning["values"]["path"]
        .as_str()
        .unwrap_or_else(|| panic!("answer_exported names its file: {header}"));
    assert!(
        Path::new(path).starts_with(exports),
        "written to the export directory: {path}"
    );
    let bytes = fs::read(path).expect("read the export");
    assert_eq!(
        bytes,
        serde_json::to_vec(whole).expect("the line encodes"),
        "the file's bytes are the whole line"
    );
    assert_eq!(
        header["export"],
        json!({
            "path": path,
            "sha256": format!("sha256:{:x}", Sha256::digest(bytes.as_slice())),
            "size": bytes.len(),
        }),
        "the header's export: {header}"
    );
    assert_eq!(
        (
            &warning["values"]["actual_sha256"],
            &warning["values"]["actual_bytes"]
        ),
        (&header["export"]["sha256"], &header["export"]["size"]),
        "the warning names the file as the export does: {header}"
    );
}

/// The `tools/call` result of `exit` is within the limit, not an error, and its text the line.
#[track_caller]
fn assert_answered_with_its_text(exit: &Exit) -> Value {
    let result = calls::call_result(exit, ERA);
    assert_eq!(result["isError"], json!(false), "{result}");
    assert!(
        protocol::complete(ERA, result.clone()).to_string().len() <= budget::RESULT_LIMIT,
        "within the limit"
    );
    text_line(&result)
}

#[test]
fn pp1_a_plan_over_the_budget_is_exported_whole_beside_what_must_be_read() {
    let temp = tempfile::tempdir().expect("tempdir");
    let exports = temp.path().join("materials");

    // `place --plan`.
    let changes = schedule_changes(4, 2);
    let exit = place_plan(250, &changes);
    let whole = line_of(&exit);
    assert!(
        budget::carried(&whole, ERA, false).is_err(),
        "setup: the line fits neither way, as the plan of 52 034 bytes did"
    );
    let answered = shaped(&exports, "place", &exit);
    assert_eq!(answered.code, 0, "a success stays one");
    let header = line_of(&answered);
    assert_exported_whole(&header, &whole, &exports);
    assert_eq!(
        keys(&header),
        ["plan", "export", "warnings", "outcome"],
        "{header}"
    );
    assert_eq!(keys(&header["plan"]), ["schedule_changes"], "{header}");
    assert_eq!(
        header["plan"]["schedule_changes"], changes,
        "schedule_changes whole, where the line holds it"
    );
    assert_eq!(header["outcome"], whole["outcome"], "the line's outcome");
    assert_eq!(
        header["warnings"].as_array().map(Vec::len),
        Some(1),
        "answer_exported, the line having none of its own: {header}"
    );
    assert_eq!(assert_answered_with_its_text(&answered), header);

    // `deploy --plan`: the outcome alone beside the export (the manual names no member of it).
    let files = (0..800)
        .map(|index| json!({"dst": format!("runtime/neutral-{index:04}.bin"), "action": "replace"}))
        .collect::<Vec<_>>();
    let deploy = cli_line::success(
        object(json!({
            "steps": [
                {"step": "stage", "outcome": {"code": "install_command_done"}, "result": {"kept": "0123abcd"}},
                {"step": "plan", "outcome": {"code": "install_command_done"}, "result": {
                    "plan_id": PLAN_ID,
                    "kind": "update",
                    "plan": {"files": files},
                    "notice": {"replaced": 800, "added": 0},
                }},
            ],
        })),
        &done(InstallLocation::Deploy),
    );
    let whole = line_of(&deploy);
    assert!(budget::carried(&whole, ERA, false).is_err(), "setup");
    let header = line_of(&shaped(&exports, "deploy", &deploy));
    assert_exported_whole(&header, &whole, &exports);
    assert_eq!(keys(&header), ["export", "warnings", "outcome"], "{header}");
    assert_eq!(header["outcome"], whole["outcome"]);

    // `op status`: the members the manual names as its answer.
    let packs = (0..400)
        .map(|index| json!({"game": "neutral", "package_id": package(index), "sha256": "0".repeat(64)}))
        .collect::<Vec<_>>();
    let status = cli_line::success(
        object(json!({
            "root": "C:\\ActingCommand",
            "packs": packs,
            "unfinished": null,
            "last_operations": [
                {"operation_id": "op-3", "kind": "pack_place", "result": "install_command_done", "undone": false},
                {"operation_id": "op-2", "kind": "update", "result": "install_command_done", "undone": false},
            ],
            "leftovers": 0,
            "journal": {
                "operation_id": "op-3",
                "kind": "pack_place",
                "steps": [{"n": 1, "kind": "plan", "state": "done"}],
            },
        })),
        &done(InstallLocation::Status),
    );
    let whole = line_of(&status);
    assert!(budget::carried(&whole, ERA, false).is_err(), "setup");
    let header = line_of(&shaped(&exports, "op status", &status));
    assert_exported_whole(&header, &whole, &exports);
    assert_eq!(
        keys(&header),
        [
            "unfinished",
            "last_operations",
            "journal",
            "export",
            "warnings",
            "outcome"
        ],
        "{header}"
    );
    for member in ["unfinished", "last_operations", "journal", "outcome"] {
        assert_eq!(header[member], whole[member], "{member}: whole");
    }
}

#[test]
fn pp1_a_larger_plan_keeps_the_four_lists_then_the_counts() {
    let temp = tempfile::tempdir().expect("tempdir");
    let exports = temp.path().join("materials");
    let pointer = |name: &str| format!("/plan/schedule_changes/{name}");

    // `packages` does not fit: the four lists stay, `packages` counted in its place.
    let changes = schedule_changes(250, 2);
    assert!(
        !budget::fits(&json!({"plan": {"schedule_changes": changes}}), ERA),
        "setup: schedule_changes whole does not fit"
    );
    let exit = place_plan(3, &changes);
    let whole = line_of(&exit);
    assert!(budget::carried(&whole, ERA, false).is_err(), "setup");
    let header = line_of(&shaped(&exports, "place", &exit));
    assert_exported_whole(&header, &whole, &exports);
    assert_eq!(
        header["plan"]["schedule_changes"],
        json!({
            "packages_total": 250,
            "enters": changes["enters"],
            "leaves": changes["leaves"],
            "content_only": changes["content_only"],
            "unread": changes["unread"],
        }),
        "the four lists, packages counted: {header}"
    );
    assert_eq!(
        header["truncated"],
        json!([pointer("packages")]),
        "{header}"
    );
    assert_eq!(header["outcome"], whole["outcome"]);
    assert_answered_with_its_text(&shaped(&exports, "place", &exit));

    // Larger still: `enters` does not fit either; every list counted.
    let changes = schedule_changes(250, 1000);
    let without_rows = json!({"plan": {"schedule_changes": {
        "enters": changes["enters"],
        "leaves": changes["leaves"],
        "content_only": changes["content_only"],
        "unread": changes["unread"],
    }}});
    assert!(
        !budget::fits(&without_rows, ERA),
        "setup: the four lists do not fit"
    );
    let exit = place_plan(3, &changes);
    let whole = line_of(&exit);
    let header = line_of(&shaped(&exports, "place", &exit));
    assert_exported_whole(&header, &whole, &exports);
    assert_eq!(
        header["plan"]["schedule_changes"],
        json!({
            "packages_total": 250,
            "enters_total": 1000,
            "leaves_total": 1,
            "content_only_total": 0,
            "unread_total": 1,
        }),
        "the counts: {header}"
    );
    assert_eq!(
        header["truncated"],
        json!([
            pointer("packages"),
            pointer("enters"),
            pointer("leaves"),
            pointer("content_only"),
            pointer("unread"),
        ]),
        "{header}"
    );
    assert_eq!(header["outcome"], whole["outcome"]);
    assert_answered_with_its_text(&shaped(&exports, "place", &exit));

    // `undo --plan`, by the same rule: `reverses` whole, then its `schedule_changes`.
    let reverses = json!([
        {"n": 3, "kind": "pack_item", "target": "packs/neutral.pack.0001"},
        {"n": 2, "kind": "pack_item", "target": "packs/neutral.pack.0000"},
    ]);
    let changes = schedule_changes(250, 2);
    let undo = cli_line::success(
        object(json!({
            "operation_id": "op-3",
            "reverses": reverses,
            "schedule_changes": changes,
        })),
        &done(InstallLocation::Undo),
    );
    let whole = line_of(&undo);
    assert!(budget::carried(&whole, ERA, false).is_err(), "setup");
    let header = line_of(&shaped(&exports, "undo", &undo));
    assert_exported_whole(&header, &whole, &exports);
    assert_eq!(
        keys(&header),
        [
            "reverses",
            "schedule_changes",
            "truncated",
            "export",
            "warnings",
            "outcome"
        ],
        "{header}"
    );
    assert_eq!(header["reverses"], reverses, "reverses whole");
    assert_eq!(
        keys(&header["schedule_changes"]),
        [
            "packages_total",
            "enters",
            "leaves",
            "content_only",
            "unread"
        ],
        "{header}"
    );
    assert_eq!(
        header["truncated"],
        json!(["/schedule_changes/packages"]),
        "{header}"
    );
}

#[test]
fn pp1_run_show_of_the_job_answers_the_header_and_the_export() {
    let temp = tempfile::tempdir().expect("tempdir");
    let exports = temp.path().join("materials");
    let exit = place_plan(250, &schedule_changes(4, 2));
    let whole = line_of(&exit);
    let process_jobs = jobs::Jobs::new(
        jobs::Limits {
            running: 2,
            kept: 2,
        },
        0,
    );

    // Before the slice the job kept the module's line as it came: `ac_run_show` of its handle
    // answered `output_budget_exceeded` and the names of its members, as on production 0.12.70.
    let as_it_came = calls::finished(&exit);
    let before = process_jobs
        .start(process_jobs.handle(), "ac_place", false, move || as_it_came)
        .expect("the job starts");
    before.wait_ended();
    let shown = text_line(&server::job_answer(&before, true, ERA).result);
    assert_eq!(
        (&shown["outcome"]["code"], &shown["summary"]["members"]),
        (
            &json!("output_budget_exceeded"),
            &json!(["plan", "instances", "facts", "steps"])
        ),
        "setup: the answer observed on production: {shown}"
    );

    // Now the job keeps the header, made as every served call's job is.
    let answered = shaped(&exports, "place", &exit);
    let header = line_of(&answered);
    let ended = calls::finished(&answered);
    let job = process_jobs
        .start(process_jobs.handle(), "ac_place", false, move || ended)
        .expect("the job starts");
    job.wait_ended();
    // `ac_run_show` of its handle.
    let shown = server::job_answer(&job, true, ERA);
    assert!(
        shown.handle.is_none(),
        "its line, no handle: {}",
        shown.result
    );
    assert_eq!(shown.result["isError"], json!(false), "{}", shown.result);
    assert!(
        protocol::complete(ERA, shown.result.clone())
            .to_string()
            .len()
            <= budget::RESULT_LIMIT,
        "within the limit"
    );
    let line = text_line(&shown.result);
    assert_eq!(line, header, "the header");
    assert!(
        line.get("summary").is_none() && line.get("members").is_none(),
        "not a summary of member names: {line}"
    );
    assert_exported_whole(&line, &whole, &exports);
    assert!(
        line["plan"].get("schedule_changes").is_some(),
        "what must be read before acting: {line}"
    );
}

#[test]
fn pp1_an_answer_within_the_budget_and_a_refusal_are_as_before() {
    let temp = tempfile::tempdir().expect("tempdir");
    let exports = temp.path().join("materials");

    // A plan repeated as text beside `structuredContent`.
    let small = place_plan(3, &schedule_changes(2, 1));
    assert!(
        budget::fits(&line_of(&small), ERA),
        "setup: it fits with its text"
    );
    // A plan sent in `structuredContent` alone beside a summary text: within the budget too.
    let structured_only = place_plan(100, &schedule_changes(2, 1));
    let result = calls::call_result(&structured_only, ERA);
    assert!(
        result["isError"] == json!(false)
            && result["content"][0]["text"]
                .as_str()
                .is_some_and(|text| text.contains("structured_content_only")),
        "setup: within the budget in structuredContent alone: {}",
        result["content"]
    );
    // A refusal over the budget: `place`'s refusal of a placement whose plan could not read the
    // zip's side, its `schedule_changes` beside it (`place::authorize`).
    let refusal = cli_line::receipt(
        object(json!({"schedule_changes": schedule_changes(400, 2)})),
        &Outcome::new(ActingctlCode::InstallCommandRefused)
            .operation(InstallLocation::PackPlace)
            .install_refusal(InstallRefusal::PlaceEnablesUnknown),
        true,
    );
    assert!(
        refusal.code != 0 && budget::carried(&line_of(&refusal), ERA, true).is_err(),
        "setup: a refusal over the budget"
    );

    for (what, exit) in [
        ("repeated as text", &small),
        ("in structuredContent alone", &structured_only),
        ("a refusal over the budget", &refusal),
    ] {
        let answered = shaped(&exports, "place", exit);
        assert_eq!(answered.line, exit.line, "{what}: the line as it came");
        assert_eq!(answered.code, exit.code, "{what}: its exit code");
        assert_eq!(
            calls::call_result(&answered, ERA),
            calls::call_result(exit, ERA),
            "{what}: answered byte for byte as before"
        );
    }
    assert!(!exports.exists(), "nothing written: {}", exports.display());
}

#[test]
fn pp1_an_export_directory_that_cannot_be_written() {
    let temp = tempfile::tempdir().expect("tempdir");
    // A file where the export directory's parent would be: the writer cannot make the directory.
    let blocked = temp.path().join("blocked");
    fs::write(&blocked, b"neutral").expect("write the file in the way");
    let exports = blocked.join("materials");

    let changes = schedule_changes(4, 2);
    let exit = place_plan(250, &changes);
    let whole = line_of(&exit);
    let answered = shaped(&exports, "place", &exit);
    let header = line_of(&answered);
    assert_eq!(
        header["warnings"][0]["outcome"]["code"], "material_export_failed",
        "first: {header}"
    );
    assert!(header.get("export").is_none(), "no export: {header}");
    assert_eq!(
        keys(&header),
        ["plan", "warnings", "outcome"],
        "the header's parts and outcome: {header}"
    );
    assert_eq!(header["plan"]["schedule_changes"], changes);
    assert_eq!(header["outcome"], whole["outcome"]);
    assert_eq!(assert_answered_with_its_text(&answered), header);
    assert_eq!(
        fs::read(blocked.as_path()).expect("read the file in the way"),
        b"neutral",
        "nothing written"
    );

    // A header that does not fit even so (the line's own warnings): `output_budget_exceeded` with
    // its job's handle, and `ac_run_show` of the handle its summary.
    let mut noisy = place_plan(250, &changes);
    if let Ok(Value::Object(members)) = &mut noisy.line {
        let warnings = (0..NOISY)
            .map(|index| {
                json!({"outcome": {
                    "code": "install_caveat_noted",
                    "category": "warning",
                    "values": {"names": [package(index)], "note": "n".repeat(80)},
                }})
            })
            .collect::<Vec<_>>();
        members.insert("warnings".to_owned(), Value::Array(warnings));
    }
    let answered = shaped(&exports, "place", &noisy);
    let header = line_of(&answered);
    assert_eq!(
        header["warnings"][0]["outcome"]["code"], "material_export_failed",
        "first"
    );
    assert_eq!(
        header["truncated"],
        json!(["/plan/schedule_changes"]),
        "the part left to the line: {}",
        header["truncated"]
    );
    let ended = calls::finished(&answered);
    assert!(
        calls::sent(&ended, ERA).is_err(),
        "setup: the header does not fit"
    );
    let process_jobs = jobs::Jobs::new(
        jobs::Limits {
            running: 1,
            kept: 1,
        },
        0,
    );
    let job = process_jobs
        .start(process_jobs.handle(), "ac_place", false, move || ended)
        .expect("the job starts");
    job.wait_ended();
    let call = server::job_answer(&job, false, ERA);
    assert_eq!(call.handle.as_deref(), Some(job.handle.as_str()));
    let line = text_line(&call.result);
    assert_eq!(
        (&line["outcome"]["code"], &line["handle"]),
        (&json!("output_budget_exceeded"), &json!(job.handle)),
        "{line}"
    );
    let shown = text_line(&server::job_answer(&job, true, ERA).result);
    assert_eq!(
        shown["outcome"]["code"], "output_budget_exceeded",
        "{shown}"
    );
    assert_eq!(
        (
            &shown["summary"]["outcome"],
            &shown["summary"]["warnings_total"],
            &shown["summary"]["truncated"],
        ),
        (
            &whole["outcome"],
            &json!(NOISY + 1),
            &json!(["/plan/schedule_changes"]),
        ),
        "the job's summary: {shown}"
    );
}
