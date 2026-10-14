// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #336 L6 (§12.8, `contracts/policy-suspension.md`): the read-only report behind
//! `actingd suspended`: the scheduled (task, instance) pairs a failure paused, those a package
//! update lifted, and those that keep failing without accumulating (`repeating`).
//!
//! The ledger is opened read-only without referenced material and without the owner lock, so
//! a running daemon is not disturbed. Each pair is judged with the same lift rule as live
//! admission (§12.7), against the configuration the caller loaded. Error frames are read
//! read-only and compared again for display; the recorded failure identity stays the verdict.
//! Workflow #336 PKG-S3: the report reads the packages a paused pair names, read-only, to
//! measure their content hashes now (each package once per report); it still writes nothing.
//!
//! Workflow #372 K3 adds `clock_slots`: the clock-slot records of K2 (`goal_missed` planning
//! signals whose `signal_id` starts with `signal:clock_slot:`) of the 48 h before the ledger's
//! latest event, each with the fate its pair's next admitted dispatch settles.

use crate::RuntimeHostConfig;
use crate::codes::{HostCode, HostLocation};
use crate::failure_identity::{
    FailureIdentity, failure_rows, failure_step, identifier_text, locate_error_frame,
};
use crate::suspension_lift::{LiftCheckTime, LiftChecks, PausedPackages, SuspensionLiftView};
use actingcommand_artifact_store::read_projected_verified;
use actingcommand_contract::codes::{ContractLocation, SharedCode};
use actingcommand_contract::outcome::{CauseRelation, Code, CodeStr, Location, Outcome};
use actingcommand_contract::{
    CONFIG_SUBSYSTEMS_FACT_KEY, EventPayload, EventQuery, EventSeverity, EventType,
    PolicyBudgetDimension, PolicyExecutionOutcome, PolicyFailureDisposition, PolicyFailureRecord,
    PolicyPayload, PolicyPlanningSignalKind, RunId, RuntimePayload, TaskPayload, TaskSemanticFact,
    scheduled_failure_code_str,
};
use actingcommand_execution_kernel::{compare_failure_frames, failure_frame_ccoeff};
use actingcommand_ledger::{
    GlobalLedger, GlobalLedgerEvidence, GlobalLedgerEvidenceConfig, LedgerArtifactReference,
    PersistedEvent,
};
use actingcommand_policy::{CatalogSources, compile_catalog};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

const SCHEMA_VERSION: &str = "actingcommand.actingd.suspended.v2";
/// Workflow #336 PKG-S3: a pause lifts once the content hash of a package it names changes.
const LIFTS_WHEN: &str = "content_hash_changes";
/// `clock_slots` reads the records of this span before the ledger's latest event (#372 K3).
const CLOCK_SLOT_WINDOW_MS: u64 = 48 * 60 * 60 * 1000;
/// `clock_slots` keeps at most this many entries, newest first; `clock_slots_total` then counts
/// them all.
const CLOCK_SLOT_LIMIT: usize = 200;
/// The `signal_id` prefix of a clock-slot record (#372 K2).
const CLOCK_SLOT_SIGNAL_PREFIX: &str = "signal:clock_slot:";

/// What `actingd suspended` reads: the loaded configuration and when its file last changed.
pub struct SuspensionReportRequest<'a> {
    pub config_path: &'a Path,
    /// The configuration file's modification time, when the file system reports one.
    pub config_modified_unix_ms: Option<u64>,
    pub host: &'a RuntimeHostConfig,
    pub catalog: &'a CatalogSources,
}

/// A report that could not be produced (Workflow #378 A6a): its outcome, the code with its
/// operation, values and links.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuspensionReportError {
    code: Code,
    outcome: Outcome,
}

impl SuspensionReportError {
    fn new(code: impl Into<Code>, operation: impl Into<Location>) -> Self {
        let code = code.into();
        Self {
            code,
            outcome: Outcome::new(code).operation(operation),
        }
    }

    /// The failure's code, the top of its outcome.
    pub const fn code(&self) -> Code {
        self.code
    }

    /// The failure with what its outcome says beyond its code and operation.
    fn with_outcome(mut self, values: impl FnOnce(Outcome) -> Outcome) -> Self {
        self.outcome = values(self.outcome);
        self
    }

    /// The failure's outcome, which actingd's suspended report carries.
    pub const fn outcome(&self) -> &Outcome {
        &self.outcome
    }
}

impl fmt::Display for SuspensionReportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.outcome)
    }
}

impl std::error::Error for SuspensionReportError {}

/// One recorded `policy.execution_recorded` of a pair.
struct Execution {
    sequence: u64,
    severity: EventSeverity,
    decision_id: String,
    observed_at_unix_ms: u64,
    outcome: PolicyExecutionOutcome,
}

/// The read-only ledger snapshot and the artifact root its frames are read from.
struct Snapshot<'a> {
    ledger: GlobalLedgerEvidence,
    artifact_root: &'a Path,
}

impl Snapshot<'_> {
    fn query(&self, query: EventQuery) -> Result<Vec<PersistedEvent>, SuspensionReportError> {
        let mut events = self.ledger.query(&query);
        events.sort_by_key(PersistedEvent::sequence);
        Ok(events)
    }

    /// The run's `task.failed` terminal (its failure's spelling and the record's severity,
    /// which is that failure's category) and the native detail of its lifecycle failure record
    /// of that spelling (or of `fallback`, the recorded failure's code, when the run has no
    /// terminal). A failure is spelled as the policy spells it (coordinator ruling 7): its code,
    /// or for `outcome_relay_unregistered` the original spelling it keeps as `raw_code`, as the
    /// report showed it before ledger revision 3. Without a terminal, the run's failure severity
    /// is its `runtime.failed` record's: the lifecycle failure of that spelling, else the run's
    /// latest one (coordinator ruling, 2026-10-13); `None` when the run has neither.
    fn run_failure(
        &self,
        run_id: Option<RunId>,
        fallback: &CodeStr,
    ) -> (
        Option<(CodeStr, EventSeverity)>,
        Option<EventSeverity>,
        Value,
    ) {
        let Some(run_id) = run_id else {
            return (None, None, Value::Null);
        };
        let by_run = |event_type| EventQuery {
            event_type: Some(event_type),
            run_id: Some(run_id),
            ..EventQuery::default()
        };
        let terminal = self
            .ledger
            .query(&by_run(EventType::TaskFailed))
            .iter()
            .find_map(|event| match event.payload() {
                EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
                    TaskSemanticFact::TerminalCommitted {
                        outcome: Some(outcome),
                        ..
                    } => Some((scheduled_failure_code_str(outcome), event.severity())),
                    _ => None,
                },
                _ => None,
            });
        let code = terminal.as_ref().map_or(fallback, |(spelling, _)| spelling);
        let runtime_failures = self.ledger.query(&by_run(EventType::RuntimeFailed));
        // Ledger revision 3: the lifecycle failure's code is the record's outcome and its
        // native detail the `raw_text` of that outcome's direct foreign link.
        let lifecycle_failures = runtime_failures
            .iter()
            .rev()
            .filter_map(|event| match event.payload() {
                EventPayload::Runtime(RuntimePayload::Failed(payload))
                    if payload.lifecycle_failure().is_some()
                        && scheduled_failure_code_str(payload.outcome()) == *code =>
                {
                    Some((event, payload))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let detail = lifecycle_failures.iter().find_map(|(_, payload)| {
            payload.outcome().causes().iter().find_map(|link| {
                link.outcome()
                    .value("raw_text")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
        });
        let severity = match &terminal {
            Some((_, severity)) => Some(*severity),
            None => lifecycle_failures
                .first()
                .map(|(event, _)| event.severity())
                .or_else(|| runtime_failures.last().map(|event| event.severity())),
        };
        (
            terminal,
            severity,
            detail.map_or(Value::Null, Value::String),
        )
    }

    /// §12.4 scope and step re-derived from the run's rows, for display.
    fn step(&self, run_id: Option<RunId>, terminal: bool) -> Result<Value, SuspensionReportError> {
        let Some(run_id) = run_id.filter(|_| terminal) else {
            return Ok(json!({ "scope": "prepare", "operation_label": "prepare" }));
        };
        let rows = failure_rows(run_id, |query| self.query(query))?;
        let chain = rows
            .iter()
            .any(|event| event.event_type() == EventType::TaskEntryPreflight);
        let (scope, operation) = failure_step(&rows, chain);
        Ok(json!({ "scope": scope, "operation_label": operation }))
    }

    fn frame(
        &self,
        run_id: Option<RunId>,
    ) -> Result<Option<LedgerArtifactReference>, SuspensionReportError> {
        match run_id {
            Some(run_id) => locate_error_frame(run_id, |query| self.query(query)),
            None => Ok(None),
        }
    }

    /// The two error frames and their recomputed comparison; the verdict is returned too.
    /// Workflow #378 A6a: why frames differ, or why they could not be compared, is the
    /// comparison's `outcome` (a code with its links), and a failed `ccoeff` its
    /// `ccoeff_failure`; v1 named them as text (`reason`, `ccoeff_error`).
    fn frames(
        &self,
        previous_run: Option<RunId>,
        current_run: Option<RunId>,
    ) -> Result<(Value, Option<&'static str>), SuspensionReportError> {
        let previous = self.frame(previous_run)?;
        let current = self.frame(current_run)?;
        let describe = |artifact: &Option<LedgerArtifactReference>| {
            artifact.as_ref().map_or(Value::Null, |artifact| {
                json!({
                    "frame_id": artifact.frame_id().map(identifier_text),
                    "artifact_sha256": artifact.sha256(),
                })
            })
        };
        let unavailable = |why: Outcome| {
            json!({
                "source": "recomputed", "status": "unavailable", "outcome": why,
                "changed_cells_milli": null, "digest_mean_milli": null,
                "ccoeff": null, "ccoeff_failure": null,
            })
        };
        let (comparison, verdict) = match (&previous, &current) {
            (Some(previous), Some(current)) => {
                let read = |artifact: &LedgerArtifactReference| {
                    read_projected_verified(self.artifact_root, &artifact.project(true))
                };
                match (read(previous), read(current)) {
                    (Ok(previous), Ok(current)) => {
                        match compare_failure_frames(&previous, &current) {
                            Ok(compared) => {
                                let ccoeff = failure_frame_ccoeff(&previous, &current);
                                (
                                    json!({
                                        "source": "recomputed",
                                        "status": compared.verdict.as_str(),
                                        "outcome": compared.reason.map(Outcome::new),
                                        "changed_cells_milli": compared.changed_cells_milli,
                                        "digest_mean_milli": compared.digest_mean_milli,
                                        "ccoeff": ccoeff.as_ref().ok(),
                                        "ccoeff_failure": ccoeff.as_ref().err().map(compare_failure),
                                    }),
                                    Some(compared.verdict.as_str()),
                                )
                            }
                            Err(error) => (unavailable(compare_failure(&error)), None),
                        }
                    }
                    (Err(error), _) | (_, Err(error)) => (
                        unavailable(Outcome::new(HostCode::SuspendedFrameUnreadable).joined(
                            CauseRelation::CausedBy,
                            crate::RuntimeHostError::artifact_read(error).outcome(),
                        )),
                        None,
                    ),
                }
            }
            _ => (
                unavailable(Outcome::new(HostCode::SuspendedFrameMissing)),
                None,
            ),
        };
        Ok((
            json!({
                "previous": describe(&previous),
                "current": describe(&current),
                "comparison": comparison,
            }),
            verdict,
        ))
    }
}

/// Workflow #378 A6a: frames that could not be compared, as an outcome: the kernel's code with
/// the decoder's text as its `foreign_data_error` link.
fn compare_failure(error: &actingcommand_execution_kernel::FailureFrameCompareError) -> Outcome {
    Outcome::new(error.reason()).caused_by(
        Outcome::new(actingcommand_contract::codes::ContractCode::ForeignDataError)
            .raw_source(actingcommand_contract::outcome::RawSource::Other)
            .raw_text(error.message()),
    )
}

/// `actingd suspended`: the suspended, lifted and repeating pairs of the configured state
/// root, judged against the configuration in `request`.
pub fn suspension_report(
    request: &SuspensionReportRequest<'_>,
) -> Result<Value, SuspensionReportError> {
    let manifest = request.host.procedure_manifest().ok_or_else(|| {
        SuspensionReportError::new(
            SharedCode::SuspendedPolicyUnconfigured,
            ContractLocation::ReadSuspensionConfig,
        )
    })?;
    let compiled = compile_catalog(request.catalog).map_err(|_| {
        SuspensionReportError::new(
            SharedCode::PolicyCatalogCompileFailed,
            ContractLocation::ReadSuspensionConfig,
        )
    })?;
    let procedure_refs = compiled
        .catalog()
        .tasks
        .tasks
        .iter()
        .map(|task| (task.id.clone(), task.procedure_ref.clone()))
        .collect::<BTreeMap<_, _>>();
    let state_root = request.host.state_root();
    if !state_root.is_dir() {
        return Err(SuspensionReportError::new(
            HostCode::SuspendedLedgerUnavailable,
            HostLocation::OpenSuspensionLedger,
        )
        .with_outcome(|outcome| outcome.state_root(state_root)));
    }
    let ledger = GlobalLedger::open_evidence(
        GlobalLedgerEvidenceConfig::new(state_root).sqlite_material_not_read(),
        |_| None,
    )
    .map_err(|error| {
        SuspensionReportError::new(
            HostCode::SuspendedLedgerUnavailable,
            HostLocation::OpenSuspensionLedger,
        )
        .with_outcome(|outcome| {
            outcome
                .state_root(state_root)
                .joined(CauseRelation::CausedBy, Outcome::relayed(error.code()))
        })
    })?;
    if !ledger.is_complete() {
        return Err(SuspensionReportError::new(
            HostCode::SuspendedLedgerIncomplete,
            HostLocation::OpenSuspensionLedger,
        ));
    }
    let snapshot = Snapshot {
        ledger,
        artifact_root: state_root,
    };
    // Workflow #336 PKG-S3: each package is measured once for the whole report.
    let checks = LiftChecks::default();
    let view = SuspensionLiftView {
        manifest,
        prerequisite_packages: request.host.prerequisite_packages(),
        return_home_packages: request.host.return_home_packages(),
        checks: &checks,
        time: LiftCheckTime::ONE_READ,
    };
    let through_sequence = snapshot.ledger.latest_sequence();
    let intents = snapshot
        .query(EventQuery {
            event_type: Some(EventType::PolicyDispatchIntent),
            ..EventQuery::default()
        })?
        .into_iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Policy(PolicyPayload::DispatchIntent(payload)) => Some((
                payload.decision_id().to_owned(),
                event.links().run_id().copied(),
            )),
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    let mut pairs = BTreeMap::<(String, String), Vec<Execution>>::new();
    for event in snapshot.query(EventQuery {
        event_type: Some(EventType::PolicyExecutionRecorded),
        ..EventQuery::default()
    })? {
        if let EventPayload::Policy(PolicyPayload::ExecutionRecorded(payload)) = event.payload() {
            pairs
                .entry((
                    payload.task_id().to_owned(),
                    payload.instance_id().to_owned(),
                ))
                .or_default()
                .push(Execution {
                    sequence: event.sequence(),
                    severity: event.severity(),
                    decision_id: payload.decision_id().to_owned(),
                    observed_at_unix_ms: payload.observed_at_unix_ms(),
                    outcome: payload.policy_result().clone(),
                });
        }
    }
    let daemon_started_at_unix_ms = snapshot
        .query(EventQuery {
            event_type: Some(EventType::RuntimeFactRecorded),
            ..EventQuery::default()
        })?
        .iter()
        .rev()
        .find_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::FactRecorded(payload))
                if payload.record().key == CONFIG_SUBSYSTEMS_FACT_KEY =>
            {
                Some(payload.record().observed_at_unix_ms)
            }
            _ => None,
        });
    let mut warnings = Vec::new();
    match daemon_started_at_unix_ms {
        None => warnings.push("daemon_start_unknown".to_owned()),
        Some(started) => {
            if request
                .config_modified_unix_ms
                .is_some_and(|modified| modified > started)
            {
                warnings.push("config_newer_than_daemon_start".to_owned());
            }
        }
    }
    let (mut suspended, mut lifted, mut repeating) = (Vec::new(), Vec::new(), Vec::new());
    for ((task_id, instance_id), executions) in &pairs {
        let Some(latest) = executions.last() else {
            continue;
        };
        let PolicyExecutionOutcome::Failed { failure } = &latest.outcome else {
            continue;
        };
        let pair = format!("{task_id}/{instance_id}");
        // Workflow #378 A2-P: the record keeps the failure's `outcome` and its identity apart;
        // the report shows them as before, the identity, else the failure's code.
        let failure_code = scheduled_failure_code_str(&failure.outcome);
        let recorded = failure
            .failure_identity
            .clone()
            .unwrap_or_else(|| failure_code.as_str().to_owned());
        let identity = failure
            .failure_identity
            .as_deref()
            .and_then(FailureIdentity::parse);
        if identity.is_none() && recorded.contains("~v2~") {
            warnings.push(format!("error_code_unparsed:{pair}"));
        }
        let previous = executions
            .len()
            .checked_sub(2)
            .and_then(|index| executions.get(index));
        let current_run = intents.get(&latest.decision_id).copied().flatten();
        let base = identity
            .as_ref()
            .map_or_else(|| recorded.clone(), |identity| identity.base.clone());
        let (terminal, run_severity, detail) = snapshot.run_failure(current_run, &failure_code);
        let failure_spelling = terminal
            .as_ref()
            .map_or(base.as_str(), |(spelling, _)| spelling.as_str());
        if failure.disposition == PolicyFailureDisposition::PausedTask {
            let procedure_ref = procedure_refs.get(task_id);
            if procedure_ref.is_none() {
                warnings.push(format!("task_not_in_catalog:{pair}"));
            }
            // Workflow #336 PKG-S3: what the pause recorded, from its failure identity alone.
            let paused_packages = PausedPackages::of_identity(failure.failure_identity.as_deref());
            let package =
                package_json(&view, procedure_ref, &paused_packages).map_err(|_| lift_failed())?;
            let paused = json!({
                "execution_sequence": latest.sequence,
                "observed_at_unix_ms": latest.observed_at_unix_ms,
                "decision_id": latest.decision_id,
                "run_id": current_run.as_ref().map(identifier_text),
            });
            // Workflow #361 M6: the first interrupted settlement in a row is lifted by the
            // restart that recorded it, whatever the configuration (review P5).
            let lift = if crate::policy_control::interruption_lifted_by_restart(
                &failure_code,
                failure.consecutive_same_error,
            ) {
                Some(("restart".to_owned(), "active"))
            } else if let Some(procedure_ref) = procedure_ref {
                let verdict = view
                    .lifted(procedure_ref, &latest.decision_id, &paused_packages)
                    .map_err(|_| lift_failed())?;
                if !verdict.unmeasured.is_empty() {
                    warnings.push(format!("lift_check_hash_unmeasured:{pair}"));
                }
                // A package changed in place lifts the pause at the running daemon's next check
                // of the pair, without a restart.
                verdict.lifted.map(|lift| (lift.label(), "next_evaluation"))
            } else {
                None
            };
            if let Some((lifted_by, effective)) = lift {
                lifted.push(json!({
                    "task_id": task_id,
                    "instance_id": instance_id,
                    "lifted_by": lifted_by,
                    "effective": effective,
                    "paused": paused,
                    "package": package,
                }));
                continue;
            }
            // The failed execution this suspension repeats, when it counted more than one.
            let previous = previous.filter(|previous| {
                failure.consecutive_same_error > 1
                    && matches!(previous.outcome, PolicyExecutionOutcome::Failed { .. })
            });
            let previous_run =
                previous.and_then(|previous| intents.get(&previous.decision_id).copied().flatten());
            let (frames, verdict) = snapshot.frames(previous_run, current_run)?;
            if verdict == Some("different")
                && identity.as_ref().is_some_and(FailureIdentity::accumulates)
            {
                warnings.push(format!("comparison_disagrees_with_record:{pair}"));
            }
            let step = match &identity {
                Some(_) => snapshot.step(current_run, terminal.is_some())?,
                None => Value::Null,
            };
            let previous_json = previous.map(|previous| {
                json!({
                    "execution_sequence": previous.sequence,
                    "disposition": execution_disposition(&previous.outcome),
                    "run_id": previous_run.as_ref().map(identifier_text),
                })
            });
            // Coordinator ruling 6: the severity of the run's failure terminal, which is its
            // failure's category; without a terminal, the run's `runtime.failed` record's. The
            // execution record's own severity, its failure outcome's category (Workflow #378
            // A2-P), stands in only when the run has neither.
            let severity = run_severity.unwrap_or(latest.severity);
            suspended.push(json!({
                "task_id": task_id,
                "instance_id": instance_id,
                "procedure_ref": procedure_ref,
                "failure": failure.outcome,
                "failure_identity": failure.failure_identity,
                "run_failure": Outcome::relayed(failure_spelling),
                "consecutive_same_error": failure.consecutive_same_error,
                "effective_class": wire(&failure.effective_class),
                "severity": wire(&severity),
                "paused": paused,
                "previous": previous_json,
                "step": step,
                "detail": detail,
                "package": package,
                "frames": frames,
                "lifts_when": LIFTS_WHEN,
                "takeover": format!(
                    "actingctl task-run --state-root {} --instance {instance_id} --package <package> --package-ref <package-ref>",
                    state_root.display()
                ),
            }));
            continue;
        }
        // Repeating: a failure that does not accumulate, after a failure with its prefix.
        let Some(identity) = identity.filter(|identity| !identity.accumulates()) else {
            continue;
        };
        let Some((previous, previous_failure)) =
            previous.and_then(|previous| match &previous.outcome {
                PolicyExecutionOutcome::Failed { failure } => Some((previous, failure)),
                PolicyExecutionOutcome::Succeeded { .. } => None,
            })
        else {
            continue;
        };
        if previous_failure
            .failure_identity
            .as_deref()
            .and_then(FailureIdentity::parse)
            .is_none_or(|previous| previous.prefix() != identity.prefix())
        {
            continue;
        }
        let previous_run = intents.get(&previous.decision_id).copied().flatten();
        repeating.push(json!({
            "task_id": task_id,
            "instance_id": instance_id,
            "failure": failure.outcome,
            "failure_identity": failure.failure_identity,
            "run_failure": Outcome::relayed(failure_spelling),
            "step": snapshot.step(current_run, terminal.is_some())?,
            "latest": {
                "execution_sequence": latest.sequence,
                "run_id": current_run.as_ref().map(identifier_text),
            },
            "previous": {
                "execution_sequence": previous.sequence,
                "run_id": previous_run.as_ref().map(identifier_text),
            },
            "detail": detail,
        }));
    }
    let (clock_slots, clock_slots_total) = clock_slots(&snapshot)?;
    let mut report = json!({
        "schema_version": SCHEMA_VERSION,
        "config_path": request.config_path.to_string_lossy(),
        "state_root": state_root.to_string_lossy(),
        "through_sequence": through_sequence,
        "daemon_started_at_unix_ms": daemon_started_at_unix_ms,
        "suspended": suspended,
        "lifted": lifted,
        "repeating": repeating,
        "warnings": warnings,
        "clock_slots": clock_slots,
    });
    if let Some(total) = clock_slots_total {
        report["clock_slots_total"] = json!(total);
    }
    Ok(report)
}

/// A lift check that could not read its own memory (a poisoned lock).
fn lift_failed() -> SuspensionReportError {
    SuspensionReportError::new(
        HostCode::RuntimeStatePoisoned,
        HostLocation::CheckSuspensionLift,
    )
}

/// The content hashes a pair was paused on and has now (Workflow #336 PKG-S3): the main
/// package's and each recorded layer's prefix (`null` when not recorded or not measurable now).
fn package_json(
    view: &SuspensionLiftView<'_>,
    procedure_ref: Option<&String>,
    paused: &PausedPackages,
) -> crate::RuntimeHostResult<Value> {
    let current_main = match procedure_ref {
        Some(procedure_ref) => view.current_main_hash(procedure_ref)?,
        None => None,
    };
    let mut layers = Vec::with_capacity(paused.layers.len());
    for layer in &paused.layers {
        let current = view.current_layer_hash(layer)?;
        layers.push(json!({
            "kind": layer.kind(),
            "package_id": view.layer_package_id(layer),
            "paused_hash_prefix": layer.hash(),
            "current_hash_prefix": current,
            "changed": current.as_deref().is_some_and(|current| current != layer.hash()),
        }));
    }
    Ok(json!({
        "paused_hash_prefix": paused.main.as_ref().map(|main| main.hash.as_str()),
        "current_hash_prefix": current_main,
        "layers": layers,
    }))
}

fn execution_disposition(outcome: &PolicyExecutionOutcome) -> Value {
    match outcome {
        PolicyExecutionOutcome::Failed {
            failure: PolicyFailureRecord { disposition, .. },
        } => wire(disposition),
        PolicyExecutionOutcome::Succeeded { .. } => Value::Null,
    }
}

/// A value's wire form, as its records serialize it.
fn wire(value: &impl serde::Serialize) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// One admitted dispatch of a pair: the run that settles a clock-slot record (#372 K3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct SlotDispatch {
    pub(crate) sequence: u64,
    pub(crate) admitted_at_unix_ms: u64,
}

/// One clock-slot record as the ledger holds it (#372 K2).
pub(crate) struct SlotSignal<'a> {
    pub(crate) sequence: u64,
    pub(crate) recorded_at_unix_ms: u64,
    pub(crate) signal_id: &'a str,
    pub(crate) task_id: Option<&'a str>,
    pub(crate) instance_id: &'a str,
    /// The record's outcome: its code with its values (Workflow #378 A2-P).
    pub(crate) outcome: &'a Outcome,
}

/// A clock-slot record's values, deserialized from its typed map.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ClockSlotRecord {
    code: CodeStr,
    task_id: String,
    instance_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    gate_code: Option<CodeStr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    budget_dimension: Option<PolicyBudgetDimension>,
    occurrence_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    window_start_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    window_end_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    next_eligible_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    next_occurrence_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    first_run: Option<bool>,
}

/// What became of a clock slot (model-372 §3.2, §3.3): a refused or deferred record's
/// occurrence is served in the same window, served in the next one, collapsed into the next
/// occurrence, or not known yet; an owed record's selection was served or absorbed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum SlotFate {
    SameWindow,
    NextWindow,
    Collapsed,
    Pending,
    Served,
    Absorbed,
}

#[derive(Serialize)]
struct ClockSlotEntry {
    #[serde(flatten)]
    record: ClockSlotRecord,
    slot_fate: SlotFate,
    slot_fate_settled: bool,
    recorded_sequence: u64,
    recorded_at_unix_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_dispatch: Option<SlotDispatch>,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum ParseState {
    Unparsed,
}

/// A record whose outcome does not read back; it adds no report-level warning.
#[derive(Serialize)]
struct UnparsedClockSlot<'a> {
    parse_state: ParseState,
    signal_id: &'a str,
    code: &'a CodeStr,
}

impl ClockSlotRecord {
    /// K2's outcome read back (model-372 §4.5; Workflow #378 A2-P): the code and the outcome's
    /// values except its task and alias go into a map, the payload's task and instance join
    /// them, and the record is deserialized from it. `None` when the code is not a clock-slot
    /// code, when a value is not one the record holds or has the wrong type, or when a value its
    /// code requires is absent.
    fn parse(signal: &SlotSignal<'_>) -> Option<Self> {
        let mut map = Map::new();
        map.insert(
            "code".to_owned(),
            Value::String(signal.outcome.code().as_str().to_owned()),
        );
        for (key, value) in signal.outcome.values() {
            if key != "task_id" && key != "alias" {
                map.insert(key.clone(), value.clone());
            }
        }
        for (key, value) in [
            ("task_id", signal.task_id?),
            ("instance_id", signal.instance_id),
        ] {
            if map
                .insert(key.to_owned(), Value::String(value.to_owned()))
                .is_some()
            {
                return None;
            }
        }
        let record = serde_json::from_value::<Self>(Value::Object(map)).ok()?;
        let complete = if record.code == HostCode::ClockSlotOwed.code() {
            record.window_start_unix_ms.is_some()
        } else {
            (record.code == HostCode::ClockSlotRefused.code()
                || record.code == HostCode::ClockSlotDeferred.code())
                && record.gate_code.is_some()
        };
        complete.then_some(record)
    }

    /// The fate, and whether it is settled: by the pair's next admitted dispatch, or, without
    /// one, by the reference time reaching the next occurrence. An unsettled refused or deferred
    /// record shows the fate its own `next_eligible_unix_ms` predicts.
    fn fate(
        &self,
        next_dispatch: Option<SlotDispatch>,
        reference_unix_ms: u64,
    ) -> (SlotFate, bool) {
        let reaches_next = |time: u64| {
            self.next_occurrence_unix_ms
                .is_some_and(|next_occurrence| time >= next_occurrence)
        };
        if self.code == HostCode::ClockSlotOwed.code() {
            return match next_dispatch {
                Some(dispatch) if reaches_next(dispatch.admitted_at_unix_ms) => {
                    (SlotFate::Absorbed, true)
                }
                Some(_) => (SlotFate::Served, true),
                None if reaches_next(reference_unix_ms) => (SlotFate::Absorbed, true),
                None => (SlotFate::Pending, false),
            };
        }
        // A time at or after the next occurrence collapses into it; before it, the end of the
        // activity window the record was written in separates the same window from the next.
        let classify = |time: u64| {
            if reaches_next(time) {
                SlotFate::Collapsed
            } else if self.window_end_unix_ms.is_some_and(|end| time < end) {
                SlotFate::SameWindow
            } else {
                SlotFate::NextWindow
            }
        };
        match next_dispatch {
            Some(dispatch) => (classify(dispatch.admitted_at_unix_ms), true),
            None if reaches_next(reference_unix_ms) => (SlotFate::Collapsed, true),
            None => (
                self.next_eligible_unix_ms
                    .map_or(SlotFate::Pending, classify),
                false,
            ),
        }
    }
}

/// One `clock_slots` entry: the record with its fate, `next_dispatch` when the pair was
/// admitted after it, or an unparsed entry that carries the outcome's code.
pub(crate) fn clock_slot_entry(
    signal: &SlotSignal<'_>,
    next_dispatch: Option<SlotDispatch>,
    reference_unix_ms: u64,
) -> Value {
    match ClockSlotRecord::parse(signal) {
        Some(record) => {
            let (slot_fate, slot_fate_settled) = record.fate(next_dispatch, reference_unix_ms);
            wire(&ClockSlotEntry {
                record,
                slot_fate,
                slot_fate_settled,
                recorded_sequence: signal.sequence,
                recorded_at_unix_ms: signal.recorded_at_unix_ms,
                next_dispatch,
            })
        }
        None => wire(&UnparsedClockSlot {
            parse_state: ParseState::Unparsed,
            signal_id: signal.signal_id,
            code: signal.outcome.code(),
        }),
    }
}

/// `clock_slots` (model-372 §4.5): the clock-slot records of the 48 h before the ledger's
/// latest event, newest first and at most 200, and how many there are when the list is cut.
/// A record's next dispatch is its pair's first admitted dispatch after it in the ledger; the
/// reference time is the latest event's time. Records are selected by their kind and their
/// `signal_id` prefix, never by their outcome.
fn clock_slots(
    snapshot: &Snapshot<'_>,
) -> Result<(Vec<Value>, Option<usize>), SuspensionReportError> {
    let Some(reference_unix_ms) = snapshot
        .ledger
        .events()
        .last()
        .map(PersistedEvent::timestamp_unix_ms)
    else {
        return Ok((Vec::new(), None));
    };
    let in_window = |event_type| EventQuery {
        event_type: Some(event_type),
        from_timestamp_unix_ms: Some(reference_unix_ms.saturating_sub(CLOCK_SLOT_WINDOW_MS)),
        ..EventQuery::default()
    };
    let mut dispatches = BTreeMap::<(String, String), Vec<SlotDispatch>>::new();
    for event in snapshot.query(in_window(EventType::PolicyDispatchAdmitted))? {
        if let EventPayload::Policy(PolicyPayload::DispatchAdmitted(payload)) = event.payload()
            && let Some(admission) = payload.admission()
        {
            dispatches
                .entry((
                    payload.task_id().to_owned(),
                    payload.instance_id().to_owned(),
                ))
                .or_default()
                .push(SlotDispatch {
                    sequence: event.sequence(),
                    admitted_at_unix_ms: admission.activity.admitted_at_unix_ms,
                });
        }
    }
    let mut entries = Vec::new();
    for event in snapshot
        .query(in_window(EventType::PolicyPlanningSignalObserved))?
        .iter()
        .rev()
    {
        let EventPayload::Policy(PolicyPayload::PlanningSignalObserved(payload)) = event.payload()
        else {
            continue;
        };
        if payload.kind() != PolicyPlanningSignalKind::GoalMissed
            || !payload.signal_id().starts_with(CLOCK_SLOT_SIGNAL_PREFIX)
        {
            continue;
        }
        let next_dispatch = payload
            .task_id()
            .and_then(|task_id| {
                dispatches.get(&(task_id.to_owned(), payload.instance_id().to_owned()))
            })
            .and_then(|admitted| {
                admitted
                    .iter()
                    .find(|dispatch| dispatch.sequence > event.sequence())
                    .copied()
            });
        let signal = SlotSignal {
            sequence: event.sequence(),
            recorded_at_unix_ms: event.timestamp_unix_ms(),
            signal_id: payload.signal_id(),
            task_id: payload.task_id(),
            instance_id: payload.instance_id(),
            outcome: payload.outcome(),
        };
        entries.push(clock_slot_entry(&signal, next_dispatch, reference_unix_ms));
    }
    let total = entries.len();
    entries.truncate(CLOCK_SLOT_LIMIT);
    Ok((entries, (total > CLOCK_SLOT_LIMIT).then_some(total)))
}
