// SPDX-License-Identifier: AGPL-3.0-only

//! The single static tool table (#338 §二, §四 工具表). `tools/list`, `--list-tools`, the
//! `mcp-config` example and the skill's generated section all read it. Here too are the
//! tier gate, the result envelope `{ok, result, warnings?}` / `{ok:false, error}` and the
//! output budget (§四 结果形状 / 预算).

use super::events;
use super::jobs::Jobs;
use super::lab;
use super::observer;
use super::operator;
use super::protocol::Era;
use super::runs;
use super::runtime::RuntimeAccess;
use crate::codes::ActingctlCode;
use actingcommand_contract::outcome::Code;
use serde_json::{Map, Value, json};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

/// The `instructions` both clients read (§四 instructions).
pub(super) const INSTRUCTIONS: &str = "ActingCommand local control (tools-only). Start with ac_overview. Long operations return a handle at once; a run's handle is its Runtime request_id and survives restarts, other job handles last only as long as this process; poll ac_get_run with wait_s. If an ac_run_pack call was interrupted before it returned, call ac_overview before submitting again. After an uncertain result call ac_get_run before anything else; never resend a write with new arguments. Approvals, actingd configuration edits and daemon restarts belong to the person. Tiers: observer (read), operator (device/scheduling), author (Lab recording); tools outside the enabled tiers answer tier_not_enabled. Manual: skill `actingcommand`.";

/// The end of every Lab tool's description: its job handle lives in this process only.
macro_rules! lab_job {
    () => {
        " Not done within the call budget, the answer is {handle, job_phase} for ac_get_run; actinglab keeps running and is never stopped from here. This handle is a job of this MCP process and ends with it (ac_get_run then answers handle_unknown); afterwards read the recording state with ac_record_status."
    };
}

/// The end of every offline check's description.
macro_rules! check_job {
    () => {
        " Not done within the call budget, the answer is {handle, job_phase} for ac_get_run. This handle is a job of this MCP process and ends with it (ac_get_run then answers handle_unknown); afterwards run the check again."
    };
}

/// How a Lab tool reports actinglab's failures.
macro_rules! lab_errors {
    () => {
        " Failures keep actinglab's class by its exit code (2 usage, 3 safety, 4 device, 5 runtime, 6 usage not_implemented) with its error verbatim in details.lab_error; any other exit, or no JSON envelope, is runtime lab_process_failed with the stderr tail. String arguments may not start with --. An answer larger than the output budget is written whole to %TEMP%\\actingcommand-mcp\\materials\\<sha256>.json and answered as {req_id, export {path, sha256, size}, overflowed: true} with the warning answer_exported; an error that large keeps its class, code and message, with lab_error trimmed to its code, message and scalar details, lab_error_trimmed, req_id and export."
    };
}

/// The longest a tool call waits inside the server; Runtime IO itself is bounded at 5 s.
pub(super) const CALL_BUDGET: Duration = Duration::from_secs(25);
pub(super) const DEFAULT_LIST: u64 = 20;
pub(super) const MAX_LIST: u64 = 100;
/// The structured payload, which the text content repeats.
const PAYLOAD_LIMIT: usize = 11 * 1024;
/// The whole `tools/call` result: text plus structuredContent.
const RESULT_LIMIT: usize = 24 * 1024;
/// Room for the 2026-07-28 `resultType` and `_meta` server identity.
const MODERN_FIELDS_RESERVE: usize = 256;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Tier {
    Observer,
    Operator,
    Author,
}

impl Tier {
    const ALL: [Self; 3] = [Self::Observer, Self::Operator, Self::Author];

    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Observer => "observer",
            Self::Operator => "operator",
            Self::Author => "author",
        }
    }
}

/// The tiers this process serves. observer is always on; operator and author only when
/// `--tier` names them.
#[derive(Clone, Copy, Default)]
pub(super) struct TierSet {
    operator: bool,
    author: bool,
}

impl TierSet {
    /// `observer|operator|author[,…]`; `None` for an empty or unknown name.
    pub(super) fn parse(text: &str) -> Option<Self> {
        let mut tiers = Self::default();
        for name in text.split(',') {
            match Tier::ALL
                .into_iter()
                .find(|tier| tier.as_str() == name.trim())?
            {
                Tier::Observer => {}
                Tier::Operator => tiers.operator = true,
                Tier::Author => tiers.author = true,
            }
        }
        Some(tiers)
    }

    pub(super) const fn contains(self, tier: Tier) -> bool {
        match tier {
            Tier::Observer => true,
            Tier::Operator => self.operator,
            Tier::Author => self.author,
        }
    }

    /// The `--tier` argument naming exactly these tiers.
    pub(super) fn argument(self) -> String {
        Tier::ALL
            .into_iter()
            .filter(|tier| self.contains(*tier))
            .map(Tier::as_str)
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// What a tool sees of the server while it runs.
pub(super) struct ToolContext<'a> {
    pub(super) runtime: &'a RuntimeAccess,
    /// Set by `notifications/cancelled`: stop waiting; the answer is not sent.
    pub(super) cancelled: &'a AtomicBool,
    /// The end of this call's 25 s budget.
    pub(super) deadline: Instant,
    /// This server process's identity; it binds every cursor it issues.
    pub(super) session: u64,
    pub(super) jobs: &'a Jobs,
}

pub(super) type ToolOutcome = Result<ToolSuccess, ToolError>;

pub(super) struct ToolSuccess {
    pub(super) result: Value,
    pub(super) warnings: Vec<Value>,
}

impl ToolSuccess {
    pub(super) const fn new(result: Value) -> Self {
        Self {
            result,
            warnings: Vec::new(),
        }
    }
}

/// `{class, code, message, blocked_by, details}`; boxed so results stay small.
pub(super) struct ToolError(Box<ToolErrorBody>);

struct ToolErrorBody {
    class: &'static str,
    code: ToolCode,
    message: String,
    blocked_by: Option<String>,
    details: Map<String, Value>,
}

/// The code a tool error answers with.
pub(super) enum ToolCode {
    /// A code this server emits.
    Emitted(Code),
    /// A code passed on as another program or the Runtime reported it: actinglab's envelope
    /// error, a job's stored error and the Runtime's material read failure, which A5's
    /// `{class, outcome}` relays as links. It also carries actingctl's own dynamic family
    /// `material_<state>` (a material read without a failure): a code that carries the read
    /// state, which A5 splits into one fixed code with a `material_state` value instead of
    /// relaying it.
    Relayed { relayed_code: String },
}

impl ToolCode {
    /// The code's spelling.
    fn as_str(&self) -> &str {
        match self {
            Self::Emitted(code) => code.as_str(),
            Self::Relayed { relayed_code } => relayed_code,
        }
    }
}

impl ToolError {
    pub(super) fn new(
        class: &'static str,
        code: impl Into<Code>,
        message: impl Into<String>,
    ) -> Self {
        Self::coded(class, ToolCode::Emitted(code.into()), message)
    }

    /// An error with this code, emitted or relayed.
    pub(super) fn coded(class: &'static str, code: ToolCode, message: impl Into<String>) -> Self {
        Self(Box::new(ToolErrorBody {
            class,
            code,
            message: message.into(),
            blocked_by: None,
            details: Map::new(),
        }))
    }

    pub(super) fn usage(code: impl Into<Code>, message: impl Into<String>) -> Self {
        Self::new("usage", code, message)
    }

    pub(super) fn with_detail(mut self, key: &str, value: Value) -> Self {
        self.0.details.insert(key.to_owned(), value);
        self
    }

    /// What has to change before the call can succeed.
    pub(super) fn blocked_by(mut self, pointer: impl Into<String>) -> Self {
        self.0.blocked_by = Some(pointer.into());
        self
    }

    /// The code this server emitted; `None` for a relayed one.
    pub(super) fn code(&self) -> Option<Code> {
        match &self.0.code {
            ToolCode::Emitted(code) => Some(*code),
            ToolCode::Relayed { .. } => None,
        }
    }

    /// The spelling of the code, emitted or relayed, as the error's JSON carries it.
    pub(super) fn code_spelling(&self) -> &str {
        self.0.code.as_str()
    }

    /// One member of the details, when present.
    pub(super) fn detail(&self, key: &str) -> Option<&Value> {
        self.0.details.get(key)
    }

    /// `{class, code, message, blocked_by, details}`, leaving the error in place.
    pub(super) fn to_value(&self) -> Value {
        json!({
            "class": self.0.class,
            "code": self.0.code.as_str(),
            "message": self.0.message,
            "blocked_by": self.0.blocked_by,
            "details": self.0.details,
        })
    }

    pub(super) fn message(&self) -> &str {
        &self.0.message
    }

    /// `{class, code, message, blocked_by, details}`.
    pub(super) fn into_value(self) -> Value {
        let body = *self.0;
        json!({
            "class": body.class,
            "code": body.code.as_str(),
            "message": body.message,
            "blocked_by": body.blocked_by,
            "details": body.details,
        })
    }

    /// The Runtime's host failure code this error carries, if any.
    pub(super) fn host_code(&self) -> Option<&str> {
        self.0
            .details
            .get("host_failure")
            .and_then(|failure| failure.get("code"))
            .and_then(Value::as_str)
    }

    /// The same failure reported beside a successful result.
    pub(super) fn into_warning(self) -> Value {
        let body = *self.0;
        json!({
            "code": body.code.as_str(),
            "message": body.message,
            "blocked_by": body.blocked_by,
            "details": body.details,
        })
    }
}

/// A tool's arguments, checked against its input schema by hand: unknown keys are refused,
/// as `additionalProperties: false` says.
pub(super) struct Arguments<'a> {
    map: &'a Map<String, Value>,
}

impl<'a> Arguments<'a> {
    pub(super) fn new(map: &'a Map<String, Value>, allowed: &[&str]) -> Result<Self, ToolError> {
        match map.keys().find(|key| !allowed.contains(&key.as_str())) {
            Some(unknown) => Err(invalid_argument(unknown, "is not an argument of this tool")),
            None => Ok(Self { map }),
        }
    }

    /// The argument as given, when present.
    pub(super) fn value(&self, key: &str) -> Option<&'a Value> {
        self.map.get(key).filter(|value| !value.is_null())
    }

    pub(super) fn has(&self, key: &str) -> bool {
        self.map.get(key).is_some_and(|value| !value.is_null())
    }

    pub(super) fn string(&self, key: &str) -> Result<Option<&'a str>, ToolError> {
        match self.map.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(text)) if !text.is_empty() => Ok(Some(text.as_str())),
            Some(_) => Err(invalid_argument(key, "must be a non-empty string")),
        }
    }

    pub(super) fn required_string(&self, key: &str) -> Result<&'a str, ToolError> {
        self.string(key)?
            .ok_or_else(|| invalid_argument(key, "is required"))
    }

    pub(super) fn integer(
        &self,
        key: &str,
        minimum: u64,
        maximum: u64,
    ) -> Result<Option<u64>, ToolError> {
        match self.map.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(value) => value
                .as_u64()
                .filter(|number| (minimum..=maximum).contains(number))
                .map(Some)
                .ok_or_else(|| {
                    invalid_argument(
                        key,
                        &format!("must be an integer from {minimum} to {maximum}"),
                    )
                }),
        }
    }
}

pub(super) fn invalid_argument(field: &str, reason: &str) -> ToolError {
    ToolError::usage(
        ActingctlCode::ArgumentsInvalid,
        format!("argument {field} {reason}"),
    )
    .with_detail("field", json!(field))
}

/// One row of the tool table.
pub(super) struct ToolDef {
    pub(super) name: &'static str,
    title: &'static str,
    pub(super) tier: Tier,
    description: &'static str,
    read_only: bool,
    destructive: bool,
    idempotent: bool,
    input_schema: fn() -> Value,
    result_schema: fn() -> Value,
    pub(super) run: fn(&ToolContext<'_>, &Map<String, Value>) -> ToolOutcome,
}

/// Every tool in this build; `tools/list` serves those of the enabled tiers.
pub(super) static TOOLS: &[ToolDef] = &[
    ToolDef {
        name: "ac_overview",
        title: "ActingCommand overview",
        tier: Tier::Observer,
        description: "Snapshot of the local ActingCommand Runtime; call it first. Returns daemon {online, owner_epoch}, global_pause, and one row per configured instance: alias, instance_id, adb_port, game_id, lease_active, queued requests, its scheduling pause {revision, stage, pause_reason}, its resource_fence {cause, since_unix_ms, awaiting_person} when an unconfirmed close of its own resources fenced it, its monitor, and run: its newest run admitted in run_window (the last 24 h; the open run when one is open) as a brief actingcommand.run-status.v1, with run_incomplete when that read did not finish; plus lab_tool {present} and install {root, state_root, build}. When actingd does not answer, daemon is {online: false, error {code, message}} (code runtime_unavailable, or install_state_root_unresolved) and there is no instances field at all: that is no answer, not an empty system. instance (alias, instance_id or ADB port) narrows the rows to one. incomplete true means part of the snapshot could not be read (see warnings). It only reads: it starts, stops and pauses nothing, and it neither shows nor infers whether an emulator is running.",
        read_only: true,
        destructive: false,
        idempotent: true,
        input_schema: overview_input,
        result_schema: overview_result,
        run: observer::overview,
    },
    // Workflow #389 MCP-1 (MCP-0 command table v1.1 §4.1): ac_events is `actingctl events`.
    ToolDef {
        name: "ac_events",
        title: "Runtime ledger events",
        tier: Tier::Observer,
        description: "One page of Runtime ledger events, oldest first: the same read, parameters and page as actingctl events (QueryEvents, which records nothing in the ledger). Filters, all combined: instance (alias, instance_id or ADB port), view (events = every event, observation, changes, errors = warning and above, health, lab), since_unix_ms and until_unix_ms, after_sequence and through_sequence, event_type, exclude_event_types (at most 32, never the event_type), min_severity and max_severity (debug, info, warning, error, fatal), origin_module, task_id, run_id, request_id and correlation_id. Without since_unix_ms or after_sequence the page starts at the beginning of the ledger. Each row carries seq, ts, type, severity, instance, request_id, run_id, task_id, correlation_id, the ledger's diagnostic code and materials [{ref, media_type, size}] for ac_material (materials_truncated when that list was shortened to fit the output budget); detail payload adds the event's projected payload verbatim, detail brief (the default) has none. limit defaults to 20 (at most 100). last_sequence is the seq of the page's last row: pass it as after_sequence to read on from there, also after a restart. next_cursor continues the same query within its snapshot: send it back with the same filters and without after_sequence. truncated true means a brief page was cut to the output budget and next_cursor resumes after its last row. A payload page larger than the output budget is written whole to %TEMP%\\actingcommand-mcp\\materials\\<sha256>.json and answered as {export {path, sha256, size}, events_count, next_cursor, last_sequence} with the warning answer_exported. A cursor dies when the Runtime connection or this server restarts (cursor_invalid): read again from the first page or from after_sequence. To follow the ledger, poll this tool with after_sequence and wait_s (0-20, default 0), not ac_overview: an empty page is read again every second until rows arrive or wait_s ends, and waited_ms says how long it waited.",
        read_only: true,
        destructive: false,
        idempotent: true,
        input_schema: events::input_schema,
        result_schema: events::result_schema,
        run: events::events,
    },
    ToolDef {
        name: "ac_material",
        title: "Runtime material",
        tier: Tier::Observer,
        description: "Reads one committed material named by a materials[].ref of ac_events, after the Runtime verified it. mode text returns up to max_bytes (default 4096, at most 8192) of a text or JSON material from offset, with next_offset while more remains. mode export writes the whole verified material to %TEMP%\\actingcommand-mcp\\materials\\<sha256>.<ext> and returns only {path, sha256, size}; use it for screenshots and archives, which are never inlined.",
        read_only: true,
        destructive: false,
        idempotent: true,
        input_schema: material_input,
        result_schema: material_result,
        run: observer::material,
    },
    // Workflow #389 MCP-5a (MCP-0 command table v1.1 §4.5): ac_observe ↔ actingctl observe --export.
    ToolDef {
        name: "ac_observe",
        title: "Observe one frame",
        tier: Tier::Observer,
        description: "Takes one frame of one instance now, as actingctl observe --export does: instance (alias, instance_id or ADB port) is resolved with one status read, the Runtime captures the frame read-only (no input, no run, nothing started or stopped) and records it in its ledger and material store, and the frame is read back verified and written whole to %TEMP%\\actingcommand-mcp\\materials\\<sha256>.png. Returns {width, height, verdict, capture_backend, frame {ref, media_type, size, sha256}, export {path, sha256, size}}; frame.ref is a materials ref for ac_material. Every call captures a new frame. A frame over 32 MiB answers material_too_large and writes nothing.",
        read_only: true,
        destructive: false,
        idempotent: false,
        input_schema: instance_only_input,
        result_schema: super::observe_export::result,
        run: super::observe_export::observe,
    },
    // End of MCP-5a.
    ToolDef {
        name: "ac_get_run",
        title: "Run status",
        tier: Tier::Observer,
        description: "One contained run as actingcommand.run-status.v1, read in full from its ledger events by handle (the Runtime request_id an ac_run_pack returns) or by run_id; give exactly one. The state is not_found, admitted, running, succeeded, failed, cancelled or interrupted_unterminated (admitted before a later Runtime start and never ended: uncertain). wait_s (0-25, default 0) waits for a change: the run is read again every second while its state is not_found, admitted or running, within the 25 s call budget; then the latest status is returned. Call it again to keep waiting. request_id is null only when a run_id lookup found no run. A submit refused before admission stays not_found with job.phase failed and the Runtime's error (for example QueueExpired, class safety, after a wait for a busy instance) in job.outcome.error. Another job handle of this process (ac_pause, ac_resume, ac_emulator, ac_stop_run or a Lab tool) answers {handle, job {kind, phase, warnings, outcome}}; a job handle this process does not hold answers handle_unknown.",
        read_only: true,
        destructive: false,
        idempotent: true,
        input_schema: get_run_input,
        result_schema: get_run_result,
        run: runs::get_run,
    },
    ToolDef {
        name: "ac_diagnose",
        title: "Diagnosis",
        tier: Tier::Observer,
        // Workflow #389 MCP-7 (command table §4.8): instance optional; runs_grouped and
        // failures_grouped; the generation actingd suspended reads is found at every call.
        description: "What went wrong within a bounded window, on one instance or, without instance, on every instance: the same read and result as actingctl diagnose. since_unix_ms is the window start (default the last 24 h, at most 7 days back, else since_out_of_range); the window used is echoed. Returns instance {alias, instance_id} (only with instance); runs: the recent failed or open runs (failed, admitted, running, interrupted_unterminated) as brief actingcommand.run-status.v1, newest first, from the 10 most recent runs; errors_page: the first page (oldest first) of the errors view in the window, rows as in ac_events, with next_cursor to continue in ac_events (view errors, same instance and since_unix_ms) when no task_id filter is set; suspended, lift and repeating: the rows of the read-only actingd suspended report (the instance's, else all), verbatim, and report_warnings: that report's warnings; clock_slots: at most 10 clock_slots rows of that report recorded since since_unix_ms (due clock slots refused, deferred or served late: refused and deferred rows first, then owed rows, newest first in each), verbatim, with clock_slots_total when fewer are kept (the full list is in actingd suspended); suspended_report: that report's status (unavailable with suspended_report_state_root_mismatch when it read another state root than this server). actingd suspended reports the configuration of the generation install\\active.json selects at this call, found again at every call, not the packs the running Runtime has loaded. runs_grouped: every failed or open run of the window grouped by (catalog_task_id: the policy task id of a scheduled run, absent for a manual run; failure_code; final_page), each {count, first_seq, last_seq, latest_request_id, instances}; failures_grouped: the window's runtime.failed and command.rejected records and the policy.execution_recorded records of runs that paused their task, grouped by (type, catalog_task_id, code), each {count, first_seq, last_seq}; both most frequent first, read with QueryEvents, which records nothing; a relayed code the catalog does not register keeps its relayed spelling. task_id narrows runs, errors and both groups to one Runtime task. incomplete true means a read did not finish. To fit the output budget, lists are cut in this order: failures_grouped, runs_grouped (the fewest-count groups first), clock_slots (owed rows first, oldest first), errors_page, runs (oldest first), repeating, lift, suspended, report_warnings, each marked <list>_truncated; runs cut for budget can be read with ac_get_run / ac_events.",
        read_only: true,
        destructive: false,
        idempotent: true,
        input_schema: diagnose_input,
        result_schema: diagnose_result,
        run: runs::diagnose,
    },
    // Workflow #389 MCP-3 (MCP-0 command table v1.1 §4.3): ac_runs ↔ actingctl runs.
    ToolDef {
        name: "ac_runs",
        title: "Run list",
        tier: Tier::Observer,
        description: "The contained runs, manual and scheduled, first recorded in a window, newest first, as actingctl runs lists them: since_unix_ms (default the last 24 h, at most 7 days back, else since_out_of_range) and until_unix_ms (optional, not before since_unix_ms); the window used is echoed. One row per run: request_id, run_id, task_id, catalog_task_id (the policy task id of a scheduled run; absent for a manual run), alias, instance_id, dispatch, origin, state, admitted_at_unix_ms (its first fact), ended_at_unix_ms and duration_ms (once it ended), task_result, failure_code, final_page and executed_steps (from its terminal). Filters: instance (alias, instance_id or ADB port; without it every instance), task_prefix (catalog_task_id prefix; a manual run never matches), dispatch (manual or scheduled), state (open = admitted or running, succeeded, failed, cancelled, interrupted_unterminated). limit defaults to 20 (at most 100). next_cursor continues the same list at the same ledger snapshot: send it back with the same filters (since_unix_ms and until_unix_ms may be left out); it stays valid across restarts. truncated true means the page was cut to the output budget; incomplete true, with the warning call_budget_exhausted, means the read budget ended the page early; next_cursor continues either. Runs are read run by run, so an instance's other events do not count. It only reads: one status read at most names the instance and the rows' aliases.",
        read_only: true,
        destructive: false,
        idempotent: true,
        input_schema: super::run_list::input,
        result_schema: super::run_list::result,
        run: super::run_list::runs,
    },
    // End of MCP-3.
    ToolDef {
        name: "ac_resources_list",
        title: "Targetable resources",
        tier: Tier::Observer,
        description: "What one instance can target, as the Runtime reports it, unchanged: targetable resources (resource, fact_key, producing tasks, defaults, what must be given, the current observation) and the not-targetable ones with their reason. policy_instance is the value the instance field of a policy document takes. An actingd older than v0.11.0 answers runtime_operation_unsupported: read the resources of the scheduling catalog instead.",
        read_only: true,
        destructive: false,
        idempotent: true,
        input_schema: instance_only_input,
        result_schema: resources_list_result,
        run: operator::resources_list,
    },
    ToolDef {
        name: "ac_targets_get",
        title: "Active resource targets",
        tier: Tier::Observer,
        description: "The resource target policy one instance holds now, as the Runtime reports it, unchanged: active {policy_sha256, schema_version, version, event_id, valid_until_unix_ms, expired, targets, conditions}, or no active field when it holds none. An actingd older than v0.11.0 answers runtime_operation_unsupported: read the resources of the scheduling catalog instead.",
        read_only: true,
        destructive: false,
        idempotent: true,
        input_schema: instance_only_input,
        result_schema: targets_get_result,
        run: operator::targets_get,
    },
    // ---- Workflow #389 MCP-11 (begin): `ac_packs` ↔ `actingctl packs`; observer before U-3,
    // a general read after it (command table §4.12). ----
    ToolDef {
        name: "ac_packs",
        title: "Bound packages",
        tier: Tier::Observer,
        description: super::packs::DESCRIPTION,
        read_only: true,
        destructive: false,
        idempotent: true,
        input_schema: super::packs::input_schema,
        result_schema: super::packs::result_schema,
        run: super::packs::run,
    },
    // ---- Workflow #389 MCP-11 (end) ----
    ToolDef {
        name: "ac_pack_check",
        title: "Check a task package",
        tier: Tier::Observer,
        description: concat!(
            "Checks one task package offline, as actinglab --json package digest and package preflight do, and answers both verbatim: {digest, preflight}. preflight runs with package_ref when it is given, otherwise with the digest's reference. With package_ref, package_ref_check {given, matches_digest} says whether it is the digest's reference, and preflight is actinglab's verdict on it. preflight's coverage says how far the check went: it neither recognizes nor executes anything. A failed preflight keeps the digest in details.digest.",
            lab_errors!(),
            check_job!()
        ),
        read_only: true,
        destructive: false,
        idempotent: true,
        input_schema: lab::pack_check_input,
        result_schema: lab::pack_check_result,
        run: lab::pack_check,
    },
    ToolDef {
        name: "ac_catalog_check",
        title: "Check a business catalog",
        tier: Tier::Observer,
        description: concat!(
            "Compiles one business identity catalog offline, as actinglab --json resource catalog --repo <repo> --catalog <catalog> --catalog-server <server> [--field <field>] does, and answers its compile result verbatim. catalog is relative to repo; field defaults to business_id. It only reads the catalog file and writes nothing.",
            lab_errors!(),
            check_job!()
        ),
        read_only: true,
        destructive: false,
        idempotent: true,
        input_schema: lab::catalog_check_input,
        result_schema: lab::lab_result,
        run: lab::catalog_check,
    },
    ToolDef {
        name: "ac_run_pack",
        title: "Run a task package",
        tier: Tier::Operator,
        description: "Runs one task package on one instance, as actingctl task-run does, and answers at once with {handle, correlation_id, phase: submitting}; handle is the run's Runtime request_id: follow it with ac_get_run (wait_s) until the state is terminal. package is a package path; package_ref defaults to the actinglab package digest of it (needs the install's tools). A recovery package is recovery_package with an optional recovery_package_ref, never the ref alone. deadline_s (60-1800, default 1800) bounds the run. It never pauses scheduling: on a busy instance the run waits for its turn in the instance's queue, at most min(60 s, its deadline minus 30 s); not granted in time it comes back in ac_get_run's job.outcome.error as the Runtime's QueueExpired (lease_queue_expired), class safety; for exclusive use call ac_pause before and ac_resume after. If this call is interrupted before it answers, call ac_overview before submitting again.",
        read_only: false,
        destructive: false,
        idempotent: false,
        input_schema: run_pack_input,
        result_schema: run_pack_result,
        run: operator::run_pack,
    },
    ToolDef {
        name: "ac_stop_run",
        title: "Stop a run",
        tier: Tier::Operator,
        description: "Stops one manual run (MCP, CLI, UI or Lab) by its handle, the Runtime request_id, from any process: the run's package is abandoned and the screen stays where it is; there is no return home, no recovery package and no rerun; only touch points that may still be held are lifted. Answers {cancellation, touch_release, job_phase}; touch_release is done, failed or not_needed: not_needed includes a run that had already ended before this call, and failed (for example QueueExpired) can mean the still-connected submitter already lifted them itself. When this server submitted the run and its job still waits, only the stop is sent and that job lifts the touches. Otherwise a background job waits for the run to end, up to the longest run deadline; not done within wait_s (default 20) the answer is {handle, job_phase} for ac_get_run. This handle is a job of this MCP process and ends with it (ac_get_run then answers handle_unknown); afterwards read the pause's revision and owner_epoch, or the emulator's state, from ac_overview, and a stopped run with ac_get_run by its request_id. A scheduled run cannot be stopped by a client: the Runtime's refusal comes back, blocked_by ac_pause.",
        read_only: false,
        destructive: false,
        idempotent: false,
        input_schema: stop_run_input,
        result_schema: stop_run_result,
        run: operator::stop_run,
    },
    ToolDef {
        name: "ac_pause",
        title: "Pause scheduling",
        tier: Tier::Operator,
        description: "Pauses scheduling, the same pause as actingctl pause: for one instance (alias, instance_id or ADB port) or, without instance, everywhere. reason is a code (default mcp.pause); drain_timeout_s 1-600 (default 60). An instance pause drains every in-flight contained run on that instance, manual runs and other sessions' runs included: when the drain times out the Runtime asks them all to stop. The pause stays until ac_resume: it survives an actingd restart (restored with a new owner_epoch and revision 1; an instance pause comes back released) and does not end with this MCP process. Answers {paused (the Runtime's result with its revision), owner_epoch}: pass the owner_epoch and the revision to ac_resume. Not done within the call budget, the answer is {handle, job_phase} for ac_get_run. This handle is a job of this MCP process and ends with it (ac_get_run then answers handle_unknown); afterwards read the pause's revision and owner_epoch, or the emulator's state, from ac_overview, and a stopped run with ac_get_run by its request_id.",
        read_only: false,
        destructive: false,
        idempotent: false,
        input_schema: pause_input,
        result_schema: job_result,
        run: operator::pause,
    },
    ToolDef {
        name: "ac_resume",
        title: "Resume scheduling",
        tier: Tier::Operator,
        description: "Lifts exactly the scheduling pause the caller saw: expected_owner_epoch and expected_revision are the owner_epoch and the revision ac_pause gave (or ac_overview shows). The Runtime refuses any other pause (scheduling_pause_owner_epoch_mismatch, scheduling_pause_revision_mismatch): someone else's pause, or one made again or restored after a restart (read its owner_epoch and revision from ac_overview). An older actingd answers runtime_operation_unsupported and nothing is lifted. An instance resume reconnects the device at once and runs its self-check. Answers {resumed} or, beyond the call budget, {handle, job_phase} for ac_get_run. This handle is a job of this MCP process and ends with it (ac_get_run then answers handle_unknown); afterwards read the pause's revision and owner_epoch, or the emulator's state, from ac_overview, and a stopped run with ac_get_run by its request_id.",
        read_only: false,
        destructive: false,
        idempotent: false,
        input_schema: resume_input,
        result_schema: job_result,
        run: operator::resume,
    },
    ToolDef {
        name: "ac_emulator",
        title: "Emulator control",
        tier: Tier::Operator,
        description: "Starts, stops or restarts one instance's emulator through the Runtime (action start, stop or restart), as actingctl emulator does; the Runtime takes it only from an operator origin. stop and restart end whatever runs on that emulator. It can take up to 235 s (a restart, a stop and then a start, up to 445 s): within the call budget the answer is {controlled}, otherwise {handle, job_phase} for ac_get_run. This handle is a job of this MCP process and ends with it (ac_get_run then answers handle_unknown); afterwards read the pause's revision and owner_epoch, or the emulator's state, from ac_overview, and a stopped run with ac_get_run by its request_id.",
        read_only: false,
        destructive: true,
        idempotent: false,
        input_schema: emulator_input,
        result_schema: job_result,
        run: operator::emulator,
    },
    ToolDef {
        name: "ac_targets_set",
        title: "Set resource targets",
        tier: Tier::Operator,
        description: "Sets one instance's resource targets: targets are actingcommand.resource-targets.v2 targets ([] withdraws the policy when the Runtime takes that), and the server writes the v2 document around them with the instance value ac_resources_list reports (policy_instance). valid_days (1-365) sets valid_until; without it none is written. The Runtime checks the document; a refusal comes back with its position (details.rejection). Use ac_resources_list for what may be targeted and ac_targets_get for what is active.",
        read_only: false,
        destructive: false,
        idempotent: false,
        input_schema: targets_set_input,
        result_schema: targets_set_result,
        run: operator::targets_set,
    },
    ToolDef {
        name: "ac_lab_observe",
        title: "Lab observe",
        tier: Tier::Author,
        description: concat!(
            "Runs actinglab --json observe, one argument per flag, and answers its data verbatim. Offline (scene) it detects the page of a PNG scene with the package (package with package_ref, or zip with expected_sha256) and touches no Runtime. With capture the Runtime takes the instance's current frame (instance: its alias) and records the Lab request in its ledger; this server then records one client.action (surface mcp) carrying the answer's req_id.",
            lab_errors!(),
            lab_job!()
        ),
        read_only: false,
        destructive: false,
        idempotent: false,
        input_schema: lab::observe_input,
        result_schema: lab::lab_result,
        run: lab::observe,
    },
    ToolDef {
        name: "ac_lab_do",
        title: "Lab do",
        tier: Tier::Author,
        description: concat!(
            "Runs actinglab --json do, one argument per flag, and answers its data verbatim. With capture it acts on the instance through the Runtime (target: a Runtime element id, or tap or swipe; instance: its alias), which records the Lab request in its ledger; this server then records one client.action (surface mcp) carrying the answer's req_id. Offline (scene) actinglab only plans, with dry_run, and touches no Runtime. The destructive guard flags (destructive, allow_destructive) are not read on the capture path (with capture and without dry_run): that is actinglab's own behaviour, and this server adds no guard of its own.",
            lab_errors!(),
            lab_job!()
        ),
        read_only: false,
        destructive: false,
        idempotent: false,
        input_schema: lab::do_input,
        result_schema: lab::lab_result,
        run: lab::act,
    },
    ToolDef {
        name: "ac_record_start",
        title: "Start a recording",
        tier: Tier::Author,
        description: concat!(
            "Runs actinglab --json record start: starts a recording session and its Lab recording for task_id, kept in actinglab's state files (state_dir). It opens no Runtime connection and records no client.action. --force is never passed: an active recording is not overwritten from here; ending it is ac_record_stop, overwriting it is for the person on the CLI.",
            lab_errors!(),
            lab_job!()
        ),
        read_only: false,
        destructive: false,
        idempotent: false,
        input_schema: lab::record_start_input,
        result_schema: lab::lab_result,
        run: lab::record_start,
    },
    ToolDef {
        name: "ac_record_mark",
        title: "Mark a recording step",
        tier: Tier::Author,
        description: concat!(
            "Runs actinglab --json record mark --request-json <request> with one actingcommand.lab-record-mark.v1 request (marks, a click, samples, a transition or a step action; contracts/lab-recording.md). actinglab self-tests every mark on the step's frames; dry_run only checks. It changes the recording's files only: no Runtime connection, no client.action.",
            lab_errors!(),
            lab_job!()
        ),
        read_only: false,
        destructive: false,
        idempotent: false,
        input_schema: lab::record_mark_input,
        result_schema: lab::lab_result,
        run: lab::record_mark,
    },
    ToolDef {
        name: "ac_record_stop",
        title: "Stop a recording",
        tier: Tier::Author,
        description: concat!(
            "Runs actinglab --json record stop: stops the recording session and generates its linear-steps package (lab_dir for the package directory; dry_run validates and writes nothing), answered verbatim, with lab.binding_example, lab.binding_requires and the other binding parts ac_binding_draft takes. Only the stop that generates the package answers lab.binding_requires; a later one answers lab.status already_generated without it. An answer larger than the output budget still carries lab.binding_example, lab.binding_requires, lab.prerequisite_entry_example, lab.catalog_on_failure_example and lab.package_ref inline when they fit. It opens no Runtime connection and records no client.action.",
            lab_errors!(),
            lab_job!()
        ),
        read_only: false,
        destructive: false,
        idempotent: false,
        input_schema: lab::record_stop_input,
        result_schema: lab::lab_result,
        run: lab::record_stop,
    },
    ToolDef {
        name: "ac_record_status",
        title: "Recording status",
        tier: Tier::Author,
        description: concat!(
            "Runs actinglab --json record status: the recording session and its Lab recording steps, verbatim. It reads the recording; actinglab creates its recording state directory if it is missing.",
            lab_errors!(),
            lab_job!()
        ),
        read_only: true,
        destructive: false,
        idempotent: true,
        input_schema: lab::record_status_input,
        result_schema: lab::lab_result,
        run: lab::record_status,
    },
    ToolDef {
        name: "ac_binding_draft",
        title: "Binding draft",
        tier: Tier::Author,
        description: concat!(
            "Turns an ac_record_stop answer into a binding draft and changes nothing. Give exactly one of record_stop (the answer itself, also an exported one that kept its lab parts) and record_stop_export {path, sha256} (the export of an ac_record_stop answer, read only from %TEMP%\\actingcommand-mcp\\materials and only when its sha256 matches). binding_example, binding_requires, prerequisite_entry_example and catalog_on_failure_example come verbatim from it; binding_requires is null, with the warning binding_requires_unavailable, when the answer has none (a record stop answering already_generated). admission: actinglab package preflight on the recorded package (binding_example.scheduled_execution.package_path with lab.package_ref); its coverage is the admission scope, preflight_error when it does not pass. check_config {config, exit_code, status, error, report_export}: actingd check-config on the install's current actingd.config.json, which it only reads; the whole report is exported, status and error are copied from it; the draft is not in it, and nothing is merged into any configuration or copy. manual_steps: binding_requires, then edit the configuration, approve in the UI, restart the daemon; all of them are the person's.",
            lab_errors!(),
            check_job!()
        ),
        read_only: true,
        destructive: false,
        idempotent: true,
        input_schema: lab::binding_draft_input,
        result_schema: lab::binding_draft_result,
        run: lab::binding_draft,
    },
];

pub(super) fn find(name: &str) -> Option<&'static ToolDef> {
    TOOLS.iter().find(|tool| tool.name == name)
}

/// The tool definition in the shape of the session's protocol version.
pub(super) fn definition(tool: &ToolDef, era: Era) -> Value {
    let mut definition = Map::new();
    definition.insert("name".to_owned(), json!(tool.name));
    if era.structured() {
        definition.insert("title".to_owned(), json!(tool.title));
    }
    definition.insert("description".to_owned(), json!(tool.description));
    definition.insert("inputSchema".to_owned(), (tool.input_schema)());
    if era.structured() {
        definition.insert(
            "outputSchema".to_owned(),
            output_schema((tool.result_schema)()),
        );
    }
    definition.insert(
        "annotations".to_owned(),
        json!({
            "title": tool.title,
            "readOnlyHint": tool.read_only,
            "destructiveHint": tool.destructive,
            "idempotentHint": tool.idempotent,
            "openWorldHint": false,
        }),
    );
    Value::Object(definition)
}

/// `--list-tools --format json`: every tool in the build with its tier.
pub(super) fn list_json() -> Value {
    Value::Array(
        TOOLS
            .iter()
            .map(|tool| {
                let mut definition = definition(tool, Era::Modern);
                definition["tier"] = json!(tool.tier.as_str());
                definition
            })
            .collect(),
    )
}

/// `--list-tools --format markdown`: the skill's generated tool table.
pub(super) fn list_markdown() -> String {
    let mut table =
        String::from("| Tool | Tier | Hints | Arguments | Description |\n|---|---|---|---|---|\n");
    for tool in TOOLS {
        let schema = (tool.input_schema)();
        let required = schema
            .get("required")
            .and_then(Value::as_array)
            .map(|names| names.iter().filter_map(Value::as_str).collect::<Vec<_>>())
            .unwrap_or_default();
        let arguments = schema
            .get("properties")
            .and_then(Value::as_object)
            .map(|properties| {
                properties
                    .keys()
                    .map(|name| {
                        if required.contains(&name.as_str()) {
                            format!("`{name}`")
                        } else {
                            format!("`{name}?`")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let mut hints = Vec::new();
        if tool.read_only {
            hints.push("read-only");
        }
        if tool.destructive {
            hints.push("destructive");
        }
        if tool.idempotent {
            hints.push("idempotent");
        }
        let name = tool.name;
        let tier = tool.tier.as_str();
        let hints = hints.join(", ");
        let description = tool.description.replace('|', "\\|");
        table.push_str(&format!(
            "| `{name}` | {tier} | {hints} | {arguments} | {description} |\n"
        ));
    }
    table
}

/// A call to a tool of a tier this process was not started with.
pub(super) fn tier_not_enabled(tool: &ToolDef) -> ToolError {
    ToolError::new(
        "safety",
        ActingctlCode::TierNotEnabled,
        format!(
            "{} belongs to tier {}, which this server was not started with",
            tool.name,
            tool.tier.as_str()
        ),
    )
    .with_detail("required_tier", json!(tool.tier.as_str()))
    .blocked_by("the person restarts mcp-serve with --tier including this tier")
}

/// The `tools/call` result: the envelope as compact JSON text, and from 2025-06-18 on also
/// as `structuredContent`. A result over the budget becomes `output_budget_exceeded`.
pub(super) fn call_result(outcome: ToolOutcome, era: Era) -> Value {
    let mut structured = envelope(outcome);
    if !fits(&structured) {
        let payload_bytes = structured.to_string().len();
        structured = envelope(Err(ToolError::usage(
            ActingctlCode::OutputBudgetExceeded,
            "the result is larger than the 24 KiB tool output budget; narrow the request or lower limit",
        )
        .with_detail("payload_bytes", json!(payload_bytes))));
    }
    let is_error = structured.get("ok") == Some(&Value::Bool(false));
    let mut result = Map::new();
    result.insert(
        "content".to_owned(),
        json!([{"type": "text", "text": structured.to_string()}]),
    );
    if era.structured() {
        result.insert("structuredContent".to_owned(), structured);
    }
    result.insert("isError".to_owned(), json!(is_error));
    Value::Object(result)
}

/// Whether a successful `result` stays inside the output budget.
pub(super) fn fits_success(result: &Value) -> bool {
    fits(&json!({"ok": true, "result": result}))
}

fn fits(structured: &Value) -> bool {
    let payload = structured.to_string();
    if payload.len() > PAYLOAD_LIMIT {
        return false;
    }
    let whole = json!({
        "content": [{"type": "text", "text": payload}],
        "structuredContent": structured,
        "isError": false,
    });
    whole.to_string().len() + MODERN_FIELDS_RESERVE <= RESULT_LIMIT
}

fn envelope(outcome: ToolOutcome) -> Value {
    match outcome {
        Ok(success) => {
            let mut envelope = Map::new();
            envelope.insert("ok".to_owned(), json!(true));
            envelope.insert("result".to_owned(), success.result);
            if !success.warnings.is_empty() {
                envelope.insert("warnings".to_owned(), Value::Array(success.warnings));
            }
            Value::Object(envelope)
        }
        Err(error) => json!({"ok": false, "error": error.into_value()}),
    }
}

/// Both envelope forms, with the tool's own result schema.
fn output_schema(result: Value) -> Value {
    json!({
        "type": "object",
        "properties": {
            "ok": {"type": "boolean"},
            "result": result,
            "warnings": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "code": {"type": "string"},
                        "message": {"type": "string"},
                        "blocked_by": {"anyOf": [{"type": "string"}, {"type": "null"}]},
                        "details": {"type": "object"},
                    },
                    "required": ["code", "message"],
                },
            },
            "error": {
                "type": "object",
                "properties": {
                    "class": {
                        "type": "string",
                        "enum": ["usage", "safety", "device", "runtime", "uncertain"],
                    },
                    "code": {"type": "string"},
                    "message": {"type": "string"},
                    "blocked_by": {"anyOf": [{"type": "string"}, {"type": "null"}]},
                    "details": {"type": "object"},
                },
                "required": ["class", "code", "message", "blocked_by", "details"],
            },
        },
        "required": ["ok"],
        "oneOf": [
            {"type": "object", "properties": {"ok": {"const": true}}, "required": ["result"]},
            {"type": "object", "properties": {"ok": {"const": false}}, "required": ["error"]},
        ],
    })
}

fn instance_argument() -> Value {
    json!({
        "type": "string",
        "minLength": 1,
        "description": "Instance alias, instance_id or ADB port.",
    })
}

fn overview_input() -> Value {
    json!({
        "type": "object",
        "properties": {"instance": instance_argument()},
        "additionalProperties": false,
    })
}

fn overview_result() -> Value {
    json!({
        "type": "object",
        "properties": {
            "daemon": {
                "type": "object",
                "properties": {
                    "online": {"type": "boolean"},
                    "owner_epoch": {"type": "string"},
                    "error": {"type": "object"},
                },
                "required": ["online"],
            },
            "global_pause": {"type": "object"},
            "instances": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "alias": {"type": "string"},
                        "instance_id": {"type": "string"},
                        "adb_port": {"type": "integer"},
                        "game_id": {"type": "string"},
                        "lease_active": {"type": "boolean"},
                        "queued": {"type": "integer"},
                        "pause": {"type": "object"},
                        "resource_fence": {"type": "object"},
                        "monitor": {"type": "object"},
                        "run": run_status_schema(),
                        "run_incomplete": {"type": "boolean"},
                    },
                    "required": ["alias", "instance_id", "lease_active", "queued"],
                },
            },
            "run_window": {
                "type": "object",
                "properties": {"since_unix_ms": {"type": "integer"}},
                "required": ["since_unix_ms"],
            },
            "lab_tool": {
                "type": "object",
                "properties": {"present": {"type": "boolean"}},
                "required": ["present"],
            },
            "install": {
                "type": "object",
                "properties": {
                    "root": {"type": "string"},
                    "state_root": {"type": "string"},
                    "build": {"type": "object"},
                },
            },
            "incomplete": {"type": "boolean"},
        },
        "required": ["daemon", "lab_tool", "install"],
    })
}

fn material_input() -> Value {
    json!({
        "type": "object",
        "properties": {
            "ref": {
                "type": "string",
                "minLength": 1,
                "description": "A materials[].ref from ac_events.",
            },
            "mode": {"type": "string", "enum": ["text", "export"]},
            "offset": {
                "type": "integer",
                "minimum": 0,
                "default": 0,
                "description": "mode text only: the first byte to return.",
            },
            "max_bytes": {
                "type": "integer",
                "minimum": 1,
                "maximum": 8192,
                "default": 4096,
                "description": "mode text only.",
            },
        },
        "required": ["ref", "mode"],
        "additionalProperties": false,
    })
}

fn material_result() -> Value {
    json!({
        "type": "object",
        "properties": {
            "text": {"type": "string"},
            "offset": {"type": "integer"},
            "bytes": {"type": "integer"},
            "next_offset": {"type": "integer"},
            "total_bytes": {"type": "integer"},
            "media_type": {"type": "string"},
            "path": {"type": "string"},
            "sha256": {"type": "string"},
            "size": {"type": "integer"},
        },
    })
}

/// `actingcommand.run-status.v1` as runtime-client projects it (Workflow #338 R1).
fn run_status_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "schema_version": {"type": "string"},
            "request_id": {"type": ["string", "null"]},
            "correlation_id": {"type": "string"},
            "run_id": {"type": "string"},
            "task_id": {"type": "string"},
            "instance_id": {"type": "string"},
            "dispatch": {"type": "string", "enum": ["manual", "scheduled", "unknown"]},
            "origin": {"type": "string", "enum": ["cli", "ui", "lab", "scheduler", "unknown"]},
            "package_ref": {},
            "recovery_packages": {"type": "array"},
            "state": {
                "type": "string",
                "enum": [
                    "not_found",
                    "admitted",
                    "running",
                    "succeeded",
                    "failed",
                    "cancelled",
                    "interrupted_unterminated",
                ],
            },
            "terminal": {"type": "object"},
            "lease": {"type": "object"},
            "progress": {"type": "object"},
            "evidence": {"type": "object"},
        },
        "required": [
            "schema_version",
            "request_id",
            "dispatch",
            "origin",
            "recovery_packages",
            "state",
            "evidence",
        ],
    })
}

fn get_run_input() -> Value {
    json!({
        "type": "object",
        "properties": {
            "handle": {
                "type": "string",
                "minLength": 1,
                "description": "The Runtime request_id of the run (an ac_run_pack handle), or a job handle of this process.",
            },
            "run_id": {"type": "string", "minLength": 1, "description": "The run's run_id."},
            "wait_s": {"type": "integer", "minimum": 0, "maximum": 25, "default": 0},
        },
        "additionalProperties": false,
    })
}

fn diagnose_input() -> Value {
    json!({
        "type": "object",
        "properties": {
            // Workflow #389 MCP-7: optional; without it every instance.
            "instance": {
                "type": "string",
                "minLength": 1,
                "description": "Instance alias, instance_id or ADB port; without it every instance.",
            },
            "task_id": {"type": "string", "minLength": 1, "description": "A Runtime task_id."},
            "since_unix_ms": {
                "type": "integer",
                "minimum": 0,
                "description": "Window start; default 24 h ago, at most 7 days ago.",
            },
        },
        "additionalProperties": false,
    })
}

fn diagnose_result() -> Value {
    let mut result = json!({
        "type": "object",
        "properties": {
            "instance": {"type": "object"},
            "window": {
                "type": "object",
                "properties": {"since_unix_ms": {"type": "integer"}},
                "required": ["since_unix_ms"],
            },
            "runs": {"type": "array", "items": run_status_schema()},
            "runs_truncated": {"type": "boolean"},
            "errors_page": {
                "type": "object",
                "properties": {
                    "events": {"type": "array", "items": {"type": "object"}},
                    "more": {"type": "boolean"},
                    "truncated": {"type": "boolean"},
                    "next_cursor": {"type": "string"},
                    "snapshot_ledger_position": {"type": "integer"},
                },
                "required": ["events", "more", "snapshot_ledger_position"],
            },
            "suspended": {"type": "array", "items": {"type": "object"}},
            "suspended_truncated": {"type": "boolean"},
            "lift": {"type": "array", "items": {"type": "object"}},
            "lift_truncated": {"type": "boolean"},
            "repeating": {"type": "array", "items": {"type": "object"}},
            "repeating_truncated": {"type": "boolean"},
            "report_warnings": {"type": "array"},
            "report_warnings_truncated": {"type": "boolean"},
            "clock_slots": {"type": "array", "items": {"type": "object"}},
            "clock_slots_truncated": {"type": "boolean"},
            "clock_slots_total": {"type": "integer"},
            "suspended_report": {"type": "object"},
            // Workflow #389 MCP-7 (command table §4.8).
            "runs_grouped": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "catalog_task_id": {"type": "string"},
                        "final_page": {"type": "string"},
                        "count": {"type": "integer"},
                        "first_seq": {"type": "integer"},
                        "last_seq": {"type": "integer"},
                        "latest_request_id": {"type": ["string", "null"]},
                        "instances": {"type": "array", "items": {"type": "string"}},
                    },
                    "required": ["count", "first_seq", "last_seq", "latest_request_id", "instances"],
                },
            },
            "runs_grouped_truncated": {"type": "boolean"},
            "failures_grouped": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "type": {"type": "string"},
                        "catalog_task_id": {"type": "string"},
                        "count": {"type": "integer"},
                        "first_seq": {"type": "integer"},
                        "last_seq": {"type": "integer"},
                    },
                    "required": ["type", "code", "count", "first_seq", "last_seq"],
                },
            },
            "failures_grouped_truncated": {"type": "boolean"},
            "incomplete": {"type": "boolean"},
        },
        "required": [
            "window",
            "runs",
            "errors_page",
            "suspended",
            "lift",
            "repeating",
            "report_warnings",
            "clock_slots",
            "suspended_report",
            "runs_grouped",
            "failures_grouped",
        ],
    });
    // Workflow #389 MCP-7: a group's codes, members of its row like final_page (as ac_runs adds a
    // row's failure_code).
    result["properties"]["runs_grouped"]["items"]["properties"]["failure_code"] =
        json!({"type": "string"});
    result["properties"]["failures_grouped"]["items"]["properties"]["code"] =
        json!({"type": "string"});
    result
}

/// A job of this process: its kind, phase, warnings and, once it ended, its outcome.
fn job_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "kind": {"type": "string"},
            "phase": {"type": "string"},
            "warnings": {"type": "array"},
            "outcome": {},
        },
        "required": ["kind", "phase", "warnings"],
    })
}

/// A run status (with this process's job when it has one), or a job that is not a run.
fn get_run_result() -> Value {
    let mut run = run_status_schema();
    run["properties"]["job"] = job_schema();
    json!({
        "anyOf": [
            run,
            {
                "type": "object",
                "properties": {"handle": {"type": "string"}, "job": job_schema()},
                "required": ["handle", "job"],
            },
        ],
    })
}

fn instance_only_input() -> Value {
    json!({
        "type": "object",
        "properties": {"instance": instance_argument()},
        "required": ["instance"],
        "additionalProperties": false,
    })
}

fn view_header_properties() -> Value {
    json!({
        "instance_alias": {"type": "string"},
        "policy_instance": {"type": "string"},
        "evaluated_at_unix_ms": {"type": "integer"},
        "as_of_ledger_position": {"type": "integer"},
        "catalog_hash": {"type": "string"},
    })
}

fn resources_list_result() -> Value {
    let mut properties = view_header_properties();
    properties["targetable"] = json!({"type": "array", "items": {"type": "object"}});
    properties["not_targetable"] = json!({"type": "array", "items": {"type": "object"}});
    json!({
        "type": "object",
        "properties": properties,
        "required": ["instance_alias", "policy_instance", "targetable", "not_targetable"],
    })
}

fn targets_get_result() -> Value {
    let mut properties = view_header_properties();
    properties["active"] = json!({"anyOf": [{"type": "object"}, {"type": "null"}]});
    json!({
        "type": "object",
        "properties": properties,
        "required": ["instance_alias", "policy_instance"],
    })
}

fn run_pack_input() -> Value {
    json!({
        "type": "object",
        "properties": {
            "instance": instance_argument(),
            "package": {"type": "string", "minLength": 1, "description": "The task package's path."},
            "package_ref": {
                "type": "string",
                "minLength": 1,
                "description": "The package's sha256 or content reference; default: actinglab package digest.",
            },
            "recovery_package": {"type": "string", "minLength": 1},
            "recovery_package_ref": {"type": "string", "minLength": 1},
            "deadline_s": {"type": "integer", "minimum": 60, "maximum": 1800, "default": 1800},
        },
        "required": ["instance", "package"],
        "additionalProperties": false,
    })
}

fn run_pack_result() -> Value {
    json!({
        "type": "object",
        "properties": {
            "handle": {"type": "string"},
            "correlation_id": {"type": "string"},
            "phase": {"type": "string"},
        },
        "required": ["handle", "correlation_id", "phase"],
    })
}

fn stop_run_input() -> Value {
    json!({
        "type": "object",
        "properties": {
            "handle": {"type": "string", "minLength": 1, "description": "The run's Runtime request_id."},
            "wait_s": {"type": "integer", "minimum": 0, "maximum": 25, "default": 20},
        },
        "required": ["handle"],
        "additionalProperties": false,
    })
}

fn stop_run_result() -> Value {
    json!({
        "type": "object",
        "properties": {
            "cancellation": {"type": "object"},
            "touch_release": {"type": "string", "enum": ["done", "failed", "not_needed"]},
            "touch_release_error": {"type": "object"},
            "job_phase": {"type": "string"},
            "handle": {"type": "string"},
        },
    })
}

fn pause_input() -> Value {
    json!({
        "type": "object",
        "properties": {
            "instance": instance_argument(),
            "reason": {"type": "string", "pattern": "^[a-z0-9_.-]{1,64}$", "default": "mcp.pause"},
            "drain_timeout_s": {"type": "integer", "minimum": 1, "maximum": 600, "default": 60},
        },
        "additionalProperties": false,
    })
}

fn resume_input() -> Value {
    json!({
        "type": "object",
        "properties": {
            "instance": instance_argument(),
            "expected_owner_epoch": {
                "type": "string",
                "minLength": 1,
                "description": "The owner_epoch ac_pause gave or ac_overview shows.",
            },
            "expected_revision": {
                "type": "integer",
                "minimum": 1,
                "description": "The pause revision ac_pause gave or ac_overview shows.",
            },
        },
        "required": ["expected_owner_epoch", "expected_revision"],
        "additionalProperties": false,
    })
}

fn emulator_input() -> Value {
    json!({
        "type": "object",
        "properties": {
            "instance": instance_argument(),
            "action": {"type": "string", "enum": ["start", "stop", "restart"]},
        },
        "required": ["instance", "action"],
        "additionalProperties": false,
    })
}

/// A job's outcome within the call budget, or its handle and phase.
fn job_result() -> Value {
    json!({
        "type": "object",
        "properties": {
            "paused": {"type": "object"},
            "owner_epoch": {"type": "string"},
            "resumed": {"type": "object"},
            "controlled": {"type": "object"},
            "handle": {"type": "string"},
            "job_phase": {"type": "string"},
        },
    })
}

fn targets_set_input() -> Value {
    json!({
        "type": "object",
        "properties": {
            "instance": instance_argument(),
            "targets": {
                "type": "array",
                "maxItems": 16,
                "items": {"type": "object"},
                "description": "actingcommand.resource-targets.v2 targets; [] withdraws.",
            },
            "valid_days": {"type": "integer", "minimum": 1, "maximum": 365},
        },
        "required": ["instance", "targets"],
        "additionalProperties": false,
    })
}

fn targets_set_result() -> Value {
    json!({
        "type": "object",
        "properties": {"applied": {"type": "object"}},
        "required": ["applied"],
    })
}
