// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #434 LAB-5G2b (register §1.287, "Slice LAB-5G2b" and its second part;
//! contracts/lab-two-entries.md §9.3; the coordinator's work orders `rd-1112/brief-lab5g2b.md`,
//! points 3, 4 and 9, and `rd-1112/brief-lab5g2b2.md`, points 1 and 2): what this build
//! serves, the front's own members and the answer budget, in the server's own code. No Runtime, no
//! clock, no sleep: every call here is refused or served before anything reaches a Runtime, or is
//! a row that reads none; exports go to a temporary folder.
//!
//! - `mcp_served_and_refused_partition`: every offered row is served or refused by the one rule
//!   (`tools::serving`), stated here from the table and the rescue module's list (adapted by the
//!   slice's second part, whose rule it pins): served are the rows `core_ops` serves and, on
//!   Windows, the rescue rows the rescue module serves (its `mcp config`, `mcp entries` and `mcp
//!   plan` among them; Workflow #434 LAB-6S0, adapted: `deploy` and `rollback` too, whose run the
//!   front forces detached and whose detached run carries `Lab-MCP`, where they were refused
//!   until it did); refused is a rescue row the module does not serve, which is not offered
//!   (Workflow #434 LAB-5G4b, adapted: every other row, of no module this process serves itself,
//!   is served by a `serve` child: the three `app` rows, `standard-pack check`, the debugging rows
//!   actinglab's own commands serve and every authoring row, `record draft` among them; this test
//!   starts no child, and no detached run, as each call's arguments are refused first). Every row
//!   the module serves is served. A call of a refused row would answer
//!   `mcp_operation_unsupported` with its tool, its arguments unread; a call of a served row reads
//!   its arguments first (an undeclared one is `arguments_invalid`, `unknown`, before the code
//!   that serves it runs).
//! - `mcp_front_members_in_one_place`: the front's members are one table; `material` has
//!   `export` and no `out`, `frame` no `out`, `deploy` and `rollback` (Windows) no `detach`;
//!   `material` without `export`, or with it false, is handed on as given, with it true with
//!   `out` the export directory; `frame` always with it; `deploy` and `rollback` with `detach`
//!   true; `out`, `detach` and an `export` that is no boolean are refused.
//! - `mcp_budget_cut_and_export`: a shaped row that fits given whole (`codes`, a page of the most
//!   entries, which today's budget takes), a shaped row cut (`schedule`'s answer with more
//!   candidate rows than the budget takes: fewer kept, `candidates_truncated`, within 24 KiB; the
//!   coordinator's correction of 11-12: the first round expected the `codes` page to be cut, and
//!   it fits), a shaped row exported (`facts`' answer within this server's budget: the header,
//!   `answer_exported` naming the file, the file the whole answer), and a line without shaping
//!   over the limit answered `output_budget_exceeded` with the bytes it takes and the limit, in
//!   each protocol era.

use crate::budget;
use crate::calls::{self, Service};
use crate::protocol::{self, Era, LegacyVersion};
use crate::tools::{self, Catalog, Listing, Serving, Supplied};
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_lab_ops::budget as lab_budget;
use actingcommand_lab_ops::cli_line;
use actingcommand_lab_ops::code_lookup;
use actingcommand_lab_ops::codes::ActingctlCode;
use actingcommand_lab_ops::core_ops::{self, Caller, Place};
use actingcommand_lab_ops::events::CursorBinding;
use actingcommand_lab_ops::facts;
use actingcommand_lab_ops::front::{Connection, Front};
use actingcommand_lab_ops::locate::AuthoringModule;
use actingcommand_lab_ops::schedule;
use actingcommand_lab_ops::table::{self, Operation, OperationModule, OperationTable};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// The era most of these calls are answered in: 2025-11-25, with `structuredContent`.
const ERA: Era = Era::Legacy(LegacyVersion::V20251125);
const EVERY_MODULE: [OperationModule; 4] = [
    OperationModule::Core,
    OperationModule::Debugging,
    OperationModule::Authoring,
    OperationModule::Rescue,
];

fn embedded() -> OperationTable {
    table::operation_table().expect("the operation table this build embeds reads")
}

fn catalog_of(table: &OperationTable) -> Catalog {
    Catalog::of(table).expect("the table's tools are generated")
}

fn row<'a>(table: &'a OperationTable, name: &str) -> &'a Operation {
    table
        .operation(name)
        .unwrap_or_else(|| panic!("the table has no row {name}"))
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

/// The line of a call of `operation`'s tool `tool` with `arguments`, as the service answers it.
fn line_of(service: &Service, operation: &Operation, tool: &str, arguments: Value) -> Value {
    service
        .line(operation, tool, &object(arguments), ERA)
        .line
        .expect("the line encodes")
}

/// The line a `tools/call` result carries as its text, which its `structuredContent` repeats.
fn text_line(result: &Value) -> Value {
    let text = result["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("a result without its text: {result}"));
    let line = serde_json::from_str::<Value>(text)
        .unwrap_or_else(|error| panic!("the text is no JSON ({error}): {result}"));
    if let Some(structured) = result.get("structuredContent") {
        assert_eq!(structured, &line, "structuredContent repeats the line");
    }
    line
}

/// Workflow #434 LAB-5G2b, second part: the classes of the partition, by the rule; the register
/// states how many rows each holds. Workflow #434 LAB-5G4b (adapted, its work order point 1): the
/// rows LAB-5G2b refused because actinglab's own commands serve them, the authoring rows among
/// them, are one class, served by a `serve` child. Workflow #434 LAB-6S0 (adapted): the class of
/// the rows refused because the front forces their run detached is gone; they are served by the
/// rescue module.
const SERVED_CORE: &str = "served by lab_ops::core_ops";
const SERVED_RESCUE: &str = "served by the rescue module";
const SERVED_CHILD: &str = "served by a serve child";
const REFUSED_UNSERVED: &str = "refused: a rescue row no code serves";

/// Whether the rescue module serves the rescue row `name`, by its own list, on Windows, where it
/// runs.
#[cfg(windows)]
fn rescue_module_serves(name: &str) -> bool {
    actingcommand_lab_rescue::rescue::SERVED.contains(&name)
}

/// Elsewhere it serves none.
#[cfg(not(windows))]
fn rescue_module_serves(_name: &str) -> bool {
    false
}

/// How many rows the rescue module's own list holds, on Windows; none elsewhere.
#[cfg(windows)]
fn rescue_module_rows() -> usize {
    actingcommand_lab_rescue::rescue::SERVED.len()
}

/// Elsewhere it serves none.
#[cfg(not(windows))]
fn rescue_module_rows() -> usize {
    0
}

#[test]
fn mcp_served_and_refused_partition() {
    let table = embedded();
    let catalog = catalog_of(&table);
    let exports = tempfile::tempdir().expect("tempdir");
    let service = Service::new(exports.path().to_path_buf());
    let mut classes: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for operation in &table.operations {
        let tool = tools::tool_name(operation);
        if !catalog.offers(&tool) {
            continue;
        }
        // The rule, stated from the table and the rescue module's list (work order points 1 and 2
        // of the second part; Workflow #434 LAB-5G4b, point 1: a row of no module this process
        // serves itself goes to a child; Workflow #434 LAB-6S0: a row whose run the front forces
        // detached is served as every other row the module serves).
        let (expected, class) = if core_ops::serves(operation) {
            (Serving::Core, SERVED_CORE)
        } else if operation.module == OperationModule::Rescue {
            if rescue_module_serves(&operation.name) {
                (Serving::Rescue, SERVED_RESCUE)
            } else {
                (Serving::Refused, REFUSED_UNSERVED)
            }
        } else {
            (Serving::Child, SERVED_CHILD)
        };
        assert_eq!(
            tools::serving(operation),
            expected,
            "{}: {class}",
            operation.name
        );
        let line = line_of(
            &service,
            operation,
            &tool,
            json!({"neutral_undeclared": true}),
        );
        let outcome = &line["outcome"];
        if expected == Serving::Refused {
            assert_eq!(
                (&outcome["code"], &outcome["values"]["mcp_tool"]),
                (&json!("mcp_operation_unsupported"), &json!(tool)),
                "{}: refused, its arguments unread: {line}",
                operation.name
            );
        } else {
            assert_eq!(
                (
                    &outcome["code"],
                    &outcome["values"]["argument"],
                    &outcome["values"]["argument_problem"],
                ),
                (
                    &json!("arguments_invalid"),
                    &json!("neutral_undeclared"),
                    &json!("unknown"),
                ),
                "{}: served, so its arguments are read, and an undeclared one is refused before \
                 the code that serves it runs: {line}",
                operation.name
            );
        }
        classes
            .entry(class)
            .or_default()
            .push(operation.name.clone());
    }
    let count = |class: &str| classes.get(class).map_or(0, Vec::len);
    let holds = |class: &str, name: &str| {
        classes
            .get(class)
            .is_some_and(|names| names.iter().any(|row| row == name))
    };
    let offered = catalog
        .listed(&Listing::Named(EVERY_MODULE.to_vec()), &unrooted)
        .len();
    assert_eq!(
        classes.values().map(Vec::len).sum::<usize>(),
        offered,
        "every offered row in one class: {classes:?}"
    );
    for (class, name) in [
        (SERVED_CHILD, "app launch"),
        (SERVED_CHILD, "standard-pack check"),
        (SERVED_CHILD, "watch"),
        (SERVED_CHILD, "record draft"),
        (SERVED_CORE, "status"),
    ] {
        assert!(holds(class, name), "setup: {name} is {class}: {classes:?}");
    }
    // Every row the rescue module serves is offered, and served (Workflow #434 LAB-6S0, adapted:
    // those whose run the front forces detached too).
    assert_eq!(
        count(SERVED_RESCUE),
        rescue_module_rows(),
        "the rescue module's rows: {classes:?}"
    );
    if cfg!(windows) {
        for name in [
            "leftovers",
            "up",
            "mcp config",
            "mcp entries",
            "mcp plan",
            "deploy",
            "rollback",
        ] {
            assert!(
                holds(SERVED_RESCUE, name),
                "setup: {name} is served on Windows: {classes:?}"
            );
        }
    }
}

#[test]
fn mcp_front_members_in_one_place() {
    assert_eq!(
        tools::SUPPLIED,
        [
            ("material", "out", Supplied::ExportDirectoryOnExport),
            ("frame", "out", Supplied::ExportDirectory),
            ("deploy", "detach", Supplied::True),
            ("rollback", "detach", Supplied::True),
        ],
        "the front's own members, in one place"
    );
    assert_eq!(tools::EXPORT, "export");

    // The schemas.
    let table = embedded();
    let catalog = catalog_of(&table);
    let every = catalog
        .listed(&Listing::Named(EVERY_MODULE.to_vec()), &unrooted)
        .into_iter()
        .map(|definition| {
            let name = definition["name"].as_str().unwrap_or_default().to_owned();
            (name, definition)
        })
        .collect::<BTreeMap<_, _>>();
    let properties = |tool: &str| {
        every
            .get(tool)
            .unwrap_or_else(|| panic!("{tool} is listed"))["inputSchema"]["properties"]
            .clone()
    };
    let material = properties("ac_material");
    assert!(
        material.get("out").is_none(),
        "material: no out: {material}"
    );
    assert_eq!(material["export"]["type"], "boolean", "material: export");
    assert!(
        material.get("ref").is_some(),
        "material: its parameters stay"
    );
    let frame = properties("ac_frame");
    assert!(frame.get("out").is_none(), "frame: no out: {frame}");
    assert!(frame.get("export").is_none(), "frame: no export: {frame}");
    if cfg!(windows) {
        for tool in ["ac_deploy", "ac_rollback"] {
            let detached = properties(tool);
            assert!(
                detached.get("detach").is_none() && detached.get("plan").is_some(),
                "{tool}: no detach: {detached}"
            );
        }
    }

    // What the serving code is given.
    let exports = tempfile::tempdir().expect("tempdir");
    let directory = exports
        .path()
        .to_str()
        .expect("the temporary folder is UTF-8")
        .to_owned();
    let prepared = |name: &str, arguments: Value| {
        calls::prepare(row(&table, name), &object(arguments), exports.path())
    };
    let given = |name: &str, arguments: Value| {
        prepared(name, arguments)
            .unwrap_or_else(|refused| panic!("{name}: refused {}", refused.outcome))
    };
    let problem = |name: &str, arguments: Value| {
        let refused = prepared(name, arguments).expect_err("refused");
        let outcome = cli_line::outcome_value(&refused.outcome, Detail::Full);
        assert_eq!(outcome["code"], "arguments_invalid", "{name}: {outcome}");
        (
            outcome["values"]["argument"].clone(),
            outcome["values"]["argument_problem"].clone(),
        )
    };
    assert_eq!(
        given("material", json!({"ref": "neutral-ref"})),
        object(json!({"ref": "neutral-ref"})),
        "material without export: its text is read"
    );
    assert_eq!(
        given("material", json!({"ref": "neutral-ref", "export": false})),
        object(json!({"ref": "neutral-ref"})),
        "material with export false"
    );
    assert_eq!(
        given("material", json!({"ref": "neutral-ref", "export": true})),
        object(json!({"ref": "neutral-ref", "out": directory})),
        "material with export: written whole to the export directory"
    );
    assert_eq!(
        problem("material", json!({"ref": "neutral-ref", "export": "yes"})),
        (json!("export"), json!("wrong_type"))
    );
    assert_eq!(
        problem("material", json!({"ref": "neutral-ref", "out": directory})),
        (json!("out"), json!("unknown"))
    );
    assert_eq!(
        given("frame", json!({"instance": "neutral.frame"})),
        object(json!({"instance": "neutral.frame", "out": directory})),
        "frame: always written to the export directory"
    );
    assert_eq!(
        problem(
            "frame",
            json!({"instance": "neutral.frame", "out": directory})
        ),
        (json!("out"), json!("unknown"))
    );
    if cfg!(windows) {
        assert_eq!(
            given("deploy", json!({"target": "v0.0.0"})),
            object(json!({"target": "v0.0.0", "detach": true})),
            "deploy: detached"
        );
        assert_eq!(
            given("rollback", json!({})),
            object(json!({"detach": true})),
            "rollback: detached"
        );
        assert_eq!(
            problem("deploy", json!({"target": "v0.0.0", "detach": false})),
            (json!("detach"), json!("unknown"))
        );
    }
}

/// The caller the command line gives the serving code for an offline row: no budget.
fn without_budget(place: &dyn Fn() -> Place) -> Caller<'_> {
    Caller {
        place,
        unlocated: "",
        connection: Connection::Connect(Front::Mcp),
        budget: None,
        cursor: CursorBinding::fresh(),
    }
}

#[test]
fn mcp_budget_cut_and_export() {
    let table = embedded();
    let exports = tempfile::tempdir().expect("tempdir");
    let service = Service::new(exports.path().to_path_buf());

    // A shaped row that fits is given whole: `codes`, a page of the most entries (the page's limit
    // is sized to today's budget), reads no Runtime.
    let codes = row(&table, "codes");
    let limit = code_lookup::MAX_LIMIT;
    let arguments = object(json!({"limit": limit}));
    let unplaced = || Place {
        state_root: Err(Outcome::new(ActingctlCode::InstallStateRootUnresolved)),
        root: None,
    };
    let whole = core_ops::answer(codes, &arguments, &without_budget(&unplaced))
        .line
        .expect("the line encodes");
    let page = usize::try_from(limit).expect("a page's size");
    assert_eq!(
        whole["codes"].as_array().map(Vec::len),
        Some(page),
        "setup: the command line's page holds every entry asked for"
    );
    let result = service.call(codes, "ac_codes", &arguments, ERA);
    assert_eq!(result["isError"], json!(false), "{result}");
    let given = text_line(&result);
    assert_eq!(
        given["codes"].as_array().map(Vec::len),
        Some(page),
        "a page that fits is given whole"
    );
    assert!(given.get("truncated").is_none(), "nothing is cut: {given}");
    assert!(
        protocol::complete(ERA, result.clone()).to_string().len() <= budget::RESULT_LIMIT,
        "within the limit"
    );

    // A shaped row cut: `schedule`'s answer with more candidate rows than this server's budget
    // takes (the cut that row's serving code makes of an explanation, `schedule::within_budget`).
    let rows = 400;
    let candidates = (0..rows)
        .map(|index| json!({"task_id": format!("neutral.task.{index}"), "detail": "x".repeat(64)}))
        .collect::<Vec<_>>();
    let explained = json!({
        "evaluated_at_unix_ms": 1,
        "catalog_hash": "sha256:0",
        "instances": [{
            "instance_alias": "neutral",
            "candidates": candidates,
            "candidates_total": rows,
        }],
    });
    let cut = service.within_budget(ERA, |answer_budget| {
        schedule::within_budget(explained, Some(answer_budget))
    });
    let kept = cut["instances"][0]["candidates"]
        .as_array()
        .map_or(0, Vec::len);
    assert!(
        (1..rows).contains(&kept),
        "the candidate rows are cut to the budget: {kept} of {rows}"
    );
    assert_eq!(
        cut["instances"][0]["candidates_truncated"],
        json!(true),
        "the cut is marked"
    );
    assert_eq!(
        cut["instances"][0]["candidates_total"],
        json!(rows),
        "the total stays"
    );
    let exit = cli_line::success(
        object(cut),
        &Outcome::new(ActingctlCode::LabOperationCompleted),
    );
    let result = calls::call_result(&exit, ERA);
    assert_eq!(result["isError"], json!(false), "{result}");
    assert!(
        protocol::complete(ERA, result.clone()).to_string().len() <= budget::RESULT_LIMIT,
        "within the limit"
    );

    // A shaped row exported: `facts`' answer within this server's budget.
    let records = (0..400)
        .map(|index| json!({"key": format!("neutral.record.{index}"), "value": "x".repeat(64)}))
        .collect::<Vec<_>>();
    let answer = json!({
        "scope": "program",
        "read_at_unix_ms": 1,
        "records_total": 400,
        "records_kept": 400,
        "records": records,
    });
    let fitted = service
        .within_budget(ERA, |answer_budget| {
            facts::within_budget(answer.clone(), Some(answer_budget))
        })
        .expect("the answer is written whole");
    let exit = cli_line::success(
        lab_budget::line_members(fitted, "result", Vec::new()),
        &Outcome::new(ActingctlCode::RuntimeFactsReported),
    );
    let result = calls::call_result(&exit, ERA);
    let line = text_line(&result);
    let warning = &line["warnings"][0]["outcome"];
    assert_eq!(warning["code"], "answer_exported", "{line}");
    let path = warning["values"]["path"]
        .as_str()
        .unwrap_or_else(|| panic!("answer_exported names its file: {line}"));
    assert_eq!(line["export"]["path"], json!(path), "the header's export");
    assert_eq!(line["records_total"], json!(400), "the header");
    assert!(line.get("records").is_none(), "the header only: {line}");
    assert!(
        Path::new(path).starts_with(exports.path()),
        "written to the export directory: {path}"
    );
    let written = serde_json::from_slice::<Value>(&fs::read(path).expect("read the export"))
        .expect("the export is JSON");
    assert_eq!(written, answer, "the file holds the whole answer");

    // A row without answer shaping whose line does not fit: never cut, never sent over the limit.
    let mut members = Map::new();
    members.insert(
        "text".to_owned(),
        Value::String("x".repeat(budget::RESULT_LIMIT)),
    );
    let exit = cli_line::success(members, &Outcome::new(ActingctlCode::LabOperationCompleted));
    for era in [
        Era::Legacy(LegacyVersion::V20250326),
        Era::Legacy(LegacyVersion::V20251125),
        Era::Modern,
    ] {
        let result = calls::call_result(&exit, era);
        assert_eq!(result["isError"], json!(true), "{era:?}");
        let line = text_line(&result);
        let outcome = &line["outcome"];
        assert_eq!(outcome["code"], "output_budget_exceeded", "{era:?}: {line}");
        assert_eq!(
            outcome["values"]["limit_bytes"],
            json!(budget::RESULT_LIMIT),
            "{era:?}: the limit"
        );
        let actual = outcome["values"]["actual_bytes"].as_u64().unwrap_or(0);
        assert!(
            usize::try_from(actual).is_ok_and(|actual| actual > budget::RESULT_LIMIT),
            "{era:?}: the bytes it takes: {line}"
        );
    }
}
