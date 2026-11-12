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
//! - **A call** (Workflow #434 LAB-5G2b, [`serving`]; `crate::calls` serves it): a row
//!   `lab_ops::core_ops` serves is served in this process, and (its second part) on Windows a
//!   rescue row the rescue module serves, but one whose run the front forces detached
//!   ([`runs_detached`]), which is answered with `mcp_operation_unsupported` ([`unsupported`]) as
//!   the CLI line of a refusal, `{outcome}`; Workflow #434 LAB-5G4b: every other row through a
//!   `serve` child (`crate::children`). The front's own members ([`SUPPLIED`], [`EXPORT`])
//!   change the schema of the rows they belong to, and a parameter whose type is a path says in
//!   its description, on a row this build serves, that it takes an absolute path
//!   ([`ABSOLUTE_PATH`]).
//! - **The jobs' members** (Workflow #434 LAB-5G3, [`ADDED`]): `ac_run_show` and `ac_run_stop`
//!   take `wait_s`, from 0 to the call budget in effect (`--call-budget`,
//!   [`Catalog::with_call_budget`]), and `ac_events` and `ac_codes` a `cursor` they give back; the
//!   instructions name that budget ([`instructions`]).

use crate::codes::McpCode;
use actingcommand_contract::outcome::{Category, Detail, Outcome};
use actingcommand_lab_ops::cli_line;
use actingcommand_lab_ops::codes::ArgumentProblem;
use actingcommand_lab_ops::locate::{self, AUTHORING_KEY, AuthoringModule};
use actingcommand_lab_ops::table::{
    ModuleRole, Operation, OperationModule, OperationStatus, OperationTable, Param, ParamForm,
    ParamType, RuntimeNeed, TABLE_PATH,
};
use serde_json::{Map, Value, json};
use std::fmt;
use std::sync::OnceLock;

/// The `instructions` of `initialize` and `server/discover`: the survey's draft
/// (`lab5g-step0-survey.md` §6), its names corrected by the coordinator's ruling 6784514298,
/// point 1, and (Workflow #434 LAB-5G2b) which kinds of row this build serves. Workflow #434
/// LAB-5G3: the handles, the jobs and the cursors as this build has them, and the call budget in
/// effect where [`CALL_BUDGET_MARK`] stands ([`instructions`]). Workflow #434 LAB-5G4b: the rows
/// served through `serve` children, the two still refused, and what an answer over the budget is
/// for a row whose answer this process does not shape.
pub(crate) const INSTRUCTIONS: &str = "ActingCommand Lab over MCP (actinglab-mcp). One tool per operation of the Lab's operation table: the tool is ac_ and the CLI words with spaces and - as _ (emu start is ac_emu_start), and its arguments are the CLI's parameters by name. Two tools keep other names: one frame is ac_frame and the authoring observe is ac_lab_observe; no tool is named ac_observe. Start with ac_status. A result is the line actinglab --json prints: the members and outcome; a refusal is {outcome, message?, details?}; warnings are in warnings. This build serves in its own process the operations actinglab's CLI module serves (the core ones and run start, run stop, reset, drill) and, on Windows, the rescue module's as Lab-MCP; every other one through a child actinglab serve --stdio --front mcp as Lab-MCP, one call per child at a time (a call while --children children are busy answers job_capacity_exhausted); paths given absolute. Only deploy and rollback, which must run detached, answer mcp_operation_unsupported, and nothing is sent, run or written for them. Every call it serves runs as a job: one not done within {call_budget_s} s (this server's call budget) answers {handle, job_phase} and goes on; follow it with ac_run_show (id the handle, wait_s). ac_run_start answers at once with its run's handle, the run's request_id, which survives restarts; any other handle lives only as long as this process (then handle_unknown). If an ac_run_start call was interrupted before it answered, call ac_status before submitting again. After an interrupted or uncertain write call ac_status or ac_run_show before anything else; never resend a write with new arguments. An answer over 24 KiB is cut where its operation marks the cut, or written whole to %TEMP%\\actingcommand-mcp\\materials with the warning answer_exported (operations served in this process), else sent in structuredContent alone beside a summary text; never cut silently; one that still does not fit answers output_budget_exceeded with its handle, and ac_run_show gives that job's summary. An events cursor belongs to this process and its Runtime connection (cursor_invalid after a reconnect): read again from after; a codes or runs cursor carries its own position. A Lab operation the Runtime answered whose check then failed is lab_operation_check_failed: read its effect before sending it again. Irreversible operations record who started them and ask nothing. This build does not serve deploy and rollback through MCP: use the command line. A module left out of the list (by --list, or the authoring module by lab_module_authoring) still answers its tools' calls. Manual: skill actingcommand.";
/// Workflow #434 LAB-5G3: where [`INSTRUCTIONS`] names the call budget.
const CALL_BUDGET_MARK: &str = "{call_budget_s}";
/// Workflow #434 LAB-5G3: today's call budget, in seconds (`apps/actingctl/src/mcp/tools.rs`,
/// `CALL_BUDGET`): the start option `--call-budget` when it is not given.
pub(crate) const DEFAULT_CALL_BUDGET_S: u64 = 25;

/// Workflow #434 LAB-5G3: [`INSTRUCTIONS`] for a call budget of `budget_s` seconds.
pub(crate) fn instructions(budget_s: u64) -> String {
    INSTRUCTIONS.replace(CALL_BUDGET_MARK, &budget_s.to_string())
}

/// The slices whose planned rows actinglab offers: its words' `BUILT_SLICES`
/// (`apps/actinglab/src/cli_module/words.rs`), which this server's test holds equal.
pub(crate) const ACTINGLAB_SLICES: [&str; 5] = ["LAB-1", "LAB-1b", "LAB-2", "R-6", "R-8"];
/// The slice whose planned rows this server offers itself (§9.2): `record draft`, `mcp config`,
/// `mcp entries` and, since LAB-5G5b, `mcp plan` (Workflow #434 LAB-5G2b, second part: the three
/// `mcp` rows served with the rescue module's others; Workflow #434 LAB-5G4b: `record draft`
/// through a `serve` child, which refuses it until actinglab has its command, [`serving`]).
pub(crate) const OWN_SLICE: &str = "LAB-5";
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

/// Workflow #434 LAB-5G2b: what the front gives a row's parameter itself, which the row's tool
/// leaves out of its schema and refuses as an argument it does not know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Supplied {
    /// This server's export directory, always: one frame is written there.
    ExportDirectory,
    /// This server's export directory when the tool's [`EXPORT`] is true: the whole material is
    /// written there, and the answer names the file.
    ExportDirectoryOnExport,
    /// `true`: through MCP the row runs detached (contract §5), answering at once. Workflow #434
    /// LAB-5G2b, second part: such a row is refused for now ([`runs_detached`]).
    True,
}

/// Workflow #434 LAB-5G2b (work order point 4; board #434, the coordinator's ruling 9 of
/// 6775192250; contract §5): the front's own members, in one place. Each entry is a row, its
/// parameter the front gives itself and what it gives. A row with
/// [`Supplied::ExportDirectoryOnExport`] also takes the boolean [`EXPORT`] beside its parameters.
pub(crate) const SUPPLIED: [(&str, &str, Supplied); 4] = [
    ("material", "out", Supplied::ExportDirectoryOnExport),
    ("frame", "out", Supplied::ExportDirectory),
    ("deploy", "detach", Supplied::True),
    ("rollback", "detach", Supplied::True),
];
/// Workflow #434 LAB-5G2b: the tool's own member of a row whose parameter is
/// [`Supplied::ExportDirectoryOnExport`].
pub(crate) const EXPORT: &str = "export";
/// Its description.
const EXPORT_NOTE: &str = "true: write the whole material to this server's export directory and answer the file {path, sha256, size}; absent or false: read its text";
/// Workflow #434 LAB-5G2b: what the description of a parameter whose type is a path says on a row
/// this build serves, where a relative path is refused (stricter than today's server).
pub(crate) const ABSOLUTE_PATH: &str = "an absolute path: a relative one is refused";

/// Workflow #434 LAB-5G3: a member the front adds to a row's tool beside the row's parameters,
/// which the front reads itself and does not hand to the serving code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Added {
    /// `wait_s`: whole seconds from 0 to the call budget, `default_s` when it is absent (never
    /// more than the budget); `note` begins its description.
    Wait { default_s: u64, note: &'static str },
    /// `cursor`: a cursor the call gives back; `note` is its description.
    Cursor { note: &'static str },
}

/// Workflow #434 LAB-5G3 (work order points 3, 4 and 6; the survey's §3, its table of front
/// members): the members the front adds, in one place, each a row, the member and what it is.
/// `ac_run_show`'s `id` also takes a job handle, and `ac_runs` takes back its cursor as its row's
/// own parameter; neither is a member added here.
pub(crate) const ADDED: [(&str, &str, Added); 4] = [
    (
        "run show",
        "wait_s",
        Added::Wait {
            default_s: 0,
            note: "seconds to wait: a run is read again every second while it is not found, admitted or running, and a job handle given as id is waited for until its job ends",
        },
    ),
    (
        "run stop",
        "wait_s",
        Added::Wait {
            default_s: 20,
            note: "seconds to wait for the stop and the reset; not done by then, the answer is {handle, job_phase} for ac_run_show",
        },
    ),
    (
        "events",
        "cursor",
        Added::Cursor {
            note: "the next_cursor of an earlier page, taken back on the Runtime connection it was issued on; once that connection was made again it is cursor_invalid: read again from after",
        },
    ),
    (
        "codes",
        "cursor",
        Added::Cursor {
            note: "the next_cursor of the previous page, given with the same prefix, owner and limit",
        },
    ),
];

/// Workflow #434 LAB-5G3: the property of the member `added` the front adds, for a call budget of
/// `budget_s` seconds: a wait an integer from 0 to the budget, its default never more than it,
/// its description naming the budget; a cursor a string.
fn added_schema(added: Added, budget_s: u64) -> Value {
    match added {
        Added::Wait { default_s, note } => json!({
            "type": "integer",
            "minimum": 0,
            "maximum": budget_s,
            "default": default_s.min(budget_s),
            "description": format!("{note}; from 0 to {budget_s}, this server's call budget in seconds"),
        }),
        Added::Cursor { note } => json!({"type": "string", "description": note}),
    }
}

/// Workflow #434 LAB-5G2b: how this build serves the calls of a row's tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Serving {
    /// By `lab_ops::core_ops`, in this process: the rows actinglab's CLI module serves.
    Core,
    /// Workflow #434 LAB-5G2b, second part: by the rescue module (`lab_rescue::rescue`), in this
    /// process, on Windows, with the initiator `Lab-MCP`.
    Rescue,
    /// Workflow #434 LAB-5G4b: by actinglab's own `serve` loop in a child process
    /// (`crate::children`), one request per child at a time.
    Child,
    /// Not by this build: `mcp_operation_unsupported`, nothing read, sent, run or written.
    Refused,
}

/// Workflow #434 LAB-5G2b: the rule that decides how a row's calls are served. A row
/// `lab_ops::core_ops` serves (the core rows but those actinglab's own commands serve, and `run
/// start`, `run stop`, `reset`, `drill`): [`Serving::Core`]. The second part: a rescue row the
/// rescue module serves (`lab_rescue::rescue::serves`, `mcp config`, `mcp entries` and `mcp plan`
/// among them), on Windows, where it runs: [`Serving::Rescue`]; a rescue row refused for a reason
/// of its own stays refused: one whose run the front forces detached ([`runs_detached`]), and one
/// the module does not serve, or every one where it does not run. Workflow #434 LAB-5G4b: every
/// other row, a row of no module this process serves itself, goes to a `serve` child
/// ([`Serving::Child`]): the three `app` rows, `standard-pack check`, the debugging and authoring
/// rows actinglab's own commands serve, and `record draft`, whose refusal the child gives until
/// actinglab has a command for it.
pub(crate) fn serving(operation: &Operation) -> Serving {
    if actingcommand_lab_ops::core_ops::serves(operation) {
        return Serving::Core;
    }
    if operation.module == OperationModule::Rescue {
        if rescue_serves(&operation.name) && !runs_detached(&operation.name) {
            return Serving::Rescue;
        }
        return Serving::Refused;
    }
    Serving::Child
}

/// Workflow #434 LAB-5G2b, second part (the coordinator's ruling of 2026-11-12, not the owner's
/// words): whether the front forces the row `row` to run detached, by [`SUPPLIED`] (a parameter
/// it gives [`Supplied::True`], the row's `detach`: `deploy` and `rollback`). Such a row is
/// refused and starts nothing: the rescue module's detached run is an `actinglab.exe` handed the
/// initiator in `ACTINGCOMMAND_INITIATOR`, which refuses `Lab-MCP` there (`lab_ops::initiator`,
/// `of_lab_process`), so the operation would answer "detached" and never run. How a detached run
/// carries `Lab-MCP` is decided with LAB-6.
pub(crate) fn runs_detached(row: &str) -> bool {
    SUPPLIED
        .iter()
        .any(|(of, _, supplied)| *of == row && *supplied == Supplied::True)
}

/// Workflow #434 LAB-5G2b: whether the table marks `operation` as writing to the Runtime: it
/// writes and it reaches the Runtime. Its calls record today's provenance first.
pub(crate) fn writes_to_runtime(operation: &Operation) -> bool {
    operation.writes && operation.runtime != RuntimeNeed::Offline
}

/// Whether `param` is required through MCP: by its row, or by its `required_in` naming this
/// front end.
pub(crate) fn required_here(param: &Param) -> bool {
    param.required
        || param
            .required_in
            .as_ref()
            .is_some_and(|fronts| fronts.iter().any(|front| front == FRONT_END))
}

/// Whether a parameter type is a path (`path`, `file`, `dir`).
pub(crate) fn is_path(value_type: &ParamType) -> bool {
    matches!(
        value_type,
        ParamType::Path | ParamType::File | ParamType::Dir
    )
}

/// What the front gives the parameter `param` of the row `row` itself, if anything.
pub(crate) fn supplied(row: &str, param: &str) -> Option<Supplied> {
    SUPPLIED
        .iter()
        .find(|(of, name, _)| *of == row && *name == param)
        .map(|(_, _, supplied)| *supplied)
}

/// Whether the row `row` takes [`EXPORT`].
pub(crate) fn takes_export(row: &str) -> bool {
    SUPPLIED
        .iter()
        .any(|(of, _, supplied)| *of == row && *supplied == Supplied::ExportDirectoryOnExport)
}

/// The description of a path parameter on a row this build serves: its note, and
/// [`ABSOLUTE_PATH`].
pub(crate) fn absolute_note(note: Option<&str>) -> String {
    match note {
        Some(note) => format!("{note}; {ABSOLUTE_PATH}"),
        None => ABSOLUTE_PATH.to_owned(),
    }
}

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
    /// Workflow #434 LAB-5G2b: its row, which a call of the tool serves.
    operation: Operation,
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
            let definition = definition(operation, &name, DEFAULT_CALL_BUDGET_S)?;
            tools.push(Tool {
                name,
                module: operation.module,
                definition,
                operation: operation.clone(),
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

    /// Workflow #434 LAB-5G3: the same tools with the front's waits ([`ADDED`]) for a call budget of
    /// `budget_s` seconds (the start option `--call-budget`), where [`Self::of`] has today's.
    pub(crate) fn with_call_budget(mut self, budget_s: u64) -> Result<Self, Outcome> {
        for tool in &mut self.tools {
            tool.definition = definition(&tool.operation, &tool.name, budget_s)?;
        }
        Ok(self)
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

    /// Workflow #434 LAB-5G2b: the row whose tool is `name`, when it is an offered row's.
    pub(crate) fn operation(&self, name: &str) -> Option<&Operation> {
        self.tools
            .iter()
            .find(|tool| tool.name == name)
            .map(|tool| &tool.operation)
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

/// Workflow #434 LAB-5G2b, second part: whether the rescue module serves the rescue row `name`,
/// on Windows, where it runs.
#[cfg(windows)]
fn rescue_serves(name: &str) -> bool {
    actingcommand_lab_rescue::rescue::serves(name)
}

/// Elsewhere it serves none.
#[cfg(not(windows))]
fn rescue_serves(_name: &str) -> bool {
    false
}

/// The tool of `operation`: its entry of [`NAMED`], else the table's rule.
pub(crate) fn tool_name(operation: &Operation) -> String {
    NAMED
        .iter()
        .find(|(row, _)| *row == operation.name)
        .map_or_else(|| operation.mcp_tool(), |(_, tool)| (*tool).to_owned())
}

/// The tool definition of `operation`, named `name`, for a call budget of `budget_s` seconds; the
/// same in every protocol version.
fn definition(operation: &Operation, name: &str, budget_s: u64) -> Result<Value, Outcome> {
    let mut definition = Map::new();
    definition.insert("name".to_owned(), json!(name));
    if let Some(help) = &operation.help {
        definition.insert("description".to_owned(), json!(help));
    }
    definition.insert("inputSchema".to_owned(), input_schema(operation, budget_s)?);
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

/// The input schema of `operation`: one property per parameter, in the row's order. Workflow
/// #434 LAB-5G2b: but a parameter the front gives itself ([`SUPPLIED`]), and with [`EXPORT`]
/// after them where the row takes it; on a row this build serves, a path parameter's description
/// says that it takes an absolute path ([`absolute_note`]). Workflow #434 LAB-5G3: then the members
/// the front adds to the row ([`ADDED`]), in that table's order, for a call budget of `budget_s`.
fn input_schema(operation: &Operation, budget_s: u64) -> Result<Value, Outcome> {
    let mut properties = Map::new();
    let mut required = Vec::new();
    let served = serving(operation) != Serving::Refused;
    for param in &operation.params {
        if supplied(&operation.name, &param.name).is_some() {
            continue;
        }
        let mut property = param_schema(param)?;
        if served && is_path(&param.value_type) {
            property["description"] = json!(absolute_note(param.note.as_deref()));
        }
        properties.insert(param.name.clone(), property);
        if required_here(param) {
            required.push(Value::String(param.name.clone()));
        }
    }
    if takes_export(&operation.name) {
        properties.insert(
            EXPORT.to_owned(),
            json!({"type": "boolean", "description": EXPORT_NOTE}),
        );
    }
    for (_, member, added) in ADDED.iter().filter(|(row, _, _)| *row == operation.name) {
        properties.insert((*member).to_owned(), added_schema(*added, budget_s));
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

/// The refusal of a call of the tool `tool` of a row this build does not serve ([`serving`];
/// since Workflow #434 LAB-5G4b, of the offered rows, only `deploy` and `rollback`):
/// `mcp_operation_unsupported` with the tool (`mcp_tool`).
pub(crate) fn unsupported(tool: &str) -> Outcome {
    Outcome::new(McpCode::OperationUnsupported).mcp_tool(tool)
}
