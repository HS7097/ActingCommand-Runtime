// SPDX-License-Identifier: AGPL-3.0-only

//! Runtime-owned content bindings for external procedure packages.

use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::RuntimeErrorCode;
use actingcommand_policy::{DispatchIntent, PolicyEvaluation};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const MAX_PROCEDURE_TOKEN_BYTES: usize = 256;
const SHA256_PREFIX: &str = "sha256:";

/// Immutable binding between a procedure alias and approved package content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcedureBinding {
    procedure_ref: String,
    package_digest: String,
    operation_id: String,
    yield_points: Vec<String>,
    binding_digest: String,
}

impl ProcedureBinding {
    pub fn new(
        procedure_ref: impl Into<String>,
        package_digest: impl Into<String>,
        operation_id: impl Into<String>,
        yield_points: Vec<String>,
    ) -> RuntimeHostResult<Self> {
        let procedure_ref = procedure_ref.into();
        let package_digest = package_digest.into();
        let operation_id = operation_id.into();
        validate_token(&procedure_ref, "procedure_ref")?;
        validate_token(&operation_id, "operation_id")?;
        validate_sha256(&package_digest, "package_digest")?;
        for yield_point in &yield_points {
            validate_token(yield_point, "yield_point")?;
        }
        let binding_digest = binding_digest(
            &procedure_ref,
            &package_digest,
            &operation_id,
            &yield_points,
        )?;
        Ok(Self {
            procedure_ref,
            package_digest,
            operation_id,
            yield_points,
            binding_digest,
        })
    }

    pub fn procedure_ref(&self) -> &str {
        &self.procedure_ref
    }

    pub fn package_digest(&self) -> &str {
        &self.package_digest
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
                    "procedure_manifest_alias_duplicate",
                    "build_procedure_manifest",
                ));
            }
        }
        if indexed.is_empty() {
            return Err(manifest_fatal(
                "procedure_manifest_empty",
                "build_procedure_manifest",
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
            if intent.package_digest.is_some() || intent.procedure_binding_digest.is_some() {
                return Err(manifest_fatal(
                    "procedure_intent_already_bound",
                    "bind_policy_evaluation",
                ));
            }
            let old_decision_id = intent.decision_id.clone();
            let old_reason_chain_id = intent.reason_chain_id.clone();
            let binding = self.resolve(&intent.procedure_ref, "bind_policy_evaluation")?;
            binding.validate_descriptor(intent, "bind_policy_evaluation")?;
            let decision_id = bound_decision_id(&old_decision_id, binding.binding_digest())?;
            let reason_chain_id = format!(
                "reason:{}",
                decision_id.strip_prefix("decision:").ok_or_else(|| {
                    manifest_fatal(
                        "procedure_decision_identity_invalid",
                        "bind_policy_evaluation",
                    )
                })?
            );
            intent.decision_id = decision_id.clone();
            intent.reason_chain_id = reason_chain_id.clone();
            intent.package_digest = Some(binding.package_digest.clone());
            intent.procedure_binding_digest = Some(binding.binding_digest.clone());
            if identities
                .insert(
                    old_decision_id,
                    (old_reason_chain_id, decision_id, reason_chain_id),
                )
                .is_some()
            {
                return Err(manifest_fatal(
                    "procedure_decision_identity_duplicate",
                    "bind_policy_evaluation",
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
                    "procedure_reason_chain_identity_mismatch",
                    "bind_policy_evaluation",
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
                "procedure_reason_chain_missing",
                "bind_policy_evaluation",
            ));
        }
        *evaluation = bound;
        Ok(())
    }

    pub(crate) fn validate_intent(
        &self,
        intent: &DispatchIntent,
        operation: &'static str,
    ) -> RuntimeHostResult<()> {
        let binding = self.resolve(&intent.procedure_ref, operation)?;
        binding.validate_descriptor(intent, operation)?;
        if intent.package_digest.as_deref() != Some(binding.package_digest()) {
            return Err(manifest_request(
                "procedure_package_digest_mismatch",
                operation,
            ));
        }
        if intent.procedure_binding_digest.as_deref() != Some(binding.binding_digest()) {
            return Err(manifest_request(
                "procedure_binding_digest_mismatch",
                operation,
            ));
        }
        Ok(())
    }

    fn resolve(
        &self,
        procedure_ref: &str,
        operation: &'static str,
    ) -> RuntimeHostResult<&ProcedureBinding> {
        self.bindings
            .get(procedure_ref)
            .ok_or_else(|| manifest_request("procedure_manifest_entry_missing", operation))
    }
}

impl ProcedureBinding {
    fn validate_descriptor(
        &self,
        intent: &DispatchIntent,
        operation: &'static str,
    ) -> RuntimeHostResult<()> {
        if intent.operation_id != self.operation_id {
            return Err(manifest_request("procedure_operation_mismatch", operation));
        }
        if intent.yield_points != self.yield_points {
            return Err(manifest_request(
                "procedure_yield_points_mismatch",
                operation,
            ));
        }
        Ok(())
    }
}

fn binding_digest(
    procedure_ref: &str,
    package_digest: &str,
    operation_id: &str,
    yield_points: &[String],
) -> RuntimeHostResult<String> {
    let bytes = serde_json::to_vec(&(procedure_ref, package_digest, operation_id, yield_points))
        .map_err(|_| manifest_fatal("procedure_binding_encode_failed", "bind_procedure"))?;
    Ok(format!("{SHA256_PREFIX}{:x}", Sha256::digest(bytes)))
}

fn bound_decision_id(decision_id: &str, binding_digest: &str) -> RuntimeHostResult<String> {
    let bytes = serde_json::to_vec(&(decision_id, binding_digest)).map_err(|_| {
        manifest_fatal(
            "procedure_decision_identity_encode_failed",
            "bind_policy_evaluation",
        )
    })?;
    Ok(format!("decision:{:x}", Sha256::digest(bytes)))
}

fn validate_token(value: &str, field: &'static str) -> RuntimeHostResult<()> {
    if value.is_empty()
        || value.len() > MAX_PROCEDURE_TOKEN_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
    {
        return Err(manifest_fatal("procedure_manifest_token_invalid", field));
    }
    Ok(())
}

fn validate_sha256(value: &str, field: &'static str) -> RuntimeHostResult<()> {
    let Some(hex) = value.strip_prefix(SHA256_PREFIX) else {
        return Err(manifest_fatal("procedure_package_digest_invalid", field));
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(manifest_fatal("procedure_package_digest_invalid", field));
    }
    Ok(())
}

fn manifest_request(code: &'static str, operation: &'static str) -> RuntimeHostError {
    RuntimeHostError::request(code, operation, RuntimeErrorCode::InvalidRequest)
}

fn manifest_fatal(code: &'static str, operation: &'static str) -> RuntimeHostError {
    RuntimeHostError::fatal(code, operation, RuntimeErrorCode::RuntimeFatal)
}
