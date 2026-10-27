// SPDX-License-Identifier: AGPL-3.0-only

//! The formal stop, the installer's start, the hold's release and the composite update
//! (Workflow #421 INST-2; `contracts/install-cli.md`, "Formal stop, start and the hold"; #421
//! deploy design §4, §8 S2; Alice's decisions 74 A and 75 as changed on 10-27):
//!
//! - `install stop --root R --hold <reason> [--known-pause ALIAS=REASON]... [--ack-guard <id>]...`
//!   journals S0 and what the hold defers, holds and drains, closes (`request-shutdown`) and
//!   reads S0's ledger head.
//! - `install start --root R [--ack-guard <id>]...` starts the root's Runtime (`launch`) and runs
//!   the post-start checks (`postcheck`) against the S0 of the operation that stopped it; a
//!   mismatch is the caveat `post_check_mismatch`.
//! - `install release-hold --root R --reason <reason>` resumes only the instances paused with
//!   exactly that reason and hands the deferred work back (`hold`).
//! - `install guards --root R --for stop|start [--hold <reason>] [--known-pause …]...` reports
//!   the guards and changes nothing.
//! - `install update apply … --stop --hold <reason> [--known-pause …]... [--start]
//!   [--ack-guard <id>]... [--detach]` is one operation: the hold, the close, the update's steps,
//!   the start and the post-start checks, and the release unless `install.hold_after_start`
//!   keeps the hold. **Between the new start and the resume** (decision 74 A) a failed
//!   fail-class check, or a start that ended, rolls the update back by itself: the new Runtime is
//!   closed, the rollback gate toward the previous build is evaluated with this operation's S0;
//!   when it passes the file steps are reversed, the previous build started and checked, the hold
//!   released (when its checks passed) and the command ends refused `deploy_rolled_back`; when
//!   the gate fails the new build and the hold stay and it ends `rollback_gate_failed`. After the
//!   resume nothing rolls back by itself. `--detach` runs the same command again through the
//!   watchdog's manual-run method and answers at once.
//!
//! Every step is journalled and resumable (`install resume`); nothing is killed or deleted.

use super::guards;
use super::hold::{self, HoldInputs, PauseIntent};
use super::host::Host;
use super::journal::{self, Opening, Operation, PlannedStep, Previous, RollbackRecord, StepKind};
use super::journal::{StepState, state_invalid};
use super::launch;
use super::lock::WriterLock;
use super::manifest::{self, Half};
use super::plan::{self, Loaded, PlanInputs, PlanKind, Planned};
use super::postcheck;
use super::programs::{self, Deadline};
use super::state::{self, OperationKind, Paths};
use super::update;
use super::{Answer, Flags};
use crate::codes::{
    InstallCaveat, InstallGuard, InstallLocation, InstallRefusal, RollbackGatePart,
};
use crate::watchdog::decide::CLOCK_SLACK_MS;
use actingcommand_contract::outcome::{CauseRelation, Outcome};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// How one INST-2 step ended.
pub(crate) enum StepEnd {
    /// The step is done, with its evidence.
    Done(Option<Value>),
    /// The operation ends with this refusal; the members say what stays (the hold, the build).
    Ends(Outcome, Map<String, Value>),
}

/// The members of an answer whose hold stays: `next` names `release-hold` with its reason.
fn hold_kept(paths: &Paths, reason: Option<&str>) -> Map<String, Value> {
    let mut members = Map::new();
    if let Some(reason) = reason {
        members.insert(
            "next".to_owned(),
            json!({"release_hold": ["install", "release-hold", "--root", paths.plain, "--reason", reason]}),
        );
        members.insert("hold".to_owned(), json!({"reason": reason, "kept": true}));
    }
    members
}

/// The hold's reason a journal names: its inputs' `hold`, else its pause step's.
fn hold_reason(journal: &journal::Journal) -> Option<String> {
    journal
        .inputs
        .get("hold")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| pause_intent(journal).map(|intent| intent.reason))
}

/// The pause step's intent of a journal, else the hold a start copied (`inputs.hold_intent`).
fn pause_intent(journal: &journal::Journal) -> Option<PauseIntent> {
    journal
        .steps
        .iter()
        .find(|step| step.kind == StepKind::Pause)
        .and_then(|step| PauseIntent::of(&step.intent))
        .or_else(|| journal.inputs.get("hold_intent").and_then(PauseIntent::of))
}

/// The `--ack-guard` acknowledgments a journal recorded.
fn acks_of(journal: &journal::Journal) -> Vec<String> {
    journal
        .acknowledged
        .iter()
        .filter_map(|ack| ack.strip_prefix("guard:"))
        .map(str::to_owned)
        .collect()
}

/// Whether the operation is a composite update, the only operation that rolls back by itself.
fn composite_update(journal: &journal::Journal) -> bool {
    journal.kind == OperationKind::Update
        && journal
            .steps
            .iter()
            .any(|step| step.kind == StepKind::Pause)
        && journal
            .steps
            .iter()
            .any(|step| step.kind == StepKind::Start)
}

/// Whether step `n`'s intent marks it as one of an automatic rollback's.
fn of_rollback(intent: &Value) -> bool {
    intent
        .get("rollback")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Runs the INST-2 step `n` of `operation`. A failure here is the journal's or the machine's
/// (the caller makes it `install_operation_interrupted`); a refusal is [`StepEnd::Ends`].
pub(crate) fn step(
    operation: &mut Operation<'_>,
    n: usize,
    loaded: &Loaded,
    notes: &mut Vec<Outcome>,
) -> Result<StepEnd, Outcome> {
    match operation.journal().steps[n - 1].kind {
        StepKind::Pause => pause(operation, n, loaded),
        StepKind::Close => close(operation, n, loaded, notes),
        StepKind::Start => start(operation, n, loaded, notes),
        StepKind::PostCheck => post_check(operation, n, loaded, notes),
        StepKind::ReleaseHold => release_hold(operation, n, loaded, notes),
        StepKind::ReverseFiles => reverse_files(operation, n),
        _ => Err(state_invalid(
            operation.journal().kind.location(),
            operation.paths().journal(operation.id()),
            None,
        )),
    }
}

fn pause(operation: &mut Operation<'_>, n: usize, loaded: &Loaded) -> Result<StepEnd, Outcome> {
    let command = operation.journal().kind.location();
    let step = operation.journal().steps[n - 1].clone();
    let paths = operation.paths().clone();
    let intent = PauseIntent::of(&step.intent)
        .ok_or_else(|| state_invalid(command, paths.journal(operation.id()), None))?;
    let s0_pauses = operation
        .journal()
        .s0
        .as_ref()
        .and_then(|s0| s0.pauses.as_ref())
        .map(hold::pause_reasons_of)
        .unwrap_or_default();
    let deferred = step
        .evidence
        .as_ref()
        .and_then(|evidence| evidence.get("deferred"))
        .cloned();
    let drained = hold::drained_of(step.evidence.as_ref());
    operation.start(n)?;
    let runtime = operation.host().runtime();
    match hold::run_pause(
        runtime,
        &loaded.state_root,
        command,
        &intent,
        &s0_pauses,
        drained,
    ) {
        Ok(drained) => Ok(StepEnd::Done(Some(json!({
            "deferred": deferred,
            "drained": hold::drained_value(&drained),
            "held": intent.instances,
            "known": intent.known,
            "runs_stopped_at_checkpoint": hold::runs_cut(&drained),
        })))),
        Err(refusal) => Ok(StepEnd::Ends(
            refusal,
            hold_kept(&paths, Some(&intent.reason)),
        )),
    }
}

fn close(
    operation: &mut Operation<'_>,
    n: usize,
    loaded: &Loaded,
    notes: &mut Vec<Outcome>,
) -> Result<StepEnd, Outcome> {
    let command = operation.journal().kind.location();
    let paths = operation.paths().clone();
    let reason = hold_reason(operation.journal());
    let wait_ms = operation.journal().steps[n - 1]
        .intent
        .get("shutdown_wait_ms")
        .and_then(Value::as_u64)
        .unwrap_or(loaded.settings.stop_shutdown_wait_ms);
    operation.start(n)?;
    let host = operation.host();
    let (evidence, caveat) = match hold::run_close(
        host.runtime(),
        &paths.root,
        &loaded.state_root,
        command,
        wait_ms,
    ) {
        Ok(closed) => closed,
        Err(refusal) => {
            return Ok(StepEnd::Ends(refusal, hold_kept(&paths, reason.as_deref())));
        }
    };
    notes.extend(caveat);
    // S0's ledger head, read with the live build's reader now that the Runtime stopped.
    let head = match programs::ledger_head(
        host,
        &paths.root.join(Half::Tools.dir()),
        &loaded.state_root,
        Deadline::ledger_read(&loaded.settings),
    ) {
        Ok(head) => head,
        Err(failure) => {
            let refusal = failure.onto(
                super::refusal(command, InstallRefusal::LedgerHeadUnreadable)
                    .path(&loaded.state_root),
            );
            return Ok(StepEnd::Ends(refusal, hold_kept(&paths, reason.as_deref())));
        }
    };
    operation.record_s0_head(head)?;
    // Nothing of the Runtime may still run before a file changes.
    if let Err(refusal) = launch::refuse_running(host, &paths, &loaded.state_root, command) {
        return Ok(StepEnd::Ends(refusal, hold_kept(&paths, reason.as_deref())));
    }
    let mut evidence = evidence;
    evidence["head"] = json!(head);
    Ok(StepEnd::Done(Some(evidence)))
}

fn start(
    operation: &mut Operation<'_>,
    n: usize,
    loaded: &Loaded,
    notes: &mut Vec<Outcome>,
) -> Result<StepEnd, Outcome> {
    let command = operation.journal().kind.location();
    let paths = operation.paths().clone();
    let step = operation.journal().steps[n - 1].clone();
    let reason = hold_reason(operation.journal());
    let commit = step
        .intent
        .get("commit")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| state_invalid(command, paths.journal(operation.id()), None))?;
    let host = operation.host();
    // A start under way when the installer stopped: a Runtime that answers and started since the
    // spawn is that start's.
    if step.state == StepState::Started
        && let Some(evidence) = &step.evidence
        && let Some(at) = evidence.get("at_unix_ms").and_then(Value::as_u64)
        && at > 0
        && let Some(owner) = host.runtime().answering_owner(&loaded.state_root)
        && owner.started_at_unix_ms >= at.saturating_sub(CLOCK_SLACK_MS)
    {
        let mut evidence = evidence.clone();
        evidence["ready"] = json!({
            "pid": owner.pid,
            "owner_epoch": owner.owner_epoch,
            "started_at_unix_ms": owner.started_at_unix_ms,
        });
        if evidence.get("ready_ms").is_none_or(Value::is_null) {
            evidence["ready_ms"] = json!(owner.started_at_unix_ms.saturating_sub(at));
        }
        return Ok(StepEnd::Done(Some(evidence)));
    }
    operation.start(n)?;
    let acks = acks_of(operation.journal());
    let launched = {
        let mut progress = |value: &Value| operation.progress(n, value.clone());
        launch::start(host, &paths, loaded, command, &commit, &acks, &mut progress)
    };
    let launched = match launched {
        Ok(launched) => launched,
        Err(refusal) => {
            return Ok(StepEnd::Ends(refusal, hold_kept(&paths, reason.as_deref())));
        }
    };
    if launched.stalled {
        notes.push(super::caveat(command, InstallCaveat::StartStalled).path(&launched.log));
    }
    let evidence = launched.evidence(&commit);
    if launched.ready.is_some() {
        return Ok(StepEnd::Done(Some(evidence)));
    }
    let failure = launched.failure.clone().unwrap_or_else(|| {
        Outcome::new(crate::codes::ActingctlCode::ExitedDuringStartup).log(&launched.log)
    });
    operation.progress(n, evidence.clone())?;
    // A start left running at its deadline is the stall rule's: no rollback, nothing killed.
    if !launched.left_running && composite_update(operation.journal()) && !of_rollback(&step.intent)
    {
        return begin_rollback(
            operation,
            n,
            loaded,
            vec!["ready".to_owned()],
            evidence,
            notes,
        );
    }
    let refusal = super::refusal(command, InstallRefusal::RuntimeStartFailed)
        .path(launched.log.as_path())
        .joined(CauseRelation::CausedBy, failure);
    let mut members = hold_kept(&paths, reason.as_deref());
    members.insert("start".to_owned(), evidence);
    Ok(StepEnd::Ends(refusal, members))
}

/// The evidence of the newest start step before step `n`.
fn start_before(operation: &Operation<'_>, n: usize) -> Option<Value> {
    operation.journal().steps[..n - 1]
        .iter()
        .rev()
        .find(|step| step.kind == StepKind::Start)
        .and_then(|step| step.evidence.clone())
}

fn post_check(
    operation: &mut Operation<'_>,
    n: usize,
    loaded: &Loaded,
    notes: &mut Vec<Outcome>,
) -> Result<StepEnd, Outcome> {
    let command = operation.journal().kind.location();
    let paths = operation.paths().clone();
    let step = operation.journal().steps[n - 1].clone();
    let rollback = of_rollback(&step.intent);
    let journal = operation.journal().clone();
    let started = start_before(operation, n).unwrap_or(Value::Null);
    let log = started
        .get("log")
        .and_then(Value::as_str)
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    let ready = started
        .get("ready_ms")
        .and_then(Value::as_u64)
        .ok_or_else(|| "not_ready".to_owned());
    let hold = pause_intent(&journal);
    let state = state::load_state_or_default(&paths)?;
    let config_sha256 = postcheck::config_sha256(&paths.config());
    let added = if rollback {
        Vec::new()
    } else {
        postcheck::added_keys(journal.plan.as_ref())
    };
    operation.start(n)?;
    let rows = postcheck::evaluate(
        operation.host(),
        &loaded.state_root,
        &postcheck::CheckInputs {
            ready,
            log: &log,
            s0: journal.s0.as_ref(),
            hold: hold.as_ref(),
            config: &loaded.config,
            config_sha256: &config_sha256,
            state: &state,
            added_keys: &added,
        },
    );
    let failed = postcheck::failed(&rows);
    let mismatched = postcheck::mismatched(&rows);
    let evidence = json!({
        "commit": step.intent.get("commit"),
        "checks": postcheck::values(&rows),
        "failed": failed,
    });
    if mismatched > 0 && (failed.is_empty() || rollback || journal.kind == OperationKind::Start) {
        notes.push(super::caveat(command, InstallCaveat::PostCheckMismatch).count(mismatched));
    }
    if failed.is_empty() || rollback || journal.kind == OperationKind::Start {
        return Ok(StepEnd::Done(Some(evidence)));
    }
    if composite_update(&journal) {
        return begin_rollback(operation, n, loaded, failed, evidence, notes);
    }
    let refusal = super::refusal(command, InstallRefusal::PostCheckFailed).count(failed.len());
    let mut members = hold_kept(&paths, hold_reason(&journal).as_deref());
    members.insert("post_check".to_owned(), json!(postcheck::values(&rows)));
    members.insert("failed_checks".to_owned(), json!(failed));
    Ok(StepEnd::Ends(refusal, members))
}

/// The automatic rollback of a composite update, between the new start and the resume
/// (decision 74 A): the new Runtime closed when it runs (nothing is killed: one that does not
/// stop ends the command `post_check_failed`), the rollback gate toward the previous build with
/// this operation's S0, and when it passes the rollback's steps planned after the current one
/// (the current step done with `evidence` and the gate's rows). A gate that fails ends the
/// command `rollback_gate_failed`: the new build and the hold stay, a forward fix only.
fn begin_rollback(
    operation: &mut Operation<'_>,
    n: usize,
    loaded: &Loaded,
    failed: Vec<String>,
    evidence: Value,
    notes: &mut Vec<Outcome>,
) -> Result<StepEnd, Outcome> {
    let command = operation.journal().kind.location();
    let paths = operation.paths().clone();
    let journal = operation.journal().clone();
    let reason = hold_reason(&journal);
    let host = operation.host();
    let runtime = host.runtime();
    let mut members = hold_kept(&paths, reason.as_deref());
    members.insert("failed_checks".to_owned(), json!(failed));
    members.insert("post_check".to_owned(), evidence.clone());
    let Some(previous) = journal.previous.live.clone() else {
        let refusal = super::refusal(command, InstallRefusal::PostCheckFailed).count(failed.len());
        return Ok(StepEnd::Ends(refusal, members));
    };
    // 1. The new Runtime closed, when it runs.
    let running = match runtime.status(&loaded.state_root) {
        Ok(running) => running.is_some(),
        Err(cause) => {
            let refusal = super::refusal(command, InstallRefusal::PostCheckFailed)
                .count(failed.len())
                .joined(CauseRelation::CausedBy, cause);
            return Ok(StepEnd::Ends(refusal, members));
        }
    };
    if running
        && let Err(cause) = runtime.shutdown(
            &paths.root,
            &loaded.state_root,
            loaded.settings.stop_shutdown_wait_ms,
        )
    {
        let refusal = super::refusal(command, InstallRefusal::PostCheckFailed)
            .count(failed.len())
            .joined(CauseRelation::CausedBy, cause);
        return Ok(StepEnd::Ends(refusal, members));
    }
    // 2. The rollback gate toward the previous build, with this operation's S0.
    let planned = match plan::plan(
        host,
        &paths,
        PlanInputs {
            kind: PlanKind::Rollback,
            commit: previous.clone(),
            accept_ledger_one_way: false,
            preview: false,
            operation: command,
        },
    ) {
        Ok(planned) => planned,
        Err(refused) => {
            let refusal = super::refusal(command, InstallRefusal::RollbackGateFailed)
                .rollback_gate_parts([RollbackGatePart::InterfaceEdges])
                .commit_sha(previous)
                .joined(CauseRelation::CausedBy, refused.into_answer().outcome);
            return Ok(StepEnd::Ends(refusal, members));
        }
    };
    let gate = update::gate(host, &paths, &planned);
    members.insert("gate".to_owned(), json!(gate.rows));
    if let Some(refusal) = gate.refusal(&planned) {
        return Ok(StepEnd::Ends(refusal, members));
    }
    notes.extend(
        gate.with_notes(super::done(command))
            .causes()
            .iter()
            .map(|link| link.outcome().clone()),
    );
    // 3. The rollback's steps.
    operation.record_rollback(RollbackRecord {
        started_at_unix_ms: host.now_unix_ms(),
        to: previous.clone(),
        failed,
        reversed_at_unix_ms: None,
    })?;
    let mut steps: Vec<PlannedStep> = vec![
        (
            StepKind::ReverseFiles,
            "runtime".to_owned(),
            json!({"to": previous}),
        ),
        (
            StepKind::Start,
            "runtime".to_owned(),
            json!({"commit": previous, "rollback": true}),
        ),
        (
            StepKind::PostCheck,
            "runtime".to_owned(),
            json!({"commit": previous, "rollback": true}),
        ),
    ];
    if let Some(reason) = &reason {
        steps.push((
            StepKind::ReleaseHold,
            "runtime".to_owned(),
            json!({"reason": reason, "rollback": true}),
        ));
    }
    // The planned steps not begun (the new build's checks, the release) are replaced.
    operation.skip_planned(n + 1, "rollback")?;
    operation.append_steps(steps)?;
    let mut evidence = evidence;
    evidence["rollback_gate"] = json!(gate.rows);
    Ok(StepEnd::Done(Some(evidence)))
}

/// The reversal of the operation's file steps, as `install undo` reverses them (each displaced
/// file kept), then the previous build verified in place.
fn reverse_files(operation: &mut Operation<'_>, n: usize) -> Result<StepEnd, Outcome> {
    let command = operation.journal().kind.location();
    let paths = operation.paths().clone();
    let to = operation.journal().steps[n - 1]
        .intent
        .get("to")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| state_invalid(command, paths.journal(operation.id()), None))?;
    operation.start(n)?;
    let ms2 = operation.host().now_unix_ms();
    let steps = operation.journal().steps.clone();
    let previous = operation.journal().previous.clone();
    let mut kept = Vec::new();
    for step in steps.iter().rev() {
        if !matches!(
            step.kind,
            StepKind::StageFile | StepKind::SwapFile | StepKind::RecordState
        ) || !matches!(step.state, StepState::Started | StepState::Done)
        {
            continue;
        }
        let evidence = super::undo::reverse_step(operation, step, &previous, ms2)?;
        kept.extend(evidence.iter().cloned());
        operation.undone(step.n, Some(json!({ "kept": evidence })))?;
    }
    match manifest::verify_build(&paths.root) {
        Ok(build) if build.commit == to => {}
        _ => {
            return Err(super::adopt::verify_live(command, &paths)
                .err()
                .unwrap_or_else(|| {
                    super::refusal(command, InstallRefusal::LiveBuildInvalid).path(&paths.root)
                }));
        }
    }
    if let Some(mut rollback) = operation.journal().rollback.clone() {
        rollback.reversed_at_unix_ms = Some(operation.host().now_unix_ms());
        operation.record_rollback(rollback)?;
    }
    Ok(StepEnd::Done(Some(json!({"to": to, "kept": kept}))))
}

/// The deferred work a hold of `reason` recorded and how its drains ended: the pause step of
/// this operation, else of the newest journalled operation whose hold has that reason.
fn deferred_for(
    operation: &Operation<'_>,
    reason: &str,
) -> (Option<Value>, BTreeMap<String, super::runtime::Drained>) {
    let of = |journal: &journal::Journal| {
        journal
            .steps
            .iter()
            .find(|step| {
                step.kind == StepKind::Pause
                    && PauseIntent::of(&step.intent).is_some_and(|intent| intent.reason == reason)
            })
            .map(|step| {
                (
                    step.evidence
                        .as_ref()
                        .and_then(|evidence| evidence.get("deferred"))
                        .cloned(),
                    hold::drained_of(step.evidence.as_ref()),
                )
            })
    };
    if let Some(found) = of(operation.journal()) {
        return found;
    }
    let paths = operation.paths();
    for id in journal::operation_ids(paths)
        .unwrap_or_default()
        .iter()
        .rev()
    {
        if let Ok(other) =
            Operation::load(operation.host(), paths, id, InstallLocation::ReleaseHold)
            && let Some(found) = of(other.journal())
        {
            return found;
        }
    }
    (None, BTreeMap::new())
}

fn release_hold(
    operation: &mut Operation<'_>,
    n: usize,
    loaded: &Loaded,
    notes: &mut Vec<Outcome>,
) -> Result<StepEnd, Outcome> {
    let command = operation.journal().kind.location();
    let paths = operation.paths().clone();
    let step = operation.journal().steps[n - 1].clone();
    let reason = step
        .intent
        .get("reason")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| state_invalid(command, paths.journal(operation.id()), None))?;
    // A rollback whose previous build's checks failed keeps the hold.
    if of_rollback(&step.intent) {
        let checks_failed = operation.journal().steps[..n - 1]
            .iter()
            .rev()
            .find(|earlier| earlier.kind == StepKind::PostCheck)
            .and_then(|earlier| earlier.evidence.as_ref())
            .and_then(|evidence| evidence.get("failed"))
            .and_then(Value::as_array)
            .is_some_and(|failed| !failed.is_empty());
        if checks_failed {
            return Ok(StepEnd::Done(Some(
                json!({"reason": reason, "skipped": "post_check_failed"}),
            )));
        }
    }
    operation.start(n)?;
    let host = operation.host();
    let runtime = host.runtime();
    let resumed = match hold::release(runtime, &loaded.state_root, command, &reason) {
        Ok(resumed) => resumed,
        Err(refusal) => return Ok(StepEnd::Ends(refusal, hold_kept(&paths, Some(&reason)))),
    };
    let released_at = host.now_unix_ms();
    let (deferred, drained) = deferred_for(operation, &reason);
    let plan_after = runtime.plan(&loaded.state_root);
    let handed_back = hold::hand_back(
        deferred.as_ref(),
        &drained,
        &resumed,
        plan_after.as_ref(),
        released_at,
    );
    let cut = hold::runs_cut(&drained);
    if cut > 0 {
        notes.push(super::caveat(command, InstallCaveat::RunsNotContinued).count(cut));
    }
    Ok(StepEnd::Done(Some(json!({
        "reason": reason,
        "resumed": resumed,
        "handed_back": handed_back,
    }))))
}

/// Ends `operation` refused: its result recorded and `current.json` retired; the answer carries
/// `members` (what stays: the hold, the build).
pub(crate) fn end(
    operation: Operation<'_>,
    refusal: Outcome,
    members: Map<String, Value>,
    notes: &[Outcome],
) -> Result<Answer, Outcome> {
    // The operation's id on the family's own refusal; a fatal cause that tops it (the
    // fatal-link rule) keeps its own values.
    let refusal = if refusal.is(crate::codes::ActingctlCode::InstallCommandRefused) {
        refusal.install_operation_id(operation.id().to_owned())
    } else {
        refusal
    };
    let refusal = plan::with_notes(refusal, notes);
    operation.finish(&refusal)?;
    Ok(Answer {
        members,
        outcome: refusal,
        text: None,
    })
}

/// The counts of an update's file steps, for its answer.
pub(crate) struct FileCounts {
    pub(crate) replaced: usize,
    pub(crate) added: usize,
}

/// The evidence of every step of `kind` that ran, in order: the operation's own steps, or with
/// `rollback` an automatic rollback's.
fn evidence_of(journal: &journal::Journal, kind: StepKind, rollback: bool) -> Vec<Value> {
    journal
        .steps
        .iter()
        .filter(|step| step.kind == kind && of_rollback(&step.intent) == rollback)
        .filter_map(|step| step.evidence.clone())
        .filter(|evidence| evidence.get("skipped").is_none())
        .collect()
}

/// The end of an operation whose steps all ran: its outcome by kind (an update or rollback
/// done, or refused `deploy_rolled_back` after an automatic rollback; a stop, start or release
/// done) with the operation's members, its caveats as `note` links.
pub(crate) fn finish(
    operation: Operation<'_>,
    notes: Vec<Outcome>,
    counts: FileCounts,
) -> Result<Answer, Outcome> {
    let journal = operation.journal().clone();
    let paths = operation.paths().clone();
    let command = journal.kind.location();
    let operation_id = operation.id().to_owned();
    let target = journal
        .inputs
        .get("commit")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let starts = evidence_of(&journal, StepKind::Start, false);
    let checks = evidence_of(&journal, StepKind::PostCheck, false);
    let mut releases = evidence_of(&journal, StepKind::ReleaseHold, false);
    releases.extend(evidence_of(&journal, StepKind::ReleaseHold, true));
    let released = releases
        .iter()
        .any(|release| release.get("resumed").is_some());
    let reason = hold_reason(&journal);
    let mut members = Map::new();
    let mut insert = |name: &str, value: Value| {
        members.insert(name.to_owned(), value);
    };
    insert("operation_id", json!(operation_id));
    let outcome = match journal.kind {
        OperationKind::Update | OperationKind::Rollback => {
            let verified = manifest::verify_build(&paths.root)
                .map(|build| build.runtime.files.len() + build.tools.files.len())
                .unwrap_or(0);
            insert("from_commit", json!(journal.previous.live));
            insert("to_commit", json!(target));
            insert("replaced", json!(counts.replaced));
            insert("added", json!(counts.added));
            insert("verified", json!(verified));
            if !starts.is_empty() {
                insert("start", starts[0].clone());
            }
            if !checks.is_empty() {
                insert("post_check", checks[0].clone());
            }
            match &journal.rollback {
                Some(rollback) => {
                    insert("failed_checks", json!(rollback.failed));
                    insert(
                        "rollback",
                        json!({
                            "to": rollback.to,
                            "start": evidence_of(&journal, StepKind::Start, true).first(),
                            "post_check": evidence_of(&journal, StepKind::PostCheck, true).first(),
                            "released": released,
                        }),
                    );
                    super::refusal(command, InstallRefusal::DeployRolledBack)
                        .commit_sha(rollback.to.clone())
                        .count(rollback.failed.len())
                        .install_operation_id(operation_id.clone())
                }
                None => {
                    insert(
                        "next",
                        json!({"undo": ["install", "undo", "--root", paths.plain, "--operation", operation_id]}),
                    );
                    super::done(command)
                        .install_operation_id(operation_id.clone())
                        .commit_sha(target.clone().unwrap_or_default())
                        .installation_changed(true)
                }
            }
        }
        OperationKind::Stop => {
            insert("s0", json!(journal.s0));
            insert(
                "held",
                evidence_of(&journal, StepKind::Pause, false)
                    .first()
                    .cloned()
                    .unwrap_or(Value::Null),
            );
            super::done(command)
                .install_operation_id(operation_id.clone())
                .installation_changed(false)
        }
        OperationKind::Start => {
            let started = starts.first().cloned().unwrap_or(Value::Null);
            if !checks.is_empty() {
                insert("post_check", checks[0].clone());
            }
            let ready = &started["ready"];
            let mut done = super::done(command)
                .install_operation_id(operation_id.clone())
                .installation_changed(false);
            if let Some(pid) = ready["pid"].as_u64() {
                done = done.pid(pid);
            }
            if let Some(epoch) = ready["owner_epoch"].as_str() {
                done = done.owner_epoch(epoch);
            }
            if let Some(log) = started["log"].as_str() {
                done = done.log(log);
            }
            insert("start", started);
            done
        }
        OperationKind::ReleaseHold => {
            let resumed = releases
                .first()
                .and_then(|release| release.get("resumed"))
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            insert("release", releases.first().cloned().unwrap_or(Value::Null));
            super::done(command)
                .install_operation_id(operation_id.clone())
                .count(resumed)
        }
        OperationKind::ReleaseStage | OperationKind::Adopt => {
            return Err(operation.interrupted(state_invalid(
                command,
                paths.journal(&operation_id),
                None,
            )));
        }
    };
    if let Some(reason) = &reason
        && !released
        && matches!(
            journal.kind,
            OperationKind::Update | OperationKind::Rollback | OperationKind::Stop
        )
    {
        members.extend(hold_kept(&paths, Some(reason)));
    } else if let Some(release) = releases.last() {
        members.insert(
            "hold".to_owned(),
            json!({"reason": reason, "kept": false, "release": release}),
        );
    }
    let outcome = plan::with_notes(outcome, &notes);
    operation.finish(&outcome)?;
    Ok(Answer {
        members,
        outcome,
        text: None,
    })
}

/// The steps of a composite update: the hold and the close, the update's own steps, then with
/// `--start` the start and the post-start checks, and the release unless
/// `install.hold_after_start` keeps the hold.
fn composite_steps(
    planned: &Planned,
    ms: u64,
    hold: &HoldInputs,
    held: &[String],
    start: bool,
    loaded: &Loaded,
) -> Vec<PlannedStep> {
    let settings = &loaded.settings;
    let mut steps = vec![
        (
            StepKind::Pause,
            "runtime".to_owned(),
            hold::pause_intent(hold, held, settings.stop_drain_timeout_ms),
        ),
        (
            StepKind::Close,
            "runtime".to_owned(),
            json!({"shutdown_wait_ms": settings.stop_shutdown_wait_ms}),
        ),
    ];
    steps.extend(update::update_steps(planned, ms));
    if start {
        let commit = json!({"commit": planned.target.commit});
        steps.push((StepKind::Start, "runtime".to_owned(), commit.clone()));
        steps.push((StepKind::PostCheck, "runtime".to_owned(), commit));
        if !settings.hold_after_start {
            steps.push((
                StepKind::ReleaseHold,
                "runtime".to_owned(),
                json!({"reason": hold.reason}),
            ));
        }
    }
    steps
}

/// The hold's S0, the guards and the deferred record, read from the running Runtime before
/// anything changes; the guards' refusal as an answer with their rows.
struct HoldStart {
    seen: super::runtime::RuntimeSeen,
    config: super::runtime::ConfigSeen,
    held: Vec<String>,
    rows: Vec<guards::GuardRow>,
    deferred: Value,
}

fn hold_start(
    host: &dyn Host,
    paths: &Paths,
    loaded: &Loaded,
    operation: InstallLocation,
    hold: &HoldInputs,
    with_memory: bool,
    acks: &[String],
) -> Result<Result<HoldStart, Answer>, Outcome> {
    let runtime = host.runtime();
    let seen = match runtime.status(&loaded.state_root) {
        Ok(Some(seen)) => seen,
        Ok(None) => {
            return Err(super::refusal(operation, InstallRefusal::RuntimeNotRunning)
                .path(&loaded.state_root));
        }
        Err(cause) => {
            return Err(
                super::refusal(operation, InstallRefusal::ProcessesUnreadable)
                    .joined(CauseRelation::CausedBy, cause),
            );
        }
    };
    let config = runtime
        .config(&loaded.state_root)
        .map_err(|cause| hold::stop_failed(operation, cause))?;
    let plan_seen = runtime.plan(&loaded.state_root);
    let held = hold::held_instances(&seen, &hold.reason);
    let now = host.now_unix_ms();
    let mut rows = vec![
        guards::foreign_pauses(&seen, &hold.reason, &hold.known),
        guards::schedule_near(plan_seen.as_ref(), now, &held, &loaded.settings),
        guards::adb(host, paths, &loaded.config, &loaded.settings),
    ];
    if with_memory {
        rows.push(guards::memory(host, &loaded.settings));
    }
    if let Err(refusal) = guards::refuse(operation, &rows, acks) {
        return Ok(Err(
            Answer::new(refusal).with("guards", guards::values(&rows))?
        ));
    }
    let deferred = hold::deferred_of(now, &seen, plan_seen.as_ref(), &held);
    Ok(Ok(HoldStart {
        seen,
        config,
        held,
        rows,
        deferred,
    }))
}

/// Opens a composite update or rollback (`update apply … --stop --hold …`): the hold's reads and
/// guards from the running Runtime, then the operation with S0, the guards, the
/// acknowledgments and the deferred record journalled. A running image that would change and may
/// not be renamed refuses before anything else.
pub(crate) fn open_composite<'h>(
    host: &'h dyn Host,
    paths: &Paths,
    loaded: &Loaded,
    planned: &Planned,
    hold: &HoldInputs,
    start: bool,
    acks: &[String],
) -> Result<Result<Operation<'h>, Answer>, Outcome> {
    let operation = planned.inputs.operation;
    if let Some(image) = planned.running.blocked.first() {
        let refusal = super::refusal(operation, InstallRefusal::RunningImageBlocks)
            .path(&image.path)
            .pid(image.pid);
        return Ok(Err(planned.answer(refusal)?));
    }
    let begun = match hold_start(host, paths, loaded, operation, hold, start, acks)? {
        Ok(begun) => begun,
        Err(answer) => return Ok(Err(answer)),
    };
    let mut acknowledged = if planned.inputs.accept_ledger_one_way {
        vec!["accept_ledger_one_way".to_owned()]
    } else {
        Vec::new()
    };
    acknowledged.extend(acks.iter().map(|ack| format!("guard:{ack}")));
    let opening = Opening {
        kind: match planned.inputs.kind {
            PlanKind::Update => OperationKind::Update,
            PlanKind::Rollback => OperationKind::Rollback,
        },
        inputs: json!({
            "commit": planned.target.commit,
            "accept_ledger_one_way": planned.inputs.accept_ledger_one_way,
            "stop": true,
            "hold": hold.reason,
            "known_pauses": hold.known,
            "start": start,
        }),
        plan_id: Some(planned.id.clone()),
        plan: Some(planned.document.clone()),
        acknowledged,
        s0: Some(hold::s0_of(&begun.seen, &begun.config)),
        previous: Previous {
            live: Some(planned.live.commit.clone()),
            state: Some(planned.state.clone()),
        },
    };
    let mut opened = Operation::open(host, paths, opening, |ms| {
        composite_steps(planned, ms, hold, &begun.held, start, loaded)
    })?;
    opened
        .record_guards(guards::values(&begun.rows), acks)
        .map_err(|cause| opened.interrupted(cause))?;
    opened
        .progress(1, json!({"deferred": begun.deferred}))
        .map_err(|cause| opened.interrupted(cause))?;
    Ok(Ok(opened))
}

/// `install stop --root <root> --hold <reason> [--known-pause ALIAS=REASON]... [--ack-guard
/// <id>]...`.
pub(crate) fn stop_command(words: &[&str], host: &dyn Host) -> Result<Answer, Outcome> {
    const STOP: InstallLocation = InstallLocation::Stop;
    let flags = Flags::parse_lists(
        words,
        &["--root", "--hold"],
        &["--known-pause", "--ack-guard"],
        &[],
    )?;
    let paths = super::resolve_root(&flags)?;
    let hold = match hold::parse_hold(&flags)? {
        Some(hold) => hold,
        None => {
            return Err(flags
                .required("--hold")
                .err()
                .unwrap_or_else(|| super::unusable("--hold")));
        }
    };
    let acks = guards::parse_acks(&flags.list("--ack-guard"))?;
    journal::refuse_unfinished(STOP, &paths)?;
    let loaded = plan::load(&paths, STOP)?;
    let begun = match hold_start(host, &paths, &loaded, STOP, &hold, false, &acks)? {
        Ok(begun) => begun,
        Err(answer) => return Ok(answer),
    };
    let opening = Opening {
        kind: OperationKind::Stop,
        inputs: json!({"hold": hold.reason, "known_pauses": hold.known}),
        plan_id: None,
        plan: None,
        acknowledged: acks.iter().map(|ack| format!("guard:{ack}")).collect(),
        s0: Some(hold::s0_of(&begun.seen, &begun.config)),
        previous: Previous {
            live: loaded.state.live.as_ref().map(|live| live.commit.clone()),
            state: None,
        },
    };
    let settings = &loaded.settings;
    let mut operation = Operation::open(host, &paths, opening, |_| {
        vec![
            (
                StepKind::Pause,
                "runtime".to_owned(),
                hold::pause_intent(&hold, &begun.held, settings.stop_drain_timeout_ms),
            ),
            (
                StepKind::Close,
                "runtime".to_owned(),
                json!({"shutdown_wait_ms": settings.stop_shutdown_wait_ms}),
            ),
        ]
    })?;
    operation
        .record_guards(guards::values(&begun.rows), &acks)
        .map_err(|cause| operation.interrupted(cause))?;
    operation
        .progress(1, json!({"deferred": begun.deferred}))
        .map_err(|cause| operation.interrupted(cause))?;
    update::run(operation, &loaded)
}

/// The S0 and the hold of the newest journalled operation that stopped the Runtime: what a
/// standalone start's post-start checks judge against.
fn last_stop(host: &dyn Host, paths: &Paths) -> Option<(String, journal::S0, Option<Value>)> {
    for id in journal::operation_ids(paths)
        .unwrap_or_default()
        .iter()
        .rev()
    {
        let Ok(loaded) = Operation::load(host, paths, id, InstallLocation::Start) else {
            continue;
        };
        let journal = loaded.journal();
        if let Some(s0) = &journal.s0
            && s0.epoch.is_some()
        {
            let hold = journal
                .steps
                .iter()
                .find(|step| step.kind == StepKind::Pause)
                .map(|step| step.intent.clone());
            return Some((id.clone(), s0.clone(), hold));
        }
    }
    None
}

/// `install start --root <root> [--ack-guard <id>]...`.
pub(crate) fn start_command(words: &[&str], host: &dyn Host) -> Result<Answer, Outcome> {
    const START: InstallLocation = InstallLocation::Start;
    let flags = Flags::parse_lists(words, &["--root"], &["--ack-guard"], &[])?;
    let paths = super::resolve_root(&flags)?;
    let acks = guards::parse_acks(&flags.list("--ack-guard"))?;
    journal::refuse_unfinished(START, &paths)?;
    let loaded = plan::load(&paths, START)?;
    let _lock = WriterLock::take(&paths, loaded.settings.lock_wait_ms, START)?;
    journal::refuse_unfinished(START, &paths)?;
    let live = super::adopt::verify_live(START, &paths)?;
    launch::refuse_running(host, &paths, &loaded.state_root, START)?;
    // The configuration passes the check-config of the build in `runtime\`.
    let checked = programs::check_config(
        host,
        &paths.root.join(Half::Runtime.dir()),
        &paths.config(),
        None,
        Deadline::check_config(&loaded.settings),
    )
    .map_err(|failure| {
        failure.onto(
            super::refusal(START, InstallRefusal::CheckConfigUnanswered)
                .commit_sha(live.commit.clone()),
        )
    })?;
    if !checked.passed() {
        return Err(super::refusal(START, InstallRefusal::ConfigRefusedByTarget)
            .commit_sha(live.commit.clone())
            .joined(CauseRelation::CausedBy, checked.received));
    }
    let rows = vec![guards::adb(host, &paths, &loaded.config, &loaded.settings)];
    if let Err(refusal) = guards::refuse(START, &rows, &acks) {
        return Answer::new(refusal).with("guards", guards::values(&rows));
    }
    let stopped = last_stop(host, &paths);
    let opening = Opening {
        kind: OperationKind::Start,
        inputs: json!({
            "commit": live.commit,
            "s0_from": stopped.as_ref().map(|(id, _, _)| id),
            "hold_intent": stopped.as_ref().and_then(|(_, _, hold)| hold.clone()),
        }),
        plan_id: None,
        plan: None,
        acknowledged: acks.iter().map(|ack| format!("guard:{ack}")).collect(),
        s0: stopped.map(|(_, s0, _)| s0),
        previous: Previous {
            live: Some(live.commit.clone()),
            state: None,
        },
    };
    let commit = live.commit.clone();
    let mut operation = Operation::open(host, &paths, opening, |_| {
        vec![
            (
                StepKind::Start,
                "runtime".to_owned(),
                json!({"commit": commit}),
            ),
            (
                StepKind::PostCheck,
                "runtime".to_owned(),
                json!({"commit": commit}),
            ),
        ]
    })?;
    operation
        .record_guards(guards::values(&rows), &acks)
        .map_err(|cause| operation.interrupted(cause))?;
    update::run(operation, &loaded)
}

/// `install release-hold --root <root> --reason <reason>`.
pub(crate) fn release_hold_command(words: &[&str], host: &dyn Host) -> Result<Answer, Outcome> {
    const RELEASE: InstallLocation = InstallLocation::ReleaseHold;
    let flags = Flags::parse(words, &["--root", "--reason"], &[])?;
    let paths = super::resolve_root(&flags)?;
    let reason = flags.required("--reason")?;
    if actingcommand_contract::validate_scheduling_pause_reason(reason).is_err() {
        return Err(super::unusable("--reason"));
    }
    journal::refuse_unfinished(RELEASE, &paths)?;
    let loaded = plan::load(&paths, RELEASE)?;
    if host
        .runtime()
        .status(&loaded.state_root)
        .map_err(|cause| {
            super::refusal(RELEASE, InstallRefusal::RuntimeNotRunning)
                .joined(CauseRelation::CausedBy, cause)
        })?
        .is_none()
    {
        return Err(super::refusal(RELEASE, InstallRefusal::RuntimeNotRunning)
            .path(loaded.state_root.as_path()));
    }
    let opening = Opening {
        kind: OperationKind::ReleaseHold,
        inputs: json!({"hold": reason}),
        plan_id: None,
        plan: None,
        acknowledged: Vec::new(),
        s0: None,
        previous: Previous {
            live: loaded.state.live.as_ref().map(|live| live.commit.clone()),
            state: None,
        },
    };
    let operation = Operation::open(host, &paths, opening, |_| {
        vec![(
            StepKind::ReleaseHold,
            "runtime".to_owned(),
            json!({"reason": reason}),
        )]
    })?;
    update::run(operation, &loaded)
}

/// `install guards --root <root> --for stop|start [--hold <reason>] [--known-pause
/// ALIAS=REASON]...`: the guards the command would evaluate now; nothing changes. A hot
/// placement's guards come with INST-4.
pub(crate) fn guards_command(words: &[&str], host: &dyn Host) -> Result<Answer, Outcome> {
    const GUARDS: InstallLocation = InstallLocation::GuardsReport;
    let flags = Flags::parse_lists(
        words,
        &["--root", "--for", "--hold"],
        &["--known-pause"],
        &[],
    )?;
    let paths = super::resolve_root(&flags)?;
    let loaded = plan::load(&paths, GUARDS)?;
    let runtime = host.runtime();
    let rows = match flags.required("--for")? {
        "stop" => {
            let hold = hold::parse_hold(&flags)?.unwrap_or(HoldInputs {
                reason: String::new(),
                known: BTreeMap::new(),
            });
            let mut rows = Vec::new();
            match runtime.status(&loaded.state_root) {
                Ok(Some(seen)) => {
                    let held = hold::held_instances(&seen, &hold.reason);
                    let plan = runtime.plan(&loaded.state_root);
                    rows.push(guards::foreign_pauses(&seen, &hold.reason, &hold.known));
                    rows.push(guards::schedule_near(
                        plan.as_ref(),
                        host.now_unix_ms(),
                        &held,
                        &loaded.settings,
                    ));
                }
                Ok(None) => {
                    for guard in [InstallGuard::ForeignPauses, InstallGuard::ScheduleNear] {
                        rows.push(guards::GuardRow {
                            id: guard.as_str(),
                            guard,
                            result: guards::GuardResult::NotEvaluated,
                            values: json!({"runtime": "not_running"}),
                            ackable: true,
                        });
                    }
                }
                Err(failure) => {
                    return Err(super::refusal(GUARDS, InstallRefusal::ProcessesUnreadable)
                        .joined(CauseRelation::CausedBy, failure));
                }
            }
            rows.push(guards::adb(host, &paths, &loaded.config, &loaded.settings));
            rows
        }
        "start" => vec![
            guards::memory(host, &loaded.settings),
            guards::adb(host, &paths, &loaded.config, &loaded.settings),
        ],
        _ => return Err(super::unusable("--for")),
    };
    let failing = rows
        .iter()
        .filter(|row| row.result != guards::GuardResult::Pass)
        .count();
    Answer::new(super::done(GUARDS).count(failing)).with("guards", guards::values(&rows))
}

/// `--detach`: the same command run again without it, through the watchdog's manual-run method,
/// its output appended to `install\detached-<ms>.log`; the answer is at once, with the process
/// id and the log, and `install status` follows the operation.
pub(crate) fn detach(
    host: &dyn Host,
    paths: &Paths,
    words: &[&str],
    operation: InstallLocation,
) -> Result<Answer, Outcome> {
    let verb = match operation {
        InstallLocation::RollbackApply => "rollback",
        _ => "update",
    };
    let mut arguments = vec!["install".to_owned(), verb.to_owned(), "apply".to_owned()];
    let mut index = 0;
    while index < words.len() {
        let word = words[index];
        if word == "--detach" {
            index += 1;
            continue;
        }
        if word == "--root" {
            arguments.push(word.to_owned());
            arguments.push(paths.plain.clone());
            index += 2;
            continue;
        }
        arguments.push(word.to_owned());
        index += 1;
    }
    super::files::create_dir_all(&paths.install())?;
    let log = paths
        .install()
        .join(format!("detached-{}.log", host.now_unix_ms()));
    let pid = host.detach(&arguments, &paths.root, &log)?;
    Answer::new(
        super::done(operation)
            .installation_changed(false)
            .pid(pid)
            .log(&log),
    )
    .with("detached", true)?
    .with("log", super::files::plain(&log))?
    .with(
        "next",
        json!({"status": ["install", "status", "--root", paths.plain]}),
    )
}
