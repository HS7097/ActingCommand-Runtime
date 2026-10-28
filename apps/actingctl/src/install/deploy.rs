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
//!   fail-class check, or a start that ended, rolls the update back by itself. The rollback's
//!   steps are journalled before anything acts (fix round 1, review S2 H2): the new Runtime's
//!   close, the reversal, the previous build's start and checks, and the release of the hold
//!   (when its checks passed); the command ends refused `deploy_rolled_back`. The rollback gate
//!   toward the previous build (with this operation's S0) is evaluated before the new Runtime
//!   closes (Workflow #421 6441776975, P12): when it fails nothing is closed, the new build, its
//!   Runtime and the hold stay, and the command ends `rollback_gate_failed`. After the resume
//!   nothing rolls back by itself. `--detach` runs the same command again through the watchdog's
//!   manual-run method and answers at once.
//!
//! A stop or composite refused before it changed a file releases the hold it took (fix round 1,
//! review S2 M4) and still ends refused; one that already closed the Runtime names `install
//! start` in `next`. Every answer reports the runs its drain cut (`runs_not_continued`) and the
//! idle wakes the Runtime derives again (`idle_wakes_rederived`), never silently (review S2 M6).
//! Every step is journalled and resumable (`install resume`); nothing is killed or deleted.

use super::guards;
use super::hold::{self, HoldInputs, HoldReport, PauseIntent, ReleaseEvidence, ReleaseIntent};
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
use actingcommand_contract::SchedulingPauseReason;
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

/// The members of an answer whose hold stays: `next` names `release-hold` with its reason, and
/// `install start` when the Runtime is down (closed, or a start that failed: review S2 M4).
fn hold_kept(
    paths: &Paths,
    reason: Option<&SchedulingPauseReason>,
    runtime_down: bool,
) -> Map<String, Value> {
    let mut members = Map::new();
    let mut next = Map::new();
    if runtime_down {
        next.insert(
            "start".to_owned(),
            json!(["install", "start", "--root", paths.plain]),
        );
    }
    if let Some(reason) = reason {
        next.insert(
            "release_hold".to_owned(),
            json!([
                "install",
                "release-hold",
                "--root",
                paths.plain,
                "--reason",
                reason
            ]),
        );
        members.insert(
            "hold".to_owned(),
            HoldReport {
                reason,
                kept: true,
                release: None,
            }
            .value(),
        );
    }
    if !next.is_empty() {
        members.insert("next".to_owned(), Value::Object(next));
    }
    members
}

/// The hold of a journal: its inputs' `hold`, else its pause step's reason.
fn hold_of(journal: &journal::Journal) -> Option<SchedulingPauseReason> {
    journal
        .inputs
        .get("hold")
        .and_then(Value::as_str)
        .and_then(|given| SchedulingPauseReason::new(given).ok())
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
        StepKind::Pause => pause(operation, n, loaded, notes),
        StepKind::Close => close(operation, n, loaded, notes),
        StepKind::Start => start(operation, n, loaded, notes),
        StepKind::PostCheck => post_check(operation, n, loaded, notes),
        StepKind::ReleaseHold => release_hold(operation, n, loaded),
        StepKind::ReverseFiles => reverse_files(operation, n, loaded, notes),
        _ => Err(state_invalid(
            operation.journal().kind.location(),
            operation.paths().journal(operation.id()),
            None,
        )),
    }
}

fn pause(
    operation: &mut Operation<'_>,
    n: usize,
    loaded: &Loaded,
    notes: &mut Vec<Outcome>,
) -> Result<StepEnd, Outcome> {
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
    let host = operation.host();
    match hold::run_pause(
        host,
        &loaded.state_root,
        command,
        &intent,
        &s0_pauses,
        drained,
        loaded.settings.stop_busy_wait_ms,
    ) {
        Ok(drained) => Ok(StepEnd::Done(Some(json!({
            "deferred": deferred,
            "drained": hold::drained_value(&drained),
            "held": intent.instances,
            "known": intent.known,
            "runs_stopped_at_checkpoint": hold::runs_cut(&drained),
        })))),
        Err(hold::PauseRefused { refusal, drained }) => {
            // The runs the drains before the refusal cut are journalled and reported (re-review
            // S2 N3).
            let cut = hold::runs_cut(&drained);
            operation.progress(
                n,
                json!({
                    "deferred": deferred,
                    "drained": hold::drained_value(&drained),
                    "held": intent.instances,
                    "known": intent.known,
                    "runs_stopped_at_checkpoint": cut,
                }),
            )?;
            if cut > 0 {
                notes.push(super::caveat(command, InstallCaveat::RunsNotContinued).count(cut));
            }
            // Nothing changed yet (review S2 M4): the instances this hold paused are released
            // again, and the command still ends refused. A release that fails keeps the hold.
            match hold::release(host.runtime(), &loaded.state_root, command, &intent.reason) {
                Ok(released) => {
                    let mut members = Map::new();
                    members.insert(
                        "hold".to_owned(),
                        HoldReport {
                            reason: &intent.reason,
                            kept: false,
                            release: Some(&json!({"resumed": released})),
                        }
                        .value(),
                    );
                    Ok(StepEnd::Ends(refusal, members))
                }
                Err(release_failure) => {
                    notes.push(release_failure);
                    Ok(StepEnd::Ends(
                        refusal,
                        hold_kept(&paths, Some(&intent.reason), false),
                    ))
                }
            }
        }
    }
}

/// The instructions and the other parties' pauses as the close reads them before
/// `request-shutdown` (review S2 L5): S0 again, without the hold's own pauses.
fn reread_s0(operation: &mut Operation<'_>, loaded: &Loaded) -> Result<(), Outcome> {
    let runtime = operation.host().runtime();
    let Ok(Some(mut seen)) = runtime.status(&loaded.state_root) else {
        return Ok(());
    };
    let Ok(config) = runtime.config(&loaded.state_root) else {
        return Ok(());
    };
    if let Some(intent) = pause_intent(operation.journal()) {
        for instance in &mut seen.instances {
            if intent.instances.contains(&instance.alias)
                && instance
                    .pause
                    .as_ref()
                    .is_some_and(|pause| pause.reason == intent.reason)
            {
                instance.pause = None;
            }
        }
    }
    operation.record_s0_reads(
        hold::pauses_value(&seen),
        hold::instructions_value(&config.instructions),
    )
}

fn close(
    operation: &mut Operation<'_>,
    n: usize,
    loaded: &Loaded,
    notes: &mut Vec<Outcome>,
) -> Result<StepEnd, Outcome> {
    let command = operation.journal().kind.location();
    let paths = operation.paths().clone();
    let reason = hold_of(operation.journal());
    let intent = operation.journal().steps[n - 1].intent.clone();
    let wait_ms = intent
        .get("shutdown_wait_ms")
        .and_then(Value::as_u64)
        .unwrap_or(loaded.settings.stop_shutdown_wait_ms);
    let rollback = of_rollback(&intent);
    // The rollback gate before the new Runtime closes (Workflow #421 6441776975, P12): a gate
    // that fails ends the command before anything is closed, and the new Runtime keeps running,
    // held. A close resumed past the gate's pass does not evaluate it again.
    if rollback
        && let Some(reverse) = reversal_after(operation.journal(), n)
        && !gate_passed(operation.journal(), reverse)
        && let Some(end) = rollback_gate(operation, reverse, loaded, notes, false)?
    {
        return Ok(end);
    }
    operation.start(n)?;
    if !rollback {
        reread_s0(operation, loaded)?;
    }
    let host = operation.host();
    let (evidence, caveat) = match hold::run_close(
        host.runtime(),
        &paths.root,
        &loaded.state_root,
        command,
        wait_ms,
    ) {
        Ok(closed) => closed,
        Err(refusal) if rollback => {
            // The new Runtime does not stop: nothing is killed; the rollback ends here.
            let failed = rollback_failed(operation.journal());
            let refusal = super::refusal(command, InstallRefusal::PostCheckFailed)
                .count(failed.len())
                .joined(CauseRelation::CausedBy, refusal);
            let mut members = hold_kept(&paths, reason.as_ref(), false);
            members.insert("failed_checks".to_owned(), json!(failed));
            return Ok(StepEnd::Ends(refusal, members));
        }
        Err(refusal) => {
            // `request-shutdown` sent and its wait passed: the Runtime may still go down, so
            // `next` names `install start` too, which refuses a Runtime still running (re-review
            // S2 N1).
            return Ok(StepEnd::Ends(
                refusal,
                hold_kept(&paths, reason.as_ref(), true),
            ));
        }
    };
    notes.extend(caveat);
    if rollback {
        // The rollback's close of the new Runtime: S0 stays the operation's own.
        if let Err(refusal) = launch::refuse_running(host, &paths, &loaded.state_root, command) {
            return Ok(StepEnd::Ends(
                refusal,
                hold_kept(&paths, reason.as_ref(), false),
            ));
        }
        return Ok(StepEnd::Done(Some(evidence)));
    }
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
            return Ok(StepEnd::Ends(
                refusal,
                hold_kept(&paths, reason.as_ref(), true),
            ));
        }
    };
    operation.record_s0_head(head)?;
    // Nothing of the Runtime may still run before a file changes.
    if let Err(refusal) = launch::refuse_running(host, &paths, &loaded.state_root, command) {
        return Ok(StepEnd::Ends(
            refusal,
            hold_kept(&paths, reason.as_ref(), true),
        ));
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
    let reason = hold_of(operation.journal());
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
    // `--ack-guard memory` of a stop or composite acknowledges the row before the stop only;
    // the start's own guard is acknowledged only by `install start --ack-guard memory` (review
    // S2 M2).
    let held = operation
        .journal()
        .steps
        .iter()
        .any(|step| step.kind == StepKind::Pause);
    let acks = acks_of(operation.journal())
        .into_iter()
        .filter(|ack| !held || ack != InstallGuard::Memory.as_str())
        .collect::<Vec<_>>();
    let launched = {
        let mut progress = |value: &Value| operation.progress(n, value.clone());
        launch::start(host, &paths, loaded, command, &commit, &acks, &mut progress)
    };
    let launched = match launched {
        Ok(launched) => launched,
        Err(refusal) => {
            return Ok(StepEnd::Ends(
                refusal,
                hold_kept(&paths, reason.as_ref(), true),
            ));
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
        return begin_rollback(operation, n, loaded, vec!["ready".to_owned()], evidence);
    }
    let refusal = super::refusal(command, InstallRefusal::RuntimeStartFailed)
        .path(launched.log.as_path())
        .joined(CauseRelation::CausedBy, failure);
    let mut members = hold_kept(&paths, reason.as_ref(), !launched.left_running);
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
    let ready = started.get("ready_ms").and_then(Value::as_u64);
    let started_epoch = started["ready"]["owner_epoch"].as_str();
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
            started_epoch,
            log: &log,
            s0: journal.s0.as_ref(),
            hold: hold.as_ref(),
            config: &loaded.config,
            config_sha256: &config_sha256,
            begun_config_sha256: journal.inputs.get("config_sha256").and_then(Value::as_str),
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
        return begin_rollback(operation, n, loaded, failed, evidence);
    }
    let refusal = super::refusal(command, InstallRefusal::PostCheckFailed).count(failed.len());
    let mut members = hold_kept(&paths, hold_of(&journal).as_ref(), false);
    members.insert("post_check".to_owned(), json!(postcheck::values(&rows)));
    members.insert("failed_checks".to_owned(), json!(failed));
    Ok(StepEnd::Ends(refusal, members))
}

/// The automatic rollback of a composite update, between the new start and the resume
/// (decision 74 A). Its steps are journalled here, before anything acts (review S2 H2: a crash
/// from now on resumes the rollback and never judges the new build again): the new Runtime's
/// close (nothing is killed), the reversal, the previous build's start and checks, and the
/// release of the hold. The close evaluates the rollback gate toward the previous build, with
/// this operation's S0, before it closes anything (P12); a gate that fails ends the command
/// `rollback_gate_failed` with the new build, its Runtime and the hold kept, a forward fix only.
/// The current step is done with `evidence`.
fn begin_rollback(
    operation: &mut Operation<'_>,
    n: usize,
    loaded: &Loaded,
    failed: Vec<String>,
    evidence: Value,
) -> Result<StepEnd, Outcome> {
    let command = operation.journal().kind.location();
    let paths = operation.paths().clone();
    let journal = operation.journal().clone();
    let reason = hold_of(&journal);
    let Some(previous) = journal.previous.live.clone() else {
        let refusal = super::refusal(command, InstallRefusal::PostCheckFailed).count(failed.len());
        let mut members = hold_kept(&paths, reason.as_ref(), false);
        members.insert("failed_checks".to_owned(), json!(failed));
        members.insert("post_check".to_owned(), evidence);
        return Ok(StepEnd::Ends(refusal, members));
    };
    let mut steps: Vec<PlannedStep> = vec![
        (
            StepKind::Close,
            "runtime".to_owned(),
            json!({"rollback": true, "shutdown_wait_ms": loaded.settings.stop_shutdown_wait_ms}),
        ),
        (
            StepKind::ReverseFiles,
            "runtime".to_owned(),
            json!({"to": previous, "failed": failed}),
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
            ReleaseIntent::value(reason, true),
        ));
    }
    // The planned steps not begun (the new build's checks, the release) are replaced.
    operation.skip_planned(n + 1, "rollback")?;
    operation.append_steps(steps)?;
    let mut evidence = evidence;
    evidence["rollback"] = json!({"to": previous, "failed": failed});
    Ok(StepEnd::Done(Some(evidence)))
}

/// Why the automatic rollback began: its reversal step's `failed`.
fn rollback_failed(journal: &journal::Journal) -> Vec<String> {
    journal
        .steps
        .iter()
        .find(|step| step.kind == StepKind::ReverseFiles)
        .and_then(|step| step.intent.get("failed"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

/// The automatic rollback's reversal step after step `n` (the rollback's close).
fn reversal_after(journal: &journal::Journal, n: usize) -> Option<usize> {
    journal.steps[n..]
        .iter()
        .find(|step| step.kind == StepKind::ReverseFiles)
        .map(|step| step.n)
}

/// Whether the reversal step `reverse` journals the rollback gate's pass.
fn gate_passed(journal: &journal::Journal, reverse: usize) -> bool {
    journal.steps[reverse - 1]
        .evidence
        .as_ref()
        .and_then(|evidence| evidence.get("gate_passed"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// The rollback gate toward the build the update replaced (the reversal step `reverse`'s `to`),
/// with this operation's S0. Workflow #421 6441776975 (P12): the rollback's close evaluates it
/// before the new Runtime closes (`runtime_closed` false), so its plan is a preview: the running
/// Runtime and its owner lock, which the close then ends, do not refuse it, and a running image
/// that would change and may not be renamed refuses as the plan after a close would. A gate that
/// passes journals the rollback and the pass in the reversal step's evidence and answers `None`;
/// one that fails answers [`StepEnd::Ends`] with the command's refusal `rollback_gate_failed` and
/// what stays: the new build and the hold (`next` names `release-hold`, and `install start` only
/// when no Runtime runs under the root, as after a new start that exited), the failed checks and
/// the gate's rows. A reversal that finds no pass journalled evaluates it after the close
/// (`runtime_closed`).
fn rollback_gate(
    operation: &mut Operation<'_>,
    reverse: usize,
    loaded: &Loaded,
    notes: &mut Vec<Outcome>,
    runtime_closed: bool,
) -> Result<Option<StepEnd>, Outcome> {
    let command = operation.journal().kind.location();
    let paths = operation.paths().clone();
    let to = operation.journal().steps[reverse - 1]
        .intent
        .get("to")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| state_invalid(command, paths.journal(operation.id()), None))?;
    let journal = operation.journal().clone();
    let failed = rollback_failed(&journal);
    let host = operation.host();
    let runtime_down =
        runtime_closed || launch::refuse_running(host, &paths, &loaded.state_root, command).is_ok();
    let mut members = hold_kept(&paths, hold_of(&journal).as_ref(), runtime_down);
    members.insert("failed_checks".to_owned(), json!(failed));
    if let Some(checked) = journal
        .steps
        .iter()
        .rev()
        .find(|step| {
            step.kind == StepKind::PostCheck
                && !of_rollback(&step.intent)
                && step
                    .evidence
                    .as_ref()
                    .is_some_and(|e| e.get("skipped").is_none())
        })
        .and_then(|step| step.evidence.clone())
    {
        members.insert("post_check".to_owned(), checked);
    }
    let refused_by = |cause: Outcome| {
        super::refusal(command, InstallRefusal::RollbackGateFailed)
            .rollback_gate_parts([RollbackGatePart::InterfaceEdges])
            .commit_sha(to.clone())
            .joined(CauseRelation::CausedBy, cause)
    };
    let planned = match plan::plan(
        host,
        &paths,
        PlanInputs {
            kind: PlanKind::Rollback,
            commit: to.clone(),
            accept_ledger_one_way: false,
            preview: !runtime_closed,
            operation: command,
        },
    ) {
        Ok(planned) => planned,
        Err(refused) => {
            return Ok(Some(StepEnd::Ends(
                refused_by(refused.into_answer().outcome),
                members,
            )));
        }
    };
    if !runtime_closed && let Some(image) = planned.running.blocked.first() {
        let blocked = super::refusal(command, InstallRefusal::RunningImageBlocks)
            .path(&image.path)
            .pid(image.pid);
        return Ok(Some(StepEnd::Ends(refused_by(blocked), members)));
    }
    let gate = update::gate(host, &paths, &planned);
    members.insert("gate".to_owned(), json!(gate.rows));
    if let Some(refusal) = gate.refusal(&planned) {
        return Ok(Some(StepEnd::Ends(refusal, members)));
    }
    notes.extend(
        gate.with_notes(super::done(command))
            .causes()
            .iter()
            .map(|link| link.outcome().clone()),
    );
    operation.record_rollback(RollbackRecord {
        started_at_unix_ms: host.now_unix_ms(),
        to,
        failed,
        reversed_at_unix_ms: None,
    })?;
    operation.progress(
        reverse,
        json!({"gate_passed": true, "rollback_gate": gate.rows}),
    )?;
    Ok(None)
}

/// The reversal: the rollback gate's pass, which the rollback's close journalled before it closed
/// the new Runtime (a reversal without one evaluates the gate now, once), then no actingd running
/// under the root (checked every time), then the operation's file steps reversed as `install
/// undo` reverses them (each displaced file kept), then the previous build verified in place.
fn reverse_files(
    operation: &mut Operation<'_>,
    n: usize,
    loaded: &Loaded,
    notes: &mut Vec<Outcome>,
) -> Result<StepEnd, Outcome> {
    let command = operation.journal().kind.location();
    let paths = operation.paths().clone();
    let step = operation.journal().steps[n - 1].clone();
    let to = step
        .intent
        .get("to")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| state_invalid(command, paths.journal(operation.id()), None))?;
    operation.start(n)?;
    if !gate_passed(operation.journal(), n)
        && let Some(end) = rollback_gate(operation, n, loaded, notes, true)?
    {
        return Ok(end);
    }
    // No program file is reversed under a running actingd, as `install undo` checks (re-review
    // S2 N4): also a rollback resumed past its gate. One found running leaves the operation
    // unfinished, its gate's pass journalled, for `install resume` once it is gone.
    launch::refuse_running(operation.host(), &paths, &loaded.state_root, command)?;
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
    let gate = operation.journal().steps[n - 1]
        .evidence
        .as_ref()
        .and_then(|evidence| evidence.get("rollback_gate"))
        .cloned();
    Ok(StepEnd::Done(Some(
        json!({"to": to, "kept": kept, "gate_passed": true, "rollback_gate": gate}),
    )))
}

/// The deferred work a hold of `reason` recorded and how its drains ended: the pause step of
/// this operation, else of the newest journalled operation whose hold has that reason.
fn deferred_for(
    operation: &Operation<'_>,
    reason: &SchedulingPauseReason,
) -> (Option<Value>, BTreeMap<String, super::runtime::Drained>) {
    let of = |journal: &journal::Journal| {
        journal
            .steps
            .iter()
            .find(|step| {
                step.kind == StepKind::Pause
                    && PauseIntent::of(&step.intent).is_some_and(|intent| &intent.reason == reason)
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
) -> Result<StepEnd, Outcome> {
    let command = operation.journal().kind.location();
    let paths = operation.paths().clone();
    let intent: ReleaseIntent = operation.intent(n)?;
    let reason = intent.reason;
    // A rollback whose previous build's checks failed keeps the hold.
    if intent.rollback {
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
                ReleaseEvidence {
                    reason: &reason,
                    skipped: Some("post_check_failed"),
                    resumed: None,
                    handed_back: None,
                }
                .value(),
            )));
        }
    }
    operation.start(n)?;
    let host = operation.host();
    let runtime = host.runtime();
    let resumed = match hold::release(runtime, &loaded.state_root, command, &reason) {
        Ok(resumed) => resumed,
        Err(refusal) => {
            return Ok(StepEnd::Ends(
                refusal,
                hold_kept(&paths, Some(&reason), false),
            ));
        }
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
    Ok(StepEnd::Done(Some(
        ReleaseEvidence {
            reason: &reason,
            skipped: None,
            resumed: Some(&resumed),
            handed_back: Some(&handed_back),
        }
        .value(),
    )))
}

/// Whether `notes` already carry the caveat `caveat`.
fn has(notes: &[Outcome], caveat: InstallCaveat) -> bool {
    notes
        .iter()
        .any(|note| note.value("install_caveat").and_then(Value::as_str) == Some(caveat.as_str()))
}

/// The caveats of a hold that an answer must carry (review S2 M6, decision 75's two gaps): the
/// runs its drains stopped at a checkpoint (`runs_not_continued`), and the held instances whose
/// owed idle wakes could not be read and are derived again by the Runtime
/// (`idle_wakes_rederived`). Those of this operation's own pause step, else (a release of the
/// hold) of the newest pause step with the released reason. Each is added once.
fn hold_caveats(operation: &Operation<'_>, config: &Value, notes: &mut Vec<Outcome>) {
    let journal = operation.journal();
    let command = journal.kind.location();
    let own = journal
        .steps
        .iter()
        .find(|step| step.kind == StepKind::Pause && step.state == StepState::Done)
        .map(|step| {
            (
                step.evidence
                    .as_ref()
                    .and_then(|evidence| evidence.get("deferred"))
                    .cloned(),
                hold::drained_of(step.evidence.as_ref()),
                PauseIntent::of(&step.intent).map(|intent| intent.instances),
            )
        });
    let (deferred, drained, held) = match own {
        Some(found) => found,
        None if journal.kind == OperationKind::ReleaseHold => {
            let Some(reason) = hold_of(journal) else {
                return;
            };
            let (deferred, drained) = deferred_for(operation, &reason);
            let held = deferred
                .as_ref()
                .and_then(|deferred| deferred.get("instances"))
                .and_then(Value::as_array)
                .map(|rows| {
                    rows.iter()
                        .filter_map(|row| Some(row.get("alias")?.as_str()?.to_owned()))
                        .collect()
                });
            (deferred, drained, held)
        }
        None => return,
    };
    let cut = hold::runs_cut(&drained);
    if cut > 0 && !has(notes.as_slice(), InstallCaveat::RunsNotContinued) {
        notes.push(super::caveat(command, InstallCaveat::RunsNotContinued).count(cut));
    }
    let waking = hold::idle_waking(config, &held.unwrap_or_default());
    if waking > 0
        && hold::idle_wakes_unread(deferred.as_ref())
        && !has(notes.as_slice(), InstallCaveat::IdleWakesRederived)
    {
        notes.push(super::caveat(command, InstallCaveat::IdleWakesRederived).count(waking));
    }
}

/// Ends `operation` refused: its result recorded and `current.json` retired; the answer carries
/// `members` (what stays: the hold, the build) and the hold's caveats.
pub(crate) fn end(
    operation: Operation<'_>,
    refusal: Outcome,
    members: Map<String, Value>,
    mut notes: Vec<Outcome>,
    loaded: &Loaded,
) -> Result<Answer, Outcome> {
    hold_caveats(&operation, &loaded.config, &mut notes);
    // The operation's id on the family's own refusal; a fatal cause that tops it (the
    // fatal-link rule) keeps its own values.
    let refusal = if refusal.is(crate::codes::ActingctlCode::InstallCommandRefused) {
        refusal.install_operation_id(operation.id().to_owned())
    } else {
        refusal
    };
    let refusal = plan::with_notes(refusal, &notes);
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
    mut notes: Vec<Outcome>,
    counts: FileCounts,
    loaded: &Loaded,
) -> Result<Answer, Outcome> {
    hold_caveats(&operation, &loaded.config, &mut notes);
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
    let reason = hold_of(&journal);
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
        members.extend(hold_kept(&paths, Some(reason), false));
    } else if let (Some(reason), Some(release)) = (&reason, releases.last()) {
        members.insert(
            "hold".to_owned(),
            HoldReport {
                reason,
                kept: false,
                release: Some(release),
            }
            .value(),
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
                ReleaseIntent::value(&hold.reason, false),
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
        guards::foreign_pauses(&seen, Some(&hold.reason), &hold.known),
        guards::schedule_near(plan_seen.as_ref(), now, &held, &loaded.settings),
        guards::adb(host, paths, &loaded.config, &loaded.settings),
    ];
    if with_memory {
        // Before the stop the running Runtime's working set counts: the stop releases it (review
        // S2 M2).
        rows.push(guards::memory_before_stop(host, paths, &loaded.settings));
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
/// acknowledgments, the configuration file's SHA-256 (what `config` is judged against) and the
/// deferred record journalled. A running image that would change and may not be renamed refuses
/// before anything else.
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
            "config_sha256": loaded.config_sha256,
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
        inputs: json!({
            "hold": hold.reason,
            "known_pauses": hold.known,
            "config_sha256": loaded.config_sha256,
        }),
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

/// Whether a start log (`actingd-<ms>.log`) newer than `since_unix_ms` lies in a log folder of
/// `root`: a Runtime started since then (the watchdog, a reboot, another start).
fn started_since(host: &dyn Host, root: &std::path::Path, since_unix_ms: u64) -> bool {
    host.log_dirs(root).iter().any(|directory| {
        std::fs::read_dir(directory)
            .into_iter()
            .flatten()
            .flatten()
            .any(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .and_then(|name| name.strip_prefix("actingd-")?.strip_suffix(".log"))
                    .and_then(|ms| ms.parse::<u64>().ok())
                    .is_some_and(|ms| ms > since_unix_ms)
            })
    })
}

/// The S0 and the hold of the newest journalled operation that stopped the Runtime: what a
/// standalone start's post-start checks judge against; none when a Runtime started after that
/// operation ended (review S2 L4: its S0 is no longer what this start restores).
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
            if journal
                .finished_at_unix_ms
                .is_none_or(|finished| started_since(host, &paths.root, finished))
            {
                return None;
            }
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
            "config_sha256": loaded.config_sha256,
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
    let reason = SchedulingPauseReason::new(flags.required("--reason")?)
        .map_err(|_| super::unusable("--reason"))?;
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
            ReleaseIntent::value(&reason, false),
        )]
    })?;
    update::run(operation, &loaded)
}

/// `install guards --root <root> --for stop|start [--hold <reason>] [--known-pause
/// ALIAS=REASON]...`: the guards the command would evaluate now; nothing changes. A hot
/// placement's guards come with INST-4. `count` is how many guards would refuse.
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
            let hold = hold::parse_hold(&flags)?;
            let known = hold
                .as_ref()
                .map(|hold| hold.known.clone())
                .unwrap_or_default();
            let mut rows = Vec::new();
            match runtime.status(&loaded.state_root) {
                Ok(Some(seen)) => {
                    let held = match &hold {
                        Some(hold) => hold::held_instances(&seen, &hold.reason),
                        None => seen
                            .instances
                            .iter()
                            .filter(|instance| instance.pause.is_none())
                            .map(|instance| instance.alias.clone())
                            .collect(),
                    };
                    let plan = runtime.plan(&loaded.state_root);
                    rows.push(guards::foreign_pauses(
                        &seen,
                        hold.as_ref().map(|hold| &hold.reason),
                        &known,
                    ));
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
                            informational: guard == InstallGuard::ScheduleNear,
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
    let failing = rows.iter().filter(|row| row.refuses()).count();
    let done = plan::with_notes(
        super::done(GUARDS).count(failing),
        &plan::retired_caveats(GUARDS, &loaded.settings),
    );
    Answer::new(done).with("guards", guards::values(&rows))
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
