// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #336 L6 (§12.8, `contracts/policy-suspension.md`): the read-only report behind
//! `actingd suspended`: the scheduled (task, instance) pairs a failure paused, those a package
//! update lifted, and those that keep failing without accumulating (`repeating`).
//!
//! The ledger is opened read-only without referenced material and without the owner lock, so
//! a running daemon is not disturbed. Each pair is judged with the same lift rule as live
//! admission (§12.7), against the configuration the caller loaded. Error frames are read
//! read-only and compared again for display; the recorded failure identity stays the verdict.

use crate::RuntimeHostConfig;
use crate::failure_identity::{
    FailureIdentity, IdentityLayer, SuspensionLiftView, failure_rows, failure_step,
    identifier_text, locate_error_frame,
};
use actingcommand_artifact_store::read_projected_verified;
use actingcommand_contract::{
    CONFIG_PARAMETERS_FACT_KEY, EventPayload, EventQuery, EventSeverity, EventType, PackageRef,
    PolicyExecutionOutcome, PolicyFailureDisposition, PolicyFailureRecord, PolicyPayload, RunId,
    RuntimePayload, TaskPayload, TaskSemanticFact,
};
use actingcommand_execution_kernel::{compare_failure_frames, failure_frame_ccoeff};
use actingcommand_ledger::{
    GlobalLedger, GlobalLedgerEvidence, GlobalLedgerEvidenceConfig, LedgerArtifactReference,
    PersistedEvent,
};
use actingcommand_policy::{CatalogSources, compile_catalog};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

const SCHEMA_VERSION: &str = "actingcommand.actingd.suspended.v1";
const LIFTS_WHEN: &str = "main_digest_changes_or_any_layer_changes";

/// What `actingd suspended` reads: the loaded configuration and when its file last changed.
pub struct SuspensionReportRequest<'a> {
    pub config_path: &'a Path,
    /// The configuration file's modification time, when the file system reports one.
    pub config_modified_unix_ms: Option<u64>,
    pub host: &'a RuntimeHostConfig,
    pub catalog: &'a CatalogSources,
}

/// A report that could not be produced, with its code and operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuspensionReportError {
    code: &'static str,
    operation: &'static str,
    detail: Option<String>,
}

impl SuspensionReportError {
    const fn new(code: &'static str, operation: &'static str) -> Self {
        Self {
            code,
            operation,
            detail: None,
        }
    }

    fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub const fn code(&self) -> &'static str {
        self.code
    }

    pub const fn operation(&self) -> &'static str {
        self.operation
    }

    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }
}

impl fmt::Display for SuspensionReportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} during {}", self.code, self.operation)?;
        if let Some(detail) = &self.detail {
            write!(formatter, ": {detail}")?;
        }
        Ok(())
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

    /// The run's terminal failure code and the native detail of its lifecycle failure record
    /// of that code (or of `fallback_code` when the run has no terminal).
    fn run_failure(&self, run_id: Option<RunId>, fallback_code: &str) -> (Option<String>, Value) {
        let Some(run_id) = run_id else {
            return (None, Value::Null);
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
                        failure_code: Some(code),
                        ..
                    } => Some(code.clone()),
                    _ => None,
                },
                _ => None,
            });
        let code = terminal.clone().unwrap_or_else(|| fallback_code.to_owned());
        let detail = self
            .ledger
            .query(&by_run(EventType::RuntimeFailed))
            .iter()
            .rev()
            .find_map(|event| match event.payload() {
                EventPayload::Runtime(RuntimePayload::Failed(payload)) => payload
                    .lifecycle_failure()
                    .filter(|record| record.code() == code)
                    .and_then(|record| record.native_detail())
                    .map(|detail| detail.text().to_owned()),
                _ => None,
            });
        (terminal, detail.map_or(Value::Null, Value::String))
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
        let unavailable = |reason: &str| {
            json!({
                "source": "recomputed", "status": "unavailable", "reason": reason,
                "changed_cells_milli": null, "digest_mean_milli": null,
                "ccoeff": null, "ccoeff_error": null,
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
                                        "reason": compared.reason,
                                        "changed_cells_milli": compared.changed_cells_milli,
                                        "digest_mean_milli": compared.digest_mean_milli,
                                        "ccoeff": ccoeff.as_ref().ok(),
                                        "ccoeff_error": ccoeff.as_ref().err().map(ToString::to_string),
                                    }),
                                    Some(compared.verdict.as_str()),
                                )
                            }
                            Err(error) => (unavailable(error.reason()), None),
                        }
                    }
                    _ => (unavailable("artifact_unreadable"), None),
                }
            }
            _ => (unavailable("frame_missing"), None),
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

/// `actingd suspended`: the suspended, lifted and repeating pairs of the configured state
/// root, judged against the configuration in `request`.
pub fn suspension_report(
    request: &SuspensionReportRequest<'_>,
) -> Result<Value, SuspensionReportError> {
    let manifest = request.host.procedure_manifest().ok_or_else(|| {
        SuspensionReportError::new("suspended_policy_unconfigured", "read_suspension_config")
    })?;
    let compiled = compile_catalog(request.catalog).map_err(|_| {
        SuspensionReportError::new("policy_catalog_compile_failed", "read_suspension_config")
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
            "suspended_ledger_unavailable",
            "open_suspension_ledger",
        )
        .with_detail("state_root_missing"));
    }
    let ledger = GlobalLedger::open_evidence(
        GlobalLedgerEvidenceConfig::new(state_root).sqlite_material_not_read(),
        |_| None,
    )
    .map_err(|error| {
        SuspensionReportError::new("suspended_ledger_unavailable", "open_suspension_ledger")
            .with_detail(error.code())
    })?;
    if !ledger.is_complete() {
        return Err(SuspensionReportError::new(
            "suspended_ledger_incomplete",
            "open_suspension_ledger",
        ));
    }
    let snapshot = Snapshot {
        ledger,
        artifact_root: state_root,
    };
    let view = SuspensionLiftView {
        manifest,
        prerequisite_packages: request.host.prerequisite_packages(),
        return_home_packages: request.host.return_home_packages(),
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
                (
                    payload.package_digest().clone(),
                    event.links().run_id().copied(),
                ),
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
                    outcome: payload.outcome().clone(),
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
                if payload.record().key == CONFIG_PARAMETERS_FACT_KEY =>
            {
                Some(payload.record().observed_at_unix_ms)
            }
            _ => None,
        });
    let mut warnings = Vec::new();
    let pending_restart = match daemon_started_at_unix_ms {
        None => {
            warnings.push("daemon_start_unknown".to_owned());
            false
        }
        Some(started) => {
            let newer = request
                .config_modified_unix_ms
                .is_some_and(|modified| modified > started);
            if newer {
                warnings.push("config_newer_than_daemon_start".to_owned());
            }
            newer
        }
    };
    let (mut suspended, mut lifted, mut repeating) = (Vec::new(), Vec::new(), Vec::new());
    for ((task_id, instance_id), executions) in &pairs {
        let Some(latest) = executions.last() else {
            continue;
        };
        let PolicyExecutionOutcome::Failed { failure } = &latest.outcome else {
            continue;
        };
        let pair = format!("{task_id}/{instance_id}");
        let identity = FailureIdentity::parse(&failure.error_code);
        if identity.is_none() && failure.error_code.contains("~v1~") {
            warnings.push(format!("error_code_unparsed:{pair}"));
        }
        let previous = executions
            .len()
            .checked_sub(2)
            .and_then(|index| executions.get(index));
        let (paused_digest, current_run) = intents
            .get(&latest.decision_id)
            .map_or((None, None), |(digest, run_id)| (Some(digest), *run_id));
        let base = identity.as_ref().map_or_else(
            || failure.error_code.clone(),
            |identity| identity.base.clone(),
        );
        let (terminal, detail) = snapshot.run_failure(current_run, &base);
        if failure.disposition == PolicyFailureDisposition::PausedTask {
            let procedure_ref = procedure_refs.get(task_id);
            if procedure_ref.is_none() {
                warnings.push(format!("task_not_in_catalog:{pair}"));
            }
            let layers = identity
                .as_ref()
                .map_or(&[][..], |identity| identity.layers.as_slice());
            let package = package_json(&view, procedure_ref, paused_digest, layers);
            let paused = json!({
                "execution_sequence": latest.sequence,
                "observed_at_unix_ms": latest.observed_at_unix_ms,
                "decision_id": latest.decision_id,
                "run_id": current_run.as_ref().map(identifier_text),
            });
            // Workflow #361 M6: an interrupted settlement is lifted by the restart that
            // recorded it, whatever the configuration.
            let lift = if failure.error_code == crate::policy_control::POLICY_SETTLEMENT_INTERRUPTED
            {
                Some(("restart".to_owned(), "active"))
            } else {
                procedure_ref
                    .and_then(|procedure_ref| view.lifted(procedure_ref, paused_digest, layers))
                    .map(|lift| {
                        let effective = if pending_restart {
                            "pending_restart"
                        } else {
                            "active"
                        };
                        (lift.label(), effective)
                    })
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
            let previous_run = previous.and_then(|previous| {
                intents
                    .get(&previous.decision_id)
                    .and_then(|(_, run_id)| *run_id)
            });
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
            suspended.push(json!({
                "task_id": task_id,
                "instance_id": instance_id,
                "procedure_ref": procedure_ref,
                "error_code": failure.error_code,
                "failure_code": terminal.as_deref().unwrap_or(&base),
                "consecutive_same_error": failure.consecutive_same_error,
                "effective_class": wire(&failure.effective_class),
                "severity": wire(&latest.severity),
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
        if FailureIdentity::parse(&previous_failure.error_code)
            .is_none_or(|previous| previous.prefix() != identity.prefix())
        {
            continue;
        }
        let previous_run = intents
            .get(&previous.decision_id)
            .and_then(|(_, run_id)| *run_id);
        repeating.push(json!({
            "task_id": task_id,
            "instance_id": instance_id,
            "error_code": failure.error_code,
            "failure_code": terminal.as_deref().unwrap_or(&base),
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
    Ok(json!({
        "schema_version": SCHEMA_VERSION,
        "status": "ok",
        "config_path": request.config_path.to_string_lossy(),
        "state_root": state_root.to_string_lossy(),
        "through_sequence": through_sequence,
        "daemon_started_at_unix_ms": daemon_started_at_unix_ms,
        "suspended": suspended,
        "lifted": lifted,
        "repeating": repeating,
        "warnings": warnings,
    }))
}

/// The main package digest a pair was paused on and has now, and each recorded layer.
fn package_json(
    view: &SuspensionLiftView<'_>,
    procedure_ref: Option<&String>,
    paused_digest: Option<&PackageRef>,
    layers: &[IdentityLayer],
) -> Value {
    json!({
        "paused": paused_digest.map(PackageRef::prefixed_wire_value),
        "current": procedure_ref
            .and_then(|procedure_ref| view.current_main(procedure_ref))
            .map(PackageRef::prefixed_wire_value),
        "layers": layers
            .iter()
            .map(|layer| {
                let current = view.current_layer_digest(layer);
                json!({
                    "kind": layer.kind(),
                    "package_id": view.layer_package_id(layer),
                    "paused_digest_prefix": layer.digest(),
                    "current_digest_prefix": current,
                    "changed": current.as_deref() != Some(layer.digest()),
                })
            })
            .collect::<Vec<_>>(),
    })
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
