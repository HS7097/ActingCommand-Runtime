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
use actingcommand_lab_ops::table::Operation;
use actingcommand_lab_ops::wait::{Waited, wait_at_most};
use actingcommand_lab_ops::{
    capability_gap, code_lookup, configure, devices, diagnose, emulators, events, evidence_read,
    fact_publish, facts, frames, ladder_drill, material, observe_export, overview, packs,
    perf_preset, remedy, routine_summary, run_evidence, run_list, run_show, schedule, scheduling,
    shutdown, targets, wait, wake_test,
};
use actingcommand_runtime_client::{ContainedTaskResetOutcome, RuntimeClient, RuntimeClientError};
use serde::Serialize;
use serde_json::{Map, Value};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The member a result that is not a JSON object goes under (actingctl's `RESULT_MEMBER`).
const RESULT_MEMBER: &str = "result";
/// `pause`'s drain when none is given (the operation table's default).
const DEFAULT_DRAIN: Duration = Duration::from_secs(300);

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
    // Offline operations: no state root.
    match name {
        "codes" => {
            let mut argv = Argv::new(&["codes"]);
            argv.word_of(params, "code");
            return dispatched(code_lookup::dispatch(&argv.0));
        }
        "gap" => {
            let mut argv = Argv::new(&[capability_gap::COMMAND]);
            argv.more(operation, params);
            return dispatched(capability_gap::dispatch(&argv.0));
        }
        "packs" => {
            let mut argv = Argv::new(&["packs"]);
            argv.flag_of(params, "instance", "--instance");
            if let Some(root) = place::place().root {
                argv.flag("--root", root);
            }
            return dispatched(packs::dispatch(&argv.0));
        }
        _ => {}
    }
    let located = place::place();
    let state_root = match &located.state_root {
        Ok(state_root) => state_root.clone(),
        Err(outcome) => return cli_line::failure(outcome, Some(place::UNLOCATED)),
    };
    if let Some(exit) = dispatched_with_state_root(operation, params, &located, &state_root) {
        return exit;
    }
    // The operations called here take only the row's parameters: a further flag or word a row
    // with `params_more` let through is refused by name, never ignored.
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

/// The operations LAB-1 moved with their parsing, given actingctl's words; `None` for another.
fn dispatched_with_state_root(
    operation: &Operation,
    params: &Map<String, Value>,
    place: &Place,
    state_root: &Path,
) -> Option<Exit> {
    let detail = cli_line::detail().unwrap_or(Detail::Full);
    let root = place.root.clone();
    let exit = match operation.name.as_str() {
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
            overview::dispatch(&argv.0)
        }
        "summary" => {
            let mut argv = Argv::new(&[routine_summary::COMMAND]);
            argv.state_root(state_root);
            argv.flag_of(params, "instance", "--instance");
            argv.switch_of(params, "hour", routine_summary::HOUR);
            argv.switch_of(params, "day", routine_summary::DAY);
            argv.flag_of(params, "since", routine_summary::SINCE);
            routine_summary::dispatch(&argv.0, detail)
        }
        "events" => {
            let mut argv = Argv::new(&["events"]);
            argv.state_root(state_root);
            argv.flag_of(params, "instance", "--instance");
            if let Err(exit) = argv.since(params) {
                return Some(exit);
            }
            if let Some(types) = list_of(params, "type") {
                argv.flag("--event-types", types.join(","));
            }
            argv.flag_of(params, "run", "--run-id");
            argv.flag_of(params, "request", "--request-id");
            argv.flag_of(params, "after", "--after-sequence");
            argv.flag_of(params, "limit", "--limit");
            events::dispatch(&argv.0)
        }
        "runs" => {
            let mut argv = Argv::new(&["runs"]);
            argv.state_root(state_root);
            argv.flag_of(params, "instance", "--instance");
            if let Err(exit) = argv.since(params) {
                return Some(exit);
            }
            argv.more(operation, params);
            run_list::dispatch(&argv.0)
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
            argv.more(operation, params);
            run_evidence::dispatch(&argv.0)
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
                return Some(exit);
            }
            diagnose::dispatch(&argv.0)
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
            remedy::dispatch(&argv.0, detail)
        }
        "frame" if params.contains_key("out") => {
            let mut argv = Argv::new(&["observe"]);
            argv.state_root(state_root);
            argv.flag_of(params, "instance", "--instance");
            argv.flag_of(params, "out", "--export");
            observe_export::dispatch(&argv.0)
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
            perf_preset::dispatch(&argv.0, detail)
        }
        "evidence read" => {
            let mut argv = Argv::new(&[evidence_read::COMMAND, evidence_read::CONFIRM_READ]);
            argv.state_root(state_root);
            argv.flag_of(params, "artifact", "--artifact");
            argv.flag_of(params, "run", "--run");
            evidence_read::dispatch(&argv.0, detail)
        }
        "devices" | "devices bind" | "devices unbind" => {
            let action = match operation.name.as_str() {
                "devices bind" => devices::BIND,
                "devices unbind" => devices::UNBIND,
                _ => devices::LIST,
            };
            let mut argv = Argv::new(&[devices::COMMAND, action]);
            argv.word_of(params, "instance");
            argv.words_of(params);
            argv.state_root(state_root);
            if action != devices::UNBIND
                && let Some(root) = root
            {
                argv.flag("--root", root);
            }
            argv.more_flags(operation, params);
            devices::dispatch(&argv.0)
        }
        _ => return None,
    };
    Some(dispatched(exit))
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
        "material" => {
            let reference = required_text(params, "ref")?;
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

    /// The source command's further words (`words`), in order.
    fn words_of(&mut self, params: &Map<String, Value>) {
        for word in list_of(params, super::words::MORE_WORDS).unwrap_or_default() {
            self.word(word);
        }
    }

    /// The source command's further flags: every parameter the row does not declare.
    fn more_flags(&mut self, operation: &Operation, params: &Map<String, Value>) {
        for (name, value) in super::words::further_flags(operation, params) {
            let flag = flag_name(name);
            match value {
                Value::Bool(true) => self.word(flag.as_str()),
                Value::Bool(false) => {}
                Value::Array(items) => {
                    for item in items {
                        match item {
                            Value::Bool(true) => self.word(flag.as_str()),
                            other => self.flag(&flag, scalar(other)),
                        }
                    }
                }
                other => self.flag(&flag, scalar(other)),
            }
        }
    }

    /// The source command's further words, then its further flags.
    fn more(&mut self, operation: &Operation, params: &Map<String, Value>) {
        self.words_of(params);
        self.more_flags(operation, params);
    }
}

/// A parameter's value as one command-line word.
fn scalar(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}
