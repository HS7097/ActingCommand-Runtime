// SPDX-License-Identifier: AGPL-3.0-only

//! Prerequisite packages of `linear_steps` packages and their entry gate (Workflow #336 L2b;
//! `contracts/linear-steps.md`, "Prerequisite packages").
//!
//! A `linear_steps` package may name, in its `control.json` `prerequisite_package_id`, the
//! package that brings the screen to its first step; a `linear_steps` prerequisite may name its
//! own, at most three packages besides the dependent one. The chain is resolved against the
//! host's `prerequisite_packages` map and admitted at preparation (before any lease of a direct
//! or host-scheduled run, inside the held lease of a policy run). Every refusal there is denied
//! with a `contained_task_prerequisite_*` code and writes no task record.
//!
//! A `linear_steps` package that declares no prerequisite package and has a first step page
//! falls back to the host's return-home package of its game and server (Workflow #336 L2c,
//! R16): one more layer at the end of the chain, outside the three declared ones.
//!
//! At run time the gate checks the first step on one frame; when it does not pass, the
//! prerequisite package runs inside the same run (its own prerequisite first, layer by layer),
//! and then the first step is awaited for its step timeout. Only the existing
//! `TaskEntryPreflight` facts are written: a fact belongs to the innermost prerequisite package
//! opened (`EntryRecoveryPackageAdmitted`) and not yet closed (`EntryRecoveryCompleted` /
//! `EntryRecoveryFailed`), or to the dependent package when none is open.

use super::contained_task::{EntryRecoveryRuntime, PackageIdentity, prepare_contained_task};
use super::*;
use actingcommand_contract::{PackageRef, TaskTimingBudgetOrigin};

/// At most three declared prerequisite packages besides the dependent package; a return-home
/// layer (Workflow #336 L2c) is not counted.
pub(super) const MAX_PREREQUISITE_DEPTH: usize = 3;
/// The step indices of one gated run, prerequisite packages included.
const MAX_GATED_STEPS: u64 = 1_000;

const UNBOUND: &str = "contained_task_prerequisite_unbound";
const CYCLE: &str = "contained_task_prerequisite_cycle";
const DEPTH_EXCEEDED: &str = "contained_task_prerequisite_depth_exceeded";
const ADMISSION_FAILED: &str = "contained_task_prerequisite_admission_failed";
const MISMATCH: &str = "contained_task_prerequisite_mismatch";
pub(super) const INCOMPATIBLE: &str = "contained_task_prerequisite_incompatible";
const STEP_LIMIT: &str = "contained_task_prerequisite_step_limit";
const FINAL_PAGE_MISSING: &str = "contained_task_prerequisite_final_page_missing";
const RESOLVE_OPERATION: &str = "resolve_prerequisite_chain";

fn prerequisite_refusal(code: &'static str, detail: String) -> RequestFailure {
    RequestFailure::request(
        RuntimeHostError::request(code, RESOLVE_OPERATION, RuntimeErrorCode::PackageInvalid)
            .with_native_detail(detail),
        RuntimeReceiptState::Denied,
        None,
    )
}

/// A prerequisite package that is not admitted: the loader's refusal stays attached as the
/// related failure and a resource declaration rejection travels with it, as for a startup
/// package. A poisoning or fatal failure is returned unchanged.
fn prerequisite_admission_failure(mut failure: RequestFailure, detail: String) -> RequestFailure {
    if failure.poison_runtime || failure.error.is_fatal() {
        return failure;
    }
    let mut error = RuntimeHostError::request(
        ADMISSION_FAILED,
        RESOLVE_OPERATION,
        RuntimeErrorCode::PackageInvalid,
    )
    .with_native_detail(detail)
    .with_related_failure("package_admission", &failure.error);
    error.lifecycle.resource_declaration = failure.error.lifecycle.resource_declaration.take();
    failure.error = Box::new(error);
    failure
}

/// Workflow #336 L2d: why a return-home package from `return_home_packages` cannot stand in for
/// `dependent` on the stuck-recovery ladder's first rung or in the page-graph home entry, as the
/// detail of a `contained_task_prerequisite_incompatible` refusal: the checks the chain applies to
/// a return-home layer, then a prerequisite package of its own, which neither path runs.
pub(super) fn configured_return_home_incompatibility(
    package: &PreparedContainedTask,
    dependent: &PackageIdentity,
) -> Option<String> {
    let reason = package.prerequisite_incompatibility().or_else(|| {
        if package.game() != dependent.game {
            Some("game")
        } else if package.server() != dependent.server {
            Some("server")
        } else if package.resolution() != dependent.resolution {
            Some("resolution")
        } else if package.prerequisite_package_id().is_some() {
            Some("return_home_declares_prerequisite")
        } else {
            None
        }
    })?;
    Some(format!(
        "package_id={} reason={reason} source=return_home",
        package.package_label()
    ))
}

/// Workflow #336 L2d: the ladder's refusal of a configured return-home package, before any lease.
pub(super) fn configured_return_home_refusal(detail: String) -> RequestFailure {
    prerequisite_refusal(INCOMPATIBLE, detail)
}

impl HostShared {
    /// The return-home package `layer` falls back to (Workflow #336 L2c, R16): only a
    /// `linear_steps` package that declares no prerequisite package and has a first step page
    /// (no application entry, R24), when the host maps its game and server to a package that is
    /// not already in the chain.
    fn return_home_fallback(
        &self,
        layer: &PreparedContainedTask,
        visited: &BTreeSet<String>,
    ) -> Option<String> {
        if layer.prerequisite_package_id().is_some() || layer.linear_entry_page().is_none() {
            return None;
        }
        self.return_home_packages
            .get(&(layer.game().to_owned(), layer.server().to_owned()))
            .filter(|package_id| !visited.contains(*package_id))
            .cloned()
    }

    /// The binding of the return-home package `return_home_packages` names for `game` and
    /// `server` (Workflow #336 L2d, R23), for the stuck-recovery ladder and the page-graph home
    /// entry of a run whose request binds no recovery package. actingd binds every configured
    /// package id at startup (`return_home_package_unbound`); a host configured otherwise fails.
    pub(super) fn configured_return_home(
        &self,
        game: &str,
        server: &str,
    ) -> RuntimeHostResult<Option<&ContainedTaskRecoveryBinding>> {
        let Some(package_id) = self
            .return_home_packages
            .get(&(game.to_owned(), server.to_owned()))
        else {
            return Ok(None);
        };
        self.prerequisite_packages
            .get(package_id)
            .map(Some)
            .ok_or_else(|| {
                RuntimeHostError::fatal(
                    "return_home_package_unbound",
                    "resolve_return_home_package",
                    RuntimeErrorCode::RuntimeFatal,
                )
                .with_native_detail(format!("package_id={package_id}"))
            })
    }

    /// Resolves and admits the prerequisite chain of `prepared` (§5.2.1): empty when it declares
    /// no prerequisite package and has no return-home fallback, otherwise the packages outermost
    /// first. `material_deadline` is read only when there is a chain to admit.
    pub(super) fn resolve_prerequisite_chain(
        &self,
        instance_alias: &str,
        prepared: &PreparedContainedTask,
        material_deadline: impl FnOnce() -> Result<Instant, RequestFailure>,
    ) -> Result<Vec<PreparedContainedTask>, RequestFailure> {
        let mut visited = BTreeSet::from([prepared.package_label().to_owned()]);
        // The next package id and whether it is the return-home fallback.
        let mut next = match prepared.prerequisite_package_id() {
            Some(first) => Some((first.to_owned(), false)),
            None => self
                .return_home_fallback(prepared, &visited)
                .map(|package_id| (package_id, true)),
        };
        if next.is_none() {
            return Ok(Vec::new());
        }
        let deadline = material_deadline()?;
        let mut chain: Vec<PreparedContainedTask> = Vec::new();
        let mut declared = 0;
        while let Some((package_id, return_home)) = next {
            let layer = chain.len() + 1;
            let source = if return_home {
                " source=return_home"
            } else {
                ""
            };
            let detail =
                |extra: &str| format!("layer={layer} package_id={package_id}{extra}{source}");
            let Some(binding) = self.prerequisite_packages.get(&package_id) else {
                return Err(prerequisite_refusal(UNBOUND, detail("")));
            };
            if !visited.insert(package_id.clone()) {
                return Err(prerequisite_refusal(CYCLE, detail("")));
            }
            if !return_home {
                if declared >= MAX_PREREQUISITE_DEPTH {
                    return Err(prerequisite_refusal(DEPTH_EXCEEDED, detail("")));
                }
                declared += 1;
            }
            let request = ContainedTaskRequest::new(
                binding.package_path(),
                binding.expected_sha256().clone(),
            )
            .map_err(|_| prerequisite_refusal(ADMISSION_FAILED, detail("")))?;
            let admitted = prepare_contained_task(
                instance_alias,
                &request,
                self.execution.vision_provider(),
                deadline,
            )
            .map_err(|failure| prerequisite_admission_failure(failure, detail("")))?;
            if admitted.package_label() != package_id {
                return Err(prerequisite_refusal(
                    MISMATCH,
                    detail(&format!(
                        " declared_package_id={}",
                        admitted.package_label()
                    )),
                ));
            }
            let dependent = chain.last().unwrap_or(prepared);
            let incompatible = admitted.prerequisite_incompatibility().or_else(|| {
                if admitted.game() != dependent.game() {
                    Some("game")
                } else if admitted.server() != dependent.server() {
                    Some("server")
                } else if admitted.resolution() != dependent.resolution() {
                    Some("resolution")
                } else {
                    None
                }
            });
            if let Some(reason) = incompatible {
                return Err(prerequisite_refusal(
                    INCOMPATIBLE,
                    detail(&format!(" reason={reason}")),
                ));
            }
            // Only a `linear_steps` package declares a prerequisite; a page-graph package ends
            // the chain. The return-home layer falls back no further.
            next = match admitted.prerequisite_package_id() {
                Some(package_id) => Some((package_id.to_owned(), false)),
                None if return_home => None,
                None => self
                    .return_home_fallback(&admitted, &visited)
                    .map(|package_id| (package_id, true)),
            };
            chain.push(admitted);
        }
        let steps = chain
            .iter()
            .chain(std::iter::once(prepared))
            .map(|task| u64::from(task.maximum_executed_steps()))
            .sum::<u64>();
        if steps > MAX_GATED_STEPS {
            return Err(prerequisite_refusal(
                STEP_LIMIT,
                format!("maximum_executed_steps={steps}"),
            ));
        }
        Ok(chain)
    }
}

/// The prerequisite packages of one gated run: those opened and not yet closed, outermost
/// first, and the steps the completed ones executed.
struct Gate {
    open: Vec<PackageRef>,
    executed: u32,
}

/// The entry gate of a `linear_steps` package whose prerequisite chain is not empty (§5.3):
/// the gate, then `EntryTargetDisposition { Started }` and the package itself, its step indices
/// after those of the prerequisite packages that ran.
pub(super) fn run_linear_gated(
    prepared: &PreparedContainedTask,
    prerequisites: &[PreparedContainedTask],
    runtime: &mut RuntimeContainedTask<'_>,
) -> Result<ContainedTaskOutcome, ContainedTaskRunError<RequestFailure>> {
    let mut gate = Gate {
        open: Vec::new(),
        executed: 0,
    };
    gate_layer(runtime, &mut gate, 0, prepared, prerequisites)?;
    runtime.step_index_offset = gate.executed;
    runtime
        .record_entry_fact(TaskSemanticFact::EntryTargetDisposition {
            disposition: TaskEntryTargetDisposition::Started,
            failure_code: None,
        })
        .map_err(ContainedTaskRunError::Boundary)?;
    let mut outcome = prepared.run(runtime)?;
    outcome.executed_steps = outcome
        .executed_steps
        .checked_add(gate.executed)
        .ok_or_else(|| ContainedTaskRunError::task(STEP_LIMIT))?;
    Ok(outcome)
}

/// Layer `layer` of the gate: `package` and, in `chain`, its prerequisite package followed by
/// that package's own chain. Each prerequisite package runs at most once per run.
fn gate_layer(
    runtime: &mut RuntimeContainedTask<'_>,
    gate: &mut Gate,
    layer: usize,
    package: &PreparedContainedTask,
    chain: &[PreparedContainedTask],
) -> Result<(), ContainedTaskRunError<RequestFailure>> {
    let Some((prerequisite, rest)) = chain.split_first() else {
        return Ok(());
    };
    let required_page = package
        .linear_entry_page()
        .map(str::to_owned)
        .ok_or_else(|| ContainedTaskRunError::task("contained_task_state_invalid"))?;
    let initial = package
        .recognize_linear_entry(runtime)
        .map_err(|error| fail_recognition(runtime, gate, layer == 0, error))?;
    runtime
        .record_entry_fact(TaskSemanticFact::EntryRecognition {
            phase: TaskEntryRecognitionPhase::Initial,
            required_page: required_page.clone(),
            matched: initial,
        })
        .map_err(ContainedTaskRunError::Boundary)?;
    runtime
        .record_entry_fact(TaskSemanticFact::EntryRecoveryDecision { required: !initial })
        .map_err(ContainedTaskRunError::Boundary)?;
    if initial {
        return Ok(());
    }
    let package_sha256 = prerequisite.package_sha256().to_owned();
    runtime
        .record_entry_fact(TaskSemanticFact::EntryRecoveryPackageAdmitted {
            package_sha256: package_sha256.clone(),
        })
        .map_err(ContainedTaskRunError::Boundary)?;
    gate.open.push(package_sha256.clone());
    gate_layer(runtime, gate, layer + 1, prerequisite, rest)?;
    // Preparation already refused a chain whose step bounds exceed the limit.
    if u64::from(gate.executed) + u64::from(prerequisite.maximum_executed_steps()) > MAX_GATED_STEPS
    {
        return Err(fail_gate(
            runtime,
            gate,
            ContainedTaskRunError::task(STEP_LIMIT),
        ));
    }
    if runtime.configuration_records > 0 {
        runtime
            .record_configuration(
                EffectiveConfigurationFacts::EntryRecovery {
                    package_sha256: package_sha256.clone(),
                    timing: prerequisite.effective_timing(),
                },
                None,
                None,
                None,
            )
            .map_err(ContainedTaskRunError::Boundary)?;
    }
    runtime.step_index_offset = gate.executed;
    let previous_timing = runtime.task_timing.context();
    let execution = {
        let mut prerequisite_runtime = EntryRecoveryRuntime { inner: runtime };
        prerequisite.run_as_prerequisite(&mut prerequisite_runtime)
    };
    if let Err(ContainedTaskRunError::Task(error)) = &execution {
        runtime
            .task_timing
            .task_failure(error.timing(), error.timing_check_position());
    }
    runtime.task_timing.replace_context(previous_timing);
    // The prerequisite package's own failure code is carried out unchanged.
    let outcome = execution.map_err(|error| close_open(runtime, &gate.open, error, false))?;
    let Some(final_page) = outcome.final_page.clone() else {
        return Err(fail_gate(
            runtime,
            gate,
            ContainedTaskRunError::task(FINAL_PAGE_MISSING),
        ));
    };
    runtime
        .record_entry_fact(TaskSemanticFact::EntryRecoveryCompleted {
            package_sha256,
            final_page,
            executed_steps: outcome.executed_steps,
        })
        .map_err(ContainedTaskRunError::Boundary)?;
    gate.open.pop();
    gate.executed = match gate.executed.checked_add(outcome.executed_steps) {
        Some(executed) => executed,
        None => {
            return Err(fail_gate(
                runtime,
                gate,
                ContainedTaskRunError::task(STEP_LIMIT),
            ));
        }
    };
    // Page ids of different packages are not compared: the first step is awaited instead.
    let budget = Duration::from_millis(package.effective_timing().step_timeout.milliseconds);
    let origin = if layer == 0 {
        TaskTimingBudgetOrigin::Task
    } else {
        TaskTimingBudgetOrigin::EntryRecovery
    };
    // Workflow #336 L2c: a layer without a declared prerequisite package has the return-home
    // package as its next layer, and its recheck fails with its own code.
    let return_home = package
        .prerequisite_package_id()
        .is_none()
        .then(|| prerequisite.package_label());
    let waited = package
        .await_linear_entry(runtime, budget, origin, layer, return_home)
        .map_err(|error| fail_recognition(runtime, gate, false, error))?;
    runtime
        .record_entry_fact(TaskSemanticFact::EntryRecognition {
            phase: TaskEntryRecognitionPhase::PostRecovery,
            required_page,
            matched: waited.unmatched.is_none(),
        })
        .map_err(ContainedTaskRunError::Boundary)?;
    if let Some(error) = waited.unmatched {
        // The failure's timing belongs to the wait's own budget.
        runtime.task_timing.replace_context(Some(waited.timing));
        return Err(fail_gate(runtime, gate, ContainedTaskRunError::Task(error)));
    }
    Ok(())
}

/// A failure of the gate's own recognition. Only the outermost first check, before any fact of
/// the gate and before any prerequisite package ran, returns it unchanged, as the home entry
/// preflight does; every later one, the outermost recheck included, closes the open packages
/// and the gate once.
fn fail_recognition(
    runtime: &RuntimeContainedTask<'_>,
    gate: &Gate,
    outermost_first_check: bool,
    error: ContainedTaskRunError<RequestFailure>,
) -> ContainedTaskRunError<RequestFailure> {
    if outermost_first_check {
        error
    } else {
        close_open(runtime, &gate.open, error, true)
    }
}

/// A failure the gate decides itself: the open packages and the gate are closed with its code.
fn fail_gate(
    runtime: &RuntimeContainedTask<'_>,
    gate: &Gate,
    error: ContainedTaskRunError<RequestFailure>,
) -> ContainedTaskRunError<RequestFailure> {
    let code = match &error {
        ContainedTaskRunError::Task(error) => error.code(),
        ContainedTaskRunError::Boundary(failure)
        | ContainedTaskRunError::NonfatalOperation(failure) => failure.error.code(),
    };
    match record_closed(runtime, &gate.open, code) {
        Ok(()) => error,
        Err(failure) => ContainedTaskRunError::Boundary(failure),
    }
}

/// Closes every open prerequisite package with `error`'s code, innermost first, and the gate
/// once (`FailClosed`), classified as the home entry recovery classifies a failed recovery
/// package: a recognition failure, an unknown page or an input backend failure is recorded even
/// when the operation consumed its last remaining execution budget. A task failure other than
/// those is carried out as its code alone unless `keep_task_error`.
fn close_open(
    runtime: &RuntimeContainedTask<'_>,
    open: &[PackageRef],
    error: ContainedTaskRunError<RequestFailure>,
    keep_task_error: bool,
) -> ContainedTaskRunError<RequestFailure> {
    match error {
        ContainedTaskRunError::Task(error)
            if matches!(
                error.code(),
                "contained_task_recognition_failed" | "contained_task_page_unknown"
            ) =>
        {
            let mut primary = RuntimeHostError::request(
                error.code(),
                "run_contained_task",
                RuntimeErrorCode::BackendOperationFailed,
            );
            if let Some(detail) = error.detail() {
                primary = primary.with_native_detail(detail.to_owned());
            }
            match runtime.record_geometry_triggered_prerequisite_failure(open, &primary) {
                Ok(()) => ContainedTaskRunError::Task(error),
                Err(failure) => ContainedTaskRunError::Boundary(failure),
            }
        }
        ContainedTaskRunError::Task(error) => match record_closed(runtime, open, error.code()) {
            Ok(()) if keep_task_error => ContainedTaskRunError::Task(error),
            Ok(()) => ContainedTaskRunError::task(error.code()),
            Err(failure) => ContainedTaskRunError::Boundary(failure),
        },
        ContainedTaskRunError::NonfatalOperation(failure)
            if !failure.poison_runtime
                && !failure.error.is_fatal()
                && matches!(
                    failure.error.code(),
                    "input_backend_operation_failed" | "input_backend_open_failed"
                ) =>
        {
            match runtime.record_geometry_triggered_prerequisite_failure(open, &failure.error) {
                Ok(()) => ContainedTaskRunError::NonfatalOperation(failure),
                Err(record) => ContainedTaskRunError::Boundary(record),
            }
        }
        ContainedTaskRunError::NonfatalOperation(failure) => {
            match record_closed(runtime, open, failure.error.code()) {
                Ok(()) => ContainedTaskRunError::NonfatalOperation(failure),
                Err(record) => ContainedTaskRunError::Boundary(record),
            }
        }
        ContainedTaskRunError::Boundary(failure) => {
            match record_closed(runtime, open, failure.error.code()) {
                Ok(()) => ContainedTaskRunError::Boundary(failure),
                Err(record) => ContainedTaskRunError::Boundary(record),
            }
        }
    }
}

/// `EntryRecoveryFailed` for every open prerequisite package, innermost first, then the one
/// `EntryTargetDisposition { FailClosed }` of the gate.
fn record_closed(
    runtime: &RuntimeContainedTask<'_>,
    open: &[PackageRef],
    code: &str,
) -> Result<(), RequestFailure> {
    for package_sha256 in open.iter().rev() {
        runtime.record_entry_fact(TaskSemanticFact::EntryRecoveryFailed {
            package_sha256: package_sha256.clone(),
            failure_code: code.to_owned(),
        })?;
    }
    runtime.record_entry_fact(TaskSemanticFact::EntryTargetDisposition {
        disposition: TaskEntryTargetDisposition::FailClosed,
        failure_code: Some(code.to_owned()),
    })
}
