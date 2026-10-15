// SPDX-License-Identifier: AGPL-3.0-only

//! `actingctl overview` (Workflow #389 MCP-8; MCP-0 command table v1.1 §4.9), the CLI original
//! of MCP `ac_overview`, and the read both share. An overview is a composite read (#389 R5) of
//! named steps: Status (the daemon, the global pause and the instances), MonitorStatus (each
//! row's monitor) and, per instance, its newest run of the last 24 h (QueryEvents); none of them
//! records anything in the ledger (§2.5). `--instance` (alias, instance_id or ADB port) narrows
//! the rows to one; `--sections` answers only the blocks it names, and the reads only those
//! blocks need are skipped; `daemon` and `incomplete` are always answered. The install block and
//! the Lab tool are found from the program root and the install root as mcp-serve finds them
//! (command table D5).
//!
//! stdout is one line, the same JSON as the `ac_overview` result, with every block whole, and
//! the warnings as a top-level `warnings` member (D3). Only MCP cuts blocks to its output budget
//! (`<block>_truncated`, §2.2; the order is in `mcp::observer`).
//!
//! Workflow #378 A5: the line also carries `outcome`: `runtime_overview_reported`, or the failure
//! alone.
//!
//! Workflow #385 P-m: the block `performance` is the shadow measurement's status
//! (`PerformanceStatus`, PERF §4.6), the object `actingctl perf-status` prints; `--instance`
//! narrows its rows as it narrows the instance rows.

use crate::cli_line;
use crate::codes::{ActingctlCode, ArgumentProblem};
use crate::instance_selector::{self, SelectorMiss};
use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::{Detail, IoOp, Outcome, RawSource};
use actingcommand_contract::{
    EventActor, EventSource, InstalledProcess, RuntimeControlPlaneStatus, RuntimeErrorCode,
    RuntimeInstanceStatus, RuntimeMonitorInstanceStatus, RuntimePerformanceStatus,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig, RuntimeClientError};
use serde_json::{Map, Value, json};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const USAGE: &str = "usage: actingctl overview --state-root <path> [--root <install root>] [--instance <alias|instance_id|ADB port>] [--sections <block>[,<block>...]]\nblocks: global_pause, instances, run_window, lab_tool, install, performance (default all); daemon and incomplete are always answered";
/// The run window: the last 24 h.
const RUN_WINDOW_MS: u64 = 24 * 60 * 60 * 1000;
/// `<root>\runtime\BUILD-MANIFEST.json` is read whole up to this size.
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;
/// How long the CLI reads: MCP's 25 s call budget, so that both sides end a long read alike.
const CLI_READ_BUDGET: Duration = Duration::from_secs(25);
/// Time kept back from the read's deadline before another run read starts.
const READ_RESERVE: Duration = Duration::from_secs(1);

/// The blocks `--sections` / `sections` names (command table §4.9). `daemon` and `incomplete`
/// are always answered and are not among them; a slice that adds a block adds its name here and
/// its place in the truncation order of `mcp::observer`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Section {
    GlobalPause,
    Instances,
    RunWindow,
    LabTool,
    Install,
    /// Workflow #385 P-m (PERF §4.6): the shadow measurement's status, as `perf-status`
    /// prints it.
    Performance,
}

impl Section {
    pub(crate) const ALL: [Self; 6] = [
        Self::GlobalPause,
        Self::Instances,
        Self::RunWindow,
        Self::LabTool,
        Self::Install,
        Self::Performance,
    ];

    /// The block's member name in the answer.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::GlobalPause => "global_pause",
            Self::Instances => "instances",
            Self::RunWindow => "run_window",
            Self::LabTool => "lab_tool",
            Self::Install => "install",
            Self::Performance => "performance",
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::GlobalPause => 0,
            Self::Instances => 1,
            Self::RunWindow => 2,
            Self::LabTool => 3,
            Self::Install => 4,
            Self::Performance => 5,
        }
    }
}

/// The blocks one overview answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Sections([bool; 6]);

impl Sections {
    /// Every block: `--sections` / `sections` omitted.
    pub(crate) const ALL: Self = Self([true; 6]);

    /// The blocks `names` names, each once or more; `None` when one of them names no block.
    pub(crate) fn parse<'a>(names: impl IntoIterator<Item = &'a str>) -> Option<Self> {
        let mut sections = [false; 6];
        for name in names {
            let section = Section::ALL
                .into_iter()
                .find(|section| section.name() == name)?;
            sections[section.index()] = true;
        }
        Some(Self(sections))
    }

    pub(crate) const fn contains(self, section: Section) -> bool {
        self.0[section.index()]
    }
}

/// One overview, as either side parsed it.
pub(crate) struct OverviewRequest {
    /// Alias, instance_id or ADB port; without it every instance.
    pub(crate) instance: Option<String>,
    pub(crate) sections: Sections,
}

/// Where the install lives, as the side found it.
pub(crate) struct Place<'a> {
    /// The program root: the selected slot `<root>\<A|B>`, or the install root of a layout
    /// without slots; its `runtime\BUILD-MANIFEST.json` is the build.
    pub(crate) root: Option<&'a Path>,
    /// The install root that holds `tools\actinglab.exe`.
    pub(crate) install_root: Option<&'a Path>,
    pub(crate) state_root: Option<&'a Path>,
}

/// One overview: its blocks, and the warnings each side renders as its own.
pub(crate) struct Overview {
    pub(crate) result: Map<String, Value>,
    pub(crate) warnings: Vec<Warning>,
}

/// A part of the overview that could not be read, reported beside it.
pub(crate) enum Warning {
    /// `install_build_manifest_unreadable`, or `call_budget_exhausted` naming the instance
    /// (`alias`) whose run was not read.
    Outcome(Outcome),
    /// A Runtime read failed: each side renders the client's failure as it renders it elsewhere.
    Runtime(RuntimeClientError),
}

/// Why an overview was refused or failed.
pub(crate) enum Failure {
    /// A CLI flag is unknown, repeated, missing its value or malformed: the flag as given.
    Argument {
        argument: String,
        problem: ArgumentProblem,
    },
    /// The CLI's `--root` names no install this process may read: its location's condition.
    Install(Outcome),
    Selector {
        miss: SelectorMiss,
        selector: String,
    },
    Clock,
    Runtime(RuntimeClientError),
}

impl Failure {
    fn argument(argument: impl AsRef<OsStr>, problem: ArgumentProblem) -> Self {
        Self::Argument {
            argument: argument.as_ref().to_string_lossy().into_owned(),
            problem,
        }
    }

    /// The failure as an outcome (Workflow #378 A5). A CLI flag failure is
    /// `actingctl_usage_invalid` with `argument` and `argument_problem` (MCP answers
    /// `arguments_invalid` for its argument failures before any read).
    pub(crate) fn outcome(&self) -> Outcome {
        match self {
            Self::Argument { argument, problem } => {
                Outcome::new(ActingctlCode::ActingctlUsageInvalid)
                    .argument(argument.as_str())
                    .argument_problem(*problem)
            }
            Self::Install(outcome) => outcome.clone(),
            Self::Selector { miss, selector } => miss.outcome(selector),
            Self::Clock => Outcome::new(ActingctlCode::ClockUnavailable),
            Self::Runtime(error) => error.outcome(),
        }
    }
}

/// The overview `request` asks for, read with `client` until `deadline`. `client` is the
/// Runtime connection, or the `{class, outcome}` the side renders when actingd does not answer:
/// then `daemon` is `{online: false, error}` and there is no `instances` member at all, since no
/// answer is not an empty system.
pub(crate) fn read(
    client: Result<&RuntimeClient, Value>,
    request: &OverviewRequest,
    place: &Place<'_>,
    deadline: Instant,
) -> Result<Overview, Failure> {
    let sections = request.sections;
    let mut warnings = Vec::new();
    // The install block first: its warning comes before any Runtime one.
    let install = sections
        .contains(Section::Install)
        .then(|| install_view(place, &mut warnings));
    let lab_tool = sections.contains(Section::LabTool).then(|| {
        let present = place
            .install_root
            .is_some_and(|root| root.join("tools").join("actinglab.exe").is_file());
        json!({"present": present})
    });
    let mut result = Map::new();
    let client = match client {
        Ok(client) => client,
        Err(error) => {
            result.insert(
                "daemon".to_owned(),
                json!({"online": false, "error": error}),
            );
            insert(&mut result, Section::LabTool, lab_tool);
            insert(&mut result, Section::Install, install);
            return Ok(Overview { result, warnings });
        }
    };
    let status = client.status().map_err(Failure::Runtime)?;
    let selected = match &request.instance {
        Some(selector) => Some(
            instance_selector::select(status.instances(), selector)
                .map_err(|miss| Failure::Selector {
                    miss,
                    selector: selector.clone(),
                })?
                .instance_alias(),
        ),
        None => None,
    };
    let window = if sections.contains(Section::Instances) || sections.contains(Section::RunWindow) {
        Some(now_unix_ms()?.saturating_sub(RUN_WINDOW_MS))
    } else {
        None
    };
    let mut incomplete = false;
    let instances = window
        .filter(|_| sections.contains(Section::Instances))
        .map(|window| {
            instance_rows(
                client,
                &status,
                selected,
                window,
                deadline,
                &mut warnings,
                &mut incomplete,
            )
        });
    result.insert(
        "daemon".to_owned(),
        json!({"online": true, "owner_epoch": status.owner_epoch()}),
    );
    if let Some(pause) = status
        .scheduling_pause()
        .filter(|_| sections.contains(Section::GlobalPause))
    {
        result.insert(Section::GlobalPause.name().to_owned(), json!(pause));
    }
    insert(&mut result, Section::Instances, instances);
    insert(
        &mut result,
        Section::RunWindow,
        window
            .filter(|_| sections.contains(Section::RunWindow))
            .map(|window| json!({"since_unix_ms": window})),
    );
    insert(&mut result, Section::LabTool, lab_tool);
    insert(&mut result, Section::Install, install);
    // Workflow #385 P-m (PERF §4.6): the shadow measurement's status; a read that fails
    // leaves a warning and no block, never an empty one.
    if sections.contains(Section::Performance) {
        match performance_block(client, selected) {
            Ok(status) => {
                result.insert(Section::Performance.name().to_owned(), json!(status));
            }
            Err(error) => {
                warnings.push(Warning::Runtime(error));
                incomplete = true;
            }
        }
    }
    if incomplete {
        result.insert("incomplete".to_owned(), json!(true));
    }
    Ok(Overview { result, warnings })
}

/// Workflow #385 P-m (PERF §4.6): the shadow measurement's status (`PerformanceStatus`), its
/// rows narrowed to the instance `selected` names (an alias) when given. `perf-status` prints
/// it and `ac_overview` answers it as its `performance` block.
pub(crate) fn performance_block(
    client: &RuntimeClient,
    selected: Option<&str>,
) -> Result<RuntimePerformanceStatus, RuntimeClientError> {
    let mut status = client.performance_status()?;
    if let Some(alias) = selected {
        status.instances.retain(|row| row.instance_alias == alias);
    }
    Ok(status)
}

fn insert(result: &mut Map<String, Value>, section: Section, block: Option<Value>) {
    if let Some(block) = block {
        result.insert(section.name().to_owned(), block);
    }
}

/// Now, in Unix milliseconds.
fn now_unix_ms() -> Result<u64, Failure> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
        .ok_or(Failure::Clock)
}

/// One row per instance of `status` (only the `selected` one when given), each with its monitor
/// and its newest run first recorded since `window`. A monitor or run read that fails leaves a
/// warning and marks the overview incomplete; a run read past the deadline is not started.
fn instance_rows(
    client: &RuntimeClient,
    status: &RuntimeControlPlaneStatus,
    selected: Option<&str>,
    window: u64,
    deadline: Instant,
    warnings: &mut Vec<Warning>,
    incomplete: &mut bool,
) -> Value {
    let monitors = match client.monitor_status() {
        Ok(monitors) => Some(monitors),
        Err(error) => {
            warnings.push(Warning::Runtime(error));
            *incomplete = true;
            None
        }
    };
    let mut rows = Vec::new();
    for instance in status
        .instances()
        .iter()
        .filter(|instance| selected.is_none_or(|alias| instance.instance_alias() == alias))
    {
        let monitor = monitors.as_ref().and_then(|monitors| {
            monitors
                .instances()
                .iter()
                .find(|monitor| monitor.instance_alias() == instance.instance_alias())
        });
        let mut row = instance_row(instance, monitor);
        // At most one brief run status per instance: its newest admitted run in the window,
        // which is the open one when a run is open.
        if Instant::now() + READ_RESERVE >= deadline {
            // The call budget left no time to read the runs of this instance.
            *incomplete = true;
            row["run_incomplete"] = json!(true);
            warnings.push(Warning::Outcome(
                Outcome::new(ActingctlCode::CallBudgetExhausted).alias(instance.instance_alias()),
            ));
        } else {
            match client.recent_runs(instance.instance_id(), window, 1) {
                Ok(latest) => {
                    if latest.incomplete {
                        *incomplete = true;
                        row["run_incomplete"] = json!(true);
                    }
                    if let Some(run) = latest.runs.first() {
                        row["run"] = json!(run);
                    }
                }
                Err(error) => {
                    *incomplete = true;
                    row["run_incomplete"] = json!(true);
                    warnings.push(Warning::Runtime(error));
                }
            }
        }
        rows.push(row);
    }
    Value::Array(rows)
}

fn instance_row(
    instance: &RuntimeInstanceStatus,
    monitor: Option<&RuntimeMonitorInstanceStatus>,
) -> Value {
    let mut row = Map::new();
    row.insert("alias".to_owned(), json!(instance.instance_alias()));
    row.insert("instance_id".to_owned(), json!(instance.instance_id()));
    if let Some(port) = instance.adb_port() {
        row.insert("adb_port".to_owned(), json!(port));
    }
    if let Some(game_id) = instance.game_id() {
        row.insert("game_id".to_owned(), json!(game_id));
    }
    row.insert("lease_active".to_owned(), json!(instance.lease_active()));
    row.insert("queued".to_owned(), json!(instance.queued_request_count()));
    if let Some(pause) = instance.pause() {
        // Data members whose names a code would take (`stage`, `*_reason`) are inserted by
        // name, outside `json!`.
        let mut members = Map::new();
        members.insert("revision".to_owned(), json!(pause.revision));
        members.insert("stage".to_owned(), json!(pause.stage));
        members.insert("pause_reason".to_owned(), json!(pause.pause_reason));
        row.insert("pause".to_owned(), Value::Object(members));
    }
    // Workflow #403: an instance an unconfirmed close fenced, and whether it waits for a person.
    if let Some(fence) = instance.resource_fence() {
        row.insert(
            "resource_fence".to_owned(),
            json!({
                "cause": fence.cause,
                "since_unix_ms": fence.since_unix_ms,
                "awaiting_person": fence.awaiting_person,
            }),
        );
    }
    if let Some(monitor) = monitor.filter(|monitor| monitor.policy().is_some()) {
        row.insert(
            "monitor".to_owned(),
            json!({"policy": monitor.policy(), "state": monitor.state()}),
        );
    }
    Value::Object(row)
}

/// `{root, state_root, build}`: the program root, the state root and, when the program root holds
/// `runtime\BUILD-MANIFEST.json`, its `commit_sha` and `source_artifact_name`. An unreadable
/// manifest is the warning `install_build_manifest_unreadable` (`path`), caused by the read
/// failure or the parser's `foreign_data_error`.
fn install_view(place: &Place<'_>, warnings: &mut Vec<Warning>) -> Value {
    let mut install = Map::new();
    if let Some(root) = place.root {
        install.insert("root".to_owned(), json!(root.display().to_string()));
    }
    if let Some(state_root) = place.state_root {
        install.insert(
            "state_root".to_owned(),
            json!(state_root.display().to_string()),
        );
    }
    let manifest = place
        .root
        .map(|root| root.join("runtime").join("BUILD-MANIFEST.json"))
        .filter(|manifest| manifest.is_file());
    if let Some(manifest) = manifest {
        let document = read_bounded(&manifest, MAX_MANIFEST_BYTES).and_then(|bytes| {
            serde_json::from_slice::<Value>(&bytes).map_err(|error| {
                Outcome::new(ContractCode::ForeignDataError)
                    .raw_source(RawSource::Json)
                    .raw_text(error.to_string())
                    .path(&manifest)
            })
        });
        match document {
            Ok(document) => {
                let mut build = Map::new();
                for key in ["commit_sha", "source_artifact_name"] {
                    if let Some(value) = document.get(key).filter(|value| value.is_string()) {
                        build.insert(key.to_owned(), value.clone());
                    }
                }
                install.insert("build".to_owned(), Value::Object(build));
            }
            Err(cause) => warnings.push(Warning::Outcome(
                Outcome::new(ActingctlCode::InstallBuildManifestUnreadable)
                    .path(&manifest)
                    .caused_by(cause),
            )),
        }
    }
    Value::Object(install)
}

/// Reads a whole file of at most `limit` bytes: a failed open or read is `foreign_os_error`
/// with the `path` and `io_op`; a larger file is `mcp_file_too_large` with `path` and
/// `limit_bytes`. mcp-serve reads its files with it too (`mcp::runtime`).
pub(crate) fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, Outcome> {
    let file = fs::File::open(path)
        .map_err(|error| Outcome::from_io_error(&error, IoOp::Open).path(path))?;
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| Outcome::from_io_error(&error, IoOp::Read).path(path))?;
    if bytes.len() as u64 > limit {
        return Err(Outcome::new(ActingctlCode::McpFileTooLarge)
            .path(path)
            .limit_bytes(limit));
    }
    Ok(bytes)
}

/// Runs `overview`; `None` leaves every other command to the CLI, unchanged. One result line on
/// stdout (Workflow #378 A5): the overview with `runtime_overview_reported`, or `{outcome}` with
/// the failure (a flag failure's developer line adds the usage) and exit 1.
pub(crate) fn dispatch(arguments: &[OsString]) -> Option<ExitCode> {
    if arguments.first()?.to_str()? != "overview" {
        return None;
    }
    Some(match run(arguments) {
        Ok(members) => cli_line::success(
            members,
            &Outcome::new(ActingctlCode::RuntimeOverviewReported),
        ),
        Err(failure @ Failure::Argument { .. }) => {
            cli_line::failure(&failure.outcome(), Some(USAGE))
        }
        Err(failure) => cli_line::failure(&failure.outcome(), None),
    })
}

fn run(arguments: &[OsString]) -> Result<Map<String, Value>, Failure> {
    let (state_root, root, request) = parse(arguments)?;
    let deadline = Instant::now() + CLI_READ_BUDGET;
    let (program_root, install_root) = install_roots(root.as_deref()).map_err(Failure::Install)?;
    let place = Place {
        root: program_root.as_deref(),
        install_root: install_root.as_deref(),
        state_root: Some(&state_root),
    };
    let detail = cli_line::detail().unwrap_or(Detail::Full);
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        &state_root,
        actingcommand_runtime_client::RuntimeChannel::Cli,
        EventActor::Cli,
        EventSource::Cli,
    ));
    let connection = match &client {
        Ok(client) => Ok(client),
        Err(error) => Err(daemon_error(error, &state_root, detail)),
    };
    let overview = read(connection, &request, &place, deadline)?;
    let mut members = overview.result;
    if !overview.warnings.is_empty() {
        let warnings = overview
            .warnings
            .iter()
            .map(|warning| match warning {
                Warning::Outcome(outcome) => cli_line::warning(outcome),
                Warning::Runtime(error) => cli_line::warning(&error.outcome()),
            })
            .collect();
        members.insert("warnings".to_owned(), Value::Array(warnings));
    }
    Ok(members)
}

/// actingd does not answer for this state root: `{class, outcome}` as mcp-serve's daemon block
/// writes it, the class the client's and the outcome `runtime_unavailable` with the state root,
/// caused by the client's outcome.
fn daemon_error(error: &RuntimeClientError, state_root: &Path, detail: Detail) -> Value {
    let outcome = Outcome::new(RuntimeErrorCode::RuntimeUnavailable)
        .state_root(state_root)
        .caused_by(error.outcome());
    json!({
        "class": error.disposition().as_str(),
        "outcome": cli_line::outcome_value(&outcome, detail),
    })
}

/// The program root and the install root, found as mcp-serve finds them (`mcp::runtime`,
/// command table D5): this process's install selection, which a given `--root` must name; else
/// the selection of `--root`, or `--root` itself when it has none; else the install root this
/// executable runs from, when it runs from one.
fn install_roots(root: Option<&Path>) -> Result<(Option<PathBuf>, Option<PathBuf>), Outcome> {
    let process = actingcommand_contract::process_installation()
        .map_err(|error| Outcome::new(error.code()))?;
    let installation = match process {
        Some(selected) => {
            if let Some(root) = root {
                // A --root that is not the root this process runs from, readable or not.
                let mismatch = || {
                    Outcome::new(ActingctlCode::InstallProcessRootMismatch)
                        .path(root)
                        .install_root(selected.root())
                };
                let canonical = fs::canonicalize(root).map_err(|error| {
                    mismatch()
                        .caused_by(Outcome::from_io_error(&error, IoOp::Canonicalize).path(root))
                })?;
                if canonical != selected.root() {
                    return Err(mismatch());
                }
            }
            Some(selected.clone())
        }
        None => root
            .map(InstalledProcess::read)
            .transpose()
            .map_err(|error| Outcome::new(error.code()))?
            .flatten(),
    };
    let program_root = installation
        .as_ref()
        .map(InstalledProcess::program_root)
        .or_else(|| root.map(Path::to_path_buf))
        .or_else(crate::packs::executable_install_root);
    let install_root = installation
        .as_ref()
        .map(|selected| selected.root().to_path_buf())
        .or_else(|| program_root.clone());
    Ok((program_root, install_root))
}

/// `--state-root` (required), `--root` and the flags of §4.9, each at most once and with a
/// value; any other flag, a repeated one or a malformed value is `actingctl_usage_invalid` with
/// the flag (`argument`) and what is wrong with it (`argument_problem`). `--sections` is a
/// comma-separated list of block names.
fn parse(arguments: &[OsString]) -> Result<(PathBuf, Option<PathBuf>, OverviewRequest), Failure> {
    let mut state_root = None;
    let mut root = None;
    let mut instance = None;
    let mut sections = None;
    let mut rest = arguments[1..].iter();
    while let Some(flag) = rest.next() {
        let (name, slot) = match flag.to_str() {
            Some(name @ "--state-root") => (name, &mut state_root),
            Some(name @ "--root") => (name, &mut root),
            Some(name @ "--instance") => (name, &mut instance),
            Some(name @ "--sections") => (name, &mut sections),
            Some(_) => return Err(Failure::argument(flag, ArgumentProblem::Unknown)),
            None => return Err(Failure::argument(flag, ArgumentProblem::NotUtf8)),
        };
        let value = rest
            .next()
            .ok_or_else(|| Failure::argument(name, ArgumentProblem::ValueMissing))?
            .to_str()
            .ok_or_else(|| Failure::argument(name, ArgumentProblem::NotUtf8))?;
        if value.trim().is_empty() {
            return Err(Failure::argument(name, ArgumentProblem::ValueMissing));
        }
        if slot.replace(value.to_owned()).is_some() {
            return Err(Failure::argument(name, ArgumentProblem::Repeated));
        }
    }
    let sections = match sections {
        None => Sections::ALL,
        Some(text) => Sections::parse(text.split(','))
            .ok_or_else(|| Failure::argument("--sections", ArgumentProblem::NotRecognised))?,
    };
    let state_root =
        state_root.ok_or_else(|| Failure::argument("--state-root", ArgumentProblem::Required))?;
    Ok((
        PathBuf::from(state_root),
        root.map(PathBuf::from),
        OverviewRequest { instance, sections },
    ))
}
