// SPDX-License-Identifier: AGPL-3.0-only

//! `actingctl install undo --root <root> --operation <id>` (Workflow #421 INST-1;
//! `contracts/install-cli.md`, "Undo and resume", "Steps"): reverses an unfinished operation, or
//! the last finished one when no operation followed it. An update or rollback, finished or not,
//! in which a Runtime of its own build has started, or after which an owner started, is undone
//! only when a rollback to the build before it would pass: the interface edges, then the whole
//! rollback gate (register §1.242). Each started or done step is reversed from the last: a part
//! renamed
//! `<name>.abandoned-<ms2>`, a swapped file renamed `<dst>.rolledback-<ms2>` and its `.old-<ms>`
//! put back, a placed build renamed aside, the previous state rewritten. Nothing is deleted.

use super::deploy::of_rollback;
use super::files;
use super::host::Host;
use super::journal::{self, Operation, StepKind, StepState};
use super::lock::WriterLock;
use super::manifest;
use super::plan::{self, PlanInputs, PlanKind};
use super::state::{self, CurrentState, OperationKind, Paths};
use super::update::{self, SwapIntent};
use super::{Answer, FlagLists, Flags};
use crate::codes::{InstallLocation, InstallRefusal, RollbackGatePart};
use crate::watchdog::decide::{Journal as OwnerJournal, OwnerLock};
use crate::watchdog::observe;
use actingcommand_contract::outcome::Outcome;
use serde::Deserialize;
use serde_json::{Value, json};
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
pub(crate) fn set_aside(path: &Path, marker: &str, ms: u64) -> Result<Option<String>, Outcome> {
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

/// When the last owner of the state root started; `None` when its journal names none. An owner
/// journal that cannot be read, or whose record this installer does not recognise, refuses
/// `processes_unreadable`, and one a running owner holds locked refuses `owner_lock_held` (review
/// S1 F6): neither is taken for "no owner started".
fn owner_started_at(state_root: &Path) -> Result<Option<u64>, Outcome> {
    let unreadable = || super::refusal(UNDO, InstallRefusal::ProcessesUnreadable).path(state_root);
    let (lock, journal) =
        observe::owner_journal(state_root).map_err(|cause| unreadable().caused_by(cause))?;
    if lock == OwnerLock::Locked {
        return Err(super::refusal(UNDO, InstallRefusal::OwnerLockHeld).path(state_root));
    }
    match journal {
        OwnerJournal::Record { record, .. } => Ok(Some(record.started_at_unix_ms)),
        OwnerJournal::Absent => Ok(None),
        OwnerJournal::Unrecognised { failure } => Err(unreadable().caused_by(failure)),
    }
}

/// Whether a start step's evidence shows a process spawned: an attempt that came ready or ended,
/// a progress sample of a process under way, or an owner a resumed start found. An attempt whose
/// spawn failed, one the memory guard stopped and a step skipped for a rollback show none.
fn spawned(evidence: &Value) -> bool {
    !evidence["ready"].is_null()
        || evidence["samples"]
            .as_array()
            .is_some_and(|samples| !samples.is_empty())
        || evidence["attempts"].as_array().is_some_and(|attempts| {
            attempts
                .iter()
                .any(|attempt| attempt.get("ready").is_some() || attempt.get("ended").is_some())
        })
}

/// Register §1.242: whether this operation started a Runtime of the build it installed (an
/// automatic rollback's start of the previous build does not count). A start step with no evidence
/// is one refused before its spawn when the operation ended (`launch::start`'s error), and one
/// that may have spawned when it did not (a crash before its first progress): that is not taken
/// for "none started".
fn started_inside(journal: &journal::Journal) -> bool {
    journal
        .steps
        .iter()
        .filter(|step| step.kind == StepKind::Start && !of_rollback(&step.intent))
        .any(|step| match (step.state, &step.evidence) {
            (StepState::Started, None) => !journal.finished(),
            (StepState::Started | StepState::Done, Some(evidence)) => spawned(evidence),
            _ => false,
        })
}

/// The flags `install undo` takes (Workflow #434 LAB-5B1c).
pub(crate) static UNDO_FLAGS: FlagLists = FlagLists {
    with_value: &["--root", "--operation"],
    lists: &[],
    switches: &[],
};

/// `install undo`. Workflow #434 LAB-3: the rescue module's `undo` runs it with its face, and the
/// journal's undo record carries its initiator.
pub(crate) fn undo(words: &[&str], host: &dyn Host, face: super::Face) -> Result<Answer, Outcome> {
    let flags = Flags::parse_by(words, &UNDO_FLAGS)?;
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
    // Workflow #453 S4: the rescue module's `new` sets aside what it wrote by its own table.
    if kind == OperationKind::New {
        return super::new::undo(operation, host, face);
    }
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
    let mut notes = Vec::new();
    // An undo already under way goes on from where it stopped: nothing here judges it again.
    if operation.journal().undo.is_none() {
        // A Runtime owner started since the operation began, inside it or after its end.
        let mut owner_started = false;
        if operation.journal().finished() {
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
                owner_started = owner_started_at(&loaded.state_root)?
                    .is_some_and(|started| started > operation.ms());
            }
        }
        // Register §1.242: once a Runtime of the undone build has started, also inside the
        // operation, the undo is a rollback: the edges, then the whole gate. An automatic rollback
        // records itself only once its gate passed, and its files may be reversed by now.
        if changes_program
            && (owner_started || started_inside(operation.journal()))
            && operation.journal().rollback.is_none()
        {
            let loaded = plan::load(&paths, UNDO)?;
            match refuse_by_edges(host, &paths, &loaded, operation.journal())? {
                Ok(warnings) => notes = warnings,
                Err(refused) => return Ok(refused),
            }
            notes.extend(refuse_by_gate(host, &paths, operation.journal())?);
        }
    }
    // Workflow #453 S6 fix 1: a placement begun while no Runtime ran moved a game root's files
    // with no hold, so its undo takes the writer lock and then applies the stopped rule the
    // placement applied (`launch::refuse_running`: `owner_lock_held`, `runtime_running`,
    // `processes_unreadable`), before anything is moved; the undo of any other operation is
    // unchanged. Workflow #453 S3 fix 1: so does the undo of a `config_replace` begun while no
    // Runtime answered (no hold, no S0), judged by the live configuration or, after a swap stopped
    // between its two renames, by the file the swap kept (`config_swap::stopped_target`). Workflow
    // #453 S9: so does the undo of a `vision place`, which always moved its files with the Runtime
    // stopped.
    let cold = (kind == OperationKind::PackPlace && super::place::is_cold(&operation))
        || kind == OperationKind::VisionPlace;
    let cold_config =
        kind == OperationKind::ConfigReplace && super::config_swap::is_cold(&operation);
    let _lock = if changes_program || cold {
        let loaded = plan::load(&paths, UNDO)?;
        let lock = WriterLock::take(&paths, loaded.settings.lock_wait_ms, UNDO)?;
        if cold {
            super::launch::refuse_running(host, &paths, &loaded.state_root, UNDO)?;
        }
        Some(lock)
    } else if cold_config {
        let (state_root, lock_wait_ms) =
            super::config_swap::stopped_target(&paths, &operation, UNDO)?;
        let lock = WriterLock::take(&paths, lock_wait_ms, UNDO)?;
        super::launch::refuse_running(host, &paths, &state_root, UNDO)?;
        Some(lock)
    } else {
        None
    };
    let ms2 = operation.begin_undo(face.initiator())?;
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
    let outcome = plan::with_notes(
        super::done(UNDO)
            .install_operation_id(operation_id.clone())
            .installation_changed(true),
        &notes,
    );
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
        // Workflow #434 LAB-3: an item of the rescue module's `place`, reversed as a swap.
        StepKind::PackItem => super::place::reverse_item(operation, step, ms2)?,
        // Workflow #453 S4: the configuration and the state root's folder of the rescue module's
        // `new`.
        StepKind::WriteConfig | StepKind::CreateStateRoot | StepKind::CreateVisionRoot => {
            super::new::reverse_own(operation, step, ms2)?
        }
        // Workflow #453 S9: a file of the rescue module's `vision place`, reversed as a swap.
        StepKind::VisionFile => super::vision::reverse_file(step, ms2)?,
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

/// Register §1.242: the interface edges of a rollback to the build before the operation, for the
/// undo of an update or rollback whose build has started. The two builds' ranges are those the
/// operation's journalled plan read from their manifests, so an edge is judged whatever the live
/// tree holds at this point, and nothing else refuses. Failed edges are the answer `interface_edge`
/// with `commit_sha`, `count` and every row in `edges`; otherwise the caveats of the rows.
fn refuse_by_edges(
    host: &dyn Host,
    paths: &Paths,
    loaded: &plan::Loaded,
    journal: &journal::Journal,
) -> Result<Result<Vec<Outcome>, Answer>, Outcome> {
    let invalid = || journal::state_invalid(UNDO, paths.journal(&journal.operation_id), None);
    let document = journal.plan.as_ref().ok_or_else(invalid)?;
    let (previous, previous_speaks) = plan::planned_side(document, "live").ok_or_else(invalid)?;
    let (_, undone_speaks) = plan::planned_side(document, "target").ok_or_else(invalid)?;
    let mut warnings = Vec::new();
    let edges = plan::rollback_edges(
        host,
        loaded,
        &undone_speaks,
        &previous_speaks,
        UNDO,
        &mut warnings,
    );
    let failed = edges
        .iter()
        .filter(|edge| edge.result == plan::EdgeResult::Fail)
        .count();
    if failed == 0 {
        return Ok(Ok(warnings));
    }
    let refusal = plan::with_notes(
        super::refusal(UNDO, InstallRefusal::InterfaceEdge)
            .commit_sha(previous)
            .count(failed),
        &warnings,
    );
    Ok(Err(Answer::new(refusal).with("edges", &edges)?))
}

/// Register §1.242: whether `install undo` of this update or rollback, as it just ended, is certain
/// to be refused by the interface edges: what a composite's success leaves out of its `next`.
pub(crate) fn edges_refuse_undo(
    host: &dyn Host,
    paths: &Paths,
    loaded: &plan::Loaded,
    journal: &journal::Journal,
) -> bool {
    started_inside(journal) && !matches!(refuse_by_edges(host, paths, loaded, journal), Ok(Ok(_)))
}

/// The rollback gate toward the build before the operation, for the undo of an update or rollback
/// once a Runtime of its build has started: its refusal (`rollback_gate_failed` of `install undo`),
/// or the gate's caveats when it passes.
fn refuse_by_gate(
    host: &dyn Host,
    paths: &Paths,
    journal: &journal::Journal,
) -> Result<Vec<Outcome>, Outcome> {
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
        None => Ok(gate
            .with_notes(super::done(UNDO))
            .causes()
            .iter()
            .map(|link| link.outcome().clone())
            .collect()),
    }
}
