// SPDX-License-Identifier: AGPL-3.0-only

//! Observer tools (#338 §四 工具表): ac_overview and ac_material. They only read; like the
//! CLI's reads they record no client.action. Every fact comes from the Runtime and
//! runtime-client unchanged; this module only selects, pages and bounds it. ac_events lives in
//! `events` (Workflow #389 MCP-1). Errors and warnings are outcomes (Workflow #378 A5).
//! ac_overview is `actingctl overview` (`crate::overview`, Workflow #389 MCP-8): the same read
//! and result, cut here to the output budget in the order of the MCP-0 command table §4.9.
//! #422 OV-24K (v0.12.20): the budget is counted on the bytes sent in the protocol version the
//! session negotiated, the JSON once where twice does not fit (`tools::fits_sent`); `fact_refresh`
//! is cut last, and a cut answer says so.

use super::protocol::Era;
use super::runtime::{self, Connected};
use super::tools::{
    self, Arguments, McpBlocker, ToolContext, ToolError, ToolOutcome, ToolSuccess,
    invalid_argument, out_of_range,
};
use crate::codes::{ActingctlCode, ArgumentProblem};
use crate::material;
use crate::overview::{self, Failure, OverviewRequest, Place, Sections, Warning};
use actingcommand_contract::outcome::{CauseRelation, Outcome};
use actingcommand_contract::{ProjectedArtifactReference, RuntimeInstanceStatus};
use actingcommand_runtime_client::RuntimeClientError;
use serde_json::{Map, Value, json};
use std::env;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

/// MCP-0 command table §4.9: the top-level blocks ac_overview cuts first, in this order, when its
/// answer does not fit the output budget. A slice that adds an overview block registers its place
/// here in the same PR; a block the answer does not hold is passed over. Workflow #432 AGT-1b:
/// `agent_wake` is not here: it is a few members of the Status read and is never cut.
const CUT_BLOCKS: [&str; 8] = [
    "host",
    "startup",
    "tagged_state",
    "ledger",
    "performance",
    "watchdog",
    "catalog",
    "config_generation",
];
/// Then each instance row's `run`, from the last row back, then each row's `monitor`, the same
/// way, then each row's `foreground` (Workflow #389 MCP-5b registers it last of the row blocks: it
/// is the smallest); then `install.build`.
const CUT_ROW_BLOCKS: [&str; 3] = ["run", "monitor", "foreground"];
/// #422 OV-24K: last of all, what the fact refresh waits for: its rows are the warnings an
/// operator needs (`fact_refresh_unbound`, …), so it outlasts every less urgent block.
const CUT_LAST: &str = "fact_refresh";
/// #422 OV-24K: the member of a cut answer listing what was cut, in the order it was cut.
const TRUNCATED: &str = "truncated";
// Workflow #434 LAB-1b: ac_material's text bounds and the largest material it exports are
// `material`'s (`MAX_TEXT_BYTES`, `DEFAULT_TEXT_BYTES`, `material_export::MAX_EXPORT_BYTES`).
/// Time kept back from the call budget for writing the exported file.
const EXPORT_WRITE_RESERVE: Duration = Duration::from_secs(2);

/// ac_overview: `actingctl overview` over MCP (Workflow #389 MCP-8). Its arguments are the CLI's
/// flags (R2; `sections` a list where the CLI takes a comma-separated value); the server's
/// location stands for the CLI's `--root` and `--state-root`. The result is the CLI's stdout JSON
/// without its `warnings`, which go beside it, fitted to the output budget (`fit_answer`).
pub(super) fn overview(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["instance", "sections"])?;
    let instance = arguments.string("instance")?.map(str::to_owned);
    let sections = match arguments.value("sections") {
        None => Sections::ALL,
        Some(Value::Array(names)) if names.is_empty() => {
            return Err(invalid_argument("sections", ArgumentProblem::ValueMissing));
        }
        Some(Value::Array(names)) => {
            let names = names
                .iter()
                .map(|name| {
                    name.as_str()
                        .ok_or_else(|| invalid_argument("sections", ArgumentProblem::WrongType))
                })
                .collect::<Result<Vec<_>, _>>()?;
            Sections::parse(names)
                .ok_or_else(|| invalid_argument("sections", ArgumentProblem::NotRecognised))?
        }
        Some(_) => return Err(invalid_argument("sections", ArgumentProblem::WrongType)),
    };
    let request = OverviewRequest { instance, sections };
    let location = context.runtime.locate();
    let place = Place {
        root: location.root.as_deref(),
        install_root: location.install_root.as_deref(),
        state_root: location.state_root.as_deref().ok(),
    };
    let connected = context.runtime.connect();
    // No connection: the daemon block says why, as the tool error `{class, outcome}`.
    let client = match &connected {
        Ok(connected) => Ok(&connected.client),
        Err(error) => Err(error.to_value()),
    };
    let answer = overview::read(client, &request, &place, context.deadline, &tools::envelope)
        .map_err(|failure| match &failure {
            Failure::Argument { argument, problem } => invalid_argument(argument, *problem),
            Failure::Selector { .. } => ToolError::outcome("usage", failure.outcome()),
            Failure::Install(_) | Failure::Clock => {
                ToolError::outcome("runtime", failure.outcome())
            }
            Failure::Runtime(error) => runtime_failure(context, connected.as_ref().ok(), error),
        })?;
    let warnings = answer
        .warnings
        .into_iter()
        .map(|warning| match warning {
            Warning::Outcome(outcome) => tools::warning(&outcome),
            Warning::Runtime(error) => {
                runtime_failure(context, connected.as_ref().ok(), &error).into_warning()
            }
        })
        .collect::<Vec<_>>();
    Ok(fit_answer(
        Value::Object(answer.result),
        warnings,
        context.era,
    ))
}

/// #422 OV-24K: an ac_overview answer, `result` with `warnings`, within the output budget as it is
/// sent in `era`, the protocol version the session negotiated (fix 1, L1; `tools::fits_sent`: the
/// bytes sent, the JSON once beside a summary text with every warning where twice does not fit).
/// An answer that fits is returned whole. Otherwise it is never cut silently: the warning
/// `output_budget_exceeded` says how many bytes the whole answer takes (`actual_bytes`) and the
/// budget (`limit_bytes`), and `cut_to_fit` cuts it, naming each cut in `truncated`.
pub(super) fn fit_answer(mut result: Value, mut warnings: Vec<Value>, era: Era) -> ToolSuccess {
    if !tools::fits_sent(&result, &warnings, era) {
        let whole = tools::sent_size(&result, &warnings, era);
        warnings.push(tools::budget_exceeded(whole).into_warning());
        cut_to_fit(&mut result, |result| {
            tools::fits_sent(result, &warnings, era)
        });
    }
    ToolSuccess { result, warnings }
}

/// A Runtime read failure as a tool error: on the server's connection, which is dropped when the
/// client locked it.
fn runtime_failure(
    context: &ToolContext<'_>,
    connected: Option<&Connected>,
    error: &actingcommand_runtime_client::RuntimeClientError,
) -> ToolError {
    match connected {
        Some(connected) => context.runtime.failure(connected, error),
        None => runtime::client_error(error),
    }
}

/// Cuts `result`, an ac_overview answer, one block at a time in the order of the MCP-0 command
/// table §4.9 until `fits` takes it: the top-level blocks of [`CUT_BLOCKS`], then each instance
/// row's `run` from the last row back, then each row's `monitor` the same way, then each row's
/// `foreground` (MCP-5b), then `install.build`, then `fact_refresh` (#422 OV-24K: its warnings
/// outlast every less urgent block). Each cut block leaves `<block>_truncated: true` where it
/// was, and `truncated` lists the JSON pointer of every cut block in the order it was cut (#422
/// OV-24K). The daemon, the global pause, each row's identity, `emulator_state` (Workflow #390
/// 九处-1), `backend_selfcheck` (#411 A7), `not_running` (Workflow #386 SR-6b, MCP-19b),
/// `activity` (Workflow #429 R-0b2), `pause`, `lease_active` and `queued`, `incomplete` and each
/// `<block>_incomplete` are never cut: an answer that still does not fit when nothing is left to
/// cut is returned as it is, and the call sends it with as many warnings in the summary text as
/// fit, or answers `output_budget_exceeded` (`tools::tool_call_result`).
pub(super) fn cut_to_fit(result: &mut Value, fits: impl Fn(&Value) -> bool) {
    let mut cut = Vec::new();
    while !fits(result) {
        let Some(pointer) = cut_next(result) else {
            return;
        };
        cut.push(pointer);
        if let Some(members) = result.as_object_mut() {
            members.insert(TRUNCATED.to_owned(), json!(cut));
        }
    }
}

/// The next cut of [`cut_to_fit`]: the JSON pointer of the block it cut, `None` when nothing is
/// left to cut.
fn cut_next(result: &mut Value) -> Option<String> {
    let members = result.as_object_mut()?;
    for block in CUT_BLOCKS {
        if members.remove(block).is_some() {
            members.insert(format!("{block}_truncated"), json!(true));
            return Some(format!("/{block}"));
        }
    }
    let instances = overview::Section::Instances.name();
    for block in CUT_ROW_BLOCKS {
        let Some(rows) = members.get_mut(instances).and_then(Value::as_array_mut) else {
            break;
        };
        for (index, row) in rows.iter_mut().enumerate().rev() {
            if let Some(row) = row.as_object_mut()
                && row.remove(block).is_some()
            {
                row.insert(format!("{block}_truncated"), json!(true));
                return Some(format!("/{instances}/{index}/{block}"));
            }
        }
    }
    let install = overview::Section::Install.name();
    if let Some(members) = members.get_mut(install).and_then(Value::as_object_mut)
        && members.remove("build").is_some()
    {
        members.insert("build_truncated".to_owned(), json!(true));
        return Some(format!("/{install}/build"));
    }
    if members.remove(CUT_LAST).is_some() {
        members.insert(format!("{CUT_LAST}_truncated"), json!(true));
        return Some(format!("/{CUT_LAST}"));
    }
    None
}

/// The members an exported `ac_routine_summary` answer keeps beside `export`.
const ROUTINE_SUMMARY_EXPORTED_HEADER: [&str; 3] = ["owner_epoch", "granularity", "revision"];

pub(super) const ROUTINE_SUMMARY_DESCRIPTION: &str = "The routine summary's health statistics, the same read and answer as actingctl routine-summary (Workflow #429 R-12): per instance, the policy runs and direct runs the Runtime settled since it started, by task and by an hour (granularity hour) or a calendar day (granularity day) of the Runtime host's local clock (a day across a daylight saving change has 23 or 25 hours), read from the Runtime's memory; nothing is read from or written to the ledger. Health statistics only, never a count of runs done or allowed. Answers {owner_epoch, granularity, revision, instances: [{instance_alias, coverage_from_unix_ms, buckets: [{start_unix_ms, end_unix_ms, revision, tasks: [{task_id, successes, failures: [{code, count}], interruptions}]}]}]}: a run that succeeded is a success, one that failed counts under its outcome code (failures is absent when there is none), one that ended contained_task_cancelled (a stop or a drain's cancel) is an interruption, never a failure; a run counts in the bucket of its settle time. Each instance keeps its last 48 hour buckets and 14 day buckets: coverage_from_unix_ms is when its statistics are complete from, the Runtime's start until an older bucket was dropped, then the start of the oldest bucket kept. revision is the summary's revision now: pass it as since to read only the buckets changed after it; a page whose owner_epoch differs from the one the cursor came with means the Runtime restarted (its memory and revisions begin again), so drop the cursor and read without since. instance (alias, instance_id or ADB port) narrows it to that instance, resolved with one status read; without it every instance. An answer larger than the output budget is written whole to %TEMP%\\actingcommand-mcp\\materials\\<sha256>.json and answered as {owner_epoch, granularity, revision, export {path, sha256, size}} with the warning answer_exported; narrow it with instance or since.";

/// `{granularity, instance?, since?}`: `granularity` ↔ `--hour | --day`, `instance` and `since`
/// as the CLI's flags.
pub(super) fn routine_summary_input() -> Value {
    json!({
        "type": "object",
        "properties": {
            "granularity": {
                "enum": ["hour", "day"],
                "description": "The bucket width: an hour or a calendar day of the Runtime host's local clock.",
            },
            "instance": {
                "type": "string",
                "minLength": 1,
                "description": "Instance alias, instance_id or ADB port; without it every instance.",
            },
            "since": {
                "type": "integer",
                "minimum": 0,
                "description": "The revision of an earlier answer of the same owner_epoch: only the buckets changed after it.",
            },
        },
        "required": ["granularity"],
        "additionalProperties": false,
    })
}

/// The answer's shape: `RoutineSummaryPage`, or its header beside `export`.
pub(super) fn routine_summary_result() -> Value {
    let mut failure = json!({
        "type": "object",
        "properties": {"count": {"type": "integer"}},
        "required": ["code", "count"],
    });
    failure["properties"]["code"] = json!({"type": "string"});
    let task = json!({
        "type": "object",
        "properties": {
            "task_id": {"type": "string"},
            "successes": {"type": "integer"},
            "failures": {"type": "array", "items": failure},
            "interruptions": {"type": "integer"},
        },
        "required": ["task_id", "successes", "interruptions"],
    });
    json!({
        "type": "object",
        "properties": {
            "owner_epoch": {"type": "string"},
            "granularity": {"enum": ["hour", "day"]},
            "revision": {"type": "integer"},
            "instances": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "instance_alias": {"type": "string"},
                        "coverage_from_unix_ms": {"type": "integer"},
                        "buckets": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "start_unix_ms": {"type": "integer"},
                                    "end_unix_ms": {"type": "integer"},
                                    "revision": {"type": "integer"},
                                    "tasks": {"type": "array", "items": task},
                                },
                                "required": ["start_unix_ms", "end_unix_ms", "revision", "tasks"],
                            },
                        },
                    },
                    "required": ["instance_alias", "coverage_from_unix_ms", "buckets"],
                },
            },
            "export": {
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "sha256": {"type": "string"},
                    "size": {"type": "integer"},
                },
            },
        },
        "required": ["owner_epoch", "granularity", "revision"],
    })
}

/// `ac_routine_summary {granularity, instance?, since?}` (Workflow #429 R-12): the CLI's read
/// (`crate::routine_summary::read`) on the server's connection, its page the answer; a page larger
/// than the output budget is exported, as `ac_remedy_get` does.
pub(super) fn routine_summary(
    context: &ToolContext<'_>,
    arguments: &Map<String, Value>,
) -> ToolOutcome {
    use crate::routine_summary::{self as summary, Failure};
    use actingcommand_contract::RoutineSummaryGranularity;

    let arguments = Arguments::new(arguments, &["granularity", "instance", "since"])?;
    let granularity = match arguments.required_string("granularity")? {
        "hour" => RoutineSummaryGranularity::Hour,
        "day" => RoutineSummaryGranularity::Day,
        _ => {
            return Err(invalid_argument(
                "granularity",
                ArgumentProblem::NotRecognised,
            ));
        }
    };
    let selector = arguments.string("instance")?;
    let since_revision = arguments.integer("since", 0, u64::MAX)?;
    let connected = context.runtime.connect()?;
    let page = summary::read(&connected.client, selector, granularity, since_revision).map_err(
        |failure| match failure {
            Failure::Runtime(error) => context.runtime.failure(&connected, &error),
            selector @ Failure::Selector { .. } => ToolError::outcome("usage", selector.outcome()),
        },
    )?;
    let answer = json!(page);
    if tools::fits_success(&answer) {
        return Ok(ToolSuccess::new(answer));
    }
    let export = super::events::export_json(&answer)?;
    let mut exported = ROUTINE_SUMMARY_EXPORTED_HEADER
        .iter()
        .filter_map(|member| {
            answer
                .get(*member)
                .map(|value| ((*member).to_owned(), value.clone()))
        })
        .collect::<Map<_, _>>();
    let mut warning = Outcome::new(ActingctlCode::AnswerExported);
    if let Some(path) = export["path"].as_str() {
        warning = warning.path(path);
    }
    exported.insert("export".to_owned(), export);
    Ok(ToolSuccess {
        result: Value::Object(exported),
        warnings: vec![tools::warning(&warning)],
    })
}

/// The instance an `instance` argument names by alias, instance_id or ADB port, as the
/// Runtime's status lists them: the selector the CLI shares (`crate::instance_selector`).
pub(super) fn select_instance<'s>(
    instances: &'s [RuntimeInstanceStatus],
    selector: &str,
) -> Result<&'s RuntimeInstanceStatus, ToolError> {
    crate::instance_selector::select(instances, selector)
        .map_err(|miss| ToolError::outcome("usage", miss.outcome(selector)))
}

// One committed material, found again from its ref at the ledger's current snapshot (or, for
// ac_observe, the frame its observation just recorded). Workflow #434 LAB-1b: `material`'s.
pub(super) use crate::material::Material;

/// Workflow #434 LAB-1b: the ref, the reads and the text answer are `material`'s; the call
/// budget, the cancellation and the export directory stay here.
pub(super) fn material(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["ref", "mode", "offset", "max_bytes"])?;
    let reference = arguments.required_string("ref")?;
    let mode = arguments.required_string("mode")?;
    let (position, artifact_id) =
        material::parse_ref(reference).map_err(|failure| material_error(context, None, failure))?;
    match mode {
        "text" => {
            let offset = arguments.integer("offset", 0, u64::MAX)?.unwrap_or(0);
            let max_bytes = arguments
                .integer("max_bytes", 1, material::MAX_TEXT_BYTES)?
                .unwrap_or(material::DEFAULT_TEXT_BYTES);
            let connected = context.runtime.connect()?;
            let found = material::resolve(&connected.client, position, artifact_id)
                .map_err(|failure| material_error(context, Some(&connected), failure))?;
            material::read_text(
                &connected.client,
                &found,
                offset,
                max_bytes,
                &tools::fits_success,
            )
            .map(ToolSuccess::new)
            .map_err(|failure| material_error(context, Some(&connected), failure))
        }
        "export" => {
            // offset and max_bytes belong to mode text; export writes the whole material.
            for argument in ["offset", "max_bytes"] {
                if arguments.has(argument) {
                    return Err(invalid_argument(argument, ArgumentProblem::Conflicting));
                }
            }
            let connected = context.runtime.connect()?;
            let found = material::resolve(&connected.client, position, artifact_id)
                .map_err(|failure| material_error(context, Some(&connected), failure))?;
            material_export(context, &connected, &found)
        }
        _ => Err(invalid_argument("mode", ArgumentProblem::NotRecognised)),
    }
}

/// A material failure as a tool error, with the classes ac_material always answered: an
/// argument's or a ref's failure `usage` (a material that is no text with `mcp_blocker`
/// `material_mode`), a material the Runtime did not provide `runtime`, a Runtime failure the
/// client's on the server's connection (with what a whole read reached as its `note` link).
fn material_error(
    context: &ToolContext<'_>,
    connected: Option<&Connected>,
    failure: material::Failure,
) -> ToolError {
    let client_failure = |error: &RuntimeClientError| match connected {
        Some(connected) => context.runtime.failure(connected, error),
        None => runtime::client_error(error),
    };
    match failure {
        material::Failure::Usage(outcome) => ToolError::outcome("usage", outcome),
        material::Failure::NotText(outcome) => {
            ToolError::outcome("usage", outcome).blocked_by(McpBlocker::MaterialMode)
        }
        material::Failure::OutOfRange {
            argument,
            minimum,
            maximum,
        } => out_of_range(argument, minimum, maximum),
        material::Failure::Unavailable(outcome) => ToolError::outcome("runtime", outcome),
        material::Failure::Runtime(error) => client_failure(&error),
        material::Failure::RuntimeNoted { error, reached } => {
            client_failure(&error).with(|outcome| outcome.link(CauseRelation::Note, reached))
        }
    }
}

/// ac_material mode export and ac_observe: the whole material, read within the call budget until
/// the call is cancelled (`material::read_whole`), written to the export directory. Workflow #434
/// LAB-1b: the read is `material`'s.
pub(super) fn material_export(
    context: &ToolContext<'_>,
    connected: &Connected,
    material: &Material,
) -> ToolOutcome {
    let deadline = context
        .deadline
        .checked_sub(EXPORT_WRITE_RESERVE)
        .unwrap_or(context.deadline);
    let keep_reading = || !context.cancelled.load(Ordering::SeqCst);
    let bytes = material::read_whole(&connected.client, material, deadline, &keep_reading)
        .map_err(|failure| material_error(context, Some(connected), failure))?;
    write_export(&material.artifact, &bytes)
}

/// Writes the verified bytes to `%TEMP%\actingcommand-mcp\materials\<sha256>.<ext>`
/// through a partial file moved into place.
fn write_export(artifact: &ProjectedArtifactReference, bytes: &[u8]) -> ToolOutcome {
    export_bytes(&artifact.sha256, artifact.kind.extension(), bytes).map(ToolSuccess::new)
}

/// Where every export of this server goes (ac_material export, Lab answers over budget).
pub(super) fn export_directory() -> PathBuf {
    env::temp_dir().join("actingcommand-mcp").join("materials")
}

/// The export writer (`crate::material_export::write`, which `actingctl observe --export`
/// shares) on the export directory: `bytes`, whose `sha256` (`sha256:<hex>`) the caller knows, to
/// `<export directory>\<hex>.<extension>` through a partial file moved into place;
/// `{path, sha256, size}`. A failed write is the writer's `material_export_failed`, class
/// runtime.
pub(super) fn export_bytes(
    sha256: &str,
    extension: &str,
    bytes: &[u8],
) -> Result<Value, ToolError> {
    crate::material_export::write(&export_directory(), sha256, extension, bytes)
        .map_err(|failure| ToolError::outcome("runtime", failure.outcome()))
}
