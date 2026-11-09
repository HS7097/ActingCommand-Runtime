// SPDX-License-Identifier: AGPL-3.0-only

//! The rescue module's `config replace <file>` and `config restore <backup>` (Workflow #434
//! LAB-3; `contracts/lab-two-entries.md` §7; `contracts/install-cli.md`, "The Lab's rescue
//! module"): the root's configuration file replaced by another, as one journalled operation
//! (`kind` `config_replace`). The file's content is never printed.
//!
//! Before anything changes: the file is read (`--sha`, when given, must be its SHA-256) and the
//! live build's check-config (`--install-root <root>`) must pass it. Then, while a Runtime runs:
//! the hold and its guards (they report, never refuse), the formal close, the swap (the file
//! staged as `actingd.config.json.new-<ms>.part`, the live file kept as
//! `actingd.config.json.prev-<ms>`, the part renamed in and its SHA-256 checked), the start and
//! its post-start checks (the `config` check against the new file); the operation ends with the
//! hold kept, and the module reads `alive` and releases it (`crate::rescue`). Without a running
//! Runtime only the swap runs; the next start reads the new file.

use super::deploy::HoldStart;
use super::files;
use super::host::Host;
use super::journal::{Opening, Operation, PlannedStep, Previous, StepKind};
use super::manifest::Half;
use super::plan::Loaded;
use super::programs::{self, Deadline};
use super::state::{OperationKind, Paths};
use crate::codes::{InstallLocation, InstallRefusal};
use actingcommand_contract::outcome::{CauseRelation, IoOp, Outcome};
use actingcommand_lab_ops::initiator::Initiator;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// The configuration file of a single-location root.
const CONFIG_FILE: &str = "actingd.config.json";

/// The file a `config replace` or `config restore` puts in place, read and checked.
pub(crate) struct Candidate {
    /// Its path, resolved.
    pub(crate) path: PathBuf,
    /// Its SHA-256, lowercase hex.
    pub(crate) sha256: String,
    /// The live configuration's SHA-256.
    pub(crate) live_sha256: String,
}

/// Reads and checks `file` for the command `command`: `--sha`, when given, its SHA-256; the live
/// build's check-config passes it as the root's configuration. A file that cannot be resolved,
/// or whose SHA-256 is not `--sha`, is `actingctl_usage_invalid` (`unusable`); a check-config
/// that does not answer refuses `check_config_unanswered`, one that refuses the file
/// `config_refused_by_target`.
pub(crate) fn candidate(
    host: &dyn Host,
    paths: &Paths,
    loaded: &Loaded,
    command: InstallLocation,
    file: &Path,
    sha: Option<&str>,
) -> Result<Candidate, Outcome> {
    let path = std::fs::canonicalize(file).map_err(|error| {
        super::unusable("<file>").caused_by(files::os_failure(file, &error, IoOp::Canonicalize))
    })?;
    let (sha256, _) = files::sha256_file(&path).map_err(|error| {
        super::unusable("<file>").caused_by(files::os_failure(&path, &error, IoOp::Read))
    })?;
    if let Some(sha) = sha
        && !sha.eq_ignore_ascii_case(&sha256)
    {
        return Err(super::unusable("--sha"));
    }
    let live_sha256 = loaded
        .config_sha256
        .strip_prefix("sha256:")
        .unwrap_or(&loaded.config_sha256)
        .to_owned();
    let checked = programs::check_config(
        host,
        &paths.root.join(Half::Runtime.dir()),
        &path,
        Some(&paths.root),
        Deadline::check_config(&loaded.settings),
    )
    .map_err(|failure| {
        failure.onto(super::refusal(
            command,
            InstallRefusal::CheckConfigUnanswered,
        ))
    })?;
    if !checked.passed() {
        return Err(
            super::refusal(command, InstallRefusal::ConfigRefusedByTarget)
                .path(&path)
                .joined(CauseRelation::CausedBy, checked.received),
        );
    }
    Ok(Candidate {
        path,
        sha256,
        live_sha256,
    })
}

/// The steps: with a hold, the pause and the close first, and after the swap the start, its
/// checks and the release; the swap alone otherwise.
fn steps(
    candidate: &Candidate,
    commit: Option<&str>,
    hold: Option<(&HoldStart, &Loaded)>,
    ms: u64,
) -> Vec<PlannedStep> {
    let part = format!("{CONFIG_FILE}.new-{ms}.part");
    let kept = format!("{CONFIG_FILE}.prev-{ms}");
    let mut steps: Vec<PlannedStep> = Vec::new();
    if let Some((begun, loaded)) = hold {
        steps.push((
            StepKind::Pause,
            "runtime".to_owned(),
            super::hold::pause_intent(
                &begun.hold,
                &begun.held,
                loaded.settings.stop_drain_timeout_ms,
            ),
        ));
        steps.push((
            StepKind::Close,
            "runtime".to_owned(),
            json!({"shutdown_wait_ms": loaded.settings.stop_shutdown_wait_ms}),
        ));
    }
    steps.push((
        StepKind::StageFile,
        part.clone(),
        json!({
            "dst": CONFIG_FILE,
            "part": part,
            "source": files::plain(&candidate.path),
            "sha256": candidate.sha256,
        }),
    ));
    steps.push((
        StepKind::SwapFile,
        CONFIG_FILE.to_owned(),
        json!({
            "dst": CONFIG_FILE,
            "part": part,
            "old": kept,
            "old_sha256": candidate.live_sha256,
            "sha256": candidate.sha256,
        }),
    ));
    // The hold stays after the checks: the module reads `alive` first, then releases it.
    if hold.is_some() {
        let commit = json!({"commit": commit});
        steps.push((StepKind::Start, "runtime".to_owned(), commit.clone()));
        steps.push((StepKind::PostCheck, "runtime".to_owned(), commit));
    }
    steps
}

/// Opens the operation (`config_replace`) for `candidate`, with the hold `hold` (its S0, guards
/// and deferred record journalled) when a Runtime runs; the kept file's name is the inputs'
/// `backup`.
pub(crate) fn open<'h>(
    host: &'h dyn Host,
    paths: &Paths,
    loaded: &Loaded,
    candidate: &Candidate,
    hold: Option<&HoldStart>,
    initiator: Option<Initiator>,
) -> Result<Operation<'h>, Outcome> {
    let commit = loaded.state.live.as_ref().map(|live| live.commit.clone());
    let opening = Opening {
        kind: OperationKind::ConfigReplace,
        initiator,
        one_way: false,
        inputs: json!({
            "commit": commit,
            "file": files::plain(&candidate.path),
            // The `config` post-start check judges the file the operation puts in place.
            "config_sha256": format!("sha256:{}", candidate.sha256),
            "replaced_sha256": format!("sha256:{}", candidate.live_sha256),
            "hold": hold.map(|begun| begun.hold.reason.clone()),
            "known_pauses": hold.map(|begun| begun.hold.known.clone()),
        }),
        plan_id: None,
        plan: None,
        acknowledged: Vec::new(),
        s0: hold.map(|begun| super::hold::s0_of(&begun.seen, &begun.config)),
        previous: Previous {
            live: commit.clone(),
            state: None,
        },
    };
    let mut operation = Operation::open(host, paths, opening, |ms| {
        steps(
            candidate,
            commit.as_deref(),
            hold.map(|begun| (begun, loaded)),
            ms,
        )
    })?;
    // The kept file's name, by the operation's own milliseconds.
    let backup = files::plain(
        &paths
            .root
            .join(format!("{CONFIG_FILE}.prev-{}", operation.ms())),
    );
    operation
        .set_input("backup", Value::String(backup))
        .map_err(|cause| operation.interrupted(cause))?;
    if let Some(begun) = hold {
        operation
            .record_guards(super::guards::values(&begun.rows), &[])
            .map_err(|cause| operation.interrupted(cause))?;
        operation
            .progress(1, json!({"deferred": begun.deferred}))
            .map_err(|cause| operation.interrupted(cause))?;
    }
    Ok(operation)
}
