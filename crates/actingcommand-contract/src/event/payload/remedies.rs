// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 B15 (contracts/remedies.md section 2; model-b15 section 4): the package members of
// declared remedies, `package` interface revision 4. A mapping of `scheduling_outcome` may declare
// a `remedy`: the run that ends on its key is a success whose business result is not the expected
// one, and the opaque `need` names what may make it reachable through the opaque `capabilities`.
// A package's `control.json` may declare what it `provides`, with named parameters whose options
// and default it declares. The Runtime compares these names for equality only: it pairs no need
// with a capability and orders no capability. Task data only; never a member of a persisted event.

use super::SanitizationError;
use crate::codes::SanitizationCode;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The most capabilities one remedy names.
pub const MAX_REMEDY_CAPABILITIES: usize = 8;
/// The most entries of a package's `provides`.
pub const MAX_PROVIDED_CAPABILITIES: usize = 8;
/// The most parameters one provided capability declares.
pub const MAX_PROVIDED_PARAMETERS: usize = 8;
/// The most options one parameter declares.
pub const MAX_PARAMETER_OPTIONS: usize = 16;
/// The name no remedy and no provider may give a capability.
pub const RESERVED_CAPABILITY: &str = "none";

/// Whether `value` is a token of the remedy charset `^[a-z][a-z0-9_]{0,63}$`: a need, a
/// capability, a parameter name or an option.
pub fn remedy_token_valid(value: &str) -> bool {
    let bytes = value.as_bytes();
    matches!(bytes.first(), Some(b'a'..=b'z'))
        && bytes.len() <= 64
        && bytes
            .iter()
            .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'_'))
}

/// Whether `value` can name a capability: a token of the remedy charset other than `none`.
pub fn capability_token_valid(value: &str) -> bool {
    remedy_token_valid(value) && value != RESERVED_CAPABILITY
}

// Interface anchor `package` revision 4 (contracts/component-interfaces.md): a reader without
// `remedy` and `provides` refuses a pack that declares them; raise the revision in
// distribution/windows/component-interfaces.json when packs need what older readers lack.

/// The `remedy` of one mapping (`task.json`): the need a run ending on the mapping's key has, the
/// capabilities that can serve it, and the ones the package allows when no target record and no
/// configuration decide.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemedyDeclaration {
    need: String,
    capabilities: Vec<String>,
    #[serde(rename = "default", default, skip_serializing_if = "Option::is_none")]
    default_capabilities: Option<Vec<String>>,
}

impl RemedyDeclaration {
    pub fn need(&self) -> &str {
        &self.need
    }

    pub fn capabilities(&self) -> &[String] {
        &self.capabilities
    }

    /// The package default, a subset of [`Self::capabilities`]; empty when the package declares
    /// none (`default` absent or `[]`).
    pub fn default_capabilities(&self) -> &[String] {
        self.default_capabilities.as_deref().unwrap_or_default()
    }

    /// The rules of contracts/remedies.md section 2.1: `need` is a token; `capabilities` are 1 to
    /// [`MAX_REMEDY_CAPABILITIES`] unique capability tokens; `default` names each of them at most
    /// once. A refusal is `invalid_scheduling_outcome_declaration` with the member as its field.
    pub fn validate(&self) -> Result<(), SanitizationError> {
        let invalid = |field| {
            SanitizationError::new(SanitizationCode::InvalidSchedulingOutcomeDeclaration, field)
        };
        if !remedy_token_valid(&self.need) {
            return Err(invalid("remedy.need"));
        }
        if self.capabilities.is_empty() || self.capabilities.len() > MAX_REMEDY_CAPABILITIES {
            return Err(invalid("remedy.capabilities"));
        }
        let mut capabilities = BTreeSet::new();
        for capability in &self.capabilities {
            if !capability_token_valid(capability) || !capabilities.insert(capability.as_str()) {
                return Err(invalid("remedy.capabilities"));
            }
        }
        let mut defaults = BTreeSet::new();
        for capability in self.default_capabilities() {
            if !capabilities.contains(capability.as_str()) || !defaults.insert(capability.as_str())
            {
                return Err(invalid("remedy.default"));
            }
        }
        Ok(())
    }
}

/// One entry of a package's `provides` (`control.json`): a capability the package serves, whether
/// serving it is irreversible, and the parameters whose options and default state its scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvidedCapability {
    capability: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    irreversible: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parameters: Option<Vec<ProvidedParameter>>,
}

impl ProvidedCapability {
    pub fn capability(&self) -> &str {
        &self.capability
    }

    /// Whether the package declares the capability irreversible (absent: `false`).
    pub fn irreversible(&self) -> bool {
        self.irreversible.unwrap_or(false)
    }

    /// The declared parameters, in declaration order; empty when it declares none.
    pub fn parameters(&self) -> &[ProvidedParameter] {
        self.parameters.as_deref().unwrap_or_default()
    }
}

/// One parameter of a provided capability: its name, its options and the option it takes when no
/// target record and no configuration choose one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvidedParameter {
    name: String,
    options: Vec<String>,
    #[serde(rename = "default")]
    default_option: String,
}

impl ProvidedParameter {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn options(&self) -> &[String] {
        &self.options
    }

    pub fn default_option(&self) -> &str {
        &self.default_option
    }
}

/// The rule a refused `provides` breaks: the `reason` of `contained_task_control_invalid`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvidesProblem {
    /// The list, a capability or an `irreversible` marker.
    ProvidesInvalid,
    /// A capability's parameters.
    ProvidesParametersInvalid,
}

impl ProvidesProblem {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProvidesInvalid => "provides_invalid",
            Self::ProvidesParametersInvalid => "provides_parameters_invalid",
        }
    }
}

/// A refused `provides`: the rule it breaks and the JSON pointer, in `control.json`, of the member
/// that breaks it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvidesViolation {
    pub problem: ProvidesProblem,
    pub pointer: String,
}

impl ProvidesViolation {
    fn provides(pointer: String) -> Self {
        Self {
            problem: ProvidesProblem::ProvidesInvalid,
            pointer,
        }
    }

    fn parameters(pointer: String) -> Self {
        Self {
            problem: ProvidesProblem::ProvidesParametersInvalid,
            pointer,
        }
    }
}

/// The rules of contracts/remedies.md section 2.2: 1 to [`MAX_PROVIDED_CAPABILITIES`] entries
/// whose capabilities are unique capability tokens; each capability with at most
/// [`MAX_PROVIDED_PARAMETERS`] parameters, at least one when it is irreversible; parameter names
/// are tokens unique across the whole list; each parameter has 1 to [`MAX_PARAMETER_OPTIONS`]
/// unique option tokens and a default that is one of them.
pub fn validate_provides(provides: &[ProvidedCapability]) -> Result<(), ProvidesViolation> {
    if provides.is_empty() || provides.len() > MAX_PROVIDED_CAPABILITIES {
        return Err(ProvidesViolation::provides("/provides".to_owned()));
    }
    let mut capabilities = BTreeSet::new();
    let mut names = BTreeSet::new();
    for (index, provided) in provides.iter().enumerate() {
        let pointer = format!("/provides/{index}");
        if !capability_token_valid(&provided.capability)
            || !capabilities.insert(provided.capability.as_str())
        {
            return Err(ProvidesViolation::provides(format!("{pointer}/capability")));
        }
        let parameters = provided.parameters();
        if parameters.len() > MAX_PROVIDED_PARAMETERS
            || (provided.irreversible() && parameters.is_empty())
        {
            return Err(ProvidesViolation::parameters(format!(
                "{pointer}/parameters"
            )));
        }
        for (index, parameter) in parameters.iter().enumerate() {
            let pointer = format!("{pointer}/parameters/{index}");
            if !remedy_token_valid(&parameter.name) || !names.insert(parameter.name.as_str()) {
                return Err(ProvidesViolation::parameters(format!("{pointer}/name")));
            }
            if parameter.options.is_empty() || parameter.options.len() > MAX_PARAMETER_OPTIONS {
                return Err(ProvidesViolation::parameters(format!("{pointer}/options")));
            }
            let mut options = BTreeSet::new();
            for (index, option) in parameter.options.iter().enumerate() {
                if !remedy_token_valid(option) || !options.insert(option.as_str()) {
                    return Err(ProvidesViolation::parameters(format!(
                        "{pointer}/options/{index}"
                    )));
                }
            }
            if !options.contains(parameter.default_option.as_str()) {
                return Err(ProvidesViolation::parameters(format!("{pointer}/default")));
            }
        }
    }
    Ok(())
}
