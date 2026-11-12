// SPDX-License-Identifier: AGPL-3.0-only

//! A call of one of this server's tools (Workflow #434 LAB-5G2b; register §1.287, "Slice
//! LAB-5G2b"; `contracts/lab-two-entries.md` §9.3; the survey's §2 to §4 as the coordinator's
//! rulings amend them).
//!
//! - **Which rows are served** (`tools::serving`): a row `lab_ops::core_ops` serves, by
//!   `core_ops` in this process ([`Service::core`]); and (the slice's second part) on Windows a
//!   rescue row the rescue module serves, by the module in this process with the initiator
//!   `Lab-MCP` ([`rescue_line`]), but `deploy` and `rollback`, whose run the front forces
//!   detached. Every other call answers `mcp_operation_unsupported`, its arguments unread, as
//!   before.
//! - **The arguments** ([`prepare`]), before anything is sent or written: a member the row does
//!   not declare (`Operation::undeclared`), and a parameter the front gives itself, is refused as
//!   today's server refuses an argument it does not know (`arguments_invalid`, `unknown`); so is a
//!   member of the wrong type (`wrong_type`; text that is none of its words or no duration
//!   `not_recognised`) and a required one missing (`required`); a relative path where the
//!   parameter's type is a path is `not_absolute`, its developer text saying that an absolute
//!   path is required; a string that begins with `--` is refused as `serve` refuses it
//!   (`actingctl_usage_invalid`, `not_recognised`). A member given as `null` is absent, as today's
//!   server reads it. Then the front's own members (`tools::SUPPLIED`, `tools::EXPORT`).
//! - **The answer** ([`call_result`]) is the line `actinglab --json` prints for the same row and
//!   parameters: the text content and, from 2025-06-18 on, `structuredContent`; `isError` true
//!   exactly when the command line would exit non-zero; within the answer budget
//!   ([`crate::budget`]), which the serving code also cuts or exports a shaped answer to.
//! - **Jobs** (Workflow #434 LAB-5G3; `crate::jobs`, `crate::server`): every served call runs as
//!   a job; what the server's jobs, waits and cursors ask of the serving code is the last part of
//!   this module ([`front_members`], [`finished`], [`Service::watched`], [`Service::taken_back`],
//!   [`Service::run_start`]).

use crate::budget;
use crate::protocol::Era;
use crate::runtime::{self, Runtime};
use crate::tools::{self, Added, Serving, Supplied};
use actingcommand_contract::outcome::Outcome;
use actingcommand_lab_ops::budget::Budget;
use actingcommand_lab_ops::cli_line::{self, Exit};
use actingcommand_lab_ops::code_lookup;
use actingcommand_lab_ops::codes::{ActingctlCode, ArgumentProblem};
use actingcommand_lab_ops::core_ops::{self, Caller};
use actingcommand_lab_ops::events::{self, CursorBinding};
use actingcommand_lab_ops::front::{Connection, Front};
use actingcommand_lab_ops::table::{Operation, Param, ParamForm, ParamType, RuntimeNeed};
use actingcommand_runtime_client::{PreparedContainedTask, RuntimeClient};
use serde_json::{Map, Value, json};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The parameter of the instance a write names in its client action.
const INSTANCE: &str = "instance";

/// What serves the calls: this server's Runtime connection and its export directory.
pub(crate) struct Service {
    runtime: Runtime,
    exports: PathBuf,
}

impl Service {
    /// A service whose exports go to `exports` (`budget::export_directory` in the server).
    pub(crate) fn new(exports: PathBuf) -> Self {
        Self {
            runtime: Runtime::new(),
            exports,
        }
    }

    /// The `tools/call` result of a call of `operation`'s tool `tool` with `arguments`, in `era`.
    pub(crate) fn call(
        &self,
        operation: &Operation,
        tool: &str,
        arguments: &Map<String, Value>,
        era: Era,
    ) -> Value {
        call_result(&self.line(operation, tool, arguments, era), era)
    }

    /// The line of that call: `mcp_operation_unsupported` for a row this build does not serve;
    /// else the refusal of its arguments, or the line of the code that serves it.
    pub(crate) fn line(
        &self,
        operation: &Operation,
        tool: &str,
        arguments: &Map<String, Value>,
        era: Era,
    ) -> Exit {
        let serving = tools::serving(operation);
        if serving == Serving::Refused {
            return cli_line::failure(&tools::unsupported(tool), None);
        }
        match prepare(operation, arguments, &self.exports) {
            Ok(params) if serving == Serving::Rescue => rescue_line(operation, &params),
            Ok(params) => self.core(operation, tool, &params, era),
            Err(refused) => cli_line::failure(&refused.outcome, refused.text),
        }
    }

    /// `body` with this server's answer budget in `era`: the measure of the line as sent
    /// (`budget::fits`) and the export to this service's directory.
    pub(crate) fn within_budget<R>(&self, era: Era, body: impl FnOnce(&Budget<'_>) -> R) -> R {
        let fits = |answer: &Value| budget::fits(answer, era);
        let write = |sha256: &str, extension: &str, bytes: &[u8]| {
            budget::write_export(&self.exports, sha256, extension, bytes)
        };
        body(&Budget::new(&fits).with_export(&write))
    }

    /// A core row, served by `lab_ops::core_ops` in this process. A row that needs no Runtime,
    /// and one whose state root this server could not locate, is served as the command line
    /// serves it (the serving code refuses an unlocated state root before anything is sent). A
    /// row that writes to the Runtime goes on a write connection of its own ([`Self::write`]);
    /// any other on the kept connection. A Runtime that does not answer the kept connection is
    /// left to the serving code, which connects as the command line does and answers its line
    /// (`status` reports the Runtime offline; any other row the client's failure).
    fn core(
        &self,
        operation: &Operation,
        tool: &str,
        params: &Map<String, Value>,
        era: Era,
    ) -> Exit {
        self.within_budget(era, |answer_budget| {
            let place = runtime::place();
            let state_root = match (&place.state_root, operation.runtime) {
                (Ok(state_root), RuntimeNeed::Required | RuntimeNeed::Optional) => {
                    state_root.clone()
                }
                _ => {
                    return self.served(
                        operation,
                        params,
                        Connection::Connect(Front::Mcp),
                        self.runtime.generation(),
                        answer_budget,
                    );
                }
            };
            if tools::writes_to_runtime(operation) {
                return self.write(operation, tool, params, &state_root, answer_budget);
            }
            match self.runtime.kept(&state_root) {
                Ok(kept) => {
                    let exit = self.served(
                        operation,
                        params,
                        Connection::Kept(&kept.client),
                        kept.generation,
                        answer_budget,
                    );
                    self.runtime.settle(&kept);
                    exit
                }
                Err(_) => self.served(
                    operation,
                    params,
                    Connection::Connect(Front::Mcp),
                    self.runtime.generation(),
                    answer_budget,
                ),
            }
        })
    }

    /// A row that writes to the Runtime: today's provenance on a connection of its own
    /// (`runtime::begin_write`, the instance it names resolved as today's server resolves it),
    /// then the row on that connection; the provenance's warnings join the line's. A step of the
    /// provenance that fails ends the call with its line, and the row is not served.
    fn write(
        &self,
        operation: &Operation,
        tool: &str,
        params: &Map<String, Value>,
        state_root: &Path,
        answer_budget: &Budget<'_>,
    ) -> Exit {
        let alias = match instance_of(operation, params) {
            Some(selector) => match self.runtime.alias(state_root, selector) {
                Ok(alias) => Some(alias),
                Err(ended) => return *ended,
            },
            None => None,
        };
        let write = match runtime::begin_write(state_root, tool, alias.as_deref()) {
            Ok(write) => write,
            Err(ended) => return *ended,
        };
        let exit = self.served(
            operation,
            params,
            Connection::Kept(&write.client),
            self.runtime.generation(),
            answer_budget,
        );
        with_warnings(exit, write.warnings)
    }

    /// The row's line from `lab_ops::core_ops` on `connection`, within `answer_budget`, its
    /// cursors bound to this server's session and the connection's `generation`.
    fn served(
        &self,
        operation: &Operation,
        params: &Map<String, Value>,
        connection: Connection<'_>,
        generation: u64,
        answer_budget: &Budget<'_>,
    ) -> Exit {
        let caller = Caller {
            place: &runtime::place,
            unlocated: runtime::UNLOCATED,
            connection,
            budget: Some(answer_budget),
            cursor: CursorBinding {
                session: self.runtime.session(),
                generation,
            },
        };
        core_ops::answer(operation, params, &caller)
    }
}

/// Workflow #434 LAB-5G2b, second part: a rescue row, served by the rescue module in this process
/// for the root this program runs from, with the initiator `Lab-MCP`
/// (`lab_rescue::rescue::answer_here`), its answer the line actinglab's `--json` and `serve
/// --stdio` print for it (`apps/actinglab/src/cli_module.rs`, `rescue::line`): a line as it is, a
/// notice the line `agent_channel_reported` with its `text`, a hook that did its work that line
/// without members, a registered session's failed hook its failure, refused. No provenance is
/// recorded here: the module makes its own connection on its own channel and its journal records
/// the initiator (ruling 17 of 6775192250). Its budget is the core rows' (`call_result`).
#[cfg(windows)]
fn rescue_line(operation: &Operation, params: &Map<String, Value>) -> Exit {
    use actingcommand_lab_ops::initiator::Initiator;
    use actingcommand_lab_rescue::agent_channel::Answer;
    match actingcommand_lab_rescue::rescue::answer_here(&operation.name, params, Initiator::LabMcp)
    {
        Answer::Line(exit) => exit,
        Answer::Text(text) => {
            let mut members = Map::new();
            members.insert("text".to_owned(), Value::String(text.to_owned()));
            cli_line::success(members, &Outcome::new(ActingctlCode::AgentChannelReported))
        }
        Answer::Silent => cli_line::success(
            Map::new(),
            &Outcome::new(ActingctlCode::AgentChannelReported),
        ),
        Answer::Hook { outcome, text } => cli_line::refused(&outcome, Some(&text)),
    }
}

/// Elsewhere the rescue module does not run, and `tools::serving` serves no rescue row: the call
/// is refused as one this build does not serve.
#[cfg(not(windows))]
fn rescue_line(operation: &Operation, _params: &Map<String, Value>) -> Exit {
    cli_line::failure(&tools::unsupported(&tools::tool_name(operation)), None)
}

/// The instance a write names, when its row declares the parameter and the call gives it.
fn instance_of<'p>(operation: &Operation, params: &'p Map<String, Value>) -> Option<&'p str> {
    let declared = operation
        .params
        .iter()
        .any(|param| param.name == INSTANCE && param.value_type == ParamType::Instance);
    if !declared {
        return None;
    }
    params.get(INSTANCE).and_then(Value::as_str)
}

/// `exit` with `warnings` after the line's own.
fn with_warnings(mut exit: Exit, warnings: Vec<Value>) -> Exit {
    if warnings.is_empty() {
        return exit;
    }
    if let Ok(Value::Object(line)) = &mut exit.line {
        match line.get_mut("warnings") {
            Some(Value::Array(given)) => given.extend(warnings),
            _ => {
                line.insert("warnings".to_owned(), Value::Array(warnings));
            }
        }
    }
    exit
}

/// The developer text of a relative path where an absolute one is required.
const ABSOLUTE_REQUIRED: &str = "an absolute path is required: a resident server's current \
     directory means nothing to its caller";

/// The refusal of a call's arguments: its outcome, and the developer text of its line.
#[derive(Debug)]
pub(crate) struct Refused {
    pub(crate) outcome: Outcome,
    pub(crate) text: Option<&'static str>,
}

impl From<Outcome> for Refused {
    fn from(outcome: Outcome) -> Self {
        Self {
            outcome,
            text: None,
        }
    }
}

/// The parameters the code that serves `operation` is given for `arguments`, or the refusal of
/// the first argument at fault; nothing is sent or written before it (see the module's
/// description). `exports` is this server's export directory, which the front gives `material`
/// with `export` and `frame` (`tools::SUPPLIED`).
pub(crate) fn prepare(
    operation: &Operation,
    arguments: &Map<String, Value>,
    exports: &Path,
) -> Result<Map<String, Value>, Refused> {
    let row = operation.name.as_str();
    let takes_export = tools::takes_export(row);
    let members = arguments
        .keys()
        .map(String::as_str)
        .filter(|member| !(takes_export && *member == tools::EXPORT));
    if let Some(member) = operation.undeclared(members).first() {
        return Err(invalid(member, ArgumentProblem::Unknown).into());
    }
    if let Some(member) = arguments
        .keys()
        .find(|member| tools::supplied(row, member).is_some())
    {
        return Err(invalid(member, ArgumentProblem::Unknown).into());
    }
    let mut export = false;
    let mut params = Map::new();
    for (member, value) in arguments {
        if value.is_null() {
            continue;
        }
        if takes_export && member == tools::EXPORT {
            export = value
                .as_bool()
                .ok_or_else(|| invalid(member, ArgumentProblem::WrongType))?;
            continue;
        }
        let Some(param) = operation.params.iter().find(|param| &param.name == member) else {
            return Err(invalid(member, ArgumentProblem::Unknown).into());
        };
        checked(param, value).map_err(|problem| invalid(member, problem))?;
        if flag_like(value) {
            // As `serve` refuses it (actinglab's `cli_module::words::from_json`, `not_a_flag`).
            return Err(cli_line::usage_invalid(
                Some(member.as_str()),
                ArgumentProblem::NotRecognised,
            )
            .into());
        }
        if relative_path(param, value) {
            return Err(Refused {
                outcome: invalid(member, ArgumentProblem::NotAbsolute),
                text: Some(ABSOLUTE_REQUIRED),
            });
        }
        params.insert(member.clone(), value.clone());
    }
    if let Some(param) = operation.params.iter().find(|param| {
        tools::required_here(param)
            && tools::supplied(row, &param.name).is_none()
            && !params.contains_key(&param.name)
    }) {
        return Err(invalid(&param.name, ArgumentProblem::Required).into());
    }
    for (_, param, supplied) in tools::SUPPLIED.iter().filter(|(of, _, _)| *of == row) {
        let value = match supplied {
            Supplied::ExportDirectory => directory(exports)?,
            Supplied::ExportDirectoryOnExport if export => directory(exports)?,
            Supplied::ExportDirectoryOnExport => continue,
            Supplied::True => Value::Bool(true),
        };
        params.insert((*param).to_owned(), value);
    }
    Ok(params)
}

/// The export directory as a parameter's value: its text, which must be UTF-8 to reach the
/// serving code whole (`material_export_failed` with its path otherwise).
fn directory(exports: &Path) -> Result<Value, Outcome> {
    exports
        .to_str()
        .map(|text| Value::String(text.to_owned()))
        .ok_or_else(|| Outcome::new(ActingctlCode::MaterialExportFailed).path(exports))
}

/// `arguments_invalid` for one argument (`argument`, as the call names it) and what is wrong
/// with it, as today's server refuses one.
fn invalid(argument: &str, problem: ArgumentProblem) -> Outcome {
    Outcome::new(ActingctlCode::ArgumentsInvalid)
        .argument(argument)
        .argument_problem(problem)
}

/// A parameter's JSON value against its declared type, as `serve` takes it: a switch a boolean,
/// a repeated parameter a list of at least one value of its type; a value of another JSON type
/// is `wrong_type`, text that is none of an enumeration's words, or no duration,
/// `not_recognised`.
fn checked(param: &Param, value: &Value) -> Result<(), ArgumentProblem> {
    let recognised = |fits: bool| {
        if fits {
            Ok(())
        } else {
            Err(ArgumentProblem::NotRecognised)
        }
    };
    let one = |value: &Value| -> Result<(), ArgumentProblem> {
        let fits = match &param.value_type {
            ParamType::Integer => value.is_i64() || value.is_u64(),
            ParamType::Unsigned => value.is_u64(),
            // A JSON number, or the decimal text the command line's words give.
            ParamType::Decimal => {
                value.as_f64().is_some_and(f64::is_finite)
                    || value.as_str().and_then(decimal).is_some()
            }
            ParamType::Boolean => value.is_boolean(),
            ParamType::Json => true,
            ParamType::ListOfStrings => value
                .as_array()
                .is_some_and(|items| items.iter().all(Value::is_string)),
            ParamType::Duration => {
                return match value.as_str() {
                    Some(text) => recognised(core_ops::parse_duration(text).is_some()),
                    None => Err(ArgumentProblem::WrongType),
                };
            }
            ParamType::Enum(words) => {
                return match value.as_str() {
                    Some(text) => recognised(words.iter().any(|known| known == text)),
                    None => Err(ArgumentProblem::WrongType),
                };
            }
            _ => value.is_string(),
        };
        if fits {
            Ok(())
        } else {
            Err(ArgumentProblem::WrongType)
        }
    };
    match (param.repeat, value) {
        (true, Value::Array(items)) if !items.is_empty() => items.iter().try_for_each(&one),
        (true, _) => Err(ArgumentProblem::WrongType),
        (false, _) if param.form == ParamForm::Switch => {
            if value.is_boolean() {
                Ok(())
            } else {
                Err(ArgumentProblem::WrongType)
            }
        }
        (false, value) => one(value),
    }
}

/// A decimal number as written: finite, as Rust reads an `f64`.
fn decimal(text: &str) -> Option<f64> {
    text.parse::<f64>().ok().filter(|number| number.is_finite())
}

/// Whether a value is a string that begins with `--`, or a list holding one: refused for every
/// row with the code and word `serve` answers for it since LAB-5G4a (`actingctl_usage_invalid`,
/// `not_recognised`), since the serving code turns many rows' parameters into command words,
/// where it would be read as a flag. Numbers, booleans, objects and strings that begin with one
/// dash pass.
fn flag_like(value: &Value) -> bool {
    let flag = |value: &Value| value.as_str().is_some_and(|text| text.starts_with("--"));
    match value {
        Value::Array(items) => items.iter().any(flag),
        other => flag(other),
    }
}

/// Whether a parameter whose type is a path is given a relative path (or a list holding one):
/// refused `not_absolute`, since a resident server's current directory means nothing to its
/// caller. Stricter than today's server, which resolves or passes relative paths.
fn relative_path(param: &Param, value: &Value) -> bool {
    if !tools::is_path(&param.value_type) {
        return false;
    }
    let relative = |value: &Value| {
        value
            .as_str()
            .is_some_and(|text| !Path::new(text).is_absolute())
    };
    match value {
        Value::Array(items) => items.iter().any(relative),
        other => relative(other),
    }
}

/// The `tools/call` result of `exit` in `era`: the line as the text content and, where the
/// version has it, as `structuredContent`, `isError` when the command line would exit non-zero,
/// within the limit (`budget::carried`); a line over it is `output_budget_exceeded` with both
/// numbers. A line that did not encode is `actingctl_output_failed`, as `serve` answers it. The
/// developer line goes to stderr, as `serve` writes it.
pub(crate) fn call_result(exit: &Exit, era: Era) -> Value {
    sent(&finished(exit), era)
        .unwrap_or_else(|sent_bytes| refusal_result(&budget::exceeded(sent_bytes), era))
}

/// Workflow #434 LAB-5G3: the line `exit` is answered with, once: a line that did not encode is
/// `actingctl_output_failed`, as `serve` answers it, and the developer line goes to stderr, as
/// `serve` writes it.
pub(crate) fn finished(exit: &Exit) -> Finished {
    let line = match &exit.line {
        Ok(line) => {
            if let Some(developer) = &exit.developer {
                eprintln!("{}", developer.line(crate::PROGRAM));
            }
            line.clone()
        }
        Err(text) => {
            eprintln!(
                "{}",
                cli_line::developer_text(crate::PROGRAM, &Exit::output_failed(), Some(text))
            );
            refusal_line(&Exit::output_failed())
        }
    };
    Finished {
        line,
        is_error: exit.code != 0,
    }
}

/// Workflow #434 LAB-5G3: the `tools/call` result carrying `finished` in `era` within the limit
/// (`budget::carried`), or, when it fits neither way, the bytes it takes as sent.
pub(crate) fn sent(finished: &Finished, era: Era) -> Result<Value, usize> {
    budget::carried(&finished.line, era, finished.is_error)
}

/// The `tools/call` result of a call `refusal` answers: the line of a refusal, `{outcome}`, as
/// text and, from 2025-06-18 on, as `structuredContent`; `isError` true.
pub(crate) fn refusal_result(refusal: &Outcome, era: Era) -> Value {
    let line = refusal_line(refusal);
    budget::tool_result(&line, era, line.to_string(), true)
}

/// `{outcome}`, the line of a refusal, in this server's detail.
fn refusal_line(refusal: &Outcome) -> Value {
    let mut line = Map::new();
    line.insert("outcome".to_owned(), tools::envelope(refusal));
    Value::Object(line)
}

// ---- Workflow #434 LAB-5G3 (register §1.287, "Slice LAB-5G3"): what the server's jobs, waits and
// cursors ask of the serving code. The jobs themselves are `crate::jobs`; when a call waits and
// what it answers, `crate::server`.

/// The command words that give a cursor back to the commands of `lab-ops`.
const CURSOR_FLAG: &str = "--cursor";

/// Workflow #434 LAB-5G3: a call's line once its job ended, as it is answered ([`finished`]): the
/// line, and whether the command line would exit non-zero for it.
#[derive(Debug, Clone)]
pub(crate) struct Finished {
    pub(crate) line: Value,
    pub(crate) is_error: bool,
}

/// Workflow #434 LAB-5G3: the members the front adds to a row's tool beside its parameters
/// (`tools::ADDED`), as a call gives them.
#[derive(Debug, Default)]
pub(crate) struct FrontMembers {
    /// `wait_s`, or its default, on a row whose tool takes it.
    pub(crate) wait: Option<Duration>,
    /// `cursor`, when it is given.
    pub(crate) cursor: Option<String>,
}

/// Workflow #434 LAB-5G3: the members the front adds to `operation`'s tool, read from `arguments`
/// and taken out of them, and the rest, the row's, which [`prepare`] reads. `wait_s` is a whole
/// number of seconds from 0 to the call budget (`call_budget`), its default when it is absent or
/// `null` (never more than the budget); `cursor` is text. A value of another JSON type is
/// `arguments_invalid` `wrong_type`, a wait outside its range `out_of_range` with the range, and a
/// cursor that begins with `--` is refused as `serve` refuses such a value. Nothing is sent or
/// written before.
pub(crate) fn front_members(
    operation: &Operation,
    mut arguments: Map<String, Value>,
    call_budget: Duration,
) -> Result<(FrontMembers, Map<String, Value>), Refused> {
    let mut members = FrontMembers::default();
    let budget_s = call_budget.as_secs();
    for (_, member, added) in tools::ADDED
        .iter()
        .filter(|(row, _, _)| *row == operation.name)
    {
        let value = arguments.remove(*member).filter(|value| !value.is_null());
        match added {
            Added::Wait { default_s, .. } => {
                let seconds = match value {
                    None => (*default_s).min(budget_s),
                    Some(value) => match value.as_u64() {
                        Some(seconds) if seconds <= budget_s => seconds,
                        Some(_) => return Err(out_of_range(member, budget_s).into()),
                        None if value.is_i64() => return Err(out_of_range(member, budget_s).into()),
                        None => return Err(invalid(member, ArgumentProblem::WrongType).into()),
                    },
                };
                members.wait = Some(Duration::from_secs(seconds));
            }
            Added::Cursor { .. } => {
                let Some(value) = value else {
                    continue;
                };
                if flag_like(&value) {
                    return Err(cli_line::usage_invalid(
                        Some(*member),
                        ArgumentProblem::NotRecognised,
                    )
                    .into());
                }
                let cursor = value
                    .as_str()
                    .ok_or_else(|| invalid(member, ArgumentProblem::WrongType))?;
                members.cursor = Some(cursor.to_owned());
            }
        }
    }
    Ok((members, arguments))
}

/// `arguments_invalid` `out_of_range` for a wait of more than the call budget, `budget_s`, or
/// less than nothing, with the range.
fn out_of_range(member: &str, budget_s: u64) -> Outcome {
    invalid(member, ArgumentProblem::OutOfRange)
        .range_min(0_u64)
        .range_max(budget_s)
}

/// A line `lab-ops`' command built; a command that did not take its first word is a defect of
/// this server, answered as `lab_ops::core_ops` answers it.
fn not_served(exit: Option<Exit>) -> Exit {
    exit.unwrap_or_else(|| {
        cli_line::failure(
            &cli_line::usage_invalid(None, ArgumentProblem::Unknown),
            Some("the operation's command was not served"),
        )
    })
}

/// Workflow #434 LAB-5G3: a run prepared for `ac_run_start` on a connection of its own, after
/// today's provenance, and not yet submitted.
pub(crate) struct StartedRun {
    client: RuntimeClient,
    prepared: PreparedContainedTask,
    /// The warnings recording the provenance gave (each `{outcome}`), which the run's lines carry.
    pub(crate) warnings: Vec<Value>,
}

impl StartedRun {
    /// The run's Runtime request id, its handle.
    pub(crate) fn request_id(&self) -> Value {
        json!(self.prepared.request_id())
    }

    pub(crate) fn correlation_id(&self) -> Value {
        json!(self.prepared.correlation_id())
    }

    /// Submits the run and waits for its receipt: the line `run start` gives for it, with the
    /// provenance's warnings after its own.
    pub(crate) fn submitted(self) -> Exit {
        let exit = core_ops::run_start_ended(self.client.submit_prepared(self.prepared));
        with_warnings(exit, self.warnings)
    }
}

impl Service {
    /// Workflow #434 LAB-5G3: whether a call of `operation` with `arguments` (the row's, without
    /// the front's added members) is answered at once, with no job: a row this build does not
    /// serve, or arguments [`prepare`] refuses. Nothing is sent or written for it.
    pub(crate) fn refuses(&self, operation: &Operation, arguments: &Map<String, Value>) -> bool {
        tools::serving(operation) == Serving::Refused
            || prepare(operation, arguments, &self.exports).is_err()
    }

    /// Workflow #434 LAB-5G3: the line of a read (`run show`) served as [`Self::line`] serves it,
    /// but read again on the one kept connection as long as `again` says after each line (a run
    /// read every second while it may change, today's `ac_get_run`), so that no `Health` comes
    /// between the reads. Without a located state root or an answering Runtime it is read once, as
    /// [`Self::line`] reads it.
    pub(crate) fn watched(
        &self,
        operation: &Operation,
        tool: &str,
        arguments: &Map<String, Value>,
        era: Era,
        again: &mut dyn FnMut(&Exit) -> bool,
    ) -> Exit {
        let params = match prepare(operation, arguments, &self.exports) {
            Ok(params) => params,
            Err(refused) => return cli_line::failure(&refused.outcome, refused.text),
        };
        let place = runtime::place();
        let Ok(state_root) = &place.state_root else {
            return self.line(operation, tool, arguments, era);
        };
        let Ok(kept) = self.runtime.kept(state_root) else {
            return self.line(operation, tool, arguments, era);
        };
        let exit = self.within_budget(era, |answer_budget| {
            loop {
                let exit = self.served(
                    operation,
                    &params,
                    Connection::Kept(&kept.client),
                    kept.generation,
                    answer_budget,
                );
                if !again(&exit) {
                    break exit;
                }
            }
        });
        self.runtime.settle(&kept);
        exit
    }

    /// Workflow #434 LAB-5G3 (work order point 6): a call of `events` or `codes` that gives a
    /// cursor back, served on the layer below the row with the command's words and `--cursor`,
    /// within this server's budget. `codes`' cursor carries its own position and filters and binds
    /// to no session: its words are the command line's (`core_ops::offline_words`). `events`'
    /// cursor is taken under this server's binding, its session and the kept connection's
    /// generation (register "Slice LAB-5G1b", L5G1b-3; `events::dispatch_within`), so one issued
    /// before the connection was made again is `cursor_invalid`, nothing read.
    pub(crate) fn taken_back(
        &self,
        operation: &Operation,
        arguments: &Map<String, Value>,
        cursor: &str,
        era: Era,
    ) -> Exit {
        let params = match prepare(operation, arguments, &self.exports) {
            Ok(params) => params,
            Err(refused) => return cli_line::failure(&refused.outcome, refused.text),
        };
        let cursor_words = [OsString::from(CURSOR_FLAG), OsString::from(cursor)];
        self.within_budget(era, |answer_budget| {
            if let Some(mut argv) = core_ops::offline_words(operation, &params, None) {
                argv.extend(cursor_words);
                return not_served(code_lookup::dispatch_within(&argv, Some(answer_budget)));
            }
            let place = runtime::place();
            let state_root = match &place.state_root {
                Ok(state_root) => state_root.clone(),
                Err(outcome) => return cli_line::failure(outcome, Some(runtime::UNLOCATED)),
            };
            let mut argv = match core_ops::handed_words(
                operation,
                &params,
                place.root.as_deref(),
                &state_root,
            ) {
                Some(Ok(argv)) => argv,
                Some(Err(exit)) => return exit,
                None => return not_served(None),
            };
            argv.extend(cursor_words);
            let binding = |generation| CursorBinding {
                session: self.runtime.session(),
                generation,
            };
            match self.runtime.kept(&state_root) {
                Ok(kept) => {
                    let exit = not_served(events::dispatch_within(
                        &argv,
                        Connection::Kept(&kept.client),
                        Some(answer_budget),
                        binding(kept.generation),
                    ));
                    self.runtime.settle(&kept);
                    exit
                }
                Err(_) => not_served(events::dispatch_within(
                    &argv,
                    Connection::Connect(Front::Mcp),
                    Some(answer_budget),
                    binding(self.runtime.generation()),
                )),
            }
        })
    }

    /// Workflow #434 LAB-5G3 (work order point 5; today's `ac_run_pack`, `operator.rs`): `run
    /// start` prepared and not submitted: the instance resolved as a write's (one status read on
    /// the kept connection), today's provenance recorded on a connection of its own
    /// (`runtime::begin_write`), then the run read as `run start` reads it and prepared on that
    /// connection (`core_ops::run_start_prepared`), named by its request id. A step that fails
    /// ends the call with its line, and nothing of the run is sent.
    pub(crate) fn run_start(
        &self,
        operation: &Operation,
        tool: &str,
        arguments: &Map<String, Value>,
    ) -> Result<StartedRun, Exit> {
        let params = prepare(operation, arguments, &self.exports)
            .map_err(|refused| cli_line::failure(&refused.outcome, refused.text))?;
        let place = runtime::place();
        let state_root = match &place.state_root {
            Ok(state_root) => state_root.clone(),
            Err(outcome) => return Err(cli_line::failure(outcome, Some(runtime::UNLOCATED))),
        };
        let alias = match instance_of(operation, &params) {
            Some(selector) => Some(
                self.runtime
                    .alias(&state_root, selector)
                    .map_err(|ended| *ended)?,
            ),
            None => None,
        };
        let write =
            runtime::begin_write(&state_root, tool, alias.as_deref()).map_err(|ended| *ended)?;
        let prepared = core_ops::run_start_prepared(&params, &write.client)?;
        Ok(StartedRun {
            client: write.client,
            prepared,
            warnings: write.warnings,
        })
    }
}
