// SPDX-License-Identifier: AGPL-3.0-only

//! Runtime-owned bindings of procedure aliases (Workflow #336 PKG-S3, option 丙).
//!
//! A binding is `procedure_ref`, `operation_id` and `yield_points`; its digest is the tuple v3
//! of these three and names no package content (M-PKG D13). The package is found by its path,
//! the configured `scheduled_execution.package_path`, which the binding carries as a location
//! outside the tuple: a scheduled run executes the request built from that path, and the
//! suspension lift check measures the package there.

use crate::codes::{HostCode, HostLocation};
use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::RuntimeErrorCode;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::{Code, Location};
use actingcommand_policy::{DispatchIntent, PolicyEvaluation};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const MAX_PROCEDURE_TOKEN_BYTES: usize = 256;
const SHA256_PREFIX: &str = "sha256:";
/// The domain of the binding tuple v3 (Workflow #336 PKG-S3): no package content.
const BINDING_DOMAIN: &str = "actingcommand.procedure-binding.v3";

/// Immutable binding of a procedure alias to its operation and yield points, with the location
/// of its package when the configuration schedules it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcedureBinding {
    procedure_ref: String,
    operation_id: String,
    yield_points: Vec<String>,
    binding_digest: String,
    /// The package's location (`scheduled_execution.package_path`, as the scheduled request
    /// names it); not part of the binding tuple.
    package_path: Option<String>,
}

impl ProcedureBinding {
    pub fn new(
        procedure_ref: impl Into<String>,
        operation_id: impl Into<String>,
        yield_points: Vec<String>,
    ) -> RuntimeHostResult<Self> {
        let procedure_ref = procedure_ref.into();
        let operation_id = operation_id.into();
        validate_token(&procedure_ref)?;
        validate_token(&operation_id)?;
        for yield_point in &yield_points {
            validate_token(yield_point)?;
        }
        let binding_digest = binding_digest(&procedure_ref, &operation_id, &yield_points)?;
        Ok(Self {
            procedure_ref,
            operation_id,
            yield_points,
            binding_digest,
            package_path: None,
        })
    }

    /// The binding with its package's location: the path the scheduled request of this
    /// procedure names. It changes neither the binding digest nor any decision identity.
    #[must_use]
    pub fn with_package_path(mut self, package_path: impl Into<String>) -> Self {
        self.package_path = Some(package_path.into());
        self
    }

    pub fn procedure_ref(&self) -> &str {
        &self.procedure_ref
    }

    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }

    pub fn yield_points(&self) -> &[String] {
        &self.yield_points
    }

    pub fn binding_digest(&self) -> &str {
        &self.binding_digest
    }

    /// The package's location, when the configuration schedules this procedure.
    pub fn package_path(&self) -> Option<&str> {
        self.package_path.as_deref()
    }
}

/// Trusted manifest used to bind policy decisions before they can reach lease admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcedureManifest {
    bindings: BTreeMap<String, ProcedureBinding>,
}

impl ProcedureManifest {
    pub fn new(bindings: impl IntoIterator<Item = ProcedureBinding>) -> RuntimeHostResult<Self> {
        let mut indexed = BTreeMap::new();
        for binding in bindings {
            let procedure_ref = binding.procedure_ref.clone();
            if indexed.insert(procedure_ref, binding).is_some() {
                return Err(manifest_fatal(
                    HostCode::ProcedureManifestAliasDuplicate,
                    HostLocation::BuildProcedureManifest,
                ));
            }
        }
        if indexed.is_empty() {
            return Err(manifest_fatal(
                HostCode::ProcedureManifestEmpty,
                HostLocation::BuildProcedureManifest,
            ));
        }
        Ok(Self { bindings: indexed })
    }

    pub fn binding(&self, procedure_ref: &str) -> Option<&ProcedureBinding> {
        self.bindings.get(procedure_ref)
    }

    pub(crate) fn bind_evaluation(
        &self,
        evaluation: &mut PolicyEvaluation,
    ) -> RuntimeHostResult<()> {
        let mut bound = evaluation.clone();
        let mut identities = BTreeMap::new();
        for intent in &mut bound.dispatch_intents {
            if intent.procedure_binding_digest.is_some() {
                return Err(manifest_fatal(
                    HostCode::ProcedureIntentAlreadyBound,
                    HostLocation::BindPolicyEvaluation,
                ));
            }
            let old_decision_id = intent.decision_id.clone();
            let old_reason_chain_id = intent.reason_chain_id.clone();
            let binding =
                self.resolve(&intent.procedure_ref, HostLocation::BindPolicyEvaluation)?;
            binding.validate_descriptor(intent, HostLocation::BindPolicyEvaluation)?;
            let decision_id = bound_decision_id(&old_decision_id, binding.binding_digest())?;
            let reason_chain_id = format!(
                "reason:{}",
                decision_id.strip_prefix("decision:").ok_or_else(|| {
                    manifest_fatal(
                        HostCode::ProcedureDecisionIdentityInvalid,
                        HostLocation::BindPolicyEvaluation,
                    )
                })?
            );
            intent.decision_id = decision_id.clone();
            intent.reason_chain_id = reason_chain_id.clone();
            intent.procedure_binding_digest = Some(binding.binding_digest.clone());
            if identities
                .insert(
                    old_decision_id,
                    (old_reason_chain_id, decision_id, reason_chain_id),
                )
                .is_some()
            {
                return Err(manifest_fatal(
                    HostCode::ProcedureDecisionIdentityDuplicate,
                    HostLocation::BindPolicyEvaluation,
                ));
            }
        }
        for reason_chain in &mut bound.reason_chains {
            let Some((old_reason_chain_id, decision_id, reason_chain_id)) =
                identities.get(&reason_chain.decision_id)
            else {
                continue;
            };
            if &reason_chain.id != old_reason_chain_id {
                return Err(manifest_fatal(
                    HostCode::ProcedureReasonChainIdentityMismatch,
                    HostLocation::BindPolicyEvaluation,
                ));
            }
            reason_chain.decision_id = decision_id.clone();
            reason_chain.id = reason_chain_id.clone();
        }
        if bound.dispatch_intents.iter().any(|intent| {
            !bound.reason_chains.iter().any(|reason_chain| {
                reason_chain.id == intent.reason_chain_id
                    && reason_chain.decision_id == intent.decision_id
            })
        }) {
            return Err(manifest_fatal(
                HostCode::ProcedureReasonChainMissing,
                HostLocation::BindPolicyEvaluation,
            ));
        }
        *evaluation = bound;
        Ok(())
    }

    pub(crate) fn validate_intent(
        &self,
        intent: &DispatchIntent,
        operation: impl Into<Location>,
    ) -> RuntimeHostResult<()> {
        let operation: Location = operation.into();
        let binding = self.resolve(&intent.procedure_ref, operation)?;
        binding.validate_descriptor(intent, operation)?;
        if intent.procedure_binding_digest.as_deref() != Some(binding.binding_digest()) {
            return Err(manifest_request(
                HostCode::ProcedureBindingDigestMismatch,
                operation,
            ));
        }
        Ok(())
    }

    fn resolve(
        &self,
        procedure_ref: &str,
        operation: impl Into<Location>,
    ) -> RuntimeHostResult<&ProcedureBinding> {
        let operation: Location = operation.into();
        self.bindings
            .get(procedure_ref)
            .ok_or_else(|| manifest_request(SharedCode::ProcedureManifestEntryMissing, operation))
    }
}

impl ProcedureBinding {
    fn validate_descriptor(
        &self,
        intent: &DispatchIntent,
        operation: impl Into<Location>,
    ) -> RuntimeHostResult<()> {
        let operation: Location = operation.into();
        if intent.operation_id != self.operation_id {
            return Err(manifest_request(
                HostCode::ProcedureOperationMismatch,
                operation,
            ));
        }
        if intent.yield_points != self.yield_points {
            return Err(manifest_request(
                HostCode::ProcedureYieldPointsMismatch,
                operation,
            ));
        }
        Ok(())
    }
}

/// The binding tuple v3 (`procedure_ref`, `operation_id`, `yield_points`; M-PKG D13): a package
/// changed in place, or found at another path, keeps its binding digest and its decisions.
fn binding_digest(
    procedure_ref: &str,
    operation_id: &str,
    yield_points: &[String],
) -> RuntimeHostResult<String> {
    let bytes = serde_json::to_vec(&(BINDING_DOMAIN, procedure_ref, operation_id, yield_points))
        .map_err(|_| {
            manifest_fatal(
                HostCode::ProcedureBindingEncodeFailed,
                HostLocation::BindProcedure,
            )
        })?;
    Ok(format!("{SHA256_PREFIX}{:x}", Sha256::digest(bytes)))
}

fn bound_decision_id(decision_id: &str, binding_digest: &str) -> RuntimeHostResult<String> {
    let bytes = serde_json::to_vec(&(decision_id, binding_digest)).map_err(|_| {
        manifest_fatal(
            HostCode::ProcedureDecisionIdentityEncodeFailed,
            HostLocation::BindPolicyEvaluation,
        )
    })?;
    Ok(format!("decision:{:x}", Sha256::digest(bytes)))
}

/// A binding's token refusal, under the binding's operation (Workflow #378 A2-P: the operation
/// was the refused field's name, data carried as a location; no caller records which field).
fn validate_token(value: &str) -> RuntimeHostResult<()> {
    if value.is_empty()
        || value.len() > MAX_PROCEDURE_TOKEN_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
    {
        return Err(manifest_fatal(
            HostCode::ProcedureManifestTokenInvalid,
            HostLocation::BindProcedure,
        ));
    }
    Ok(())
}

fn manifest_request(code: impl Into<Code>, operation: impl Into<Location>) -> RuntimeHostError {
    RuntimeHostError::request(code, operation, RuntimeErrorCode::InvalidRequest)
}

fn manifest_fatal(code: impl Into<Code>, operation: impl Into<Location>) -> RuntimeHostError {
    RuntimeHostError::fatal(code, operation, RuntimeErrorCode::RuntimeHalted)
}
