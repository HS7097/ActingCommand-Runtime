// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #434 LAB-5G2a (register §1.287, "Slice LAB-5G2a"; contracts/lab-two-entries.md §8.3,
//! §9.1, §9.2 and §9.3; the coordinator's rulings on board #434, 6775192250 points 13 to 15 and
//! 19, and 6784514298 points 1 and 2): the tools `actinglab-mcp` generates from the operation
//! table this build embeds, their names, and what `--list` and the authoring option change. No
//! Runtime, no clock, no sleep; the authoring option's places are made-up installation roots in
//! a temporary folder, read as LAB-5F's test reads them.
//!
//! - `mcp_tools_from_op_table`: the list of every module is exactly the offered rows, in the
//!   table's order (actinglab's offered rows, by the slices its words name, the rescue module's
//!   on Windows, and the planned rows of slice LAB-5); each tool's properties, `required`,
//!   defaults, enumerations, notes, description and hints come from its row, as the survey's type
//!   table says; a doctored copy of the table changes the list accordingly, and the members the
//!   tools do not read change nothing.
//! - `mcp_tool_names_keep_two_exceptions`: the frame's tool is `ac_frame`, the authoring observe's
//!   `ac_lab_observe`, held in one place; no tool is `ac_observe`; every other tool takes the
//!   table's rule; every name matches `^[a-zA-Z0-9_-]{1,64}$` and none is given twice.
//! - `mcp_list_option_and_authoring_gate`: `--list` lists exactly the modules it names, the
//!   authoring module included where its option would leave it out, and refuses a word that is no
//!   module and one given twice; without it the authoring module is listed outside any
//!   installation root and where `lab_module_authoring` is true, and not where it is false,
//!   absent, no boolean, in a configuration that is no JSON or none; neither changes a tool's
//!   definition or which tools a call may name. The start option `--workers` (the coordinator's
//!   ruling of 2026-11-12 on #1639): 8 without it, the count given from 1 to 64; 0, 65 and -1
//!   refused `out_of_range` with the range, a word that is no whole number `wrong_type`, the
//!   option without its value `value_missing`, twice `repeated`, beside `--list-tools`
//!   `conflicting`, each with the usage line.

use crate::tools::{self, ACTINGLAB_SLICES, AuthoringPlace, Catalog, Listing, NAMED, Serving};
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_lab_ops::cli_line;
use actingcommand_lab_ops::codes::ArgumentProblem;
use actingcommand_lab_ops::locate::{self, AuthoringModule};
use actingcommand_lab_ops::table::{
    self, Operation, OperationModule, OperationStatus, OperationTable, Param, ParamForm, ParamType,
    RuntimeNeed,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

const EVERY_MODULE: [OperationModule; 4] = [
    OperationModule::Core,
    OperationModule::Debugging,
    OperationModule::Authoring,
    OperationModule::Rescue,
];
/// The modules a default list holds where the authoring option leaves its module out.
const WITHOUT_AUTHORING: [OperationModule; 3] = [
    OperationModule::Core,
    OperationModule::Debugging,
    OperationModule::Rescue,
];
/// The slice whose planned rows this server offers itself (§9.2).
const LAB5: &str = "LAB-5";

fn embedded() -> OperationTable {
    table::operation_table().expect("the operation table this build embeds reads")
}

fn catalog_of(table: &OperationTable) -> Catalog {
    Catalog::of(table).expect("the table's tools are generated")
}

fn every_module() -> Listing {
    Listing::Named(EVERY_MODULE.to_vec())
}

/// A place outside any installation root, where the authoring module is listed.
fn unrooted() -> Result<AuthoringModule, Outcome> {
    Ok(AuthoringModule::Unrooted)
}

fn listed(catalog: &Catalog, listing: &Listing, place: AuthoringPlace<'_>) -> Vec<Value> {
    catalog.listed(listing, place)
}

fn name_of(definition: &Value) -> String {
    definition["name"]
        .as_str()
        .unwrap_or_else(|| panic!("a tool definition without a name: {definition}"))
        .to_owned()
}

fn names(definitions: &[Value]) -> Vec<String> {
    definitions.iter().map(name_of).collect()
}

fn by_name(definitions: &[Value]) -> BTreeMap<String, Value> {
    definitions
        .iter()
        .map(|definition| (name_of(definition), definition.clone()))
        .collect()
}

fn row<'a>(table: &'a OperationTable, name: &str) -> &'a Operation {
    table
        .operation(name)
        .unwrap_or_else(|| panic!("the table has no row {name}"))
}

fn row_mut<'a>(table: &'a mut OperationTable, name: &str) -> &'a mut Operation {
    table
        .operations
        .iter_mut()
        .find(|operation| operation.name == name)
        .unwrap_or_else(|| panic!("the table has no row {name}"))
}

/// The slices actinglab's words offer (`BUILT_SLICES` in `apps/actinglab/src/cli_module/words.rs`),
/// read from that file's text: actinglab is a program, so its constant is out of this test's reach.
fn actinglab_built_slices() -> Vec<String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../actinglab/src/cli_module/words.rs");
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let declared = &text[text
        .find("const BUILT_SLICES")
        .expect("actinglab's words declare BUILT_SLICES")..];
    let open = declared
        .find("= [")
        .expect("BUILT_SLICES is an array written out")
        + "= [".len();
    let close = open + declared[open..].find(']').expect("BUILT_SLICES ends");
    declared[open..close]
        .split(',')
        .map(|item| item.trim().trim_matches('"').to_owned())
        .filter(|item| !item.is_empty())
        .collect()
}

/// Whether the rescue module serves the rescue row `name`, on Windows, where it runs.
#[cfg(windows)]
fn rescue_served(name: &str) -> bool {
    actingcommand_lab_rescue::rescue::SERVED.contains(&name)
}

#[cfg(not(windows))]
fn rescue_served(_name: &str) -> bool {
    false
}

/// The rows the server must offer, stated from the table: a planned row of a slice actinglab's
/// words offer or of slice LAB-5; of the rescue module only on Windows, and only a row it serves or
/// one of slice LAB-5.
fn must_offer(operation: &Operation, built: &[String]) -> bool {
    if operation.status != OperationStatus::Planned {
        return false;
    }
    let own = operation.slice == LAB5;
    if operation.module == OperationModule::Rescue {
        return cfg!(windows) && (own || rescue_served(&operation.name));
    }
    own || built.contains(&operation.slice)
}

/// Whether the parameter is required through MCP: by its row, or by `required_in` naming `mcp`.
fn required_through_mcp(param: &Param) -> bool {
    param.required
        || param
            .required_in
            .as_ref()
            .is_some_and(|fronts| fronts.iter().any(|front| front == "mcp"))
}

/// The members of a JSON object, in order.
fn members(value: &Value) -> Vec<String> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("not a JSON object: {value}"))
        .keys()
        .cloned()
        .collect()
}

/// A tool's definition is its row's (contract §9.1; the survey's type table, §1; rulings 13 to
/// 15): name, description, input schema and hints, and nothing else. Workflow #434 LAB-5G2b
/// (adapted, its work order's point 4 and 5): but for the front's own members (`tools::SUPPLIED`:
/// a parameter the front gives itself is no property, `export` follows the parameters of a row
/// that takes it) and, on a row this build serves, the absolute path a path parameter's
/// description asks for.
fn assert_definition_from_row(operation: &Operation, definition: &Value) {
    let row_name = operation.name.as_str();
    let mut wanted = vec!["name"];
    if operation.help.is_some() {
        wanted.push("description");
    }
    wanted.extend(["inputSchema", "annotations"]);
    assert_eq!(
        members(definition),
        wanted,
        "{row_name}: members (no title, no outputSchema)"
    );
    assert_eq!(
        definition["name"],
        tools::tool_name(operation),
        "{row_name}"
    );
    assert_eq!(
        definition.get("description").and_then(Value::as_str),
        operation.help.as_deref(),
        "{row_name}: description"
    );
    assert_eq!(
        definition["annotations"],
        json!({
            "readOnlyHint": !operation.writes,
            "destructiveHint": operation.irreversible || operation.disruptive,
            "openWorldHint": false,
        }),
        "{row_name}: hints (no title, no idempotentHint)"
    );
    let schema = &definition["inputSchema"];
    assert_eq!(schema["type"], "object", "{row_name}");
    assert_eq!(schema["additionalProperties"], false, "{row_name}");
    let properties = &schema["properties"];
    let taken = operation
        .params
        .iter()
        .filter(|param| tools::supplied(row_name, &param.name).is_none())
        .collect::<Vec<_>>();
    let mut expected = taken
        .iter()
        .map(|param| param.name.clone())
        .collect::<Vec<_>>();
    if tools::takes_export(row_name) {
        expected.push(tools::EXPORT.to_owned());
        assert_eq!(
            properties[tools::EXPORT]["type"],
            "boolean",
            "{row_name}: export"
        );
    }
    // Workflow #434 LAB-5G3 (adapted, its work order points 3 and 4): then the members the front
    // adds to the row (`tools::ADDED`, each a wait), in that table's order.
    for (_, member, _) in tools::ADDED.iter().filter(|(row, _, _)| *row == row_name) {
        expected.push((*member).to_owned());
        assert_eq!(
            properties[*member]["type"], "integer",
            "{row_name}: {member}"
        );
    }
    assert_eq!(
        members(properties),
        expected,
        "{row_name}: one property per parameter the front does not give, in the row's order"
    );
    let required = taken
        .iter()
        .filter(|param| required_through_mcp(param))
        .map(|param| json!(param.name))
        .collect::<Vec<_>>();
    match schema.get("required") {
        None => assert!(required.is_empty(), "{row_name}: required missing"),
        Some(given) => assert_eq!(given, &json!(required), "{row_name}: required"),
    }
    let served = tools::serving(operation) != Serving::Refused;
    for param in taken {
        assert_property(row_name, param, &properties[param.name.as_str()], served);
    }
}

/// One parameter's property: its type's schema, a repeated one an array of at least one, its
/// typed default and its note; Workflow #434 LAB-5G2b (adapted): on a row this build serves
/// (`served`), a path parameter's note followed by the absolute path it takes.
fn assert_property(row_name: &str, param: &Param, property: &Value, served: bool) {
    let at = format!("{row_name} {}", param.name);
    let value = if param.repeat {
        assert_eq!(property["type"], "array", "{at}: repeat");
        assert_eq!(property["minItems"], 1, "{at}: repeat");
        &property["items"]
    } else {
        property
    };
    let string_with = |member: &str, expected: Value| {
        assert_eq!(value["type"], "string", "{at}");
        assert_eq!(value[member], expected, "{at}: {member}");
    };
    match (param.form, &param.value_type) {
        (ParamForm::Switch, _) | (_, ParamType::Boolean) => {
            assert_eq!(value["type"], "boolean", "{at}");
        }
        (_, ParamType::Json) => assert_eq!(value.get("type"), None, "{at}: any value"),
        (_, ParamType::Integer) => {
            assert_eq!(value["type"], "integer", "{at}");
            assert_eq!(value.get("minimum"), None, "{at}");
        }
        (_, ParamType::Unsigned) => {
            assert_eq!(value["type"], "integer", "{at}");
            assert_eq!(value["minimum"], 0, "{at}");
        }
        (_, ParamType::Decimal) => assert_eq!(value["type"], "number", "{at}"),
        (_, ParamType::ListOfStrings) => {
            assert_eq!(value["type"], "array", "{at}");
            assert_eq!(value["items"], json!({"type": "string"}), "{at}");
        }
        (_, ParamType::Enum(words)) => string_with("enum", json!(words)),
        (_, ParamType::Duration) => string_with("pattern", json!("^[0-9]+(ms|s|m|h)$")),
        (_, ParamType::KeyValue) => string_with("pattern", json!("^[^=]+=.*$")),
        (_, ParamType::OperationId) => string_with("pattern", json!("^op-[0-9]+$")),
        (_, ParamType::Instance) => string_with("minLength", json!(1)),
        _ => {
            assert_eq!(value["type"], "string", "{at}");
            assert_eq!(value.get("pattern"), None, "{at}");
            assert_eq!(value.get("enum"), None, "{at}");
        }
    }
    match &param.default {
        None => assert_eq!(property.get("default"), None, "{at}: no default"),
        Some(text) => {
            let typed = match (param.form, &param.value_type) {
                (ParamForm::Switch, _) | (_, ParamType::Boolean) => json!(text == "true"),
                (_, ParamType::Integer) => json!(text.parse::<i64>().expect("an integer default")),
                (_, ParamType::Unsigned) => {
                    json!(text.parse::<u64>().expect("an unsigned default"))
                }
                (_, ParamType::Decimal) => json!(text.parse::<f64>().expect("a decimal default")),
                _ => json!(text),
            };
            let typed = if param.repeat { json!([typed]) } else { typed };
            assert_eq!(property["default"], typed, "{at}: default");
        }
    }
    let path = matches!(
        param.value_type,
        ParamType::Path | ParamType::File | ParamType::Dir
    );
    let described = if served && path {
        Some(match &param.note {
            Some(note) => format!("{note}; {}", tools::ABSOLUTE_PATH),
            None => tools::ABSOLUTE_PATH.to_owned(),
        })
    } else {
        param.note.clone()
    };
    assert_eq!(
        property.get("description").and_then(Value::as_str),
        described.as_deref(),
        "{at}: the note is the description"
    );
}

#[test]
fn mcp_tools_from_op_table() {
    let table = embedded();
    let catalog = catalog_of(&table);
    let built = actinglab_built_slices();
    let ours = ACTINGLAB_SLICES
        .iter()
        .map(|slice| (*slice).to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        ours,
        built.iter().cloned().collect::<BTreeSet<_>>(),
        "the server offers the slices actinglab's words offer"
    );

    // The list of every module is exactly the offered rows, in the table's order.
    let every = listed(&catalog, &every_module(), &unrooted);
    let rows = table
        .operations
        .iter()
        .filter(|operation| must_offer(operation, &built))
        .collect::<Vec<_>>();
    assert!(
        rows.iter()
            .any(|operation| operation.module == OperationModule::Core)
            && rows
                .iter()
                .any(|operation| operation.slice == LAB5
                    && operation.status == OperationStatus::Planned),
        "setup: the table offers core rows and rows of slice LAB-5"
    );
    assert_eq!(
        names(&every),
        rows.iter()
            .map(|operation| tools::tool_name(operation))
            .collect::<Vec<_>>(),
        "the tools are the offered rows'"
    );
    for (operation, definition) in rows.iter().zip(&every) {
        assert_definition_from_row(operation, definition);
    }
    assert!(
        table.operations.iter().any(|operation| operation
            .params
            .iter()
            .any(|param| param.required_in.is_some())),
        "setup: a parameter is required through MCP only"
    );

    // A doctored copy of the table changes the list accordingly.
    let mut doctored = table.clone();
    {
        let status = row_mut(&mut doctored, "status");
        status.writes = !status.writes;
        status.help = Some("Reads a doctored sentence.".to_owned());
        status.params.push(Param {
            name: "doctored".to_owned(),
            form: ParamForm::Option,
            value_type: ParamType::Unsigned,
            required: false,
            repeat: false,
            default: Some("7".to_owned()),
            required_in: Some(vec!["mcp".to_owned()]),
            note: Some("a doctored note".to_owned()),
        });
    }
    {
        let events = row_mut(&mut doctored, "events");
        let view = events
            .params
            .iter_mut()
            .find(|param| param.name == "view")
            .expect("events takes view");
        view.value_type = ParamType::Enum(vec!["one".to_owned(), "two".to_owned()]);
    }
    row_mut(&mut doctored, "facts").irreversible = true;
    row_mut(&mut doctored, "schedule").status = OperationStatus::Reserved;
    // And nothing else does: the members the tools do not read.
    {
        let summary = row_mut(&mut doctored, "summary");
        summary.output = "a doctored output".to_owned();
        summary.notes = Some("a doctored note".to_owned());
        summary.source = vec!["a doctored source".to_owned()];
        summary.runtime = RuntimeNeed::Offline;
        summary.requests.clear();
    }
    let changed = listed(&catalog_of(&doctored), &every_module(), &unrooted);
    let mut expected_names = names(&every);
    expected_names.retain(|name| name != "ac_schedule");
    assert_eq!(
        names(&changed),
        expected_names,
        "a row made reserved leaves the list, and only it"
    );
    let original = by_name(&every);
    let changed = by_name(&changed);
    let status = &changed["ac_status"];
    assert_definition_from_row(row(&doctored, "status"), status);
    assert_ne!(
        status["annotations"]["readOnlyHint"], original["ac_status"]["annotations"]["readOnlyHint"],
        "writes flips readOnlyHint"
    );
    assert_eq!(status["description"], "Reads a doctored sentence.");
    assert_eq!(
        status["inputSchema"]["properties"]["doctored"],
        json!({"type": "integer", "minimum": 0, "default": 7, "description": "a doctored note"}),
        "an added parameter is an added property"
    );
    assert!(
        status["inputSchema"]["required"]
            .as_array()
            .is_some_and(|required| required.contains(&json!("doctored"))),
        "required_in naming mcp requires it: {status}"
    );
    assert_eq!(
        changed["ac_events"]["inputSchema"]["properties"]["view"]["enum"],
        json!(["one", "two"]),
        "an enumeration is the row's"
    );
    assert_definition_from_row(row(&doctored, "events"), &changed["ac_events"]);
    assert_eq!(
        (
            &original["ac_facts"]["annotations"]["destructiveHint"],
            &changed["ac_facts"]["annotations"]["destructiveHint"],
        ),
        (&json!(false), &json!(true)),
        "irreversible makes the tool destructive"
    );
    for (name, definition) in &changed {
        if matches!(name.as_str(), "ac_status" | "ac_events" | "ac_facts") {
            continue;
        }
        assert_eq!(
            definition, &original[name],
            "{name}: no doctored member it reads, so its definition is the same"
        );
    }
}

#[test]
fn mcp_tool_names_keep_two_exceptions() {
    // The two exceptions, held in one place (board #434, 6784514298, point 1).
    assert_eq!(
        NAMED,
        [("frame", "ac_frame"), ("observe", "ac_lab_observe")]
    );
    let table = embedded();
    let frame = row(&table, "frame");
    assert_eq!(frame.module, OperationModule::Core, "frame is a core row");
    assert_eq!(tools::tool_name(frame), "ac_frame");
    let observe = row(&table, "observe");
    assert_eq!(
        observe.module,
        OperationModule::Authoring,
        "observe is the authoring module's"
    );
    assert_eq!(
        observe.mcp_tool(),
        "ac_observe",
        "setup: the table's rule would give the authoring observe today's name of one frame"
    );
    assert_eq!(tools::tool_name(observe), "ac_lab_observe");
    for operation in &table.operations {
        if NAMED.iter().all(|(named, _)| *named != operation.name) {
            assert_eq!(
                tools::tool_name(operation),
                operation.mcp_tool(),
                "{}: the table's rule",
                operation.name
            );
        }
    }

    let catalog = catalog_of(&table);
    let every = names(&listed(&catalog, &every_module(), &unrooted));
    for exception in ["ac_frame", "ac_lab_observe"] {
        assert!(
            every.contains(&exception.to_owned()),
            "{exception} is listed: {every:?}"
        );
    }
    assert!(
        !every.contains(&"ac_observe".to_owned()),
        "no tool is named ac_observe: {every:?}"
    );
    assert!(
        !catalog.offers("ac_observe"),
        "a call cannot name ac_observe"
    );
    for name in &every {
        assert!(
            (1..=64).contains(&name.len())
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'),
            "{name} does not match ^[a-zA-Z0-9_-]{{1,64}}$"
        );
    }
    let mut unique = every.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        every.len(),
        "a name is given twice: {every:?}"
    );
}

/// A place of the program, as LAB-5F's test makes them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Place {
    True,
    False,
    Absent,
    NotBoolean,
    NotJson,
    Missing,
    Unrooted,
}

/// The program `actinglab-mcp` at `place`, made up under `base`: outside any installation root,
/// or in a single-location root's `runtime\` beside the root's configuration.
fn program_at(base: &Path, place: Place) -> PathBuf {
    if place == Place::Unrooted {
        let unpacked = base.join("unpacked-tools");
        fs::create_dir_all(&unpacked).expect("make the unpacked folder");
        return unpacked.join("actinglab-mcp.exe");
    }
    let root = base.join("install-root");
    let runtime = root.join("runtime");
    fs::create_dir_all(&runtime).expect("make the root's runtime folder");
    fs::write(runtime.join("BUILD-MANIFEST.json"), b"{}\n").expect("write the build manifest");
    let config = root.join("actingd.config.json");
    let mut document = json!({});
    match place {
        Place::True => document["lab_module_authoring"] = json!(true),
        Place::False => document["lab_module_authoring"] = json!(false),
        Place::NotBoolean => document["lab_module_authoring"] = json!("true"),
        _ => {}
    }
    match place {
        Place::NotJson => fs::write(config, b"{\"lab_module_authoring\": true,\n")
            .expect("write the configuration"),
        Place::Missing => {}
        _ => fs::write(config, document.to_string()).expect("write the configuration"),
    }
    runtime.join("actinglab-mcp.exe")
}

/// The command line `words`, as the program reads its arguments.
fn parse_words(words: &[&str]) -> Result<crate::Invocation, crate::Ended> {
    let arguments = words
        .iter()
        .map(|word| OsString::from(*word))
        .collect::<Vec<_>>();
    crate::parse(&arguments)
}

/// The module of the row whose tool is `name`.
fn module_of(table: &OperationTable, name: &str) -> OperationModule {
    table
        .operations
        .iter()
        .find(|operation| tools::tool_name(operation) == name)
        .map(|operation| operation.module)
        .unwrap_or_else(|| panic!("no row's tool is {name}"))
}

#[test]
fn mcp_list_option_and_authoring_gate() {
    let table = embedded();
    let catalog = catalog_of(&table);
    let every = listed(&catalog, &every_module(), &unrooted);
    let full = by_name(&every);
    let of_modules = |modules: &[OperationModule]| {
        names(&every)
            .into_iter()
            .filter(|name| modules.contains(&module_of(&table, name)))
            .collect::<Vec<_>>()
    };
    assert!(
        !of_modules(&[OperationModule::Authoring]).is_empty()
            && !of_modules(&[OperationModule::Core]).is_empty(),
        "setup: core and authoring tools exist"
    );

    // `--list` names the modules listed, read from the command line.
    assert_eq!(
        crate::parse(&[]).expect("no option parses"),
        crate::Invocation {
            list: None,
            list_tools: false,
            workers: 8,
        }
    );
    let invocation = crate::parse(&[
        OsString::from("--list-tools"),
        OsString::from("--list"),
        OsString::from("core,rescue"),
    ])
    .expect("--list-tools --list core,rescue parses");
    assert!(invocation.list_tools);
    let words = invocation.list.expect("--list is given");
    assert_eq!(words, ["core", "rescue"]);
    let named = catalog.named(&words).expect("core and rescue are modules");
    assert_eq!(
        named,
        Listing::Named(vec![OperationModule::Core, OperationModule::Rescue])
    );
    assert_eq!(
        names(&listed(&catalog, &named, &unrooted)),
        of_modules(&[OperationModule::Core, OperationModule::Rescue])
    );
    for (given, problem) in [
        (vec!["tiers"], ArgumentProblem::NotRecognised),
        (vec!["Core"], ArgumentProblem::NotRecognised),
        (vec!["core", ""], ArgumentProblem::NotRecognised),
        (vec!["mcp"], ArgumentProblem::NotRecognised),
        (vec!["core", "core"], ArgumentProblem::Repeated),
    ] {
        let given = given.into_iter().map(str::to_owned).collect::<Vec<_>>();
        assert_eq!(catalog.named(&given), Err(problem), "--list {given:?}");
    }

    // The start option `--workers` (the coordinator's ruling of 2026-11-12 on #1639): eight
    // without it, the count given from 1 to 64, and a usage refusal for anything else.
    for (words, workers) in [
        (vec!["--list", "core"], 8),
        (vec!["--workers", "3", "--list", "core"], 3),
        (vec!["--workers", "1"], 1),
        (vec!["--workers", "64"], 64),
    ] {
        let invocation = parse_words(&words).expect("the line parses");
        assert_eq!(invocation.workers, workers, "{words:?}");
    }
    for (words, problem, ranged) in [
        (vec!["--workers", "0"], "out_of_range", true),
        (vec!["--workers", "65"], "out_of_range", true),
        (vec!["--workers", "-1"], "out_of_range", true),
        (vec!["--workers", "many"], "wrong_type", false),
        (vec!["--workers", "2.5"], "wrong_type", false),
        (vec!["--workers"], "value_missing", false),
        (vec!["--workers", "2", "--workers", "3"], "repeated", false),
        (vec!["--list-tools", "--workers", "2"], "conflicting", false),
    ] {
        let ended = parse_words(&words).expect_err("the line is refused");
        let outcome = cli_line::outcome_value(&ended.outcome, Detail::Full);
        assert_eq!(outcome["code"], "actingctl_usage_invalid", "{words:?}");
        assert_eq!(outcome["values"]["argument"], "--workers", "{words:?}");
        assert_eq!(outcome["values"]["argument_problem"], problem, "{words:?}");
        let range = (
            &outcome["values"]["range_min"],
            &outcome["values"]["range_max"],
        );
        if ranged {
            assert_eq!(range, (&json!(1), &json!(64)), "{words:?}: the range");
        } else {
            assert_eq!(range, (&Value::Null, &Value::Null), "{words:?}: no range");
        }
        assert_eq!(ended.text, Some(crate::USAGE), "{words:?}: the usage line");
    }

    // The authoring option decides only the default.
    let authoring_only = catalog
        .named(&["authoring".to_owned()])
        .expect("authoring is a module");
    for place in [
        Place::True,
        Place::False,
        Place::Absent,
        Place::NotBoolean,
        Place::NotJson,
        Place::Missing,
        Place::Unrooted,
    ] {
        let base = tempfile::tempdir().expect("tempdir");
        let program = program_at(base.path(), place);
        let read = || locate::authoring_module_from(&program, None);
        let default = listed(&catalog, &Listing::Default, &read);
        let expected = if matches!(place, Place::True | Place::Unrooted) {
            names(&every)
        } else {
            of_modules(&WITHOUT_AUTHORING)
        };
        assert_eq!(names(&default), expected, "{place:?}: the default list");
        assert_eq!(
            names(&listed(&catalog, &authoring_only, &read)),
            of_modules(&[OperationModule::Authoring]),
            "{place:?}: --list authoring lists the authoring module whatever the option says"
        );
        assert_eq!(
            names(&listed(&catalog, &every_module(), &read)),
            names(&every),
            "{place:?}: --list naming every module lists every tool"
        );
        // And nothing else: a listed tool's definition is the full list's, and a call may name
        // every offered row's tool, listed or not.
        for definition in default {
            assert_eq!(
                definition,
                full[&name_of(&definition)],
                "{place:?}: a listing leaves a definition as it is"
            );
        }
        for name in full.keys() {
            assert!(catalog.offers(name), "{place:?}: {name} may be called");
        }
    }
}
