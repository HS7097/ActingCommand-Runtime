// SPDX-License-Identifier: AGPL-3.0-only

//! Typed client-action and approval records accepted by Runtime IPC.

use crate::SanitizationError;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::Path;

const MAX_IDENTIFIER_BYTES: usize = 128;
const MAX_PATH_SAFE_VALUE_BYTES: usize = 256;
const MAX_REDACTED_INPUT_BYTES: u32 = 4_096;

/// Classifies the client-side interaction without persisting arbitrary UI text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientActionKind {
    Button,
    Input,
    Command,
}

/// Closed value surface for client actions; arbitrary text must be redacted before IPC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ClientActionValue {
    Boolean(bool),
    Integer(i64),
    PathSafeString(String),
    Redacted { sha256: String, byte_count: u32 },
}

/// One auditable button, input, or command submitted through a typed client boundary.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientActionRecord {
    surface_id: String,
    control_id: String,
    kind: ClientActionKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    instance_alias: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<ClientActionValue>,
}

impl ClientActionRecord {
    pub fn new(
        surface_id: impl Into<String>,
        control_id: impl Into<String>,
        kind: ClientActionKind,
        instance_alias: Option<String>,
        value: Option<ClientActionValue>,
    ) -> Result<Self, SanitizationError> {
        let record = Self {
            surface_id: surface_id.into(),
            control_id: control_id.into(),
            kind,
            instance_alias,
            value,
        };
        record.validate()?;
        Ok(record)
    }

    pub fn validate(&self) -> Result<(), SanitizationError> {
        validate_identifier(&self.surface_id, "surface_id")?;
        validate_identifier(&self.control_id, "control_id")?;
        if let Some(instance_alias) = &self.instance_alias {
            validate_identifier(instance_alias, "instance_alias")?;
        }
        match (&self.kind, &self.value) {
            (ClientActionKind::Button, None) | (ClientActionKind::Command, None) => Ok(()),
            (ClientActionKind::Input | ClientActionKind::Command, Some(value)) => value.validate(),
            _ => Err(SanitizationError::new(
                "invalid_client_action_value",
                "client_action",
            )),
        }
    }

    pub fn surface_id(&self) -> &str {
        &self.surface_id
    }

    pub fn control_id(&self) -> &str {
        &self.control_id
    }

    pub const fn kind(&self) -> ClientActionKind {
        self.kind
    }

    pub fn instance_alias(&self) -> Option<&str> {
        self.instance_alias.as_deref()
    }

    pub const fn value(&self) -> Option<&ClientActionValue> {
        self.value.as_ref()
    }
}

impl fmt::Debug for ClientActionRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClientActionRecord")
            .field("surface_id", &self.surface_id)
            .field("control_id", &self.control_id)
            .field("kind", &self.kind)
            .field("instance_alias", &self.instance_alias)
            .field("value", &self.value.as_ref().map(|_| "<typed-redacted>"))
            .finish()
    }
}

impl ClientActionValue {
    fn validate(&self) -> Result<(), SanitizationError> {
        match self {
            Self::PathSafeString(value) if path_safe_string(value) => Ok(()),
            Self::Redacted { sha256, byte_count }
                if *byte_count > 0
                    && *byte_count <= MAX_REDACTED_INPUT_BYTES
                    && canonical_sha256(sha256) =>
            {
                Ok(())
            }
            Self::Boolean(_) | Self::Integer(_) => Ok(()),
            _ => Err(SanitizationError::new(
                "invalid_client_action_value",
                "client_action_value",
            )),
        }
    }
}

/// The authorization effect represented by one approval fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDisposition {
    Approved,
    Rejected,
    Pinned,
    Revoked,
}

impl ApprovalDisposition {
    pub const fn grants_authority(self) -> bool {
        matches!(self, Self::Approved | Self::Pinned)
    }
}

/// Closed target category used by public approval projections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalTargetKind {
    Catalog,
    Plan,
    Decision,
}

/// Immutable object identity to which an approval fact is bound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ApprovalTarget {
    Catalog {
        catalog_hash: String,
        catalog_version: u64,
    },
    Plan {
        plan_id: String,
        catalog_hash: String,
        catalog_version: u64,
    },
    Decision {
        decision_id: String,
        catalog_hash: String,
        catalog_version: u64,
    },
}

impl ApprovalTarget {
    pub const fn kind(&self) -> ApprovalTargetKind {
        match self {
            Self::Catalog { .. } => ApprovalTargetKind::Catalog,
            Self::Plan { .. } => ApprovalTargetKind::Plan,
            Self::Decision { .. } => ApprovalTargetKind::Decision,
        }
    }

    pub fn validate(&self) -> Result<(), SanitizationError> {
        let (catalog_hash, catalog_version) = match self {
            Self::Catalog {
                catalog_hash,
                catalog_version,
            } => (catalog_hash, catalog_version),
            Self::Plan {
                plan_id,
                catalog_hash,
                catalog_version,
            } => {
                validate_identifier(plan_id, "plan_id")?;
                (catalog_hash, catalog_version)
            }
            Self::Decision {
                decision_id,
                catalog_hash,
                catalog_version,
            } => {
                validate_identifier(decision_id, "decision_id")?;
                (catalog_hash, catalog_version)
            }
        };
        if *catalog_version == 0 || !canonical_sha256(catalog_hash) {
            return Err(SanitizationError::new(
                "invalid_approval_target",
                "approval_target",
            ));
        }
        Ok(())
    }

    pub fn catalog_hash(&self) -> &str {
        match self {
            Self::Catalog { catalog_hash, .. }
            | Self::Plan { catalog_hash, .. }
            | Self::Decision { catalog_hash, .. } => catalog_hash,
        }
    }

    pub const fn catalog_version(&self) -> u64 {
        match self {
            Self::Catalog {
                catalog_version, ..
            }
            | Self::Plan {
                catalog_version, ..
            }
            | Self::Decision {
                catalog_version, ..
            } => *catalog_version,
        }
    }
}

/// One typed approval, rejection, pin, or revocation recorded by Runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalDecisionRecord {
    approval_id: String,
    disposition: ApprovalDisposition,
    target: ApprovalTarget,
    reason_code: String,
}

impl ApprovalDecisionRecord {
    pub fn new(
        approval_id: impl Into<String>,
        disposition: ApprovalDisposition,
        target: ApprovalTarget,
        reason_code: impl Into<String>,
    ) -> Result<Self, SanitizationError> {
        let record = Self {
            approval_id: approval_id.into(),
            disposition,
            target,
            reason_code: reason_code.into(),
        };
        record.validate()?;
        Ok(record)
    }

    pub fn validate(&self) -> Result<(), SanitizationError> {
        if !self.approval_id.starts_with("approval:") {
            return Err(SanitizationError::new("invalid_approval_id", "approval_id"));
        }
        validate_identifier(&self.approval_id, "approval_id")?;
        validate_identifier(&self.reason_code, "reason_code")?;
        self.target.validate()
    }

    pub fn approval_id(&self) -> &str {
        &self.approval_id
    }

    pub const fn disposition(&self) -> ApprovalDisposition {
        self.disposition
    }

    pub const fn target(&self) -> &ApprovalTarget {
        &self.target
    }

    pub fn reason_code(&self) -> &str {
        &self.reason_code
    }
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
        return Err(SanitizationError::new(
            "invalid_interaction_identifier",
            field,
        ));
    }
    Ok(())
}

fn path_safe_string(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_PATH_SAFE_VALUE_BYTES
        && value != "."
        && !value.contains('/')
        && !value.contains('\\')
        && !value.contains(':')
        && !value.contains("..")
        && !value.chars().any(char::is_control)
        && !Path::new(value).is_absolute()
}

fn canonical_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash() -> String {
        format!("sha256:{}", "a".repeat(64))
    }

    #[test]
    fn client_action_rejects_raw_or_path_unsafe_text() {
        let error = ClientActionRecord::new(
            "settings",
            "profile_name",
            ClientActionKind::Input,
            None,
            Some(ClientActionValue::PathSafeString("../secret".to_owned())),
        )
        .expect_err("path traversal must not enter the ledger");
        assert_eq!(error.code(), "invalid_client_action_value");

        ClientActionRecord::new(
            "settings",
            "profile_name",
            ClientActionKind::Input,
            None,
            Some(ClientActionValue::Redacted {
                sha256: hash(),
                byte_count: 12,
            }),
        )
        .expect("redacted input");
    }

    #[test]
    fn approval_is_bound_to_a_typed_target() {
        let record = ApprovalDecisionRecord::new(
            "approval:fixture-a",
            ApprovalDisposition::Approved,
            ApprovalTarget::Catalog {
                catalog_hash: hash(),
                catalog_version: 1,
            },
            "user_confirmed",
        )
        .expect("approval");
        assert!(record.disposition().grants_authority());
        for target in [
            ApprovalTarget::Plan {
                plan_id: "plan:fixture-a".to_owned(),
                catalog_hash: hash(),
                catalog_version: 1,
            },
            ApprovalTarget::Decision {
                decision_id: "decision:fixture-a".to_owned(),
                catalog_hash: hash(),
                catalog_version: 1,
            },
        ] {
            ApprovalDecisionRecord::new(
                "approval:fixture-b",
                ApprovalDisposition::Rejected,
                target,
                "user_rejected",
            )
            .expect("typed approval target");
        }
        assert!(!ApprovalDisposition::Rejected.grants_authority());
        assert!(!ApprovalDisposition::Revoked.grants_authority());

        let error = ApprovalDecisionRecord::new(
            "fixture-a",
            ApprovalDisposition::Approved,
            ApprovalTarget::Catalog {
                catalog_hash: hash(),
                catalog_version: 1,
            },
            "user_confirmed",
        )
        .expect_err("approval prefix is required");
        assert_eq!(error.code(), "invalid_approval_id");
    }
}
