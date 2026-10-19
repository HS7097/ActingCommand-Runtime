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
// Workflow #411 B15 R5: a run's read-only parameters.
use actingcommand_contract::{FactContent, FactRecord, FactScope, FactValue, InstanceFactSnapshot};
use sha2::{Digest, Sha256};

/// Workflow #411 B15 R5 (contracts/remedies.md section 4; contracts/instance-fact-store.md, "Run
/// parameters"): the key prefix reserved for a run's read-only parameters, shared with Workflow
/// #411 B8. Nothing may publish a key under it; its records exist only as an overlay of the
/// instance fact snapshots of one run.
pub const RUN_PARAMETERS_KEY_PREFIX: &str = "session.dispatch.";
/// The key of one parameter's record is this prefix followed by the parameter's name.
const RUN_PARAMETER_KEY_PREFIX: &str = "session.dispatch.parameter.";
/// `source_detector`, `source_snapshot_id` and `schema_version` of an overlay record.
const RUN_PARAMETER_DETECTOR: &str = "runtime.run-parameters";
const RUN_PARAMETER_SNAPSHOT: &str = "run-parameters";
const RUN_PARAMETER_SCHEMA: &str = "fact.v1";

/// Workflow #411 B15 R5: the read-only parameters one run of a package reads as
/// `session.dispatch.parameter.<name>` = `<value>`. A policy run reads the values its dispatch
/// carries (`policy_dispatch_parameters`), observed at the dispatch's admission; a run without a
/// dispatch reads every parameter the package declares with its declared default, observed when
/// the snapshot is taken. Nothing is stored: [`Self::overlay`] adds them to one snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunParameters {
    package_id: String,
    values: Vec<(String, String)>,
    admitted_at_unix_ms: Option<u64>,
}

impl RunParameters {
    /// Adds one record per parameter to `snapshot`, after the records it holds: the key
    /// `session.dispatch.parameter.<name>`, the value as a string, the snapshot's instance as its
    /// scope, confidence 1000, no expiry, observed at the dispatch's admission or, for a run
    /// without a dispatch, at `now_unix_ms`. The snapshot keeps its identity, the stored
    /// projection's; a select step's `input_sha256` covers the records added.
    pub fn overlay(&self, snapshot: &mut InstanceFactSnapshot, now_unix_ms: u64) {
        if self.values.is_empty() {
            return;
        }
        let observed_at_unix_ms = self.admitted_at_unix_ms.unwrap_or(now_unix_ms);
        let bundle_hash = format!(
            "{:x}",
            Sha256::digest(format!("package:{}", self.package_id).as_bytes())
        );
        let scope = FactScope::Instance {
            instance_id: snapshot.context.instance_id.clone(),
        };
        for (name, value) in &self.values {
            snapshot.records.push(FactRecord {
                scope: scope.clone(),
                key: format!("{RUN_PARAMETER_KEY_PREFIX}{name}"),
                content: FactContent::Inline {
                    value: FactValue::String(value.clone()),
                },
                observed_at_unix_ms,
                expires_at_unix_ms: None,
                ttl_policy: None,
                confidence_milli: 1_000,
                source_detector: RUN_PARAMETER_DETECTOR.to_owned(),
                source_snapshot_id: RUN_PARAMETER_SNAPSHOT.to_owned(),
                schema_version: RUN_PARAMETER_SCHEMA.to_owned(),
                resource_bundle_hash: bundle_hash.clone(),
                invalidate_on: Vec::new(),
            });
        }
    }
}

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
            provides: control.provides,
        })
    }

    /// Every parameter the package's `provides` declares, by name, with its declared default, in
    /// declaration order; empty when it declares none. The values a run without a dispatch
    /// receives (contracts/remedies.md section 4).
    pub fn declared_parameter_defaults(&self) -> Vec<(String, String)> {
        self.control
            .provides
            .iter()
            .flat_map(ProvidedCapability::parameters)
            .map(|parameter| {
                (
                    parameter.name().to_owned(),
                    parameter.default_option().to_owned(),
                )
            })
            .collect()
    }

    /// Workflow #411 B15 R5: the parameters of a run of this package without a dispatch (a
    /// direct run, a run the host starts itself, a Lab run): every parameter it declares, with
    /// its declared default (contracts/remedies.md section 4).
    pub fn default_run_parameters(&self) -> RunParameters {
        RunParameters {
            package_id: self.control.package_id.clone(),
            values: self.declared_parameter_defaults(),
            admitted_at_unix_ms: None,
        }
    }

    /// Workflow #411 B15 R5: the parameters a policy dispatch of this package carries,
    /// `(parameter_name, parameter_value)` each, observed at the dispatch's admission
    /// `admitted_at_unix_ms`.
    pub fn dispatched_run_parameters(
        &self,
        values: Vec<(String, String)>,
        admitted_at_unix_ms: u64,
    ) -> RunParameters {
        RunParameters {
            package_id: self.control.package_id.clone(),
            values,
            admitted_at_unix_ms: Some(admitted_at_unix_ms),
        }
    }

    /// #411 Lab RP (contracts/remedies.md section 4): the parameters a Lab evaluation of this
    /// package reads, without a dispatch: every parameter it declares with its declared default,
    /// each `(name, value)` of `given` replacing its parameter's default, as a dispatch's chosen
    /// value does; observed when the snapshot is taken. Without `given` it is
    /// [`Self::default_run_parameters`]. A name the package does not declare (a package that
    /// declares none declares no name), a value that is not one of its parameter's declared
    /// options, and a name given twice are refused, never ignored.
    pub fn given_run_parameters(
        &self,
        given: &[(String, String)],
    ) -> Result<RunParameters, GivenParameterRefusal> {
        let mut parameters = self.default_run_parameters();
        if self.control.provides.is_empty() {
            return Ok(parameters);
        }
        let mut seen = BTreeSet::new();
        for (name, value) in given {
            let refused = |problem| GivenParameterRefusal {
                name: name.clone(),
                problem,
            };
            if !seen.insert(name.as_str()) {
                return Err(refused(GivenParameterProblem::Repeated));
            }
            let declared = self
                .control
                .provides
                .iter()
                .flat_map(ProvidedCapability::parameters)
                .find(|parameter| parameter.name() == name)
                .ok_or_else(|| refused(GivenParameterProblem::Undeclared))?;
            if declared.options().is_empty() {
                return Err(refused(GivenParameterProblem::NotAnOption));
            }
            let (_, default) = parameters
                .values
                .iter_mut()
                .find(|(declared, _)| declared == name)
                .ok_or_else(|| refused(GivenParameterProblem::Undeclared))?;
            let _ = (default, value);
        }
        Ok(parameters)
    }
}

/// #411 Lab RP: why a parameter given to a Lab evaluation is refused
/// ([`PreparedContainedTask::given_run_parameters`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GivenParameterProblem {
    /// The package declares no parameter of this name.
    Undeclared,
    /// The value is not one of the options the parameter declares.
    NotAnOption,
    /// The name is given more than once.
    Repeated,
}

/// #411 Lab RP: a refused given parameter, by its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GivenParameterRefusal {
    pub name: String,
    pub problem: GivenParameterProblem,
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
        .any(|provided| named.contains(provided.capability()))
    {
        return Err(ContainedTaskError::with_detail(
            ExecutionKernelCode::ContainedTaskControlInvalid.code(),
            "reason=provides_own_remedy",
        ));
    }
    Ok(())
}
