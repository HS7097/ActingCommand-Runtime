// SPDX-License-Identifier: AGPL-3.0-only

//! `actingctl install adopt --root <root>` (Workflow #421 INST-1; `contracts/install-cli.md`,
//! "Adopt"): the first command on a root that scripts installed. The live `runtime\` and
//! `tools\` are verified against their own manifests, exactly the listed files are kept as
//! `install\builds\<live commit>`, and `state.json` records the live build, the pack register
//! and the configuration's SHA-256.

use super::files::{self, join_relative};
use super::journal::{self, Opening, Operation, PlannedStep, Previous, StepKind, StepState};
use super::manifest::{self, BuildManifest, BuildProblem, Half};
use super::state::{self, BuildOrigin, KeptBuild, LiveBuild, OperationKind, PackEntry, Paths};
use super::{Answer, Flags, Host};
use crate::codes::{InstallLocation, InstallRefusal};
use actingcommand_contract::outcome::{IoOp, Outcome};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// The names of kept items a standard-pack root may hold beside its game roots.
const KEPT_MARKERS: [&str; 4] = [".old-", ".abandoned-", ".rolledback-", ".part"];
const BUNDLE_FILE: &str = "bundle.json";

/// The refusal of `install adopt` with `refusal`.
fn refusal(refusal: InstallRefusal) -> Outcome {
    super::refusal(InstallLocation::Adopt, refusal)
}

/// The refusal `live_build_modified` (`count`, `files`) of the command `operation`.
fn live_build_modified(operation: InstallLocation, problems: &[manifest::FileProblem]) -> Outcome {
    super::refusal(operation, InstallRefusal::LiveBuildModified)
        .count(problems.len())
        .files(problems.iter().map(manifest::FileProblem::record))
}

/// The live build verified against its own manifests, for the command `operation`: the refusal
/// `live_build_invalid` when a manifest cannot be read or the two name different commits,
/// `live_build_modified` (`count`, `files`) when a listed file differs.
pub(crate) fn verify_live(
    operation: InstallLocation,
    paths: &Paths,
) -> Result<manifest::Build, Outcome> {
    manifest::verify_build(&paths.root).map_err(|problem| match problem {
        BuildProblem::Manifest(half, cause) => {
            let outcome = super::refusal(operation, InstallRefusal::LiveBuildInvalid)
                .path(paths.root.join(half.dir()).join(manifest::MANIFEST_FILE));
            match cause {
                Some(cause) => outcome.caused_by(cause),
                None => outcome,
            }
        }
        BuildProblem::Commits => {
            super::refusal(operation, InstallRefusal::LiveBuildInvalid).path(&paths.root)
        }
        BuildProblem::Files(problems) => live_build_modified(operation, &problems),
    })
}

/// The configuration's SHA-256 (`sha256:<hex>`) and its document; both `None` without a
/// configuration file.
fn read_config(paths: &Paths) -> Result<Option<(String, Value)>, Outcome> {
    let path = paths.config();
    if !path.exists() {
        return Ok(None);
    }
    let unreadable = |cause: Outcome| {
        refusal(InstallRefusal::ConfigUnreadable)
            .path(&path)
            .caused_by(cause)
    };
    let bytes = files::read_all(&path).map_err(unreadable)?;
    let document = serde_json::from_slice(files::without_bom(&bytes))
        .map_err(|error| unreadable(files::json_failure(&error)))?;
    Ok(Some((
        format!("sha256:{}", files::sha256_bytes(&bytes)),
        document,
    )))
}

/// The pack register of the configuration's standard-pack root (`standard_packs.root`, relative
/// to the root): one entry per game root holding a `bundle.json`, with the bundle's source
/// repository and commit, the declaration unknown (no digest, option 丙). Kept items
/// (`.old-<ms>` and the like) are no game roots.
fn read_packs(paths: &Paths, config: Option<&Value>) -> Result<Vec<PackEntry>, Outcome> {
    let Some(root) = config
        .and_then(|config| config.get("standard_packs"))
        .and_then(|section| section.get("root"))
        .and_then(Value::as_str)
    else {
        return Ok(Vec::new());
    };
    let packs_root = if Path::new(root).is_absolute() {
        PathBuf::from(root)
    } else {
        paths.root.join(root)
    };
    let entries = match std::fs::read_dir(&packs_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(refusal(InstallRefusal::ConfigUnreadable)
                .path(&packs_root)
                .caused_by(files::os_failure(&packs_root, &error, IoOp::ReadDir)));
        }
    };
    let mut packs = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            refusal(InstallRefusal::ConfigUnreadable)
                .path(&packs_root)
                .caused_by(files::os_failure(&packs_root, &error, IoOp::ReadDir))
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if KEPT_MARKERS.iter().any(|marker| name.contains(marker)) {
            continue;
        }
        let bundle = entry.path().join(BUNDLE_FILE);
        if !bundle.is_file() {
            continue;
        }
        let document = files::read_json::<Value>(&bundle).ok();
        let text = |pointer: &str| {
            document
                .as_ref()
                .and_then(|document| document.pointer(pointer))
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        packs.push(PackEntry {
            game: text("/game").unwrap_or(name),
            repository: text("/source/repository"),
            commit: text("/source/commit"),
            tag: None,
            package_range: None,
        });
    }
    packs.sort_by(|left, right| left.game.cmp(&right.game));
    Ok(packs)
}

/// The steps' intent: the live build and its part folder.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdoptIntent {
    commit: String,
    /// `install/builds/<commit>.new-<ms>.part`.
    part: String,
    /// `install/builds/<commit>`.
    build: String,
}

fn adopt_steps(commit: &str, ms: u64) -> Vec<PlannedStep> {
    let part = format!("install/builds/{commit}.new-{ms}.part");
    let build = format!("install/builds/{commit}");
    let intent = serde_json::json!({"commit": commit, "part": part, "build": build});
    vec![
        (StepKind::CopyBuild, part.clone(), intent.clone()),
        (StepKind::PlaceBuild, build.clone(), intent.clone()),
        (
            StepKind::RecordState,
            "install/state.json".to_owned(),
            intent,
        ),
    ]
}

/// `install adopt --root <root>`.
pub(crate) fn adopt(words: &[&str], host: &dyn Host) -> Result<Answer, Outcome> {
    let flags = Flags::parse(words, &["--root"], &[])?;
    let paths = super::resolve_root(&flags)?;
    super::refuse_ab(InstallLocation::Adopt, &paths)?;
    if !paths.is_single_location() {
        return Err(refusal(InstallRefusal::NotSingleLocation).path(&paths.root));
    }
    let live = verify_live(InstallLocation::Adopt, &paths)?;
    read_config(&paths)?;
    let previous = state::load_state_or_default(&paths).map_err(|cause| {
        journal::state_invalid(InstallLocation::Adopt, paths.state(), Some(cause))
    })?;
    let build = paths.build(&live.commit);
    let kept = manifest::verify_build(&build).is_ok_and(|kept| kept.commit == live.commit);
    if kept
        && previous.kept(&live.commit).is_some()
        && previous
            .live
            .as_ref()
            .is_some_and(|current| current.commit == live.commit)
    {
        return Answer::new(
            super::done(InstallLocation::Adopt)
                .commit_sha(live.commit.clone())
                .installation_changed(false),
        )
        .with("commit", live.commit.as_str())?
        .with("unchanged", true)?
        .with("build", files::plain(&build));
    }
    if build.exists() && !kept {
        return Err(refusal(InstallRefusal::BuildInvalid)
            .commit_sha(live.commit.clone())
            .path(&build));
    }
    let opening = Opening {
        kind: OperationKind::Adopt,
        inputs: Value::Object(serde_json::Map::new()),
        plan_id: None,
        plan: None,
        acknowledged: Vec::new(),
        s0: None,
        previous: Previous {
            live: previous.live.as_ref().map(|live| live.commit.clone()),
            state: Some(previous),
        },
    };
    let commit = live.commit.clone();
    let operation = Operation::open(host, &paths, opening, |ms| adopt_steps(&commit, ms))?;
    run(operation, &live)
}

/// Continues an unfinished adopt: the live build is verified again first.
pub(crate) fn resume(operation: Operation<'_>) -> Result<Answer, Outcome> {
    let live = verify_live(InstallLocation::Adopt, operation.paths())
        .map_err(|cause| operation.interrupted(cause))?;
    run(operation, &live)
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

fn run(mut operation: Operation<'_>, live: &manifest::Build) -> Result<Answer, Outcome> {
    let intent: AdoptIntent = operation
        .intent(1)
        .map_err(|cause| operation.interrupted(cause))?;
    let paths = operation.paths().clone();
    let part = paths.at(&intent.part);
    let build = paths.at(&intent.build);
    let placed = || manifest::verify_build(&build).is_ok_and(|kept| kept.commit == live.commit);
    // 1. copy_build: the listed files into the part folder, verified there.
    if operation.journal().steps[0].state != StepState::Done {
        if !placed() {
            operation
                .start(1)
                .map_err(|cause| operation.interrupted(cause))?;
            for half in [Half::Runtime, Half::Tools] {
                copy_half(
                    &paths.root.join(half.dir()),
                    &part.join(half.dir()),
                    live.manifest(half),
                )
                .map_err(|cause| operation.interrupted(cause))?;
            }
            if let Err(problem) = manifest::verify_build(&part) {
                // A live file changed while it was copied: the copy is abandoned and the adopt
                // ends refused, as a changed live file refuses it.
                let refused = match problem {
                    BuildProblem::Files(problems) => {
                        live_build_modified(InstallLocation::Adopt, &problems)
                    }
                    BuildProblem::Manifest(..) | BuildProblem::Commits => {
                        refusal(InstallRefusal::LiveBuildInvalid).path(&paths.root)
                    }
                };
                let abandoned =
                    part.with_file_name(format!("{}.abandoned-{}", live.commit, operation.ms()));
                files::rename(&part, &abandoned).map_err(|cause| operation.interrupted(cause))?;
                operation
                    .undone(
                        1,
                        Some(serde_json::json!({ "abandoned": files::plain(&abandoned) })),
                    )
                    .map_err(|cause| operation.interrupted(cause))?;
                operation.finish(&refused)?;
                return Err(refused);
            }
        }
        operation
            .done(1, None)
            .map_err(|cause| operation.interrupted(cause))?;
    }
    // 2. place_build
    if operation.journal().steps[1].state != StepState::Done {
        if !placed() {
            operation
                .start(2)
                .map_err(|cause| operation.interrupted(cause))?;
            files::rename(&part, &build).map_err(|cause| operation.interrupted(cause))?;
        }
        operation
            .done(2, None)
            .map_err(|cause| operation.interrupted(cause))?;
    }
    // 3. record_state: the live build, the kept build, the pack register, the configuration.
    let mut packs_count = 0;
    if operation.journal().steps[2].state != StepState::Done {
        operation
            .start(3)
            .map_err(|cause| operation.interrupted(cause))?;
        let config = read_config(&paths).map_err(|cause| operation.interrupted(cause))?;
        let packs = read_packs(&paths, config.as_ref().map(|(_, document)| document))
            .map_err(|cause| operation.interrupted(cause))?;
        packs_count = packs.len();
        let mut state =
            state::load_state_or_default(&paths).map_err(|cause| operation.interrupted(cause))?;
        if state.kept(&live.commit).is_none() {
            state = state.with_build(KeptBuild {
                commit: live.commit.clone(),
                tag: None,
                origin: BuildOrigin::Adopted,
                operation_id: operation.id().to_owned(),
            });
        }
        state.live = Some(LiveBuild {
            commit: live.commit.clone(),
            operation_id: None,
        });
        state.packs = packs;
        state.config_sha256 = config.map(|(sha256, _)| sha256);
        state::write_state(&paths, &state).map_err(|cause| operation.interrupted(cause))?;
        operation
            .done(3, None)
            .map_err(|cause| operation.interrupted(cause))?;
    }
    let operation_id = operation.id().to_owned();
    let outcome = super::done(InstallLocation::Adopt)
        .commit_sha(live.commit.clone())
        .install_operation_id(operation_id.clone())
        .installation_changed(true);
    operation.finish(&outcome)?;
    Answer::new(outcome)
        .with("commit", live.commit.as_str())?
        .with("unchanged", false)?
        .with("operation_id", operation_id)?
        .with("build", files::plain(&build))?
        .with("packs", packs_count)
}
