// SPDX-License-Identifier: AGPL-3.0-only

//! The operation journal (Workflow #421 INST-1, `contracts/install-cli.md`, "Documents"):
//! `install\operations\op-<ms>.json` (`actingcommand.install-operation.v1`) rewritten before
//! each step starts and after it ends, and `install\current.json`, written before the first step
//! and renamed `current.json.finished-<ms>` at the end. A root has at most one unfinished
//! operation; every other mutating command refuses while it is.

use super::files;
use super::host::{Host, Installer};
use super::state::{self, Current, CurrentState, InstallState, OperationKind, Paths};
use crate::codes::{ActingctlCode, InstallLocation, InstallRefusal};
use actingcommand_contract::outcome::{Detail, Outcome, OutcomeEnvelope};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

pub(crate) const OPERATION_SCHEMA: &str = "actingcommand.install-operation.v1";
/// The prefix of an operation id, `op-<unix_ms>`.
const OPERATION_PREFIX: &str = "op-";

/// The kind of a journalled step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StepKind {
    Extract,
    CopyBuild,
    VerifyTree,
    PlaceBuild,
    RecordState,
    LedgerVerify,
    StageFile,
    SwapFile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StepState {
    Planned,
    Started,
    Done,
    Undone,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Step {
    pub(crate) n: usize,
    pub(crate) kind: StepKind,
    /// The path the step changes, relative to the root with `/` separators.
    pub(crate) target: String,
    /// What the step is to do, by kind.
    pub(crate) intent: Value,
    pub(crate) state: StepState,
    pub(crate) evidence: Option<Value>,
}

/// The stop facts of an operation that stopped the Runtime (INST-2) or read the ledger head.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct S0 {
    pub(crate) head: Option<u64>,
    pub(crate) epoch: Option<String>,
    pub(crate) pauses: Option<Value>,
    pub(crate) instructions: Option<Value>,
}

/// What an undo restores: the live build and the state before the operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Previous {
    pub(crate) live: Option<String>,
    pub(crate) state: Option<InstallState>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UndoRecord {
    pub(crate) started_at_unix_ms: u64,
    pub(crate) finished_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Journal {
    pub(crate) schema_version: String,
    pub(crate) operation_id: String,
    pub(crate) kind: OperationKind,
    pub(crate) root: String,
    pub(crate) installer: Installer,
    pub(crate) inputs: Value,
    pub(crate) plan_id: Option<String>,
    pub(crate) plan: Option<Value>,
    pub(crate) guards: Vec<Value>,
    pub(crate) acknowledged: Vec<String>,
    pub(crate) s0: Option<S0>,
    pub(crate) previous: Previous,
    pub(crate) steps: Vec<Step>,
    pub(crate) undo: Option<UndoRecord>,
    pub(crate) result: Option<Value>,
    pub(crate) finished_at_unix_ms: Option<u64>,
}

impl Journal {
    /// Whether the operation ended (its result recorded).
    pub(crate) fn finished(&self) -> bool {
        self.finished_at_unix_ms.is_some()
    }
}

/// What a new operation records before its first step.
pub(crate) struct Opening {
    pub(crate) kind: OperationKind,
    pub(crate) inputs: Value,
    pub(crate) plan_id: Option<String>,
    pub(crate) plan: Option<Value>,
    pub(crate) acknowledged: Vec<String>,
    pub(crate) s0: Option<S0>,
    pub(crate) previous: Previous,
}

/// One planned step: its kind, the path it changes (relative to the root, `/` separators) and
/// its intent.
pub(crate) type PlannedStep = (StepKind, String, Value);

/// An operation in progress.
pub(crate) struct Operation<'h> {
    host: &'h dyn Host,
    paths: Paths,
    journal: Journal,
    ms: u64,
}

/// `op-<ms>`.
pub(crate) fn operation_id(ms: u64) -> String {
    format!("{OPERATION_PREFIX}{ms}")
}

/// The Unix milliseconds an operation id names; `None` for anything else.
pub(crate) fn operation_ms(operation_id: &str) -> Option<u64> {
    let digits = operation_id.strip_prefix(OPERATION_PREFIX)?;
    if digits.is_empty() || digits.len() > 20 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// The refusal `operation_unfinished` of the command `operation` for the operation
/// `current.json` names, or the root's install state refusal when it cannot be read; `Ok` when no
/// operation is unfinished.
pub(crate) fn refuse_unfinished(operation: InstallLocation, paths: &Paths) -> Result<(), Outcome> {
    match state::read_current(paths) {
        CurrentState::None => Ok(()),
        CurrentState::Unfinished(current) if operation_ms(&current.operation_id).is_some() => Err(
            super::refusal(operation, InstallRefusal::OperationUnfinished)
                .install_operation_id(current.operation_id),
        ),
        CurrentState::Unfinished(_) => Err(state_invalid(operation, paths.current(), None)),
        CurrentState::Unreadable(cause) => {
            Err(state_invalid(operation, paths.current(), Some(cause)))
        }
    }
}

/// The refusal `state_invalid` of the command `operation` for an install file that is not what
/// this build writes, with its failure when there is one.
pub(crate) fn state_invalid(
    operation: InstallLocation,
    path: PathBuf,
    cause: Option<Outcome>,
) -> Outcome {
    let refusal = super::refusal(operation, InstallRefusal::StateInvalid).path(path);
    match cause {
        Some(cause) => refusal.caused_by(cause),
        None => refusal,
    }
}

/// An outcome as the journal keeps it: its full envelope.
fn envelope(outcome: &Outcome) -> Result<Value, Outcome> {
    serde_json::to_value(outcome.envelope(Detail::Full))
        .map_err(|error| files::json_failure(&error))
}

impl<'h> Operation<'h> {
    /// Opens a new operation: refuses while another is unfinished, writes the journal with every
    /// step `steps` plans for the operation's milliseconds, then `current.json` (created only when
    /// absent).
    pub(crate) fn open(
        host: &'h dyn Host,
        paths: &Paths,
        opening: Opening,
        steps: impl FnOnce(u64) -> Vec<PlannedStep>,
    ) -> Result<Self, Outcome> {
        let command = opening.kind.location();
        refuse_unfinished(command, paths)?;
        files::create_dir_all(&paths.operations())?;
        let mut ms = host.now_unix_ms();
        while paths.journal(&operation_id(ms)).exists() {
            ms += 1;
        }
        let id = operation_id(ms);
        let steps = steps(ms)
            .into_iter()
            .enumerate()
            .map(|(index, (kind, target, intent))| Step {
                n: index + 1,
                kind,
                target,
                intent,
                state: StepState::Planned,
                evidence: None,
            })
            .collect();
        let journal = Journal {
            schema_version: OPERATION_SCHEMA.to_owned(),
            operation_id: id.clone(),
            kind: opening.kind,
            root: paths.plain.clone(),
            installer: host.installer(),
            inputs: opening.inputs,
            plan_id: opening.plan_id,
            plan: opening.plan,
            guards: Vec::new(),
            acknowledged: opening.acknowledged,
            s0: opening.s0,
            previous: opening.previous,
            steps,
            undo: None,
            result: None,
            finished_at_unix_ms: None,
        };
        let mut operation = Self {
            host,
            paths: paths.clone(),
            journal,
            ms,
        };
        operation.write()?;
        let current = Current {
            operation_id: id,
            kind: opening.kind,
            started_at_unix_ms: ms,
        };
        let bytes =
            serde_json::to_vec_pretty(&current).map_err(|error| files::json_failure(&error))?;
        if !files::create_new(&paths.current(), &bytes)? {
            // Another installer opened an operation between the check and here.
            let refusal = match refuse_unfinished(command, paths) {
                Err(refusal) => refusal,
                Ok(()) => state_invalid(command, paths.current(), None),
            };
            operation.journal.result = Some(envelope(&refusal)?);
            operation.journal.finished_at_unix_ms = Some(host.now_unix_ms());
            operation.write()?;
            return Err(refusal);
        }
        Ok(operation)
    }

    /// The journal of `operation_id`, for `resume` and `undo`; a journal that cannot be used is
    /// the refusal `state_invalid` of `command`, the command that loads it.
    pub(crate) fn load(
        host: &'h dyn Host,
        paths: &Paths,
        operation_id: &str,
        command: InstallLocation,
    ) -> Result<Self, Outcome> {
        let ms = operation_ms(operation_id)
            .ok_or_else(|| state_invalid(command, paths.journal(operation_id), None))?;
        let path = paths.journal(operation_id);
        let journal: Journal = files::read_json(&path)
            .map_err(|cause| state_invalid(command, path.clone(), Some(cause)))?;
        if journal.schema_version != OPERATION_SCHEMA || journal.operation_id != operation_id {
            return Err(state_invalid(command, path, None));
        }
        Ok(Self {
            host,
            paths: paths.clone(),
            journal,
            ms,
        })
    }

    pub(crate) fn id(&self) -> &str {
        &self.journal.operation_id
    }

    /// The operation's own Unix milliseconds: the `<ms>` of the names it keeps.
    pub(crate) fn ms(&self) -> u64 {
        self.ms
    }

    pub(crate) fn journal(&self) -> &Journal {
        &self.journal
    }

    pub(crate) fn paths(&self) -> &Paths {
        &self.paths
    }

    pub(crate) fn host(&self) -> &'h dyn Host {
        self.host
    }

    /// Step `n`'s intent as its type.
    pub(crate) fn intent<T: DeserializeOwned>(&self, n: usize) -> Result<T, Outcome> {
        let step = &self.journal.steps[n - 1];
        serde_json::from_value(step.intent.clone()).map_err(|error| {
            state_invalid(
                self.journal.kind.location(),
                self.paths.journal(self.id()),
                Some(files::json_failure(&error)),
            )
        })
    }

    /// Records step `n` started.
    pub(crate) fn start(&mut self, n: usize) -> Result<(), Outcome> {
        self.journal.steps[n - 1].state = StepState::Started;
        self.write()
    }

    /// Records step `n` done with its evidence, then lets the host stop the operation there (a
    /// crash in a test).
    pub(crate) fn done(&mut self, n: usize, evidence: Option<Value>) -> Result<(), Outcome> {
        let step = &mut self.journal.steps[n - 1];
        step.state = StepState::Done;
        step.evidence = evidence;
        self.write()?;
        self.host.after_step(&self.journal.operation_id, n)
    }

    /// Records step `n` undone (its part abandoned, its change reversed) with its evidence.
    pub(crate) fn undone(&mut self, n: usize, evidence: Option<Value>) -> Result<(), Outcome> {
        let step = &mut self.journal.steps[n - 1];
        step.state = StepState::Undone;
        step.evidence = evidence;
        self.write()
    }

    /// Starts an undo: `current.json` names the operation again when it had ended (refused
    /// while another operation is unfinished), and the journal records the undo's start. The
    /// undo's own milliseconds name what it moves aside.
    pub(crate) fn begin_undo(&mut self) -> Result<u64, Outcome> {
        if let Some(undo) = &self.journal.undo {
            return Ok(undo.started_at_unix_ms);
        }
        if self.journal.finished() {
            refuse_unfinished(InstallLocation::Undo, &self.paths)?;
            let current = Current {
                operation_id: self.journal.operation_id.clone(),
                kind: self.journal.kind,
                started_at_unix_ms: self.ms,
            };
            let bytes =
                serde_json::to_vec_pretty(&current).map_err(|error| files::json_failure(&error))?;
            if !files::create_new(&self.paths.current(), &bytes)? {
                refuse_unfinished(InstallLocation::Undo, &self.paths)?;
                return Err(state_invalid(
                    InstallLocation::Undo,
                    self.paths.current(),
                    None,
                ));
            }
        }
        let now = self.host.now_unix_ms();
        self.journal.undo = Some(UndoRecord {
            started_at_unix_ms: now,
            finished_at_unix_ms: None,
        });
        self.write()?;
        Ok(now)
    }

    /// Ends an undo: the journal records it (and `result` when the operation had none), and
    /// `current.json` is renamed `current.json.finished-<ms>`. A failure on the way is
    /// `install_operation_interrupted`: `current.json` stays, and `resume` finishes the rename.
    pub(crate) fn finish_undo(mut self, result: &Outcome) -> Result<(), Outcome> {
        let now = self.host.now_unix_ms();
        if let Some(undo) = self.journal.undo.as_mut() {
            undo.finished_at_unix_ms = Some(now);
        }
        if self.journal.result.is_none() {
            self.journal.result = Some(envelope(result).map_err(|cause| self.interrupted(cause))?);
        }
        self.journal.finished_at_unix_ms.get_or_insert(now);
        self.end(now)
    }

    /// Records the operation's result and ends it: `current.json` renamed
    /// `current.json.finished-<ms>`. A failure on the way is `install_operation_interrupted`:
    /// `current.json` stays, and `resume` finishes the rename (review S1 F5).
    pub(crate) fn finish(mut self, result: &Outcome) -> Result<(), Outcome> {
        let now = self.host.now_unix_ms();
        self.journal.result = Some(envelope(result).map_err(|cause| self.interrupted(cause))?);
        self.journal.finished_at_unix_ms = Some(now);
        self.end(now)
    }

    /// The end recorded in the journal, then `current.json` retired; a failure is
    /// `install_operation_interrupted`.
    fn end(mut self, now: u64) -> Result<(), Outcome> {
        self.write().map_err(|cause| self.interrupted(cause))?;
        self.host
            .before_retire(&self.journal.operation_id)
            .map_err(|cause| self.interrupted(cause))?;
        retire_current(&self.paths, now).map_err(|cause| self.interrupted(cause))
    }

    /// Whether the operation has ended in its journal (its own end, or its undo's) while
    /// `current.json` may still name it: what a crash between the journal's last write and the
    /// rename of `current.json` leaves (review S1 F5).
    pub(crate) fn ended(&self) -> bool {
        match &self.journal.undo {
            Some(undo) => undo.finished_at_unix_ms.is_some(),
            None => self.journal.finished(),
        }
    }

    /// `resume` of an operation that has [`ended`](Self::ended): `current.json` renamed
    /// `current.json.finished-<ms>`, and the outcome the operation ended with (its journalled
    /// result; `install_command_done` of `install undo` for an undo). A failure is
    /// `install_operation_interrupted`.
    pub(crate) fn retire_ended(self) -> Result<Outcome, Outcome> {
        let ended = match &self.journal.undo {
            Some(_) => super::done(InstallLocation::Undo)
                .install_operation_id(self.id())
                .installation_changed(true),
            None => {
                let result = self.journal.result.clone().ok_or_else(|| {
                    state_invalid(InstallLocation::Resume, self.paths.journal(self.id()), None)
                })?;
                serde_json::from_value::<OutcomeEnvelope>(result)
                    .map_err(|error| {
                        state_invalid(
                            InstallLocation::Resume,
                            self.paths.journal(self.id()),
                            Some(files::json_failure(&error)),
                        )
                    })?
                    .into_outcome()
            }
        };
        let now = self.host.now_unix_ms();
        retire_current(&self.paths, now).map_err(|cause| self.interrupted(cause))?;
        Ok(ended)
    }

    /// `install_operation_interrupted`: a step failed after the operation changed the disk; the
    /// journal and `current.json` stay for `resume` and `undo`. `operation` is the journalled
    /// command.
    pub(crate) fn interrupted(&self, cause: Outcome) -> Outcome {
        Outcome::new(ActingctlCode::InstallOperationInterrupted)
            .operation(self.journal.kind.location())
            .install_operation_id(self.id())
            .caused_by(cause)
    }

    fn write(&mut self) -> Result<(), Outcome> {
        files::write_json_atomic(
            &self.paths.journal(&self.journal.operation_id),
            &self.journal,
        )
    }
}

/// `current.json` renamed `current.json.finished-<ms>`, `<ms>` the first free one from `now`.
fn retire_current(paths: &Paths, now: u64) -> Result<(), Outcome> {
    let current = paths.current();
    if !current.exists() {
        return Ok(());
    }
    let mut ms = now;
    loop {
        let finished = files::with_suffix(&current, &format!(".finished-{ms}"));
        if !finished.exists() {
            return files::rename(&current, &finished);
        }
        ms += 1;
    }
}

/// The ids of every journalled operation of the root, oldest first.
pub(crate) fn operation_ids(paths: &Paths) -> Result<Vec<String>, Outcome> {
    let directory = paths.operations();
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(files::os_failure(
                &directory,
                &error,
                actingcommand_contract::outcome::IoOp::ReadDir,
            ));
        }
    };
    let mut operations = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            files::os_failure(
                &directory,
                &error,
                actingcommand_contract::outcome::IoOp::ReadDir,
            )
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(operation_id) = name.strip_suffix(".json")
            && let Some(ms) = operation_ms(operation_id)
        {
            operations.push((ms, operation_id.to_owned()));
        }
    }
    operations.sort();
    Ok(operations.into_iter().map(|(_, id)| id).collect())
}
