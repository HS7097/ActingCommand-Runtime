// SPDX-License-Identifier: AGPL-3.0-only

//! `actingctl install update plan|apply` and `rollback plan|apply` (Workflow #421 INST-1;
//! `contracts/install-cli.md`, "Update and rollback", "The rollback gate"). Apply takes the
//! writer lock, recomputes the plan under it (a different `plan_id` is `install_plan_changed`),
//! evaluates a rollback's gate again, reads S0, then journals its steps: the ledger verification
//! when the `ledger` ranges differ, each changed file staged as `<dst>.new-<ms>.part`, swapped
//! (`tools\` first, a replaced file kept as `<dst>.old-<ms>`), every manifest file verified in
//! place, and the state recorded. Unchanged files are never touched; nothing is deleted.

use super::files;
use super::host::Host;
use super::journal::{self, Opening, Operation, PlannedStep, Previous, S0, StepKind, StepState};
use super::lock::WriterLock;
use super::manifest::{self, Half};
use super::plan::{self, Action, PlanInputs, PlanKind, Planned, Refused};
use super::programs;
use super::state::{self, LiveBuild, OperationKind, Paths};
use super::{Answer, Flags};
use crate::codes::{ActingctlCode, InstallRefusal, RollbackGatePart};
use actingcommand_contract::outcome::{CauseRelation, Outcome, OutcomeEnvelope};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

/// How much of a start log the open-time part reads, from its beginning.
const LOG_HEAD_BYTES: u64 = 4 * 1024 * 1024;
/// The configuration member of the target's open deadline.
const OPEN_TIMEOUT: &str = "ledger_observer.open_timeout_ms";
/// The daemon line of a start log.
const DAEMON_LINE: &str = "OUTCOME actingd: ";
/// The values of the daemon line that reports the ledger open's phases.
const OPEN_PHASES: [&str; 4] = ["sql_read_ms", "verify_ms", "material_ms", "restore_ms"];
const OPEN_EVENTS: &str = "event_count";

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
    };
    let planned = match plan::plan(host, &paths, inputs) {
        Ok(planned) => planned,
        Err(refused) => return Ok(refused.into_answer()),
    };
    if kind == PlanKind::Rollback {
        let gate = gate(host, &paths, &planned);
        if let Some(refusal) = gate.refusal(&planned) {
            return planned.answer(refusal)?.with("gate", &gate.rows);
        }
        return planned
            .answer(plan::planned_outcome(&planned))?
            .with("gate", &gate.rows);
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

    /// The gate's refusal, `None` when every part passes: `install_rollback_ledger_unreadable`
    /// when only the readability failed, else `install_rollback_refused` with every failed part.
    pub(crate) fn refusal(&self, planned: &Planned) -> Option<Outcome> {
        if self.failed.is_empty() {
            return None;
        }
        let commit = planned.target.commit.clone();
        if let ([RollbackGatePart::LedgerReadability], Some((sequence, cause))) =
            (self.failed.as_slice(), &self.unreadable)
        {
            return Some(
                Outcome::new(ActingctlCode::InstallRollbackLedgerUnreadable)
                    .ledger_sequence(*sequence)
                    .commit_sha(commit)
                    .caused_by(cause.clone()),
            );
        }
        let outcome = Outcome::new(ActingctlCode::InstallRollbackRefused)
            .rollback_gate_parts(self.failed.iter().copied())
            .commit_sha(commit);
        let outcome = match &self.unreadable {
            Some((sequence, cause)) => outcome.caused_by(
                Outcome::new(ActingctlCode::InstallRollbackLedgerUnreadable)
                    .ledger_sequence(*sequence)
                    .caused_by(cause.clone()),
            ),
            None => outcome,
        };
        Some(match &self.config_refusal {
            Some(refusal) => outcome.caused_by(refusal.clone()),
            None => outcome,
        })
    }
}

/// S0 of the live build: the ledger head the operation that installed it recorded.
fn live_s0(paths: &Paths, planned: &Planned, host: &dyn Host) -> Option<u64> {
    let operation_id = planned.state.live.as_ref()?.operation_id.as_ref()?;
    let operation = Operation::load(host, paths, operation_id).ok()?;
    operation.journal().s0.as_ref()?.head
}

/// The newest start log line that reports the ledger open's phases: their sum and the event
/// count at the open.
fn newest_open(host: &dyn Host, root: &Path) -> Option<(u64, u64)> {
    let mut logs = Vec::new();
    for directory in host.log_dirs(root) {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if !(name.starts_with("actingd-") && name.ends_with(".log")) {
                continue;
            }
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .ok();
            logs.push((modified, entry.path()));
        }
    }
    logs.sort_by(|left, right| right.0.cmp(&left.0));
    logs.into_iter().find_map(|(_, path)| {
        let file = fs::File::open(&path).ok()?;
        let mut found = None;
        for line in BufReader::new(file.take(LOG_HEAD_BYTES)).lines() {
            let Ok(line) = line else {
                break;
            };
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
                .collect::<Option<Vec<_>>>();
            if let (Some(phases), Some(events)) =
                (phases, outcome.value(OPEN_EVENTS).and_then(Value::as_u64))
            {
                found = Some((phases.iter().sum(), events));
            }
        }
        found
    })
}

/// The rollback gate (`contracts/install-cli.md`, "The rollback gate"): the interface edges, the
/// target's own reader over the ledger after S0, the estimated open within the target's
/// deadline minus the margin, and the live configuration passing the target's check-config.
pub(crate) fn gate(host: &dyn Host, paths: &Paths, planned: &Planned) -> Gate {
    let mut gate = Gate {
        rows: Vec::new(),
        failed: Vec::new(),
        unreadable: None,
        config_refusal: None,
    };
    let edges_pass = planned
        .edges
        .iter()
        .filter(|edge| edge.check != "next_check_config")
        .all(|edge| edge.result != plan::EdgeResult::Fail);
    gate.part(RollbackGatePart::InterfaceEdges, edges_pass, Value::Null);
    // 2. The target's own reader reads every page after S0.
    let target_tools = paths.build(&planned.target.commit).join(Half::Tools.dir());
    let s0 = live_s0(paths, planned, host);
    let mut head = None;
    match s0 {
        None => gate.part(
            RollbackGatePart::LedgerReadability,
            false,
            json!({"s0": Value::Null}),
        ),
        Some(s0) => match programs::read_pages(host, &target_tools, &planned.state_root, s0) {
            Ok(read_head) => {
                head = Some(read_head);
                gate.part(
                    RollbackGatePart::LedgerReadability,
                    true,
                    json!({"s0": s0, "head": read_head}),
                );
            }
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
    // 3. The estimated open within the target's deadline minus the margin.
    let deadline = actingcommand_runtime_config::member(&planned.config, OPEN_TIMEOUT)
        .and_then(Value::as_u64)
        .or_else(|| {
            actingcommand_runtime_config::member(
                &plan::defaults_of(&paths.build(&planned.target.commit)),
                OPEN_TIMEOUT,
            )
            .and_then(Value::as_u64)
        });
    let margin = planned.settings.rollback_open_margin_ms;
    let estimate = newest_open(host, &paths.root).map(|(sum, events)| {
        let head = head.unwrap_or(events).max(events);
        sum.saturating_mul(head) / events.max(1)
    });
    let open_passes = matches!((deadline, estimate), (Some(deadline), Some(estimate))
        if estimate.saturating_add(margin) <= deadline);
    gate.part(
        RollbackGatePart::OpenTime,
        open_passes,
        json!({"deadline_ms": deadline, "margin_ms": margin, "estimate_ms": estimate}),
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

/// The intent of the steps that concern the whole build.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BuildIntent {
    commit: String,
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
    };
    journal::refuse_unfinished(&paths)?;
    let loaded = plan::load(&paths)?;
    let _lock = WriterLock::take(&paths, loaded.settings.lock_wait_ms)?;
    journal::refuse_unfinished(&paths)?;
    let planned = match plan::plan(host, &paths, inputs) {
        Ok(planned) => planned,
        Err(Refused::Outcome(outcome)) => return Err(outcome),
        Err(Refused::Answer(answer)) => return Ok(answer),
    };
    if planned.id != given {
        return Err(Outcome::new(ActingctlCode::InstallPlanChanged).plan_id(given));
    }
    if planned.changes().next().is_none() && planned.live.commit == planned.target.commit {
        return Answer::new(
            Outcome::new(ActingctlCode::InstallUpdateUnchanged)
                .commit_sha(planned.target.commit.clone()),
        )
        .with("commit", planned.target.commit.as_str());
    }
    if kind == PlanKind::Rollback {
        // The gate again, under the writer lock, immediately before the first change.
        let gate = gate(host, &paths, &planned);
        if let Some(refusal) = gate.refusal(&planned) {
            return planned.answer(refusal)?.with("gate", &gate.rows);
        }
    }
    // S0: the ledger head before the first change, read with the live build's reader.
    let head = programs::ledger_head(
        host,
        &paths.root.join(Half::Tools.dir()),
        &planned.state_root,
    )
    .map_err(|cause| {
        plan::refused(InstallRefusal::LedgerHeadUnreadable)
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
    let operation = Operation::open(host, &paths, opening, |ms| update_steps(&planned, ms))?;
    let answer = run(operation)?;
    let renamed = &planned.running.renamed;
    let answer = if renamed.is_empty() {
        answer
    } else {
        let warning =
            Outcome::new(ActingctlCode::InstallMcpClientsOnPreviousBuild).count(renamed.len());
        Answer {
            outcome: answer.outcome.link(CauseRelation::Note, warning),
            ..answer
        }
    };
    answer
        .with("running_renamed", renamed)?
        .with("extra", &planned.extra)
}

/// Continues an unfinished update or rollback under the writer lock.
pub(crate) fn resume(operation: Operation<'_>) -> Result<Answer, Outcome> {
    let loaded = plan::load(operation.paths()).map_err(|cause| operation.interrupted(cause))?;
    let _lock = WriterLock::take(operation.paths(), loaded.settings.lock_wait_ms)
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
    let target: String = journal.inputs["commit"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| {
            operation.interrupted(journal::state_invalid(paths.journal(operation.id()), None))
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
                    operation_host(&operation),
                    &paths.build(&target).join(Half::Runtime.dir()),
                    &config,
                ) {
                    // Nothing has changed yet: the operation ends refused.
                    let refusal = plan::refused(InstallRefusal::LedgerVerifyFailed)
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
                            plan::refused(InstallRefusal::BuildInvalid)
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
                    swap(&dst, &part, intent.old.as_deref().map(|old| paths.at(old)))
                        .map_err(|cause| operation.interrupted(cause))?;
                    if !holds(&dst, &intent.sha256) {
                        return Err(operation
                            .interrupted(plan::refused(InstallRefusal::BuildInvalid).path(&dst)));
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
                        let cause = super::adopt::verify_live(&paths).err().unwrap_or_else(|| {
                            plan::refused(InstallRefusal::LiveBuildInvalid).path(&paths.root)
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
                return Err(operation
                    .interrupted(journal::state_invalid(paths.journal(operation.id()), None)));
            }
        }
        operation
            .done(n, None)
            .map_err(|cause| operation.interrupted(cause))?;
    }
    let operation_id = operation.id().to_owned();
    let code = match journal.kind {
        OperationKind::Rollback => ActingctlCode::InstallRollbackApplied,
        _ => ActingctlCode::InstallUpdateApplied,
    };
    let outcome = Outcome::new(code)
        .install_operation_id(operation_id.clone())
        .commit_sha(target.clone());
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

/// The host of an operation (the programs it runs).
fn operation_host<'h>(operation: &Operation<'h>) -> &'h dyn Host {
    operation.host()
}

/// Swaps `part` in at `dst`: a file at `dst` that is not yet the new one is renamed `old` first
/// (a new file has no `old`), then the part renamed `dst`. Each half is done only when it is
/// missing, so a swap interrupted between the two renames is finished.
pub(crate) fn swap(dst: &Path, part: &Path, old: Option<PathBuf>) -> Result<(), Outcome> {
    if dst.exists() {
        match &old {
            Some(old) if !old.exists() => files::rename(dst, old)?,
            Some(_) => {}
            None => {
                return Err(plan::refused(InstallRefusal::BuildInvalid).path(dst));
            }
        }
    }
    if part.exists() && !dst.exists() {
        files::rename(part, dst)?;
    }
    Ok(())
}
