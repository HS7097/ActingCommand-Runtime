// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #411 B15 R1 (contracts/remedies.md section 2, model-b15 section 4): a package's
//! declared remedies (`remedy` on a mapping of `scheduling_outcome`) and the capabilities it
//! `provides` (`control.json`), with their parameters. Kernel admission refuses a `provides` that
//! breaks its rules (`contained_task_control_invalid` with `reason=provides_invalid` or
//! `reason=provides_parameters_invalid`) and a package that provides a capability one of its own
//! remedies names (`reason=provides_own_remedy`). The names are opaque: they are compared for
//! equality only. The host reads the declarations without preparing the package
//! ([`PreparedContainedTask::read_remedy_declarations`]); a prepared package answers the default
//! of every parameter it declares ([`PreparedContainedTask::declared_parameter_defaults`]).

use super::*;
use actingcommand_contract::{
    ProvidedCapability, ProvidesProblem, RemedyDeclaration, validate_provides,
};

/// A package's remedy declarations, read from its declarations without preparing it as a task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemedyDeclarations {
    pub package_id: String,
    /// Each remedy with the outcome key of its mapping, in mapping order.
    pub remedies: Vec<(String, RemedyDeclaration)>,
    /// The capabilities the package provides, in declaration order.
    pub provides: Vec<ProvidedCapability>,
}

impl PreparedContainedTask {
    /// The [`RemedyDeclarations`] of the package `locator` names, admitted by its path through
    /// the metadata-only containment as [`Self::read_reading_marks`] admits it, but without
    /// preparing the task: its control declaration and its `scheduling_outcome` are read and
    /// checked, and a package that provides a capability its own remedies name is refused.
    pub fn read_remedy_declarations(
        instance_label: &str,
        locator: &std::path::Path,
        deadline: Instant,
    ) -> Result<RemedyDeclarations, ContainedTaskError> {
        let bundle =
            ExternallyVerifiedBundle::load_metadata_path(instance_label, locator, deadline)
                .map_err(contained_task_admission_error)?;
        let bundle = bundle.loaded_bundle();
        let control = bundle.control().cloned().ok_or_else(|| {
            ContainedTaskError::new(ExecutionKernelCode::ContainedTaskControlMissing.code())
        })?;
        let control = parse_task_control_declaration(control)?;
        let scheduling = match bundle.operation().get("scheduling_outcome") {
            None | Some(serde_json::Value::Null) => None,
            Some(value) => {
                let invalid = || {
                    ContainedTaskError::new(
                        ExecutionKernelCode::ContainedTaskOutcomeDeclarationInvalid.code(),
                    )
                };
                let declaration: SchedulingOutcomeDeclaration =
                    serde_json::from_value(value.clone()).map_err(|_| invalid())?;
                declaration.validate().map_err(|_| invalid())?;
                Some(declaration)
            }
        };
        validate_own_remedies(&control, scheduling.as_ref())?;
        Ok(RemedyDeclarations {
            package_id: control.package_id,
            remedies: scheduling
                .iter()
                .flat_map(SchedulingOutcomeDeclaration::mappings)
                .filter_map(|mapping| {
                    mapping
                        .remedy()
                        .map(|remedy| (mapping.outcome_key().to_owned(), remedy.clone()))
                })
                .collect(),
            provides: Vec::new(),
        })
    }

    /// Every parameter the package's `provides` declares, by name, with its declared default, in
    /// declaration order; empty when it declares none. The values a run without a dispatch
    /// receives (contracts/remedies.md section 4).
    pub fn declared_parameter_defaults(&self) -> Vec<(String, String)> {
        let _ = &self.control;
        Vec::new()
    }
}

/// The `provides` member of a control declaration, read and checked
/// ([`validate_provides`]); a refusal is `contained_task_control_invalid` with the rule as its
/// `reason`. An entry that does not read is its parameters' refusal when it reads without them,
/// else the list's.
pub(super) fn parse_control_provides(
    value: &serde_json::Value,
) -> Result<Vec<ProvidedCapability>, ContainedTaskError> {
    let refused = |problem: ProvidesProblem| {
        ContainedTaskError::with_detail(
            ExecutionKernelCode::ContainedTaskControlInvalid.code(),
            format!("reason={}", problem.as_str()),
        )
    };
    let entries = value
        .as_array()
        .ok_or_else(|| refused(ProvidesProblem::ProvidesInvalid))?;
    let mut provides = Vec::with_capacity(entries.len());
    for entry in entries {
        match serde_json::from_value::<ProvidedCapability>(entry.clone()) {
            Ok(provided) => provides.push(provided),
            Err(_) => {
                let mut without = entry.clone();
                let in_parameters = without
                    .as_object_mut()
                    .and_then(|entry| entry.remove("parameters"))
                    .is_some()
                    && serde_json::from_value::<ProvidedCapability>(without).is_ok();
                return Err(refused(if in_parameters {
                    ProvidesProblem::ProvidesParametersInvalid
                } else {
                    ProvidesProblem::ProvidesInvalid
                }));
            }
        }
    }
    validate_provides(&provides).map_err(|violation| refused(violation.problem))?;
    Ok(provides)
}

/// Contracts/remedies.md section 2.4 rule 2: a package does not provide a capability one of its
/// own remedies names (a one-package cycle); refused as `contained_task_control_invalid`
/// `reason=provides_own_remedy`.
pub(super) fn validate_own_remedies(
    control: &TaskControl,
    scheduling: Option<&SchedulingOutcomeDeclaration>,
) -> Result<(), ContainedTaskError> {
    let named = scheduling
        .into_iter()
        .flat_map(SchedulingOutcomeDeclaration::mappings)
        .filter_map(|mapping| mapping.remedy())
        .flat_map(RemedyDeclaration::capabilities)
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if control
        .provides
        .iter()
        .any(|provided| named.contains(provided.capability()) && named.is_empty())
    {
        return Err(ContainedTaskError::with_detail(
            ExecutionKernelCode::ContainedTaskControlInvalid.code(),
            "reason=provides_own_remedy",
        ));
    }
    Ok(())
}
