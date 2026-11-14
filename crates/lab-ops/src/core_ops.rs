// SPDX-License-Identifier: AGPL-3.0-only

//! The core operations of the CLI module (Workflow #434 LAB-2), and the four debugging operations
//! whose source is no actinglab command (`run start`, `run stop`, `reset`, `drill`). Every core
//! row is served by `crates/lab-ops` (`served::SERVED`):
//! - the commands LAB-1 moved there with their own parsing (`dispatch`) are given actingctl's
//!   words for the operation's parameters and answer the line actingctl prints for them, on the
//!   Lab channel (`lab_ops::front`);
//! - every other one is called here with its parameters and shaped as actingctl's CLI shapes it:
//!   its result beside its success code, a refused receipt beside the receipt's outcome, a
//!   client failure as the client's outcome. An operation whose only source was an MCP tool
//!   answers `lab_operation_completed`.
//!
//! The instance is the operation's positional, an alias, an `instance_id` or an ADB port; one
//! status read resolves it where the request needs the alias.
//!
//! Workflow #434 LAB-5G1 (register §1.287, "Slice LAB-5G1"): moved here from actinglab's CLI
//! module (`apps/actinglab/src/cli_module/core_ops.rs`), so that another program serves these rows
//! with the same code. What stays the program's is given by its caller ([`Caller`]): where the
//! program acts ([`Place`]) and the developer text of a state root it could not locate. The three
//! helpers of actinglab's words this code reads moved with it ([`MORE_WORDS`], [`further_flags`],
//! [`parse_duration`]). The caller also says how the rows reach the Runtime
//! ([`Caller::connection`]): a connection made for each call as a front connects, as actinglab
//! does, or a connection it keeps.
//!
//! Workflow #434 LAB-5G1b (register §1.287, "Slice LAB-5G1b"): the caller also gives the answer
//! budget ([`Caller::budget`]; none: the whole answer, as the command line prints it) and the
//! binding of the cursors `events` and `diagnose`'s errors page issue and take
//! ([`Caller::cursor`]). An answer whose only source was an MCP tool answers
//! `lab_operation_completed`, a code of this crate's registry since that slice, whichever program
//! serves the row.
//!
//! Workflow #434 LAB-5B1 (register §1.287, "Slice LAB-5B1"): the code reads a row's parameters by
//! name only through the row's constant in [`crate::core_reads`] (`core_reads::Params`), and
//! builds the command line a row hands to a command of this crate from that row's constant there
//! ([`core_reads::Handed`]); the slice's tests hold each constant equal to the row.

// Workflow #434 LAB-5G1: as in actinglab, where this code was written (its `main.rs` allows it for
// the crate): a step that ends the call returns the call's line, `Exit`, as its error.
#![allow(clippy::result_large_err)]

use crate::budget::{self, Budget};
use crate::cli_line::{self, Exit};
use crate::codes::{ActingctlCode, ArgumentProblem};
use crate::core_reads::{self, Handed, Params, Part, Written};
use crate::events::CursorBinding;
use crate::front::{Connection, Reached};
use crate::instance_selector::{self, SelectorMiss};
use crate::table::Operation;
use crate::wait::{Waited, wait_at_most};
use crate::{
    capability_gap, code_lookup, configure, devices, diagnose, emulators, events, evidence_read,
    fact_publish, facts, frames, ladder_drill, material, material_export, observe_export, overview,
    packs, perf_preset, remedy, routine_summary, run_evidence, run_list, run_show, schedule,
    scheduling, shutdown, targets, wait, wake_test,
};
use actingcommand_contract::codes::{ContractLocation, ReceiptCode};
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_contract::{
    ContainedTaskRecoveryBinding, ContainedTaskRequest, MAX_SCHEDULING_PAUSE_DRAIN_TIMEOUT_MS,
    MIN_SCHEDULING_PAUSE_DRAIN_TIMEOUT_MS, RequestId, RuntimeReceiptState,
    SchedulingPauseExpectation, SchedulingPauseReason,
};
use actingcommand_runtime_client::{
    ContainedTaskResetOutcome, PreparedContainedTask, RuntimeClient, RuntimeClientError,
    RuntimeFlowOutput,
};
use serde::Serialize;
use serde_json::{Map, Value};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The member a result that is not a JSON object goes under (actingctl's `RESULT_MEMBER`).
const RESULT_MEMBER: &str = "result";
/// `pause`'s drain when none is given (the operation table's default).
const DEFAULT_DRAIN: Duration = Duration::from_secs(300);
/// Workflow #434 LAB-5B1: the rows whose steps other programs call with the parameters alone
/// ([`stop_target`], [`pack_schedule_rows`], [`run_start_prepared`], [`material_read`]), which read
/// them through that row's constant.
const TASK_STOP: &str = "task stop";
const PACK_SCHEDULE_SET: &str = "pack-schedule set";
const RUN_START: &str = "run start";
const MATERIAL: &str = "material";

/// Workflow #434 LAB-5G1: where the program acts, as it finds it (actinglab's CLI module: from its
/// own place, `apps/actinglab/src/cli_module/place.rs`).
pub struct Place {
    /// The state root, or why it is unknown.
    pub state_root: Result<PathBuf, Outcome>,
    /// The install root, when the program runs from one.
    pub root: Option<PathBuf>,
}

/// Workflow #434 LAB-5G1: what the program that serves these rows gives them, which stays the
/// program's.
pub struct Caller<'a> {
    /// The program's place, asked for when a row needs it, as before: `packs` its install root
    /// first, then every row but `codes`, `gap` and `packs` its state root.
    pub place: &'a dyn Fn() -> Place,
    /// The developer text of a state root the program could not locate.
    pub unlocated: &'a str,
    /// How the rows that reach the Runtime reach it: a connection made for the call as a front
    /// connects (actinglab: [`Connection::per_call`]), or one the caller keeps. Every such row
    /// takes it, each of the three ways: the words of a command of this crate (its `dispatch_on`),
    /// and the parameters read here. `codes`, `gap`, `packs` and `devices` read no Runtime.
    pub connection: Connection<'a>,
    /// Workflow #434 LAB-5G1b: the answer budget. `None` (actinglab): the whole answer, the line
    /// the command line prints. `Some`: handed to the step that shapes the answer of `status`,
    /// `diagnose`, `events`, `schedule`, `runs`, `run evidence` and `codes` (each cuts it) and of
    /// `facts`, `config`, `packs`, `remedy` and `summary` (each writes it whole through the
    /// budget's export and answers its header, `answer_exported` the line's one warning,
    /// [`budget::line_members`]). Every other row answers whole whatever the budget.
    pub budget: Option<&'a Budget<'a>>,
    /// Workflow #434 LAB-5G1b: the binding of the cursors `events` issues and takes and of the
    /// cursor of `diagnose`'s errors page: a session and a connection generation (actinglab: a
    /// fresh one for each call, [`CursorBinding::fresh`]). The cursors of `runs` and `codes` bind
    /// to no session and do not take it.
    pub cursor: CursorBinding,
}

/// The operations of this module: the core rows (but those actinglab's own command serves) and
/// `run start`, `run stop`, `reset`, `drill`.
pub fn serves(operation: &Operation) -> bool {
    let name = operation.name.as_str();
    if matches!(name, "run start" | "run stop" | "reset" | "drill") {
        return true;
    }
    // `app launch|restart|stop` are actinglab's own `session app` (the engine), and (Workflow #434
    // LAB-5F fix 1, a core row since the coordinator's ruling of 2026-11-11) `standard-pack check`
    // is actinglab's own `standard-pack check`; Workflow #464 P3: `standard-pack ready` likewise.
    operation.module == crate::table::OperationModule::Core
        && !matches!(
            name,
            "app launch"
                | "app restart"
                | "app stop"
                | "standard-pack check"
                | "standard-pack ready"
        )
}

/// The operation `operation` with `params`, as its line, for `caller`.
pub fn answer(operation: &Operation, params: &Map<String, Value>, caller: &Caller<'_>) -> Exit {
    let name = operation.name.as_str();
    // Offline operations: no state root; only `packs` reads the install root.
    let root = if name == "packs" {
        (caller.place)().root
    } else {
        None
    };
    if let Some(argv) = offline_words(operation, params, root.as_deref()) {
        return dispatched(match name {
            // Workflow #434 LAB-5G1b: within the caller's budget (`gap` has no shaping).
            "codes" => code_lookup::dispatch_within(&argv, caller.budget),
            "gap" => capability_gap::dispatch(&argv),
            _ => packs::dispatch_within(&argv, caller.budget),
        });
    }
    let located = (caller.place)();
    let state_root = match &located.state_root {
        Ok(state_root) => state_root.clone(),
        Err(outcome) => return cli_line::failure(outcome, Some(caller.unlocated)),
    };
    if let Some(exit) = dispatched_with_state_root(operation, params, &located, &state_root, caller)
    {
        return exit;
    }
    // The operations called here take only the row's parameters: a flag or word the row does not
    // declare is refused by name, never ignored (Workflow #434 LAB-5A: the words refuse it first,
    // since no core row has `params_more`).
    if let Some((further, _)) = further_flags(operation, params).first() {
        return usage(&flag_name(further), ArgumentProblem::Unknown);
    }
    // Workflow #434 LAB-5B1: from here every parameter is read by name through the row's constant
    // (`words`, which no row declares, is one of `core_reads::EXCEPTIONS`).
    let params = Params::of(&operation.name, params);
    if let Some(word) = list_of(params, MORE_WORDS).and_then(|words| words.first().cloned()) {
        return usage(&word, ArgumentProblem::Unknown);
    }
    let client = match connect(&state_root, caller.connection) {
        Ok(client) => client,
        Err(exit) => return exit,
    };
    let answered = if name == "shutdown" {
        shutdown_line(params, client, &state_root)
    } else {
        served(name, params, &client, caller.budget)
    };
    match answered {
        Ok(exit) | Err(exit) => exit,
    }
}

/// `shutdown [--wait]`: the receipt and, with a wait, the shutdown seen to complete. The client
/// is given to the wait, which closes its connection first (Workflow #434 LAB-5G1: a connection
/// made for this call; a kept one is the caller's, and the wait drops only a clone of it, so the
/// caller's connection stays open while the wait reads the shutdown).
fn shutdown_line(params: Params<'_>, client: Reached<'_>, state_root: &Path) -> Result<Exit, Exit> {
    let receipt = shutdown::request(&client).map_err(|error| client_failed(&error))?;
    let mut members = Map::new();
    members.insert(
        "receipt".to_owned(),
        serde_json::to_value(&receipt).map_err(|error| output_failed(&error))?,
    );
    let Some(wait) = duration(params, "wait")? else {
        return Ok(cli_line::success(
            members,
            &Outcome::new(ActingctlCode::RuntimeShutdownRequested),
        ));
    };
    let shutdown = shutdown::wait(client.into_client(), state_root, wait).map_err(|outcome| {
        cli_line::refused(
            &outcome,
            Some("the accepted shutdown was not seen to complete"),
        )
    })?;
    members.insert("shutdown".to_owned(), shutdown);
    Ok(cli_line::success(
        members,
        &Outcome::new(ActingctlCode::RuntimeShutdownCompleted),
    ))
}

/// Workflow #434 LAB-5C: the words of `codes`, `gap` and `packs`, the offline rows LAB-1 moved
/// with their parsing, as their command in `lab-ops` is given them (`root`: the install root, which
/// only `packs` reads); `None` for another row. Workflow #434 LAB-5B1: built from the row's
/// constant, `core_reads::OFFLINE`.
pub fn offline_words(
    operation: &Operation,
    params: &Map<String, Value>,
    root: Option<&Path>,
) -> Option<Vec<OsString>> {
    let handed = core_reads::OFFLINE
        .iter()
        .find(|handed| handed.row == operation.name)?;
    // No part of an offline row is refused while the words are built: none reads the clock
    // (`Written::SinceNow`, the one part that can be; the slice's test holds it).
    handed_line(
        handed,
        operation,
        Params::of(&operation.name, params),
        root,
        None,
    )
    .ok()
}

/// The operations LAB-1 moved with their parsing, given actingctl's words, on the caller's
/// connection; `None` for another. Workflow #434 LAB-5G1b: those that shape their answer within
/// the caller's budget, `events` and `diagnose` with the caller's cursor binding.
fn dispatched_with_state_root(
    operation: &Operation,
    params: &Map<String, Value>,
    place: &Place,
    state_root: &Path,
    caller: &Caller<'_>,
) -> Option<Exit> {
    let argv = match handed_words(operation, params, place.root.as_deref(), state_root)? {
        Ok(argv) => argv,
        Err(exit) => return Some(exit),
    };
    let detail = cli_line::detail().unwrap_or(Detail::Full);
    let (connection, budget, cursor) = (caller.connection, caller.budget, caller.cursor);
    let exit = match operation.name.as_str() {
        "status" => overview::dispatch_within(&argv, connection, budget),
        "summary" => routine_summary::dispatch_within(&argv, detail, connection, budget),
        "events" => events::dispatch_within(&argv, connection, budget, cursor),
        "runs" => run_list::dispatch_within(&argv, connection, budget),
        "run evidence" => run_evidence::dispatch_within(&argv, connection, budget),
        "diagnose" => diagnose::dispatch_within(&argv, connection, budget, cursor),
        // Workflow #434 LAB-5C: `remedy set`'s rows given inline go beside its words.
        "remedy set" => match Params::of(&operation.name, params).get("rows") {
            Some(rows) => remedy::dispatch_rows_on(&argv, rows, detail, connection),
            None => remedy::dispatch_on(&argv, detail, connection),
        },
        // Workflow #434 LAB-5G1b: the read within the caller's budget; `remedy lift` has none.
        "remedy" => remedy::dispatch_within(&argv, detail, connection, budget),
        "remedy lift" => remedy::dispatch_on(&argv, detail, connection),
        "frame" => observe_export::dispatch_on(&argv, connection),
        "preset" | "preset set" => perf_preset::dispatch_on(&argv, detail, connection),
        "evidence read" => evidence_read::dispatch_on(&argv, detail, connection),
        // Reads no Runtime: the state root's binding file and adb.
        "devices" | "devices bind" | "devices unbind" => devices::dispatch(&argv),
        _ => return None,
    };
    Some(dispatched(exit))
}

/// Workflow #434 LAB-5C: the words a row LAB-1 moved with its parsing gives its command in
/// `lab-ops` (actingctl's words for the row's parameters), built apart from the run so that a test
/// reads what a row hands on; `root` is the install root, `state_root` the state root. `None` for
/// a row served here with its parameters; a parameter refused while the words are built is its
/// line. `remedy set`'s rows given inline are no word: they go beside the words. Workflow #434
/// LAB-5B1: built from the row's constant, `core_reads::HANDED`; `frame` hands words only with
/// `out` (without it, it is served here).
pub fn handed_words(
    operation: &Operation,
    params: &Map<String, Value>,
    root: Option<&Path>,
    state_root: &Path,
) -> Option<Result<Vec<OsString>, Exit>> {
    let handed = core_reads::HANDED
        .iter()
        .find(|handed| handed.row == operation.name)?;
    let params = Params::of(&operation.name, params);
    if let Some(only_with) = handed.only_with
        && !params.contains_key(only_with)
    {
        return None;
    }
    Some(handed_line(
        handed,
        operation,
        params,
        root,
        Some(state_root),
    ))
}

/// Workflow #434 LAB-5B1: the command line the row's constant `handed` gives for `params`: the
/// command's words, then each part in its place (the state root when there is one, the install
/// root when the program runs from one); a parameter refused while it is written is its line.
fn handed_line(
    handed: &Handed,
    operation: &Operation,
    params: Params<'_>,
    root: Option<&Path>,
    state_root: Option<&Path>,
) -> Result<Vec<OsString>, Exit> {
    let mut argv = Argv::new(handed.command);
    for part in handed.parts {
        match *part {
            Part::Param(name, written) => argv.param(params, name, written)?,
            Part::StateRoot => {
                if let Some(state_root) = state_root {
                    argv.state_root(state_root);
                }
            }
            Part::Root => {
                if let Some(root) = root {
                    argv.flag(core_reads::ROOT_FLAG, root);
                }
            }
            // Workflow #434 LAB-5A fix 1: `devices bind`'s `--root` from the command line
            // (`words::KEPT_FLAGS`), after the module's own, as its further flags went before:
            // `devices` takes it only when the module located none.
            Part::Further => argv.further(operation, params.map()),
        }
    }
    Ok(argv.0)
}

/// The other operations, called with their parameters on `client`. Workflow #434 LAB-5G1b:
/// `facts`, `config` and `schedule` shape their answer within the caller's `budget`.
fn served(
    name: &str,
    params: Params<'_>,
    client: &RuntimeClient,
    budget: Option<&Budget<'_>>,
) -> Result<Exit, Exit> {
    Ok(match name {
        "facts" => {
            let prefix = text(params, "prefix").map(str::to_owned);
            let request = match (
                switch(params, "config"),
                switch(params, "program"),
                text(params, "instance"),
            ) {
                (true, false, None) => facts::FactsRequest::Config,
                (false, true, None) => facts::FactsRequest::Program { key_prefix: prefix },
                (false, false, Some(selector)) => facts::FactsRequest::Instance {
                    selector: selector.to_owned(),
                    key_prefix: prefix,
                },
                (false, false, None) => return Err(usage("<instance>", ArgumentProblem::Required)),
                _ => return Err(usage("--program", ArgumentProblem::Conflicting)),
            };
            let code = match request {
                facts::FactsRequest::Config => ActingctlCode::RuntimeConfigReported,
                facts::FactsRequest::Program { .. } => ActingctlCode::RuntimeFactsReported,
                facts::FactsRequest::Instance { .. } => ActingctlCode::InstanceFactsReported,
            };
            facts_line(client, &request, code, budget)?
        }
        "config" => facts_line(
            client,
            &facts::FactsRequest::Config,
            ActingctlCode::RuntimeConfigReported,
            budget,
        )?,
        "schedule" => {
            let explanation =
                schedule::explanation(client, text(params, "instance")).map_err(|failure| {
                    match failure {
                        schedule::Failure::Runtime(error) => client_failed(&error),
                        schedule::Failure::Selector { miss, selector } => {
                            selector_missed(miss, &selector)
                        }
                    }
                })?;
            // Workflow #434 LAB-5G1b: whole without a budget, as before; with one, its candidate
            // rows cut to it.
            match serde_json::to_value(&explanation) {
                Ok(answer) => cli_line::success(
                    cli_line::members(schedule::within_budget(answer, budget), RESULT_MEMBER),
                    &Outcome::new(ActingctlCode::RuntimeScheduleReported),
                ),
                Err(error) => output_failed(&error),
            }
        }
        "run show" => {
            let id = required_text(params, "id")?;
            let key = run_show::by_request_id(id)
                .or_else(|| run_show::by_run_id(id))
                .ok_or_else(|| usage("<id>", ArgumentProblem::NotRecognised))?;
            let status = run_show::read(client, key).map_err(|error| client_failed(&error))?;
            completed(run_show::answer(&status))
        }
        "material" => match material_read(params.map())? {
            MaterialRead::Text {
                reference,
                offset,
                max_bytes,
            } => {
                let found = material::parse_ref(reference)
                    .and_then(|(position, artifact_id)| {
                        material::resolve(client, position, artifact_id)
                    })
                    .and_then(|found| {
                        material::read_text(client, &found, offset, max_bytes, &|_: &Value| true)
                    })
                    .map_err(material_failed)?;
                completed(found)
            }
            // Workflow #434 LAB-5C: the whole material, read within the CLI's read budget (as
            // `frame --out` reads its frame) and written into `out`.
            MaterialRead::Whole { reference, out } => {
                let found = material::parse_ref(reference)
                    .and_then(|(position, artifact_id)| {
                        material::resolve(client, position, artifact_id)
                    })
                    .map_err(material_failed)?;
                let deadline = Instant::now() + observe_export::READ_BUDGET;
                let bytes = material::read_whole(client, &found, deadline, &|| true)
                    .map_err(material_failed)?;
                material_written(
                    &out,
                    &found.artifact.sha256,
                    found.artifact.kind.extension(),
                    &bytes,
                )
            }
        },
        "targets" => {
            let alias = alias_of(client, required_text(params, "instance")?)?;
            let view = targets::view(client, &alias).map_err(|error| client_failed(&error))?;
            completed(serde_json::to_value(&view).map_err(|error| output_failed(&error))?)
        }
        "targets set" => {
            let alias = alias_of(client, required_text(params, "instance")?)?;
            let document = match (text(params, "file"), params.get("targets")) {
                (Some(file), None) => targets::read_document(Path::new(file)).map_err(|problem| {
                    cli_line::refused(
                        &problem.attach(
                            Outcome::new(ActingctlCode::ResourceTargetsFileInvalid).path(file),
                        ),
                        Some("the policy file is unreadable, empty, larger than 65536 bytes or not UTF-8"),
                    )
                })?,
                (None, Some(Value::Array(list))) => {
                    let view =
                        targets::view(client, &alias).map_err(|error| client_failed(&error))?;
                    let valid_until = match unsigned(params, "valid_days")? {
                        Some(days) if (1..=targets::MAX_VALID_DAYS).contains(&days) => Some(
                            targets::valid_until(days).ok_or_else(|| {
                                cli_line::refused(
                                    &Outcome::new(ActingctlCode::ClockUnavailable),
                                    None,
                                )
                            })?,
                        ),
                        Some(_) => {
                            return Err(cli_line::failure(
                                &cli_line::usage_invalid(
                                    Some("--valid-days"),
                                    ArgumentProblem::OutOfRange,
                                )
                                .range_min(1_u64)
                                .range_max(targets::MAX_VALID_DAYS),
                                None,
                            ));
                        }
                        None => None,
                    };
                    targets::document(&view, valid_until, list)
                        .map_err(|outcome| cli_line::refused(&outcome, None))?
                }
                (None, Some(_)) => return Err(usage("--targets", ArgumentProblem::WrongType)),
                (Some(_), Some(_)) => {
                    return Err(usage("--targets", ArgumentProblem::Conflicting));
                }
                (None, None) => return Err(usage("<file>", ArgumentProblem::Required)),
            };
            match targets::apply(client, document) {
                Ok(applied) => cli_line::success(
                    targets::applied_answer(&applied).map_err(|error| output_failed(&error))?,
                    &Outcome::new(ActingctlCode::ResourceTargetsApplied),
                ),
                Err(error) => receipt_refused(error)?,
            }
        }
        "frame" => {
            let alias = alias_of(client, required_text(params, "instance")?)?;
            let frame =
                observe_export::observe(client, &alias).map_err(|error| client_failed(&error))?;
            reported(&frame, ActingctlCode::InstanceObserved)
        }
        "frames" => {
            let alias = alias_of(client, required_text(params, "instance")?)?;
            let count = match unsigned(params, "count")? {
                Some(count) => Some(
                    u16::try_from(count)
                        .map_err(|_| usage("--count", ArgumentProblem::OutOfRange))?,
                ),
                None => None,
            };
            let interval = duration(params, "interval")?.map(millis);
            let spec = frames::spec(count, interval)
                .ok_or_else(|| usage("--count", ArgumentProblem::OutOfRange))?;
            let captured =
                frames::capture(client, &alias, spec).map_err(|error| client_failed(&error))?;
            reported(&captured, ActingctlCode::InstanceFramesCaptured)
        }
        "discover" => {
            let discovery = emulators::discover(client).map_err(|error| client_failed(&error))?;
            let members = serde_json::to_value(&discovery)
                .map(|value| cli_line::members(value, RESULT_MEMBER))
                .map_err(|error| output_failed(&error))?;
            cli_line::success(
                members,
                &Outcome::new(ActingctlCode::InstancesDiscovered)
                    .count(discovery.instances().len()),
            )
        }
        "pause" => {
            let alias = optional_alias(client, params)?;
            let reason = SchedulingPauseReason::new(
                text(params, "reason").unwrap_or(scheduling::CLI_PAUSE_REASON),
            )
            .map_err(|_| usage("--reason", ArgumentProblem::NotRecognised))?;
            let drain = millis(duration(params, "drain")?.unwrap_or(DEFAULT_DRAIN));
            if !(MIN_SCHEDULING_PAUSE_DRAIN_TIMEOUT_MS..=MAX_SCHEDULING_PAUSE_DRAIN_TIMEOUT_MS)
                .contains(&drain)
            {
                return Err(cli_line::failure(
                    &cli_line::usage_invalid(Some("--drain"), ArgumentProblem::OutOfRange)
                        .range_min(MIN_SCHEDULING_PAUSE_DRAIN_TIMEOUT_MS)
                        .range_max(MAX_SCHEDULING_PAUSE_DRAIN_TIMEOUT_MS),
                    None,
                ));
            }
            let paused = scheduling::pause(client, scheduling::pause_scope(alias), reason, drain)
                .map_err(|error| client_failed(&error))?;
            reported(&paused, ActingctlCode::RuntimeSchedulingPaused)
        }
        "resume" => {
            let scope = scheduling::pause_scope(optional_alias(client, params)?);
            let resumed = match params.get("expect") {
                Some(expected) => {
                    let expected =
                        serde_json::from_value::<SchedulingPauseExpectation>(expected.clone())
                            .map_err(|_| usage("--expect", ArgumentProblem::WrongType))?;
                    scheduling::resume_expected(client, scope, expected)
                }
                None => scheduling::resume(client, scope),
            }
            .map_err(|error| client_failed(&error))?;
            reported(&resumed, ActingctlCode::RuntimeSchedulingResumed)
        }
        "config set" | "config clear" => {
            let action = if name == "config set" { "set" } else { "clear" };
            let member = if name == "config set" {
                "changes"
            } else {
                "keys"
            };
            let changes =
                configure::word_changes(action, &list_of(params, member).unwrap_or_default())
                    .ok_or_else(|| usage(&format!("<{member}>"), ArgumentProblem::NotRecognised))?;
            let expected = unsigned(params, "expected_version")?;
            match client.configure_settings(changes, expected) {
                Ok(configured) => cli_line::success(
                    configure::configured_answer(&configured)
                        .map_err(|error| output_failed(&error))?,
                    &Outcome::new(ActingctlCode::ConfigSettingsChanged),
                ),
                Err(error) => receipt_refused(error)?,
            }
        }
        "monitor set" => {
            let alias = alias_of(client, required_text(params, "instance")?)?;
            let recover = match params.get("recover") {
                None => false,
                Some(Value::Bool(recover)) => *recover,
                Some(_) => return Err(usage("--recover", ArgumentProblem::WrongType)),
            };
            let policy = configure::monitor_policy(
                duration(params, "interval")?.map(millis),
                text(params, "expect").map(str::to_owned),
                recover,
            )
            .ok_or_else(|| usage("--interval", ArgumentProblem::OutOfRange))?;
            let status = configure::set_monitor(client, &alias, policy)
                .map_err(|error| client_failed(&error))?;
            reported(&status, ActingctlCode::MonitorPolicySet)
        }
        "monitor clear" => {
            let alias = alias_of(client, required_text(params, "instance")?)?;
            let status =
                configure::clear_monitor(client, &alias).map_err(|error| client_failed(&error))?;
            reported(&status, ActingctlCode::MonitorPolicyCleared)
        }
        "offset" => {
            let task = required_text(params, "task")?;
            let offset = params
                .get("ms")
                .and_then(Value::as_i64)
                .ok_or_else(|| usage("<ms>", ArgumentProblem::Required))?;
            let alias = optional_alias(client, params)?;
            let published = scheduling::offset(client, task, offset, alias.as_deref()).map_err(
                |failure| match failure {
                    scheduling::OffsetFailure::Runtime(error) => client_failed(&error),
                    scheduling::OffsetFailure::InstanceUnknown(alias) => {
                        selector_missed(SelectorMiss::Unknown, &alias)
                    }
                    scheduling::OffsetFailure::ScopeAmbiguous => cli_line::refused(
                        &Outcome::new(ActingctlCode::TaskOffsetScopeAmbiguous),
                        Some("the registered instances do not name exactly one configured game; give the instance"),
                    ),
                    scheduling::OffsetFailure::Invalid(error) => cli_line::refused(
                        &Outcome::new(actingcommand_contract::codes::SharedCode::PriorityOffsetInvalid)
                            .caused_by(Outcome::new(error.code())),
                        Some("the task identifier cannot form a priority offset fact"),
                    ),
                    scheduling::OffsetFailure::Clock => cli_line::refused(
                        &Outcome::new(ActingctlCode::ClockUnavailable),
                        Some("the system clock is before the Unix epoch or out of range"),
                    ),
                },
            )?;
            cli_line::success(
                published.answer().map_err(|error| output_failed(&error))?,
                &Outcome::new(ActingctlCode::TaskOffsetPublished),
            )
        }
        "facts publish" => {
            let file = required_text(params, "file")?;
            let observation = fact_publish::read_observation(Path::new(file)).map_err(|problem| {
                cli_line::refused(
                    &problem.attach(Outcome::new(ActingctlCode::ActingctlFactRecordInvalid).path(file)),
                    Some("the fact observation file is unreadable, larger than its bound or invalid"),
                )
            })?;
            let event_id = fact_publish::publish(client, observation)
                .map_err(|error| client_failed(&error))?;
            let mut members = Map::new();
            members.insert(
                "event_id".to_owned(),
                serde_json::to_value(event_id).map_err(|error| output_failed(&error))?,
            );
            cli_line::success(members, &Outcome::new(ActingctlCode::AgentFactsPublished))
        }
        "emu start" | "emu stop" | "emu restart" => {
            let word = name.trim_start_matches("emu ");
            let action = emulators::action(word)
                .ok_or_else(|| usage(word, ArgumentProblem::NotRecognised))?;
            let alias = alias_of(client, required_text(params, "instance")?)?;
            match duration(params, "wait")? {
                None => {
                    let controlled = emulators::control(client, &alias, action)
                        .map_err(|error| client_failed(&error))?;
                    reported(&controlled, ActingctlCode::InstanceEmulatorControlled)
                }
                Some(wait) => match emulators::control_within(client, alias, action, wait)
                    .map_err(|error| client_failed(&error))?
                {
                    emulators::Bounded::Controlled(controlled) => {
                        reported(&controlled, ActingctlCode::InstanceEmulatorControlled)
                    }
                    emulators::Bounded::Expired { correlation_id } => expired(
                        ContractLocation::ControlEmulatorInstance,
                        wait,
                        None,
                        correlation_id,
                    ),
                },
            }
        }
        "emu selfcheck" => {
            let alias = alias_of(client, required_text(params, "instance")?)?;
            let result =
                emulators::self_check(client, &alias).map_err(|error| client_failed(&error))?;
            let outcome = emulators::self_check_failure(&result)
                .unwrap_or_else(|| Outcome::new(ActingctlCode::InstanceSelfCheckReported));
            let members = serde_json::to_value(&result)
                .map(|value| cli_line::members(value, RESULT_MEMBER))
                .map_err(|error| output_failed(&error))?;
            cli_line::success(members, &outcome)
        }
        "wake-test" => {
            let to = required_text(params, "channel")?;
            let queued = wake_test::queue(client, to, text(params, "instance"))
                .map_err(|error| client_failed(&error))?;
            reported(&queued, ActingctlCode::AgentWakeTestQueued)
        }
        "task stop" => task_stop(params, client)?,
        "pack-schedule" => pack_schedule_view(params, client)?,
        "pack-schedule set" => pack_schedule_set(params, client)?,
        "run start" => run_start(params, client)?,
        "run stop" => run_stop(params, client)?,
        "reset" => {
            let alias = alias_of(client, required_text(params, "instance")?)?;
            let reset = client
                .safe_reset(&alias)
                .map_err(|error| client_failed(&error))?;
            reported(&reset, ActingctlCode::InstanceResetDone)
        }
        "drill" => {
            let selector = required_text(params, "instance")?;
            let drill = ladder_drill::drill(
                client,
                selector,
                ladder_drill::mode(switch(params, "device")),
            )
            .map_err(|failure| match failure {
                ladder_drill::Failure::Selector { miss, selector } => {
                    selector_missed(miss, &selector)
                }
                ladder_drill::Failure::Runtime(error) => client_failed(&error),
            })?;
            reported(&drill, ActingctlCode::RecoveryLadderDrillQueued)
        }
        other => {
            return Err(cli_line::failure(
                &cli_line::usage_invalid(Some(other), ArgumentProblem::Unknown),
                None,
            ));
        }
    })
}

/// Workflow #429 R-6: `task stop <inst> <request> [--wait]`, `StopInstanceRun`: the Runtime stops
/// the run and resets the device itself, with the run's own lease. The answer is the stop and its
/// receipt's outcome, which carries a failed reset's failure as its cause; a refusal (a run no
/// client may stop, with `run_origin`) is the refused receipt beside its outcome.
fn task_stop(params: Params<'_>, client: &RuntimeClient) -> Result<Exit, Exit> {
    let alias = alias_of(client, required_text(params, "instance")?)?;
    let (request_id, wait) = stop_target(params.map())?;
    let stopped = match client.stop_instance_run(&alias, request_id, wait) {
        Ok(stopped) => stopped,
        Err(error) => return receipt_refused(error),
    };
    let members = serde_json::to_value(stopped.stopped)
        .map(|value| cli_line::members(value, RESULT_MEMBER))
        .map_err(|error| output_failed(&error))?;
    Ok(cli_line::success(members, &stopped.outcome))
}

/// Workflow #429 R-6: the run `task stop` names (`request`) and how long the stop waits for its
/// end (`wait`; absent: it answers at once, `wait_ms` 0), the same from the command line's words
/// and from a `serve --stdio` line.
pub fn stop_target(params: &Map<String, Value>) -> Result<(RequestId, Duration), Exit> {
    let params = Params::of(TASK_STOP, params);
    let id = required_text(params, "request")?;
    let request_id = serde_json::from_value::<RequestId>(Value::String(id.to_owned()))
        .map_err(|_| usage("<request>", ArgumentProblem::NotRecognised))?;
    let wait = duration(params, "wait")?.unwrap_or(Duration::ZERO);
    Ok((request_id, wait))
}

/// Workflow #429 R-8: `pack-schedule <inst>`, `PackScheduleView`: each scope's pack schedule list
/// version and, per task package of the instance's tasks, whether it is schedulable now and which
/// layer decided, beside the read's success code; a refusal is the client's failure.
fn pack_schedule_view(params: Params<'_>, client: &RuntimeClient) -> Result<Exit, Exit> {
    let alias = alias_of(client, required_text(params, "instance")?)?;
    let view = client
        .pack_schedule_view(&alias)
        .map_err(|error| client_failed(&error))?;
    let members = serde_json::to_value(view)
        .map(|value| cli_line::members(value, RESULT_MEMBER))
        .map_err(|error| output_failed(&error))?;
    Ok(cli_line::success(
        members,
        &Outcome::new(ReceiptCode::PackScheduleViewCompleted),
    ))
}

/// Workflow #429 R-8: `pack-schedule set <scope> <package_id> --rows <json> [--expected-version]`,
/// `EditPackSchedule`: one package's rows of one scope's list replaced (`--rows []` withdraws
/// them), compared with the list's version when one is given (`0`: the scope has no list yet).
/// The answer is the edit (the list's version after it, the next `--expected-version`, and its
/// event) beside the edit's success code; a refusal is the refused receipt beside its outcome.
fn pack_schedule_set(params: Params<'_>, client: &RuntimeClient) -> Result<Exit, Exit> {
    let scope = match pack_schedule_scope(required_text(params, "scope")?)? {
        ScheduleScope::Instance(selector) => actingcommand_contract::FactScope::Instance {
            instance_id: alias_of(client, selector)?,
        },
        ScheduleScope::Server(server_id) => actingcommand_contract::FactScope::Server {
            server_id: server_id.to_owned(),
        },
        ScheduleScope::Game(game_id) => actingcommand_contract::FactScope::Game {
            game_id: game_id.to_owned(),
        },
    };
    let package_id = required_text(params, "package_id")?;
    let rows = pack_schedule_rows(params.map())?;
    let expected_version = unsigned(params, "expected_version")?;
    let edited = match client.edit_pack_schedule(scope, package_id, rows, expected_version) {
        Ok(edited) => edited,
        Err(error) => return receipt_refused(error),
    };
    let members = serde_json::to_value(edited)
        .map(|value| cli_line::members(value, RESULT_MEMBER))
        .map_err(|error| output_failed(&error))?;
    Ok(cli_line::success(
        members,
        &Outcome::new(ReceiptCode::EditPackScheduleCompleted),
    ))
}

/// Workflow #429 R-8: the scope `pack-schedule set` names, one word: `instance:<inst>` (an alias,
/// an `instance_id` or an ADB port, resolved by one status read), `server:<server id>` or
/// `game:<game id>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleScope<'a> {
    Instance(&'a str),
    Server(&'a str),
    Game(&'a str),
}

/// Workflow #429 R-8: the scope word of `pack-schedule set`, the same from the command line's
/// words and from a `serve --stdio` line; another word, or one that names nothing after its kind,
/// is refused as `<scope>` `not_recognised`.
pub fn pack_schedule_scope(word: &str) -> Result<ScheduleScope<'_>, Exit> {
    let refused = || usage("<scope>", ArgumentProblem::NotRecognised);
    let (kind, name) = word.split_once(':').ok_or_else(refused)?;
    if name.is_empty() {
        return Err(refused());
    }
    match kind {
        "instance" => Ok(ScheduleScope::Instance(name)),
        "server" => Ok(ScheduleScope::Server(name)),
        "game" => Ok(ScheduleScope::Game(name)),
        _ => Err(refused()),
    }
}

/// Workflow #429 R-8: the rows `pack-schedule set` writes (`--rows`, a JSON list of
/// `{from_unix_ms, until_unix_ms?, schedulable}`; without `until_unix_ms` a row is open-ended); a
/// list of another shape is refused as `--rows` `wrong_type`. The Runtime checks the periods.
pub fn pack_schedule_rows(
    params: &Map<String, Value>,
) -> Result<Vec<actingcommand_contract::PackScheduleEditRow>, Exit> {
    let rows = Params::of(PACK_SCHEDULE_SET, params)
        .get("rows")
        .ok_or_else(|| usage("--rows", ArgumentProblem::Required))?;
    serde_json::from_value(rows.clone()).map_err(|_| usage("--rows", ArgumentProblem::WrongType))
}

/// `run start <inst> <package> [--recovery] [--deadline] [--wait]` (actingctl's `task-run`): the
/// package, and its recovery package, by their paths; with a wait, the run is prepared so that it
/// is named before its receipt, and a wait that ends first answers `command_wait_expired`.
fn run_start(params: Params<'_>, client: &RuntimeClient) -> Result<Exit, Exit> {
    let (alias, request) = run_start_request(params, client)?;
    let output = match duration(params, "wait")? {
        None => client
            .run_contained_task(&alias, request)
            .map_err(|error| client_failed(&error))?,
        Some(wait) => {
            let prepared = client
                .prepare_contained_task(&alias, request)
                .map_err(|error| client_failed(&error))?;
            let request_id = prepared.request_id();
            let correlation_id = prepared.correlation_id();
            let submitter = client.clone();
            match wait_at_most(wait, move || submitter.submit_prepared(prepared)) {
                Waited::Done(output) => output.map_err(|error| client_failed(&error))?,
                Waited::Expired => {
                    return Ok(expired(
                        ContractLocation::RunContainedTask,
                        wait,
                        Some(request_id),
                        Some(correlation_id),
                    ));
                }
            }
        }
    };
    run_start_answer(&output)
}

/// Workflow #434 LAB-5G3: the instance `run start` names, resolved by one status read on
/// `client`, and the run's request: the package, and its recovery package, by their paths made
/// absolute, and the deadline (absent: the longest a run may have). `run start` reads them so
/// before it runs or prepares the run, as before.
fn run_start_request(
    params: Params<'_>,
    client: &RuntimeClient,
) -> Result<(String, ContainedTaskRequest), Exit> {
    let alias = alias_of(client, required_text(params, "instance")?)?;
    let package = canonical(required_text(params, "package")?)?;
    let recovery = match text(params, "recovery") {
        Some(path) => Some(canonical(path)?),
        None => None,
    };
    let deadline = match duration(params, "deadline")? {
        Some(deadline) => millis(deadline),
        None => ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS,
    };
    let request = ContainedTaskRequest::new(package.display().to_string())
        .and_then(|request| match recovery {
            Some(path) => request.with_recovery(ContainedTaskRecoveryBinding::new(
                path.display().to_string(),
            )?),
            None => Ok(request),
        })
        .and_then(|request| request.with_response_deadline_ms(deadline))
        .map_err(|_| usage("<package>", ArgumentProblem::Unusable))?;
    Ok((alias, request))
}

/// Workflow #434 LAB-5G3 (register §1.287, "Slice LAB-5G3"): `run start` for a front that answers
/// before the run's receipt, the MCP server's job: the run read as `run start` reads it and
/// prepared on `client`, so that it is named by its request id before anything of it is sent.
/// `client.submit_prepared` then submits it, and [`run_start_ended`] is the line of its end. The
/// row's `wait` is the front's to honour; it is not read here.
pub fn run_start_prepared(
    params: &Map<String, Value>,
    client: &RuntimeClient,
) -> Result<PreparedContainedTask, Exit> {
    let (alias, request) = run_start_request(Params::of(RUN_START, params), client)?;
    client
        .prepare_contained_task(&alias, request)
        .map_err(|error| client_failed(&error))
}

/// Workflow #434 LAB-5G3: the line of a prepared run's submission ([`run_start_prepared`]): the
/// line `run start` gives for its receipt, or the client's failure.
pub fn run_start_ended(submitted: Result<RuntimeFlowOutput, RuntimeClientError>) -> Exit {
    match submitted
        .map_err(|error| client_failed(&error))
        .and_then(|output| run_start_answer(&output))
    {
        Ok(exit) | Err(exit) => exit,
    }
}

/// The line of a run's receipt: `task_run_reported` beside it, or, for a run that ended failed and
/// still came back with its receipt, the receipt's own outcome on top, and the line exits
/// non-zero.
fn run_start_answer(output: &RuntimeFlowOutput) -> Result<Exit, Exit> {
    let completed = output.receipt().state() == RuntimeReceiptState::Completed;
    let outcome = if completed {
        Outcome::new(ActingctlCode::TaskRunReported)
    } else {
        output.receipt().outcome().clone()
    };
    let members = serde_json::to_value(output)
        .map(|value| cli_line::members(value, RESULT_MEMBER))
        .map_err(|error| output_failed(&error))?;
    Ok(cli_line::receipt(members, &outcome, !completed))
}

/// `run stop <id> [--cancel-only]` (MCP `ac_stop_run`): the run's cancellation, and without
/// `--cancel-only` the reset of its instance after it, waited for up to the longest run deadline.
fn run_stop(params: Params<'_>, client: &RuntimeClient) -> Result<Exit, Exit> {
    let id = required_text(params, "id")?;
    let request_id = serde_json::from_value::<RequestId>(Value::String(id.to_owned()))
        .map_err(|_| usage("<id>", ArgumentProblem::NotRecognised))?;
    if switch(params, "cancel_only") {
        let cancellation = client
            .cancel_contained_task(request_id)
            .map_err(|error| client_failed(&error))?;
        return Ok(completed(
            serde_json::to_value(&cancellation).map_err(|error| output_failed(&error))?,
        ));
    }
    let key =
        run_show::by_request_id(id).ok_or_else(|| usage("<id>", ArgumentProblem::NotRecognised))?;
    let run = run_show::read(client, key).map_err(|error| client_failed(&error))?;
    let status = client.status().map_err(|error| client_failed(&error))?;
    let alias = run
        .instance_id
        .and_then(|instance_id| {
            status
                .instances()
                .iter()
                .find(|instance| instance.instance_id() == instance_id)
                .map(|instance| instance.instance_alias().to_owned())
        })
        .ok_or_else(|| {
            cli_line::refused(
                &Outcome::new(ActingctlCode::RunUnknown).request_id(id),
                Some("the ledger names no configured instance for this run"),
            )
        })?;
    let wait = Duration::from_millis(ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS);
    let stopped = client
        .cancel_contained_task_and_reset(&alias, request_id, wait)
        .map_err(|error| client_failed(&error))?;
    // MCP `ac_stop_run`'s answer: the cancellation and what became of the touch release.
    let mut answer = Map::new();
    answer.insert(
        "cancellation".to_owned(),
        serde_json::to_value(&stopped.status).map_err(|error| output_failed(&error))?,
    );
    match stopped.reset {
        Some(ContainedTaskResetOutcome::Done) => {
            answer.insert("touch_release".to_owned(), Value::from("done"));
        }
        Some(ContainedTaskResetOutcome::NotNeeded) => {
            answer.insert("touch_release".to_owned(), Value::from("not_needed"));
        }
        Some(ContainedTaskResetOutcome::Failed(error)) => {
            answer.insert("touch_release".to_owned(), Value::from("failed"));
            answer.insert(
                "touch_release_error".to_owned(),
                cli_line::outcome_value(
                    &error.outcome(),
                    cli_line::detail().unwrap_or(Detail::Full),
                ),
            );
        }
        None => {}
    }
    Ok(completed(Value::Object(answer)))
}

// ---- lines

/// A line `lab-ops` built; a dispatcher that did not take its first word is a defect of this
/// module, answered as an unknown word.
fn dispatched(exit: Option<Exit>) -> Exit {
    exit.unwrap_or_else(|| {
        cli_line::failure(
            &cli_line::usage_invalid(None, ArgumentProblem::Unknown),
            Some("the operation's command was not served"),
        )
    })
}

/// A typed result beside its success code.
fn reported(result: &impl Serialize, code: ActingctlCode) -> Exit {
    match serde_json::to_value(result) {
        Ok(value) => {
            cli_line::success(cli_line::members(value, RESULT_MEMBER), &Outcome::new(code))
        }
        Err(error) => output_failed(&error),
    }
}

/// An answer of an operation whose only source was an MCP tool: `lab_operation_completed`
/// (Workflow #434 LAB-5G1b: this crate's code, whichever program serves the row).
fn completed(answer: Value) -> Exit {
    cli_line::success(
        cli_line::members(answer, RESULT_MEMBER),
        &Outcome::new(ActingctlCode::LabOperationCompleted),
    )
}

/// A result that does not encode: the developer line `actingctl_output_failed` alone, exit 1.
fn output_failed(error: &serde_json::Error) -> Exit {
    Exit {
        line: Err(format!("the JSON result cannot be encoded: {error}")),
        developer: None,
        code: 1,
    }
}

/// A Runtime client failure: its outcome (a refused receipt's outcome on top), non-zero.
fn client_failed(error: &RuntimeClientError) -> Exit {
    cli_line::refused(&error.outcome(), Some(&error.to_string()))
}

/// A refused write whose receipt the line prints: `{receipt}` beside the receipt's outcome;
/// without a receipt the client's failure.
fn receipt_refused(error: RuntimeClientError) -> Result<Exit, Exit> {
    let Some(receipt) = error.received_receipt() else {
        return Err(client_failed(&error));
    };
    let mut members = Map::new();
    members.insert(
        "receipt".to_owned(),
        serde_json::to_value(receipt).map_err(|encode| output_failed(&encode))?,
    );
    Ok(cli_line::receipt(members, &error.outcome(), true))
}

/// An instance selector that names no instance, or more than one.
fn selector_missed(miss: SelectorMiss, selector: &str) -> Exit {
    let text = match miss {
        SelectorMiss::Unknown => "the runtime status lists no instance with that alias",
        SelectorMiss::Ambiguous => {
            "more than one instance of the runtime status matches that selector"
        }
    };
    cli_line::refused(&miss.outcome(selector), Some(text))
}

/// A material read that answered nothing (MCP `ac_material`'s failures).
fn material_failed(failure: material::Failure) -> Exit {
    match failure {
        material::Failure::Usage(outcome)
        | material::Failure::NotText(outcome)
        | material::Failure::Unavailable(outcome) => cli_line::refused(&outcome, None),
        material::Failure::OutOfRange {
            argument,
            minimum,
            maximum,
        } => cli_line::failure(
            &cli_line::usage_invalid(Some(argument), ArgumentProblem::OutOfRange)
                .range_min(minimum)
                .range_max(maximum),
            None,
        ),
        material::Failure::Runtime(error) => client_failed(&error),
        material::Failure::RuntimeNoted { error, reached } => cli_line::refused(
            &error.outcome().caused_by(reached),
            Some(&error.to_string()),
        ),
    }
}

/// Workflow #434 LAB-5C: what one `material` line reads, decided in `served`, after the connection
/// is made, as the base checked `--max-bytes`' range there (fix 1: so `offset` or `max_bytes`
/// beside `out` is refused only once actingd answers the connection).
#[derive(Debug, PartialEq, Eq)]
pub enum MaterialRead<'a> {
    /// At most `max_bytes` of the material's text from `offset`: the row without `out`, as before.
    Text {
        reference: &'a str,
        offset: u64,
        max_bytes: u64,
    },
    /// The whole material, written into the directory `out` (MCP `ac_material`'s mode export).
    Whole { reference: &'a str, out: PathBuf },
}

/// Workflow #434 LAB-5C: the read `params` ask for. Without `out`, as before: the ref, the slice's
/// offset (0) and size (`material::DEFAULT_TEXT_BYTES`, from 1 to `material::MAX_TEXT_BYTES`).
/// With `out`, the whole material into that directory, made absolute; `offset` and `max_bytes`
/// belong to the slice and are refused beside it (`conflicting`), as MCP refuses them with its
/// mode export.
pub fn material_read(params: &Map<String, Value>) -> Result<MaterialRead<'_>, Exit> {
    let params = Params::of(MATERIAL, params);
    let reference = required_text(params, "ref")?;
    let Some(out) = text(params, "out") else {
        let offset = unsigned(params, "offset")?.unwrap_or(0);
        let max_bytes = unsigned(params, "max_bytes")?.unwrap_or(material::DEFAULT_TEXT_BYTES);
        if !(1..=material::MAX_TEXT_BYTES).contains(&max_bytes) {
            return Err(cli_line::failure(
                &cli_line::usage_invalid(Some("--max-bytes"), ArgumentProblem::OutOfRange)
                    .range_min(1_u64)
                    .range_max(material::MAX_TEXT_BYTES),
                None,
            ));
        }
        return Ok(MaterialRead::Text {
            reference,
            offset,
            max_bytes,
        });
    };
    if let Some(slice) = ["offset", "max_bytes"]
        .into_iter()
        .find(|name| params.contains_key(name))
    {
        return Err(usage(&flag_name(slice), ArgumentProblem::Conflicting));
    }
    let out = std::path::absolute(out).map_err(|_| usage("--out", ArgumentProblem::Unusable))?;
    Ok(MaterialRead::Whole { reference, out })
}

/// Workflow #434 LAB-5C: `material --out`: the whole material's `bytes`, whose `sha256` the read
/// verified, written into `out` as `<hex>.<extension>` by the writer `frame --out` and MCP share
/// (`material_export::write`); the line names the file, `{path, sha256, size}` beside
/// `lab_operation_completed`. A failed write is the writer's `material_export_failed`.
pub fn material_written(out: &Path, sha256: &str, extension: &str, bytes: &[u8]) -> Exit {
    match material_export::write(out, sha256, extension, bytes) {
        Ok(written) => completed(written),
        Err(failure) => cli_line::refused(&failure.outcome(), None),
    }
}

/// `facts` and `config`: the read's answer beside `code`. Workflow #434 LAB-5G1b: whole without a
/// budget, as before; with one, written whole through its export when it does not fit
/// (`facts::within_budget`), the line carrying the header and `answer_exported`. An answer that
/// could not be written is that write's failure.
fn facts_line(
    client: &RuntimeClient,
    request: &facts::FactsRequest,
    code: ActingctlCode,
    budget: Option<&Budget<'_>>,
) -> Result<Exit, Exit> {
    let answer = facts::read(client, request).map_err(|failure| match failure {
        facts::Failure::Selector { miss, selector } => selector_missed(miss, &selector),
        facts::Failure::ConfigFactsMissing => cli_line::refused(
            &Outcome::new(ActingctlCode::ConfigFactsMissing),
            Some("the runtime fact snapshot holds no config.subsystems or no config.parameters.<section> record"),
        ),
        facts::Failure::Runtime(error) => client_failed(&error),
        facts::Failure::Output(error) => output_failed(&error),
    })?;
    Ok(match facts::within_budget(answer, budget) {
        Ok(fitted) => cli_line::success(
            budget::line_members(fitted, RESULT_MEMBER, Vec::new()),
            &Outcome::new(code),
        ),
        Err(failure) => cli_line::refused(&failure, None),
    })
}

/// A wait that ended before its request did: `{request_id?, correlation_id?, state}` with
/// `command_wait_expired`, exit 1.
fn expired(
    operation: ContractLocation,
    wait: Duration,
    request_id: Option<RequestId>,
    correlation_id: Option<actingcommand_contract::CorrelationId>,
) -> Exit {
    let (members, outcome) = wait::expired(operation, wait, request_id, correlation_id);
    cli_line::receipt(members, &outcome, true)
}

// ---- the Runtime and the instance

/// A connection to the Runtime of `state_root` on the Lab channel, in the detail
/// `ACTINGCOMMAND_DETAIL` asks for (Workflow #434 LAB-5G1: as `connection` says: one made for this
/// call so, or the caller's kept one, whose detail is the caller's).
fn connect<'a>(state_root: &Path, connection: Connection<'a>) -> Result<Reached<'a>, Exit> {
    connection
        .reach(state_root, |config| match cli_line::detail() {
            Ok(Detail::Codes) => config.with_detail(Detail::Codes),
            _ => config,
        })
        .map_err(|error| client_failed(&error))
}

/// The alias of the instance `selector` names (alias, `instance_id` or ADB port), after one
/// status read.
fn alias_of(client: &RuntimeClient, selector: &str) -> Result<String, Exit> {
    let status = client.status().map_err(|error| client_failed(&error))?;
    instance_selector::select(status.instances(), selector)
        .map(|instance| instance.instance_alias().to_owned())
        .map_err(|miss| selector_missed(miss, selector))
}

/// The alias of the optional `instance` parameter.
fn optional_alias(client: &RuntimeClient, params: Params<'_>) -> Result<Option<String>, Exit> {
    match text(params, "instance") {
        Some(selector) => alias_of(client, selector).map(Some),
        None => Ok(None),
    }
}

/// A package path as the Runtime opens it.
fn canonical(path: &str) -> Result<PathBuf, Exit> {
    std::fs::canonicalize(path).map_err(|error| {
        cli_line::refused(
            &Outcome::new(ActingctlCode::ActingctlPackageResolutionFailed)
                .path(path)
                .caused_by(Outcome::from_io_error(
                    &error,
                    actingcommand_contract::outcome::IoOp::Canonicalize,
                )),
            Some("the contained task package cannot be resolved"),
        )
    })
}

// ---- parameters

// Workflow #434 LAB-5B1: each reads by name through the row's constant ([`Params`]).

fn text<'a>(params: Params<'a>, name: &str) -> Option<&'a str> {
    params.get(name).and_then(Value::as_str)
}

fn required_text<'a>(params: Params<'a>, name: &str) -> Result<&'a str, Exit> {
    text(params, name).ok_or_else(|| usage(&format!("<{name}>"), ArgumentProblem::Required))
}

fn switch(params: Params<'_>, name: &str) -> bool {
    params.get(name).and_then(Value::as_bool).unwrap_or(false)
}

fn unsigned(params: Params<'_>, name: &str) -> Result<Option<u64>, Exit> {
    match params.get(name) {
        None => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| usage(&flag_name(name), ArgumentProblem::WrongType)),
    }
}

/// A `list<string>`, `key` or `key_value` parameter: its strings.
fn list_of(params: Params<'_>, name: &str) -> Option<Vec<String>> {
    match params.get(name)? {
        Value::Array(items) => Some(
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
        ),
        Value::String(item) => Some(vec![item.clone()]),
        _ => None,
    }
}

fn duration(params: Params<'_>, name: &str) -> Result<Option<Duration>, Exit> {
    match params.get(name) {
        None => Ok(None),
        Some(Value::String(text)) => parse_duration(text)
            .map(Some)
            .ok_or_else(|| usage(&flag_name(name), ArgumentProblem::WrongType)),
        Some(_) => Err(usage(&flag_name(name), ArgumentProblem::WrongType)),
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn flag_name(name: &str) -> String {
    format!("--{}", name.replace('_', "-"))
}

fn usage(argument: &str, problem: ArgumentProblem) -> Exit {
    cli_line::failure(&cli_line::usage_invalid(Some(argument), problem), None)
}

/// actingctl's words for a command LAB-1 moved, built from the operation's parameters.
struct Argv(Vec<OsString>);

impl Argv {
    fn new(words: &[&str]) -> Self {
        Self(words.iter().map(OsString::from).collect())
    }

    fn word(&mut self, word: impl Into<OsString>) {
        self.0.push(word.into());
    }

    fn flag(&mut self, flag: &str, value: impl Into<OsString>) {
        self.0.push(OsString::from(flag));
        self.0.push(value.into());
    }

    fn state_root(&mut self, state_root: &Path) {
        self.flag(core_reads::STATE_ROOT_FLAG, state_root);
    }

    /// Workflow #434 LAB-5B1: the parameter `name`, when given, as its row's constant writes it.
    /// Before the slice each row's arm wrote its parameters itself, in the same order and forms
    /// (Workflow #434 LAB-5A's `options_of` the `Written::Each` ones, in the row's order).
    fn param(&mut self, params: Params<'_>, name: &str, written: Written) -> Result<(), Exit> {
        match written {
            Written::Word => self.word_of(params, name),
            Written::Flag(flag) => self.flag_of(params, name, flag),
            Written::Switch(flag) => self.switch_of(params, name, flag),
            Written::Joined(flag) => {
                if let Some(items) = list_of(params, name) {
                    self.flag(flag, items.join(","));
                }
            }
            Written::Each(flag) => {
                if let Some(value) = params.get(name) {
                    self.written(flag, value);
                }
            }
            Written::SinceNow(flag) => self.since(params, name, flag)?,
            Written::RunOrRequest(run_flag, request_flag) => {
                if let Some(id) = text(params, name) {
                    let flag = if run_show::by_request_id(id).is_none()
                        && run_show::by_run_id(id).is_some()
                    {
                        run_flag
                    } else {
                        request_flag
                    };
                    self.flag(flag, id);
                }
            }
            // Read where it goes, beside the words (`dispatched_with_state_root`).
            Written::Beside => {}
        }
        Ok(())
    }

    /// The parameter `name` as a word, when given.
    fn word_of(&mut self, params: Params<'_>, name: &str) {
        if let Some(value) = params.get(name) {
            self.word(scalar(value));
        }
    }

    /// The parameter `name` as `flag <value>`, when given.
    fn flag_of(&mut self, params: Params<'_>, name: &str, flag: &str) {
        if let Some(value) = params.get(name) {
            self.flag(flag, scalar(value));
        }
    }

    /// The switch `name` as `flag`, when set.
    fn switch_of(&mut self, params: Params<'_>, name: &str, flag: &str) {
        if switch(params, name) {
            self.word(flag);
        }
    }

    /// `flag` (`--since-unix-ms`): now less the duration of the parameter `name` (`since`).
    fn since(&mut self, params: Params<'_>, name: &str, flag: &str) -> Result<(), Exit> {
        let Some(since) = duration(params, name)? else {
            return Ok(());
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| cli_line::refused(&Outcome::new(ActingctlCode::ClockUnavailable), None))?;
        let since = now.saturating_sub(since);
        self.flag(flag, millis(since).to_string());
        Ok(())
    }

    /// Workflow #434 LAB-5A fix 1: the members the row does not declare, which only the command
    /// line hands on (`words::KEPT_FLAGS`: `devices bind --root`), each under the flag of its
    /// name, as the row's further flags went before LAB-5A.
    fn further(&mut self, operation: &Operation, params: &Map<String, Value>) {
        for (name, value) in further_flags(operation, params) {
            self.written(&flag_name(name), value);
        }
    }

    /// `flag` with `value`: `true` the flag alone, `false` nothing, a list item by item, another
    /// value the flag and the value.
    fn written(&mut self, flag: &str, value: &Value) {
        let items = match value {
            Value::Array(items) => items.as_slice(),
            other => std::slice::from_ref(other),
        };
        for item in items {
            match item {
                Value::Bool(true) => self.word(flag),
                Value::Bool(false) => {}
                other => self.flag(flag, scalar(other)),
            }
        }
    }
}

/// A parameter's value as one command-line word.
fn scalar(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

// ---- moved with this module from actinglab's words (`apps/actinglab/src/cli_module/words.rs`),
// which reads them from here

/// The member that held the further words of the rows whose command line handed them on to a
/// command that ignored them (actinglab's `LENIENT_ON_COMMAND_LINE`, gone with Workflow #434
/// LAB-5B4); no row declares it and no command line writes it now, and a call that carries it is
/// refused by name (`serve --stdio` refuses it as undeclared).
pub const MORE_WORDS: &str = "words";

/// The members of `params` the row does not declare: on the command line, the global options and
/// the undeclared flags of actinglab's `KEPT_FLAGS` handed on (never [`MORE_WORDS`]), in the map's
/// order.
pub fn further_flags<'a>(
    operation: &Operation,
    params: &'a Map<String, Value>,
) -> Vec<(&'a str, &'a Value)> {
    params
        .iter()
        .filter(|(name, _)| {
            name.as_str() != MORE_WORDS
                && !operation.params.iter().any(|param| &param.name == *name)
        })
        .map(|(name, value)| (name.as_str(), value))
        .collect()
}

/// A duration as the table writes it: a whole number with `ms`, `s`, `m` or `h`.
pub fn parse_duration(text: &str) -> Option<Duration> {
    let (number, unit) = text
        .find(|character: char| !character.is_ascii_digit())
        .map(|split| text.split_at(split))?;
    let number = number.parse::<u64>().ok()?;
    match unit {
        "ms" => Some(Duration::from_millis(number)),
        "s" => Some(Duration::from_secs(number)),
        "m" => number.checked_mul(60).map(Duration::from_secs),
        "h" => number.checked_mul(3600).map(Duration::from_secs),
        _ => None,
    }
}
