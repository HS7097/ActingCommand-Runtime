// SPDX-License-Identifier: AGPL-3.0-only

//! `actingctl install update plan|apply` and `rollback plan|apply` (Workflow #421 INST-1;
//! `contracts/install-cli.md`, "Update and rollback", "The rollback gate"). Apply takes the
//! writer lock, recomputes the plan under it (a different `plan_id` refuses `plan_changed`),
//! evaluates a rollback's gate again, reads S0, then journals its steps: the ledger verification
//! when the `ledger` ranges differ, each changed file staged as `<dst>.new-<ms>.part`, swapped
//! (`tools\` first, a replaced file kept as `<dst>.old-<ms>`), every manifest file verified in
//! place, and the state recorded. Unchanged files are never touched; nothing is deleted.
//!
//! The rollback gate's open-time part (INST-1b-F, #421 deploy design §4 and §8 S1) judges the
//! target build's own last ledger open on this root: the start logs are attributed to builds by
//! the operation journals (the build live on the root when each start began), and the open's
//! time is the sum of all eight phases its `ledger_opened` daemon line reports, unscaled. Without
//! an open of the target the newest open of any build is judged, with the caveat
//! `open_time_from_other_build`.

use super::files;
use super::host::Host;
use super::journal::{self, Opening, Operation, PlannedStep, Previous, S0, StepKind, StepState};
use super::lock::WriterLock;
use super::manifest::{self, Half};
use super::plan::{self, Action, PlanInputs, PlanKind, Planned, Refused};
use super::programs;
use super::state::{self, LiveBuild, OperationKind, Paths};
use super::{Answer, Flags};
use crate::codes::{InstallCaveat, InstallLocation, InstallRefusal, RollbackGatePart};
use actingcommand_contract::outcome::{Outcome, OutcomeEnvelope};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// How much of a start log the open-time part reads, from its beginning: the open is among a
/// start's first lines.
const LOG_HEAD_BYTES: u64 = 4 * 1024 * 1024;
/// The configuration member of the target's open deadline.
const OPEN_TIMEOUT: &str = "ledger_observer.open_timeout_ms";
/// The daemon line of a start log.
const DAEMON_LINE: &str = "OUTCOME actingd: ";
/// The phases of a ledger open as its daemon line reports them: the four of a complete read,
/// then (since LN-8) the four of the writer open. A build before LN-8 reports only the first
/// four; a phase a line does not report counts 0.
const OPEN_PHASES: [&str; 8] = [
    "sql_read_ms",
    "verify_ms",
    "material_ms",
    "restore_ms",
    "meta_ms",
    "tail_ms",
    "index_ms",
    "import_ms",
];
/// How many of the first phases every open line carries (since #378 A6a): how a line is known
/// to report an open, by its values rather than by the spelling of its code.
const OPEN_LINE_PHASES: usize = 4;
/// A start log's name: `actingd-<ms>.log`, `<ms>` the Unix milliseconds of the start.
const START_LOG_PREFIX: &str = "actingd-";
const START_LOG_SUFFIX: &str = ".log";

fn kind_flag(kind: PlanKind) -> &'static str {
    match kind {
        PlanKind::Update => "--build",
        PlanKind::Rollback => "--to",
    }
}

/// `update plan` / `rollback plan`.
pub(crate) fn plan_command(
    words: &[&str],
    host: &dyn Host,
    kind: PlanKind,
) -> Result<Answer, Outcome> {
    let flags = Flags::parse(
        words,
        &["--root", kind_flag(kind)],
        &["--preview", "--accept-ledger-one-way"],
    )?;
    let paths = super::resolve_root(&flags)?;
    let inputs = PlanInputs {
        kind,
        commit: flags.required(kind_flag(kind))?.to_ascii_lowercase(),
        accept_ledger_one_way: flags.switch("--accept-ledger-one-way"),
        preview: flags.switch("--preview"),
        operation: kind.location(false),
    };
    let planned = match plan::plan(host, &paths, inputs) {
        Ok(planned) => planned,
        Err(refused) => return Ok(refused.into_answer()),
    };
    if kind == PlanKind::Rollback {
        let gate = gate(host, &paths, &planned);
        let outcome = match gate.refusal(&planned) {
            Some(refusal) => refusal,
            None => gate.with_notes(plan::planned_outcome(&planned)),
        };
        return planned.answer(outcome)?.with("gate", &gate.rows);
    }
    planned.answer(plan::planned_outcome(&planned))
}

/// One part of the rollback gate as the result lists it.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct GateRow {
    part: &'static str,
    result: &'static str,
    values: Value,
}

/// The rollback gate's evaluation.
pub(crate) struct Gate {
    pub(crate) rows: Vec<GateRow>,
    failed: Vec<RollbackGatePart>,
    /// The first page the target's reader refused, when it refused one.
    unreadable: Option<(u64, Outcome)>,
    /// The target's check-config refusal, when it refused the configuration.
    config_refusal: Option<Outcome>,
    /// What the evaluation noted (`open_time_from_other_build`), `note` links of its result.
    notes: Vec<Outcome>,
}

impl Gate {
    fn part(&mut self, part: RollbackGatePart, passes: bool, values: Value) {
        self.rows.push(GateRow {
            part: part.as_str(),
            result: if passes { "pass" } else { "fail" },
            values,
        });
        if !passes {
            self.failed.push(part);
        }
    }

    /// `outcome` with the gate's caveats as `note` links.
    pub(crate) fn with_notes(&self, outcome: Outcome) -> Outcome {
        plan::with_notes(outcome, &self.notes)
    }

    /// The gate's refusal, `None` when every part passes: `rollback_gate_failed` of the plan's
    /// command with every failed part (`rollback_gate_parts`), `ledger_sequence` and the reader's
    /// failure when the target's reader refused a page, the target's check-config refusal as a
    /// link, and the gate's caveats as `note` links.
    pub(crate) fn refusal(&self, planned: &Planned) -> Option<Outcome> {
        if self.failed.is_empty() {
            return None;
        }
        let refusal = super::refusal(planned.inputs.operation, InstallRefusal::RollbackGateFailed)
            .rollback_gate_parts(self.failed.iter().copied())
            .commit_sha(planned.target.commit.clone());
        let refusal = match &self.unreadable {
            Some((sequence, cause)) => refusal.ledger_sequence(*sequence).caused_by(cause.clone()),
            None => refusal,
        };
        let refusal = match &self.config_refusal {
            Some(config) => refusal.caused_by(config.clone()),
            None => refusal,
        };
        Some(self.with_notes(refusal))
    }
}

/// S0 of the live build: the ledger head the operation that installed it recorded.
fn live_s0(paths: &Paths, planned: &Planned, host: &dyn Host) -> Option<u64> {
    let operation_id = planned.state.live.as_ref()?.operation_id.as_ref()?;
    let operation = Operation::load(host, paths, operation_id, planned.inputs.operation).ok()?;
    operation.journal().s0.as_ref()?.head
}

/// When each build became the root's live build, as the operation journals record it, oldest
/// first: from `.0` (Unix milliseconds) on, the build `.1` was live. An adopt records the build
/// it found live; an update or rollback the build live when it began, its target once it ended
/// with the state recorded, and the previous build again once an undo of it ended. Before the
/// first journalled operation the journals say nothing.
pub(crate) fn live_changes(
    host: &dyn Host,
    paths: &Paths,
    operation: InstallLocation,
) -> Vec<(u64, String)> {
    let mut changes = Vec::new();
    for id in journal::operation_ids(paths).unwrap_or_default() {
        let Ok(loaded) = Operation::load(host, paths, &id, operation) else {
            continue;
        };
        let started = loaded.ms();
        let journal = loaded.journal();
        let recorded = journal.steps.iter().any(|step| {
            step.kind == StepKind::RecordState
                && matches!(step.state, StepState::Done | StepState::Undone)
        });
        match journal.kind {
            OperationKind::Adopt => {
                let commit = journal
                    .steps
                    .first()
                    .and_then(|step| step.intent.get("commit"))
                    .and_then(Value::as_str);
                if recorded && let Some(commit) = commit {
                    changes.push((started, commit.to_owned()));
                }
            }
            OperationKind::Update | OperationKind::Rollback => {
                if let Some(previous) = &journal.previous.live {
                    changes.push((started, previous.clone()));
                }
                let target = journal.inputs.get("commit").and_then(Value::as_str);
                if recorded
                    && let Some(target) = target
                    && let Some(finished) = journal.finished_at_unix_ms
                {
                    changes.push((finished, target.to_owned()));
                }
                if let Some(undo) = &journal.undo
                    && let Some(undone) = undo.finished_at_unix_ms
                    && let Some(previous) = &journal.previous.live
                {
                    changes.push((undone, previous.clone()));
                }
            }
            OperationKind::ReleaseStage => {}
        }
    }
    changes.sort_by_key(|(since, _)| *since);
    changes
}

/// The build live at `at_unix_ms` by `changes` (the latest change at or before it).
fn live_at(changes: &[(u64, String)], at_unix_ms: u64) -> Option<&str> {
    changes
        .iter()
        .rev()
        .find(|(since, _)| *since <= at_unix_ms)
        .map(|(_, commit)| commit.as_str())
}

/// One start log: its path and when its start began.
struct StartLog {
    path: PathBuf,
    at_unix_ms: u64,
}

/// Every start log (`actingd-<ms>.log`) in the folders a start of `root` logs to, the newest
/// first: when its start began is the `<ms>` of its name, else its modified time. A folder that
/// cannot be listed holds no log the part can use.
fn start_logs(host: &dyn Host, root: &Path) -> Vec<StartLog> {
    let mut logs = Vec::new();
    for directory in host.log_dirs(root) {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            let Some(stem) = name
                .strip_prefix(START_LOG_PREFIX)
                .and_then(|rest| rest.strip_suffix(START_LOG_SUFFIX))
            else {
                continue;
            };
            let at_unix_ms = stem.parse::<u64>().ok().or_else(|| {
                entry
                    .metadata()
                    .and_then(|metadata| metadata.modified())
                    .ok()
                    .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
                    .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
            });
            if let Some(at_unix_ms) = at_unix_ms {
                logs.push(StartLog {
                    path: entry.path(),
                    at_unix_ms,
                });
            }
        }
    }
    logs.sort_by_key(|log| std::cmp::Reverse(log.at_unix_ms));
    logs
}

/// The ledger open a start log reports: the sum of the phases of its first daemon line, within
/// the log's first `LOG_HEAD_BYTES`, whose top carries the phases of an open; `None` when it
/// reports none (a start refused before its open, or a log that cannot be read).
fn open_of(path: &Path) -> Option<u64> {
    let file = fs::File::open(path).ok()?;
    let mut reader = BufReader::new(file.take(LOG_HEAD_BYTES));
    let mut bytes = Vec::new();
    loop {
        bytes.clear();
        match reader.read_until(b'\n', &mut bytes) {
            Ok(0) | Err(_) => return None,
            Ok(_) => {}
        }
        let line = String::from_utf8_lossy(&bytes);
        let Some(envelope) = line.trim().strip_prefix(DAEMON_LINE) else {
            continue;
        };
        let Ok(envelope) = serde_json::from_str::<OutcomeEnvelope>(envelope) else {
            continue;
        };
        let outcome = envelope.into_outcome();
        let phases = OPEN_PHASES
            .iter()
            .map(|key| outcome.value(key).and_then(Value::as_u64))
            .collect::<Vec<_>>();
        if phases[..OPEN_LINE_PHASES].iter().all(Option::is_some) {
            return Some(phases.into_iter().flatten().fold(0, u64::saturating_add));
        }
    }
}

/// The rollback gate (`contracts/install-cli.md`, "The rollback gate"): the interface edges, the
/// target's own reader over the ledger after S0, the target's own last open within its deadline
/// minus the margin, and the live configuration passing the target's check-config.
pub(crate) fn gate(host: &dyn Host, paths: &Paths, planned: &Planned) -> Gate {
    let mut gate = Gate {
        rows: Vec::new(),
        failed: Vec::new(),
        unreadable: None,
        config_refusal: None,
        notes: Vec::new(),
    };
    let operation = planned.inputs.operation;
    let target = planned.target.commit.as_str();
    // 1. The interface edges (the check-config edge is part 4).
    let edges_pass = planned
        .edges
        .iter()
        .filter(|edge| edge.check != "next_check_config")
        .all(|edge| edge.result != plan::EdgeResult::Fail);
    gate.part(RollbackGatePart::InterfaceEdges, edges_pass, Value::Null);
    // 2. The target's own reader reads every page after S0.
    let target_tools = paths.build(target).join(Half::Tools.dir());
    match live_s0(paths, planned, host) {
        None => gate.part(
            RollbackGatePart::LedgerReadability,
            false,
            json!({"s0": Value::Null}),
        ),
        Some(s0) => match programs::read_pages(host, &target_tools, &planned.state_root, s0) {
            Ok(head) => gate.part(
                RollbackGatePart::LedgerReadability,
                true,
                json!({"s0": s0, "head": head}),
            ),
            Err(refused) => {
                gate.part(
                    RollbackGatePart::LedgerReadability,
                    false,
                    json!({"s0": s0, "first_refused": refused.sequence}),
                );
                gate.unreadable = Some((refused.sequence, refused.cause));
            }
        },
    }
    // 3. The target build's own last open on this root, all eight phases, within the target's
    // deadline minus the margin; without one, the newest open of another build, noted.
    let deadline = actingcommand_runtime_config::member(&planned.config, OPEN_TIMEOUT)
        .and_then(Value::as_u64)
        .or_else(|| {
            actingcommand_runtime_config::member(
                &plan::defaults_of(&paths.build(target)),
                OPEN_TIMEOUT,
            )
            .and_then(Value::as_u64)
        });
    let margin = planned.settings.rollback_open_margin_ms;
    let changes = live_changes(host, paths, operation);
    let logs = start_logs(host, &paths.root);
    let open_in = |log: &StartLog| open_of(&log.path).map(|sum| (log.path.clone(), sum));
    let (open, from_target) = match logs
        .iter()
        .filter(|log| live_at(&changes, log.at_unix_ms) == Some(target))
        .find_map(open_in)
    {
        Some(own) => (Some(own), true),
        None => (logs.iter().find_map(open_in), false),
    };
    if let Some((log, _)) = &open
        && !from_target
    {
        gate.notes.push(
            super::caveat(operation, InstallCaveat::OpenTimeFromOtherBuild)
                .commit_sha(target.to_owned())
                .path(log),
        );
    }
    let estimate = open.as_ref().map(|(_, sum)| *sum);
    let open_passes = matches!((deadline, estimate), (Some(deadline), Some(estimate))
        if estimate.saturating_add(margin) <= deadline);
    gate.part(
        RollbackGatePart::OpenTime,
        open_passes,
        json!({
            "deadline_ms": deadline,
            "margin_ms": margin,
            "estimate_ms": estimate,
            "log": open.as_ref().map(|(log, _)| files::plain(log)),
            "from_target_build": open.as_ref().map(|_| from_target),
        }),
    );
    // 4. The live configuration passes the target's check-config.
    let passes = planned.target_check.passed();
    if !passes {
        gate.config_refusal = Some(planned.target_check.received.clone());
    }
    gate.part(
        RollbackGatePart::Config,
        passes,
        json!({"check_config": planned.target_check.summary}),
    );
    gate
}

/// A step's intent: the file, its part, the kept build's source and the wanted SHA-256.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StageIntent {
    dst: String,
    part: String,
    source: String,
    sha256: String,
}

/// A swap's intent: the file, its part, where a replaced file goes, and both SHA-256 values.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SwapIntent {
    pub(crate) dst: String,
    pub(crate) part: String,
    pub(crate) old: Option<String>,
    pub(crate) old_sha256: Option<String>,
    pub(crate) sha256: String,
}

fn update_steps(planned: &Planned, ms: u64) -> Vec<PlannedStep> {
    let commit = &planned.target.commit;
    let whole = json!({"commit": commit});
    let mut steps = Vec::new();
    if planned.ledger_verify {
        steps.push((
            StepKind::LedgerVerify,
            format!("install/builds/{commit}/runtime/{}", programs::DAEMON),
            whole.clone(),
        ));
    }
    for file in planned.changes() {
        let part = format!("{}.new-{ms}.part", file.path);
        steps.push((
            StepKind::StageFile,
            part.clone(),
            json!({
                "dst": file.path,
                "part": part,
                "source": format!("install/builds/{commit}/{}", file.path),
                "sha256": file.sha256,
            }),
        ));
    }
    for file in planned.changes() {
        let replaced = file.action == Action::Replace;
        steps.push((
            StepKind::SwapFile,
            file.path.clone(),
            json!({
                "dst": file.path,
                "part": format!("{}.new-{ms}.part", file.path),
                "old": replaced.then(|| format!("{}.old-{ms}", file.path)),
                "old_sha256": if replaced { file.live_sha256.clone() } else { None },
                "sha256": file.sha256,
            }),
        ));
    }
    steps.push((StepKind::VerifyTree, "runtime".to_owned(), whole.clone()));
    steps.push((
        StepKind::RecordState,
        "install/state.json".to_owned(),
        whole,
    ));
    steps
}

/// `update apply` / `rollback apply`.
pub(crate) fn apply_command(
    words: &[&str],
    host: &dyn Host,
    kind: PlanKind,
) -> Result<Answer, Outcome> {
    let operation = kind.location(true);
    let flags = Flags::parse(
        words,
        &["--root", kind_flag(kind), "--plan"],
        &["--accept-ledger-one-way"],
    )?;
    let paths = super::resolve_root(&flags)?;
    let given = flags.required("--plan")?.to_owned();
    let inputs = PlanInputs {
        kind,
        commit: flags.required(kind_flag(kind))?.to_ascii_lowercase(),
        accept_ledger_one_way: flags.switch("--accept-ledger-one-way"),
        preview: false,
        operation,
    };
    journal::refuse_unfinished(operation, &paths)?;
    let loaded = plan::load(&paths, operation)?;
    let _lock = WriterLock::take(&paths, loaded.settings.lock_wait_ms, operation)?;
    journal::refuse_unfinished(operation, &paths)?;
    let planned = match plan::plan(host, &paths, inputs) {
        Ok(planned) => planned,
        Err(Refused::Outcome(outcome)) => return Err(outcome),
        Err(Refused::Answer(answer)) => return Ok(answer),
    };
    if planned.id != given {
        return Err(super::refusal(operation, InstallRefusal::PlanChanged).plan_id(given));
    }
    if planned.changes().next().is_none() && planned.live.commit == planned.target.commit {
        return Answer::new(
            super::done(operation)
                .commit_sha(planned.target.commit.clone())
                .installation_changed(false),
        )
        .with("commit", planned.target.commit.as_str())?
        .with("unchanged", true);
    }
    let mut notes = planned.warnings.clone();
    if kind == PlanKind::Rollback {
        // The gate again, under the writer lock, immediately before the first change.
        let gate = gate(host, &paths, &planned);
        if let Some(refusal) = gate.refusal(&planned) {
            return planned.answer(refusal)?.with("gate", &gate.rows);
        }
        notes.extend(gate.notes.iter().cloned());
    }
    // S0: the ledger head before the first change, read with the live build's reader.
    let head = programs::ledger_head(
        host,
        &paths.root.join(Half::Tools.dir()),
        &planned.state_root,
    )
    .map_err(|cause| {
        super::refusal(operation, InstallRefusal::LedgerHeadUnreadable)
            .path(&planned.state_root)
            .caused_by(cause)
    })?;
    let opening = Opening {
        kind: match kind {
            PlanKind::Update => OperationKind::Update,
            PlanKind::Rollback => OperationKind::Rollback,
        },
        inputs: json!({
            "commit": planned.target.commit,
            "accept_ledger_one_way": planned.inputs.accept_ledger_one_way,
        }),
        plan_id: Some(planned.id.clone()),
        plan: Some(planned.document.clone()),
        acknowledged: if planned.inputs.accept_ledger_one_way {
            vec!["accept_ledger_one_way".to_owned()]
        } else {
            Vec::new()
        },
        s0: Some(S0 {
            head: Some(head),
            epoch: None,
            pauses: None,
            instructions: None,
        }),
        previous: Previous {
            live: Some(planned.live.commit.clone()),
            state: Some(planned.state.clone()),
        },
    };
    let opened = Operation::open(host, &paths, opening, |ms| update_steps(&planned, ms))?;
    let answer = run(opened)?;
    let renamed = &planned.running.renamed;
    if !renamed.is_empty() {
        notes.push(
            super::caveat(operation, InstallCaveat::McpClientsOnPreviousBuild).count(renamed.len()),
        );
    }
    let answer = Answer {
        outcome: plan::with_notes(answer.outcome, &notes),
        ..answer
    };
    answer
        .with("running_renamed", renamed)?
        .with("extra", &planned.extra)
}

/// Continues an unfinished update or rollback under the writer lock.
pub(crate) fn resume(operation: Operation<'_>) -> Result<Answer, Outcome> {
    let loaded = plan::load(operation.paths(), InstallLocation::Resume)
        .map_err(|cause| operation.interrupted(cause))?;
    let _lock = WriterLock::take(
        operation.paths(),
        loaded.settings.lock_wait_ms,
        InstallLocation::Resume,
    )
    .map_err(|cause| operation.interrupted(cause))?;
    run(operation)
}

/// Whether `path` holds `sha256`.
fn holds(path: &Path, sha256: &str) -> bool {
    files::sha256_file(path).is_ok_and(|(actual, _)| actual == sha256)
}

/// The steps of an update or rollback, each skipped when its postcondition already holds.
fn run(mut operation: Operation<'_>) -> Result<Answer, Outcome> {
    let paths = operation.paths().clone();
    let journal = operation.journal().clone();
    let command = journal.kind.location();
    let target: String = journal.inputs["commit"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| {
            operation.interrupted(journal::state_invalid(
                command,
                paths.journal(operation.id()),
                None,
            ))
        })?;
    let from = journal.previous.live.clone();
    let (mut replaced, mut added) = (0_usize, 0_usize);
    for step in &journal.steps {
        let n = step.n;
        if step.kind == StepKind::SwapFile {
            let intent: SwapIntent = operation
                .intent(n)
                .map_err(|cause| operation.interrupted(cause))?;
            if intent.old.is_some() {
                replaced += 1;
            } else {
                added += 1;
            }
        }
        if operation.journal().steps[n - 1].state == StepState::Done {
            continue;
        }
        match step.kind {
            StepKind::LedgerVerify => {
                operation
                    .start(n)
                    .map_err(|cause| operation.interrupted(cause))?;
                let config = paths.config();
                if let Err(cause) = programs::ledger_verify(
                    operation.host(),
                    &paths.build(&target).join(Half::Runtime.dir()),
                    &config,
                ) {
                    // Nothing has changed yet: the operation ends refused.
                    let refusal = super::refusal(command, InstallRefusal::LedgerVerifyFailed)
                        .commit_sha(target.clone())
                        .caused_by(cause);
                    operation.finish(&refusal)?;
                    return Err(refusal);
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
                    files::copy_file(&paths.at(&intent.source), &part)
                        .map_err(|cause| operation.interrupted(cause))?;
                    if !holds(&part, &intent.sha256) {
                        return Err(operation.interrupted(
                            super::refusal(command, InstallRefusal::BuildInvalid)
                                .commit_sha(target.clone())
                                .path(paths.at(&intent.source)),
                        ));
                    }
                }
            }
            StepKind::SwapFile => {
                let intent: SwapIntent = operation
                    .intent(n)
                    .map_err(|cause| operation.interrupted(cause))?;
                let dst = paths.at(&intent.dst);
                let part = paths.at(&intent.part);
                if !holds(&dst, &intent.sha256) {
                    operation
                        .start(n)
                        .map_err(|cause| operation.interrupted(cause))?;
                    swap(
                        command,
                        &dst,
                        &part,
                        intent.old.as_deref().map(|old| paths.at(old)),
                    )
                    .map_err(|cause| operation.interrupted(cause))?;
                    if !holds(&dst, &intent.sha256) {
                        return Err(operation.interrupted(
                            super::refusal(command, InstallRefusal::BuildInvalid).path(&dst),
                        ));
                    }
                }
            }
            StepKind::VerifyTree => {
                operation
                    .start(n)
                    .map_err(|cause| operation.interrupted(cause))?;
                match manifest::verify_build(&paths.root) {
                    Ok(build) if build.commit == target => {}
                    _ => {
                        let cause = super::adopt::verify_live(command, &paths)
                            .err()
                            .unwrap_or_else(|| {
                                super::refusal(command, InstallRefusal::LiveBuildInvalid)
                                    .path(&paths.root)
                            });
                        return Err(operation.interrupted(cause));
                    }
                }
            }
            StepKind::RecordState => {
                operation
                    .start(n)
                    .map_err(|cause| operation.interrupted(cause))?;
                let mut state = state::load_state_or_default(&paths)
                    .map_err(|cause| operation.interrupted(cause))?;
                state.live = Some(LiveBuild {
                    commit: target.clone(),
                    operation_id: Some(operation.id().to_owned()),
                });
                state::write_state(&paths, &state).map_err(|cause| operation.interrupted(cause))?;
            }
            StepKind::Extract | StepKind::CopyBuild | StepKind::PlaceBuild => {
                return Err(operation.interrupted(journal::state_invalid(
                    command,
                    paths.journal(operation.id()),
                    None,
                )));
            }
        }
        operation
            .done(n, None)
            .map_err(|cause| operation.interrupted(cause))?;
    }
    let operation_id = operation.id().to_owned();
    let outcome = super::done(command)
        .install_operation_id(operation_id.clone())
        .commit_sha(target.clone())
        .installation_changed(true);
    operation.finish(&outcome)?;
    let verified = manifest::verify_build(&paths.root)
        .map(|build| build.runtime.files.len() + build.tools.files.len())
        .unwrap_or(0);
    Answer::new(outcome)
        .with("operation_id", &operation_id)?
        .with("from_commit", from)?
        .with("to_commit", &target)?
        .with("replaced", replaced)?
        .with("added", added)?
        .with("verified", verified)?
        .with(
            "next",
            json!({"undo": ["install", "undo", "--root", paths.plain, "--operation", operation_id]}),
        )
}

/// Swaps `part` in at `dst`: a file at `dst` that is not yet the new one is renamed `old` first
/// (a new file has no `old`, so a file found at its place refuses `build_invalid` for the
/// command `command`), then the part renamed `dst`. Each half is done only when it is missing,
/// so a swap interrupted between the two renames is finished.
pub(crate) fn swap(
    command: InstallLocation,
    dst: &Path,
    part: &Path,
    old: Option<PathBuf>,
) -> Result<(), Outcome> {
    if dst.exists() {
        match &old {
            Some(old) if !old.exists() => files::rename(dst, old)?,
            Some(_) => {}
            None => {
                return Err(super::refusal(command, InstallRefusal::BuildInvalid).path(dst));
            }
        }
    }
    if part.exists() && !dst.exists() {
        files::rename(part, dst)?;
    }
    Ok(())
}
