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

use super::place::{self, Place};
use super::words::parse_duration;
use crate::codes::ActinglabCode;
use actingcommand_contract::codes::ContractLocation;
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_contract::{
    ContainedTaskRecoveryBinding, ContainedTaskRequest, MAX_SCHEDULING_PAUSE_DRAIN_TIMEOUT_MS,
    MIN_SCHEDULING_PAUSE_DRAIN_TIMEOUT_MS, RequestId, RuntimeReceiptState,
    SchedulingPauseExpectation, SchedulingPauseReason,
};
use actingcommand_lab_ops::cli_line::{self, Exit};
use actingcommand_lab_ops::codes::{ActingctlCode, ArgumentProblem};
use actingcommand_lab_ops::instance_selector::{self, SelectorMiss};
use actingcommand_lab_ops::table::{Operation, ParamForm};
use actingcommand_lab_ops::wait::{Waited, wait_at_most};
use actingcommand_lab_ops::{
    capability_gap, code_lookup, configure, devices, diagnose, emulators, events, evidence_read,
    fact_publish, facts, frames, ladder_drill, material, material_export, observe_export, overview,
    packs, perf_preset, remedy, routine_summary, run_evidence, run_list, run_show, schedule,
    scheduling, shutdown, targets, wait, wake_test,
};
use actingcommand_runtime_client::{ContainedTaskResetOutcome, RuntimeClient, RuntimeClientError};
use serde::Serialize;
use serde_json::{Map, Value};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The member a result that is not a JSON object goes under (actingctl's `RESULT_MEMBER`).
const RESULT_MEMBER: &str = "result";
/// `pause`'s drain when none is given (the operation table's default).
const DEFAULT_DRAIN: Duration = Duration::from_secs(300);
/// Workflow #434 LAB-5C: the options of `events` that MCP `ac_events` took and the row gained,
/// each handed on under the flag of its name (`exclude_event_types`, a list, goes as one comma
/// list beside them).
const EVENTS_OPTIONS: [&str; 13] = [
    "view",
    "since_unix_ms",
    "until_unix_ms",
    "through_sequence",
    "event_type",
    "fact_key",
    "min_severity",
    "max_severity",
    "origin_module",
    "task_id",
    "correlation_id",
    "detail",
    "wait_s",
];

/// The operations of this module: the core rows and `run start`, `run stop`, `reset`, `drill`.
pub(super) fn serves(operation: &Operation) -> bool {
    let name = operation.name.as_str();
    if matches!(name, "run start" | "run stop" | "reset" | "drill") {
        return true;
    }
    // `app launch|restart|stop` are actinglab's own `session app` (the engine).
    operation.module == actingcommand_lab_ops::table::OperationModule::Core
        && !matches!(name, "app launch" | "app restart" | "app stop")
}

/// The operation `operation` with `params`, as its line.
pub(super) fn answer(operation: &Operation, params: &Map<String, Value>) -> Exit {
    let name = operation.name.as_str();
    // Offline operations: no state root; only `packs` reads the install root.
    let root = if name == "packs" {
        place::place().root
    } else {
        None
    };
    if let Some(argv) = offline_words(operation, params, root.as_deref()) {
        return dispatched(match name {
            "codes" => code_lookup::dispatch(&argv),
            "gap" => capability_gap::dispatch(&argv),
            _ => packs::dispatch(&argv),
        });
    }
    let located = place::place();
    let state_root = match &located.state_root {
        Ok(state_root) => state_root.clone(),
        Err(outcome) => return cli_line::failure(outcome, Some(place::UNLOCATED)),
    };
    if let Some(exit) = dispatched_with_state_root(operation, params, &located, &state_root) {
        return exit;
    }
    // The operations called here take only the row's parameters: a flag or word the row does not
    // declare is refused by name, never ignored (Workflow #434 LAB-5A: the words refuse it first,
    // since no core row has `params_more`).
    if let Some((further, _)) = super::words::further_flags(operation, params).first() {
        return usage(&flag_name(further), ArgumentProblem::Unknown);
    }
    if let Some(word) =
        list_of(params, super::words::MORE_WORDS).and_then(|words| words.first().cloned())
    {
        return usage(&word, ArgumentProblem::Unknown);
    }
    let client = match connect(&state_root) {
        Ok(client) => client,
        Err(exit) => return exit,
    };
    let answered = if name == "shutdown" {
        shutdown_line(params, client, &state_root)
    } else {
        served(name, params, &client)
    };
    match answered {
        Ok(exit) | Err(exit) => exit,
    }
}

/// `shutdown [--wait]`: the receipt and, with a wait, the shutdown seen to complete. The client
/// is given to the wait, which closes its connection first.
fn shutdown_line(
    params: &Map<String, Value>,
    client: RuntimeClient,
    state_root: &Path,
) -> Result<Exit, Exit> {
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
    let shutdown = shutdown::wait(client, state_root, wait).map_err(|outcome| {
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
/// only `packs` reads); `None` for another row.
pub(crate) fn offline_words(
    operation: &Operation,
    params: &Map<String, Value>,
    root: Option<&Path>,
) -> Option<Vec<OsString>> {
    let argv = match operation.name.as_str() {
        "codes" => {
            let mut argv = Argv::new(&["codes"]);
            argv.word_of(params, "code");
            // LAB-5C: the list's filters and page (the survey's difference 8), under their flags.
            argv.flag_of(params, "prefix", "--prefix");
            argv.flag_of(params, "owner", "--owner");
            argv.flag_of(params, "limit", "--limit");
            argv
        }
        "gap" => {
            let mut argv = Argv::new(&[capability_gap::COMMAND]);
            argv.options_of(operation, params, &[]);
            argv
        }
        "packs" => {
            let mut argv = Argv::new(&["packs"]);
            argv.flag_of(params, "instance", "--instance");
            if let Some(root) = root {
                argv.flag("--root", root);
            }
            // LAB-5C: the rows of one role.
            argv.flag_of(params, "role", "--role");
            argv
        }
        _ => return None,
    };
    Some(argv.0)
}

/// The operations LAB-1 moved with their parsing, given actingctl's words; `None` for another.
fn dispatched_with_state_root(
    operation: &Operation,
    params: &Map<String, Value>,
    place: &Place,
    state_root: &Path,
) -> Option<Exit> {
    let argv = match handed_words(operation, params, place.root.as_deref(), state_root)? {
        Ok(argv) => argv,
        Err(exit) => return Some(exit),
    };
    let detail = cli_line::detail().unwrap_or(Detail::Full);
    let exit = match operation.name.as_str() {
        "status" => overview::dispatch(&argv),
        "summary" => routine_summary::dispatch(&argv, detail),
        "events" => events::dispatch(&argv),
        "runs" => run_list::dispatch(&argv),
        "run evidence" => run_evidence::dispatch(&argv),
        "diagnose" => diagnose::dispatch(&argv),
        // Workflow #434 LAB-5C: `remedy set`'s rows given inline go beside its words.
        "remedy set" => match params.get("rows") {
            Some(rows) => remedy::dispatch_rows(&argv, rows, detail),
            None => remedy::dispatch(&argv, detail),
        },
        "remedy" | "remedy lift" => remedy::dispatch(&argv, detail),
        "frame" => observe_export::dispatch(&argv),
        "preset" | "preset set" => perf_preset::dispatch(&argv, detail),
        "evidence read" => evidence_read::dispatch(&argv, detail),
        "devices" | "devices bind" | "devices unbind" => devices::dispatch(&argv),
        _ => return None,
    };
    Some(dispatched(exit))
}

/// Workflow #434 LAB-5C: the words a row LAB-1 moved with its parsing gives its command in
/// `lab-ops` (actingctl's words for the row's parameters), built apart from the run so that a test
/// reads what a row hands on; `root` is the install root, `state_root` the state root. `None` for
/// a row served here with its parameters; a parameter refused while the words are built is its
/// line. `remedy set`'s rows given inline are no word: they go beside the words.
pub(crate) fn handed_words(
    operation: &Operation,
    params: &Map<String, Value>,
    root: Option<&Path>,
    state_root: &Path,
) -> Option<Result<Vec<OsString>, Exit>> {
    let argv = match operation.name.as_str() {
        "status" => {
            let mut argv = Argv::new(&["overview"]);
            argv.state_root(state_root);
            argv.flag_of(params, "instance", "--instance");
            if let Some(sections) = list_of(params, "sections") {
                argv.flag("--sections", sections.join(","));
            }
            if let Some(root) = root {
                argv.flag("--root", root);
            }
            argv
        }
        "summary" => {
            let mut argv = Argv::new(&[routine_summary::COMMAND]);
            argv.state_root(state_root);
            argv.flag_of(params, "instance", "--instance");
            argv.switch_of(params, "hour", routine_summary::HOUR);
            argv.switch_of(params, "day", routine_summary::DAY);
            argv.flag_of(params, "since", routine_summary::SINCE);
            argv
        }
        "events" => {
            let mut argv = Argv::new(&["events"]);
            argv.state_root(state_root);
            argv.flag_of(params, "instance", "--instance");
            if let Err(exit) = argv.since(params) {
                return Some(Err(exit));
            }
            if let Some(types) = list_of(params, "type") {
                argv.flag("--event-types", types.join(","));
            }
            argv.flag_of(params, "run", "--run-id");
            argv.flag_of(params, "request", "--request-id");
            argv.flag_of(params, "after", "--after-sequence");
            argv.flag_of(params, "limit", "--limit");
            // LAB-5C: what MCP `ac_events` took, after the words the row gave before.
            for name in EVENTS_OPTIONS {
                argv.flag_of(params, name, &flag_name(name));
            }
            if let Some(types) = list_of(params, "exclude_event_types") {
                argv.flag("--exclude-event-types", types.join(","));
            }
            argv
        }
        "runs" => {
            let mut argv = Argv::new(&["runs"]);
            argv.state_root(state_root);
            argv.flag_of(params, "instance", "--instance");
            if let Err(exit) = argv.since(params) {
                return Some(Err(exit));
            }
            argv.options_of(operation, params, &["since"]);
            argv
        }
        "run evidence" => {
            let mut argv = Argv::new(&[run_evidence::COMMAND, run_evidence::ACTION]);
            argv.state_root(state_root);
            if let Some(id) = text(params, "id") {
                let flag =
                    if run_show::by_request_id(id).is_none() && run_show::by_run_id(id).is_some() {
                        "--run-id"
                    } else {
                        "--request-id"
                    };
                argv.flag(flag, id);
            }
            argv.flag_of(params, "detail", "--detail");
            argv.options_of(operation, params, &["detail"]);
            argv
        }
        "diagnose" => {
            let mut argv = Argv::new(&["diagnose"]);
            argv.state_root(state_root);
            if let Some(root) = root {
                argv.flag("--root", root);
            }
            argv.flag_of(params, "instance", "--instance");
            argv.flag_of(params, "task", "--task-id");
            if let Err(exit) = argv.since(params) {
                return Some(Err(exit));
            }
            // LAB-5C: the window's start in unix ms, as MCP `ac_diagnose` took it.
            argv.flag_of(params, "since_unix_ms", "--since-unix-ms");
            argv
        }
        "remedy" | "remedy set" | "remedy lift" => {
            let action = match operation.name.as_str() {
                "remedy set" => remedy::SET,
                "remedy lift" => remedy::LIFT,
                _ => remedy::GET,
            };
            let mut argv = Argv::new(&[remedy::COMMAND, action]);
            argv.state_root(state_root);
            argv.flag_of(params, "instance", "--instance");
            argv.flag_of(params, "game", "--game");
            argv.flag_of(params, "file", "--rows");
            argv
        }
        "frame" if params.contains_key("out") => {
            let mut argv = Argv::new(&["observe"]);
            argv.state_root(state_root);
            argv.flag_of(params, "instance", "--instance");
            argv.flag_of(params, "out", "--export");
            argv
        }
        "preset" | "preset set" => {
            let mut argv = Argv::new(&[perf_preset::COMMAND]);
            if operation.name == "preset set" {
                argv.word(perf_preset::SET);
                argv.word_of(params, "name");
            } else {
                argv.word(perf_preset::GET);
            }
            argv.state_root(state_root);
            argv
        }
        "evidence read" => {
            let mut argv = Argv::new(&[evidence_read::COMMAND, evidence_read::CONFIRM_READ]);
            argv.state_root(state_root);
            argv.flag_of(params, "artifact", "--artifact");
            argv.flag_of(params, "run", "--run");
            argv
        }
        "devices" | "devices bind" | "devices unbind" => {
            let action = match operation.name.as_str() {
                "devices bind" => devices::BIND,
                "devices unbind" => devices::UNBIND,
                _ => devices::LIST,
            };
            let mut argv = Argv::new(&[devices::COMMAND, action]);
            argv.word_of(params, "instance");
            // Workflow #434 LAB-5A: `devices bind`'s handle, a declared positional (a further word
            // before).
            argv.word_of(params, "handle");
            argv.state_root(state_root);
            if action != devices::UNBIND
                && let Some(root) = root
            {
                argv.flag("--root", root);
            }
            // Workflow #434 LAB-5A fix 1: `devices bind`'s `--root` from the command line
            // (`words::KEPT_FLAGS`), after the module's own, as its further flags went before:
            // `devices` takes it only when the module located none.
            argv.further(operation, params);
            argv
        }
        _ => return None,
    };
    Some(Ok(argv.0))
}

/// The other operations, called with their parameters on `client`.
fn served(name: &str, params: &Map<String, Value>, client: &RuntimeClient) -> Result<Exit, Exit> {
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
            facts_line(client, &request, code)?
        }
        "config" => facts_line(
            client,
            &facts::FactsRequest::Config,
            ActingctlCode::RuntimeConfigReported,
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
            reported(&explanation, ActingctlCode::RuntimeScheduleReported)
        }
        "run show" => {
            let id = required_text(params, "id")?;
            let key = run_show::by_request_id(id)
                .or_else(|| run_show::by_run_id(id))
                .ok_or_else(|| usage("<id>", ArgumentProblem::NotRecognised))?;
            let status = run_show::read(client, key).map_err(|error| client_failed(&error))?;
            completed(run_show::answer(&status))
        }
        "material" => match material_read(params)? {
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

/// `run start <inst> <package> [--recovery] [--deadline] [--wait]` (actingctl's `task-run`): the
/// package, and its recovery package, by their paths; with a wait, the run is prepared so that it
/// is named before its receipt, and a wait that ends first answers `command_wait_expired`.
fn run_start(params: &Map<String, Value>, client: &RuntimeClient) -> Result<Exit, Exit> {
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
    // A run that ended failed may still come back with its receipt: the receipt's own outcome
    // is the top, and the line exits non-zero.
    let completed = output.receipt().state() == RuntimeReceiptState::Completed;
    let outcome = if completed {
        Outcome::new(ActingctlCode::TaskRunReported)
    } else {
        output.receipt().outcome().clone()
    };
    let members = serde_json::to_value(&output)
        .map(|value| cli_line::members(value, RESULT_MEMBER))
        .map_err(|error| output_failed(&error))?;
    Ok(cli_line::receipt(members, &outcome, !completed))
}

/// `run stop <id> [--cancel-only]` (MCP `ac_stop_run`): the run's cancellation, and without
/// `--cancel-only` the reset of its instance after it, waited for up to the longest run deadline.
fn run_stop(params: &Map<String, Value>, client: &RuntimeClient) -> Result<Exit, Exit> {
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

/// An answer of an operation whose only source was an MCP tool: `lab_operation_completed`.
fn completed(answer: Value) -> Exit {
    cli_line::success(
        cli_line::members(answer, RESULT_MEMBER),
        &Outcome::new(ActinglabCode::LabOperationCompleted),
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

/// Workflow #434 LAB-5C: what one `material` line reads, decided before anything is sent.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum MaterialRead<'a> {
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
pub(crate) fn material_read(params: &Map<String, Value>) -> Result<MaterialRead<'_>, Exit> {
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
        .find(|name| params.contains_key(*name))
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
pub(crate) fn material_written(out: &Path, sha256: &str, extension: &str, bytes: &[u8]) -> Exit {
    match material_export::write(out, sha256, extension, bytes) {
        Ok(written) => completed(written),
        Err(failure) => cli_line::refused(&failure.outcome(), None),
    }
}

/// `facts` and `config`: the read's answer beside `code`.
fn facts_line(
    client: &RuntimeClient,
    request: &facts::FactsRequest,
    code: ActingctlCode,
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
    Ok(reported(&answer, code))
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
/// `ACTINGCOMMAND_DETAIL` asks for.
fn connect(state_root: &Path) -> Result<RuntimeClient, Exit> {
    let config = actingcommand_lab_ops::front::client_config(state_root);
    let config = match cli_line::detail() {
        Ok(Detail::Codes) => config.with_detail(Detail::Codes),
        _ => config,
    };
    RuntimeClient::connect(config).map_err(|error| client_failed(&error))
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
fn optional_alias(
    client: &RuntimeClient,
    params: &Map<String, Value>,
) -> Result<Option<String>, Exit> {
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

fn text<'a>(params: &'a Map<String, Value>, name: &str) -> Option<&'a str> {
    params.get(name).and_then(Value::as_str)
}

fn required_text<'a>(params: &'a Map<String, Value>, name: &str) -> Result<&'a str, Exit> {
    text(params, name).ok_or_else(|| usage(&format!("<{name}>"), ArgumentProblem::Required))
}

fn switch(params: &Map<String, Value>, name: &str) -> bool {
    params.get(name).and_then(Value::as_bool).unwrap_or(false)
}

fn unsigned(params: &Map<String, Value>, name: &str) -> Result<Option<u64>, Exit> {
    match params.get(name) {
        None => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| usage(&flag_name(name), ArgumentProblem::WrongType)),
    }
}

/// A `list<string>`, `key` or `key_value` parameter: its strings.
fn list_of(params: &Map<String, Value>, name: &str) -> Option<Vec<String>> {
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

fn duration(params: &Map<String, Value>, name: &str) -> Result<Option<Duration>, Exit> {
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
        self.flag("--state-root", state_root);
    }

    /// The parameter `name` as a word, when given.
    fn word_of(&mut self, params: &Map<String, Value>, name: &str) {
        if let Some(value) = params.get(name) {
            self.word(scalar(value));
        }
    }

    /// The parameter `name` as `flag <value>`, when given.
    fn flag_of(&mut self, params: &Map<String, Value>, name: &str, flag: &str) {
        if let Some(value) = params.get(name) {
            self.flag(flag, scalar(value));
        }
    }

    /// The switch `name` as `flag`, when set.
    fn switch_of(&mut self, params: &Map<String, Value>, name: &str, flag: &str) {
        if switch(params, name) {
            self.word(flag);
        }
    }

    /// `--since-unix-ms`: now less the `since` duration.
    fn since(&mut self, params: &Map<String, Value>) -> Result<(), Exit> {
        let Some(since) = duration(params, "since")? else {
            return Ok(());
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| cli_line::refused(&Outcome::new(ActingctlCode::ClockUnavailable), None))?;
        let since = now.saturating_sub(since);
        self.flag("--since-unix-ms", millis(since).to_string());
        Ok(())
    }

    /// Workflow #434 LAB-5A: the row's options and switches but those in `handled`, in the row's
    /// order, each under the flag of its name (`--` and the name with `_` as `-`), as the command's
    /// parser in `lab-ops` names them: a switch set as the flag alone, a list item by item,
    /// another value as the flag and its value. Before LAB-5A they were the row's further flags,
    /// handed on under the same names.
    fn options_of(&mut self, operation: &Operation, params: &Map<String, Value>, handled: &[&str]) {
        for param in &operation.params {
            if param.form == ParamForm::Positional || handled.iter().any(|name| *name == param.name)
            {
                continue;
            }
            if let Some(value) = params.get(&param.name) {
                self.written(&flag_name(&param.name), value);
            }
        }
    }

    /// Workflow #434 LAB-5A fix 1: the members the row does not declare, which only the command
    /// line hands on (`words::KEPT_FLAGS`: `devices bind --root`), each under the flag of its
    /// name, as the row's further flags went before LAB-5A.
    fn further(&mut self, operation: &Operation, params: &Map<String, Value>) {
        for (name, value) in super::words::further_flags(operation, params) {
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
