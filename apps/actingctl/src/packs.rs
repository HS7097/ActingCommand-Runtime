// SPDX-License-Identifier: AGPL-3.0-only

//! `actingctl packs` (Workflow #389 MCP-11, command table v1.1 §4.12): the packages the selected
//! configuration generation binds, read offline from that `actingd.config.json` (ruling Q2). It
//! sends no Runtime request and writes no ledger record; `source.kind` says the answer comes from
//! the selected generation, which a running actingd may not have loaded yet. Each row carries the
//! package id, its path and its role only (option 丙: no package digest). `ac_packs`
//! (`mcp/packs.rs`) answers the same document through [`read`].

use crate::codes::ActingctlCode;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Code;
use actingcommand_contract::{InstalledProcess, RuntimeErrorCode};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const COMMAND: &str = "packs";
const USAGE: &str = "usage: actingctl packs [--root <install root>] [--instance <alias|instance_id|adb port>] [--role procedure|prerequisite|return_home|startup|resource_package]";
/// The configuration of an install root without slots, as mcp-serve finds it.
const ROOT_CONFIG_FILE: &str = "actingd.config.json";
/// The file of a package that names its `package_id`.
const CONTROL_FILE: &str = "control.json";
/// A procedure binding's `procedure_ref` is this prefix and the package id (M-PKG §4.4).
const PROCEDURE_PACKAGE_PREFIX: &str = "package.";
/// A package's control file is read whole up to this size.
const MAX_CONTROL_BYTES: u64 = 1024 * 1024;

/// Where a configuration binds a package (command table §4.12, roles). The recognition
/// maintenance binding comes with D2-S2.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Role {
    /// `policy.procedure_manifest[]`: installation level.
    Procedure,
    /// `prerequisite_packages[]`: installation level.
    Prerequisite,
    /// `return_home_packages[]`, through the prerequisite package it names: installation level.
    ReturnHome,
    /// `instances[].startup_package`: per instance.
    Startup,
    /// `instances[].resource_package`: per instance, path only.
    ResourcePackage,
}

#[cfg_attr(not(feature = "mcp"), allow(dead_code))]
impl Role {
    pub(crate) const ALL: [Self; 5] = [
        Self::Procedure,
        Self::Prerequisite,
        Self::ReturnHome,
        Self::Startup,
        Self::ResourcePackage,
    ];

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

/// A warning in the shape every MCP warning has today, `{code, message, blocked_by, details}`
/// (A5 replaces it with `{outcome}`).
#[derive(Serialize)]
pub(crate) struct Warning {
    #[serde(serialize_with = "spelling")]
    code: Code,
    message: String,
    blocked_by: Option<String>,
    details: WarningDetails,
}

#[derive(Serialize, PartialEq, Eq)]
struct WarningDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    alias: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    package_id: Option<String>,
}

fn spelling<S: Serializer>(code: &Code, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(code.as_str())
}

pub(crate) struct Answer {
    packs: Packs,
    warnings: Vec<Warning>,
}

#[cfg_attr(not(feature = "mcp"), allow(dead_code))]
impl Answer {
    pub(crate) const fn packs(&self) -> &Packs {
        &self.packs
    }

    pub(crate) fn warnings(&self) -> &[Warning] {
        &self.warnings
    }

    /// What the CLI prints, and what `ac_packs` exports when its answer is over the output
    /// budget: the answer with its warnings as a top-level member, present only when there are
    /// some (command table D3).
    pub(crate) fn document(&self) -> Document<'_> {
        Document {
            packs: &self.packs,
            warnings: &self.warnings,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct Document<'a> {
    #[serde(flatten)]
    packs: &'a Packs,
    #[serde(skip_serializing_if = "no_warnings")]
    warnings: &'a [Warning],
}

fn no_warnings(warnings: &&[Warning]) -> bool {
    warnings.is_empty()
}

/// A refusal of `packs`: its registered code, the developer text and, for a configuration that
/// cannot be read, the configuration's path and the install selection's own code beneath it.
#[derive(Debug)]
pub(crate) struct PacksError {
    code: Code,
    message: String,
    path: Option<String>,
    cause: Option<Code>,
}

#[cfg_attr(not(feature = "mcp"), allow(dead_code))]
impl PacksError {
    fn new(code: impl Into<Code>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            path: None,
            cause: None,
        }
    }

    fn usage() -> Self {
        Self::new(ActingctlCode::ActingctlUsageInvalid, USAGE)
    }

    pub(crate) const fn code(&self) -> Code {
        self.code
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }

    pub(crate) fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    pub(crate) const fn cause(&self) -> Option<Code> {
        self.cause
    }
}

impl fmt::Display for PacksError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)?;
        match self.cause {
            Some(cause) => write!(formatter, " ({cause})"),
            None => Ok(()),
        }
    }
}

/// The parts of `actingd.config.json` this command reads; every other member is left unread.
#[derive(Deserialize)]
struct ConfigFile {
    instances: Vec<InstanceEntry>,
    #[serde(default)]
    policy: Option<PolicySection>,
    #[serde(default)]
    prerequisite_packages: Option<Vec<PrerequisiteEntry>>,
    #[serde(default)]
    return_home_packages: Option<Vec<ReturnHomeEntry>>,
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

#[derive(Deserialize)]
struct PrerequisiteEntry {
    package_id: String,
    package_path: PathBuf,
}

#[derive(Deserialize)]
struct ReturnHomeEntry {
    package_id: String,
}

/// A package's control file, for its `package_id`.
#[derive(Deserialize)]
struct ControlFile {
    package_id: String,
}

/// Reads the selected configuration `config` (through `installation`'s checked read when there
/// is an install selection, as actingd does) and lists its package bindings. A configuration
/// that cannot be read is `config_read_failed`, one that does not decode `config_decode_failed`
/// (ruling Q6).
pub(crate) fn read(
    installation: Option<&InstalledProcess>,
    config: &Path,
    query: &Query<'_>,
) -> Result<Answer, PacksError> {
    let unreadable = |path: &Path, cause: Option<Code>| {
        let mut error = PacksError::new(
            SharedCode::ConfigReadFailed,
            format!("cannot read the selected configuration {}", path.display()),
        );
        error.path = Some(path.display().to_string());
        error.cause = cause;
        error
    };
    let config = std::path::absolute(config).map_err(|_| unreadable(config, None))?;
    let bytes = match installation {
        Some(selected) => selected
            .config_bytes()
            .map_err(|error| unreadable(&config, Some(error.code())))?,
        None => fs::read(&config).map_err(|_| unreadable(&config, None))?,
    };
    let file = serde_json::from_slice::<ConfigFile>(&bytes).map_err(|error| {
        let mut refused = PacksError::new(
            SharedCode::ConfigDecodeFailed,
            format!(
                "the selected configuration {} does not decode: {error}",
                config.display()
            ),
        );
        refused.path = Some(config.display().to_string());
        refused
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
    Ok(Answer {
        packs: Packs {
            source: Source {
                kind: SourceKind::SelectedConfig,
                config: config.display().to_string(),
            },
            instances,
            installation,
        },
        warnings: rows.warnings,
    })
}

/// The configured instance a selector names by alias, instance_id or ADB port.
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
    let selected = matching.next().ok_or_else(|| {
        PacksError::new(
            RuntimeErrorCode::InstanceUnknown.code(),
            format!("no configured instance has the alias, instance_id or ADB port {selector}"),
        )
    })?;
    if matching.next().is_some() {
        return Err(PacksError::new(
            ActingctlCode::InstanceAmbiguous,
            format!(
                "{selector} names more than one configured instance; pass its alias or instance_id"
            ),
        ));
    }
    Ok(selected)
}

struct Rows {
    /// The configuration's folder: relative paths resolve against it, as actingd's do.
    base: PathBuf,
    role: Option<Role>,
    warnings: Vec<Warning>,
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

    fn warn(&mut self, code: Code, message: String, details: WarningDetails) {
        if !self
            .warnings
            .iter()
            .any(|warning| warning.code == code && warning.details == details)
        {
            self.warnings.push(Warning {
                code,
                message,
                blocked_by: None,
                details,
            });
        }
    }

    /// Records a bound path that does not exist or cannot be read.
    fn unreadable(&mut self, path: &Path, alias: Option<&str>) {
        let message = match alias {
            Some(alias) => format!(
                "instance {alias} binds {}, which does not exist or cannot be read",
                path.display()
            ),
            None => format!(
                "the configuration binds {}, which does not exist or cannot be read",
                path.display()
            ),
        };
        self.warn(
            ActingctlCode::PackageBindingUnreadable.code(),
            message,
            WarningDetails {
                path: Some(path.display().to_string()),
                alias: alias.map(str::to_owned),
                package_id: None,
            },
        );
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
        let prerequisites = file.prerequisite_packages.as_deref().unwrap_or_default();
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
        if self.wants(Role::Prerequisite) {
            for entry in prerequisites {
                let (path, readable) = self.installation_path(&entry.package_path);
                rows.push(InstallationRow {
                    role: Role::Prerequisite,
                    package_id: Some(entry.package_id.clone()),
                    path: Some(path),
                    readable: Some(readable),
                    procedure_ref: None,
                    operation_id: None,
                    scheduled: false,
                });
            }
        }
        if self.wants(Role::ReturnHome) {
            for entry in file.return_home_packages.as_deref().unwrap_or_default() {
                let bound = match prerequisites
                    .iter()
                    .find(|prerequisite| prerequisite.package_id == entry.package_id)
                {
                    Some(prerequisite) => Some(self.installation_path(&prerequisite.package_path)),
                    None => {
                        self.warn(
                            SharedCode::ReturnHomePackageUnbound.code(),
                            format!(
                                "return_home_packages names {}, which prerequisite_packages does not bind",
                                entry.package_id
                            ),
                            WarningDetails {
                                path: None,
                                alias: None,
                                package_id: Some(entry.package_id.clone()),
                            },
                        );
                        None
                    }
                };
                rows.push(InstallationRow {
                    role: Role::ReturnHome,
                    package_id: Some(entry.package_id.clone()),
                    readable: bound.as_ref().map(|(_, readable)| *readable),
                    path: bound.map(|(path, _)| path),
                    procedure_ref: None,
                    operation_id: None,
                    scheduled: false,
                });
            }
        }
        rows
    }
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
/// CLI. It takes no `--state-root`: it never connects to the Runtime (command table D5).
pub(crate) fn dispatch(arguments: &[OsString]) -> Option<ExitCode> {
    if arguments.first()?.to_str()? != COMMAND {
        return None;
    }
    Some(match run(&arguments[1..]) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("FATAL actingctl: {error}");
            ExitCode::FAILURE
        }
    })
}

#[derive(Default)]
struct Flags {
    root: Option<PathBuf>,
    instance: Option<String>,
    role: Option<Role>,
}

/// Each flag at most once and with its value; any other argument is a usage error.
fn parse(arguments: &[OsString]) -> Result<Flags, PacksError> {
    let mut flags = Flags::default();
    let mut index = 0;
    while index < arguments.len() {
        let flag = arguments[index].to_str().ok_or_else(PacksError::usage)?;
        let value = arguments.get(index + 1).ok_or_else(PacksError::usage)?;
        match flag {
            "--root" if flags.root.is_none() => flags.root = Some(PathBuf::from(value)),
            "--instance" if flags.instance.is_none() => {
                let selector = value
                    .to_str()
                    .filter(|selector| !selector.trim().is_empty())
                    .ok_or_else(PacksError::usage)?;
                flags.instance = Some(selector.to_owned());
            }
            "--role" if flags.role.is_none() => {
                flags.role = Some(
                    value
                        .to_str()
                        .and_then(Role::parse)
                        .ok_or_else(PacksError::usage)?,
                );
            }
            _ => return Err(PacksError::usage()),
        }
        index += 2;
    }
    Ok(flags)
}

fn run(arguments: &[OsString]) -> Result<(), PacksError> {
    let flags = parse(arguments)?;
    let (installation, config) = selected_config(flags.root.as_deref())?;
    let answer = read(
        installation.as_ref(),
        &config,
        &Query {
            instance: flags.instance.as_deref(),
            role: flags.role,
        },
    )?;
    let output_failed = |error: &dyn fmt::Display| {
        PacksError::new(
            ActingctlCode::ActingctlOutputFailed,
            format!("failed to write JSON output: {error}"),
        )
    };
    let stdout = io::stdout();
    let mut writer = stdout.lock();
    serde_json::to_writer(&mut writer, &answer.document())
        .map_err(|error| output_failed(&error))?;
    writer
        .write_all(b"\n")
        .map_err(|error| output_failed(&error))
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
    let process = actingcommand_contract::process_installation().map_err(|error| {
        PacksError::new(
            error.code(),
            "the install selection of this process cannot be read",
        )
    })?;
    if let Some(selected) = process {
        if let Some(root) = root {
            let root = fs::canonicalize(root).map_err(|error| {
                PacksError::new(
                    ActingctlCode::InstallRootUnresolved,
                    format!("--root {}: {error}", root.display()),
                )
            })?;
            if root != selected.root() {
                return Err(PacksError::new(
                    ActingctlCode::InstallProcessRootMismatch,
                    format!(
                        "--root {} is not the install root {} this actingctl runs from",
                        root.display(),
                        selected.root().display()
                    ),
                ));
            }
        }
        let current = current_selection(selected.root())?;
        let config = current.config_path();
        return Ok((Some(current), config));
    }
    if let Some(root) = root {
        let selected = InstalledProcess::read(root).map_err(|error| {
            PacksError::new(
                error.code(),
                format!("the install selection of {} cannot be read", root.display()),
            )
        })?;
        return Ok(match selected {
            Some(selected) => {
                let config = selected.config_path();
                (Some(selected), config)
            }
            None => (None, root.join(ROOT_CONFIG_FILE)),
        });
    }
    executable_install_root()
        .map(|root| (None, root.join(ROOT_CONFIG_FILE)))
        .ok_or_else(|| {
            PacksError::new(
                ActingctlCode::InstallRootUnresolved,
                "actingctl does not run from an install root; pass --root <install root>",
            )
        })
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
        .map_err(|error| {
            PacksError::new(
                error.code(),
                format!("the install selection of {} cannot be read", root.display()),
            )
        })?
        .ok_or_else(|| {
            PacksError::new(
                SharedCode::InstallSelectionUnavailable,
                format!(
                    "the install root {} no longer has a selection",
                    root.display()
                ),
            )
        })
}

/// Two levels above the running executable when `<root>\runtime\BUILD-MANIFEST.json` is a
/// file, as mcp-serve's install detection.
fn executable_install_root() -> Option<PathBuf> {
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
        let missing = refused(read(None, &config, &everything));
        assert_eq!(missing.code(), SharedCode::ConfigReadFailed.code());
        assert_eq!(missing.code().category(), Category::Error);

        fs::write(&config, b"not json").expect("write configuration");
        let malformed = refused(read(None, &config, &everything));
        assert_eq!(malformed.code(), SharedCode::ConfigDecodeFailed.code());
        assert_eq!(malformed.code().category(), Category::Error);

        // Command table §2.1: a selector that names no configured instance is instance_unknown.
        fs::write(&config, ONE_INSTANCE).expect("write configuration");
        let unknown = refused(read(
            None,
            &config,
            &Query {
                instance: Some("node-z"),
                role: None,
            },
        ));
        assert_eq!(unknown.code(), RuntimeErrorCode::InstanceUnknown.code());
        assert_eq!(unknown.code().category(), Category::Error);
    }
}
