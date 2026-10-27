// SPDX-License-Identifier: AGPL-3.0-only

//! `actingctl packs` (Workflow #389 MCP-11, command table v1.1 §4.12): the packages the selected
//! configuration generation binds, read offline from that `actingd.config.json` (ruling Q2). It
//! sends no Runtime request and writes no ledger record; `source.kind` says the answer comes from
//! the selected generation, which a running actingd may not have loaded yet. Each row carries the
//! package id, its path and its role only (option 丙: no package digest). `ac_packs`
//! (`mcp/packs.rs`) answers the same document through [`read`]. Workflow #378 A5: the CLI line
//! also carries `outcome` (`package_bindings_reported`, or the refusal alone), each warning is
//! `{outcome}`, and a refusal is its outcome. Workflow #357 DB-1 (register §1.65 TB4): when the
//! configuration sets `standard_packs.root`, the answer also lists the tables of each sound game
//! root, as check-config does (`standard_packs.games[].tables`).

use crate::cli_line;
use crate::codes::{ActingctlCode, ArgumentProblem};
use crate::instance_selector::SelectorMiss;
use actingcommand_contract::codes::{ContractCode, SharedCode};
use actingcommand_contract::outcome::{IoOp, Outcome, RawSource};
use actingcommand_contract::{
    InstalledProcess, STANDARD_PACK_BUNDLE_FILE, STANDARD_PACK_DISPLAY_LAYOUT_FILE,
    STANDARD_PACK_INTERRUPTS_FILE, STANDARD_PACK_NAVIGATION_FILE, STANDARD_PACK_PAGES_FILE,
    STANDARD_PACK_TABLES_DIRECTORY, StandardPackRootBytes, StandardPackTableReport,
    read_standard_pack_root, read_standard_pack_tables,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

const COMMAND: &str = "packs";
const USAGE: &str = "usage: actingctl packs [--root <install root>] [--instance <alias|instance_id|adb port>] [--role procedure|startup|resource_package]";
/// The configuration of an install root without slots, as mcp-serve finds it.
pub(crate) const ROOT_CONFIG_FILE: &str = "actingd.config.json";
/// The file of a package that names its `package_id`.
const CONTROL_FILE: &str = "control.json";
/// A procedure binding's `procedure_ref` is this prefix and the package id (M-PKG §4.4).
const PROCEDURE_PACKAGE_PREFIX: &str = "package.";
/// A package's control file is read whole up to this size.
const MAX_CONTROL_BYTES: u64 = 1024 * 1024;

/// Where a configuration binds a package (command table §4.12, roles). The recognition
/// maintenance binding comes with D2-S2. Workflow #373 NAV-3a (M-373 §4.3): prerequisite and
/// return-home packages are no configuration bindings any more (the standard-pack registry
/// holds them), so the roles `prerequisite` and `return_home` are gone with their keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Role {
    /// `policy.procedure_manifest[]`: installation level.
    Procedure,
    /// `instances[].startup_package`: per instance.
    Startup,
    /// `instances[].resource_package`: per instance, path only.
    ResourcePackage,
}

#[cfg_attr(not(feature = "mcp"), allow(dead_code))]
impl Role {
    pub(crate) const ALL: [Self; 3] = [Self::Procedure, Self::Startup, Self::ResourcePackage];

    /// The role a `--role` flag or `role` argument names.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        serde_json::from_value(Value::String(text.to_owned())).ok()
    }
}

/// What one call asks for: one instance (alias, instance_id or ADB port, as the configuration
/// names them) or every one, and one role or every one.
#[derive(Default)]
pub(crate) struct Query<'a> {
    pub(crate) instance: Option<&'a str>,
    pub(crate) role: Option<Role>,
}

/// The answer of `actingctl packs` and the `result` of `ac_packs`.
#[derive(Serialize)]
pub(crate) struct Packs {
    source: Source,
    instances: Vec<InstanceRow>,
    installation: Vec<InstallationRow>,
    /// Workflow #357 DB-1 (register §1.65 TB4): present when the configuration sets
    /// `standard_packs.root`.
    #[serde(skip_serializing_if = "Option::is_none")]
    standard_packs: Option<StandardPacksRow>,
}

#[cfg_attr(not(feature = "mcp"), allow(dead_code))]
impl Packs {
    pub(crate) const fn source(&self) -> &Source {
        &self.source
    }
}

/// Which configuration the answer read: the selected generation.
#[derive(Serialize)]
pub(crate) struct Source {
    kind: SourceKind,
    config: String,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum SourceKind {
    SelectedConfig,
}

#[derive(Serialize)]
struct InstanceRow {
    alias: String,
    instance_id: String,
    packs: Vec<PackRow>,
}

#[derive(Serialize)]
struct PackRow {
    role: Role,
    #[serde(skip_serializing_if = "Option::is_none")]
    package_id: Option<String>,
    path: String,
    readable: bool,
}

/// Workflow #357 DB-1 (register §1.65 TB4): the configured standard-pack root and, per sound
/// game root, its tables as check-config lists them. Per game, not per instance: neither
/// `instance` nor `role` narrows it.
#[derive(Serialize)]
struct StandardPacksRow {
    root: String,
    games: Vec<StandardPackGameRow>,
}

#[derive(Serialize)]
struct StandardPackGameRow {
    game: String,
    tables: Vec<StandardPackTableReport>,
}

/// A binding of the installation. `readable` is present exactly when `path` is.
#[derive(Serialize)]
struct InstallationRow {
    role: Role,
    #[serde(skip_serializing_if = "Option::is_none")]
    package_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    readable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    procedure_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    operation_id: Option<String>,
    scheduled: bool,
}

pub(crate) struct Answer {
    packs: Packs,
    /// Each warning once: `package_binding_unreadable {path, alias?}`.
    warnings: Vec<Outcome>,
}

#[cfg_attr(not(feature = "mcp"), allow(dead_code))]
impl Answer {
    pub(crate) const fn packs(&self) -> &Packs {
        &self.packs
    }

    pub(crate) fn warnings(&self) -> &[Outcome] {
        &self.warnings
    }

    /// What the CLI prints beside its `outcome`, and what `ac_packs` exports when its answer is
    /// over the output budget: the answer with its warnings, each `{outcome}` as the side's
    /// `warning` renders it, as a top-level member present only when there are some (command
    /// table D3).
    pub(crate) fn document(
        &self,
        warning: impl Fn(&Outcome) -> Value,
    ) -> Result<Map<String, Value>, serde_json::Error> {
        let mut document = cli_line::members(serde_json::to_value(&self.packs)?, "packs");
        if !self.warnings.is_empty() {
            document.insert(
                "warnings".to_owned(),
                Value::Array(self.warnings.iter().map(warning).collect()),
            );
        }
        Ok(document)
    }
}

/// A refusal of `packs` as its outcome (Workflow #378 A5): the code with its values (a
/// configuration's `path`, a selector's `instance_selector`, a flag's `argument`) and the
/// failure beneath it as its cause.
#[derive(Debug)]
pub(crate) struct PacksError(Outcome);

impl PacksError {
    fn usage(argument: impl AsRef<OsStr>, problem: ArgumentProblem) -> Self {
        Self(
            Outcome::new(ActingctlCode::ActingctlUsageInvalid)
                .argument(argument.as_ref().to_string_lossy())
                .argument_problem(problem),
        )
    }

    /// The refusal's outcome.
    pub(crate) fn outcome(&self) -> Outcome {
        self.0.clone()
    }
}

impl fmt::Display for PacksError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// The parts of `actingd.config.json` this command reads; every other member is left unread.
#[derive(Deserialize)]
struct ConfigFile {
    instances: Vec<InstanceEntry>,
    #[serde(default)]
    policy: Option<PolicySection>,
    /// Workflow #357 DB-1: the standard-pack root (Workflow #373 NAV-6).
    #[serde(default)]
    standard_packs: Option<StandardPacksSection>,
}

#[derive(Deserialize)]
struct StandardPacksSection {
    root: PathBuf,
}

#[derive(Deserialize)]
struct InstanceEntry {
    alias: String,
    instance_id: String,
    #[serde(default)]
    port: Option<u16>,
    #[serde(default)]
    startup_package: Option<StartupEntry>,
    #[serde(default)]
    resource_package: Option<PathBuf>,
}

#[derive(Deserialize)]
struct StartupEntry {
    package: PathBuf,
}

#[derive(Deserialize)]
struct PolicySection {
    procedure_manifest: Vec<ProcedureEntry>,
}

#[derive(Deserialize)]
struct ProcedureEntry {
    procedure_ref: String,
    operation_id: String,
    #[serde(default)]
    scheduled_execution: Option<ScheduledExecution>,
}

#[derive(Deserialize)]
struct ScheduledExecution {
    #[serde(default)]
    package_path: Option<PathBuf>,
}

/// A package's control file, for its `package_id`.
#[derive(Deserialize)]
struct ControlFile {
    package_id: String,
}

/// Reads the selected configuration `config` (through `installation`'s checked read when there
/// is an install selection, as actingd does) and lists its package bindings. A configuration
/// that cannot be read is `config_read_failed`, one that does not decode `config_decode_failed`
/// (ruling Q6), each with the configuration's `path` and the failure beneath it as its cause.
pub(crate) fn read(
    installation: Option<&InstalledProcess>,
    config: &Path,
    query: &Query<'_>,
) -> Result<Answer, PacksError> {
    read_answer(installation, config, query, false)
}

/// Workflow #357 DB-1 (register §1.65 TB4): [`read`] for `actingctl packs` and `ac_packs`, which
/// also list the tables of the configured standard-pack root (`standard_packs`). Only these two
/// read the tables: other readers of the bindings (the overview) do not.
pub(crate) fn read_with_tables(
    installation: Option<&InstalledProcess>,
    config: &Path,
    query: &Query<'_>,
) -> Result<Answer, PacksError> {
    read_answer(installation, config, query, true)
}

fn read_answer(
    installation: Option<&InstalledProcess>,
    config: &Path,
    query: &Query<'_>,
    tables: bool,
) -> Result<Answer, PacksError> {
    let unreadable = |path: &Path, cause: Outcome| {
        PacksError(
            Outcome::new(SharedCode::ConfigReadFailed)
                .path(path)
                .caused_by(cause),
        )
    };
    let config = std::path::absolute(config)
        .map_err(|error| unreadable(config, Outcome::from_io_error(&error, IoOp::Absolute)))?;
    let bytes = match installation {
        Some(selected) => selected
            .config_bytes()
            .map_err(|error| unreadable(&config, Outcome::new(error.code())))?,
        None => fs::read(&config)
            .map_err(|error| unreadable(&config, Outcome::from_io_error(&error, IoOp::Read)))?,
    };
    let file = serde_json::from_slice::<ConfigFile>(&bytes).map_err(|error| {
        PacksError(
            Outcome::new(SharedCode::ConfigDecodeFailed)
                .path(&config)
                .caused_by(
                    Outcome::new(ContractCode::ForeignDataError)
                        .raw_source(RawSource::Json)
                        .raw_text(error.to_string()),
                ),
        )
    })?;
    let selected = match query.instance {
        Some(selector) => vec![select(&file.instances, selector)?],
        None => file.instances.iter().collect(),
    };
    let mut rows = Rows {
        base: config.parent().map(Path::to_path_buf).unwrap_or_default(),
        role: query.role,
        warnings: Vec::new(),
    };
    let instances = selected
        .into_iter()
        .map(|instance| rows.instance(instance))
        .collect();
    let installation = rows.installation(&file);
    let standard_packs = match &file.standard_packs {
        Some(section) if tables => Some(rows.standard_packs(&section.root, system_unix_ms()?)),
        _ => None,
    };
    Ok(Answer {
        packs: Packs {
            source: Source {
                kind: SourceKind::SelectedConfig,
                config: config.display().to_string(),
            },
            instances,
            installation,
            standard_packs,
        },
        warnings: rows.warnings,
    })
}

/// The system clock in Unix milliseconds; a clock before 1970 is `system_clock_invalid`.
fn system_unix_ms() -> Result<u64, PacksError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
        .ok_or_else(|| PacksError(Outcome::new(SharedCode::SystemClockInvalid)))
}

/// The configured instance a selector names by alias, instance_id or ADB port; else
/// `instance_unknown` or `instance_ambiguous` with the selector.
fn select<'c>(
    instances: &'c [InstanceEntry],
    selector: &str,
) -> Result<&'c InstanceEntry, PacksError> {
    let port = selector.parse::<u16>().ok();
    let mut matching = instances.iter().filter(|instance| {
        instance.alias == selector
            || instance.instance_id == selector
            || port.is_some_and(|port| instance.port == Some(port))
    });
    let selected = matching
        .next()
        .ok_or_else(|| PacksError(SelectorMiss::Unknown.outcome(selector)))?;
    if matching.next().is_some() {
        return Err(PacksError(SelectorMiss::Ambiguous.outcome(selector)));
    }
    Ok(selected)
}

struct Rows {
    /// The configuration's folder: relative paths resolve against it, as actingd's do.
    base: PathBuf,
    role: Option<Role>,
    warnings: Vec<Outcome>,
}

impl Rows {
    fn wants(&self, role: Role) -> bool {
        self.role.is_none_or(|wanted| wanted == role)
    }

    /// The absolute path a configured path names: a relative one resolves against the
    /// configuration's folder, one component at a time.
    fn resolve(&self, configured: &Path) -> PathBuf {
        if configured.is_absolute() {
            return configured.to_path_buf();
        }
        let mut path = self.base.clone();
        for component in configured.components() {
            path.push(component);
        }
        path
    }

    /// Records a warning once.
    fn warn(&mut self, warning: Outcome) {
        if !self.warnings.contains(&warning) {
            self.warnings.push(warning);
        }
    }

    /// Records a bound path that does not exist or cannot be read: the path, and the instance
    /// that binds it (`alias`) unless the installation does.
    fn unreadable(&mut self, path: &Path, alias: Option<&str>) {
        let warning = Outcome::new(ActingctlCode::PackageBindingUnreadable).path(path);
        self.warn(match alias {
            Some(alias) => warning.alias(alias),
            None => warning,
        });
    }

    fn instance(&mut self, instance: &InstanceEntry) -> InstanceRow {
        let mut packs = Vec::new();
        if self.wants(Role::Startup)
            && let Some(startup) = &instance.startup_package
        {
            let path = self.resolve(&startup.package);
            let package_id = startup_package_id(&path);
            let readable = match path_kind(&path) {
                PathKind::Directory => package_id.is_some(),
                PathKind::File => true,
                PathKind::Unreadable => false,
            };
            if !readable {
                self.unreadable(&path, Some(&instance.alias));
            }
            packs.push(PackRow {
                role: Role::Startup,
                package_id,
                path: path.display().to_string(),
                readable,
            });
        }
        if self.wants(Role::ResourcePackage)
            && let Some(resource) = &instance.resource_package
        {
            let path = self.resolve(resource);
            let readable = !matches!(path_kind(&path), PathKind::Unreadable);
            if !readable {
                self.unreadable(&path, Some(&instance.alias));
            }
            packs.push(PackRow {
                role: Role::ResourcePackage,
                package_id: None,
                path: path.display().to_string(),
                readable,
            });
        }
        InstanceRow {
            alias: instance.alias.clone(),
            instance_id: instance.instance_id.clone(),
            packs,
        }
    }

    /// A bound path of the installation as `(path, readable)`.
    fn installation_path(&mut self, configured: &Path) -> (String, bool) {
        let path = self.resolve(configured);
        let readable = !matches!(path_kind(&path), PathKind::Unreadable);
        if !readable {
            self.unreadable(&path, None);
        }
        (path.display().to_string(), readable)
    }

    fn installation(&mut self, file: &ConfigFile) -> Vec<InstallationRow> {
        let mut rows = Vec::new();
        if self.wants(Role::Procedure) {
            for binding in file
                .policy
                .as_ref()
                .map(|policy| policy.procedure_manifest.as_slice())
                .unwrap_or_default()
            {
                let bound = binding
                    .scheduled_execution
                    .as_ref()
                    .and_then(|scheduled| scheduled.package_path.as_deref())
                    .map(|path| self.installation_path(path));
                rows.push(InstallationRow {
                    role: Role::Procedure,
                    package_id: binding
                        .procedure_ref
                        .strip_prefix(PROCEDURE_PACKAGE_PREFIX)
                        .filter(|package_id| !package_id.is_empty())
                        .map(str::to_owned),
                    readable: bound.as_ref().map(|(_, readable)| *readable),
                    path: bound.map(|(path, _)| path),
                    procedure_ref: Some(binding.procedure_ref.clone()),
                    operation_id: Some(binding.operation_id.clone()),
                    scheduled: binding.scheduled_execution.is_some(),
                });
            }
        }
        rows
    }

    /// Workflow #357 DB-1 (register §1.65 TB3, TB4): the tables of every sound game root below
    /// the configured standard-pack root, read as the Runtime's startup and check-config read
    /// them, their column expiries judged at `now_unix_ms`. A broken game root's tables are not
    /// read and the root is not listed (check-config and the Runtime's start report it); a root
    /// that cannot be listed is the warning `package_binding_unreadable`, and a `tables/` that
    /// cannot be listed is the warning `standard_pack_table_unreadable`.
    fn standard_packs(&mut self, configured: &Path, now_unix_ms: u64) -> StandardPacksRow {
        let root = self.resolve(configured);
        let mut games = Vec::new();
        match game_directories(&root) {
            Some(directories) => {
                for (game, directory) in directories {
                    if !sound_game_root(&directory, &game) {
                        continue;
                    }
                    let tables = match read_standard_pack_tables(&directory, &game) {
                        Ok(files) => files
                            .iter()
                            .map(|file| StandardPackTableReport::of(file, now_unix_ms))
                            .collect(),
                        Err(problem) => {
                            self.warn(problem);
                            Vec::new()
                        }
                    };
                    games.push(StandardPackGameRow { game, tables });
                }
            }
            None => self.unreadable(&root, None),
        }
        StandardPacksRow {
            root: root.display().to_string(),
            games,
        }
    }
}

/// The game directories below a standard-pack root, by name; `None` when the root cannot be
/// listed.
fn game_directories(root: &Path) -> Option<Vec<(String, PathBuf)>> {
    let mut games = Vec::new();
    for entry in fs::read_dir(root).ok()? {
        let entry = entry.ok()?;
        let kind = entry.file_type().ok()?;
        let name = entry.file_name();
        if kind.is_dir()
            && let Some(name) = name.to_str()
        {
            games.push((name.to_owned(), entry.path()));
        }
    }
    games.sort();
    Some(games)
}

/// Whether `directory` is a sound game root of `game` as the Runtime reads it (the contract's
/// [`read_standard_pack_root`] over its root files, and `tables/` making it one): a root that
/// is broken or whose root files cannot be read is not.
fn sound_game_root(directory: &Path, game: &str) -> bool {
    let read = |file: &str| match fs::read(directory.join(file)) {
        Ok(bytes) => Some(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(None),
        Err(_) => None,
    };
    // Workflow #392 DA-4: `display-layout.json` is checked as the Runtime checks it.
    let (Some(bundle), Some(pages), Some(interrupts), Some(navigation), Some(display_layout)) = (
        read(STANDARD_PACK_BUNDLE_FILE),
        read(STANDARD_PACK_PAGES_FILE),
        read(STANDARD_PACK_INTERRUPTS_FILE),
        read(STANDARD_PACK_NAVIGATION_FILE),
        read(STANDARD_PACK_DISPLAY_LAYOUT_FILE),
    ) else {
        return false;
    };
    let tables = fs::symlink_metadata(directory.join(STANDARD_PACK_TABLES_DIRECTORY)).is_ok();
    matches!(
        read_standard_pack_root(
            StandardPackRootBytes {
                bundle: bundle.as_deref(),
                pages: pages.as_deref(),
                interrupts: interrupts.as_deref(),
                navigation: navigation.as_deref(),
                tables,
                display_layout: display_layout.as_deref(),
            },
            game,
        ),
        Ok(Some(_))
    )
}

enum PathKind {
    Directory,
    File,
    Unreadable,
}

/// A directory whose entries can be listed, a file that opens, or neither.
fn path_kind(path: &Path) -> PathKind {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => match fs::read_dir(path) {
            Ok(_) => PathKind::Directory,
            Err(_) => PathKind::Unreadable,
        },
        Ok(_) => match fs::File::open(path) {
            Ok(_) => PathKind::File,
            Err(_) => PathKind::Unreadable,
        },
        Err(_) => PathKind::Unreadable,
    }
}

/// The `package_id` a package folder's control file names: one file read, no hash (M-PKG
/// §4.3). A package file (a container) is not opened, so it has none here.
fn startup_package_id(path: &Path) -> Option<String> {
    let file = fs::File::open(path.join(CONTROL_FILE)).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_CONTROL_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_CONTROL_BYTES {
        return None;
    }
    serde_json::from_slice::<ControlFile>(&bytes)
        .ok()
        .map(|control| control.package_id)
        .filter(|package_id| !package_id.is_empty())
}

/// Runs `packs` when the first argument names it; `None` leaves every other command to the
/// CLI. It takes no `--state-root`: it never connects to the Runtime (command table D5). One
/// result line on stdout (Workflow #378 A5): the bindings with `package_bindings_reported`, or
/// `{outcome}` with the refusal (a flag refusal's developer line adds the usage) and exit 1.
pub(crate) fn dispatch(arguments: &[OsString]) -> Option<ExitCode> {
    if arguments.first()?.to_str()? != COMMAND {
        return None;
    }
    Some(match run(&arguments[1..]) {
        Ok(document) => cli_line::success(
            document,
            &Outcome::new(ActingctlCode::PackageBindingsReported),
        ),
        Err(PacksError(outcome)) if outcome.is(ActingctlCode::ActingctlUsageInvalid) => {
            cli_line::failure(&outcome, Some(USAGE))
        }
        Err(PacksError(outcome)) => cli_line::failure(&outcome, None),
    })
}

#[derive(Default)]
struct Flags {
    root: Option<PathBuf>,
    instance: Option<String>,
    role: Option<Role>,
}

/// Each flag at most once and with its value; any other argument is a usage error naming the
/// flag (`argument`) and what is wrong with it (`argument_problem`).
fn parse(arguments: &[OsString]) -> Result<Flags, PacksError> {
    let mut flags = Flags::default();
    let mut index = 0;
    while index < arguments.len() {
        let flag = arguments[index]
            .to_str()
            .ok_or_else(|| PacksError::usage(&arguments[index], ArgumentProblem::NotUtf8))?;
        let given = match flag {
            "--root" => flags.root.is_some(),
            "--instance" => flags.instance.is_some(),
            "--role" => flags.role.is_some(),
            _ => return Err(PacksError::usage(flag, ArgumentProblem::Unknown)),
        };
        if given {
            return Err(PacksError::usage(flag, ArgumentProblem::Repeated));
        }
        let value = arguments
            .get(index + 1)
            .ok_or_else(|| PacksError::usage(flag, ArgumentProblem::ValueMissing))?;
        if flag == "--root" {
            flags.root = Some(PathBuf::from(value));
        } else {
            let text = value
                .to_str()
                .ok_or_else(|| PacksError::usage(flag, ArgumentProblem::NotUtf8))?;
            if text.trim().is_empty() {
                return Err(PacksError::usage(flag, ArgumentProblem::ValueMissing));
            }
            if flag == "--instance" {
                flags.instance = Some(text.to_owned());
            } else {
                flags.role = Some(
                    Role::parse(text)
                        .ok_or_else(|| PacksError::usage(flag, ArgumentProblem::NotRecognised))?,
                );
            }
        }
        index += 2;
    }
    Ok(flags)
}

/// The CLI's document: the bindings and their warnings, each `{outcome}` in this process's
/// detail. One that does not encode is `actingctl_output_failed`, caused by the encoder's
/// failure.
fn run(arguments: &[OsString]) -> Result<Map<String, Value>, PacksError> {
    let flags = parse(arguments)?;
    let (installation, config) = selected_config(flags.root.as_deref())?;
    let answer = read_with_tables(
        installation.as_ref(),
        &config,
        &Query {
            instance: flags.instance.as_deref(),
            role: flags.role,
        },
    )?;
    answer.document(cli_line::warning).map_err(|error| {
        PacksError(
            Outcome::new(ActingctlCode::ActingctlOutputFailed).caused_by(
                Outcome::new(ContractCode::ForeignDataError)
                    .raw_source(RawSource::Json)
                    .raw_text(error.to_string()),
            ),
        )
    })
}

/// The selected configuration generation, found as mcp-serve finds it (`mcp/runtime.rs`): this
/// process's install selection; else the install selection of `--root`; else
/// `<root>\actingd.config.json` of a root without one, `--root` or the install root this
/// executable runs from. An installed process reads its root's selection again here, from
/// `install\active.json` as it is now (`current_selection`), not the one it holds from its start:
/// `ac_packs` calls this at every call with the server's `--root`, so a long-running mcp-serve
/// answers the generation selected now, as a fresh `actingctl packs` does (train 4 review
/// ruling), also when the fixed entry started it with an inherited selection (Workflow #389
/// MCP-7 scope extension).
pub(crate) fn selected_config(
    root: Option<&Path>,
) -> Result<(Option<InstalledProcess>, PathBuf), PacksError> {
    // The install selection of this process cannot be read: the contract's code.
    let process = actingcommand_contract::process_installation()
        .map_err(|error| PacksError(Outcome::new(error.code())))?;
    if let Some(selected) = process {
        if let Some(given) = root {
            // `--root` that does not resolve, or is not the install root this actingctl runs
            // from (`install_root`).
            let root = fs::canonicalize(given).map_err(|error| {
                PacksError(
                    Outcome::new(ActingctlCode::InstallRootUnresolved)
                        .path(given)
                        .caused_by(Outcome::from_io_error(&error, IoOp::Canonicalize)),
                )
            })?;
            if root != selected.root() {
                return Err(PacksError(
                    Outcome::new(ActingctlCode::InstallProcessRootMismatch)
                        .path(root)
                        .install_root(selected.root()),
                ));
            }
        }
        let current = current_selection(selected.root())?;
        let config = current.config_path();
        return Ok((Some(current), config));
    }
    if let Some(root) = root {
        // The install selection of `--root` cannot be read: the contract's code.
        let selected =
            InstalledProcess::read(root).map_err(|error| PacksError(Outcome::new(error.code())))?;
        return Ok(match selected {
            Some(selected) => {
                let config = selected.config_path();
                (Some(selected), config)
            }
            None => (None, root.join(ROOT_CONFIG_FILE)),
        });
    }
    // actingctl does not run from an install root, and no `--root` was given.
    executable_install_root()
        .map(|root| (None, root.join(ROOT_CONFIG_FILE)))
        .ok_or_else(|| PacksError(Outcome::new(ActingctlCode::InstallRootUnresolved)))
}

/// The selection `install\active.json` of the install root `root` holds now (Workflow #389 MCP-7
/// scope extension). An installed process holds the selection of its start for its whole life:
/// the one the fixed entry `<root>\runtime\actingctl.exe` handed it (`InstalledProcess::read`
/// prefers that inherited snapshot), or the one its slot read when it started. A long-running
/// process (mcp-serve) reads this instead at every call, as a process started now through the
/// fixed entry would be handed it. A selection that cannot be read answers the contract's install
/// code, one the root no longer has `install_selection_unavailable`.
pub(crate) fn current_selection(root: &Path) -> Result<InstalledProcess, PacksError> {
    InstalledProcess::read_active(root)
        .map_err(|error| PacksError(Outcome::new(error.code())))?
        .ok_or_else(|| PacksError(Outcome::new(SharedCode::InstallSelectionUnavailable)))
}

/// Two levels above the running executable when `<root>\runtime\BUILD-MANIFEST.json` is a
/// file, as mcp-serve's install detection; `actingctl overview` finds its install the same way.
pub(crate) fn executable_install_root() -> Option<PathBuf> {
    let executable = fs::canonicalize(std::env::current_exe().ok()?).ok()?;
    let root = executable.parent()?.parent()?.to_path_buf();
    root.join("runtime")
        .join("BUILD-MANIFEST.json")
        .is_file()
        .then_some(root)
}

// Test-only: Workflow #389 MCP-11, the refusals of `packs` (command table §4.12, ruling Q6).
#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_contract::outcome::Category;
    use tempfile::TempDir;

    /// A configuration with one instance and no package binding.
    const ONE_INSTANCE: &str =
        r#"{"instances": [{"alias": "node-a", "instance_id": "neutral-a"}]}"#;

    #[track_caller]
    fn refused(answer: Result<Answer, PacksError>) -> PacksError {
        match answer {
            Ok(_) => panic!("packs answered where a refusal is required"),
            Err(error) => error,
        }
    }

    #[test]
    fn each_refusal_of_packs_carries_its_registered_code() {
        let root = TempDir::new().expect("tempdir");
        let config = root.path().join(ROOT_CONFIG_FILE);
        let everything = Query::default();

        // Ruling Q6: a configuration that cannot be read is actingd's config_read_failed,
        // one that does not decode its config_decode_failed, both shared through SharedCode.
        let missing = refused(read(None, &config, &everything)).outcome();
        assert!(missing.is(SharedCode::ConfigReadFailed), "{missing}");
        assert_eq!(missing.category(), Category::Error);

        fs::write(&config, b"not json").expect("write configuration");
        let malformed = refused(read(None, &config, &everything)).outcome();
        assert!(malformed.is(SharedCode::ConfigDecodeFailed), "{malformed}");
        assert_eq!(malformed.category(), Category::Error);

        // Command table §2.1: a selector that names no configured instance is instance_unknown.
        fs::write(&config, ONE_INSTANCE).expect("write configuration");
        let unknown = refused(read(
            None,
            &config,
            &Query {
                instance: Some("node-z"),
                role: None,
            },
        ))
        .outcome();
        assert!(
            unknown.is(actingcommand_contract::RuntimeErrorCode::InstanceUnknown.code()),
            "{unknown}"
        );
        assert_eq!(unknown.category(), Category::Error);
    }
}
