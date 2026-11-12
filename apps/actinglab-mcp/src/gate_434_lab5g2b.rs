// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #434 LAB-5G2b (register §1.287, "Slice LAB-5G2b"; contracts/lab-two-entries.md §9.3;
//! the coordinator's work order `rd-1112/brief-lab5g2b.md`, points 3, 4 and 9): what this build
//! serves, the front's own members and the answer budget, in the server's own code. No Runtime, no
//! clock, no sleep: every call here is refused or served before anything reaches a Runtime, or is
//! a row that reads none; exports go to a temporary folder.
//!
//! - `mcp_served_and_refused_partition`: every offered row is served or refused by the one rule
//!   (`tools::serving`), stated here from the table: refused are the rows of slice LAB-5 (`record
//!   draft`; on Windows the rescue module's `mcp config` and `mcp entries`), the core and
//!   debugging rows `lab_ops::core_ops` does not serve (the three `app` rows, `standard-pack
//!   check`, the debugging rows actinglab's own commands serve), every authoring row and (until
//!   the slice's second part) every rescue row; served are the rows `core_ops` serves. A call of a
//!   refused row answers `mcp_operation_unsupported` with its tool, its arguments unread; a call of
//!   a served row reads its arguments (an undeclared one is `arguments_invalid`, `unknown`).
//! - `mcp_front_members_in_one_place`: the front's members are one table; `material` has
//!   `export` and no `out`, `frame` no `out`, `deploy` and `rollback` (Windows) no `detach`;
//!   `material` without `export`, or with it false, is handed on as given, with it true with
//!   `out` the export directory; `frame` always with it; `deploy` and `rollback` with `detach`
//!   true; `out`, `detach` and an `export` that is no boolean are refused.
//! - `mcp_budget_cut_and_export`: a shaped row cut (`codes`, a page of the most entries: fewer
//!   kept, `truncated`, within 24 KiB), a shaped row exported (`facts`' answer within this
//!   server's budget: the header, `answer_exported` naming the file, the file the whole answer),
//!   and a line without shaping over the limit answered `output_budget_exceeded` with the bytes
//!   it takes and the limit, in each protocol era.

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

#[test]
fn mcp_served_and_refused_partition() {
    let table = embedded();
    let catalog = catalog_of(&table);
    let exports = tempfile::tempdir().expect("tempdir");
    let service = Service::new(exports.path().to_path_buf());
    let mut refused = Vec::new();
    let mut served = 0;
    for operation in &table.operations {
        let tool = tools::tool_name(operation);
        if !catalog.offers(&tool) {
            continue;
        }
        // The rule, stated from the table (work order point 9).
        let stays_refused = operation.slice == "LAB-5"
            || (matches!(
                operation.module,
                OperationModule::Core | OperationModule::Debugging
            ) && !core_ops::serves(operation))
            || matches!(
                operation.module,
                OperationModule::Authoring | OperationModule::Rescue
            );
        let serving = tools::serving(operation);
        assert_eq!(
            serving == Serving::Refused,
            stays_refused,
            "{}: served or refused by the rule",
            operation.name
        );
        let line = line_of(
            &service,
            operation,
            &tool,
            json!({"neutral_undeclared": true}),
        );
        let outcome = &line["outcome"];
        if stays_refused {
            assert_eq!(
                (&outcome["code"], &outcome["values"]["mcp_tool"]),
                (&json!("mcp_operation_unsupported"), &json!(tool)),
                "{}: refused, its arguments unread: {line}",
                operation.name
            );
            refused.push(operation.name.clone());
            continue;
        }
        assert!(core_ops::serves(operation), "{}", operation.name);
        assert_eq!(serving, Serving::Core, "{}", operation.name);
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
            "{}: served, so its arguments are read, and an undeclared one is refused: {line}",
            operation.name
        );
        served += 1;
    }
    for name in ["app launch", "app restart", "app stop", "record draft"] {
        assert!(
            refused.iter().any(|refused| refused == name),
            "setup: {name} stays refused: {refused:?}"
        );
    }
    assert!(
        served > 0 && !refused.is_empty(),
        "setup: rows of both kinds"
    );
    if cfg!(windows) {
        for name in ["mcp config", "mcp entries", "deploy", "up"] {
            assert!(
                refused.iter().any(|refused| refused == name),
                "setup: {name} stays refused on Windows: {refused:?}"
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
        prepared(name, arguments).unwrap_or_else(|refusal| panic!("{name}: refused {refusal}"))
    };
    let problem = |name: &str, arguments: Value| {
        let refusal = prepared(name, arguments).expect_err("refused");
        let outcome = cli_line::outcome_value(&refusal, Detail::Full);
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

    // A shaped row cut: `codes`, a page of the most entries, reads no Runtime.
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
    let cut = text_line(&result);
    let kept = cut["codes"].as_array().map_or(0, Vec::len);
    assert!(
        (1..page).contains(&kept),
        "the page is cut to the budget: {kept} of {page}"
    );
    assert_eq!(cut["truncated"], json!(true), "the cut is marked");
    assert!(cut["next_cursor"].is_string(), "the page goes on: {cut}");
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
