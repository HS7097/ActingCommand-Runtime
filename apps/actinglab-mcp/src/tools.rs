// SPDX-License-Identifier: AGPL-3.0-only

//! The tools (Workflow #434 LAB-5G2a; `contracts/lab-two-entries.md` §8.3 and §9; register
//! §1.287, "Slice LAB-5G2a"): one tool for each row of the operation table this server offers,
//! generated from the row, and which of them `tools/list` lists.
//!
//! - **Offered** ([`offered`]): the rows actinglab offers (its words' rule: a planned row of a
//!   built slice, [`ACTINGLAB_SLICES`]; a rescue row only on Windows, where the rescue module runs,
//!   and only one it serves) and the planned rows of slice LAB-5, which this server offers itself
//!   (§9.2). Every offered row is callable, listed or not.
//! - **Name**: the table's rule, `ac_` and the row's words with spaces and `-` as `_`
//!   (`Operation::mcp_tool`), but for the two rows of [`NAMED`] (board #434, the coordinator's
//!   ruling 6784514298, point 1: a name is never reused with another meaning). No tool is named
//!   `ac_observe`.
//! - **Definition**: `description` is the row's `help`; `inputSchema` has one property per
//!   parameter, typed as the survey's type table says (`lab5g-step0-survey.md` §1), a parameter's
//!   `note` its `description`, its `default` typed, a repeated one an array of at least one item;
//!   `required` the row's required parameters and those whose `required_in` names this front end;
//!   no other property (`additionalProperties: false`). The annotations: `readOnlyHint` is "the row
//!   does not write", `destructiveHint` "irreversible or disruptive", `openWorldHint` false, no
//!   `idempotentHint`. No `title` and no `outputSchema` (the rulings 13 to 15 of 6775192250).
//! - **Listed** ([`Listing`]): the modules `--list` names; without it every installed module, the
//!   authoring module only where its option lists it (§8.3: read at each listing, through
//!   `lab_ops::locate::authoring_module`). Not listed is never refused: a call names any offered
//!   row's tool.
//! - **A call**: this build serves none. Each call of a tool is answered with
//!   `mcp_operation_unsupported` ([`unsupported`]) as the CLI line of a refusal, `{outcome}`.

use crate::codes::McpCode;
use crate::protocol::Era;
use actingcommand_contract::outcome::{Category, Detail, Outcome};
use actingcommand_lab_ops::cli_line;
use actingcommand_lab_ops::codes::ArgumentProblem;
use actingcommand_lab_ops::locate::{self, AUTHORING_KEY, AuthoringModule};
use actingcommand_lab_ops::table::{
    ModuleRole, Operation, OperationModule, OperationStatus, OperationTable, Param, ParamForm,
    ParamType, TABLE_PATH,
};
use serde_json::{Map, Value, json};
use std::fmt;
use std::sync::OnceLock;

/// The `instructions` of `initialize` and `server/discover`: the survey's draft
/// (`lab5g-step0-survey.md` §6), its names corrected by the coordinator's ruling 6784514298,
/// point 1, and this build's one rule, that it serves no call yet.
pub(crate) const INSTRUCTIONS: &str = "ActingCommand Lab over MCP (actinglab-mcp). One tool per operation of the Lab's operation table: the tool is ac_ and the CLI words with spaces and - as _ (emu start is ac_emu_start), and its arguments are the CLI's parameters by name. Two tools keep other names: one frame is ac_frame and the authoring observe is ac_lab_observe; no tool is named ac_observe. Start with ac_status. A result is the line actinglab --json prints: the members and outcome; a refusal is {outcome, message?, details?}; warnings are in warnings. This build serves no call yet: each call answers mcp_operation_unsupported, and nothing is sent, run or written. A call not done within 25 s answers {handle, job_phase}: follow it with ac_run_show (id, wait_s). A run's handle is its request_id and survives restarts; any other handle lives only as long as this process (then handle_unknown). After an interrupted or uncertain write call ac_status or ac_run_show before anything else; never resend a write with new arguments. An answer over 24 KiB is cut where its operation marks the cut, or written whole to %TEMP%\\actingcommand-mcp\\materials with the warning answer_exported; never cut silently. Cursors belong to this process and its Runtime connection (cursor_invalid after a reconnect): read again from after. A Lab operation the Runtime answered whose check then failed is lab_operation_check_failed: read its effect before sending it again. Irreversible operations record who started them and ask nothing; deploy and rollback run detached: follow with ac_op_status. A module left out of the list (by --list, or the authoring module by lab_module_authoring) still answers its tools' calls. Manual: skill actingcommand.";

/// The slices whose planned rows actinglab offers: its words' `BUILT_SLICES`
/// (`apps/actinglab/src/cli_module/words.rs`), which this server's test holds equal.
pub(crate) const ACTINGLAB_SLICES: [&str; 5] = ["LAB-1", "LAB-1b", "LAB-2", "R-6", "R-8"];
/// The slice whose planned rows this server offers itself (§9.2): `record draft`, `mcp config`,
/// `mcp entries`.
const OWN_SLICE: &str = "LAB-5";
/// This front end: the id of its `modules` row of role `front_end`, which a parameter's
/// `required_in` names where MCP requires the parameter although the row does not.
const FRONT_END: &str = "mcp";
/// The rows whose tool the table's rule does not name, each with its tool, in one place (board
/// #434, the coordinator's ruling 6784514298, point 1): one frame is `ac_frame` (today's
/// `ac_observe`, a name no tool takes any more), and the authoring module's observe keeps today's
/// `ac_lab_observe`, which the rule would make `ac_observe`.
pub(crate) const NAMED: [(&str, &str); 2] = [("frame", "ac_frame"), ("observe", "ac_lab_observe")];
/// The words `--list` takes, each the id of a module row of role `operations` and the module the
/// rows name so (the table's `module` words).
const MODULES: [(&str, OperationModule); 4] = [
    ("core", OperationModule::Core),
    ("debugging", OperationModule::Debugging),
    ("authoring", OperationModule::Authoring),
    ("rescue", OperationModule::Rescue),
];
/// The modules every default list holds; the authoring module joins them where its option lists
/// it.
const ALWAYS_LISTED: [OperationModule; 3] = [
    OperationModule::Core,
    OperationModule::Debugging,
    OperationModule::Rescue,
];
/// What a `duration` parameter takes: a number with `ms`, `s`, `m` or `h`
/// (`lab_ops::core_ops::parse_duration`).
const DURATION_PATTERN: &str = "^[0-9]+(ms|s|m|h)$";
/// What a `key_value` parameter takes: `KEY=VALUE`.
const KEY_VALUE_PATTERN: &str = "^[^=]+=.*$";
/// What an `operation_id` parameter takes: `op-<ms>`.
const OPERATION_ID_PATTERN: &str = "^op-[0-9]+$";

/// The outcome detail of every outcome this server writes (model v2.2 §7.1: full by default;
/// `ACTINGCOMMAND_DETAIL=codes` keeps only the registered names), set once at start.
static DETAIL: OnceLock<Detail> = OnceLock::new();

/// Sets this server's outcome detail once, at start.
pub(crate) fn set_detail(detail: Detail) {
    let _ = DETAIL.set(detail);
}

/// An outcome as this server writes it: the `actingcommand.outcome.v1` envelope in the server's
/// detail.
pub(crate) fn envelope(outcome: &Outcome) -> Value {
    cli_line::outcome_value(outcome, DETAIL.get().copied().unwrap_or(Detail::Full))
}

/// What says whether the authoring module is listed by default: the place of this process's
/// program (`lab_ops::locate::authoring_module`), or a made-up one in a test.
pub(crate) type AuthoringPlace<'a> = &'a dyn Fn() -> Result<AuthoringModule, Outcome>;

/// Which modules' tools `tools/list` lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Listing {
    /// Without `--list`: every installed module, the authoring module only where its option
    /// lists it.
    Default,
    /// `--list`: these modules, whatever the authoring option says.
    Named(Vec<OperationModule>),
}

/// One offered row's tool.
struct Tool {
    name: String,
    module: OperationModule,
    definition: Value,
}

/// Every tool this build offers, generated once from the table.
pub(crate) struct Catalog {
    /// In the table's order.
    tools: Vec<Tool>,
    /// The ids of the table's module rows of role `operations`: the words `--list` takes.
    modules: Vec<String>,
}

impl Catalog {
    /// The tools of `table`'s offered rows. A default the row's type does not read is the
    /// table's `foreign_data_error`, with the decoder's text and the table's `path`.
    pub(crate) fn of(table: &OperationTable) -> Result<Self, Outcome> {
        let mut tools = Vec::new();
        for operation in table.operations.iter().filter(|row| offered(row)) {
            let name = tool_name(operation);
            let definition = definition(operation, &name)?;
            tools.push(Tool {
                name,
                module: operation.module,
                definition,
            });
        }
        let modules = table
            .modules
            .iter()
            .filter(|module| module.role == ModuleRole::Operations)
            .map(|module| module.id.clone())
            .collect();
        Ok(Self { tools, modules })
    }

    /// The listing `--list` names with `words`: each a module id of the table, at most once. A
    /// word that is none (an empty one too) is `not_recognised`, a word given twice `repeated`.
    pub(crate) fn named(&self, words: &[String]) -> Result<Listing, ArgumentProblem> {
        let mut named = Vec::new();
        for word in words {
            let module = MODULES
                .iter()
                .find(|(id, _)| *id == word.as_str() && self.modules.contains(word))
                .map(|(_, module)| *module)
                .ok_or(ArgumentProblem::NotRecognised)?;
            if named.contains(&module) {
                return Err(ArgumentProblem::Repeated);
            }
            named.push(module);
        }
        Ok(Listing::Named(named))
    }

    /// Whether `name` is the tool of an offered row: a call may name it, listed or not.
    pub(crate) fn offers(&self, name: &str) -> bool {
        self.tools.iter().any(|tool| tool.name == name)
    }

    /// The `tools/list` result of `listing`, `{tools}`, as this process's place lists it.
    pub(crate) fn list_result(&self, listing: &Listing) -> Value {
        json!({"tools": self.listed(listing, &locate::authoring_module)})
    }

    /// The definitions `listing` lists, in the table's order; `place` says whether the default
    /// lists the authoring module, read at each listing.
    pub(crate) fn listed(&self, listing: &Listing, place: AuthoringPlace<'_>) -> Vec<Value> {
        let modules = match listing {
            Listing::Named(modules) => modules.clone(),
            Listing::Default => default_modules(place),
        };
        self.tools
            .iter()
            .filter(|tool| modules.contains(&tool.module))
            .map(|tool| tool.definition.clone())
            .collect()
    }
}

/// The modules the default lists: [`ALWAYS_LISTED`], and the authoring module where its option
/// lists it. An option that cannot be read leaves it out, and the developer line says why, once
/// for this listing (§8.3); it refuses nothing.
fn default_modules(place: AuthoringPlace<'_>) -> Vec<OperationModule> {
    let mut modules = ALWAYS_LISTED.to_vec();
    match place() {
        Ok(module) if module.installed() => modules.push(OperationModule::Authoring),
        Ok(_) => {}
        Err(cause) => eprintln!(
            "{} {}: {cause} the authoring module's tools are left out of this listing, since \
             {AUTHORING_KEY} could not be read",
            cli_line::level(Category::Warning),
            crate::PROGRAM
        ),
    }
    modules
}

/// Whether this server offers `operation`: a planned row of a slice actinglab offers, or of this
/// server's own; a rescue row only where the rescue module runs ([`rescue_offered`]).
fn offered(operation: &Operation) -> bool {
    if operation.status != OperationStatus::Planned {
        return false;
    }
    if operation.module == OperationModule::Rescue {
        return rescue_offered(operation);
    }
    operation.slice == OWN_SLICE || ACTINGLAB_SLICES.contains(&operation.slice.as_str())
}

/// On Windows, where the rescue module runs: a rescue row it serves, or one of this server's own.
#[cfg(windows)]
fn rescue_offered(operation: &Operation) -> bool {
    operation.slice == OWN_SLICE || actingcommand_lab_rescue::rescue::serves(&operation.name)
}

/// Elsewhere the rescue module does not run, and no rescue row is offered.
#[cfg(not(windows))]
fn rescue_offered(_operation: &Operation) -> bool {
    false
}

/// The tool of `operation`: its entry of [`NAMED`], else the table's rule.
pub(crate) fn tool_name(operation: &Operation) -> String {
    NAMED
        .iter()
        .find(|(row, _)| *row == operation.name)
        .map_or_else(|| operation.mcp_tool(), |(_, tool)| (*tool).to_owned())
}

/// The tool definition of `operation`, named `name`; the same in every protocol version.
fn definition(operation: &Operation, name: &str) -> Result<Value, Outcome> {
    let mut definition = Map::new();
    definition.insert("name".to_owned(), json!(name));
    if let Some(help) = &operation.help {
        definition.insert("description".to_owned(), json!(help));
    }
    definition.insert("inputSchema".to_owned(), input_schema(operation)?);
    definition.insert(
        "annotations".to_owned(),
        json!({
            "readOnlyHint": !operation.writes,
            "destructiveHint": operation.irreversible || operation.disruptive,
            "openWorldHint": false,
        }),
    );
    Ok(Value::Object(definition))
}

/// The input schema of `operation`: one property per parameter, in the row's order.
fn input_schema(operation: &Operation) -> Result<Value, Outcome> {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for param in &operation.params {
        properties.insert(param.name.clone(), param_schema(param)?);
        let required_here = param
            .required_in
            .as_ref()
            .is_some_and(|fronts| fronts.iter().any(|front| front == FRONT_END));
        if param.required || required_here {
            required.push(Value::String(param.name.clone()));
        }
    }
    let mut schema = Map::new();
    schema.insert("type".to_owned(), json!("object"));
    schema.insert("properties".to_owned(), Value::Object(properties));
    if !required.is_empty() {
        schema.insert("required".to_owned(), Value::Array(required));
    }
    schema.insert("additionalProperties".to_owned(), Value::Bool(false));
    Ok(Value::Object(schema))
}

/// One parameter's property: a switch a boolean, any other its type's schema; a repeated one an
/// array of at least one; its typed `default` and its `note` as `description`.
fn param_schema(param: &Param) -> Result<Value, Outcome> {
    let value = match param.form {
        ParamForm::Switch => json!({"type": "boolean"}),
        ParamForm::Positional | ParamForm::Option => value_schema(&param.value_type),
    };
    let mut schema = if param.repeat {
        json!({"type": "array", "items": value, "minItems": 1})
    } else {
        value
    };
    if let Some(text) = &param.default {
        let typed = typed_default(param, text)?;
        schema["default"] = if param.repeat { json!([typed]) } else { typed };
    }
    if let Some(note) = &param.note {
        schema["description"] = json!(note);
    }
    Ok(schema)
}

/// The schema of one value of a parameter type (§9.1), as `serve` takes it.
fn value_schema(value_type: &ParamType) -> Value {
    match value_type {
        ParamType::Instance => json!({"type": "string", "minLength": 1}),
        ParamType::Integer => json!({"type": "integer"}),
        ParamType::Unsigned => json!({"type": "integer", "minimum": 0}),
        ParamType::Decimal => json!({"type": "number"}),
        ParamType::Boolean => json!({"type": "boolean"}),
        ParamType::Duration => json!({"type": "string", "pattern": DURATION_PATTERN}),
        ParamType::Json => json!({}),
        ParamType::ListOfStrings => json!({"type": "array", "items": {"type": "string"}}),
        ParamType::Enum(words) => json!({"type": "string", "enum": words}),
        ParamType::KeyValue => json!({"type": "string", "pattern": KEY_VALUE_PATTERN}),
        ParamType::OperationId => json!({"type": "string", "pattern": OPERATION_ID_PATTERN}),
        ParamType::String
        | ParamType::Path
        | ParamType::File
        | ParamType::Dir
        | ParamType::Key
        | ParamType::RequestId
        | ParamType::RunRef
        | ParamType::MaterialRef
        | ParamType::Code
        | ParamType::Scope
        | ParamType::TagOrCommit
        | ParamType::Sha256 => json!({"type": "string"}),
    }
}

/// A parameter's default as the JSON value of its type: a switch's or a boolean's `true` or
/// `false`, an integer's or an unsigned's number, a decimal's number; any other the text.
fn typed_default(param: &Param, text: &str) -> Result<Value, Outcome> {
    if param.form == ParamForm::Switch {
        return text
            .parse::<bool>()
            .map(Value::Bool)
            .map_err(|error| unreadable(&error));
    }
    match &param.value_type {
        ParamType::Boolean => text
            .parse::<bool>()
            .map(Value::Bool)
            .map_err(|error| unreadable(&error)),
        ParamType::Integer => text
            .parse::<i64>()
            .map(Value::from)
            .map_err(|error| unreadable(&error)),
        ParamType::Unsigned => text
            .parse::<u64>()
            .map(Value::from)
            .map_err(|error| unreadable(&error)),
        ParamType::Decimal => serde_json::from_str::<serde_json::Number>(text)
            .map(Value::Number)
            .map_err(|error| unreadable(&error)),
        _ => Ok(Value::String(text.to_owned())),
    }
}

/// A default its type does not read: the table's `foreign_data_error`, as the table's reader
/// refuses a file it cannot decode.
fn unreadable(error: &impl fmt::Display) -> Outcome {
    actingcommand_lab_ops::foreign_data_error(error).path(TABLE_PATH)
}

/// The refusal of every call of the tool `tool` in this build, which serves none:
/// `mcp_operation_unsupported` with the tool (`mcp_tool`).
pub(crate) fn unsupported(tool: &str) -> Outcome {
    Outcome::new(McpCode::OperationUnsupported).mcp_tool(tool)
}

/// The `tools/call` result of a call `refusal` answers: the CLI line of a refusal, `{outcome}`,
/// as compact JSON text and, from 2025-06-18 on, as `structuredContent`; `isError` true.
pub(crate) fn call_result(refusal: &Outcome, era: Era) -> Value {
    let mut line = Map::new();
    line.insert("outcome".to_owned(), envelope(refusal));
    let line = Value::Object(line);
    let mut result = Map::new();
    result.insert(
        "content".to_owned(),
        json!([{"type": "text", "text": line.to_string()}]),
    );
    if era.structured() {
        result.insert("structuredContent".to_owned(), line);
    }
    result.insert("isError".to_owned(), Value::Bool(true));
    Value::Object(result)
}
