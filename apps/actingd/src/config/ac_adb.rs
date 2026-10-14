// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #337 R2: the adb an AC install ships in `<install root>\tools\platform-tools\`.
//!
//! The install root is recognised from the running executable alone: `R` is two levels
//! above the canonical `current_exe`, and `R` is an install root only when
//! `R\runtime\BUILD-MANIFEST.json` is a file, the evidence acsetup itself takes for "installed".
//! acsetup runs nothing from its staging directory: its `check-config` runs with the actingd of
//! the slot it prepares. Anything else (a development build, H3, an older acsetup's zip-named
//! staging) is no install root and keeps the earlier behaviour.
//! Workflow #359: a program in an A/B slot (`<root>\<A|B>\runtime`, holding its slot lock)
//! takes `tools\` from the install root `<root>` that lock names, not from its slot.
//!
//! An instance uses this adb when the daemon runs from an install root and the instance's
//! `adb_path` is absent or names that same file. `require` then hashes the distributed
//! binaries against the pin this build carries before the ledger opens and refuses with
//! `adb_install_missing` or `adb_install_mismatch`; there is no fallback to another adb.

use crate::codes::ActingdCode;
use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::{Code, FileState, IoOp, Outcome, RawSource, Record};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

/// The pin the Windows exact-SHA build checks the shipped platform-tools files against. Its
/// `distributed_files` are the only source of the expected SHA-256 values here.
const TOOL_SOURCES_PIN: &str =
    include_str!("../../../../scripts/windows-tools/windows-tool-sources.v1.json");
/// The pin path of the adb itself, relative to `<install root>\tools`; the pin must list it.
const PINNED_ADB: &str = "platform-tools/adb.exe";
/// What to do about a refused install adb; the end of every refusal.
const REMEDY: &str = "fix: reinstall tools\\platform-tools under this install root (acsetup \
v0.10 or later for an acsetup install), or set the instance's adb_path to another adb (MuMu's \
own adb as before; an explicit instance may name any other adb)";

/// The adb of the install root the daemon runs from; the file may be missing.
#[derive(Clone)]
pub(super) struct InstalledAdb {
    /// The canonical install root holding `tools\`: `R`, or a slot program's A/B root.
    root: PathBuf,
    /// `<root>\tools\platform-tools\adb.exe`.
    adb: PathBuf,
}

impl InstalledAdb {
    /// The install root of the running executable, or `None` when it runs from anywhere
    /// else. Resolved once per assembly; an executable path that cannot be resolved is no
    /// install root either, exactly as a missing `runtime\BUILD-MANIFEST.json`.
    pub(super) fn detect() -> Option<Self> {
        let executable = fs::canonicalize(std::env::current_exe().ok()?).ok()?;
        let root = executable.parent()?.parent()?.to_path_buf();
        if !root.join("runtime").join("BUILD-MANIFEST.json").is_file() {
            return None;
        }
        // A lock error has already stopped startup: `process_installation_for` takes this
        // same cached slot lock first, for running and candidate checks alike.
        let root = match actingcommand_contract::process_slot_lock() {
            Ok(Some(lock)) => lock.root().to_path_buf(),
            Ok(None) | Err(_) => root,
        };
        let adb = root.join("tools").join("platform-tools").join("adb.exe");
        Some(Self { root, adb })
    }

    /// The adb path an instance that uses this adb registers.
    pub(super) fn path_string(&self) -> String {
        self.adb.to_string_lossy().into_owned()
    }

    /// Whether a declared `adb_path` names this adb, whether or not the file exists: only an
    /// absolute path can. It is canonicalized; when it does not exist, its nearest existing
    /// ancestor is canonicalized and the remaining components are joined back. The result is
    /// compared with this adb ignoring case.
    fn is_named_by(&self, declared: &str) -> bool {
        let declared = Path::new(declared);
        if !declared.is_absolute() {
            return false;
        }
        std::path::absolute(declared)
            .ok()
            .and_then(|absolute| canonical_through_existing_ancestor(&absolute))
            .is_some_and(|resolved| {
                resolved.to_string_lossy().to_lowercase()
                    == self.adb.to_string_lossy().to_lowercase()
            })
    }

    /// Every distributed binary of the pin against the installed file, in pin order.
    fn verify(&self) -> Vec<Finding> {
        let pinned = match pinned_binaries() {
            Ok(pinned) => pinned,
            Err(unreadable) => return vec![Finding::PinUnreadable(unreadable)],
        };
        pinned
            .into_iter()
            .filter_map(|file| {
                let path = file
                    .path
                    .split('/')
                    .fold(self.root.join("tools"), |path, part| path.join(part));
                match fs::read(&path) {
                    // Workflow #378 A6a: the OS error is the finding's `foreign_os_error` link.
                    Err(error) => Some(Finding::Missing {
                        state: if error.kind() == std::io::ErrorKind::NotFound {
                            FileState::Missing
                        } else {
                            FileState::Unreadable
                        },
                        cause: Outcome::from_io_error(&error, IoOp::Read).path(&path),
                        path,
                    }),
                    Ok(bytes) => {
                        let actual = format!("{:x}", Sha256::digest(bytes));
                        (actual != file.sha256).then_some(Finding::Mismatch {
                            path,
                            expected: file.sha256,
                            actual,
                        })
                    }
                }
            })
            .collect()
    }
}

/// The nearest existing ancestor's canonical path with the rest of `path` joined back.
fn canonical_through_existing_ancestor(path: &Path) -> Option<PathBuf> {
    path.ancestors().find_map(|ancestor| {
        let canonical = fs::canonicalize(ancestor).ok()?;
        let rest = path.strip_prefix(ancestor).ok()?;
        Some(if rest.as_os_str().is_empty() {
            canonical
        } else {
            canonical.join(rest)
        })
    })
}

/// How one instance's adb was resolved at assembly.
#[derive(Clone, Default)]
pub(super) struct InstanceAdb {
    /// The install root's adb, when the daemon runs from an install root.
    installed: Option<InstalledAdb>,
    /// Whether the instance uses it: `adb_path` absent or naming that same file.
    selected: bool,
}

impl InstanceAdb {
    /// A device instance's resolution from its declared `adb_path` (Workflow #337 §5.2).
    pub(super) fn resolve(installed: Option<&InstalledAdb>, declared: Option<&str>) -> Self {
        Self {
            installed: installed.cloned(),
            selected: installed.is_some_and(|installed| {
                declared.is_none_or(|declared| installed.is_named_by(declared))
            }),
        }
    }

    /// The install root's adb when the instance uses it.
    pub(super) fn selected(&self) -> Option<&InstalledAdb> {
        self.installed.as_ref().filter(|_| self.selected)
    }
}

/// What `assemble` hands to `require`: the install root's adb, if any, and the aliases of the
/// instances that use it. Not a configuration field; nothing of it reaches the ledger.
pub(crate) struct AdbRequirement {
    installed: Option<InstalledAdb>,
    users: Vec<String>,
}

impl AdbRequirement {
    pub(super) fn new(installed: Option<InstalledAdb>) -> Self {
        Self {
            installed,
            users: Vec::new(),
        }
    }

    /// Records an instance that uses the install root's adb.
    pub(super) fn add_user(&mut self, alias: &str) {
        self.users.push(alias.to_owned());
    }
}

/// The state `check-config` reports as `adb_default` for an install root.
pub(crate) struct AdbDefault {
    path: PathBuf,
    state: &'static str,
}

impl AdbDefault {
    pub(crate) fn report(&self) -> serde_json::Value {
        serde_json::json!({ "path": self.path.to_string_lossy(), "state": self.state })
    }
}

/// Checks the install root's adb, identically for `check-config` and daemon startup and
/// before any side effect: `Ok(None)` outside an install root; at an install root the three
/// distributed binaries are hashed against the pin, and a missing or different file refuses
/// only when an instance uses that adb. Zero users still report the state.
pub(crate) fn require(
    requirement: &AdbRequirement,
) -> Result<Option<AdbDefault>, AdbInstallRejection> {
    let Some(installed) = &requirement.installed else {
        return Ok(None);
    };
    let findings = installed.verify();
    let missing = findings
        .iter()
        .any(|finding| matches!(finding, Finding::Missing { .. }));
    if findings.is_empty() || requirement.users.is_empty() {
        let state = if findings.is_empty() {
            "ok"
        } else if missing {
            "missing"
        } else {
            "sha256_mismatch"
        };
        return Ok(Some(AdbDefault {
            path: installed.adb.clone(),
            state,
        }));
    }
    Err(AdbInstallRejection {
        code: if missing {
            ActingdCode::AdbInstallMissing.code()
        } else {
            ActingdCode::AdbInstallMismatch.code()
        },
        tools_dir: installed.root.join("tools"),
        adb: installed.adb.clone(),
        users: requirement.users.clone(),
        findings,
    })
}

/// One distributed binary that is not as pinned.
enum Finding {
    /// The file cannot be read (absent, quarantined, not a file): `missing` when it is absent,
    /// else `unreadable`, with the OS error.
    Missing {
        path: PathBuf,
        state: FileState,
        cause: Outcome,
    },
    Mismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },
    /// The pin compiled into this build cannot be read, so no file can be confirmed:
    /// `adb_install_pin_invalid`.
    PinUnreadable(Outcome),
}

impl Finding {
    /// Workflow #378 A6a: the file as a `files` record (`path`, `state`, and the two SHA-256
    /// of a mismatch); an unreadable pin names no file.
    fn record(&self) -> Option<Record> {
        match self {
            Self::Missing { path, state, .. } => Some(Record::new().path(path).state(*state)),
            Self::Mismatch {
                path,
                expected,
                actual,
            } => Some(
                Record::new()
                    .path(path)
                    .state(FileState::Sha256Mismatch)
                    .expected_sha256(format!("sha256:{expected}"))
                    .actual_sha256(format!("sha256:{actual}")),
            ),
            Self::PinUnreadable(_) => None,
        }
    }

    /// The failure behind the finding: the OS error of an unreadable file, or the unreadable
    /// pin.
    fn cause(&self) -> Option<&Outcome> {
        match self {
            Self::Missing { cause, .. } | Self::PinUnreadable(cause) => Some(cause),
            Self::Mismatch { .. } => None,
        }
    }
}

impl fmt::Display for Finding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing { path, cause, .. } => {
                write!(formatter, "missing {:?} ({cause})", path.to_string_lossy())
            }
            Self::Mismatch {
                path,
                expected,
                actual,
            } => write!(
                formatter,
                "{:?} sha256 expected {expected}, actual {actual}",
                path.to_string_lossy()
            ),
            Self::PinUnreadable(unreadable) => write!(
                formatter,
                "the platform-tools pin of this build is unreadable: {unreadable}"
            ),
        }
    }
}

/// A refused install root adb: the code, the adb, the instances that use it and every file
/// that is not as pinned.
pub(crate) struct AdbInstallRejection {
    code: Code,
    /// `<root>\tools`, which acsetup reinstalls.
    tools_dir: PathBuf,
    adb: PathBuf,
    users: Vec<String>,
    findings: Vec<Finding>,
}

impl AdbInstallRejection {
    pub(crate) const fn code(&self) -> Code {
        self.code
    }

    /// Workflow #378 A6a (model section 7.4): the refusal with the adb (`adb_path`), the
    /// instances that use it (`aliases`), the Tools folder (`tools_dir`) and each file that is
    /// not as pinned (`files`); each file's OS error, or an unreadable pin, is a `caused_by`
    /// link. What to do is the code's catalog `handling`; the developer line names it too.
    pub(crate) fn outcome(&self) -> Outcome {
        let files = self
            .findings
            .iter()
            .filter_map(Finding::record)
            .collect::<Vec<_>>();
        let outcome = Outcome::new(self.code)
            .adb_path(&self.adb)
            .aliases(&self.users)
            .tools_dir(&self.tools_dir);
        let outcome = if files.is_empty() {
            outcome
        } else {
            outcome.files(files)
        };
        self.findings
            .iter()
            .filter_map(Finding::cause)
            .fold(outcome, |outcome, cause| outcome.caused_by(cause.clone()))
    }
}

impl fmt::Display for AdbInstallRejection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "AC-installed adb {:?} used by instance(s) {:?}: ",
            self.adb.to_string_lossy(),
            self.users
        )?;
        for (index, finding) in self.findings.iter().enumerate() {
            if index > 0 {
                formatter.write_str("; ")?;
            }
            write!(formatter, "{finding}")?;
        }
        write!(formatter, "; {REMEDY}")
    }
}

#[derive(Deserialize)]
struct ToolSourcesPin {
    components: BTreeMap<String, PinComponent>,
}

#[derive(Deserialize)]
struct PinComponent {
    #[serde(default)]
    distributed_files: Option<Vec<PinnedFile>>,
}

#[derive(Deserialize)]
struct PinnedFile {
    path: String,
    sha256: String,
}

/// The distributed binaries (`.exe`, `.dll`) of the one pin component that carries
/// `distributed_files`, which must include the adb itself. A pin that breaks this is
/// `adb_install_pin_invalid` (Workflow #378 A6a), with the decoder's text as its
/// `foreign_data_error` link when it does not decode.
fn pinned_binaries() -> Result<Vec<PinnedFile>, Outcome> {
    let invalid = || Outcome::new(ActingdCode::AdbInstallPinInvalid);
    let pin = serde_json::from_str::<ToolSourcesPin>(TOOL_SOURCES_PIN).map_err(|error| {
        invalid().caused_by(
            Outcome::new(ContractCode::ForeignDataError)
                .raw_source(RawSource::Json)
                .raw_text(error.to_string()),
        )
    })?;
    let mut carriers = pin
        .components
        .into_values()
        .filter_map(|component| component.distributed_files);
    let files = match (carriers.next(), carriers.next()) {
        (Some(files), None) => files,
        _ => return Err(invalid()),
    };
    let binaries = files
        .into_iter()
        .filter(|file| {
            let path = file.path.to_ascii_lowercase();
            path.ends_with(".exe") || path.ends_with(".dll")
        })
        .collect::<Vec<_>>();
    if let Some(file) = binaries.iter().find(|file| {
        file.path.is_empty()
            || file
                .path
                .split('/')
                .any(|part| part.is_empty() || part == "..")
            || file.sha256.len() != 64
            || !file
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    }) {
        return Err(invalid().path(&file.path));
    }
    if !binaries.iter().any(|file| file.path == PINNED_ADB) {
        return Err(invalid());
    }
    Ok(binaries)
}
