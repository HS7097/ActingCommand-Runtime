// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

pub const PROVIDER_PAYLOAD_SCHEMA: &str = "actingcommand.payload.provider.v1";
pub const MAX_PROVIDER_STARTUP_TEXT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderBackend {
    Configured,
    FastdeployPpocr,
    Onnxruntime,
    MumuManager,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderStartupStage {
    ManifestRead,
    ManifestParse,
    PathBinding,
    ModelIdentity,
    BackendConstruction,
    RegistryBinding,
    InstanceDiscovery,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderNativeFailure {
    pub module: String,
    pub code: String,
    pub severity: String,
    pub message: String,
}

/// One instance reported by `MuMuManager info -v all`, with the alias it was bound to, if any.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveredInstanceObservation {
    pub instance_index: u16,
    pub instance_name: String,
    pub adb_host: String,
    pub adb_port: u16,
    pub running: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound_alias: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProviderStartupObservation {
    Started {
        stage: ProviderStartupStage,
    },
    Completed {
        stage: ProviderStartupStage,
    },
    Binding {
        field: String,
        configured: String,
        base: String,
        resolved: String,
    },
    ModelBinding {
        model_ref: String,
        model_sha256: String,
    },
    Failed {
        stage: ProviderStartupStage,
        failure: ProviderNativeFailure,
    },
    /// One successful `MuMuManager` discovery run per startup (Workflow #316).
    InstanceDiscovery {
        source: String,
        mumu_manager_path: String,
        version: String,
        instances: Vec<DiscoveredInstanceObservation>,
    },
    NotConfigured,
    Ready,
}

/// Constructor facts only. Inference and lazy model initialization are not performed here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderStartupRecord {
    pub owner_epoch: OwnerEpoch,
    pub backend: ProviderBackend,
    pub observation: ProviderStartupObservation,
}

impl ProviderStartupRecord {
    pub fn validate(&self) -> Result<(), SanitizationError> {
        let valid =
            |value: &str| !value.is_empty() && value.len() <= MAX_PROVIDER_STARTUP_TEXT_BYTES;
        let valid = match &self.observation {
            ProviderStartupObservation::Binding {
                field,
                configured,
                base,
                resolved,
            } => {
                base.len() <= MAX_PROVIDER_STARTUP_TEXT_BYTES
                    && [field, configured, resolved]
                        .into_iter()
                        .all(|value| valid(value))
            }
            ProviderStartupObservation::Failed { failure, .. } => [
                &failure.module,
                &failure.code,
                &failure.severity,
                &failure.message,
            ]
            .into_iter()
            .all(|value| valid(value)),
            ProviderStartupObservation::ModelBinding {
                model_ref,
                model_sha256,
            } => {
                valid(model_ref)
                    && model_sha256.len() == 64
                    && model_sha256
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
            }
            ProviderStartupObservation::InstanceDiscovery {
                source,
                mumu_manager_path,
                version,
                instances,
            } => {
                [source, mumu_manager_path, version]
                    .into_iter()
                    .all(|value| valid(value))
                    && instances.iter().all(|instance| {
                        valid(&instance.instance_name)
                            && valid(&instance.adb_host)
                            && instance.adb_port != 0
                            && instance.bound_alias.as_deref().is_none_or(&valid)
                    })
            }
            _ => true,
        };
        if !valid {
            return Err(SanitizationError::new(
                "invalid_provider_startup_record",
                "provider_payload",
            ));
        }
        Ok(())
    }
}

pub struct ProviderPayloadDraft(ProviderStartupRecord);

impl ProviderPayloadDraft {
    pub fn observed(record: ProviderStartupRecord) -> Self {
        Self(record)
    }

    pub(super) fn sanitize(
        self,
        fingerprinter: &dyn SecretFingerprinter,
    ) -> Result<ProviderPayload, SanitizationError> {
        self.0.validate()?;
        Ok(ProviderPayload {
            record: self.0,
            audit: AuditInput::new().sanitize(fingerprinter)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderPayload {
    pub record: ProviderStartupRecord,
    audit: SanitizedAudit,
}

impl PayloadDetail for ProviderPayload {
    fn action(&self) -> EventAction {
        EventAction::ProviderStartup
    }
    fn diagnostic_code(&self) -> Option<DiagnosticCode> {
        matches!(
            self.record.observation,
            ProviderStartupObservation::Failed { .. }
        )
        .then_some(DiagnosticCode::ProviderStartupFailed)
    }
    fn effect_disposition(&self) -> Option<EffectDisposition> {
        None
    }
    fn audit(&self) -> &SanitizedAudit {
        &self.audit
    }
}

impl FamilyPayload for ProviderPayload {
    fn event_type(&self) -> EventType {
        EventType::ProviderStartupObserved
    }
    fn detail(&self) -> &dyn PayloadDetail {
        self
    }
}
