// SPDX-License-Identifier: AGPL-3.0-only

//! Prerequisite packages and their entry gate (Workflow #336 L2b; `contracts/linear-steps.md`,
//! "Prerequisite packages").
//!
//! A `linear_steps` package, and (Workflow #373 NAV-3a, M-373 §4.2) a `navigable_route` package,
//! may name, in its `control.json` `prerequisite_package_id`, the package that brings the screen
//! to its entry; a prerequisite package of either mode may name its own, at most three packages
//! besides the dependent one. The chain is resolved against the standard-pack registry of the
//! dependent package's game (Workflow #373 NAV-3a, §4.3: `<standard_packs.root>/<game>/
//! bundle.json`, read when the run is prepared) and admitted at preparation (before any lease of
//! a direct or host-scheduled run, inside the held lease of a policy run). Every refusal there is
//! denied with a `contained_task_prerequisite_*` code (or a broken game root's own code) and
//! writes no task record.
//!
//! A layer with a page entry that declares no prerequisite package falls back to the registry's
//! return-home package of its game and server (Workflow #336 L2c, R16): one more layer at the end
//! of the chain, outside the three declared ones. A page-graph package run as the run's own
//! package keeps its home entry instead (R23).
//!
//! At run time the gate checks the layer's entry (a linear package's first step on one frame, a
//! page-graph package's entry page or entry page set as its home entry is checked); when it does
//! not pass, the prerequisite package runs inside the same run (its own prerequisite first, layer
//! by layer), and then the entry is awaited for its step timeout. Only the existing
//! `TaskEntryPreflight` facts are written: a fact belongs to the innermost prerequisite package
//! opened (`EntryRecoveryPackageAdmitted`) and not yet closed (`EntryRecoveryCompleted` /
//! `EntryRecoveryFailed`), or to the dependent package when none is open; each layer's records
//! carry its `role`, `prerequisite` or `return_home`.

use super::contained_task::{EntryRecoveryRuntime, PackageIdentity, prepare_contained_task};
use super::failure_settlement::ResolvedLayer;
use super::standard_pack::GameRegistry;
use super::*;
use crate::codes::{HostCode, HostLocation};
use crate::error::spelled;
use actingcommand_contract::codes::{ContractLocation, DeviceSelfCheckFailure, SharedCode};
use actingcommand_contract::outcome::Code;
use actingcommand_contract::{TaskEntryRecoveryRole, TaskTimingBudgetOrigin};
pub(super) use actingcommand_execution_kernel::MAX_PREREQUISITE_DEPTH;
use actingcommand_execution_kernel::{MAX_GATED_STEPS, PrerequisiteChain};

const ADMISSION_FAILED: HostCode = HostCode::ContainedTaskPrerequisiteAdmissionFailed;
pub(super) const INCOMPATIBLE: &str = SharedCode::ContainedTaskPrerequisiteIncompatible.as_str();
const STEP_LIMIT: &str = SharedCode::ContainedTaskPrerequisiteStepLimit.as_str();
const FINAL_PAGE_MISSING: &str = HostCode::ContainedTaskPrerequisiteFinalPageMissing.as_str();
const RESOLVE_OPERATION: HostLocation = HostLocation::ResolvePrerequisiteChain;

fn prerequisite_refusal(code: impl Into<Code>, detail: String) -> RequestFailure {
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

/// Workflow #336 L2d: why the registry's return-home package (Workflow #373 NAV-3a) cannot stand
/// in for `dependent` on the stuck-recovery ladder's first rung or in the page-graph home entry,
/// as the
/// detail of a `contained_task_prerequisite_incompatible` refusal: the checks the chain applies to
/// a return-home layer, then a prerequisite package of its own, which neither path runs.
pub(super) fn configured_return_home_incompatibility(
    package: &PreparedContainedTask,
    dependent: &PackageIdentity,
) -> Option<String> {
    let reason = package
        .package_descriptor()
        .prerequisite_incompatibility_with(
            &dependent.game,
            &dependent.server,
            dependent.resolution,
            true,
        )?;
    Some(format!(
        "package_id={} reason={reason} source=return_home",
        package.package_label()
    ))
}

/// Workflow #336 L2d: the ladder's refusal of a configured return-home package, before any lease.
pub(super) fn configured_return_home_refusal(detail: String) -> RequestFailure {
    prerequisite_refusal(SharedCode::ContainedTaskPrerequisiteIncompatible, detail)
}

/// A refusal of the shared prerequisite predicate, relayed with the kernel's code and detail and
/// denied before any lease: a chain layer's, and (Workflow #336 PKG-S4, M-PKG D7) a return-home
/// package that is not the package its registry entry names.
pub(super) fn kernel_refusal(
    error: actingcommand_execution_kernel::ContainedTaskError,
) -> RequestFailure {
    registry_refusal(error, None)
}

/// [`kernel_refusal`] with, for `contained_task_prerequisite_unbound`, why the registry does not
/// bind the package (Workflow #373 NAV-3a, M-373 §4.3: value `registry_problem`).
fn registry_refusal(
    error: actingcommand_execution_kernel::ContainedTaskError,
    problem: Option<actingcommand_contract::RegistryProblem>,
) -> RequestFailure {
    let refused = RuntimeHostError::relayed_request(
        error.code().as_str(),
        RESOLVE_OPERATION.as_str(),
        RuntimeErrorCode::PackageInvalid,
    )
    .with_native_detail(error.detail().unwrap_or("").to_owned());
    let refused = match problem {
        Some(problem) => refused.with_outcome_values(|outcome| outcome.registry_problem(problem)),
        None => refused,
    };
    RequestFailure::request(refused, RuntimeReceiptState::Denied, None)
}

/// Workflow #373 NAV-3a (M-373 §4.3): the registry's return-home package of a failed or
/// entering package's game and server, as the ladder's first rung and the page-graph home entry
/// take it (R23): its `package_id` and where it is.
#[derive(Debug, Clone)]
pub(super) struct RegistryReturnHome {
    pub(super) package_id: String,
    pub(super) binding: ContainedTaskRecoveryBinding,
}

impl HostShared {
    /// The return-home package `layer` falls back to (Workflow #336 L2c, R16): a layer with a
    /// page entry that declares no prerequisite package (no application entry, R24; Workflow #373
    /// NAV-3a, a page-graph layer too), when the registry of its game names a return-home
    /// package for its server that is not already in the chain.
    fn next_prerequisite(
        chain: &PrerequisiteChain,
        registry: &GameRegistry,
    ) -> Option<actingcommand_execution_kernel::PrerequisiteLink> {
        chain.next(registry.return_home_package(chain.dependent().server()))
    }

    /// Workflow #373 NAV-3a (M-373 §4.3): the registry's return-home package of `game` and
    /// `server` (Workflow #336 L2d, R23), for the stuck-recovery ladder and the page-graph home
    /// entry of a run whose request binds no recovery package; `None` without a root, without a
    /// root of the game, or when its bundle names none for the server. The registry is read now;
    /// a broken game root is its own refusal.
    pub(super) fn registry_return_home(
        &self,
        game: &str,
        server: &str,
    ) -> Result<Option<RegistryReturnHome>, RequestFailure> {
        let registry = self.read_game_registry(game)?;
        let Some(package_id) = registry.return_home_package(server) else {
            return Ok(None);
        };
        let path = registry.package_path(package_id).map_err(|_| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                SharedCode::ContainedTaskStateInvalid,
                HostLocation::ResolvePrerequisiteChain,
                RuntimeErrorCode::RuntimeHalted,
            ))
        })?;
        let binding = ContainedTaskRecoveryBinding::new(path.to_string_lossy()).map_err(|_| {
            prerequisite_refusal(
                ADMISSION_FAILED,
                format!("package_id={package_id} source=return_home"),
            )
        })?;
        Ok(Some(RegistryReturnHome {
            package_id: package_id.to_owned(),
            binding,
        }))
    }

    /// [`Self::registry_return_home`] for the stuck-recovery ladder: a broken game root is
    /// recorded with its own outcome (`runtime.lifecycle_observed`, phase `standard_pack_read`)
    /// and the ladder goes on without a return-home package; only a fatal failure stops it.
    pub(super) fn ladder_return_home(
        &self,
        game: &str,
        server: &str,
    ) -> RuntimeHostResult<Option<RegistryReturnHome>> {
        match self.registry_return_home(game, server) {
            Ok(found) => Ok(found),
            Err(failure) if failure.poison_runtime || failure.error.is_fatal() => {
                Err(*failure.error)
            }
            Err(failure) => {
                self.record_standard_pack_read(failure.error.outcome())?;
                Ok(None)
            }
        }
    }

    /// Resolves and admits the prerequisite chain of `prepared` (§5.2.1): empty when it declares
    /// no prerequisite package and has no return-home fallback, otherwise the packages outermost
    /// first. `material_deadline` is read only when there is a chain to admit. Workflow #373
    /// NAV-3a: the registry of the package's game is read only when the package declares a
    /// prerequisite package or may fall back to the return-home package.
    pub(super) fn resolve_prerequisite_chain(
        &self,
        instance_alias: &str,
        prepared: &PreparedContainedTask,
        material_deadline: impl FnOnce() -> Result<Instant, RequestFailure>,
    ) -> Result<Vec<PreparedContainedTask>, RequestFailure> {
        self.resolve_prerequisite_chain_recorded(
            instance_alias,
            prepared,
            material_deadline,
            &mut Vec::new(),
        )
    }

    /// `resolve_prerequisite_chain` that also records each layer's package id and source as
    /// soon as it is known, a layer that is then refused included (Workflow #336 L6: the
    /// scheduled path keeps them for the failure identity, §5.2.1 last item).
    pub(super) fn resolve_prerequisite_chain_recorded(
        &self,
        instance_alias: &str,
        prepared: &PreparedContainedTask,
        material_deadline: impl FnOnce() -> Result<Instant, RequestFailure>,
        recorded: &mut Vec<ResolvedLayer>,
    ) -> Result<Vec<PreparedContainedTask>, RequestFailure> {
        let mut qualification = PrerequisiteChain::new(prepared.package_descriptor());
        if qualification
            .dependent()
            .prerequisite_package_id()
            .is_none()
            && !qualification.may_fall_back()
        {
            return Ok(Vec::new());
        }
        // Workflow #373 NAV-3a (§4.3): every layer has the game of the package (a layer of
        // another game is refused as incompatible), so one registry serves the whole chain.
        let registry = self.read_game_registry(prepared.game())?;
        let mut next = Self::next_prerequisite(&qualification, &registry);
        if next.is_none() {
            return Ok(Vec::new());
        }
        let deadline = material_deadline()?;
        let mut chain: Vec<PreparedContainedTask> = Vec::new();
        while let Some(link) = next {
            let dependent = chain.last().unwrap_or(prepared);
            let path = registry.package_path(&link.package_id);
            recorded.push(ResolvedLayer {
                package_id: link.package_id.clone(),
                return_home: link
                    .return_home
                    .then(|| (dependent.game().to_owned(), dependent.server().to_owned())),
                package_path: path.as_ref().ok().cloned(),
            });
            qualification
                .begin(&link, path.is_ok())
                .map_err(|error| registry_refusal(error, path.as_ref().err().copied()))?;
            let path = path.expect("shared prerequisite predicate checked the binding");
            let request = ContainedTaskRequest::new(path.to_string_lossy())
                .map_err(|_| prerequisite_refusal(ADMISSION_FAILED, link.detail("")))?;
            let admitted = prepare_contained_task(
                instance_alias,
                &request,
                self.execution()?.vision_provider(),
                deadline,
            )
            .map_err(|failure| prerequisite_admission_failure(failure, link.detail("")))?;
            qualification
                .admit(&link, admitted.package_descriptor())
                .map_err(kernel_refusal)?;
            next = Self::next_prerequisite(&qualification, &registry);
            chain.push(admitted);
        }
        qualification.finish().map_err(kernel_refusal)?;
        Ok(chain)
    }
}

/// The prerequisite packages of one gated run: those opened and not yet closed, outermost
/// first, by `package_id` (Workflow #336 PKG-S2) with their role (Workflow #373 NAV-3a), and the
/// steps the completed ones executed.
struct Gate {
    open: Vec<(String, TaskEntryRecoveryRole)>,
    executed: u32,
}

/// The entry gate of a package whose prerequisite chain is not empty (§5.3; Workflow #373
/// NAV-3a, a `linear_steps` or a `navigable_route` package): the gate, then
/// `EntryTargetDisposition { Started }` and the package itself, its step indices after those of
/// the prerequisite packages that ran.
pub(super) fn run_gated(
    prepared: &PreparedContainedTask,
    prerequisites: &[PreparedContainedTask],
    runtime: &mut RuntimeContainedTask<'_>,
) -> Result<ContainedTaskOutcome, ContainedTaskRunError<RequestFailure>> {
    let mut gate = Gate {
        open: Vec::new(),
        executed: 0,
    };
    gate_layer(runtime, &mut gate, 0, prepared, prerequisites)?;
    // Workflow #373 NAV-3a: the gate recorded the entry decision; a page-graph package's own
    // entry check in its run records only its recognition (as after the home entry).
    runtime.entry_preflight_recorded = true;
    // Workflow #373 NAV-6: after the steps of the interrupt layers that ran so far too.
    runtime
        .begin_layer(gate.executed)
        .ok_or_else(|| ContainedTaskRunError::task(STEP_LIMIT))?;
    runtime
        .record_entry_fact(TaskSemanticFact::EntryTargetDisposition {
            disposition: TaskEntryTargetDisposition::Started,
            outcome: None,
        })
        .map_err(ContainedTaskRunError::Boundary)?;
    let mut outcome = runtime.run_main(prepared)?;
    outcome.executed_steps = runtime
        .layer_end(outcome.executed_steps)
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
    // Workflow #373 NAV-3a: a linear package's entry is its first step's page; a page-graph
    // package's is its entry page or its entry page set (M-373 §4.2).
    let required_pages = package.gate_entry_pages().ok_or_else(|| {
        ContainedTaskRunError::task(SharedCode::ContainedTaskStateInvalid.as_str())
    })?;
    let initial = package
        .recognize_gate_entry(runtime)
        .map_err(|error| fail_recognition(runtime, gate, layer == 0, error))?;
    let matched = initial.is_some();
    runtime
        .record_entry_fact(TaskSemanticFact::EntryRecognition {
            phase: TaskEntryRecognitionPhase::Initial,
            required_pages: required_pages.clone(),
            matched_page: initial,
            matched,
        })
        .map_err(ContainedTaskRunError::Boundary)?;
    runtime
        .record_entry_fact(TaskSemanticFact::EntryRecoveryDecision { required: !matched })
        .map_err(ContainedTaskRunError::Boundary)?;
    if matched {
        return Ok(());
    }
    // Workflow #373 NAV-3a (M-373 §8.3): a layer that declares no prerequisite package has the
    // registry's return-home package as its next layer; a declared one is a prerequisite.
    let role = if package.prerequisite_package_id().is_some() {
        TaskEntryRecoveryRole::ReturnHome
    } else {
        TaskEntryRecoveryRole::Prerequisite
    };
    let package_label = prerequisite.package_label().to_owned();
    runtime
        .record_entry_fact(TaskSemanticFact::EntryRecoveryPackageAdmitted {
            package_label: package_label.clone(),
            role: Some(role),
        })
        .map_err(ContainedTaskRunError::Boundary)?;
    gate.open.push((package_label.clone(), role));
    gate_layer(runtime, gate, layer + 1, prerequisite, rest)?;
    // Preparation already refused a chain whose step bounds exceed the limit; Workflow #373
    // NAV-6: the interrupt layers' steps count too.
    if u64::from(gate.executed)
        + u64::from(prerequisite.maximum_executed_steps())
        + u64::from(runtime.interrupt_steps)
        > MAX_GATED_STEPS
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
                    package_label: package_label.clone(),
                    timing: prerequisite.effective_timing(),
                },
                None,
                None,
                None,
            )
            .map_err(ContainedTaskRunError::Boundary)?;
    }
    runtime
        .begin_layer(gate.executed)
        .ok_or_else(|| fail_gate(runtime, gate, ContainedTaskRunError::task(STEP_LIMIT)))?;
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
            package_label,
            final_page,
            executed_steps: outcome.executed_steps,
            role: Some(role),
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
    // Page ids of different packages are not compared: the entry is awaited instead.
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
        .await_gate_entry(runtime, budget, origin, layer, return_home)
        .map_err(|error| fail_recognition(runtime, gate, false, error))?;
    runtime
        .record_entry_fact(TaskSemanticFact::EntryRecognition {
            phase: TaskEntryRecognitionPhase::PostRecovery,
            required_pages,
            matched: waited.matched.is_some(),
            matched_page: waited.matched,
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
        ContainedTaskRunError::Task(error) => error.code().as_str(),
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
    open: &[(String, TaskEntryRecoveryRole)],
    error: ContainedTaskRunError<RequestFailure>,
    keep_task_error: bool,
) -> ContainedTaskRunError<RequestFailure> {
    match error {
        ContainedTaskRunError::Task(error)
            if spelled(
                error.code().as_str(),
                &[
                    SharedCode::ContainedTaskRecognitionFailed.code(),
                    SharedCode::ContainedTaskPageUnknown.code(),
                ],
            ) =>
        {
            let mut primary = RuntimeHostError::relayed_request(
                error.code().as_str(),
                ContractLocation::RunContainedTask.as_str(),
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
        ContainedTaskRunError::Task(error) => {
            match record_closed(runtime, open, error.code().as_str()) {
                Ok(()) if keep_task_error => ContainedTaskRunError::Task(error),
                Ok(()) => ContainedTaskRunError::task(error.code().as_str()),
                Err(failure) => ContainedTaskRunError::Boundary(failure),
            }
        }
        ContainedTaskRunError::NonfatalOperation(failure)
            if !failure.poison_runtime
                && !failure.error.is_fatal()
                && spelled(
                    failure.error.code(),
                    &[
                        SharedCode::InputBackendOperationFailed.code(),
                        DeviceSelfCheckFailure::InputBackendOpenFailed.code(),
                    ],
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
    open: &[(String, TaskEntryRecoveryRole)],
    code: &str,
) -> Result<(), RequestFailure> {
    // Ledger revision 3: the gate's failure is each fact's outcome, the code it relays.
    let outcome = actingcommand_contract::outcome::Outcome::relayed(code);
    for (package_label, role) in open.iter().rev() {
        runtime.record_entry_fact(TaskSemanticFact::EntryRecoveryFailed {
            package_label: Some(package_label.clone()),
            outcome: outcome.clone(),
            role: Some(*role),
        })?;
    }
    runtime.record_entry_fact(TaskSemanticFact::EntryTargetDisposition {
        disposition: TaskEntryTargetDisposition::FailClosed,
        outcome: Some(outcome),
    })
}
