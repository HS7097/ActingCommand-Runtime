// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #453 S4 (`contracts/install-cli.md`, "Fresh root"; register, "Slice #453 S4"): the
//! rescue row `new <new_root> --build <dir> [--new-state-root <dir>] [--mumu-root <dir>]
//! [--lab-authoring true|false] [--watchdog]`, a fresh installation root built from a verified
//! release or kept build, with its first configuration. `actingctl install` has no `new`.
//!
//! - **Where.** The row acts on the root it names, not on the root of the program that runs it:
//!   it is the one rescue row served without a located root (`crate::rescue`, `answer_here`), so
//!   the release's own `actinglab.exe` and the Lab's MCP server run it before any root exists. The
//!   positional is `new_root` and the state root's option `new_state_root`, since the module
//!   refuses a parameter named `root` or `state_root` on every row.
//! - **Refused before anything is written** (`install_command_refused` of `install_new`): a
//!   `new_root` that exists and is not an empty folder (`root_not_empty`), a state root that
//!   exists and is not an empty folder (`state_root_not_empty`), a `--build` that does not verify
//!   (a release folder: the release reader's `release_check_failed` with its `failures`; a kept
//!   build folder: `build_invalid`; a build whose Runtime member carries no first configuration's
//!   template: `build_invalid` with the template's path), and an empty `actingd-config` edge
//!   between the running installer and the build (`interface_edge`, the row in `edges`). Nothing
//!   is downloaded: `--build` is a folder already there.
//! - **One journalled operation** of kind `new` (off the watchdog's pass list: while it is
//!   unfinished no watchdog starts a Runtime on the root, and a root that never started stays
//!   down), under `install\writer.lock`, its steps in this order: `copy_build` (the build's listed
//!   files into `install\builds\<c>.new-<ms>.part`, with its `release.json`), `verify_tree`,
//!   `place_build`, one `stage_file` and one `swap_file` for every file of the kept build into
//!   `runtime\` and `tools\` (all new), `verify_tree` of the live tree, `write_config`,
//!   `create_state_root` and `record_state`. Each step is skipped when its postcondition holds,
//!   so `resume` continues it; `undo` sets every written item aside and deletes nothing.
//! - **The first configuration** is the template the Runtime member ships
//!   (`actingd.config.first.json`), its values the template's; the command fills only
//!   `state_root` (absolute), `secret_fingerprint_salt` (32 bytes of the operating system's
//!   generator written as 64 lowercase hexadecimal digits: never printed, journalled, answered or
//!   logged), `lab_module_authoring`, and `mumu_root` when it is given. It is written beside the
//!   live file as `actingd.config.json.new-<ms>.part` and checked by the new live build's
//!   check-config with `--install-root <new_root>` before it is renamed into place; a refusal ends
//!   the operation refused with the part kept.
//! - **Not this command's** (Workflow #453 S11): it mints no installation identity and writes no
//!   state-root record. It creates the state root's folder, empty, and writes `install\state.json`,
//!   so the first start of the root mints the identity and claims the state root.
//! - **`--watchdog`**: the watchdog's task registered for the new root, last, after the operation
//!   ended; nothing is started.

use super::files::{self, join_relative};
use super::host::Host;
use super::journal::{self, Opening, Operation, PlannedStep, Previous, Step, StepKind, StepState};
use super::lock::WriterLock;
use super::manifest::{self, BuildManifest, BuildProblem, Half};
use super::plan::{Edge, EdgeResult, Party};
use super::programs::{self, Deadline};
use super::state::{
    self, BuildOrigin, Current, InstallState, KeptBuild, LiveBuild, OperationKind, Paths,
};
use super::update::{self, SwapIntent};
use super::{Answer, Face};
use crate::cli_line;
use crate::codes::{
    ActingctlCode, ArgumentProblem, InstallCaveat, InstallLocation, InstallRefusal, ReleaseCheck,
};
use actingcommand_contract::outcome::{CauseRelation, IoOp, Outcome};
use actingcommand_lab_ops::core_reads::Params;
use actingcommand_lab_ops::initiator::Initiator;
use actingcommand_runtime_config::install::InstallSettings;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

/// The row's name in the operation table.
pub(crate) const ROW: &str = "new";
/// The command of every outcome of the row.
const NEW: InstallLocation = InstallLocation::New;
/// The first configuration's template in the Runtime member (`runtime\`).
pub(crate) const TEMPLATE_FILE: &str = "actingd.config.first.json";
/// The release-facts file a release folder may hold beside its zips.
const RELEASE_FACTS: &str = "release-facts.json";
/// A kept build's record of its release (`release stage` writes it).
const RELEASE_RECORD: &str = "release.json";
/// The schema of a kept build's `release.json`.
const RELEASE_RECORD_SCHEMA: &str = "actingcommand.install-release.v1";
/// The state root's folder below the new root when `--new-state-root` is not given.
const DEFAULT_STATE_FOLDER: &str = "state";
/// The members of the first configuration the command fills; every other value is the
/// template's.
const STATE_ROOT_MEMBER: &str = "state_root";
const SALT_MEMBER: &str = "secret_fingerprint_salt";
const MUMU_ROOT_MEMBER: &str = "mumu_root";
const AUTHORING_MEMBER: &str = actingcommand_runtime_config::lab_module::AUTHORING;
/// How many bytes of the operating system's generator the salt takes.
const SALT_BYTES: usize = 32;
/// The interface the edge between the running installer and the build is judged on.
const CONFIG_INTERFACE: &str = "actingd-config";
/// The Lab's program in a root's Tools member, which runs the rows that follow on the new root.
const LAB_PROGRAM: &str = "actinglab.exe";

/// What the operation needs to run, resume and undo its steps, as its journal keeps it. Never the
/// salt.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NewInputs {
    /// `--build` as given.
    build: String,
    /// The folder the build is copied from: the release reader's verified work folder, or the
    /// kept build folder itself.
    source: String,
    commit: String,
    origin: BuildOrigin,
    tag: Option<String>,
    /// The kept build's `release.json`, written into its folder.
    release_record: Option<Value>,
    /// The state root, absolute, as the configuration names it.
    state_root: String,
    /// Whether the state root's folder was there (empty) before the operation.
    state_root_existed: bool,
    mumu_root: Option<String>,
    lab_authoring: bool,
    watchdog: bool,
}

/// The intent of the steps on the kept build: its commit, its part folder and its folder.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct BuildIntent {
    commit: String,
    #[serde(default)]
    part: Option<String>,
    #[serde(default)]
    build: Option<String>,
}

/// The intent of a `stage_file` step.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StageIntent {
    dst: String,
    part: String,
    source: String,
    sha256: String,
}

/// The intent of `write_config`: the part the first configuration is written to, and the
/// template it is made from, relative to the root.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigIntent {
    part: String,
    template: String,
}

/// The intent of `create_state_root`: the folder, absolute, and whether it was there before.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StateRootIntent {
    path: String,
    existed: bool,
}

/// What a folder the row is to fill holds.
enum Fill {
    Absent,
    Empty,
    NotEmpty,
}

/// What `path` holds: nothing, an empty folder, or anything else (a file is not empty).
fn fill_of(path: &Path) -> std::io::Result<Fill> {
    match std::fs::metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Fill::Absent),
        Err(error) => Err(error),
        Ok(metadata) if !metadata.is_dir() => Ok(Fill::NotEmpty),
        Ok(_) => Ok(if std::fs::read_dir(path)?.next().is_none() {
            Fill::Empty
        } else {
            Fill::NotEmpty
        }),
    }
}

/// `path` absolute, against the current folder when it is relative.
fn absolute(path: &Path, word: &str) -> Result<PathBuf, Outcome> {
    std::path::absolute(path).map_err(|error| {
        super::unusable(word).caused_by(files::os_failure(path, &error, IoOp::Canonicalize))
    })
}

/// A text parameter of the row.
fn text<'a>(params: Params<'a>, name: &str) -> Option<&'a str> {
    params.get(name).and_then(Value::as_str)
}

/// The row's answer for `params` on `host`, as `initiator`: the answer, or a refusal without
/// members (a usage refusal among them).
pub(crate) fn row(
    host: &dyn Host,
    initiator: Initiator,
    params: Params<'_>,
) -> Result<Answer, Outcome> {
    let Some(new_root) = text(params, "new_root").filter(|root| !root.is_empty()) else {
        return Err(cli_line::usage_invalid(
            Some("new_root"),
            ArgumentProblem::Required,
        ));
    };
    let Some(build) = text(params, "build").filter(|build| !build.is_empty()) else {
        return Err(cli_line::usage_invalid(
            Some("--build"),
            ArgumentProblem::Required,
        ));
    };
    let lab_authoring = match params.get("lab_authoring") {
        None | Some(Value::Null) => true,
        Some(Value::Bool(given)) => *given,
        Some(_) => {
            return Err(cli_line::usage_invalid(
                Some("--lab-authoring"),
                ArgumentProblem::WrongType,
            ));
        }
    };
    let watchdog = params
        .get("watchdog")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let root = absolute(Path::new(new_root), "new_root")?;
    let state_root = match text(params, "new_state_root") {
        Some(given) => absolute(Path::new(given), "--new-state-root")?,
        None => root.join(DEFAULT_STATE_FOLDER),
    };
    let mumu_root = text(params, "mumu_root")
        .map(|given| absolute(Path::new(given), "--mumu-root"))
        .transpose()?;

    // 1. The new root and the state root, absent or empty.
    if let Some(refusal) = refuse_filled(&root, InstallRefusal::RootNotEmpty)? {
        return Ok(refusal);
    }
    let state_root_existed = match fill_of(&state_root) {
        Ok(Fill::Absent) => false,
        Ok(Fill::Empty) => true,
        Ok(Fill::NotEmpty) => {
            return Err(super::refusal(NEW, InstallRefusal::StateRootNotEmpty).path(&state_root));
        }
        Err(error) => {
            return Err(super::refusal(NEW, InstallRefusal::StateRootNotEmpty)
                .path(&state_root)
                .caused_by(files::os_failure(&state_root, &error, IoOp::ReadDir)));
        }
    };
    // 2. The build verifies.
    let verified = match verify_build(host, Path::new(build))? {
        Ok(verified) => verified,
        Err(refused) => return Ok(refused),
    };
    // 3. The interface edge between the running installer and the build.
    let edge = config_edge(host, &verified.build);
    if edge.result != EdgeResult::Pass {
        let refusal = super::refusal(NEW, InstallRefusal::InterfaceEdge)
            .commit_sha(verified.build.commit.clone())
            .count(1_u32);
        return Answer::new(refusal).with("edges", [edge]);
    }

    // The first write: the root's folder, then the writer lock and the operation.
    files::create_dir_all(&root)?;
    let paths = Paths::resolve(&root)?;
    let settings = settings()?;
    let lock = WriterLock::take(&paths, settings.lock_wait_ms, NEW)?;
    let inputs = NewInputs {
        build: files::plain(Path::new(build)),
        source: files::plain(&verified.source),
        commit: verified.build.commit.clone(),
        origin: verified.origin,
        tag: verified.tag.clone(),
        release_record: verified.release_record.clone(),
        state_root: files::plain(&state_root),
        state_root_existed,
        mumu_root: mumu_root.as_deref().map(files::plain),
        lab_authoring,
        watchdog,
    };
    let opening = Opening {
        kind: OperationKind::New,
        initiator: Face::Rescue(initiator).initiator(),
        one_way: false,
        inputs: serde_json::to_value(&inputs).map_err(|error| files::json_failure(&error))?,
        plan_id: None,
        plan: None,
        acknowledged: Vec::new(),
        s0: None,
        previous: Previous {
            live: None,
            state: None,
        },
    };
    let operation = Operation::open(host, &paths, opening, |ms| {
        steps(&verified.build, &verified.sums, &inputs, ms)
    })?;
    let mut answer = match run(operation, &inputs, &settings, verified.notes) {
        Ok(answer) => answer,
        // A step failed after the first write: the journal names the ways on.
        Err(outcome) if outcome.is(ActingctlCode::InstallOperationInterrupted) => {
            let operation_id = outcome
                .value("install_operation_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            Answer::new(outcome).with("next", ways_on(&paths.plain, &operation_id))?
        }
        Err(outcome) => return Err(outcome),
    };
    if let Some(work) = &verified.work_dir {
        answer.members.insert("work_dir".to_owned(), json!(work));
    }
    drop(lock);
    // `--watchdog`: last, after the operation ended and the lock was released; it starts nothing.
    if watchdog && answer.outcome.is(ActingctlCode::InstallCommandDone) {
        answer = with_watchdog(host, &paths, answer);
    }
    Ok(answer)
}

/// The refusal `refusal` of a folder that exists and is not empty, `None` for an absent or empty
/// one. A root that holds an unfinished operation of this row also names it and how to continue
/// or undo it.
fn refuse_filled(root: &Path, refusal: InstallRefusal) -> Result<Option<Answer>, Outcome> {
    match fill_of(root) {
        Ok(Fill::Absent | Fill::Empty) => Ok(None),
        Ok(Fill::NotEmpty) => {
            let refused = super::refusal(NEW, refusal).path(root);
            let current = root.join("install").join("current.json");
            match files::read_json::<Current>(&current) {
                Ok(current) if current.kind == OperationKind::New => {
                    let id = current.operation_id;
                    let plain = files::plain(root);
                    Ok(Some(
                        Answer::new(refused.install_operation_id(id.clone()))
                            .with("next", ways_on(&plain, &id))?,
                    ))
                }
                _ => Ok(Some(Answer::new(refused))),
            }
        }
        Err(error) => Err(super::refusal(NEW, refusal)
            .path(root)
            .caused_by(files::os_failure(root, &error, IoOp::ReadDir))),
    }
}

/// The ways to continue or undo an unfinished operation of this row on the root `root`: until
/// LAB-7 deletes actingctl, its `install resume` and `install undo`, which any build of this
/// release or later runs; the module's own `op resume` and `undo` from the new root's own
/// program once its configuration is in place.
fn ways_on(root: &str, operation_id: &str) -> Value {
    json!({
        "actingctl_resume": ["actingctl", "install", "resume", "--root", root, "--operation", operation_id],
        "actingctl_undo": ["actingctl", "install", "undo", "--root", root, "--operation", operation_id],
    })
}

/// The `install` section's values the operation runs with: the running installer's defaults (a
/// fresh root has no configuration yet, and the template names no `install` section).
fn settings() -> Result<InstallSettings, Outcome> {
    actingcommand_runtime_config::install::settings(&Value::Object(Map::new())).map_err(|failure| {
        super::refusal(NEW, InstallRefusal::InstallSectionInvalid).caused_by(failure.outcome())
    })
}

/// A verified build, with what the operation records of it.
struct Verified {
    /// The folder the build is copied from.
    source: PathBuf,
    build: manifest::Build,
    /// The SHA-256 of each half's `BUILD-MANIFEST.json` in `source`.
    sums: ManifestSums,
    origin: BuildOrigin,
    tag: Option<String>,
    release_record: Option<Value>,
    /// The release reader's work folder, left where it is.
    work_dir: Option<String>,
    /// What the verification noted (`release_unconfirmed`).
    notes: Vec<Outcome>,
}

/// The SHA-256 of the two manifests of a build folder.
struct ManifestSums {
    runtime: String,
    tools: String,
}

impl ManifestSums {
    /// The SHA-256 of `half`'s manifest.
    fn of(&self, half: Half) -> &str {
        match half {
            Half::Runtime => &self.runtime,
            Half::Tools => &self.tools,
        }
    }
}

/// `--build` verified: a folder holding `runtime\BUILD-MANIFEST.json` is a kept build folder,
/// checked against its own manifests; any other is a release folder, checked by the release reader
/// (`release verify`, its zips extracted into a new work folder that is left in place). The
/// refusal (with the reader's `failures`) when it does not verify.
fn verify_build(host: &dyn Host, dir: &Path) -> Result<Result<Verified, Answer>, Outcome> {
    let kept = dir
        .join(Half::Runtime.dir())
        .join(manifest::MANIFEST_FILE)
        .is_file();
    let invalid = || super::refusal(NEW, InstallRefusal::BuildInvalid).path(dir);
    let (source, origin, tag, release_record, work_dir, notes) = if kept {
        let record = files::read_json::<Value>(&dir.join(RELEASE_RECORD)).ok();
        let facts = record
            .as_ref()
            .and_then(|record| record.get("facts"))
            .filter(|facts| facts.is_object());
        let tag = facts
            .and_then(|facts| facts.get("tag"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let origin = if facts.is_some() {
            BuildOrigin::Release
        } else {
            BuildOrigin::LocalBuild
        };
        (dir.to_path_buf(), origin, tag, record, None, Vec::new())
    } else {
        let dir_word = dir.to_string_lossy().into_owned();
        let facts = dir.join(RELEASE_FACTS);
        let facts_word = facts.to_string_lossy().into_owned();
        let mut words = vec!["--dir", dir_word.as_str()];
        if facts.is_file() {
            words.extend(["--facts", facts_word.as_str()]);
        }
        let read = super::release::verify(&words, host)
            .map_err(|cause| super::unusable("--build").caused_by(cause))?;
        if read.outcome.category() > actingcommand_contract::outcome::Category::Warning {
            let checks = read
                .outcome
                .value("release_checks")
                .and_then(Value::as_array)
                .map(|checks| {
                    checks
                        .iter()
                        .filter_map(Value::as_str)
                        .filter_map(|token| {
                            ReleaseCheck::ALL
                                .iter()
                                .copied()
                                .find(|check| check.as_str() == token)
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let refusal = super::refusal(NEW, InstallRefusal::ReleaseCheckFailed)
                .release_checks(checks)
                .path(dir)
                .caused_by(read.outcome);
            let mut refused = Answer::new(refusal);
            if let Some(failures) = read.members.get("failures") {
                refused = refused.with("failures", failures)?;
            }
            return Ok(Err(refused));
        }
        // A UI release is no Runtime build.
        if read.members.contains_key("member") {
            return Ok(Err(Answer::new(invalid())));
        }
        let Some(work) = read.members.get("work_dir").and_then(Value::as_str) else {
            return Ok(Err(Answer::new(invalid())));
        };
        let facts_value = if facts.is_file() {
            Some(files::read_json::<Value>(&facts).map_err(|cause| invalid().caused_by(cause))?)
        } else {
            None
        };
        let member = |name: &str| read.members.get(name).cloned().unwrap_or(Value::Null);
        let record = json!({
            "schema_version": RELEASE_RECORD_SCHEMA,
            "commit": member("commit"),
            "tree": member("tree"),
            "tag": member("tag"),
            "prerelease": member("prerelease"),
            "interfaces": member("interfaces"),
            "files": member("files"),
            "local_build": facts_value.is_none(),
            "facts": facts_value.clone().unwrap_or(Value::Null),
        });
        let tag = read
            .members
            .get("tag")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let (origin, notes) = if facts_value.is_some() {
            (BuildOrigin::Release, Vec::new())
        } else {
            let commit = read
                .members
                .get("commit")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            (
                BuildOrigin::LocalBuild,
                vec![super::caveat(NEW, InstallCaveat::ReleaseUnconfirmed).commit_sha(commit)],
            )
        };
        (
            PathBuf::from(work),
            origin,
            tag,
            Some(record),
            Some(work.to_owned()),
            notes,
        )
    };
    let build = match manifest::verify_build(&source) {
        Ok(build) if build.speaks_agree() => build,
        Ok(_) | Err(BuildProblem::Commits) => return Ok(Err(Answer::new(invalid()))),
        Err(BuildProblem::Manifest(_, cause)) => {
            return Ok(Err(Answer::new(match cause {
                Some(cause) => invalid().caused_by(cause),
                None => invalid(),
            })));
        }
        Err(BuildProblem::Files(problems)) => {
            return Ok(Err(Answer::new(
                invalid()
                    .count(problems.len())
                    .files(problems.iter().map(manifest::FileProblem::record)),
            )));
        }
    };
    // The first configuration's template is a file of the Runtime member.
    if !build
        .runtime
        .files
        .iter()
        .any(|file| file.path == TEMPLATE_FILE)
    {
        return Ok(Err(Answer::new(
            super::refusal(NEW, InstallRefusal::BuildInvalid)
                .commit_sha(build.commit.clone())
                .path(source.join(Half::Runtime.dir()).join(TEMPLATE_FILE)),
        )));
    }
    let sum = |half: Half| {
        files::sha256_file(&source.join(half.dir()).join(manifest::MANIFEST_FILE))
            .map(|(sha256, _)| sha256)
            .map_err(|error| invalid().caused_by(files::os_failure(&source, &error, IoOp::Read)))
    };
    let sums = ManifestSums {
        runtime: sum(Half::Runtime)?,
        tools: sum(Half::Tools)?,
    };
    Ok(Ok(Verified {
        source,
        build,
        sums,
        origin,
        tag,
        release_record,
        work_dir,
        notes,
    }))
}

/// The edge `I ∩ R` for `actingd-config`: the running installer writes the configuration the
/// build reads, so the two must share a revision.
fn config_edge(host: &dyn Host, build: &manifest::Build) -> Edge {
    let installer = host
        .installer_speaks()
        .and_then(|speaks| speaks.get(CONFIG_INTERFACE).copied());
    let target = build
        .speaks()
        .and_then(|speaks| speaks.get(CONFIG_INTERFACE).copied());
    let meets = matches!((installer, target), (Some(own), Some(other))
        if own[0].max(other[0]) <= own[1].min(other[1]));
    Edge {
        interface: CONFIG_INTERFACE,
        check: "installer_meets_next",
        result: if meets {
            EdgeResult::Pass
        } else {
            EdgeResult::Fail
        },
        parties: vec![
            Party {
                party: "I",
                source: "declaration",
                range: installer,
                game: None,
            },
            Party {
                party: "R*",
                source: "manifest",
                range: target,
                game: None,
            },
        ],
        keys: Vec::new(),
    }
}

/// Every file of a half's manifest, the manifest itself last, as relative paths below the half.
fn half_files<'a>(manifest: &'a BuildManifest, manifest_sum: &'a str) -> Vec<(&'a str, &'a str)> {
    manifest
        .files
        .iter()
        .map(|file| (file.path.as_str(), file.sha256.as_str()))
        .chain(std::iter::once((manifest::MANIFEST_FILE, manifest_sum)))
        .collect()
}

/// The operation's steps for the operation's milliseconds `ms`.
fn steps(
    build: &manifest::Build,
    sums: &ManifestSums,
    inputs: &NewInputs,
    ms: u64,
) -> Vec<PlannedStep> {
    let commit = &build.commit;
    let part = format!("install/builds/{commit}.new-{ms}.part");
    let kept = format!("install/builds/{commit}");
    let whole = json!({"commit": commit, "part": part, "build": kept});
    let mut steps = vec![
        (StepKind::CopyBuild, part.clone(), whole.clone()),
        (StepKind::VerifyTree, part, whole.clone()),
        (StepKind::PlaceBuild, kept.clone(), whole),
    ];
    let mut live = Vec::new();
    for half in Half::TOOLS_FIRST {
        for (path, sha256) in half_files(build.manifest(half), sums.of(half)) {
            live.push((
                format!("{}/{path}", half.dir()),
                sha256.to_ascii_lowercase(),
            ));
        }
    }
    for (dst, sha256) in &live {
        let part = format!("{dst}.new-{ms}.part");
        steps.push((
            StepKind::StageFile,
            part.clone(),
            json!({"dst": dst, "part": part, "source": format!("{kept}/{dst}"), "sha256": sha256}),
        ));
    }
    for (dst, sha256) in &live {
        steps.push((
            StepKind::SwapFile,
            dst.clone(),
            json!({
                "dst": dst,
                "part": format!("{dst}.new-{ms}.part"),
                "old": Value::Null,
                "old_sha256": Value::Null,
                "sha256": sha256,
            }),
        ));
    }
    steps.push((
        StepKind::VerifyTree,
        Half::Runtime.dir().to_owned(),
        json!({"commit": commit}),
    ));
    let config_part = format!("{}.new-{ms}.part", state::CONFIG_FILE);
    steps.push((
        StepKind::WriteConfig,
        state::CONFIG_FILE.to_owned(),
        json!({
            "part": config_part,
            "template": format!("{}/{TEMPLATE_FILE}", Half::Runtime.dir()),
        }),
    ));
    steps.push((
        StepKind::CreateStateRoot,
        inputs.state_root.clone(),
        json!({"path": inputs.state_root, "existed": inputs.state_root_existed}),
    ));
    steps.push((
        StepKind::RecordState,
        "install/state.json".to_owned(),
        json!({"commit": commit}),
    ));
    steps
}

/// Whether `path` holds `sha256`.
fn holds(path: &Path, sha256: &str) -> bool {
    files::sha256_file(path).is_ok_and(|(actual, _)| actual == sha256)
}

/// Whether the build `commit` is kept at `build`, verified against its own manifests.
fn placed(build: &Path, commit: &str) -> bool {
    manifest::verify_build(build).is_ok_and(|kept| kept.commit == commit)
}

/// Copies exactly the listed files of `half` (and its manifest) from `from` to `to`.
fn copy_half(from: &Path, to: &Path, manifest: &BuildManifest) -> Result<(), Outcome> {
    for listed in manifest
        .files
        .iter()
        .map(|file| file.path.as_str())
        .chain(std::iter::once(manifest::MANIFEST_FILE))
    {
        files::copy_file(&join_relative(from, listed), &join_relative(to, listed))?;
    }
    Ok(())
}

/// The first configuration: the template at `template`, with the state root, a new salt, the
/// Lab authoring switch and the emulator root when it is given; every other member the
/// template's, in its order.
fn first_configuration(template: &Path, inputs: &NewInputs) -> Result<Vec<u8>, Outcome> {
    let invalid = || super::refusal(NEW, InstallRefusal::BuildInvalid).path(template);
    let document =
        files::read_json::<Value>(template).map_err(|cause| invalid().caused_by(cause))?;
    let Value::Object(mut members) = document else {
        return Err(invalid());
    };
    members.insert(
        STATE_ROOT_MEMBER.to_owned(),
        Value::String(inputs.state_root.clone()),
    );
    members.insert(SALT_MEMBER.to_owned(), Value::String(new_salt()?));
    members.insert(
        AUTHORING_MEMBER.to_owned(),
        Value::Bool(inputs.lab_authoring),
    );
    if let Some(mumu_root) = &inputs.mumu_root {
        members.insert(
            MUMU_ROOT_MEMBER.to_owned(),
            Value::String(mumu_root.clone()),
        );
    }
    let mut bytes = serde_json::to_vec_pretty(&Value::Object(members))
        .map_err(|error| files::json_failure(&error))?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// A new fingerprint salt: 32 bytes of the operating system's generator as 64 lowercase
/// hexadecimal digits. It goes into the first configuration only.
fn new_salt() -> Result<String, Outcome> {
    let mut bytes = [0_u8; SALT_BYTES];
    actingcommand_host_metrics::fill_system_random(&mut bytes)
        .map_err(|error| Outcome::from_io_error(&error, IoOp::Read))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// The operation's steps, each skipped when it is done or its postcondition holds on disk; then
/// its end. A failure after the first write is `install_operation_interrupted`, the journal and
/// `current.json` kept for `resume` and `undo`; a first configuration the build's check-config
/// refuses ends the operation refused, its part kept.
fn run(
    mut operation: Operation<'_>,
    inputs: &NewInputs,
    settings: &InstallSettings,
    notes: Vec<Outcome>,
) -> Result<Answer, Outcome> {
    let paths = operation.paths().clone();
    let host = operation.host();
    let commit = inputs.commit.as_str();
    let invalid_journal = |operation: &Operation<'_>| {
        operation.interrupted(journal::state_invalid(
            NEW,
            paths.journal(operation.id()),
            None,
        ))
    };
    let mut check_config = Value::Null;
    let mut n = 1;
    while n <= operation.journal().steps.len() {
        let step: Step = operation.journal().steps[n - 1].clone();
        if matches!(step.state, StepState::Done | StepState::Undone) {
            n += 1;
            continue;
        }
        let mut evidence = None;
        match step.kind {
            StepKind::CopyBuild => {
                let intent: BuildIntent = operation
                    .intent(n)
                    .map_err(|cause| operation.interrupted(cause))?;
                let (Some(part), Some(build)) = (intent.part, intent.build) else {
                    return Err(invalid_journal(&operation));
                };
                if !placed(&paths.at(&build), commit) {
                    operation
                        .start(n)
                        .map_err(|cause| operation.interrupted(cause))?;
                    let source = PathBuf::from(&inputs.source);
                    let verified = manifest::verify_build(&source)
                        .ok()
                        .filter(|found| found.commit == commit)
                        .ok_or_else(|| {
                            operation.interrupted(
                                super::refusal(NEW, InstallRefusal::BuildInvalid)
                                    .commit_sha(commit.to_owned())
                                    .path(&source),
                            )
                        })?;
                    let part = paths.at(&part);
                    for half in [Half::Runtime, Half::Tools] {
                        copy_half(
                            &source.join(half.dir()),
                            &part.join(half.dir()),
                            verified.manifest(half),
                        )
                        .map_err(|cause| operation.interrupted(cause))?;
                    }
                    if let Some(record) = &inputs.release_record {
                        files::write_json_atomic(&part.join(RELEASE_RECORD), record)
                            .map_err(|cause| operation.interrupted(cause))?;
                    }
                }
            }
            StepKind::VerifyTree => {
                let intent: BuildIntent = operation
                    .intent(n)
                    .map_err(|cause| operation.interrupted(cause))?;
                operation
                    .start(n)
                    .map_err(|cause| operation.interrupted(cause))?;
                match (intent.part, intent.build) {
                    // The part folder, unless the build is kept already.
                    (Some(part), Some(build)) => {
                        if !placed(&paths.at(&build), commit) && !placed(&paths.at(&part), commit) {
                            return Err(operation.interrupted(
                                super::refusal(NEW, InstallRefusal::BuildInvalid)
                                    .commit_sha(commit.to_owned())
                                    .path(paths.at(&part)),
                            ));
                        }
                    }
                    // The live tree: every manifest file in place.
                    _ => match manifest::verify_build(&paths.root) {
                        Ok(live) if live.commit == commit => {}
                        _ => {
                            let cause = super::adopt::verify_live(NEW, &paths)
                                .err()
                                .unwrap_or_else(|| {
                                    super::refusal(NEW, InstallRefusal::LiveBuildInvalid)
                                        .path(&paths.root)
                                });
                            return Err(operation.interrupted(cause));
                        }
                    },
                }
            }
            StepKind::PlaceBuild => {
                let intent: BuildIntent = operation
                    .intent(n)
                    .map_err(|cause| operation.interrupted(cause))?;
                let (Some(part), Some(build)) = (intent.part, intent.build) else {
                    return Err(invalid_journal(&operation));
                };
                let build = paths.at(&build);
                if !placed(&build, commit) {
                    operation
                        .start(n)
                        .map_err(|cause| operation.interrupted(cause))?;
                    files::rename(&paths.at(&part), &build)
                        .map_err(|cause| operation.interrupted(cause))?;
                }
            }
            StepKind::StageFile => {
                let intent: StageIntent = operation
                    .intent(n)
                    .map_err(|cause| operation.interrupted(cause))?;
                let part = paths.at(&intent.part);
                let dst = paths.at(&intent.dst);
                if !holds(&part, &intent.sha256) && !holds(&dst, &intent.sha256) {
                    operation
                        .start(n)
                        .map_err(|cause| operation.interrupted(cause))?;
                    let source = paths.at(&intent.source);
                    files::copy_file(&source, &part)
                        .map_err(|cause| operation.interrupted(cause))?;
                    if !holds(&part, &intent.sha256) {
                        return Err(operation.interrupted(
                            super::refusal(NEW, InstallRefusal::BuildInvalid)
                                .commit_sha(commit.to_owned())
                                .path(&source),
                        ));
                    }
                }
            }
            StepKind::SwapFile => {
                let intent: SwapIntent = operation
                    .intent(n)
                    .map_err(|cause| operation.interrupted(cause))?;
                let dst = paths.at(&intent.dst);
                if !holds(&dst, &intent.sha256) {
                    operation
                        .start(n)
                        .map_err(|cause| operation.interrupted(cause))?;
                    let operation_id = operation.id().to_owned();
                    update::swap(NEW, &dst, &paths.at(&intent.part), None, || {
                        host.between_renames(&operation_id, n)
                    })
                    .map_err(|cause| operation.interrupted(cause))?;
                    if !holds(&dst, &intent.sha256) {
                        return Err(operation.interrupted(
                            super::refusal(NEW, InstallRefusal::BuildInvalid).path(&dst),
                        ));
                    }
                }
            }
            StepKind::WriteConfig => {
                let intent: ConfigIntent = operation
                    .intent(n)
                    .map_err(|cause| operation.interrupted(cause))?;
                let config = paths.config();
                if !config.is_file() {
                    operation
                        .start(n)
                        .map_err(|cause| operation.interrupted(cause))?;
                    let part = paths.at(&intent.part);
                    // A part an earlier attempt wrote is checked as it is: its salt stays.
                    if !part.is_file() {
                        let bytes = first_configuration(&paths.at(&intent.template), inputs)
                            .map_err(|cause| operation.interrupted(cause))?;
                        files::write_atomic(&part, &bytes)
                            .map_err(|cause| operation.interrupted(cause))?;
                    }
                    let checked = programs::check_config(
                        host,
                        &paths.root.join(Half::Runtime.dir()),
                        &part,
                        Some(&paths.root),
                        Deadline::check_config(settings),
                    )
                    .map_err(|failure| {
                        operation.interrupted(
                            failure
                                .onto(super::refusal(NEW, InstallRefusal::CheckConfigUnanswered)),
                        )
                    })?;
                    let summary = serde_json::to_value(&checked.summary).unwrap_or(Value::Null);
                    if !checked.passed() {
                        // The candidate stays where it is, kept as every part.
                        let refusal = super::refusal(NEW, InstallRefusal::ConfigRefusedByTarget)
                            .path(&part)
                            .install_operation_id(operation.id().to_owned())
                            .joined(CauseRelation::CausedBy, checked.received);
                        operation
                            .progress(n, json!({"check_config": summary}))
                            .map_err(|cause| operation.interrupted(cause))?;
                        let operation_id = operation.id().to_owned();
                        operation.finish(&refusal)?;
                        return Answer::new(refusal)
                            .with("operation_id", &operation_id)?
                            .with("part", files::plain(&part))?
                            .with("check_config", summary)?
                            .with("next", ways_back(&paths.plain, &operation_id));
                    }
                    files::rename(&part, &config).map_err(|cause| operation.interrupted(cause))?;
                    check_config = summary.clone();
                    evidence = Some(json!({"check_config": summary}));
                }
            }
            StepKind::CreateStateRoot => {
                let intent: StateRootIntent = operation
                    .intent(n)
                    .map_err(|cause| operation.interrupted(cause))?;
                let folder = PathBuf::from(&intent.path);
                if !folder.is_dir() {
                    operation
                        .start(n)
                        .map_err(|cause| operation.interrupted(cause))?;
                    files::create_dir_all(&folder).map_err(|cause| operation.interrupted(cause))?;
                }
                evidence = Some(json!({"created": !intent.existed}));
            }
            StepKind::RecordState => {
                operation
                    .start(n)
                    .map_err(|cause| operation.interrupted(cause))?;
                let (config_sha256, _) = files::sha256_file(&paths.config()).map_err(|error| {
                    operation.interrupted(files::os_failure(&paths.config(), &error, IoOp::Read))
                })?;
                let operation_id = operation.id().to_owned();
                let recorded = InstallState {
                    live: Some(LiveBuild {
                        commit: commit.to_owned(),
                        operation_id: Some(operation_id.clone()),
                    }),
                    builds: vec![KeptBuild {
                        commit: commit.to_owned(),
                        tag: inputs.tag.clone(),
                        origin: inputs.origin,
                        operation_id,
                    }],
                    packs: Vec::new(),
                    config_sha256: Some(format!("sha256:{config_sha256}")),
                    ..InstallState::default()
                };
                state::write_state(&paths, &recorded)
                    .map_err(|cause| operation.interrupted(cause))?;
            }
            _ => return Err(invalid_journal(&operation)),
        }
        operation
            .done(n, evidence)
            .map_err(|cause| operation.interrupted(cause))?;
        n += 1;
    }
    let (config_sha256, _) = files::sha256_file(&paths.config()).map_err(|error| {
        operation.interrupted(files::os_failure(&paths.config(), &error, IoOp::Read))
    })?;
    let operation_id = operation.id().to_owned();
    let done = super::done(NEW)
        .install_operation_id(operation_id.clone())
        .commit_sha(inputs.commit.clone())
        .installation_changed(true);
    let done = super::plan::with_notes(done, &notes);
    operation.finish(&done)?;
    let lab = files::plain(&paths.root.join(Half::Tools.dir()).join(LAB_PROGRAM));
    let mut next = json!({
        "place": [lab, "place"],
        "instance_add": [lab, "instance", "add"],
        "up": [lab, "up"],
    });
    // A resumed operation asked for the watchdog: its registration follows.
    if inputs.watchdog
        && let Value::Object(next) = &mut next
    {
        next.insert(
            "watchdog_install".to_owned(),
            json!([lab, "watchdog", "install"]),
        );
    }
    Answer::new(done)
        .with("operation_id", &operation_id)?
        .with("root", &paths.plain)?
        .with("state_root", &inputs.state_root)?
        .with("config_sha256", format!("sha256:{config_sha256}"))?
        .with(
            "lab_options_enabled",
            if inputs.lab_authoring {
                vec![AUTHORING_MEMBER]
            } else {
                Vec::new()
            },
        )?
        .with(
            "build",
            json!({"commit": inputs.commit, "origin": inputs.origin, "tag": inputs.tag, "source": inputs.build}),
        )?
        .with("check_config", check_config)?
        .with("next", next)
}

/// The way back from an operation of this row that ended refused: its undo, which sets aside
/// everything it wrote (until LAB-7 deletes actingctl, `actingctl install undo`).
fn ways_back(root: &str, operation_id: &str) -> Value {
    json!({
        "actingctl_undo": ["actingctl", "install", "undo", "--root", root, "--operation", operation_id],
    })
}

/// `--watchdog`: the watchdog's task registered for the new root (the rescue row `watchdog
/// install`), after the operation. Its report is the answer's `watchdog`; one that failed is the
/// answer's top, the root's members beside it, and `next` names the row again.
fn with_watchdog(host: &dyn Host, paths: &Paths, mut answer: Answer) -> Answer {
    let answered = host.watchdog(crate::watchdog::Asked::Install, &paths.root);
    let mut report = match answered.line {
        Some(Value::Object(members)) => members,
        _ => Map::new(),
    };
    report.remove("outcome");
    if !answered.developer_before.is_empty() {
        report.insert(
            "warnings".to_owned(),
            Value::Array(
                answered
                    .developer_before
                    .iter()
                    .map(cli_line::warning)
                    .collect(),
            ),
        );
    }
    report.insert(
        "result".to_owned(),
        cli_line::warning_value(
            &answered.outcome,
            cli_line::detail().unwrap_or(actingcommand_contract::outcome::Detail::Full),
        ),
    );
    answer
        .members
        .insert("watchdog".to_owned(), Value::Object(report));
    let failed = answered.outcome.category() > actingcommand_contract::outcome::Category::Warning;
    // `next` names the registration (`run` names it for an operation that asked for it) only
    // while it is still to be done.
    if !failed && let Some(Value::Object(next)) = answer.members.get_mut("next") {
        next.remove("watchdog_install");
    }
    if failed {
        answer.outcome = answered
            .outcome
            .joined(CauseRelation::Related, answer.outcome.clone());
    }
    answer
}

/// `resume` of an unfinished operation of this row: under the writer lock, each step continued by
/// its postcondition; a first configuration not yet written gets its own salt then.
pub(crate) fn resume(operation: Operation<'_>) -> Result<Answer, Outcome> {
    let inputs: NewInputs = serde_json::from_value(operation.journal().inputs.clone())
        .map_err(|error| operation.interrupted(files::json_failure(&error)))?;
    let settings = settings().map_err(|cause| operation.interrupted(cause))?;
    let _lock = WriterLock::take(
        operation.paths(),
        settings.lock_wait_ms,
        InstallLocation::Resume,
    )
    .map_err(|cause| operation.interrupted(cause))?;
    run(operation, &inputs, &settings, Vec::new())
}

/// `undo` of an operation of this row, unfinished or the last finished one: under the writer lock
/// and with no Runtime on the root or its state root (the stopped rule: `owner_lock_held`,
/// `runtime_running`, `processes_unreadable`), every step it began set aside from the last: the
/// state file, the state root's folder when it made it, the configuration and its part, each
/// program file it placed, the kept build and its part. Nothing is deleted; the new root's folder,
/// its `install\` and the journal stay.
pub(crate) fn undo(
    mut operation: Operation<'_>,
    host: &dyn Host,
    face: Face,
) -> Result<Answer, Outcome> {
    const UNDO: InstallLocation = InstallLocation::Undo;
    let paths = operation.paths().clone();
    let inputs: NewInputs =
        serde_json::from_value(operation.journal().inputs.clone()).map_err(|error| {
            journal::state_invalid(
                UNDO,
                paths.journal(operation.id()),
                Some(files::json_failure(&error)),
            )
        })?;
    if operation.journal().undo.is_none() && operation.journal().finished() {
        let ids = journal::operation_ids(&paths)?;
        if ids.last().map(String::as_str) != Some(operation.id()) {
            return Err(super::unusable("--operation"));
        }
    }
    let settings = settings()?;
    let _lock = WriterLock::take(&paths, settings.lock_wait_ms, UNDO)?;
    super::launch::refuse_running(host, &paths, Path::new(&inputs.state_root), UNDO)?;
    let ms2 = operation.begin_undo(face.initiator())?;
    let steps = operation.journal().steps.clone();
    let previous = operation.journal().previous.clone();
    let mut kept = Vec::new();
    for step in steps.iter().rev() {
        if !matches!(step.state, StepState::Started | StepState::Done) {
            continue;
        }
        let evidence = match step.kind {
            StepKind::RecordState => super::undo::set_aside(&paths.state(), "rolledback", ms2)
                .map(|aside| aside.into_iter().collect::<Vec<_>>()),
            StepKind::WriteConfig | StepKind::CreateStateRoot => reverse_own(&operation, step, ms2),
            _ => super::undo::reverse_step(&operation, step, &previous, ms2),
        }
        .map_err(|cause| operation.interrupted(cause))?;
        kept.extend(evidence.iter().cloned());
        operation
            .undone(step.n, Some(json!({ "kept": evidence })))
            .map_err(|cause| operation.interrupted(cause))?;
    }
    let operation_id = operation.id().to_owned();
    let outcome = super::done(UNDO)
        .install_operation_id(operation_id.clone())
        .installation_changed(true);
    operation.finish_undo(&outcome)?;
    Answer::new(outcome)
        .with("operation_id", operation_id)?
        .with("kept", kept)
}

/// The reversal of this row's own step kinds (`install undo`'s table): `write_config` sets the
/// configuration aside as `actingd.config.json.rolledback-<ms2>` and its part as
/// `actingd.config.json.abandoned-<ms2>`; `create_state_root` sets the state root's folder aside
/// as `<folder>.rolledback-<ms2>` when the operation made it (one that was there, empty, stays).
pub(crate) fn reverse_own(
    operation: &Operation<'_>,
    step: &Step,
    ms2: u64,
) -> Result<Vec<String>, Outcome> {
    let paths = operation.paths();
    let mut kept = Vec::new();
    match step.kind {
        StepKind::WriteConfig => {
            let intent: ConfigIntent = operation.intent(step.n)?;
            kept.extend(super::undo::set_aside(&paths.config(), "rolledback", ms2)?);
            kept.extend(super::undo::set_aside(
                &paths.at(&intent.part),
                "abandoned",
                ms2,
            )?);
        }
        StepKind::CreateStateRoot => {
            let intent: StateRootIntent = operation.intent(step.n)?;
            if !intent.existed {
                kept.extend(super::undo::set_aside(
                    Path::new(&intent.path),
                    "rolledback",
                    ms2,
                )?);
            }
        }
        _ => {}
    }
    Ok(kept)
}
