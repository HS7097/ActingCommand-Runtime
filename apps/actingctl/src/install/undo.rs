// SPDX-License-Identifier: AGPL-3.0-only

//! `actingctl install undo --root <root> --operation <id>` (Workflow #421 INST-1;
//! `contracts/install-cli.md`, "Undo and resume", "Steps"): reverses an unfinished operation, or
//! the last finished one when no operation followed it. A finished update or rollback is undone
//! only when the rollback gate passes, trivially when no Runtime owner started since it ended.
//! Each started or done step is reversed from the last: a part renamed
//! `<name>.abandoned-<ms2>`, a swapped file renamed `<dst>.rolledback-<ms2>` and its `.old-<ms>`
//! put back, a placed build renamed aside, the previous state rewritten. Nothing is deleted.

use super::files;
use super::host::Host;
use super::journal::{self, Operation, StepKind, StepState};
use super::lock::WriterLock;
use super::manifest;
use super::plan::{self, PlanInputs, PlanKind};
use super::state::{self, CurrentState, OperationKind, Paths};
use super::update::{self, SwapIntent};
use super::{Answer, Flags};
use crate::codes::{InstallLocation, InstallRefusal, RollbackGatePart};
use crate::watchdog::decide::{Journal as OwnerJournal, OwnerLock};
use crate::watchdog::observe;
use actingcommand_contract::outcome::Outcome;
use serde::Deserialize;
use serde_json::json;
use std::path::Path;

/// The command of every outcome here.
const UNDO: InstallLocation = InstallLocation::Undo;

/// The intent members the reversal reads; the others are ignored.
#[derive(Debug, Deserialize)]
struct Moved {
    #[serde(default)]
    part: Option<String>,
    #[serde(default)]
    build: Option<String>,
    #[serde(default)]
    commit: Option<String>,
}

/// `<path>` renamed `<path-without-.new-<ms>.part>.<marker>-<ms2>`, when it exists.
fn set_aside(path: &Path, marker: &str, ms: u64) -> Result<Option<String>, Outcome> {
    if !path.exists() {
        return Ok(None);
    }
    let name = path
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    let base = name.split(".new-").next().unwrap_or(&name).to_owned();
    let mut ms = ms;
    let aside = loop {
        let candidate = path.with_file_name(format!("{base}.{marker}-{ms}"));
        if !candidate.exists() {
            break candidate;
        }
        ms += 1;
    };
    files::rename(path, &aside)?;
    Ok(Some(files::plain(&aside)))
}

/// Whether an owner of the state root started after `since_unix_ms` (an epoch since the
/// operation ended). An owner journal that cannot be read, or whose record this installer does
/// not recognise, refuses `processes_unreadable`, and one a running owner holds locked refuses
/// `owner_lock_held` (review S1 F6): neither is taken for "no owner started".
fn owner_started_since(state_root: &Path, since_unix_ms: u64) -> Result<bool, Outcome> {
    let unreadable = || super::refusal(UNDO, InstallRefusal::ProcessesUnreadable).path(state_root);
    let (lock, journal) =
        observe::owner_journal(state_root).map_err(|cause| unreadable().caused_by(cause))?;
    if lock == OwnerLock::Locked {
        return Err(super::refusal(UNDO, InstallRefusal::OwnerLockHeld).path(state_root));
    }
    match journal {
        OwnerJournal::Record { record, .. } => Ok(record.started_at_unix_ms > since_unix_ms),
        OwnerJournal::Absent => Ok(false),
        OwnerJournal::Unrecognised { failure } => Err(unreadable().caused_by(failure)),
    }
}

/// `install undo`.
pub(crate) fn undo(words: &[&str], host: &dyn Host) -> Result<Answer, Outcome> {
    let flags = Flags::parse(words, &["--root", "--operation"], &[])?;
    let paths = super::resolve_root(&flags)?;
    let operation_id = flags.required("--operation")?;
    if journal::operation_ms(operation_id).is_none() || !paths.journal(operation_id).is_file() {
        return Err(super::unusable("--operation"));
    }
    match state::read_current(&paths) {
        CurrentState::Unfinished(current) if current.operation_id == operation_id => {}
        CurrentState::Unfinished(_) | CurrentState::Unreadable(_) => {
            journal::refuse_unfinished(UNDO, &paths)?;
            return Err(super::unusable("--operation"));
        }
        CurrentState::None => {}
    }
    let mut operation = Operation::load(host, &paths, operation_id, UNDO)?;
    let kind = operation.journal().kind;
    let changes_program = matches!(kind, OperationKind::Update | OperationKind::Rollback);
    // Workflow #421 INST-2: a composite update starts the Runtime; its program files are never
    // reversed under a running actingd.
    if changes_program {
        let images = host.processes_under(&paths.root).map_err(|cause| {
            super::refusal(UNDO, InstallRefusal::ProcessesUnreadable).caused_by(cause)
        })?;
        let daemon = format!("\\{}", super::programs::DAEMON);
        if let Some(image) = images
            .iter()
            .find(|image| image.path.to_ascii_lowercase().ends_with(&daemon))
        {
            return Err(super::refusal(UNDO, InstallRefusal::RuntimeRunning)
                .path(&image.path)
                .pid(image.pid));
        }
    }
    if operation.journal().finished() && operation.journal().undo.is_none() {
        // The last finished operation only.
        let ids = journal::operation_ids(&paths)?;
        if ids.last().map(String::as_str) != Some(operation_id) {
            return Err(super::unusable("--operation"));
        }
        if changes_program {
            let loaded = plan::load(&paths, UNDO)?;
            let installed_here = loaded
                .state
                .live
                .as_ref()
                .and_then(|live| live.operation_id.as_deref())
                == Some(operation_id);
            if !installed_here {
                return Err(super::unusable("--operation"));
            }
            let ended = operation.journal().finished_at_unix_ms.unwrap_or(0);
            if owner_started_since(&loaded.state_root, ended)? {
                refuse_by_gate(host, &paths, operation.journal())?;
            }
        }
    }
    let _lock = if changes_program {
        let loaded = plan::load(&paths, UNDO)?;
        Some(WriterLock::take(
            &paths,
            loaded.settings.lock_wait_ms,
            UNDO,
        )?)
    } else {
        None
    };
    let ms2 = operation.begin_undo()?;
    let mut kept = Vec::new();
    let steps = operation.journal().steps.clone();
    let previous = operation.journal().previous.clone();
    for step in steps.iter().rev() {
        if !matches!(step.state, StepState::Started | StepState::Done) {
            continue;
        }
        let evidence = reverse_step(&operation, step, &previous, ms2)
            .map_err(|cause| operation.interrupted(cause))?;
        kept.extend(evidence.iter().cloned());
        operation
            .undone(step.n, Some(json!({ "kept": evidence })))
            .map_err(|cause| operation.interrupted(cause))?;
    }
    if changes_program && let Some(previous_commit) = &previous.live {
        match manifest::verify_build(&paths.root) {
            Ok(build) if build.commit == *previous_commit => {}
            _ => {
                let cause = super::adopt::verify_live(UNDO, &paths)
                    .err()
                    .unwrap_or_else(|| {
                        super::refusal(UNDO, InstallRefusal::LiveBuildInvalid).path(&paths.root)
                    });
                return Err(operation.interrupted(cause));
            }
        }
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

/// Reverses one started or done step of `operation` (`install undo`'s table, and an automatic
/// rollback's reversal, Workflow #421 INST-2): the paths it set aside, each kept. `ms2` names
/// what it moves aside.
pub(crate) fn reverse_step(
    operation: &Operation<'_>,
    step: &journal::Step,
    previous: &journal::Previous,
    ms2: u64,
) -> Result<Vec<String>, Outcome> {
    let paths = operation.paths().clone();
    let moved: Moved =
        serde_json::from_value(step.intent.clone()).map_err(|error| files::json_failure(&error))?;
    Ok(match step.kind {
        StepKind::SwapFile => {
            let intent: SwapIntent = operation.intent(step.n)?;
            let dst = paths.at(&intent.dst);
            let mut evidence = Vec::new();
            let swapped = files::sha256_file(&dst).is_ok_and(|(actual, _)| actual == intent.sha256)
                || (!dst.exists() && intent.old.is_some());
            if swapped {
                if let Some(aside) = set_aside(&dst, "rolledback", ms2)? {
                    evidence.push(aside);
                }
                if let Some(old) = &intent.old {
                    let old = paths.at(old);
                    if old.exists() && !dst.exists() {
                        files::rename(&old, &dst)?;
                    }
                }
            }
            // The part, when the swap never took it in.
            if let Some(aside) = set_aside(&paths.at(&intent.part), "abandoned", ms2)? {
                evidence.push(aside);
            }
            evidence
        }
        StepKind::StageFile | StepKind::Extract | StepKind::CopyBuild => {
            let part = moved.part.as_deref().map(|part| paths.at(part));
            match part {
                Some(part) => set_aside(&part, "abandoned", ms2)?.into_iter().collect(),
                None => Vec::new(),
            }
        }
        StepKind::PlaceBuild => {
            let kept_before = previous.state.as_ref().is_some_and(|state| {
                moved
                    .commit
                    .as_deref()
                    .is_some_and(|commit| state.kept(commit).is_some())
            });
            match moved.build.as_deref() {
                Some(build) if !kept_before => set_aside(&paths.at(build), "rolledback", ms2)?
                    .into_iter()
                    .collect(),
                _ => Vec::new(),
            }
        }
        StepKind::RecordState => {
            if let Some(state) = &previous.state {
                state::write_state(&paths, state)?;
            }
            Vec::new()
        }
        // Workflow #421 INST-2: the hold, the close, the start, the checks, the release and
        // a rollback's reversal change no file; a pause stays until `release-hold`.
        StepKind::VerifyTree
        | StepKind::LedgerVerify
        | StepKind::Pause
        | StepKind::Close
        | StepKind::Start
        | StepKind::PostCheck
        | StepKind::ReleaseHold
        | StepKind::ReverseFiles => Vec::new(),
    })
}

/// The rollback gate toward the build before the operation, for the undo of a finished update
/// or rollback after a Runtime owner started: its refusal (`rollback_gate_failed` of `install
/// undo`), or `Ok` when it passes.
fn refuse_by_gate(
    host: &dyn Host,
    paths: &Paths,
    journal: &journal::Journal,
) -> Result<(), Outcome> {
    let Some(previous) = journal.previous.live.clone() else {
        return Err(super::refusal(UNDO, InstallRefusal::RollbackGateFailed)
            .rollback_gate_parts([RollbackGatePart::InterfaceEdges]));
    };
    let inputs = PlanInputs {
        kind: PlanKind::Rollback,
        commit: previous,
        accept_ledger_one_way: false,
        preview: false,
        operation: UNDO,
    };
    let planned = match plan::plan(host, paths, inputs) {
        Ok(planned) => planned,
        Err(refused) => return Err(refused.into_answer().outcome),
    };
    let gate = update::gate(host, paths, &planned);
    match gate.refusal(&planned) {
        Some(refusal) => Err(refusal),
        None => Ok(()),
    }
}
