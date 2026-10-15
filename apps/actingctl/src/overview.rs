// SPDX-License-Identifier: AGPL-3.0-only

//! `actingctl overview` (Workflow #389 MCP-8; MCP-0 command table v1.1 §4.9), the CLI original
//! of MCP `ac_overview`, and the read both share. An overview is a composite read (#389 R5) of
//! named steps: Status (the daemon, the global pause and the instances), MonitorStatus (each
//! row's monitor), RuntimeFactSnapshot (each row's `foreground`, Workflow #389 MCP-5b) and, per
//! instance, its newest run of the last 24 h (QueryEvents); none of them records anything in the
//! ledger (§2.5). `--instance` (alias, instance_id or ADB port) narrows
//! the rows to one; `--sections` answers only the blocks it names, and the reads only those
//! blocks need are skipped; `daemon` and `incomplete` are always answered. The install block and
//! the Lab tool are found from the program root and the install root as mcp-serve finds them
//! (command table D5).
//!
//! Workflow #389 MCP-9a (command table §4.10) adds three blocks: `catalog`, the active policy
//! catalog generation from the program fact `config.catalog` (null while none is active);
//! `config_generation`, the generation the Runtime runs (`config.selection`) beside the one the
//! install root selects now (`install\active.json`), with `config_generation_mismatch` when they
//! differ and `package_binding_unreadable` for each binding of the selected generation that is
//! missing or unreadable (MCP-11's read); `watchdog`, `actingctl watchdog status` read locally
//! (null without an install root).
//!
//! stdout is one line, the same JSON as the `ac_overview` result, with every block whole, and
//! the warnings as a top-level `warnings` member (D3). Only MCP cuts blocks to its output budget
//! (`<block>_truncated`, §2.2; the order is in `mcp::observer`).
//!
//! Workflow #378 A5: the line also carries `outcome`: `runtime_overview_reported`, or the failure
//! alone.

use crate::cli_line;
use crate::codes::{ActingctlCode, ArgumentProblem};
use crate::instance_selector::{self, SelectorMiss};
use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::{Detail, IoOp, Outcome, RawSource};
use actingcommand_contract::{
    APPLICATION_FOREGROUND_FACT_KEY, CONFIG_CATALOG_FACT_KEY, CONFIG_SELECTION_FACT_KEY,
    ConfigCatalog, EventActor, EventSource, FactScalar, FactValue, InstalledProcess, InstanceId,
    RuntimeControlPlaneStatus, RuntimeErrorCode, RuntimeFactScope, RuntimeFactSnapshot,
    RuntimeInstanceStatus, RuntimeMonitorInstanceStatus,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig, RuntimeClientError};
use serde_json::{Map, Value, json};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const USAGE: &str = "usage: actingctl overview --state-root <path> [--root <install root>] [--instance <alias|instance_id|ADB port>] [--sections <block>[,<block>...]]\nblocks: global_pause, instances, run_window, lab_tool, install, catalog, config_generation, watchdog (default all); daemon and incomplete are always answered";
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
    // Workflow #389 MCP-9a (command table §4.10).
    Catalog,
    ConfigGeneration,
    Watchdog,
}

impl Section {
    pub(crate) const ALL: [Self; 8] = [
        Self::GlobalPause,
        Self::Instances,
        Self::RunWindow,
        Self::LabTool,
        Self::Install,
        Self::Catalog,
        Self::ConfigGeneration,
        Self::Watchdog,
    ];

    /// The block's member name in the answer.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::GlobalPause => "global_pause",
            Self::Instances => "instances",
            Self::RunWindow => "run_window",
            Self::LabTool => "lab_tool",
            Self::Install => "install",
            Self::Catalog => "catalog",
            Self::ConfigGeneration => "config_generation",
            Self::Watchdog => "watchdog",
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::GlobalPause => 0,
            Self::Instances => 1,
            Self::RunWindow => 2,
            Self::LabTool => 3,
            Self::Install => 4,
            Self::Catalog => 5,
            Self::ConfigGeneration => 6,
            Self::Watchdog => 7,
        }
    }
}

/// The blocks one overview answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Sections([bool; 8]);

impl Sections {
    /// Every block: `--sections` / `sections` omitted.
    pub(crate) const ALL: Self = Self([true; 8]);

    /// The blocks `names` names, each once or more; `None` when one of them names no block.
    pub(crate) fn parse<'a>(names: impl IntoIterator<Item = &'a str>) -> Option<Self> {
        let mut sections = [false; 8];
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
/// answer is not an empty system (nor a `catalog`; `config_generation` then has only what the
/// install root selects). `envelope` writes an outcome inside a block (the watchdog's) as the
/// side writes one.
pub(crate) fn read(
    client: Result<&RuntimeClient, Value>,
    request: &OverviewRequest,
    place: &Place<'_>,
    deadline: Instant,
    envelope: &dyn Fn(&Outcome) -> Value,
) -> Result<Overview, Failure> {
    let sections = request.sections;
    let mut warnings = Vec::new();
    // The install block first: its warning comes before any Runtime one.
    let install = sections
        .contains(Section::Install)
        .then(|| install_view(place, &mut warnings));
    // Workflow #389 MCP-9a: the blocks read locally, as the install block is.
    let selected = sections
        .contains(Section::ConfigGeneration)
        .then(|| selected_generation(place, &mut warnings))
        .flatten();
    let watchdog = sections
        .contains(Section::Watchdog)
        .then(|| watchdog_block(place, envelope));
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
            insert(
                &mut result,
                Section::ConfigGeneration,
                sections
                    .contains(Section::ConfigGeneration)
                    .then(|| config_generation_block(None, selected.as_ref(), &mut warnings)),
            );
            insert(&mut result, Section::Watchdog, watchdog);
            return Ok(Overview { result, warnings });
        }
    };
    let status = client.status().map_err(Failure::Runtime)?;
    let selected_alias = match &request.instance {
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
    // The program facts (Workflow #389 MCP-5b, MCP-9a): read once when a block needs them;
    // RuntimeFactSnapshot records nothing.
    let facts = if [
        Section::Instances,
        Section::Catalog,
        Section::ConfigGeneration,
    ]
    .into_iter()
    .any(|section| sections.contains(section))
    {
        match client.runtime_fact_snapshot() {
            Ok(facts) => Some(facts),
            Err(error) => {
                warnings.push(Warning::Runtime(error));
                incomplete = true;
                None
            }
        }
    } else {
        None
    };
    let instances = window
        .filter(|_| sections.contains(Section::Instances))
        .map(|window| {
            instance_rows(
                client,
                &status,
                selected_alias,
                window,
                deadline,
                facts.as_ref(),
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
    // Workflow #389 MCP-9a (command table §4.10).
    if sections.contains(Section::Catalog)
        && let Some(facts) = &facts
    {
        match catalog_block(facts) {
            Ok(block) => insert(&mut result, Section::Catalog, Some(block)),
            Err(outcome) => {
                warnings.push(Warning::Outcome(outcome));
                incomplete = true;
            }
        }
    }
    insert(
        &mut result,
        Section::ConfigGeneration,
        sections
            .contains(Section::ConfigGeneration)
            .then(|| config_generation_block(facts.as_ref(), selected.as_ref(), &mut warnings)),
    );
    insert(&mut result, Section::Watchdog, watchdog);
    if incomplete {
        result.insert("incomplete".to_owned(), json!(true));
    }
    Ok(Overview { result, warnings })
}

/// The generation the install root selects now and the configuration it selects.
struct Selected {
    /// `None` for an install root without generations (no `install\active.json`).
    generation: Option<u64>,
    config: PathBuf,
}

/// The configuration generation the install root selects now, read as `actingctl packs` reads
/// it (`install\active.json`, Workflow #389 MCP-11), and one `package_binding_unreadable` warning
/// per binding of that configuration whose path is missing or cannot be read (command table
/// §4.10; M-PKG §5.5: no package content is pinned, only the bound paths are checked). Without
/// an install root there is nothing to read; a selection or configuration that cannot be read is
/// its refusal, as a warning.
fn selected_generation(place: &Place<'_>, warnings: &mut Vec<Warning>) -> Option<Selected> {
    let root = place.install_root?;
    let (installation, config) = match crate::packs::selected_config(Some(root)) {
        Ok(selected) => selected,
        Err(error) => {
            warnings.push(Warning::Outcome(error.outcome()));
            return None;
        }
    };
    match crate::packs::read(
        installation.as_ref(),
        &config,
        &crate::packs::Query::default(),
    ) {
        Ok(answer) => warnings.extend(
            answer
                .warnings()
                .iter()
                .filter(|warning| warning.is(ActingctlCode::PackageBindingUnreadable))
                .cloned()
                .map(Warning::Outcome),
        ),
        Err(error) => warnings.push(Warning::Outcome(error.outcome())),
    }
    Some(Selected {
        generation: installation.map(|installation| installation.selection().generation),
        config,
    })
}

/// `{running?, selected?}`: the configuration generation the Runtime runs (`config.selection`,
/// an installed Runtime only) and the one the install root selects now; when both are known and
/// differ, the warning `config_generation_mismatch` names the selected configuration (`path`): a
/// restart is needed for it to take effect.
fn config_generation_block(
    facts: Option<&RuntimeFactSnapshot>,
    selected: Option<&Selected>,
    warnings: &mut Vec<Warning>,
) -> Value {
    let running = facts.and_then(running_generation);
    let mut block = Map::new();
    if let Some(running) = running {
        block.insert("running".to_owned(), json!(running));
    }
    if let Some(selected) = selected.and_then(|selected| selected.generation) {
        block.insert("selected".to_owned(), json!(selected));
    }
    if let (Some(running), Some(selected)) = (running, selected)
        && selected
            .generation
            .is_some_and(|generation| generation != running)
    {
        warnings.push(Warning::Outcome(
            Outcome::new(ActingctlCode::ConfigGenerationMismatch).path(&selected.config),
        ));
    }
    Value::Object(block)
}

/// The generation in the Runtime's `config.selection` row (decimal text), when it records one.
fn running_generation(facts: &RuntimeFactSnapshot) -> Option<u64> {
    let record = facts.records.iter().find(|record| {
        record.scope == RuntimeFactScope::Runtime && record.key == CONFIG_SELECTION_FACT_KEY
    })?;
    let FactValue::RecordList(rows) = &record.value else {
        return None;
    };
    match rows.first()?.get("config_generation")? {
        FactScalar::String(generation) => generation.parse().ok(),
        _ => None,
    }
}

/// The active policy catalog generation, `config.catalog` (register A5): `{catalog_id,
/// catalog_version, catalog_hash, activated_event_id, approval_id?, approval_disposition?,
/// previous_catalog_id?, previous_catalog_version?}`, or null while none is active. A record
/// that does not read back is its refusal (`invalid_fact_snapshot_record`).
fn catalog_block(facts: &RuntimeFactSnapshot) -> Result<Value, Outcome> {
    let Some(record) = facts.records.iter().find(|record| {
        record.scope == RuntimeFactScope::Runtime && record.key == CONFIG_CATALOG_FACT_KEY
    }) else {
        return Ok(Value::Null);
    };
    ConfigCatalog::from_fact_record(record)
        .map(|catalog| json!(catalog))
        .map_err(|error| Outcome::new(error.code()))
}

/// `actingctl watchdog status` of the install root, read locally (command table §4.10): its
/// `decision`, `attention`, `starts_24h` and `last_tick`; null without an install root.
fn watchdog_block(place: &Place<'_>, envelope: &dyn Fn(&Outcome) -> Value) -> Value {
    #[cfg(windows)]
    {
        if let Some(root) = place.install_root {
            return crate::watchdog::overview_block(root, envelope);
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (place, envelope);
    }
    Value::Null
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

/// One row per instance of `status` (only the `selected` one when given), each with its monitor,
/// the foreground package it last reported and its newest run first recorded since `window`. A
/// monitor, program fact or run read that fails leaves a warning and marks the overview
/// incomplete; a run read past the deadline is not started.
#[allow(clippy::too_many_arguments)]
fn instance_rows(
    client: &RuntimeClient,
    status: &RuntimeControlPlaneStatus,
    selected: Option<&str>,
    window: u64,
    deadline: Instant,
    facts: Option<&RuntimeFactSnapshot>,
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
        // Workflow #389 MCP-5b (MCP-0 command table v1.1 §4.6, D10, ruling Q7).
        if let Some(foreground) = facts.and_then(|facts| foreground(facts, instance.instance_id()))
        {
            row["foreground"] = foreground;
        }
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

/// `{package, observed_at_unix_ms}` (Workflow #389 MCP-5b, command table §4.6, D10): the
/// instance's program fact `application.foreground`, the package the foreground gate last saw
/// before a pointer input and recorded when it changed; so it is the last one seen, not a live
/// read. `None` when the instance has no such fact: no block and no error. The gate writes the
/// package as text (`record_foreground_application`), the one value this block reads.
fn foreground(facts: &RuntimeFactSnapshot, instance_id: InstanceId) -> Option<Value> {
    facts
        .records
        .iter()
        .find_map(|record| match (&record.scope, &record.value) {
            (RuntimeFactScope::Instance { instance_id: scope }, FactValue::String(package))
                if *scope == instance_id && record.key == APPLICATION_FOREGROUND_FACT_KEY =>
            {
                Some(json!({
                    "package": package,
                    "observed_at_unix_ms": record.observed_at_unix_ms,
                }))
            }
            _ => None,
        })
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
    let envelope = |outcome: &Outcome| cli_line::outcome_value(outcome, detail);
    let overview = read(connection, &request, &place, deadline, &envelope)?;
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

// Test-only: Workflow #389 MCP-9a (MCP-0 command table v1.1 §4.10; register A5), the catalog and
// configuration generation blocks of the overview and the binding lines.
#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_contract::codes::SanitizationCode;
    use actingcommand_contract::outcome::Category;
    use actingcommand_contract::{
        ApprovalDisposition, ConfigSelection, OriginModule, RuntimeFactRecord,
    };
    use tempfile::TempDir;

    fn facts(records: Vec<RuntimeFactRecord>) -> RuntimeFactSnapshot {
        RuntimeFactSnapshot {
            schema_version: "neutral".to_owned(),
            ledger_position: 1,
            taken_at_unix_ms: 1,
            snapshot_id: 1,
            part: 1,
            parts: 1,
            records,
        }
    }

    fn outcomes(warnings: &[Warning]) -> Vec<Outcome> {
        warnings
            .iter()
            .map(|warning| match warning {
                Warning::Outcome(outcome) => outcome.clone(),
                Warning::Runtime(error) => panic!("a Runtime warning: {}", error.outcome()),
            })
            .collect()
    }

    // Register A5: the catalog block is the active generation as `config.catalog` names it, its
    // versions as numbers and each optional member only when present; null while no generation
    // is active; a record that does not read back is refused (invalid_fact_snapshot_record).
    #[test]
    fn mcp9a_catalog_is_the_active_generation_null_without_one_and_refused_when_unreadable() {
        let hash = format!("sha256:{}", "a".repeat(64));
        let event = "evt_0123456789abcdef0123456789abcdef";
        let active = ConfigCatalog {
            catalog_id: "neutral.catalog".to_owned(),
            catalog_version: 3,
            catalog_hash: hash.clone(),
            activated_event_id: event.to_owned(),
            approval_id: Some("approval:neutral".to_owned()),
            approval_disposition: Some(ApprovalDisposition::Approved),
            previous_catalog_id: Some("neutral.catalog".to_owned()),
            previous_catalog_version: Some(2),
        };
        let block = |catalog: &ConfigCatalog| {
            catalog_block(&facts(vec![
                catalog.to_fact_record(1, OriginModule::Policy),
            ]))
            .expect("the catalog block")
        };
        assert_eq!(
            block(&active),
            json!({
                "catalog_id": "neutral.catalog",
                "catalog_version": 3,
                "catalog_hash": hash,
                "activated_event_id": event,
                "approval_id": "approval:neutral",
                "approval_disposition": "approved",
                "previous_catalog_id": "neutral.catalog",
                "previous_catalog_version": 2,
            })
        );
        let first = ConfigCatalog {
            approval_id: None,
            approval_disposition: None,
            previous_catalog_id: None,
            previous_catalog_version: None,
            ..active
        };
        assert_eq!(
            block(&first),
            json!({
                "catalog_id": "neutral.catalog",
                "catalog_version": 3,
                "catalog_hash": hash,
                "activated_event_id": event,
            })
        );
        assert_eq!(
            catalog_block(&facts(Vec::new())).expect("no active generation"),
            Value::Null
        );
        let unreadable = RuntimeFactRecord {
            scope: RuntimeFactScope::Runtime,
            key: CONFIG_CATALOG_FACT_KEY.to_owned(),
            value: FactValue::String("neutral".to_owned()),
            observed_at_unix_ms: 1,
            source: OriginModule::Policy,
            ttl_ms: None,
        };
        let refusal = catalog_block(&facts(vec![unreadable])).expect_err("an unreadable record");
        assert!(
            refusal.is(SanitizationCode::InvalidFactSnapshotRecord),
            "{refusal}"
        );
    }

    // §4.10: config_generation names the generation actingd runs (`config.selection`) and the
    // one the install root selects now; when both are known and differ, the warning
    // config_generation_mismatch names the selected configuration (`path`); otherwise there is
    // no warning.
    #[test]
    fn mcp9a_config_generation_names_both_generations_and_warns_only_when_they_differ() {
        let running = |generation: u64| {
            facts(vec![
                ConfigSelection {
                    config_generation: Some(generation),
                    config_sha256: format!("sha256:{}", "b".repeat(64)),
                    defaults_sha256: format!("sha256:{}", "c".repeat(64)),
                }
                .to_fact_record(1, OriginModule::Runtime),
            ])
        };
        let selected = Selected {
            generation: Some(2),
            config: PathBuf::from("neutral-install").join("actingd.config.json"),
        };
        let without_generation = Selected {
            generation: None,
            config: selected.config.clone(),
        };
        let cases = [
            (
                Some(running(1)),
                &selected,
                json!({"running": 1, "selected": 2}),
                true,
            ),
            (
                Some(running(2)),
                &selected,
                json!({"running": 2, "selected": 2}),
                false,
            ),
            (None, &selected, json!({"selected": 2}), false),
            (
                Some(running(1)),
                &without_generation,
                json!({"running": 1}),
                false,
            ),
        ];
        for (facts, selected, block, mismatch) in cases {
            let mut warnings = Vec::new();
            assert_eq!(
                config_generation_block(facts.as_ref(), Some(selected), &mut warnings),
                block
            );
            let warnings = outcomes(&warnings);
            if mismatch {
                assert!(
                    matches!(
                        warnings.as_slice(),
                        [warning] if warning.is(ActingctlCode::ConfigGenerationMismatch)
                            && warning.category() == Category::Warning
                            && warning.value("path").is_some()
                    ),
                    "{block}: {warnings:?}"
                );
            } else {
                assert!(warnings.is_empty(), "{block}: {warnings:?}");
            }
        }
    }

    // §4.10, M-PKG §5.5: each binding of the selected configuration whose path is missing is one
    // package_binding_unreadable naming its instance; a readable binding gives none; an install
    // root without generations selects none.
    #[test]
    fn mcp9a_each_unreadable_binding_of_the_selected_configuration_is_one_warning() {
        let root = TempDir::new().expect("tempdir");
        let package = root.path().join("packages").join("startup-a");
        fs::create_dir_all(&package).expect("create the package folder");
        fs::write(
            package.join("control.json"),
            json!({"package_id": "neutral.startup"}).to_string(),
        )
        .expect("write control.json");
        fs::write(
            root.path().join("actingd.config.json"),
            json!({
                "instances": [
                    {
                        "alias": "node-a",
                        "instance_id": "neutral-a",
                        "startup_package": {"package": "packages/startup-a"},
                    },
                    {
                        "alias": "node-b",
                        "instance_id": "neutral-b",
                        "startup_package": {"package": "packages/startup-missing"},
                    },
                ],
            })
            .to_string(),
        )
        .expect("write the configuration");
        let place = Place {
            root: None,
            install_root: Some(root.path()),
            state_root: None,
        };
        let mut warnings = Vec::new();
        let selected =
            selected_generation(&place, &mut warnings).expect("the selected configuration");
        assert_eq!(selected.generation, None);
        let warnings = outcomes(&warnings);
        assert!(
            matches!(
                warnings.as_slice(),
                [warning] if warning.is(ActingctlCode::PackageBindingUnreadable)
                    && warning.value("alias") == Some(&json!("node-b"))
            ),
            "{warnings:?}"
        );
    }
}
