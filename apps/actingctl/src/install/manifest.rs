// SPDX-License-Identifier: AGPL-3.0-only

//! Build manifests (`BUILD-MANIFEST.json` of the Runtime and the Tools, as the Windows exact-SHA
//! build writes them) and their interface declarations (`contracts/component-interfaces.md`,
//! "Parse rules"), and the check of a folder against its manifest (Workflow #421 INST-1).

use super::files::{self, is_plain_relative, join_relative};
use actingcommand_contract::outcome::{FileState, Outcome, Record};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub(crate) const MANIFEST_FILE: &str = "BUILD-MANIFEST.json";
/// The Runtime manifest's `runtime_payload_layout`.
pub(crate) const RUNTIME_LAYOUT: &str = "distribution-v1";
/// The Tools manifest's `tools_payload_layout`.
pub(crate) const TOOLS_LAYOUT: &str = "platform-tools-v3";
const INTERFACES_SCHEMA: &str = "actingcommand.component-interfaces.v1";
/// The six v1 interfaces every Runtime declaration names (Parse rules).
const V1_INTERFACES: [&str; 6] = [
    "actingd-config",
    "install-selection",
    "install-control",
    "ledger",
    "runtime-client",
    "package",
];
/// An interface added after v1, and the range a declaration without it is taken as.
const RUNTIME_OUTCOMES: (&str, [u64; 2]) = ("runtime-outcomes", [0, 0]);

/// The two halves of a build, each its own folder and manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Half {
    Runtime,
    Tools,
}

impl Half {
    /// Tools first, then the Runtime: the order an update swaps them in.
    pub(crate) const TOOLS_FIRST: [Self; 2] = [Self::Tools, Self::Runtime];

    /// The folder: `runtime` or `tools`.
    pub(crate) const fn dir(self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::Tools => "tools",
        }
    }

    /// The payload layout its manifest must name.
    pub(crate) const fn layout(self) -> &'static str {
        match self {
            Self::Runtime => RUNTIME_LAYOUT,
            Self::Tools => TOOLS_LAYOUT,
        }
    }

    /// The payload layout `manifest` names for this half.
    pub(crate) fn layout_of(self, manifest: &BuildManifest) -> Option<&str> {
        match self {
            Self::Runtime => manifest.runtime_payload_layout.as_deref(),
            Self::Tools => manifest.tools_payload_layout.as_deref(),
        }
    }
}

/// The members of a build manifest the installer reads; the others are ignored.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct BuildManifest {
    pub(crate) repository: String,
    pub(crate) commit_sha: String,
    #[serde(default)]
    pub(crate) tree_sha: Option<String>,
    pub(crate) files: Vec<ManifestFile>,
    #[serde(default)]
    pub(crate) runtime_payload_layout: Option<String>,
    #[serde(default)]
    pub(crate) tools_payload_layout: Option<String>,
    #[serde(default)]
    pub(crate) interfaces: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ManifestFile {
    pub(crate) path: String,
    pub(crate) size_bytes: u64,
    pub(crate) sha256: String,
}

/// An interface declaration's ranges, by interface name.
pub(crate) type Speaks = BTreeMap<String, [u64; 2]>;

/// Reads `<dir>\BUILD-MANIFEST.json`; the failure is the OS's or the parser's.
pub(crate) fn read_manifest(dir: &Path) -> Result<BuildManifest, Outcome> {
    files::read_json(&dir.join(MANIFEST_FILE))
}

impl BuildManifest {
    /// Whether every listed path stays below its folder and is listed once.
    pub(crate) fn paths_valid(&self) -> bool {
        let mut seen = BTreeSet::new();
        self.files.iter().all(|file| {
            is_plain_relative(&file.path)
                && file.path != MANIFEST_FILE
                && seen.insert(file.path.to_ascii_lowercase())
        })
    }

    /// The declared interfaces by the parse rules, `None` when the declaration breaks them.
    pub(crate) fn speaks(&self) -> Option<Speaks> {
        parse_declaration(self.interfaces.as_ref()?)
    }

    /// The listed paths and the manifest itself.
    pub(crate) fn file_set(&self) -> BTreeSet<String> {
        self.files
            .iter()
            .map(|file| file.path.clone())
            .chain(std::iter::once(MANIFEST_FILE.to_owned()))
            .collect()
    }
}

/// A Runtime or Tools declaration by the parse rules of `component-interfaces.md`: the schema,
/// `speaks` with `[min, max]` integer pairs, all six v1 interfaces named, `runtime-outcomes`
/// taken as [0, 0] when absent. Interfaces the reader does not know are kept and never checked.
pub(crate) fn parse_declaration(value: &Value) -> Option<Speaks> {
    let object = value.as_object()?;
    if object.get("schema_version")?.as_str()? != INTERFACES_SCHEMA {
        return None;
    }
    let mut speaks = Speaks::new();
    for (name, range) in object.get("speaks")?.as_object()? {
        let pair = range.as_array()?;
        let [min, max] = pair.as_slice() else {
            return None;
        };
        let (min, max) = (min.as_u64()?, max.as_u64()?);
        if min > max {
            return None;
        }
        speaks.insert(name.clone(), [min, max]);
    }
    if V1_INTERFACES
        .iter()
        .any(|interface| !speaks.contains_key(*interface))
    {
        return None;
    }
    speaks
        .entry(RUNTIME_OUTCOMES.0.to_owned())
        .or_insert(RUNTIME_OUTCOMES.1);
    Some(speaks)
}

/// One listed file that is not as its manifest says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileProblem {
    /// The file.
    pub(crate) path: PathBuf,
    /// Its path as the manifest lists it.
    pub(crate) listed: String,
    pub(crate) state: FileState,
    pub(crate) expected_sha256: String,
    pub(crate) actual_sha256: Option<String>,
}

impl FileProblem {
    /// The problem as a `files` record (`path`, `state`, the two digests when known).
    pub(crate) fn record(&self) -> Record {
        let record = Record::new().path(&self.path).state(self.state);
        let record = if files::is_sha256_hex(&self.expected_sha256) {
            record.expected_sha256(format!("sha256:{}", self.expected_sha256))
        } else {
            record
        };
        match &self.actual_sha256 {
            Some(actual) => record.actual_sha256(format!("sha256:{actual}")),
            None => record,
        }
    }
}

/// Every listed file of `manifest` under `dir` whose size or SHA-256 differs, or that cannot be
/// read. Files the manifest does not list are not looked at.
pub(crate) fn file_problems(dir: &Path, manifest: &BuildManifest) -> Vec<FileProblem> {
    manifest
        .files
        .iter()
        .filter_map(|file| {
            let path = join_relative(dir, &file.path);
            let expected = file.sha256.to_ascii_lowercase();
            match files::sha256_file(&path) {
                Ok((actual, size)) if actual == expected && size == file.size_bytes => None,
                Ok((actual, _)) => Some(FileProblem {
                    path,
                    listed: file.path.clone(),
                    state: FileState::Sha256Mismatch,
                    expected_sha256: expected,
                    actual_sha256: Some(actual),
                }),
                Err(error) => Some(FileProblem {
                    state: if error.kind() == std::io::ErrorKind::NotFound {
                        FileState::Missing
                    } else {
                        FileState::Unreadable
                    },
                    path,
                    listed: file.path.clone(),
                    expected_sha256: expected,
                    actual_sha256: None,
                }),
            }
        })
        .collect()
}

/// The files below `dir` that `manifest` does not list (nor is it), as relative paths.
pub(crate) fn unlisted_files(dir: &Path, manifest: &BuildManifest) -> Result<Vec<String>, Outcome> {
    let listed = manifest
        .file_set()
        .into_iter()
        .map(|path| path.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    Ok(files::list_files(dir)?
        .into_iter()
        .filter(|path| !listed.contains(&path.to_ascii_lowercase()))
        .collect())
}

/// A build folder (`runtime\` and `tools\` below `base`) checked against its own manifests.
pub(crate) struct Build {
    pub(crate) commit: String,
    pub(crate) runtime: BuildManifest,
    pub(crate) tools: BuildManifest,
}

impl Build {
    pub(crate) fn manifest(&self, half: Half) -> &BuildManifest {
        match half {
            Half::Runtime => &self.runtime,
            Half::Tools => &self.tools,
        }
    }

    /// The Runtime manifest's declaration; `None` when it breaks the parse rules.
    pub(crate) fn speaks(&self) -> Option<Speaks> {
        self.runtime.speaks()
    }

    /// Whether both halves declare the same, valid interfaces.
    pub(crate) fn speaks_agree(&self) -> bool {
        match (self.runtime.speaks(), self.tools.speaks()) {
            (Some(runtime), Some(tools)) => runtime == tools,
            _ => false,
        }
    }
}

/// Why a build folder does not verify.
pub(crate) enum BuildProblem {
    /// A manifest cannot be read (the half and the OS's or the parser's failure), or it lists
    /// a path that leaves its folder or repeats (no cause).
    Manifest(Half, Option<Outcome>),
    /// The two manifests name different commits, or a commit that is not 40 hex digits.
    Commits,
    /// Listed files that differ.
    Files(Vec<FileProblem>),
}

/// Reads both manifests of the build below `base` and checks every listed file.
pub(crate) fn verify_build(base: &Path) -> Result<Build, BuildProblem> {
    let read = |half: Half| -> Result<BuildManifest, BuildProblem> {
        let manifest = read_manifest(&base.join(half.dir()))
            .map_err(|cause| BuildProblem::Manifest(half, Some(cause)))?;
        if manifest.paths_valid() {
            Ok(manifest)
        } else {
            Err(BuildProblem::Manifest(half, None))
        }
    };
    let runtime = read(Half::Runtime)?;
    let tools = read(Half::Tools)?;
    if runtime.commit_sha != tools.commit_sha || !files::is_commit(&runtime.commit_sha) {
        return Err(BuildProblem::Commits);
    }
    let mut problems = file_problems(&base.join(Half::Runtime.dir()), &runtime);
    problems.extend(file_problems(&base.join(Half::Tools.dir()), &tools));
    if !problems.is_empty() {
        return Err(BuildProblem::Files(problems));
    }
    Ok(Build {
        commit: runtime.commit_sha.clone(),
        runtime,
        tools,
    })
}
