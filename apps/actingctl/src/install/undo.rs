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
use crate::codes::{ActingctlCode, InstallRefusal, RollbackGatePart};
use crate::watchdog::decide::Journal as OwnerJournal;
use crate::watchdog::observe;
use actingcommand_contract::outcome::Outcome;
use serde::Deserialize;
use serde_json::json;
use std::path::Path;

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
/// operation ended).
fn owner_started_since(state_root: &Path, since_unix_ms: u64) -> Result<bool, Outcome> {
    let (_, journal) = observe::owner_journal(state_root)?;
    Ok(match journal {
        OwnerJournal::Record { record, .. } => record.started_at_unix_ms > since_unix_ms,
        OwnerJournal::Absent | OwnerJournal::Unrecognised { .. } => false,
    })
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
            journal::refuse_unfinished(&paths)?;
            return Err(super::unusable("--operation"));
        }
        CurrentState::None => {}
    }
    let mut operation = Operation::load(host, &paths, operation_id)?;
    let kind = operation.journal().kind;
    let changes_program = matches!(kind, OperationKind::Update | OperationKind::Rollback);
    if operation.journal().finished() && operation.journal().undo.is_none() {
        // The last finished operation only.
        let ids = journal::operation_ids(&paths)?;
        if ids.last().map(String::as_str) != Some(operation_id) {
            return Err(super::unusable("--operation"));
        }
        if changes_program {
            let loaded = plan::load(&paths)?;
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
        let loaded = plan::load(&paths)?;
        Some(WriterLock::take(&paths, loaded.settings.lock_wait_ms)?)
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
        let moved: Moved = serde_json::from_value(step.intent.clone())
            .map_err(|error| operation.interrupted(files::json_failure(&error)))?;
        let evidence = match step.kind {
            StepKind::SwapFile => {
                let intent: SwapIntent = operation
                    .intent(step.n)
                    .map_err(|cause| operation.interrupted(cause))?;
                let dst = paths.at(&intent.dst);
                let mut evidence = Vec::new();
                let swapped = files::sha256_file(&dst)
                    .is_ok_and(|(actual, _)| actual == intent.sha256)
                    || (!dst.exists() && intent.old.is_some());
                if swapped {
                    if let Some(aside) = set_aside(&dst, "rolledback", ms2)
                        .map_err(|cause| operation.interrupted(cause))?
                    {
                        evidence.push(aside);
                    }
                    if let Some(old) = &intent.old {
                        let old = paths.at(old);
                        if old.exists() && !dst.exists() {
                            files::rename(&old, &dst)
                                .map_err(|cause| operation.interrupted(cause))?;
                        }
                    }
                }
                // The part, when the swap never took it in.
                if let Some(aside) = set_aside(&paths.at(&intent.part), "abandoned", ms2)
                    .map_err(|cause| operation.interrupted(cause))?
                {
                    evidence.push(aside);
                }
                evidence
            }
            StepKind::StageFile | StepKind::Extract | StepKind::CopyBuild => {
                let part = moved.part.as_deref().map(|part| paths.at(part));
                match part {
                    Some(part) => set_aside(&part, "abandoned", ms2)
                        .map_err(|cause| operation.interrupted(cause))?
                        .into_iter()
                        .collect(),
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
                    Some(build) if !kept_before => set_aside(&paths.at(build), "rolledback", ms2)
                        .map_err(|cause| operation.interrupted(cause))?
                        .into_iter()
                        .collect(),
                    _ => Vec::new(),
                }
            }
            StepKind::RecordState => {
                if let Some(state) = &previous.state {
                    state::write_state(&paths, state)
                        .map_err(|cause| operation.interrupted(cause))?;
                }
                Vec::new()
            }
            StepKind::VerifyTree | StepKind::LedgerVerify => Vec::new(),
        };
        kept.extend(evidence.iter().cloned());
        operation
            .undone(step.n, Some(json!({ "kept": evidence })))
            .map_err(|cause| operation.interrupted(cause))?;
    }
    if changes_program && let Some(previous_commit) = &previous.live {
        match manifest::verify_build(&paths.root) {
            Ok(build) if build.commit == *previous_commit => {}
            _ => {
                let cause = super::adopt::verify_live(&paths).err().unwrap_or_else(|| {
                    plan::refused(InstallRefusal::LiveBuildInvalid).path(&paths.root)
                });
                return Err(operation.interrupted(cause));
            }
        }
    }
    let operation_id = operation.id().to_owned();
    let outcome = Outcome::new(ActingctlCode::InstallOperationUndone)
        .install_operation_id(operation_id.clone());
    operation.finish_undo(&outcome)?;
    Answer::new(outcome)
        .with("operation_id", operation_id)?
        .with("kept", kept)
}

/// The rollback gate toward the build before the operation, for the undo of a finished update
/// or rollback after a Runtime owner started: its refusal, or `Ok` when it passes.
fn refuse_by_gate(
    host: &dyn Host,
    paths: &Paths,
    journal: &journal::Journal,
) -> Result<(), Outcome> {
    let Some(previous) = journal.previous.live.clone() else {
        return Err(Outcome::new(ActingctlCode::InstallRollbackRefused)
            .rollback_gate_parts([RollbackGatePart::InterfaceEdges]));
    };
    let inputs = PlanInputs {
        kind: PlanKind::Rollback,
        commit: previous,
        accept_ledger_one_way: false,
        preview: false,
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
