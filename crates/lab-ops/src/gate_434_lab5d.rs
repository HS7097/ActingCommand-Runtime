// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #434 LAB-5D (register §1.287, "Slice LAB-5D"): the slice's named tests, on small
//! made-up answers (no Runtime, no device, no clock, no sleep).
//!
//! - `lab_ops_no_budget_whole_answer`: each of the eleven reads whose MCP answer was cut or
//!   exported in MCP's own code gives, with no budget, the whole answer the CLI prints.
//! - `lab_ops_budget_cuts_keep_their_markers`: each of the seven reads that cut, given a `fits`
//!   that refuses the whole answer, cuts it as MCP's code did, with the same markers.
//! - `lab_ops_budget_exports_the_header`: each of the four reads that export (and the payload
//!   page of `events`), given a `fits` that refuses the whole answer, writes it whole through the
//!   caller's writer and answers its header with the export reference and `answer_exported`.

use crate::budget::{Budget, Fitted};
use crate::code_lookup::{self, Answer, ListQuery, Query};
use crate::codes::ActingctlCode;
use crate::diagnose::Diagnosis;
use crate::run_evidence::{Evidence, StepDetail, StepEvidence};
use crate::run_list::RunsAnswer;
use crate::{events, facts, overview, packs, remedy, routine_summary, schedule};
use actingcommand_contract::outcome::{Detail, Outcome};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::fs;
use tempfile::TempDir;

/// An overview answer with one block of each kind its cut reaches.
fn overview_answer() -> Value {
    json!({
        "daemon": {"online": true},
        "host": {"load": 1},
        "startup": {"phase": "ready"},
        "instances": [
            {"alias": "alpha", "run": {"state": "idle"}, "monitor": {"interval_ms": 1000}},
        ],
        "install": {"build": {"commit": "0000000"}, "root": "neutral"},
        "fact_refresh": {"rows": []},
    })
}

/// A diagnosis with three non-run failure groups, one run group and one run.
fn diagnosis() -> Diagnosis {
    Diagnosis {
        instance: Value::Null,
        since: 1_000,
        incomplete: false,
        incomplete_sections: Vec::new(),
        runs: vec![json!({"request_id": "request_alpha"})],
        errors: Vec::new(),
        error_sequences: Vec::new(),
        errors_more: false,
        errors_next: None,
        snapshot: 9,
        continuable: false,
        repeating: Vec::new(),
        suspended: Vec::new(),
        packs: Vec::new(),
        remedies: Vec::new(),
        reruns: Vec::new(),
        lift: Vec::new(),
        report_warnings: Vec::new(),
        report_status: json!({"status": "complete"}),
        report_outcome: None,
        clock_slots: Vec::new(),
        clock_slots_total: 0,
        runs_grouped: vec![json!({"task_id": "neutral.task", "count": 2})],
        failures_grouped: vec![
            json!({"event_type": "runtime.failed", "count": 3}),
            json!({"event_type": "command.rejected", "count": 2}),
            json!({"event_type": "policy.execution_recorded", "count": 1}),
        ],
        not_run: None,
        warnings: Vec::new(),
    }
}

/// The errors page the caller renders with its first `kept` rows (this diagnosis has none).
fn errors_page(kept: usize) -> Result<Value, String> {
    Ok(json!({"events": [], "more": false, "kept": kept, "snapshot_ledger_position": 9}))
}

/// An outcome inside a diagnosis, as a caller writes it.
fn envelope(outcome: &Outcome) -> Value {
    crate::cli_line::outcome_value(outcome, Detail::Full)
}

/// A brief page's rows, the first with two materials.
fn event_rows() -> Vec<Value> {
    vec![
        json!({"seq": 1, "materials": [{"ref": "material_one"}, {"ref": "material_two"}]}),
        json!({"seq": 2}),
        json!({"seq": 3}),
    ]
}

/// The page of `rows` with the first `kept`, made up as `events`' page renders one: with fewer
/// than all, `truncated` and a cursor after the last row kept.
fn event_page(rows: &[Value], kept: usize) -> Result<Value, String> {
    let mut page = Map::new();
    page.insert("events".to_owned(), Value::Array(rows[..kept].to_vec()));
    if kept < rows.len() {
        page.insert(
            "next_cursor".to_owned(),
            json!(format!("after.{}", rows[kept - 1]["seq"])),
        );
        page.insert("truncated".to_owned(), json!(true));
    }
    page.insert("snapshot_ledger_position".to_owned(), json!(9));
    Ok(Value::Object(page))
}

/// A schedule answer: two entries, three candidate rows and two.
fn schedule_answer() -> Value {
    let row = |task: &str| json!({"task_id": task, "state": "deferred", "eligibility": "true"});
    json!({
        "evaluated_at_unix_ms": 1_000,
        "instances": [
            {
                "instance_alias": "alpha",
                "candidates": [row("neutral.one"), row("neutral.two"), row("neutral.three")],
                "candidates_total": 3,
            },
            {
                "instance_alias": "beta",
                "candidates": [row("neutral.four"), row("neutral.five")],
                "candidates_total": 2,
            },
        ],
    })
}

/// How many candidate rows a schedule answer keeps.
fn candidate_rows(answer: &Value) -> usize {
    answer["instances"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry["candidates"].as_array())
        .map(Vec::len)
        .sum()
}

/// A run list of three rows, the newest first, with no page after it.
fn runs() -> RunsAnswer {
    RunsAnswer::of_rows(
        vec![
            (json!({"request_id": "request_three"}), 30),
            (json!({"request_id": "request_two"}), 20),
            (json!({"request_id": "request_one"}), 10),
        ],
        1_000,
        9,
        None,
    )
}

/// A run's evidence of three steps, each with two frames.
fn evidence() -> Evidence {
    let mut identity = Map::new();
    identity.insert("request_id".to_owned(), json!("request_alpha"));
    let steps = (0..3_u32)
        .map(|index| StepEvidence {
            step_index: index,
            page: "neutral/page".to_owned(),
            targets: vec![json!({"target": "neutral.button", "matched": true})],
            action: None,
            frames: vec![
                (u64::from(index) * 10 + 1, format!("frame_{index}_a")),
                (u64::from(index) * 10 + 2, format!("frame_{index}_b")),
            ],
            decided_frame: true,
            first_sequence: u64::from(index) * 10 + 1,
            last_sequence: u64::from(index) * 10 + 3,
            started_unix_ms: 1,
            finished_unix_ms: Some(2),
            failure: None,
        })
        .collect();
    Evidence {
        identity,
        steps,
        final_: json!({"page": "neutral/page"}),
        overlay: None,
        from_step: 0,
        limit: 20,
        detail: StepDetail::Full,
    }
}

/// A page of the code list of three made-up codes.
fn codes() -> code_lookup::Page {
    let mut codes = Map::new();
    for name in [
        "neutral_alpha_failed",
        "neutral_beta_failed",
        "neutral_gamma_failed",
    ] {
        let mut entry = Map::new();
        for (member, value) in [
            ("category", "error"),
            ("ladder_class", "none"),
            ("owner", "neutral-owner"),
            ("status", "active"),
        ] {
            entry.insert(member.to_owned(), json!(value));
        }
        codes.insert(name.to_owned(), Value::Object(entry));
    }
    let mut catalog = Map::new();
    catalog.insert("codes".to_owned(), Value::Object(codes));
    let query = Query::List(ListQuery {
        prefix: None,
        owner: None,
        limit: code_lookup::MAX_LIMIT,
        cursor: None,
    });
    match code_lookup::read(&Value::Object(catalog), &query) {
        Ok(Answer::List(page)) => page,
        Ok(Answer::Entry(entry)) => panic!("a list query answered an entry: {entry}"),
        Err(refusal) => panic!("the list was refused: {refusal:?}"),
    }
}

fn facts_answer() -> Value {
    json!({
        "scope": "program",
        "read_at_unix_ms": 5_000,
        "snapshot": {"records": [{"key": "neutral.key", "content": 1}]},
        "records_total": 1,
        "records_kept": 1,
    })
}

fn remedy_answer() -> Value {
    json!({
        "instance_alias": "alpha",
        "read_at_unix_ms": 5_000,
        "records": [],
        "in_effect": null,
    })
}

fn summary_answer() -> Value {
    json!({
        "owner_epoch": "epoch_alpha",
        "granularity": "hour",
        "revision": 3,
        "instances": [{"instance_alias": "alpha", "coverage_from_unix_ms": 1, "buckets": []}],
    })
}

/// The packages a made-up configuration binds: a temporary folder (kept alive by the caller) and
/// the answer of `packs` read from it.
fn packs_answer() -> (TempDir, packs::Answer) {
    let root = TempDir::new().expect("tempdir");
    let config = root.path().join("actingd.config.json");
    let bindings = (0..2)
        .map(|index| {
            json!({
                "procedure_ref": format!("package.neutral.task{index}"),
                "operation_id": format!("neutral.task{index}"),
                "yield_points": [],
            })
        })
        .collect::<Vec<_>>();
    let document = json!({"instances": [], "policy": {"procedure_manifest": bindings}});
    fs::write(&config, document.to_string()).expect("write the configuration");
    let answer = packs::read(None, &config, &packs::Query::default())
        .unwrap_or_else(|error| panic!("packs refused the configuration: {error}"));
    (root, answer)
}

/// A warning as a caller writes it beside its answer.
fn warning(outcome: &Outcome) -> Value {
    crate::cli_line::warning_value(outcome, Detail::Full)
}

/// The names of an object's members, in order.
fn members(value: &Value) -> Vec<String> {
    value
        .as_object()
        .map(|members| members.keys().cloned().collect())
        .unwrap_or_default()
}

/// What the made-up writer was given: `(sha256, extension, bytes)` per write.
type Written = RefCell<Vec<(String, String, Vec<u8>)>>;

/// The made-up writer's export reference of `bytes`.
fn reference(sha256: &str, bytes: &[u8]) -> Value {
    let hex = sha256.strip_prefix("sha256:").unwrap_or(sha256);
    json!({"path": format!("exports/{hex}.json"), "sha256": sha256, "size": bytes.len()})
}

/// `fitted` is the header `header` of `answer` beside the reference of the one write, which
/// holds `whole`, and `answer_exported` names the file.
fn assert_exported(fitted: &Fitted, written: &Written, whole: &[u8], header: &[&str], read: &str) {
    let writes = written.borrow();
    assert_eq!(
        writes.len(),
        1,
        "{read}: one write through the caller's writer"
    );
    let (sha256, extension, bytes) = &writes[0];
    assert_eq!(
        bytes.as_slice(),
        whole,
        "{read}: the whole answer is written"
    );
    assert_eq!(
        *sha256,
        format!("sha256:{:x}", Sha256::digest(whole)),
        "{read}: its sha256"
    );
    assert_eq!(extension, "json", "{read}: a JSON file");
    let expected = reference(sha256, whole);
    let mut names = header
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<Vec<_>>();
    names.push("export".to_owned());
    assert_eq!(members(&fitted.result), names, "{read}: {}", fitted.result);
    assert_eq!(fitted.result["export"], expected, "{read}");
    let note = fitted
        .exported
        .as_ref()
        .unwrap_or_else(|| panic!("{read}: no answer_exported"));
    assert!(note.is(ActingctlCode::AnswerExported), "{read}: {note:?}");
    assert_eq!(note.value("path"), Some(&expected["path"]), "{read}");
}

// No budget: each of the eleven reads gives the whole answer, the answer the CLI prints.
#[test]
fn lab_ops_no_budget_whole_answer() {
    // status
    let status = overview_answer();
    assert_eq!(overview::within_budget(status.clone(), None), status);

    // diagnose: every list whole, the errors page whole.
    let diagnosis = diagnosis();
    let whole = diagnosis.render(
        &diagnosis.kept_all(),
        errors_page(diagnosis.errors.len()).expect("errors page"),
        &envelope,
    );
    let answered = diagnosis
        .within_budget(None, &errors_page, &envelope)
        .expect("diagnose");
    assert_eq!(answered, whole);
    assert!(answered.get("failures_grouped_truncated").is_none());

    // events: a brief page and a payload page whole.
    let rows = event_rows();
    let page = events::list_within_budget(rows.clone(), None, &event_page).expect("events");
    assert_eq!(page, event_page(&rows, rows.len()).expect("page"));
    let payload = event_page(&rows, rows.len()).expect("page");
    let fitted =
        events::payload_within_budget(payload.clone(), rows.len(), None).expect("payload page");
    assert_eq!(fitted.result, payload);
    assert!(fitted.exported.is_none());

    // schedule
    let explanation = schedule_answer();
    assert_eq!(
        schedule::within_budget(explanation.clone(), None),
        explanation
    );

    // runs
    let run_list = runs();
    assert_eq!(
        run_list.within_budget(None),
        run_list.render(run_list.row_count())
    );

    // run evidence
    let steps = evidence();
    assert_eq!(steps.within_budget(None), steps.render(steps.page_len()));

    // codes
    let code_list = codes();
    assert_eq!(
        code_list.within_budget(None).expect("codes"),
        code_list.render(code_list.row_count()).expect("render")
    );

    // facts, remedy, summary
    for (read, answer, fitted) in [
        (
            "facts",
            facts_answer(),
            facts::within_budget(facts_answer(), None),
        ),
        (
            "remedy",
            remedy_answer(),
            remedy::within_budget(remedy_answer(), None),
        ),
        (
            "summary",
            summary_answer(),
            routine_summary::within_budget(summary_answer(), None),
        ),
    ] {
        let fitted = fitted.unwrap_or_else(|outcome| panic!("{read}: {outcome:?}"));
        assert_eq!(fitted.result, answer, "{read}");
        assert!(fitted.exported.is_none(), "{read}");
    }

    // packs: the bindings, the CLI's document without its warnings.
    let (_root, answer) = packs_answer();
    let fitted = answer
        .within_budget(None, &warning)
        .unwrap_or_else(|outcome| panic!("packs: {outcome:?}"));
    let mut document = answer.document(warning).expect("encode the document");
    document.remove("warnings");
    assert_eq!(fitted.result, Value::Object(document));
    assert!(fitted.exported.is_none());
}

// A `fits` that refuses the whole answer: each of the seven reads that cut cuts it as MCP's code
// did, with the same markers.
#[test]
fn lab_ops_budget_cuts_keep_their_markers() {
    // status: one block at a time, `<block>_truncated` where it was, `truncated` in cut order.
    let without_host = |answer: &Value| answer.get("host").is_none();
    let cut = overview::within_budget(overview_answer(), Some(&Budget::new(&without_host)));
    assert_eq!(cut["host_truncated"], json!(true), "{cut}");
    assert_eq!(cut["truncated"], json!(["/host"]), "{cut}");
    assert_eq!(cut["startup"], json!({"phase": "ready"}), "{cut}");
    let nothing = |_: &Value| false;
    let cut = overview::within_budget(overview_answer(), Some(&Budget::new(&nothing)));
    assert_eq!(
        cut["truncated"],
        json!([
            "/host",
            "/startup",
            "/instances/0/run",
            "/instances/0/monitor",
            "/install/build",
            "/fact_refresh",
        ]),
        "{cut}"
    );
    assert_eq!(cut["instances"][0]["run_truncated"], json!(true), "{cut}");
    assert_eq!(cut["install"]["build_truncated"], json!(true), "{cut}");
    assert_eq!(cut["fact_refresh_truncated"], json!(true), "{cut}");
    assert_eq!(cut["daemon"], json!({"online": true}), "{cut}");

    // diagnose: the fewest-count groups first, each list cut short `<list>_truncated`.
    let diagnosis = diagnosis();
    let one_group = |answer: &Value| answer["failures_grouped"].as_array().map(Vec::len) == Some(1);
    let cut = diagnosis
        .within_budget(Some(&Budget::new(&one_group)), &errors_page, &envelope)
        .expect("diagnose");
    assert_eq!(
        cut["failures_grouped"],
        json!([{"event_type": "runtime.failed", "count": 3}]),
        "{cut}"
    );
    assert_eq!(cut["failures_grouped_truncated"], json!(true), "{cut}");
    assert!(cut.get("runs_grouped_truncated").is_none(), "{cut}");
    assert_eq!(cut["runs"].as_array().map(Vec::len), Some(1), "{cut}");
    let cut = diagnosis
        .within_budget(Some(&Budget::new(&nothing)), &errors_page, &envelope)
        .expect("diagnose");
    for list in ["failures_grouped", "runs_grouped", "runs"] {
        assert_eq!(cut[list], json!([]), "{list}: {cut}");
        assert_eq!(
            cut[format!("{list}_truncated")],
            json!(true),
            "{list}: {cut}"
        );
    }

    // events: rows from the end, `truncated` and the cursor after the last row kept; then the one
    // row's material list, `materials_truncated`.
    let two_rows = |page: &Value| page["events"].as_array().map(Vec::len) <= Some(2);
    let cut = events::list_within_budget(event_rows(), Some(&Budget::new(&two_rows)), &event_page)
        .expect("events");
    assert_eq!(cut["events"].as_array().map(Vec::len), Some(2), "{cut}");
    assert_eq!(cut["truncated"], json!(true), "{cut}");
    assert_eq!(cut["next_cursor"], json!("after.2"), "{cut}");
    let one_material = |page: &Value| {
        page["events"].as_array().map(Vec::len) == Some(1)
            && page["events"][0]["materials"].as_array().map(Vec::len) == Some(1)
    };
    let cut =
        events::list_within_budget(event_rows(), Some(&Budget::new(&one_material)), &event_page)
            .expect("events");
    assert_eq!(
        cut["events"][0]["materials"],
        json!([{"ref": "material_one"}]),
        "{cut}"
    );
    assert_eq!(
        cut["events"][0]["materials_truncated"],
        json!(true),
        "{cut}"
    );
    assert_eq!(cut["next_cursor"], json!("after.1"), "{cut}");

    // schedule: candidate rows only, from the last row of the last entry, each entry that lost
    // rows `candidates_truncated`.
    let three_rows = |answer: &Value| candidate_rows(answer) <= 3;
    let cut = schedule::within_budget(schedule_answer(), Some(&Budget::new(&three_rows)));
    assert_eq!(
        cut["instances"][0],
        schedule_answer()["instances"][0],
        "{cut}"
    );
    assert_eq!(cut["instances"][1]["candidates"], json!([]), "{cut}");
    assert_eq!(
        cut["instances"][1]["candidates_truncated"],
        json!(true),
        "{cut}"
    );
    assert_eq!(cut["instances"][1]["candidates_total"], json!(2), "{cut}");

    // runs: rows from the last, `truncated` and next_cursor after the last row kept.
    let one_run = |answer: &Value| answer["runs"].as_array().map(Vec::len) == Some(1);
    let cut = runs().within_budget(Some(&Budget::new(&one_run)));
    assert_eq!(
        cut["runs"],
        json!([{"request_id": "request_three"}]),
        "{cut}"
    );
    assert_eq!(cut["truncated"], json!(true), "{cut}");
    assert_eq!(
        cut["next_cursor"],
        json!("r1.9.30.1000.-.0123456789abcdef"),
        "{cut}"
    );

    // run evidence: whole steps from the first, `truncated` and `next_step`; a first step alone
    // over the budget has its frames referenced.
    let two_steps = |answer: &Value| answer["steps"].as_array().map(Vec::len) <= Some(2);
    let cut = evidence().within_budget(Some(&Budget::new(&two_steps)));
    assert_eq!(cut["steps"].as_array().map(Vec::len), Some(2), "{cut}");
    assert_eq!(cut["truncated"], json!(true), "{cut}");
    assert_eq!(cut["next_step"], json!(2), "{cut}");
    let no_frames = |answer: &Value| {
        answer["steps"]
            .as_array()
            .is_some_and(|steps| steps.iter().all(|step| step.get("frames").is_none()))
    };
    let cut = evidence().within_budget(Some(&Budget::new(&no_frames)));
    assert_eq!(cut["steps"].as_array().map(Vec::len), Some(1), "{cut}");
    assert_eq!(cut["steps"][0]["omitted"], json!(["frames"]), "{cut}");
    assert_eq!(cut["steps"][0]["frames_total"], json!(2), "{cut}");
    assert_eq!(cut["truncated"], json!(true), "{cut}");
    assert_eq!(cut["next_step"], json!(1), "{cut}");

    // codes: rows from the last, `truncated` and next_cursor after the last row kept.
    let one_code = |answer: &Value| answer["codes"].as_array().map(Vec::len) == Some(1);
    let cut = codes()
        .within_budget(Some(&Budget::new(&one_code)))
        .expect("codes");
    assert_eq!(
        cut["codes"][0]["code"],
        json!("neutral_alpha_failed"),
        "{cut}"
    );
    assert_eq!(cut["truncated"], json!(true), "{cut}");
    assert!(
        cut["next_cursor"]
            .as_str()
            .is_some_and(|cursor| cursor.starts_with("c1.neutral_alpha_failed.")),
        "{cut}"
    );
}

// A `fits` that refuses the whole answer: each of the four reads that export, and the payload
// page of `events`, writes the whole answer through the caller's writer and answers its header
// beside the export reference, with `answer_exported` naming the file.
#[test]
fn lab_ops_budget_exports_the_header() {
    let nothing = |_: &Value| false;
    let written = Written::default();
    let writer = |sha256: &str, extension: &str, bytes: &[u8]| -> Result<Value, Outcome> {
        written
            .borrow_mut()
            .push((sha256.to_owned(), extension.to_owned(), bytes.to_vec()));
        Ok(reference(sha256, bytes))
    };
    let budget = Budget::new(&nothing).with_export(&writer);

    // facts, remedy, summary: the answer's JSON written whole, its header members kept.
    let reads: [(&str, Value, &[&str]); 3] = [
        (
            "facts",
            facts_answer(),
            &["scope", "read_at_unix_ms", "records_total", "records_kept"],
        ),
        (
            "remedy",
            remedy_answer(),
            &["instance_alias", "read_at_unix_ms", "in_effect"],
        ),
        (
            "summary",
            summary_answer(),
            &["owner_epoch", "granularity", "revision"],
        ),
    ];
    for (read, answer, header) in reads {
        written.borrow_mut().clear();
        let fitted = match read {
            "facts" => facts::within_budget(answer.clone(), Some(&budget)),
            "remedy" => remedy::within_budget(answer.clone(), Some(&budget)),
            _ => routine_summary::within_budget(answer.clone(), Some(&budget)),
        }
        .unwrap_or_else(|outcome| panic!("{read}: {outcome:?}"));
        let whole = serde_json::to_vec(&answer).expect("encode the answer");
        assert_exported(&fitted, &written, &whole, header, read);
        for member in header {
            assert_eq!(fitted.result[*member], answer[*member], "{read}: {member}");
        }
    }

    // A caller with no export gets the whole answer, for it to refuse.
    let fitted = facts::within_budget(facts_answer(), Some(&Budget::new(&nothing)))
        .unwrap_or_else(|outcome| panic!("facts: {outcome:?}"));
    assert_eq!(fitted.result, facts_answer());
    assert!(fitted.exported.is_none());

    // packs: the whole document, its warnings included, written; `{source, export}`, and the
    // note counts the warnings the file holds.
    written.borrow_mut().clear();
    let (_root, answer) = packs_answer();
    let fitted = answer
        .within_budget(Some(&budget), &warning)
        .unwrap_or_else(|outcome| panic!("packs: {outcome:?}"));
    let document = answer.document(warning).expect("encode the document");
    let whole = serde_json::to_vec(&Value::Object(document.clone())).expect("encode");
    assert_exported(&fitted, &written, &whole, &["source"], "packs");
    assert_eq!(fitted.result["source"], document["source"]);
    let count = fitted
        .exported
        .as_ref()
        .and_then(|note| note.value("count"));
    assert_eq!(count, Some(&json!(answer.warnings().len())));

    // events: a payload page written whole, `{export, events_count, next_cursor?,
    // last_sequence?}`.
    written.borrow_mut().clear();
    let page = json!({
        "events": [{"seq": 1, "payload": {}}, {"seq": 2, "payload": {}}],
        "next_cursor": "after.2",
        "snapshot_ledger_position": 9,
        "last_sequence": 2,
    });
    let fitted = events::payload_within_budget(page.clone(), 2, Some(&budget))
        .unwrap_or_else(|outcome| panic!("events: {outcome:?}"));
    let whole = serde_json::to_vec(&page).expect("encode the page");
    let writes = written.borrow();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].2, whole);
    let expected = reference(&writes[0].0, &whole);
    assert_eq!(
        members(&fitted.result),
        ["export", "events_count", "next_cursor", "last_sequence"]
    );
    assert_eq!(fitted.result["export"], expected);
    assert_eq!(fitted.result["events_count"], json!(2));
    assert_eq!(fitted.result["next_cursor"], json!("after.2"));
    assert_eq!(fitted.result["last_sequence"], json!(2));
    let note = fitted.exported.as_ref().expect("answer_exported");
    assert!(note.is(ActingctlCode::AnswerExported));
    assert_eq!(note.value("path"), Some(&expected["path"]));
}
