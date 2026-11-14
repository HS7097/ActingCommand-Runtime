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
//!
//! Workflow #421 INST-5 (first slice): `instance add` commits the candidate it composed the same
//! way, as one `config_replace` operation ([`Staged`]): the candidate, written beside the live file
//! as `actingd.config.json.new-<ms>.part`, is the swap's own part (so no copy of it stays), and the
//! journal's inputs record what was added (`instance_add`).

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

/// Workflow #421 INST-5: a candidate `instance add` composed and staged itself beside the live
/// file. The swap takes it as its part, `part` (a file name in the root), so nothing more is
/// staged; `added` is what the command added, journalled as the input [`ADDED_INPUT`].
pub(crate) struct Staged {
    pub(crate) part: String,
    pub(crate) added: Value,
}

/// The input of a `config_replace` journal that records the instance `instance add` added.
pub(crate) const ADDED_INPUT: &str = "instance_add";

/// The input of a `config_replace` journal that names the file the swap keeps the live one as.
const BACKUP_INPUT: &str = "backup";

/// Workflow #453 S3 fix 1 (the coordinator's answer 3): whether the `config_replace` `operation`
/// began while no Runtime answered (`config replace`, `config restore` or `instance add` while the
/// Runtime is stopped): its journal holds no hold and no S0. Its `resume` and `undo` apply the
/// stopped rule before anything is moved, as those of a placement begun while stopped do.
pub(crate) fn is_cold(operation: &Operation<'_>) -> bool {
    let journal = operation.journal();
    journal.s0.is_none() && journal.inputs.get("hold").is_none_or(Value::is_null)
}

/// Workflow #453 S3 fix 1: the state root and the writer lock's wait (`install.lock_wait_ms`) the
/// undo of a cold `config_replace` `operation` judges by: from the live configuration, or, when
/// there is none (a swap stopped between its two renames), from the file the swap kept it as (the
/// input `backup`), so that the undo still puts that file back. A file that cannot be read refuses
/// `config_unreadable` of `command`, an `install` section that does not read
/// `install_section_invalid`.
pub(crate) fn stopped_target(
    paths: &Paths,
    operation: &Operation<'_>,
    command: InstallLocation,
) -> Result<(PathBuf, u64), Outcome> {
    let live = paths.config();
    let path = if live.exists() {
        live
    } else {
        operation
            .journal()
            .inputs
            .get(BACKUP_INPUT)
            .and_then(Value::as_str)
            .map_or(live, PathBuf::from)
    };
    let unreadable = || super::refusal(command, InstallRefusal::ConfigUnreadable).path(&path);
    let config = files::read_json::<Value>(&path).map_err(|cause| unreadable().caused_by(cause))?;
    let settings = actingcommand_runtime_config::install::settings(&config).map_err(|failure| {
        super::refusal(command, InstallRefusal::InstallSectionInvalid)
            .path(&path)
            .caused_by(failure.outcome())
    })?;
    let state_root = config
        .get(super::plan::STATE_ROOT_MEMBER)
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .filter(|root| root.is_absolute())
        .ok_or_else(unreadable)?;
    Ok((state_root, settings.lock_wait_ms))
}

/// The steps: with a hold, the pause and the close first, and after the swap the start, its
/// checks and the release; the swap alone otherwise. The part is `staged`'s when given (INST-5).
fn steps(
    candidate: &Candidate,
    commit: Option<&str>,
    hold: Option<(&HoldStart, &Loaded)>,
    staged: Option<&Staged>,
    ms: u64,
) -> Vec<PlannedStep> {
    let part = staged.map_or_else(
        || format!("{CONFIG_FILE}.new-{ms}.part"),
        |staged| staged.part.clone(),
    );
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
/// `backup`. Workflow #421 INST-5: `staged`, `instance add`'s candidate already in its part, with
/// what it added as the input [`ADDED_INPUT`].
pub(crate) fn open<'h>(
    host: &'h dyn Host,
    paths: &Paths,
    loaded: &Loaded,
    candidate: &Candidate,
    hold: Option<&HoldStart>,
    initiator: Option<Initiator>,
    staged: Option<&Staged>,
) -> Result<Operation<'h>, Outcome> {
    let commit = loaded.state.live.as_ref().map(|live| live.commit.clone());
    let mut inputs = json!({
        "commit": commit,
        "file": files::plain(&candidate.path),
        // The `config` post-start check judges the file the operation puts in place.
        "config_sha256": format!("sha256:{}", candidate.sha256),
        "replaced_sha256": format!("sha256:{}", candidate.live_sha256),
        "hold": hold.map(|begun| begun.hold.reason.clone()),
        "known_pauses": hold.map(|begun| begun.hold.known.clone()),
    });
    if let Some(staged) = staged {
        inputs[ADDED_INPUT] = staged.added.clone();
    }
    let opening = Opening {
        kind: OperationKind::ConfigReplace,
        initiator,
        one_way: false,
        inputs,
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
            staged,
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
        .set_input(BACKUP_INPUT, Value::String(backup))
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
