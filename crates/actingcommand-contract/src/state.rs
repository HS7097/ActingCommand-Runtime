// SPDX-License-Identifier: AGPL-3.0-only

//! Versioned Runtime state, release-set, migration, and recovery contracts.

use crate::SanitizationError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub const RUNTIME_RELEASE_SET_SCHEMA_VERSION: &str = "actingcommand.release-set.v2";

const MAX_IDENTIFIER_BYTES: usize = 128;
const MAX_RELEASE_RESOURCES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseResourceVersion {
    resource_id: String,
    version: String,
    content_sha256: String,
}

impl ReleaseResourceVersion {
    pub fn new(
        resource_id: impl Into<String>,
        version: impl Into<String>,
        content_sha256: impl Into<String>,
    ) -> Result<Self, SanitizationError> {
        let value = Self {
            resource_id: resource_id.into(),
            version: version.into(),
            content_sha256: content_sha256.into(),
        };
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), SanitizationError> {
        validate_identifier(&self.resource_id, "resource_id")?;
        validate_version(&self.version, "resource_version")?;
        validate_sha256(&self.content_sha256, "resource_content_sha256")
    }

    pub fn resource_id(&self) -> &str {
        &self.resource_id
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn content_sha256(&self) -> &str {
        &self.content_sha256
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeReleaseSet {
    schema_version: String,
    release_id: String,
    runtime_version: String,
    runtime_content_sha256: String,
    ui_version: String,
    ui_content_sha256: String,
    resources: Vec<ReleaseResourceVersion>,
}

impl RuntimeReleaseSet {
    pub fn new(
        runtime_version: impl Into<String>,
        runtime_content_sha256: impl Into<String>,
        ui_version: impl Into<String>,
        ui_content_sha256: impl Into<String>,
        mut resources: Vec<ReleaseResourceVersion>,
    ) -> Result<Self, SanitizationError> {
        resources.sort_by(|left, right| left.resource_id.cmp(&right.resource_id));
        let mut value = Self {
            schema_version: RUNTIME_RELEASE_SET_SCHEMA_VERSION.to_owned(),
            release_id: String::new(),
            runtime_version: runtime_version.into(),
            runtime_content_sha256: runtime_content_sha256.into(),
            ui_version: ui_version.into(),
            ui_content_sha256: ui_content_sha256.into(),
            resources,
        };
        value.validate_components()?;
        value.release_id = release_id_for(&value)?;
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), SanitizationError> {
        if self.schema_version != RUNTIME_RELEASE_SET_SCHEMA_VERSION {
            return Err(SanitizationError::new(
                "unsupported_release_set_schema",
                "release_set",
            ));
        }
        self.validate_components()?;
        if self.release_id != release_id_for(self)? {
            return Err(SanitizationError::new(
                "release_set_identity_mismatch",
                "release_id",
            ));
        }
        Ok(())
    }

    pub fn schema_version(&self) -> &str {
        &self.schema_version
    }

    pub fn release_id(&self) -> &str {
        &self.release_id
    }

    pub fn runtime_version(&self) -> &str {
        &self.runtime_version
    }

    pub fn runtime_content_sha256(&self) -> &str {
        &self.runtime_content_sha256
    }

    pub fn ui_version(&self) -> &str {
        &self.ui_version
    }

    pub fn ui_content_sha256(&self) -> &str {
        &self.ui_content_sha256
    }

    pub fn resources(&self) -> &[ReleaseResourceVersion] {
        &self.resources
    }

    pub fn manifest_sha256(&self) -> String {
        self.release_id
            .strip_prefix("release:")
            .map_or_else(String::new, |digest| format!("sha256:{digest}"))
    }

    fn validate_components(&self) -> Result<(), SanitizationError> {
        validate_version(&self.runtime_version, "runtime_version")?;
        validate_sha256(&self.runtime_content_sha256, "runtime_content_sha256")?;
        validate_version(&self.ui_version, "ui_version")?;
        validate_sha256(&self.ui_content_sha256, "ui_content_sha256")?;
        if self.resources.is_empty() || self.resources.len() > MAX_RELEASE_RESOURCES {
            return Err(SanitizationError::new(
                "invalid_release_resource_count",
                "release_resources",
            ));
        }
        let mut seen = BTreeSet::new();
        let mut previous = None::<&str>;
        for resource in &self.resources {
            resource.validate()?;
            if !seen.insert(resource.resource_id())
                || previous.is_some_and(|value| value > resource.resource_id())
            {
                return Err(SanitizationError::new(
                    "release_resources_not_canonical",
                    "release_resources",
                ));
            }
            previous = Some(resource.resource_id());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseTransitionKind {
    Activate,
    Rollback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateValidationResult {
    Passed,
    Recovered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateRecoveryAction {
    None,
    ImportedLegacy,
    ReplayedCommitted,
    RejectedUncommitted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateTransitionStatus {
    validation_result: StateValidationResult,
    recovery_action: StateRecoveryAction,
}

impl StateTransitionStatus {
    pub const fn new(
        validation_result: StateValidationResult,
        recovery_action: StateRecoveryAction,
    ) -> Self {
        Self {
            validation_result,
            recovery_action,
        }
    }

    pub const fn validation_result(self) -> StateValidationResult {
        self.validation_result
    }

    pub const fn recovery_action(self) -> StateRecoveryAction {
        self.recovery_action
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateMigrationData {
    migration_id: String,
    state_key: String,
    from_schema_version: String,
    to_schema_version: String,
    payload_sha256: String,
    validation_result: StateValidationResult,
    recovery_action: StateRecoveryAction,
}

impl StateMigrationData {
    pub fn new(
        migration_id: impl Into<String>,
        state_key: impl Into<String>,
        from_schema_version: impl Into<String>,
        to_schema_version: impl Into<String>,
        payload_sha256: impl Into<String>,
        validation_result: StateValidationResult,
        recovery_action: StateRecoveryAction,
    ) -> Result<Self, SanitizationError> {
        let value = Self {
            migration_id: migration_id.into(),
            state_key: state_key.into(),
            from_schema_version: from_schema_version.into(),
            to_schema_version: to_schema_version.into(),
            payload_sha256: payload_sha256.into(),
            validation_result,
            recovery_action,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), SanitizationError> {
        validate_hash_identity(&self.migration_id, "migration:", "migration_id")?;
        validate_identifier(&self.state_key, "state_key")?;
        validate_version(&self.from_schema_version, "from_schema_version")?;
        validate_version(&self.to_schema_version, "to_schema_version")?;
        validate_sha256(&self.payload_sha256, "migration_payload_sha256")
    }

    pub fn migration_id(&self) -> &str {
        &self.migration_id
    }

    pub fn state_key(&self) -> &str {
        &self.state_key
    }

    pub fn from_schema_version(&self) -> &str {
        &self.from_schema_version
    }

    pub fn to_schema_version(&self) -> &str {
        &self.to_schema_version
    }

    pub fn payload_sha256(&self) -> &str {
        &self.payload_sha256
    }

    pub const fn validation_result(&self) -> StateValidationResult {
        self.validation_result
    }

    pub const fn recovery_action(&self) -> StateRecoveryAction {
        self.recovery_action
    }

    pub fn recovered_for_ledger(&self) -> Self {
        let mut value = self.clone();
        value.validation_result = StateValidationResult::Recovered;
        value.recovery_action = StateRecoveryAction::ReplayedCommitted;
        value
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseTransitionData {
    transition_id: String,
    kind: ReleaseTransitionKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_release_id: Option<String>,
    release_id: String,
    pointer_revision: u64,
    manifest_sha256: String,
    validation_result: StateValidationResult,
    recovery_action: StateRecoveryAction,
}

impl ReleaseTransitionData {
    pub fn new(
        transition_id: impl Into<String>,
        kind: ReleaseTransitionKind,
        previous_release_id: Option<String>,
        release_id: impl Into<String>,
        pointer_revision: u64,
        manifest_sha256: impl Into<String>,
        status: StateTransitionStatus,
    ) -> Result<Self, SanitizationError> {
        let value = Self {
            transition_id: transition_id.into(),
            kind,
            previous_release_id,
            release_id: release_id.into(),
            pointer_revision,
            manifest_sha256: manifest_sha256.into(),
            validation_result: status.validation_result(),
            recovery_action: status.recovery_action(),
        };
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), SanitizationError> {
        validate_hash_identity(
            &self.transition_id,
            "release-transition:",
            "release_transition_id",
        )?;
        if let Some(previous) = &self.previous_release_id {
            validate_hash_identity(previous, "release:", "previous_release_id")?;
        }
        validate_hash_identity(&self.release_id, "release:", "release_id")?;
        if self.pointer_revision == 0
            || self.previous_release_id.as_deref() == Some(self.release_id.as_str())
        {
            return Err(SanitizationError::new(
                "invalid_release_transition",
                "release_transition",
            ));
        }
        validate_sha256(&self.manifest_sha256, "release_manifest_sha256")
    }

    pub fn transition_id(&self) -> &str {
        &self.transition_id
    }

    pub const fn kind(&self) -> ReleaseTransitionKind {
        self.kind
    }

    pub fn previous_release_id(&self) -> Option<&str> {
        self.previous_release_id.as_deref()
    }

    pub fn release_id(&self) -> &str {
        &self.release_id
    }

    pub const fn pointer_revision(&self) -> u64 {
        self.pointer_revision
    }

    pub fn manifest_sha256(&self) -> &str {
        &self.manifest_sha256
    }

    pub const fn validation_result(&self) -> StateValidationResult {
        self.validation_result
    }

    pub const fn recovery_action(&self) -> StateRecoveryAction {
        self.recovery_action
    }

    pub fn recovered_for_ledger(&self) -> Self {
        let mut value = self.clone();
        value.validation_result = StateValidationResult::Recovered;
        value.recovery_action = StateRecoveryAction::ReplayedCommitted;
        value
    }
}

fn release_id_for(value: &RuntimeReleaseSet) -> Result<String, SanitizationError> {
    #[derive(Serialize)]
    struct Identity<'a> {
        schema_version: &'a str,
        runtime_version: &'a str,
        runtime_content_sha256: &'a str,
        ui_version: &'a str,
        ui_content_sha256: &'a str,
        resources: &'a [ReleaseResourceVersion],
    }

    let bytes = serde_json::to_vec(&Identity {
        schema_version: &value.schema_version,
        runtime_version: &value.runtime_version,
        runtime_content_sha256: &value.runtime_content_sha256,
        ui_version: &value.ui_version,
        ui_content_sha256: &value.ui_content_sha256,
        resources: &value.resources,
    })
    .map_err(|_| SanitizationError::new("release_set_encode_failed", "release_set"))?;
    Ok(format!("release:{:x}", Sha256::digest(bytes)))
}

fn validate_identifier(value: &str, field: &'static str) -> Result<(), SanitizationError> {
    if value.is_empty()
        || value.len() > MAX_IDENTIFIER_BYTES
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'.' | b'_' | b':' | b'-')
        })
    {
        return Err(SanitizationError::new("invalid_state_identifier", field));
    }
    Ok(())
}

fn validate_version(value: &str, field: &'static str) -> Result<(), SanitizationError> {
    if value.is_empty()
        || value.len() > MAX_IDENTIFIER_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'+'))
    {
        return Err(SanitizationError::new("invalid_state_version", field));
    }
    Ok(())
}

fn validate_sha256(value: &str, field: &'static str) -> Result<(), SanitizationError> {
    validate_hash_identity(value, "sha256:", field)
}

fn validate_hash_identity(
    value: &str,
    prefix: &'static str,
    field: &'static str,
) -> Result<(), SanitizationError> {
    if value.strip_prefix(prefix).is_none_or(|digest| {
        digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    }) {
        return Err(SanitizationError::new("invalid_state_hash", field));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(marker: char) -> String {
        format!("sha256:{}", marker.to_string().repeat(64))
    }

    #[test]
    fn release_identity_is_canonical_and_tamper_evident() {
        let left = RuntimeReleaseSet::new(
            "1.2.3",
            hash('c'),
            "2.3.4",
            hash('d'),
            vec![
                ReleaseResourceVersion::new("project-b", "b1", hash('b')).expect("resource"),
                ReleaseResourceVersion::new("project-a", "a1", hash('a')).expect("resource"),
            ],
        )
        .expect("release set");
        let right = RuntimeReleaseSet::new(
            "1.2.3",
            hash('c'),
            "2.3.4",
            hash('d'),
            vec![
                ReleaseResourceVersion::new("project-a", "a1", hash('a')).expect("resource"),
                ReleaseResourceVersion::new("project-b", "b1", hash('b')).expect("resource"),
            ],
        )
        .expect("release set");
        assert_eq!(left, right);
        assert_eq!(left.resources()[0].resource_id(), "project-a");

        let rebuilt = RuntimeReleaseSet::new(
            "1.2.3",
            hash('e'),
            "2.3.4",
            hash('d'),
            left.resources().to_vec(),
        )
        .expect("rebuilt release set");
        assert_ne!(left.release_id(), rebuilt.release_id());

        let mut value = serde_json::to_value(&left).expect("release JSON");
        value["runtime_version"] = serde_json::json!("9.9.9");
        let tampered = serde_json::from_value::<RuntimeReleaseSet>(value).expect("typed JSON");
        assert_eq!(
            tampered.validate().expect_err("identity mismatch").code(),
            "release_set_identity_mismatch"
        );
    }

    #[test]
    fn release_rejects_duplicate_or_unbounded_resource_sets() {
        let duplicate =
            ReleaseResourceVersion::new("project-a", "a1", hash('a')).expect("resource");
        assert_eq!(
            RuntimeReleaseSet::new(
                "1.0.0",
                hash('c'),
                "1.0.0",
                hash('d'),
                vec![duplicate.clone(), duplicate],
            )
            .expect_err("duplicate resource")
            .code(),
            "release_resources_not_canonical"
        );
        assert_eq!(
            RuntimeReleaseSet::new("1.0.0", hash('c'), "1.0.0", hash('d'), Vec::new())
                .expect_err("empty release")
                .code(),
            "invalid_release_resource_count"
        );
    }
}
