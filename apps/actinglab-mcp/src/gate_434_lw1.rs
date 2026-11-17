// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #434 LW-1 (register §1.435; the coordinator's work order of 11-16 on what an agent could
//! not do through the MCP front on a real walk, and its correction of item 1 the same day): the
//! server's own code, no Runtime, no clock, no sleep; exports go to a temporary folder.
//!
//! - `lw1_tap_target_and_navigate_tools_say_they_only_plan` (item 1): `ac_tap_target` and
//!   `ac_navigate` take no `capture` (their command refuses it, the retired legacy semantic
//!   capture), and their description says that they press nothing and names what does on an
//!   instance, `do` and `ensure` with capture.
//! - `lw1_a_finished_runs_answer_fits_with_its_state_outcome_and_the_rest` (item 2): the line of a
//!   run's job of 400 made-up ledger records (some 280 KB, over the 24 KiB budget, as the walk's
//!   106 KB to 514 KB were) is answered, by `ac_run_start` with `wait` and as the `job` of
//!   `ac_run_show`, within the budget: the receipt's state and outcome, the run status's state and
//!   terminal, `events_total` 400, `truncated` naming what was left out (`/events`, and the
//!   official OCR read where it does not fit beside them) and `rest` the calls that read it
//!   (`ac_run_evidence` with the request id, `ac_events` with the correlation id), both tools this
//!   server offers; an OCR read that fits stays. Fix round 1 (the review's (f)): the run's job is
//!   answered through the functions the server calls, `server::job_answer` (as `ac_run_start` with
//!   `wait` answers it) and `server::with_job` (as `ac_run_show` of the run answers it). Fix round 2
//!   (the second review's (f)): the job is made as `ac_run_start` makes it, by `jobs::Jobs::start`
//!   under the run's request id as a run's job, its body answering that line, and waited for
//!   (`jobs::Job::wait_ended`, no clock) until it ended.
//! - `lw1_every_schema_property_reaches_its_operation` (item 3): every property of every tool's
//!   input schema, each with every word its schema lists (else a made-up value of its row's type;
//!   fix round 1: a list parameter always as a list of two, each listed word in one of them),
//!   beside the row's required ones: the front's check takes it (`calls::prepare`, the members it
//!   adds `calls::front_members`); a core row read in this process holds it in its constant
//!   (`lab_ops::core_reads::READ_HERE`); a core row handed to a command of `lab-ops` writes it into
//!   the command line, whose parser refuses it as no `unknown` and no `wrong_type`, and as no
//!   `not_recognised` but for the identifiers [`IDENTIFIERS`] pins (`lab_ops::core_ops::
//!   handed_refusal`, the parse alone); a row a `serve` child or the rescue module serves is held to
//!   its receiver by the gates of Workflow #434 LAB-5B2, LAB-5B3 and LAB-5B1b/c. Before LW-1 this
//!   walk failed on `status`'s `sections` and on `events`' `type`, `event_type`,
//!   `exclude_event_types` and `origin_module`: the schema offered any text, the parser refused a
//!   word outside its list. Every entry of `tools::WORDS` lists its words in the schema. What the
//!   tools leave out of their rows' command lines is [`LEFT_OUT`], a list of its own (fix round 1):
//!   every row parameter a tool's schema does not offer, and every name outside the table that the
//!   command line of a row served in this process takes (`lab_ops::core_reads::EXCEPTIONS`) and its
//!   tool does not offer, each with its reason; a later parameter of either kind a tool leaves out
//!   fails until it is decided there.

use crate::budget;
use crate::calls::{self, Finished};
use crate::jobs;
use crate::protocol::{self, Era, LegacyVersion};
use crate::server;
use crate::tools::{self, Catalog, Listing, Serving};
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_lab_ops::cli_line;
use actingcommand_lab_ops::codes::ActingctlCode;
use actingcommand_lab_ops::core_ops;
use actingcommand_lab_ops::core_reads::{self, Written};
use actingcommand_lab_ops::locate::AuthoringModule;
use actingcommand_lab_ops::table::{
    self, Operation, OperationModule, OperationTable, Param, ParamForm, ParamType,
};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

/// The era these calls are answered in: 2025-11-25, with `structuredContent`.
const ERA: Era = Era::Legacy(LegacyVersion::V20251125);
const EVERY_MODULE: [OperationModule; 4] = [
    OperationModule::Core,
    OperationModule::Debugging,
    OperationModule::Authoring,
    OperationModule::Rescue,
];
/// A request id and a correlation id, as the Runtime spells them; made up.
const REQUEST: &str = "request_0123456789abcdef0123456789abcdef";
const CORRELATION: &str = "correlation_0123456789abcdef0123456789abcdef";
/// The state root and the install root the handed command lines name; nothing reads them.
const STATE: &str = "state";
const ROOT: &str = "root";

/// Item 3: the text parameters of the core rows handed to a command of `lab-ops` whose parser
/// reads an identifier of the Runtime's (a task, run, correlation or frame id) and refuses a
/// made-up word as `not_recognised`: the value is the caller's, from an earlier answer, and the
/// note says its form; no closed list gives it.
const IDENTIFIERS: [(&str, &str); 6] = [
    ("events", "run"),
    ("events", "task_id"),
    ("events", "correlation_id"),
    ("diagnose", "task"),
    ("evidence read", "artifact"),
    ("evidence read", "run"),
];

/// Item 1's list, pinned (the work order: "the reverse list as a pinned table, so that a later
/// slice that adds a command-line parameter must decide about the tool"), a list of its own (fix
/// round 1: written here, not taken from `tools::SUPPLIED`): what an offered row's tool leaves out
/// of the row's command line, and why (the manual, sections 1, 6 and 9;
/// `lab_ops::core_reads::EXCEPTIONS`): every parameter of the row its schema does not offer, and
/// every name outside the table that the command line of a row this server serves in its own
/// process takes and its schema does not offer (`*`: every such row). A rescue row's only where the
/// rescue module runs.
const LEFT_OUT: [(&str, &str, &str); 12] = [
    (
        "material",
        "out",
        "the server writes the material to its export directory with export",
    ),
    (
        "frame",
        "out",
        "the server writes the frame to its export directory",
    ),
    (
        "report export",
        "out",
        "the server writes the archive to its export directory",
    ),
    (
        "deploy",
        "detach",
        "through MCP it always runs detached: it replaces the server",
    ),
    (
        "rollback",
        "detach",
        "through MCP it always runs detached: it replaces the server",
    ),
    // #398 DB-2 fix round 1 (adapted: the row declares the command line's --root now, the tool
    // still leaves it out; the reason changes, not the entry).
    (
        "devices bind",
        "root",
        "the command line's own --root, a parameter of the row since #398 DB-2 fix round 1: the server hands the root it runs from",
    ),
    // #398 DB-1 (adapted: the work order leaves the row's channel out of its tool).
    (
        "devices bind",
        "bound_via",
        "its one channel ui is the installer's: a binding made through MCP keeps the channel the row writes without it",
    ),
    // #398 DB-2 (adapted: ruling 6 and the coordinator's addition leave both forms of the salt out
    // of the two rows' tools).
    (
        "devices",
        "key_salt",
        "a salt in a tool call stays in the client's transcript: through MCP a device key is made with the installation's own secret",
    ),
    (
        "devices",
        "key_salt_env",
        "a variable of the server's process is no caller's to name: through MCP a device key is made with the installation's own secret",
    ),
    (
        "devices bind",
        "key_salt",
        "a salt in a tool call stays in the client's transcript: through MCP a device key is found with the installation's own secret",
    ),
    (
        "devices bind",
        "key_salt_env",
        "a variable of the server's process is no caller's to name: through MCP a device key is found with the installation's own secret",
    ),
    (
        "*",
        "words",
        "no row declares it: the further words a row handed on before Workflow #434 LAB-5B4, refused by name",
    ),
];

fn embedded() -> OperationTable {
    table::operation_table().expect("the operation table this build embeds reads")
}

fn catalog_of(table: &OperationTable) -> Catalog {
    Catalog::of(table).expect("the table's tools are generated")
}

/// A place outside any installation root, where every module is listed.
fn unrooted() -> Result<AuthoringModule, Outcome> {
    Ok(AuthoringModule::Unrooted)
}

fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(members) => members,
        other => panic!("not a JSON object: {other}"),
    }
}

/// Every tool's definition, every module listed.
fn definitions(catalog: &Catalog) -> Vec<Value> {
    catalog.listed(&Listing::Named(EVERY_MODULE.to_vec()), &unrooted)
}

/// The definition of the tool `name`.
fn definition<'a>(every: &'a [Value], name: &str) -> &'a Value {
    every
        .iter()
        .find(|definition| definition["name"] == name)
        .unwrap_or_else(|| panic!("{name} is listed"))
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

#[test]
fn lw1_tap_target_and_navigate_tools_say_they_only_plan() {
    let table = embedded();
    let catalog = catalog_of(&table);
    let every = definitions(&catalog);
    for (tool, presses) in [
        ("ac_tap_target", "do with capture"),
        ("ac_navigate", "ensure with capture"),
    ] {
        let definition = definition(&every, tool);
        assert!(
            definition["inputSchema"]["properties"]
                .get("capture")
                .is_none(),
            "{tool}: no capture, which its command refuses: {definition}"
        );
        let description = definition["description"].as_str().unwrap_or_default();
        assert!(
            description.contains("saved frame")
                && description.contains("pressing nothing")
                && description.contains(presses),
            "{tool}: it says it plans on a saved frame only and names {presses:?}: {description}"
        );
    }
}

/// A made-up `run start` line of a run that went far: its receipt, `records` ledger records of
/// some 700 bytes each (the forensic projection carries payloads), and an official OCR read of
/// `ocr_bytes` when that is not zero.
fn run_start_line(records: usize, ocr_bytes: usize) -> Value {
    let events = (0..records)
        .map(|sequence| {
            json!({"sequence": sequence, "type": "task.step_finished", "payload": "x".repeat(600)})
        })
        .collect::<Vec<_>>();
    let mut line = json!({
        "receipt": {
            "request_id": REQUEST,
            "correlation_id": CORRELATION,
            "state": "completed",
            "result": {"task_result": "success", "executed_steps": 7, "final_page": "neutral.page"},
        },
        "events": events,
        "outcome": {"code": "task_run_reported", "category": "success"},
    });
    if ocr_bytes > 0 {
        line["official_ocr_projection"] = json!({"fields": "y".repeat(ocr_bytes)});
    }
    line
}

/// The made-up line of `run show` for that run: its state and its terminal.
fn run_show_line() -> cli_line::Exit {
    cli_line::success(
        object(json!({
            "schema_version": "actingcommand.run-status.v1",
            "request_id": REQUEST,
            "correlation_id": CORRELATION,
            "dispatch": "manual",
            "origin": "cli",
            "recovery_package_labels": [],
            "state": "succeeded",
            "terminal": {
                "outcome": "success",
                "final_page": "neutral.page",
                "executed_steps": 7,
                "sequence": 900,
                "event_id": "evt_0123456789abcdef0123456789abcdef",
            },
            "evidence": {"snapshot_ledger_position": 901, "restarted_after_admission": false},
        })),
        &Outcome::new(ActingctlCode::LabOperationCompleted),
    )
}

/// What the answer says it left out and which calls read it.
fn assert_left_out_and_rest(line: &Value, truncated: &[&str], catalog: &Catalog) {
    assert_eq!(
        line["events_total"],
        json!(400),
        "the records are counted: {line}"
    );
    assert!(
        line.get("events").is_none(),
        "the records are left out: {line}"
    );
    assert_eq!(
        line["truncated"],
        json!(truncated),
        "what was left out: {line}"
    );
    assert_eq!(
        line["rest"],
        json!([
            {"tool": "ac_run_evidence", "arguments": {"id": REQUEST}},
            {"tool": "ac_events", "arguments": {"correlation_id": CORRELATION, "detail": "payload"}},
        ]),
        "the calls that read it: {line}"
    );
    for call in line["rest"].as_array().into_iter().flatten() {
        let tool = call["tool"].as_str().unwrap_or_default();
        assert!(catalog.offers(tool), "{tool} is a tool of this server");
    }
}

#[test]
fn lw1_a_finished_runs_answer_fits_with_its_state_outcome_and_the_rest() {
    let table = embedded();
    let catalog = catalog_of(&table);
    // (the OCR read's size, whether it is kept, what is left out)
    for (ocr_bytes, kept, truncated) in [
        (2_000, true, &["/events"][..]),
        (30_000, false, &["/events", "/official_ocr_projection"][..]),
    ] {
        let ended = Finished {
            line: run_start_line(400, ocr_bytes),
            is_error: false,
        };
        assert!(
            calls::sent(&ended, ERA).is_err(),
            "setup: the run's whole line is over the budget"
        );
        // The run's job of this process, made as `ac_run_start` makes it (`server::run_start`):
        // started by `jobs::Jobs::start` under the run's request id, a run's job, its body
        // answering the run's line; waited for until it ended.
        let process_jobs = jobs::Jobs::new(
            jobs::Limits {
                running: 1,
                kept: 1,
            },
            0,
        );
        let line = ended.clone();
        let job = process_jobs
            .start(REQUEST.to_owned(), "ac_run_start", true, move || line)
            .expect("the run's job starts");
        job.wait_ended();
        assert!(
            process_jobs
                .find(REQUEST)
                .is_some_and(|kept| kept.run && kept.finished()),
            "setup: kept under the run's request id, a run's job, ended"
        );

        // `ac_run_start` with `wait`, the run ended within it: answered as the server answers it.
        let answered = server::job_answer(&job, false, ERA);
        assert!(
            answered.handle.is_none(),
            "the answer is the run's line, no handle: {}",
            answered.result
        );
        let result = answered.result;
        assert_eq!(result["isError"], json!(false), "{result}");
        assert!(
            protocol::complete(ERA, result.clone()).to_string().len() <= budget::RESULT_LIMIT,
            "within the limit"
        );
        let line = text_line(&result);
        assert_eq!(line["receipt"]["state"], "completed", "the state: {line}");
        assert_eq!(
            line["receipt"]["result"]["task_result"], "success",
            "the terminal outcome: {line}"
        );
        assert_eq!(line["receipt"]["result"]["executed_steps"], json!(7));
        assert_eq!(line["outcome"]["code"], "task_run_reported", "{line}");
        assert_eq!(
            line.get("official_ocr_projection").is_some(),
            kept,
            "the OCR read stays while it fits: {line}"
        );
        assert_left_out_and_rest(&line, truncated, &catalog);

        // `ac_run_show` of the run, its job of this process ended.
        let mut before = run_show_line();
        if let Ok(Value::Object(members)) = &mut before.line {
            members.insert(
                "job".to_owned(),
                json!({"kind": "ac_run_start", "job_phase": "done", "line": ended.line}),
            );
        }
        assert!(
            calls::sent(&calls::finished(&before), ERA).is_err(),
            "setup: before LW-1 the job's whole line made the answer of a finished run over the budget"
        );
        let shown = server::with_job(run_show_line(), &job, ERA);
        let result = calls::call_result(&shown, ERA);
        assert_eq!(result["isError"], json!(false), "{result}");
        assert!(
            protocol::complete(ERA, result.clone()).to_string().len() <= budget::RESULT_LIMIT,
            "within the limit"
        );
        let line = text_line(&result);
        assert_eq!(line["state"], "succeeded", "the run's state: {line}");
        assert_eq!(
            line["terminal"]["outcome"], "success",
            "its terminal outcome: {line}"
        );
        assert_eq!(line["terminal"]["executed_steps"], json!(7), "its count");
        assert_eq!(line["job"]["kind"], "ac_run_start", "{line}");
        assert_eq!(line["job"]["job_phase"], "done", "{line}");
        assert_eq!(
            line["job"]["line"]["receipt"]["state"], "completed",
            "{line}"
        );
        assert_eq!(
            line["job"]["line"].get("official_ocr_projection").is_some(),
            kept,
            "{line}"
        );
        assert_left_out_and_rest(&line["job"]["line"], truncated, &catalog);
    }
}

/// The values the walk gives the property `property` of `param`'s row: each word its schema lists
/// (fix round 1: a list's items two at a time, each word with the one after it, the last with the
/// first, so that every word is sent and every list has two items), else one made-up value of the
/// parameter's type (a list of two for a list parameter); `absolute` an absolute path, which every
/// path parameter takes through MCP.
fn samples(param: &Param, property: &Value, absolute: &str) -> Vec<Value> {
    let listed = property["type"] == "array";
    let words = if listed {
        &property["items"]["enum"]
    } else {
        &property["enum"]
    };
    if let Some(words) = words.as_array() {
        if !listed {
            return words.clone();
        }
        return words
            .iter()
            .zip(words.iter().cycle().skip(1))
            .map(|(word, next)| {
                if word == next {
                    json!([word])
                } else {
                    json!([word, next])
                }
            })
            .collect();
    }
    let number = || {
        property
            .get("default")
            .or_else(|| property.get("minimum"))
            .filter(|value| value.is_i64() || value.is_u64())
            .cloned()
            .unwrap_or_else(|| json!(1))
    };
    let one = match (param.form, &param.value_type) {
        (ParamForm::Switch, _) | (_, ParamType::Boolean) => json!(true),
        (_, ParamType::Instance) => json!("neutral.a"),
        (_, ParamType::Integer | ParamType::Unsigned) => number(),
        (_, ParamType::Decimal) => json!(1),
        (_, ParamType::Duration) => json!("1s"),
        (_, ParamType::ListOfStrings) => json!(["neutral.a", "neutral.b"]),
        (_, ParamType::RequestId | ParamType::RunRef) => json!(REQUEST),
        (_, ParamType::Code) => json!("actingctl_usage_invalid"),
        (_, ParamType::Json) => json!([]),
        (_, ParamType::KeyValue) => json!("neutral=1"),
        (_, ParamType::OperationId) => json!("op-1"),
        (_, ParamType::Path | ParamType::File | ParamType::Dir) => json!(absolute),
        _ => json!("neutral"),
    };
    vec![if param.repeat { json!([one]) } else { one }]
}

/// The refusal the parser of a core row's handed command line gives `params`, as an outcome value;
/// `Err` with what kept the line from being built.
fn handed_refusal(
    operation: &Operation,
    params: &Map<String, Value>,
) -> Result<Option<Value>, String> {
    let argv = match core_ops::offline_words(operation, params, Some(Path::new(ROOT))) {
        Some(argv) => argv,
        None => {
            match core_ops::handed_words(operation, params, Some(Path::new(ROOT)), Path::new(STATE))
            {
                Some(Ok(argv)) => argv,
                Some(Err(exit)) => return Err(format!("its words are refused: {:?}", exit.line)),
                None => return Err("it hands no words".to_owned()),
            }
        }
    };
    let inline = core_reads::HANDED
        .iter()
        .chain(&core_reads::OFFLINE)
        .find(|handed| handed.row == operation.name)
        .is_some_and(|handed| {
            handed
                .params()
                .any(|(name, written)| written == Written::Beside && params.contains_key(name))
        });
    Ok(core_ops::handed_refusal(&argv, inline)
        .map(|refusal| cli_line::outcome_value(&refusal, Detail::Full)))
}

#[test]
fn lw1_every_schema_property_reaches_its_operation() {
    let table = embedded();
    let catalog = catalog_of(&table);
    let every = definitions(&catalog);
    let exports = tempfile::tempdir().expect("tempdir");
    let absolute = exports
        .path()
        .join("neutral")
        .to_str()
        .expect("the temporary folder is UTF-8")
        .to_owned();
    let budget_s = Duration::from_secs(tools::DEFAULT_CALL_BUDGET_S);
    let mut failures = Vec::new();
    let mut walked = 0_usize;
    let mut left_out = BTreeSet::new();
    for definition in &every {
        let tool = definition["name"].as_str().unwrap_or_default();
        let operation = catalog
            .operation(tool)
            .unwrap_or_else(|| panic!("{tool} is an offered row's tool"));
        let row = operation.name.as_str();
        let properties = definition["inputSchema"]["properties"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        for param in &operation.params {
            if !properties.contains_key(&param.name) {
                left_out.insert((row.to_owned(), param.name.clone()));
            }
        }
        // The row's required members, each its first value.
        let base = operation
            .params
            .iter()
            .filter(|param| tools::required_here(param) && properties.contains_key(&param.name))
            .map(|param| {
                let first = samples(param, &properties[param.name.as_str()], &absolute)
                    .into_iter()
                    .next()
                    .unwrap_or(Value::Null);
                (param.name.clone(), first)
            })
            .collect::<Map<String, Value>>();
        let core_handed = tools::serving(operation) == Serving::Core
            && core_reads::READ_HERE.iter().all(|reads| reads.row != row);
        let base_refusal = if core_handed {
            match calls::prepare(operation, &base, exports.path()) {
                Ok(params) => handed_refusal(operation, &params).ok().flatten(),
                Err(refused) => {
                    failures.push(format!(
                        "{tool}: its required members are refused: {}",
                        refused.outcome
                    ));
                    continue;
                }
            }
        } else {
            None
        };
        for (member, property) in &properties {
            walked += 1;
            // The members the front adds and reads itself.
            if tools::takes_export(row) && member == tools::EXPORT {
                let mut arguments = base.clone();
                arguments.insert(member.clone(), json!(true));
                if let Err(refused) = calls::prepare(operation, &arguments, exports.path()) {
                    failures.push(format!("{tool}: {member}: refused {}", refused.outcome));
                }
                continue;
            }
            if tools::ADDED
                .iter()
                .any(|(of, name, _)| *of == row && *name == member.as_str())
            {
                let given = property
                    .get("default")
                    .cloned()
                    .unwrap_or_else(|| json!("neutral"));
                let mut arguments = Map::new();
                arguments.insert(member.clone(), given);
                match calls::front_members(operation, arguments, budget_s) {
                    Ok((_, rest)) if rest.is_empty() => {}
                    Ok((_, rest)) => failures.push(format!("{tool}: {member}: left {rest:?}")),
                    Err(refused) => {
                        failures.push(format!("{tool}: {member}: refused {}", refused.outcome));
                    }
                }
                continue;
            }
            let Some(param) = operation.params.iter().find(|param| &param.name == member) else {
                failures.push(format!(
                    "{tool}: {member}: a property no parameter of {row} declares"
                ));
                continue;
            };
            let closed = tools::words(row, member).expect("the closed lists read");
            for value in samples(param, property, &absolute) {
                let mut arguments = base.clone();
                arguments.insert(member.clone(), value.clone());
                let params = match calls::prepare(operation, &arguments, exports.path()) {
                    Ok(params) => params,
                    Err(refused) => {
                        failures.push(format!(
                            "{tool}: {member} {value}: the front refuses a value its schema allows: {}",
                            refused.outcome
                        ));
                        continue;
                    }
                };
                match tools::serving(operation) {
                    Serving::Core => {}
                    // The receivers of these rows are held to the row by the gates of Workflow #434
                    // LAB-5B2 and LAB-5B3 (a `serve` child's own commands) and LAB-5B1b/c (the
                    // rescue module), every declared parameter in both directions.
                    Serving::Child | Serving::Rescue => continue,
                    Serving::Refused => {
                        failures.push(format!("{tool}: an offered row this build refuses"));
                        continue;
                    }
                }
                if let Some(reads) = core_reads::READ_HERE.iter().find(|reads| reads.row == row) {
                    if !reads.names.contains(&member.as_str()) {
                        failures.push(format!(
                            "{tool}: {member}: its row's constant does not read it"
                        ));
                    }
                    continue;
                }
                let refusal = match handed_refusal(operation, &params) {
                    Ok(refusal) => refusal,
                    Err(why) => {
                        failures.push(format!("{tool}: {member} {value}: {why}"));
                        continue;
                    }
                };
                let Some(refusal) =
                    refusal.filter(|refusal| Some(refusal) != base_refusal.as_ref())
                else {
                    continue;
                };
                let problem = refusal["values"]["argument_problem"]
                    .as_str()
                    .unwrap_or_default();
                let identifier = closed.is_none() && IDENTIFIERS.contains(&(row, member.as_str()));
                let refused = match problem {
                    "unknown" | "wrong_type" => true,
                    "not_recognised" => !identifier,
                    _ => false,
                };
                if refused {
                    failures.push(format!(
                        "{tool}: {member} {value}: a value its schema allows, its operation's parser refuses: {refusal}"
                    ));
                }
            }
        }
    }
    assert!(
        walked > 200,
        "setup: the walk read every tool's properties ({walked})"
    );

    // Every closed list is a property of an offered tool that lists its words.
    for (row, param, _) in tools::WORDS {
        let operation = table
            .operation(row)
            .unwrap_or_else(|| panic!("the table has no row {row}"));
        let property =
            &definition(&every, &tools::tool_name(operation))["inputSchema"]["properties"][param];
        let listed = property
            .get("enum")
            .or_else(|| property["items"].get("enum"))
            .and_then(Value::as_array)
            .is_some_and(|words| !words.is_empty());
        if !listed {
            failures.push(format!(
                "{row} {param}: its schema lists no words: {property}"
            ));
        }
    }
    // Every identifier pinned is a declared text parameter of a core row handed to `lab-ops`.
    for (row, param) in IDENTIFIERS {
        let declared = table.operation(row).is_some_and(|operation| {
            core_reads::HANDED.iter().any(|handed| handed.row == row)
                && operation.params.iter().any(|declared| {
                    declared.name == param && declared.value_type == ParamType::String
                })
        });
        if !declared {
            failures.push(format!(
                "{row} {param}: pinned as an identifier, no such parameter"
            ));
        }
    }
    // Fix round 1: the names outside the table that the command line of a row served in this
    // process takes (`core_reads::EXCEPTIONS`), each one its tool does not offer; an exception of
    // every such row as `*`.
    for exception in &core_reads::EXCEPTIONS {
        let rows = every
            .iter()
            .filter_map(|definition| {
                let operation = catalog.operation(definition["name"].as_str()?)?;
                let applies = tools::serving(operation) == Serving::Core
                    && (exception.rows.is_empty()
                        || exception.rows.contains(&operation.name.as_str()));
                let offered = definition["inputSchema"]["properties"]
                    .get(exception.name)
                    .is_some();
                (applies && !offered).then(|| operation.name.clone())
            })
            .collect::<Vec<_>>();
        if exception.rows.is_empty() && !rows.is_empty() {
            left_out.insert(("*".to_owned(), exception.name.to_owned()));
        } else {
            for row in rows {
                left_out.insert((row, exception.name.to_owned()));
            }
        }
    }
    // The pinned list of what the tools leave out (item 1's reverse list).
    let pinned = LEFT_OUT
        .iter()
        .filter(|(row, _, _)| {
            *row == "*"
                || table
                    .operation(row)
                    .is_some_and(|operation| catalog.offers(&tools::tool_name(operation)))
        })
        .map(|(row, param, why)| {
            assert!(!why.is_empty(), "{row} {param}: a reason");
            ((*row).to_owned(), (*param).to_owned())
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        left_out, pinned,
        "the row parameters the tools leave out are the pinned ones: a new one is decided there"
    );
    assert!(
        failures.is_empty(),
        "schema properties their operation does not take:\n{}",
        failures.join("\n")
    );
}
