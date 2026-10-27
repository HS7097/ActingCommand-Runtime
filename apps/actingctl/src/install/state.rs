// SPDX-License-Identifier: AGPL-3.0-only

//! The installer's own files under `<root>\install\` (Workflow #421 INST-1,
//! `contracts/install-cli.md`, "Layout" and "Documents"): the paths, `state.json`
//! (`actingcommand.install-state.v1`) and `current.json`, the one unfinished operation.

use super::files;
use crate::codes::InstallLocation;
use actingcommand_contract::outcome::{IoOp, Outcome};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub(crate) const STATE_SCHEMA: &str = "actingcommand.install-state.v1";
/// The configuration of a single-location root.
pub(crate) const CONFIG_FILE: &str = "actingd.config.json";
/// acsetup's A/B selection; a root that holds it is not this installer's.
pub(crate) const SELECTION_FILE: &str = "active.json";

/// The paths of one root. `root` is canonical; `plain` is its ordinary spelling for output.
#[derive(Debug, Clone)]
pub(crate) struct Paths {
    pub(crate) root: PathBuf,
    pub(crate) plain: String,
}

impl Paths {
    /// The root as `--root` names it, resolved; a root that does not exist is refused with the
    /// OS's failure.
    pub(crate) fn resolve(argument: &Path) -> Result<Self, Outcome> {
        let root = std::fs::canonicalize(argument)
            .map_err(|error| files::os_failure(argument, &error, IoOp::Canonicalize))?;
        Ok(Self {
            plain: files::plain(&root),
            root,
        })
    }

    pub(crate) fn install(&self) -> PathBuf {
        self.root.join("install")
    }

    pub(crate) fn state(&self) -> PathBuf {
        self.install().join("state.json")
    }

    pub(crate) fn current(&self) -> PathBuf {
        self.install().join("current.json")
    }

    pub(crate) fn operations(&self) -> PathBuf {
        self.install().join("operations")
    }

    pub(crate) fn journal(&self, operation_id: &str) -> PathBuf {
        self.operations().join(format!("{operation_id}.json"))
    }

    pub(crate) fn builds(&self) -> PathBuf {
        self.install().join("builds")
    }

    pub(crate) fn build(&self, commit: &str) -> PathBuf {
        self.builds().join(commit)
    }

    pub(crate) fn selection(&self) -> PathBuf {
        self.install().join(SELECTION_FILE)
    }

    pub(crate) fn config(&self) -> PathBuf {
        self.root.join(CONFIG_FILE)
    }

    /// A path below the root named by a relative path with `/` separators.
    pub(crate) fn at(&self, relative: &str) -> PathBuf {
        files::join_relative(&self.root, relative)
    }

    /// Whether the root is a single-location root: `runtime\BUILD-MANIFEST.json` is a file.
    pub(crate) fn is_single_location(&self) -> bool {
        self.root
            .join("runtime")
            .join(super::manifest::MANIFEST_FILE)
            .is_file()
    }
}

/// `install\state.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InstallState {
    pub(crate) schema_version: String,
    /// The live build; absent until `adopt` or the first update.
    pub(crate) live: Option<LiveBuild>,
    pub(crate) builds: Vec<KeptBuild>,
    pub(crate) packs: Vec<PackEntry>,
    pub(crate) config_sha256: Option<String>,
}

impl Default for InstallState {
    fn default() -> Self {
        Self {
            schema_version: STATE_SCHEMA.to_owned(),
            live: None,
            builds: Vec::new(),
            packs: Vec::new(),
            config_sha256: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LiveBuild {
    pub(crate) commit: String,
    /// The operation that installed it; `None` after `adopt`.
    pub(crate) operation_id: Option<String>,
}

/// A build kept under `install\builds\<commit>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct KeptBuild {
    pub(crate) commit: String,
    pub(crate) tag: Option<String>,
    pub(crate) origin: BuildOrigin,
    pub(crate) operation_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BuildOrigin {
    Release,
    LocalBuild,
    Adopted,
}

/// One placed game root of the pack register. No digest (option 丙).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PackEntry {
    pub(crate) game: String,
    pub(crate) repository: Option<String>,
    pub(crate) commit: Option<String>,
    pub(crate) tag: Option<String>,
    /// The declared `package` range; `None` when unknown (an adopted root keeps no
    /// `interfaces.json`).
    pub(crate) package_range: Option<[u64; 2]>,
}

impl InstallState {
    /// The kept build of `commit`, if `state.json` lists it.
    pub(crate) fn kept(&self, commit: &str) -> Option<&KeptBuild> {
        self.builds.iter().find(|build| build.commit == commit)
    }

    /// This state with `build` listed (replacing an entry of the same commit).
    pub(crate) fn with_build(mut self, build: KeptBuild) -> Self {
        match self
            .builds
            .iter_mut()
            .find(|kept| kept.commit == build.commit)
        {
            Some(kept) => *kept = build,
            None => self.builds.push(build),
        }
        self
    }
}

/// Why the root's install state cannot be used.
pub(crate) enum StateProblem {
    /// No `state.json`.
    Missing,
    /// Unreadable, not JSON, or not this build's schema: the cause.
    Invalid(Outcome),
}

/// Reads `state.json`.
pub(crate) fn load_state(paths: &Paths) -> Result<InstallState, StateProblem> {
    let path = paths.state();
    if !path.exists() {
        return Err(StateProblem::Missing);
    }
    let state: InstallState = files::read_json(&path).map_err(StateProblem::Invalid)?;
    if state.schema_version != STATE_SCHEMA {
        return Err(StateProblem::Invalid(files::json_schema_failure(
            &path,
            &state.schema_version,
        )));
    }
    Ok(state)
}

/// `state.json` when there is one; a fresh state otherwise.
pub(crate) fn load_state_or_default(paths: &Paths) -> Result<InstallState, Outcome> {
    match load_state(paths) {
        Ok(state) => Ok(state),
        Err(StateProblem::Missing) => Ok(InstallState::default()),
        Err(StateProblem::Invalid(cause)) => Err(cause),
    }
}

/// Writes `state.json` atomically.
pub(crate) fn write_state(paths: &Paths, state: &InstallState) -> Result<(), Outcome> {
    files::create_dir_all(&paths.install())?;
    files::write_json_atomic(&paths.state(), state)
}

/// The kinds of operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OperationKind {
    ReleaseStage,
    Adopt,
    Update,
    Rollback,
}

impl OperationKind {
    /// The command that opens an operation of this kind: the `operation` of its outcomes.
    pub(crate) fn location(self) -> InstallLocation {
        match self {
            Self::ReleaseStage => InstallLocation::ReleaseStage,
            Self::Adopt => InstallLocation::Adopt,
            Self::Update => InstallLocation::UpdateApply,
            Self::Rollback => InstallLocation::RollbackApply,
        }
    }
}

/// `install\current.json`: the one unfinished operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Current {
    pub(crate) operation_id: String,
    pub(crate) kind: OperationKind,
    pub(crate) started_at_unix_ms: u64,
}

/// What `current.json` says.
pub(crate) enum CurrentState {
    /// No unfinished operation.
    None,
    /// The unfinished operation.
    Unfinished(Current),
    /// A `current.json` that cannot be read or is not this shape.
    Unreadable(Outcome),
}

/// Workflow #421 INST-1, watchdog row 7a″: `current.json` of `root` as the watchdog and
/// `actingctl start` read it. `None` when there is none; otherwise the unfinished operation's id
/// when the file can be read and names one, `Some(None)` when it cannot.
pub(crate) fn unfinished_operation(root: &Path) -> Option<Option<String>> {
    let path = root.join("install").join("current.json");
    if !path.exists() {
        return None;
    }
    Some(
        files::read_json::<Current>(&path)
            .ok()
            .map(|current| current.operation_id)
            .filter(|operation_id| super::journal::operation_ms(operation_id).is_some()),
    )
}

/// Reads `current.json`.
pub(crate) fn read_current(paths: &Paths) -> CurrentState {
    let path = paths.current();
    if !path.exists() {
        return CurrentState::None;
    }
    match files::read_json::<Current>(&path) {
        Ok(current) => CurrentState::Unfinished(current),
        Err(cause) => CurrentState::Unreadable(cause),
    }
}
