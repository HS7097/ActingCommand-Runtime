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
use crate::codes::{ActingctlCode, ArgumentProblem};
use actingcommand_contract::outcome::{Code, Detail, Outcome};
use serde_json::{Map, Value, json};
use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

/// The `instructions` both clients read (§四 instructions).
pub(super) const INSTRUCTIONS: &str = "ActingCommand local control (tools-only). Start with ac_overview. Long operations return a handle at once; a run's handle is its Runtime request_id and survives restarts, other job handles last only as long as this process; poll ac_get_run with wait_s. If an ac_run_pack call was interrupted before it returned, call ac_overview before submitting again. After an uncertain result call ac_get_run before anything else; never resend a write with new arguments. Approvals, actingd configuration file edits and daemon restarts belong to the person; a setting the Runtime may change while running is set with ac_config_set, and every change is recorded. Tiers: observer (read), operator (device/scheduling), author (Lab recording); tools outside the enabled tiers answer tier_not_enabled. Manual: skill `actingcommand`.";

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
        " Failures are {class, outcome}: an actinglab error envelope keeps its class by actinglab's exit code (2 usage, 3 safety, 4 device, 5 runtime, 6 usage) with outcome lab_tool_failed (exit_code, req_id), caused by actinglab's code (not_implemented for exit 6) and, when actinglab's error has details, by foreign_other_error whose raw_text is those details as JSON; any other exit, or no JSON envelope, is runtime lab_process_failed caused by foreign_other_error with actinglab's stderr tail as raw_text. String arguments may not start with --. An answer larger than the output budget is written whole to %TEMP%\\actingcommand-mcp\\materials\\<sha256>.json and answered as {req_id, export {path, sha256, size}, overflowed: true} with the warning answer_exported."
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

/// The tier is the catalog vocabulary `mcp_tier` (its source is this enum), the value
/// `required_tier` of `tier_not_enabled`.
impl actingcommand_contract::outcome::VocabularyToken for Tier {
    const VOCABULARY: &'static str = "mcp_tier";

    fn token(self) -> &'static str {
        self.as_str()
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

actingcommand_contract::outcome_vocabulary! {
    /// What must change before an MCP call can work (vocabulary `mcp_blocker`, Workflow #378
    /// A5, model v2.2 §7.3): the value `mcp_blocker` of a tool error. The advice itself is the
    /// code's `handling` in the catalog.
    pub(super) enum McpBlocker: "mcp_blocker" {
        /// The server's tier must include the tool's tier (`--tier`).
        ServerTier => "server_tier",
        /// The installation root must be known (`--root`).
        InstallRoot => "install_root",
        /// The root Tools folder must hold the tool.
        ToolsDir => "tools_dir",
        /// The material mode must allow it (`mode export`).
        MaterialMode => "material_mode",
        /// actingd must run on the state root.
        RuntimeRunning => "runtime_running",
        /// A person must act (configuration, approval).
        PersonAction => "person_action",
        /// The Runtime state root must be known (`--state-root` or the configuration's).
        StateRoot => "state_root",
        /// Scheduling must be paused first (`ac_pause`).
        SchedulingPause => "scheduling_pause",
        /// actingd must be a release that serves the request.
        RuntimeUpgrade => "runtime_upgrade",
    }
}

/// The outcome detail of every tool error and warning this server writes (model v2.2 §7.1:
/// class (b), full by default; `mcp-serve --detail codes` or `ACTINGCOMMAND_DETAIL=codes`).
static DETAIL: OnceLock<Detail> = OnceLock::new();

/// Sets this server's outcome detail once, at start.
pub(super) fn set_detail(detail: Detail) {
    let _ = DETAIL.set(detail);
}

fn detail() -> Detail {
    DETAIL.get().copied().unwrap_or(Detail::Full)
}

/// An outcome as this server writes it: the `actingcommand.outcome.v1` envelope in the
/// server's detail.
pub(super) fn envelope(outcome: &Outcome) -> Value {
    crate::cli_line::outcome_value(outcome, detail())
}

/// A warning beside a result: `{outcome}` (Workflow #378 A5; no English member), the same
/// form the CLI line carries.
pub(super) fn warning(outcome: &Outcome) -> Value {
    crate::cli_line::warning_value(outcome, detail())
}

/// A tool error, `{class, outcome}` (Workflow #378 A5, model v2.2 §7.3): the class (#338 R6:
/// usage, safety, device, runtime, uncertain) and one outcome, whose values and links hold
/// everything the error says; there is no `code`, `message`, `blocked_by`, `details` or
/// `explain` member. Boxed so results stay small.
#[derive(Clone)]
pub(super) struct ToolError(Box<ToolErrorBody>);

#[derive(Clone)]
struct ToolErrorBody {
    class: &'static str,
    outcome: Outcome,
}

impl ToolError {
    /// An error of this class whose outcome is `code` alone; add values with [`Self::with`].
    pub(super) fn new(class: &'static str, code: impl Into<Code>) -> Self {
        Self::outcome(class, Outcome::new(code))
    }

    /// An error of this class with this outcome.
    pub(super) fn outcome(class: &'static str, outcome: Outcome) -> Self {
        Self(Box::new(ToolErrorBody { class, outcome }))
    }

    /// A `usage` error whose outcome is `code` alone.
    pub(super) fn usage(code: impl Into<Code>) -> Self {
        Self::new("usage", code)
    }

    /// Changes the outcome: adds values with its typed setters, or links.
    pub(super) fn with(self, change: impl FnOnce(Outcome) -> Outcome) -> Self {
        let ToolErrorBody { class, outcome } = *self.0;
        Self::outcome(class, change(outcome))
    }

    /// Attaches the outcome that caused this error.
    pub(super) fn caused_by(self, link: Outcome) -> Self {
        self.with(|outcome| outcome.caused_by(link))
    }

    /// What must change before the call can work: the value `mcp_blocker`.
    pub(super) fn blocked_by(self, blocker: McpBlocker) -> Self {
        self.with(|outcome| outcome.mcp_blocker(blocker))
    }

    /// The error's class; only the gates read it apart from `{class, outcome}`.
    #[cfg(test)]
    pub(super) fn class(&self) -> &'static str {
        self.0.class
    }

    /// The error's outcome; only the gates read it apart from `{class, outcome}`.
    #[cfg(test)]
    pub(super) fn outcome_ref(&self) -> &Outcome {
        &self.0.outcome
    }

    pub(super) fn into_outcome(self) -> Outcome {
        self.0.outcome
    }

    /// Whether the error's top code is `code`.
    pub(super) fn is(&self, code: impl Into<Code>) -> bool {
        self.0.outcome.is(code)
    }

    /// `{class, outcome}`, leaving the error in place.
    pub(super) fn to_value(&self) -> Value {
        let mut error = Map::new();
        error.insert("class".to_owned(), Value::String(self.0.class.to_owned()));
        error.insert("outcome".to_owned(), envelope(&self.0.outcome));
        Value::Object(error)
    }

    /// `{class, outcome}`.
    pub(super) fn into_value(self) -> Value {
        self.to_value()
    }

    /// The same failure reported beside a successful result: `{outcome}`.
    pub(super) fn into_warning(self) -> Value {
        warning(&self.0.outcome)
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
            Some(unknown) => Err(invalid_argument(unknown, ArgumentProblem::Unknown)),
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
            Some(_) => Err(invalid_argument(key, ArgumentProblem::WrongType)),
        }
    }

    pub(super) fn required_string(&self, key: &str) -> Result<&'a str, ToolError> {
        self.string(key)?
            .ok_or_else(|| invalid_argument(key, ArgumentProblem::Required))
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
                .ok_or_else(|| out_of_range(key, minimum, maximum)),
        }
    }
}

/// `arguments_invalid` for one argument: its name (`argument`) and what is wrong with it
/// (`argument_problem`).
pub(super) fn invalid_argument(argument: &str, problem: ArgumentProblem) -> ToolError {
    ToolError::outcome(
        "usage",
        Outcome::new(ActingctlCode::ArgumentsInvalid)
            .argument(argument)
            .argument_problem(problem),
    )
}

/// `arguments_invalid` for an integer argument outside `minimum..=maximum`.
/// A bound beyond the key table's i64 integers (an argument limited only by its type) is given
/// as `i64::MAX`.
pub(super) fn out_of_range(argument: &str, minimum: u64, maximum: u64) -> ToolError {
    let bound = |value: u64| i64::try_from(value).unwrap_or(i64::MAX);
    invalid_argument(argument, ArgumentProblem::OutOfRange)
        .with(|outcome| outcome.range_min(bound(minimum)).range_max(bound(maximum)))
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
        description: "Snapshot of the local ActingCommand Runtime, the same read and answer as actingctl overview; call it first. Returns daemon {online, owner_epoch}, global_pause, and one row per configured instance: alias, instance_id, adb_port, game_id, lease_active, queued requests, its scheduling pause {revision, stage, pause_reason}, its resource_fence {cause, since_unix_ms, awaiting_person} when an unconfirmed close of its own resources fenced it, emulator_state: whether its emulator runs as the Runtime last read it (at instance discovery or an emulator start, stop or restart; never polled), {state: on|off, observed_at_unix_ms} or {state: unknown} when the Runtime holds no reading (the running state only, not the emulator's health), backend_selfcheck {<entry>: {status, checked_at_unix_ms}}: the last self-check of each backend entry (input, capture, nemu) as the Runtime recorded it at its last open (start, emulator control, resume, self-check or task; status passed, failed or unknown; never probed for this read), {} while no entry was checked, its monitor, foreground {package, observed_at_unix_ms}: the package the instance last reported in the foreground before a pointer input (not a live read; absent until one was seen), run: its newest run admitted in run_window (the last 24 h; the open run when one is open) as a brief actingcommand.run-status.v1, with run_incomplete when that read did not finish, and not_running: the instance's not-running row as ac_schedule answers it {task_id, why, next_wake_unix_ms, since_unix_ms} (its first due task that did not run, or no task_id and why due_task_missing with the next wake when nothing is due), absent when the latest policy evaluation since the Runtime started had no decision for the instance; plus lab_tool {present}, install {root, state_root, build} and performance: the shadow measurement's status as actingctl perf-status prints it (mode, phase, monitor_health, the host row and one row per instance with responsiveness, curve stage and factor, the learning and normal views' stage, share, allowed, 24 h and total dwell and stops, and the effective stage and factor, which mode observe keeps at s0 and 1000; warnings perf_probe_unavailable and perf_baseline_clamped), its rows narrowed by instance too. When actingd does not answer, daemon is {online: false, error {class, outcome}} (outcome runtime_unavailable, or install_state_root_unresolved) and there is no instances field at all: that is no answer, not an empty system. instance (alias, instance_id or ADB port) narrows the rows to one. catalog: the active policy catalog generation {catalog_id, catalog_version, catalog_hash, activated_event_id, approval_id, approval_disposition, previous_catalog_id, previous_catalog_version}, null while none is active. config_generation {running, selected}: the configuration generation actingd runs and the one the install selects now; when they differ the warning config_generation_mismatch says a restart is needed, and each package binding of the selected configuration whose path is missing or unreadable adds the warning package_binding_unreadable. watchdog: actingctl watchdog status read locally, {decision, attention, starts_24h, last_tick}, null without an installation. fact_refresh: what the scheduler's fact refresh waits for, as actingctl fact-refresh prints it: {cycle_seq, transition_seq, instances}, one row per instance with a waiting key {instance_alias, demands: [{fact_key, outcome (the latest cycle's fact_refresh_pending, _unresolved, _exhausted or _unbound with its values), package_id (the recognition maintenance package that reads it), since_unix_ms, overdue}]}, its rows narrowed by instance too; no row means nothing waits. sections (global_pause, instances, run_window, lab_tool, install, performance, catalog, config_generation, watchdog, fact_refresh; default all) answers only the blocks named; daemon and incomplete are always answered. incomplete true means part of the snapshot could not be read (see warnings). An answer larger than the output budget is cut, one block at a time until it fits: fact_refresh, performance, watchdog, catalog, config_generation, then each instance row's run (the last row first), then each row's monitor, then each row's foreground, then install.build; each cut block leaves <block>_truncated: true where it was. The daemon, the global pause, each row's identity, emulator_state, backend_selfcheck, not_running, pause, lease_active and queued, and incomplete are never cut; an answer that still does not fit is output_budget_exceeded: narrow it with instance or sections. It only reads: it starts, stops and pauses nothing, and the emulator state it shows is the last reading, not a live one.",
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
        description: "One page of Runtime ledger events, oldest first: the same read, parameters and page as actingctl events (QueryEvents, which records nothing in the ledger). Filters, all combined: instance (alias, instance_id or ADB port), view (events = every event, observation, changes, errors = warning and above, health, lab), since_unix_ms and until_unix_ms, after_sequence and through_sequence, event_type or event_types (any of at most 32 types), exclude_event_types (at most 32, never a selected type), fact_key (only the fact records that name this instance or runtime fact key), min_severity and max_severity (debug, info, warning, error, fatal), origin_module, task_id, run_id, request_id and correlation_id. Without since_unix_ms or after_sequence the page starts at the beginning of the ledger. Each row carries seq, ts, type, severity, instance, request_id, run_id, task_id, correlation_id, the ledger's diagnostic code and materials [{ref, media_type, size}] for ac_material (materials_truncated when that list was shortened to fit the output budget); detail summary adds the outcome the record carries (a state change carries none), detail payload adds the event's projected payload verbatim, detail brief (the default) has neither. limit defaults to 20 (at most 100). last_sequence is the seq of the page's last row: pass it as after_sequence to read on from there, also after a restart. next_cursor continues the same query within its snapshot: send it back with the same filters and without after_sequence. truncated true means a brief page was cut to the output budget and next_cursor resumes after its last row. A payload page larger than the output budget is written whole to %TEMP%\\actingcommand-mcp\\materials\\<sha256>.json and answered as {export {path, sha256, size}, events_count, next_cursor, last_sequence} with the warning answer_exported. A cursor dies when the Runtime connection or this server restarts (cursor_invalid): read again from the first page or from after_sequence. To follow the ledger, poll this tool with after_sequence and wait_s (0-20, default 0), not ac_overview: an empty page is read again every second until rows arrive or wait_s ends, and waited_ms says how long it waited.",
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
        description: "One contained run as actingcommand.run-status.v1, read in full from its ledger events by handle (the Runtime request_id an ac_run_pack returns) or by run_id; give exactly one. The state is not_found, admitted, running, succeeded, failed, cancelled or interrupted_unterminated (admitted before a later Runtime start and never ended: uncertain). wait_s (0-25, default 0) waits for a change: the run is read again every second while its state is not_found, admitted or running, within the 25 s call budget; then the latest status is returned. Call it again to keep waiting. request_id is null only when a run_id lookup found no run. A submit refused before admission stays not_found with job.phase failed and the Runtime's error (for example lease_queue_expired, class safety, after a wait for a busy instance) in job.job_result.error. Another job handle of this process (ac_pause, ac_resume, ac_emulator, ac_stop_run or a Lab tool) answers {handle, job {kind, phase, warnings, job_result}}; a job handle this process does not hold answers handle_unknown.",
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
        // Workflow #411 B15 R7: the instance's stopped remedy rounds (`remedies`).
        description: "What went wrong within a bounded window, on one instance or, without instance, on every instance: the same read and result as actingctl diagnose. since_unix_ms is the window start (default the last 24 h, at most 7 days back, else since_out_of_range); the window used is echoed. Returns instance {alias, instance_id} (only with instance); runs: the recent failed or open runs (failed, admitted, running, interrupted_unterminated) as brief actingcommand.run-status.v1, newest first, from the 10 most recent runs; errors_page: the first page (oldest first) of the errors view in the window, rows as in ac_events, with next_cursor to continue in ac_events (view errors, same instance and since_unix_ms) when no task_id filter is set; suspended, lift and repeating: the rows of the read-only actingd suspended report (the instance's, else all), verbatim, and report_warnings: that report's warnings; packs: that report's suspended task packages whose instances name the instance (all without instance), verbatim, each {package_id, failure_code, runtime_commit, paused_hash_prefix, current_hash_prefix, instances}; remedies: that report's remedies rows whose instance_id is the instance (all without instance), verbatim: each remedy round stopped for a person, still in effect or lifted within 48 h of the ledger's latest event, newest first, {code (policy_remedy_unchosen, policy_remedy_unavailable or policy_remedy_exhausted), remedy_problem, task_id, instance_id, outcome_key, need, recorded_sequence, recorded_at_unix_ms, lifted {sequence, at_unix_ms} once a lift was recorded}, or {parse_state, signal_id, code} for a record that does not read back (only without instance); clock_slots: at most 10 clock_slots rows of that report recorded since since_unix_ms (clock slots refused, deferred, served late, skipped after their grace or lost to a later slot: refused and superseded rows first, then deferred and expired rows, then owed rows, newest first in each), verbatim, with clock_slots_total when fewer are kept (the full list is in actingd suspended); suspended_report: that report's status (unavailable with suspended_report_state_root_mismatch when it read another state root than this server). not_run: the deferred rows of the latest policy evaluation (due, or waiting for a gate; not those blocked by their own trigger), each a candidate row as ac_schedule answers it with its instance_alias, at most scheduling.not_run.diagnose_rows (read from config.parameters.scheduling) per instance in ac_schedule's order, with not_run_total when one was left out; when it could not be read there is no not_run, a warning and incomplete. actingd suspended reports the configuration of the generation install\\active.json selects at this call, found again at every call, not the packs the running Runtime has loaded. runs_grouped: every failed or open run of the window grouped by (catalog_task_id: the policy task id of a scheduled run, absent for a manual run; failure_code; final_page), each {count, first_seq, last_seq, latest_request_id, instances}; failures_grouped: the window's runtime.failed and command.rejected records and the policy.execution_recorded records of runs that paused their task, grouped by (type, catalog_task_id, code), each {count, first_seq, last_seq}; both most frequent first, read with QueryEvents, which records nothing; a relayed code the catalog does not register keeps its relayed spelling. task_id narrows runs, errors and both groups to one Runtime task. incomplete true means a read did not finish. To fit the output budget, lists are cut in this order: failures_grouped, runs_grouped (the fewest-count groups first), clock_slots (owed rows first, then deferred and expired, oldest first), not_run (from its end), errors_page, runs (oldest first), repeating, lift, suspended, packs, remedies, report_warnings, each marked <list>_truncated; runs cut for budget can be read with ac_get_run / ac_events.",
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
    // ---- Workflow #389 MCP-6 (begin): `ac_run_evidence` ↔ `actingctl run evidence`; observer
    // before U-3, a general read after it (command table §4.7). ----
    ToolDef {
        name: "ac_run_evidence",
        title: "Run evidence",
        tier: Tier::Observer,
        description: super::run_evidence::DESCRIPTION,
        read_only: true,
        destructive: false,
        idempotent: true,
        input_schema: super::run_evidence::input_schema,
        result_schema: super::run_evidence::result_schema,
        run: super::run_evidence::run,
    },
    // ---- Workflow #389 MCP-6 (end) ----
    ToolDef {
        name: "ac_resources_list",
        title: "Targetable resources",
        tier: Tier::Observer,
        description: "What one instance can target, as the Runtime reports it, unchanged: targetable resources (resource, fact_key, producing tasks, defaults, what must be given, the current observation) and the not-targetable ones with their reason. policy_instance is the value the instance field of a policy document takes. An actingd older than v0.11.0 answers runtime_operation_unsupported with mcp_blocker runtime_upgrade: read the resources of the scheduling catalog instead.",
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
        description: "The resource target policy one instance holds now, as the Runtime reports it, unchanged: active {policy_sha256, schema_version, version, event_id, valid_until_unix_ms, expired, targets, conditions}, or no active field when it holds none. An actingd older than v0.11.0 answers runtime_operation_unsupported with mcp_blocker runtime_upgrade: read the resources of the scheduling catalog instead.",
        read_only: true,
        destructive: false,
        idempotent: true,
        input_schema: instance_only_input,
        result_schema: targets_get_result,
        run: operator::targets_get,
    },
    // ---- Workflow #387 D1b (begin): `ac_config_set` ↔ `actingctl config set|clear`; observer
    // until U-3 (model v1.2 §5.6): changing a setting is not tiered, every change is recorded. ----
    ToolDef {
        name: "ac_config_set",
        title: "Change settings",
        tier: Tier::Observer,
        description: "Changes settings the Runtime may change while it runs, as actingctl config set and config clear do. changes is a list of {key, value} (1-64): key as actingctl status --config reports it in the config.parameters.<section> rows (only rows with settable true), value its new JSON value; a change without value returns the key to its configuration-file value. All changes take effect at once, or none does: an unknown key answers config_key_unknown, a key the Runtime reads only at start config_key_not_settable (change it in the configuration file, commit it with acsetup and restart the daemon instead), a value of the wrong type or range config_value_invalid. Every change is recorded in the ledger and kept across restarts until the configuration file changes the same key. Answers {settings: [{key, value, source}]}, each changed key as it is in effect afterwards (source instruction, or explicit/default after a clear).",
        read_only: false,
        destructive: false,
        idempotent: true,
        input_schema: config_set_input,
        result_schema: config_set_result,
        run: super::settings::config_set,
    },
    // ---- Workflow #387 D1b (end) ----
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
    // ---- Workflow #389 MCP-4 (begin): `ac_facts` ↔ `actingctl facts --program | --instance`
    // and `status --config`; observer before U-3, a general read after it (command table
    // §4.4). ----
    ToolDef {
        name: "ac_facts",
        title: "Runtime and instance facts",
        tier: Tier::Observer,
        description: super::facts::DESCRIPTION,
        read_only: true,
        destructive: false,
        idempotent: true,
        input_schema: super::facts::input,
        result_schema: super::facts::result,
        run: super::facts::run,
    },
    // ---- Workflow #389 MCP-4 (end) ----
    // ---- Workflow #389 MCP-14 (begin): `ac_outcome_code` ↔ `actingctl codes`; observer before
    // U-3, a general read (offline) after it (command table §4.15). ----
    ToolDef {
        name: "ac_outcome_code",
        title: "Outcome code catalog",
        tier: Tier::Observer,
        description: super::code_lookup::DESCRIPTION,
        read_only: true,
        destructive: false,
        idempotent: true,
        input_schema: super::code_lookup::input_schema,
        result_schema: super::code_lookup::result_schema,
        run: super::code_lookup::run,
    },
    // ---- Workflow #389 MCP-14 (end) ----
    // ---- Workflow #398 RD-1c (begin): `ac_devices`, `ac_device_bind`, `ac_device_unbind` ↔
    // `actingctl devices list|bind|unbind`; observer before U-3, as D1b's configuration tools
    // (model-398 §3.3). ----
    ToolDef {
        name: "ac_devices",
        title: "Devices adb lists",
        tier: Tier::Observer,
        description: super::devices::LIST_DESCRIPTION,
        read_only: true,
        destructive: false,
        idempotent: true,
        input_schema: super::devices::list_input,
        result_schema: super::devices::list_result,
        run: super::devices::list,
    },
    ToolDef {
        name: "ac_device_bind",
        title: "Bind a device to a handset",
        tier: Tier::Observer,
        description: super::devices::BIND_DESCRIPTION,
        read_only: false,
        destructive: false,
        idempotent: true,
        input_schema: super::devices::bind_input,
        result_schema: super::devices::bind_result,
        run: super::devices::bind,
    },
    ToolDef {
        name: "ac_device_unbind",
        title: "Unbind a handset's device",
        tier: Tier::Observer,
        description: super::devices::UNBIND_DESCRIPTION,
        read_only: false,
        destructive: true,
        idempotent: true,
        input_schema: super::devices::unbind_input,
        result_schema: super::devices::unbind_result,
        run: super::devices::unbind,
    },
    // ---- Workflow #398 RD-1c (end) ----
    // ---- Workflow #410 item 6 (begin): `ac_capability_gap` ↔ `actingctl capability-gap`;
    // observer, so that every session can record what it lacked; it writes only the user's own
    // local log and touches neither the Runtime nor a device. ----
    ToolDef {
        name: "ac_capability_gap",
        title: "Record a capability gap",
        tier: Tier::Observer,
        description: super::capability_gap::DESCRIPTION,
        read_only: false,
        destructive: false,
        idempotent: false,
        input_schema: super::capability_gap::input,
        result_schema: super::capability_gap::result,
        run: super::capability_gap::run,
    },
    // ---- Workflow #410 item 6 (end) ----
    // ---- Workflow #411 B15 R6 (begin): `ac_remedy_set` ↔ `actingctl remedy set|lift`,
    // `ac_remedy_get` ↔ `actingctl remedy get`; general tools (observer until U-3): an agent
    // writes the instance's target record, and the Runtime checks every write. ----
    ToolDef {
        name: "ac_remedy_set",
        title: "Write remedy choices",
        tier: Tier::Observer,
        description: super::remedy::SET_DESCRIPTION,
        read_only: false,
        destructive: false,
        idempotent: false,
        input_schema: super::remedy::set_input,
        result_schema: super::remedy::set_result,
        run: super::remedy::set,
    },
    ToolDef {
        name: "ac_remedy_get",
        title: "Remedy choices in effect",
        tier: Tier::Observer,
        description: super::remedy::GET_DESCRIPTION,
        read_only: true,
        destructive: false,
        idempotent: true,
        input_schema: instance_only_input,
        result_schema: super::remedy::get_result,
        run: super::remedy::get,
    },
    // ---- Workflow #411 B15 R6 (end) ----
    // ---- Workflow #386 SR-6b, MCP-19b (begin): `ac_schedule` ↔ `actingctl schedule`; a general
    // read (observer until U-3) from the policy host's memory that records nothing. ----
    ToolDef {
        name: "ac_schedule",
        title: "Why tasks did not run",
        tier: Tier::Observer,
        description: super::schedule::DESCRIPTION,
        read_only: true,
        destructive: false,
        idempotent: true,
        input_schema: super::schedule::input,
        result_schema: super::schedule::result,
        run: super::schedule::schedule,
    },
    // ---- Workflow #386 SR-6b, MCP-19b (end) ----
    ToolDef {
        name: "ac_pack_check",
        title: "Check a task package",
        tier: Tier::Observer,
        description: concat!(
            "Checks one task package offline, as actinglab --json package check --package <package> does, and answers its data verbatim: {status: admitted, package_id, entry_task_id}. The check is the Runtime's own structural admission, the one check-config and the daemon apply, so it admits exactly what the Runtime admits; it recognizes and executes nothing, and a declared OCR model need not be installed. The package is named by its path only (a directory, .zip or .json container); no digest is computed. A refused package is actinglab's package_invalid, whose details code is the admission's own code.",
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
        description: "Runs one task package on one instance, as actingctl task-run does, and answers at once with {handle, correlation_id, phase: submitting}; handle is the run's Runtime request_id: follow it with ac_get_run (wait_s) until the state is terminal. package is a package path, and a recovery package is recovery_package, its path; no digest is given or computed. deadline_s (60-1800, default 1800) bounds the run. It never pauses scheduling: on a busy instance the run waits for its turn in the instance's queue, at most min(60 s, its deadline minus 30 s); not granted in time it comes back in ac_get_run's job.job_result.error as the Runtime's lease_queue_expired, class safety; for exclusive use call ac_pause before and ac_resume after. recovery_package is used only when the package's entry is checked (an entry page set, stages, home with any_of) or when a failed direct run may start the stuck-recovery ladder (recovery_ladder.rules.direct_runs = trigger); otherwise the run is refused with contained_task_recovery_binding_unused, which ac_get_run shows as the job's error. If this call is interrupted before it answers, call ac_overview before submitting again.",
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
        description: "Stops one manual run (MCP, CLI, UI or Lab) by its handle, the Runtime request_id, from any process: the run's package is abandoned and the screen stays where it is; there is no return home, no recovery package and no rerun; only touch points that may still be held are lifted. Answers {cancellation, touch_release, job_phase}; touch_release is done, failed or not_needed: not_needed includes a run that had already ended before this call, and failed (for example lease_queue_expired) can mean the still-connected submitter already lifted them itself. When this server submitted the run and its job still waits, only the stop is sent and that job lifts the touches. Otherwise a background job waits for the run to end, up to the longest run deadline; not done within wait_s (default 20) the answer is {handle, job_phase} for ac_get_run. This handle is a job of this MCP process and ends with it (ac_get_run then answers handle_unknown); afterwards read the pause's revision and owner_epoch, or the emulator's state, from ac_overview, and a stopped run with ac_get_run by its request_id. A scheduled run cannot be stopped by a client: the Runtime's refusal comes back with mcp_blocker scheduling_pause (an instance pause with ac_pause drains it).",
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
    // ---- Workflow #389 MCP-13a (begin): `ac_app` ↔ actinglab `session app`; operator before
    // U-3, option two after it (MCP-0 command table v1.1 §4.14, FCM:116). ----
    ToolDef {
        name: "ac_app",
        title: "Application control",
        tier: Tier::Operator,
        description: concat!(
            "Starts, restarts or stops the application the Runtime configures for one instance, as actinglab --json session app <action> --instance <instance> does (the Runtime's application lifecycle request), and answers actinglab's data verbatim. action is launch, restart, stop or force-stop; force-stop is the same action as stop. stop, force-stop and restart end what runs in the application, so the tool is destructive. instance is the Runtime instance alias. It records no Lab recording step (no --record).",
            lab_errors!(),
            " Not done within the call budget, the answer is {handle, job_phase} for ac_get_run; actinglab keeps running and is never stopped from here. This handle is a job of this MCP process and ends with it (ac_get_run then answers handle_unknown); afterwards read the instance with ac_overview."
        ),
        read_only: false,
        destructive: true,
        idempotent: false,
        input_schema: lab::app_input,
        result_schema: lab::lab_result,
        run: lab::app,
    },
    // ---- Workflow #389 MCP-13a (end) ----
    // ---- Workflow #390 DRILL-1 (begin): `ac_ladder_drill` ↔ `actingctl ladder-drill`; operator
    // before U-3, option two after it (forced compromise T-08). ----
    ToolDef {
        name: "ac_ladder_drill",
        title: "Recovery ladder drill",
        tier: Tier::Operator,
        description: "Asks the Runtime for a drill of one instance's stuck-recovery ladder, as actingctl ladder-drill does; the Runtime takes it only from an operator origin. The drill is admitted, climbs and is recorded like a ladder a failure started, and every record of its own (recovery_ladder_started with trigger stage drill, recovery_rung_finished, recovery_ladder_finished, recovery_ladder_suppressed) is tagged drill. device false (the default): a plan drill that never acts on the device; each rung the ladder would run is recorded skipped with drill_plan, the climb ends planned, and the cool-down window is left as it was. device true: the drill climbs its rungs on the instance under the same key, admission and fences as a real ladder (return home, an application restart, an emulator restart), so it is destructive. It starts at the configured trigger_defaults.start_rung and the cool-down holds it as trigger_defaults.respect_cooldown says. A direct run still starts no ladder. The answer is {instance_alias, mode, ladder_request_id} once the drill is admitted, before it climbs; follow it with ac_events request_id <ladder_request_id>. Refusals: recovery_ladder_drill_disabled (stuck recovery is off for the instance), recovery_ladder_drill_already_running, recovery_ladder_drill_cooling_down (next_eligible_unix_ms), recovery_admission_denied (a pause, a shutdown or business capacity), instance_unknown, invalid_emulator_control_origin.",
        read_only: false,
        destructive: true,
        idempotent: false,
        input_schema: ladder_drill_input,
        result_schema: ladder_drill_result,
        run: operator::ladder_drill,
    },
    // ---- Workflow #390 DRILL-1 (end) ----
    ToolDef {
        name: "ac_targets_set",
        title: "Set resource targets",
        tier: Tier::Operator,
        description: "Sets one instance's resource targets: targets are actingcommand.resource-targets.v2 targets ([] withdraws the policy when the Runtime takes that), and the server writes the v2 document around them with the instance value ac_resources_list reports (policy_instance). valid_days (1-365) sets valid_until; without it none is written. The Runtime checks the document; a refusal is the Runtime's outcome, with the refused position as json_pointer, line and column and the reason as resource_targets_rejection_reason. Use ac_resources_list for what may be targeted and ac_targets_get for what is active.",
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
            "Runs actinglab --json observe, one argument per flag, and answers its data verbatim. Offline (scene) it detects the page of a PNG scene with the package (package, or zip, by its path) and touches no Runtime; sample_scenes adds 1 to 4 earlier frames of the same screen, oldest first, scene being the current frame, for a package whose target consensus or candidate layout agrees over several frames (actinglab refuses too few or too many frames, frames of different sizes, and several frames for a package without such agreement, as lab_tool_failed caused by validation_failed naming --scene). With capture the Runtime takes the frames its package needs from the instance (instance: its alias; package or zip is required): those its target consensus needs (up to five; the last is the current frame) and, when it recognises a page whose candidate layout agrees over several frames, that layout's further frames (up to 25 frames in all), and records the Lab request in its ledger; this server then records one client.action (surface mcp) carrying the answer's req_id. Every answer states the frame's coordinate system as frame_coordinates {device_pixels [width, height], orientation, applies_to}: ac_lab_do's tap, tap_rect and swipe take x,y in exactly these frame pixels, counted from the top-left pixel. preview true also writes a JPEG preview (long edge at most 1024) and answers preview {path, width, height, scale}: a point read on the preview is a frame point after multiplying x and y by scale. Images are never inlined, only paths. An argument the chosen path does not read is never ignored: actinglab refuses it (lab_tool_failed caused by validation_failed, naming it as argument and argument_problem in its details).",
            lab_errors!(),
            lab_job!()
        ),
        read_only: false,
        destructive: false,
        idempotent: false,
        input_schema: lab::observe_input,
        result_schema: lab::frame_result,
        run: lab::observe,
    },
    ToolDef {
        name: "ac_lab_do",
        title: "Lab do",
        tier: Tier::Author,
        description: concat!(
            "Runs actinglab --json do, one argument per flag, and answers its data verbatim. With capture it acts on the instance through the Runtime with exactly one of target (a Runtime element id), tap or swipe (instance: its alias; without dry_run, package or zip is required), and the Runtime records the Lab request in its ledger; this server then records one client.action (surface mcp) carrying the answer's req_id. Without dry_run each of its observations (before, after, every arrival sample) takes the frames the package's target consensus needs (up to five; the last is the current frame). Offline (scene, with up to 4 earlier frames of the same screen in sample_scenes, oldest first, as for ac_lab_observe) actinglab only plans, with dry_run, and touches no Runtime. The destructive guard flags (destructive, allow_destructive) are read only offline and with dry_run: with capture and without dry_run either one set is refused before actinglab runs, arguments_invalid {argument, argument_problem: conflicting}, and this server adds no guard of its own. An argument the chosen path does not read is never ignored: actinglab refuses it (lab_tool_failed caused by validation_failed, naming it as argument and argument_problem in its details). Every answer states frame_coordinates {device_pixels [width, height], orientation, applies_to}, the coordinate system of the frame the action was chosen on: tap, tap_rect and swipe are in these frame pixels, counted from the top-left pixel, never in a preview's. After an action it attaches the result by default, so no extra observe is needed: when the page after it is known, after_attachment {kind page_projection, observation} is that page's projection (its content_sha256 and after.projection_sequence are the next call's projection_hash and projection_sequence); otherwise after_attachment {kind frame, frame_coordinates, frame_path, preview {path, width, height, scale}} names the frame after the action and its JPEG preview (long edge at most 1024; frame point = preview point times scale). preview true (capture, not dry_run) also writes the preview of a known page's frame. Images are never inlined, only paths. An after_page wait that ends without the page fails with class safety, lab_tool_failed caused by lab_arrival_timeout; its raw_text is the operation's answer, arrival last. seed (an unsigned integer, passed as --seed; offline, with dry_run, or with capture and record) draws the point pressed in the box from that seed as the Runtime draws a run's, centre-hotspot with the touch keys of the install's configuration and uniform without them, instead of the box's centre; offline and with dry_run the answer then carries actual_click.seed and actual_click.sampling {algorithm, action_seed, source_regions, parameters} as actinglab gives it (with record, the recorded click carries point_rule and sampling), and the same seed draws the same point. A Runtime element (target with capture) is pressed at its projection point and tap and swipe exactly, so seed with them is refused.",
            lab_errors!(),
            lab_job!()
        ),
        read_only: false,
        destructive: false,
        idempotent: false,
        input_schema: lab::do_input,
        result_schema: lab::frame_result,
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
            "Runs actinglab --json record stop: stops the recording session and generates its linear-steps package (lab_dir for the package directory; dry_run validates and writes nothing), answered verbatim, with lab.binding_example, lab.binding_requires and the other binding parts ac_binding_draft takes. Only the stop that generates the package answers lab.binding_requires; a later one answers lab.status already_generated without it. After ac_record_mark on a stopped recording, the next stop generates the package again and answers lab.status regenerated. An answer larger than the output budget still carries lab.binding_example, lab.binding_requires, lab.prerequisite_entry_example and lab.catalog_on_failure_example inline when they fit. It opens no Runtime connection and records no client.action.",
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
            "Turns an ac_record_stop answer into a binding draft and changes nothing. Give exactly one of record_stop (the answer itself, also an exported one that kept its lab parts) and record_stop_export {path, sha256} (the export of an ac_record_stop answer, read only from %TEMP%\\actingcommand-mcp\\materials and only when its sha256 matches). binding_example, binding_requires, prerequisite_entry_example and catalog_on_failure_example come verbatim from it; binding_requires is null, with the warning binding_requires_unavailable, when the answer has none (a record stop answering already_generated). admission: actinglab package preflight on the recorded package (binding_example.scheduled_execution.package_path, by its path); its coverage is the admission scope, preflight_error when it does not pass. check_config {config, exit_code, outcome, report_export}: actingd check-config on the install's current actingd.config.json, which it only reads; the whole report is exported, its outcome is copied from it; the draft is not in it, and nothing is merged into any configuration or copy. manual_steps: binding_requires, then edit the configuration, approve in the UI, restart the daemon; all of them are the person's.",
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

/// A call to a tool of a tier this process was not started with: `tier_not_enabled` with the
/// tool's tier and `mcp_blocker` `server_tier` (U-3 retires it with the tiers).
pub(super) fn tier_not_enabled(tool: &ToolDef) -> ToolError {
    ToolError::outcome(
        "safety",
        Outcome::new(ActingctlCode::TierNotEnabled)
            .required_tier(tool.tier)
            .mcp_tool(tool.name),
    )
    .blocked_by(McpBlocker::ServerTier)
}

/// The `tools/call` result: the envelope as compact JSON text, and from 2025-06-18 on also
/// as `structuredContent`. A result over the budget becomes `output_budget_exceeded`, with the
/// payload's size (`actual_bytes`) and the structured budget (`limit_bytes`).
pub(super) fn call_result(outcome: ToolOutcome, era: Era) -> Value {
    let mut structured = call_envelope(outcome);
    if !fits(&structured) {
        let payload_bytes = structured.to_string().len();
        structured = call_envelope(Err(ToolError::outcome(
            "usage",
            Outcome::new(ActingctlCode::OutputBudgetExceeded)
                .actual_bytes(payload_bytes)
                .limit_bytes(PAYLOAD_LIMIT),
        )));
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

fn call_envelope(outcome: ToolOutcome) -> Value {
    let mut envelope = Map::new();
    match outcome {
        Ok(success) => {
            envelope.insert("ok".to_owned(), json!(true));
            envelope.insert("result".to_owned(), success.result);
            if !success.warnings.is_empty() {
                envelope.insert("warnings".to_owned(), Value::Array(success.warnings));
            }
        }
        Err(error) => {
            envelope.insert("ok".to_owned(), json!(false));
            envelope.insert("error".to_owned(), error.into_value());
        }
    }
    Value::Object(envelope)
}

/// An `actingcommand.outcome.v1` envelope (Workflow #378 A5): code, category and values, with
/// nested links.
pub(super) fn outcome_schema() -> Value {
    // The members are inserted one by one: an outcome's own member names are not `json!` keys.
    let mut properties = Map::new();
    properties.insert(
        "schema_version".to_owned(),
        json!({"const": actingcommand_contract::outcome::OUTCOME_SCHEMA_VERSION}),
    );
    properties.insert("code".to_owned(), json!({"type": "string"}));
    properties.insert(
        "category".to_owned(),
        json!({"type": "string", "enum": ["success", "info", "warning", "error", "fatal"]}),
    );
    properties.insert("values".to_owned(), json!({"type": "object"}));
    properties.insert(
        "causes".to_owned(),
        json!({"type": "array", "items": {"type": "object"}}),
    );
    properties.insert("causes_total".to_owned(), json!({"type": "integer"}));
    properties.insert("detail".to_owned(), json!({"const": "codes"}));
    let mut schema = Map::new();
    schema.insert("type".to_owned(), json!("object"));
    schema.insert("properties".to_owned(), Value::Object(properties));
    schema.insert(
        "required".to_owned(),
        json!(["schema_version", "code", "category"]),
    );
    Value::Object(schema)
}

/// Both envelope forms, with the tool's own result schema: `{ok, result, warnings?}` with each
/// warning `{outcome}`, or `{ok: false, error {class, outcome}}`.
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
                    "properties": {"outcome": outcome_schema()},
                    "required": ["outcome"],
                },
            },
            "error": {
                "type": "object",
                "properties": {
                    "class": {
                        "type": "string",
                        "enum": ["usage", "safety", "device", "runtime", "uncertain"],
                    },
                    "outcome": outcome_schema(),
                },
                "required": ["class", "outcome"],
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
        "properties": {
            "instance": instance_argument(),
            // Workflow #389 MCP-8: `--sections`, a list here.
            "sections": {
                "type": "array",
                "minItems": 1,
                "items": {
                    "type": "string",
                    "enum": crate::overview::Section::ALL.map(crate::overview::Section::name),
                },
                "description": "Only these blocks (default all); daemon and incomplete are always answered.",
            },
        },
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
                        // Workflow #390 九处-1: the emulator as the Runtime last read it.
                        "emulator_state": {
                            "type": "object",
                            "properties": {
                                "state": {"enum": ["on", "off", "unknown"]},
                                "observed_at_unix_ms": {"type": "integer"},
                            },
                            "required": ["state"],
                        },
                        // #411 A7: the last self-check of each backend entry, from the facts.
                        "backend_selfcheck": {
                            "type": "object",
                            "additionalProperties": {
                                "type": "object",
                                "properties": {
                                    "status": {"type": "string"},
                                    "checked_at_unix_ms": {"type": "integer"},
                                },
                                "required": ["status"],
                            },
                        },
                        "monitor": {"type": "object"},
                        // Workflow #389 MCP-5b: the last foreground package seen.
                        "foreground": {
                            "type": "object",
                            "properties": {
                                "package": {"type": "string"},
                                "observed_at_unix_ms": {"type": "integer"},
                            },
                            "required": ["package", "observed_at_unix_ms"],
                        },
                        "run": run_status_schema(),
                        "run_incomplete": {"type": "boolean"},
                        // Workflow #386 SR-6b, MCP-19b: the not-running row (`ac_schedule`).
                        "not_running": {
                            "type": "object",
                            "properties": {
                                "task_id": {"type": "string"},
                                "why": {"type": "object"},
                                "next_wake_unix_ms": {"type": "integer"},
                                "since_unix_ms": {"type": "integer"},
                            },
                            "required": ["why", "since_unix_ms"],
                        },
                        // Workflow #389 MCP-8: cut to the output budget.
                        "run_truncated": {"const": true},
                        "monitor_truncated": {"const": true},
                        "foreground_truncated": {"const": true},
                    },
                    "required": ["alias", "instance_id", "lease_active", "queued", "emulator_state"],
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
                    "build_truncated": {"const": true},
                },
            },
            // Workflow #385 P-m: the shadow measurement's status (`perf-status`).
            "performance": {"type": "object"},
            "performance_truncated": {"const": true},
            // Workflow #389 MCP-9a (command table §4.10).
            "catalog": {"type": ["object", "null"]},
            "catalog_truncated": {"const": true},
            "config_generation": {
                "type": "object",
                "properties": {
                    "running": {"type": "integer"},
                    "selected": {"type": "integer"},
                },
            },
            "config_generation_truncated": {"const": true},
            "watchdog": {"type": ["object", "null"]},
            "watchdog_truncated": {"const": true},
            // Workflow #384 D2-S5: what the fact refresh waits for (`fact-refresh`).
            "fact_refresh": {"type": "object"},
            "fact_refresh_truncated": {"const": true},
            "incomplete": {"type": "boolean"},
        },
        // Workflow #389 MCP-8: `sections` may leave out every block but the daemon.
        "required": ["daemon"],
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
            "package_label": {"type": "string"},
            "recovery_package_labels": {"type": "array", "items": {"type": "string"}},
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
            "recovery_package_labels",
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
            // Workflow #386 SR-3.
            "packs": {"type": "array", "items": {"type": "object"}},
            "packs_truncated": {"type": "boolean"},
            // Workflow #411 B15 R7.
            "remedies": {"type": "array", "items": {"type": "object"}},
            "remedies_truncated": {"type": "boolean"},
            "lift": {"type": "array", "items": {"type": "object"}},
            "lift_truncated": {"type": "boolean"},
            "repeating": {"type": "array", "items": {"type": "object"}},
            "repeating_truncated": {"type": "boolean"},
            "report_warnings": {"type": "array"},
            "report_warnings_truncated": {"type": "boolean"},
            "clock_slots": {"type": "array", "items": {"type": "object"}},
            "clock_slots_truncated": {"type": "boolean"},
            "clock_slots_total": {"type": "integer"},
        },
        "required": [
            "window",
            "runs",
            "errors_page",
            "suspended",
            "packs",
            "remedies",
            "lift",
            "repeating",
            "report_warnings",
            "clock_slots",
            "suspended_report",
            "runs_grouped",
            "failures_grouped",
        ],
    });
    // The rows from not_run on follow the literal, in their order: in it they reached serde_json's
    // macro recursion limit (as Lab B-2's help did).
    let properties = &mut result["properties"];
    // Workflow #386 SR-6b, MCP-19b: the deferred rows of the latest evaluation.
    properties["not_run"] = json!({
        "type": "array",
        "items": {
            "type": "object",
            "properties": {
                "instance_alias": {"type": "string"},
                "task_id": {"type": "string"},
                "why": {"type": "object"},
            },
            "required": ["instance_alias", "task_id", "why"],
        },
    });
    properties["not_run_truncated"] = json!({"type": "boolean"});
    properties["not_run_total"] = json!({"type": "integer"});
    properties["suspended_report"] = json!({"type": "object"});
    // Workflow #389 MCP-7 (command table §4.8).
    properties["runs_grouped"] = json!({
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
    });
    properties["runs_grouped_truncated"] = json!({"type": "boolean"});
    properties["failures_grouped"] = json!({
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
    });
    properties["failures_grouped_truncated"] = json!({"type": "boolean"});
    properties["incomplete"] = json!({"type": "boolean"});
    // Workflow #389 MCP-7: a group's codes, members of its row like final_page (as ac_runs adds a
    // row's failure_code).
    result["properties"]["runs_grouped"]["items"]["properties"]["failure_code"] =
        json!({"type": "string"});
    result["properties"]["failures_grouped"]["items"]["properties"]["code"] =
        json!({"type": "string"});
    result
}

/// A job of this process: its kind, phase, warnings and, once it ended, its result
/// (`job_result`: the answer, or `{error {class, outcome}}`; Workflow #378 A5 L6 renamed it
/// from `outcome`, which names only the envelope).
fn job_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "kind": {"type": "string"},
            "phase": {"type": "string"},
            "warnings": {"type": "array"},
            "job_result": {},
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
            "recovery_package": {"type": "string", "minLength": 1, "description": "The recovery package's path; used only when the package's entry is checked or recovery_ladder.rules.direct_runs = trigger, otherwise the run is refused with contained_task_recovery_binding_unused."},
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
    let mut schema = json!({
        "type": "object",
        "properties": {
            "instance": instance_argument(),
            "drain_timeout_s": {"type": "integer", "minimum": 1, "maximum": 600, "default": 60},
        },
        "additionalProperties": false,
    });
    // The argument `reason` is the CLI's `--reason` (#389 R2): a pause reason, a name, not a
    // code; inserted by name, outside `json!`, like every other non-code member that shares a
    // code-like name.
    schema["properties"]["reason"] =
        json!({"type": "string", "pattern": "^[a-z0-9_.-]{1,64}$", "default": "mcp.pause"});
    schema
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

// ---- Workflow #390 DRILL-1 (begin): `ac_ladder_drill`'s arguments (the CLI's `--instance` and
// `--device`, R2) and result (the CLI's line without its outcome). ----
fn ladder_drill_input() -> Value {
    json!({
        "type": "object",
        "properties": {
            "instance": instance_argument(),
            "device": {
                "type": "boolean",
                "description": "true: the drill acts on the device as a real ladder does (--device); false or absent: a plan drill that never acts on it.",
            },
        },
        "required": ["instance"],
        "additionalProperties": false,
    })
}

fn ladder_drill_result() -> Value {
    json!({
        "type": "object",
        "properties": {
            "instance_alias": {"type": "string"},
            "mode": {"type": "string", "enum": ["plan", "device"]},
            "ladder_request_id": {"type": "string"},
        },
        "required": ["instance_alias", "mode", "ladder_request_id"],
    })
}
// ---- Workflow #390 DRILL-1 (end) ----

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

/// Workflow #387 D1b: `ac_config_set`'s arguments.
fn config_set_input() -> Value {
    json!({
        "type": "object",
        "properties": {
            "changes": {
                "type": "array",
                "minItems": 1,
                "maxItems": 64,
                "items": {
                    "type": "object",
                    "properties": {
                        "key": {"type": "string", "minLength": 1},
                        "value": {"description": "The new JSON value; absent or null returns the key to its configuration-file value."},
                    },
                    "required": ["key"],
                    "additionalProperties": false,
                },
            },
        },
        "required": ["changes"],
        "additionalProperties": false,
    })
}

fn config_set_result() -> Value {
    json!({
        "type": "object",
        "properties": {"settings": {"type": "array", "items": {"type": "object"}}},
        "required": ["settings"],
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
