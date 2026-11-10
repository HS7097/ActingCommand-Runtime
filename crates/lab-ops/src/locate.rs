// SPDX-License-Identifier: AGPL-3.0-only

//! The default location (Workflow #434; `contracts/lab-two-entries.md` §9.3 and §7): an
//! installed Lab program finds its root from its own place and its state root from the selected
//! configuration, so no operation takes `--root` or `--state-root` (`new` names its new root).
//!
//! The program's place, as [`locate_from`] reads it:
//! - started through the selection of an A/B root (a slot's `<root>\<A|B>\runtime\<program>`,
//!   or a fixed entry that hands it the selection): the root of that selection, and the
//!   selection `install\active.json` holds now, as `packs` and mcp-serve read it;
//! - a kept build `<root>\install\builds\<commit>\runtime\<program>`, where the rescue module
//!   runs from (§7): `<root>`;
//! - `<root>\runtime\<program>` beside `<root>\runtime\BUILD-MANIFEST.json`, the programs of a
//!   single-location root: `<root>`.
//!
//! Workflow #434 LAB-3: the Tools member's folder `tools\` in place of `runtime\` reads the same
//! way (a kept build's `install\builds\<commit>\tools\<program>`, a single-location root's
//! `<root>\tools\<program>`), since `actinglab.exe` ships in the Tools member until LAB-7 moves it
//! into the Runtime member (`tools-layout` 5).
//!
//! The configuration is the selection's, or `<root>\actingd.config.json` of a root without one;
//! the state root is its absolute `state_root`. Every failure is a registered outcome: a place
//! that is no installed root's `install_root_unresolved` with the program as `path`, a selection
//! that cannot be read the contract's install code, a state root that cannot be told
//! `install_state_root_unresolved` with the configuration as `config_path` and what kept it
//! unknown.
//!
//! Workflow #434 LAB-5F (§8.3; the coordinator's ruling of 2026-11-10 on #434, point 15): the
//! same places say whether the authoring module is installed ([`authoring_module_from`]). Outside
//! any installed root there is no configuration to read and the module counts as installed;
//! inside one the selected configuration's `lab_module_authoring` decides, and a configuration
//! that cannot be read, or whose key is not a boolean, is a failure, never read as installed.

use crate::codes::ActingctlCode;
use crate::packs::ROOT_CONFIG_FILE;
use actingcommand_contract::codes::{InstallationCode, SharedCode};
use actingcommand_contract::outcome::{IoOp, Outcome};
use actingcommand_contract::{INSTALL_SELECTION_PATH, InstalledProcess};
use serde_json::Value;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

/// Workflow #434 LAB-5F: the configuration key that says the authoring module is installed
/// (§8.3).
pub const AUTHORING_KEY: &str = actingcommand_runtime_config::lab_module::AUTHORING;

/// The members' folder of a root, a slot or a kept build that holds the Runtime's programs.
const RUNTIME_DIRECTORY: &str = "runtime";
/// Workflow #434 LAB-3: the Tools member's folder, where `actinglab.exe` ships until LAB-7.
const TOOLS_DIRECTORY: &str = "tools";
/// The Runtime member's manifest in its folder; it marks a single-location root.
const BUILD_MANIFEST: &str = "BUILD-MANIFEST.json";
/// The installer's folder of a root, and its kept builds' folder inside it.
const INSTALL_DIRECTORY: &str = "install";
const BUILDS_DIRECTORY: &str = "builds";
/// The configuration file is read whole up to this size, the size actingd reads it up to.
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

/// Where an installed Lab program acts: its install root and its state root (§9.3).
#[derive(Debug, Clone)]
pub struct Location {
    /// The install root.
    pub root: PathBuf,
    /// The root's selection as it is now; `None` for a single-location root.
    pub installation: Option<InstalledProcess>,
    /// The selected configuration file.
    pub config: PathBuf,
    /// The absolute `state_root` of that configuration.
    pub state_root: PathBuf,
}

/// The location of this process's program: its install selection when it was started through
/// one, else its own place ([`locate_from`]).
pub fn locate() -> Result<Location, Outcome> {
    let (program, process) = this_program()?;
    locate_from(&program, process)
}

/// This process's program, its path canonical, and the install selection it was started
/// through.
fn this_program() -> Result<(PathBuf, Option<&'static InstalledProcess>), Outcome> {
    let process = actingcommand_contract::process_installation()
        .map_err(|error| Outcome::new(error.code()))?;
    let program = std::env::current_exe()
        .and_then(fs::canonicalize)
        .map_err(|error| {
            Outcome::new(InstallationCode::InstallExecutableUnavailable)
                .caused_by(Outcome::from_io_error(&error, IoOp::Canonicalize))
        })?;
    Ok((program, process))
}

/// The location of a program at `program` (its path as the operating system gives it),
/// started through the install selection `process` when that is given; see the module's
/// description for the places it reads.
pub fn locate_from(
    program: &Path,
    process: Option<&InstalledProcess>,
) -> Result<Location, Outcome> {
    let (root, installation) = installed_place(program, process)?
        .ok_or_else(|| Outcome::new(ActingctlCode::InstallRootUnresolved).path(program))?;
    let config = selected_config(&root, installation.as_ref());
    let state_root = configured_state_root(&config)?;
    Ok(Location {
        root,
        installation,
        config,
        state_root,
    })
}

/// The installed root a program at `program` acts in, with the root's selection as it is now
/// (`None` for a single-location root): the root of `process` when it was started through one,
/// else the root of its own place ([`installed_root`]); `None` outside any installed root.
fn installed_place(
    program: &Path,
    process: Option<&InstalledProcess>,
) -> Result<Option<(PathBuf, Option<InstalledProcess>)>, Outcome> {
    match process {
        Some(selected) => {
            let current = crate::packs::current_selection(selected.root())
                .map_err(|error| error.outcome())?;
            Ok(Some((selected.root().to_path_buf(), Some(current))))
        }
        None => {
            let Some(root) = installed_root(program) else {
                return Ok(None);
            };
            let installation =
                InstalledProcess::read_active(&root).map_err(|error| Outcome::new(error.code()))?;
            Ok(Some((root, installation)))
        }
    }
}

/// The selected configuration of `root`: its selection's, or `<root>\actingd.config.json` of a
/// root without one.
fn selected_config(root: &Path, installation: Option<&InstalledProcess>) -> PathBuf {
    installation
        .map(InstalledProcess::config_path)
        .unwrap_or_else(|| root.join(ROOT_CONFIG_FILE))
}

/// Workflow #434 LAB-5F: what the place a Lab program runs from says of the authoring module
/// (`contracts/lab-two-entries.md` §8.3; the coordinator's ruling of 2026-11-10 on #434, point
/// 15).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthoringModule {
    /// The program runs outside any installed root: there is no configuration to read, and the
    /// module counts as installed (an unpacked Tools package, as the resource repositories' CI
    /// and scripts and the deployment tools run it).
    Unrooted,
    /// The program runs inside an installed root whose selected configuration `config` says
    /// `installed` as [`AUTHORING_KEY`] (absent or `null`: `false`, as actingd reads it).
    Configured { config: PathBuf, installed: bool },
}

impl AuthoringModule {
    /// Whether the authoring module is installed.
    pub fn installed(&self) -> bool {
        match self {
            Self::Unrooted => true,
            Self::Configured { installed, .. } => *installed,
        }
    }
}

/// Workflow #434 LAB-5F: what the place of this process's program says of the authoring module
/// ([`authoring_module_from`]).
pub fn authoring_module() -> Result<AuthoringModule, Outcome> {
    let (program, process) = this_program()?;
    authoring_module_from(&program, process)
}

/// Workflow #434 LAB-5F: what the place of a program at `program`, started through the install
/// selection `process` when that is given, says of the authoring module: outside any installed
/// root (the places [`locate_from`] reads) [`AuthoringModule::Unrooted`], else the
/// [`AUTHORING_KEY`] of the selected configuration, read as it is now. A selection that cannot be
/// read is the contract's install code; a configuration that cannot be read (or whose bytes are
/// not those its selection names) is `config_read_failed`, one that is no JSON object or whose
/// key is not a boolean `config_decode_failed`, each with the configuration as `path` and the
/// failure as its cause. A failure is never read as installed, nor as an absent key.
pub fn authoring_module_from(
    program: &Path,
    process: Option<&InstalledProcess>,
) -> Result<AuthoringModule, Outcome> {
    let Some((root, installation)) = installed_place(program, process)? else {
        return Ok(AuthoringModule::Unrooted);
    };
    let config = selected_config(&root, installation.as_ref());
    let bytes = match &installation {
        Some(selected) => selected
            .config_bytes()
            .map_err(|error| Outcome::new(error.code())),
        None => crate::overview::read_bounded(&config, MAX_CONFIG_BYTES),
    }
    .map_err(|cause| {
        Outcome::new(SharedCode::ConfigReadFailed)
            .path(&config)
            .caused_by(cause)
    })?;
    let malformed = |text: &str| {
        Outcome::new(SharedCode::ConfigDecodeFailed)
            .path(&config)
            .caused_by(crate::foreign_data_error(&text).path(&config))
    };
    let document = serde_json::from_slice::<Value>(&bytes)
        .map_err(|error| malformed(error.to_string().as_str()))?;
    let Value::Object(members) = document else {
        return Err(malformed("the configuration is not a JSON object"));
    };
    let installed = match members.get(AUTHORING_KEY) {
        None | Some(Value::Null) => false,
        Some(Value::Bool(installed)) => *installed,
        Some(_) => {
            return Err(malformed(
                format!("{AUTHORING_KEY} is not a boolean").as_str(),
            ));
        }
    };
    Ok(AuthoringModule::Configured { config, installed })
}

/// The install root a program at `program` belongs to by its place alone: a kept build's root,
/// or a single-location root; `None` for any other place.
fn installed_root(program: &Path) -> Option<PathBuf> {
    let directory = program.parent()?;
    if directory.file_name() != Some(OsStr::new(RUNTIME_DIRECTORY))
        && directory.file_name() != Some(OsStr::new(TOOLS_DIRECTORY))
    {
        return None;
    }
    let above = directory.parent()?;
    if let Some(builds) = above.parent()
        && builds.file_name() == Some(OsStr::new(BUILDS_DIRECTORY))
        && let Some(install) = builds.parent()
        && install.file_name() == Some(OsStr::new(INSTALL_DIRECTORY))
    {
        let root = install.parent()?;
        let installed = root.join(RUNTIME_DIRECTORY).join(BUILD_MANIFEST).is_file()
            || root.join(INSTALL_SELECTION_PATH).is_file();
        return installed.then(|| root.to_path_buf());
    }
    above
        .join(RUNTIME_DIRECTORY)
        .join(BUILD_MANIFEST)
        .is_file()
        .then(|| above.to_path_buf())
}

/// The absolute `state_root` of the configuration file `config`, else
/// `install_state_root_unresolved` with `config_path`: caused by the read or decode failure,
/// alone without the member, or with the value as `path` when it is not absolute.
pub fn configured_state_root(config: &Path) -> Result<PathBuf, Outcome> {
    let unresolved = || Outcome::new(ActingctlCode::InstallStateRootUnresolved).config_path(config);
    let bytes = crate::overview::read_bounded(config, MAX_CONFIG_BYTES)
        .map_err(|cause| unresolved().caused_by(cause))?;
    let document = serde_json::from_slice::<Value>(&bytes)
        .map_err(|error| unresolved().caused_by(crate::foreign_data_error(&error).path(config)))?;
    let state_root = document
        .get("state_root")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(unresolved)?;
    if !state_root.is_absolute() {
        return Err(unresolved().path(&state_root));
    }
    Ok(state_root)
}
