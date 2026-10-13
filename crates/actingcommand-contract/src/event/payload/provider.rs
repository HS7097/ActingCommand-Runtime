// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::codes::SanitizationCode;
use crate::emulator::EmulatorCapability;
use std::collections::BTreeSet;

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
    CapabilityAdmission,
}

/// A provider startup failure (ledger revision 3, model section 8.1): `outcome` is the failure
/// with its native text as a `foreign_*` link, and `module` names the module that reported
/// it. The category replaces the former `severity`; no Runtime text is kept. `module` stays a
/// member: of the modules it names (`actingd.config`, `execution-kernel`, the device's
/// `mumu_manager`, the host's `provider` and `emulator_control`) only the host's provider has
/// an `origin_module` token (acceptance CL-M3, L5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderNativeFailure {
    pub module: String,
    pub outcome: Outcome,
}

/// One instance reported by `MuMuManager info -v all`, with the alias it was bound to, if any.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveredInstanceObservation {
    pub instance_index: u16,
    pub instance_name: String,
    /// Omitted, like `adb_port`, when the provider reported no ADB endpoint (a stopped
    /// instance).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adb_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adb_port: Option<u16>,
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
    /// A folder under the vision root's `models` that breaks the folder rule (ledger revision
    /// 3, Workflow #378 A3): `outcome` is `vision_model_folder_invalid` with the folder's
    /// `path` and, when it fits the name rule, its `model_name`. Startup goes on; only the
    /// requests that name the folder fail. The specific reason is `check-config`'s report only.
    ModelFolderInvalid {
        outcome: Outcome,
    },
    /// One successful `MuMuManager` discovery run per startup (Workflow #316).
    InstanceDiscovery {
        source: String,
        mumu_manager_path: String,
        version: String,
        instances: Vec<DiscoveredInstanceObservation>,
    },
    /// The admitted provider capability profile: every closed capability id listed exactly
    /// once under its availability, each list sorted (Workflow #316 A).
    CapabilityProfile {
        provider_id: String,
        version: String,
        available: Vec<String>,
        unverified: Vec<String>,
        unavailable: Vec<String>,
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
            ProviderStartupObservation::Failed { failure, .. } => valid(&failure.module),
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
                            && instance.adb_host.as_deref().is_none_or(valid)
                            && instance.adb_port != Some(0)
                            && instance.bound_alias.as_deref().is_none_or(valid)
                    })
            }
            ProviderStartupObservation::CapabilityProfile {
                provider_id,
                version,
                available,
                unverified,
                unavailable,
            } => {
                let sorted = |ids: &[String]| {
                    ids.iter().all(|id| valid(id)) && ids.windows(2).all(|pair| pair[0] < pair[1])
                };
                let listed = [available, unverified, unavailable]
                    .into_iter()
                    .flatten()
                    .map(String::as_str)
                    .collect::<BTreeSet<_>>();
                valid(provider_id)
                    && valid(version)
                    && sorted(available)
                    && sorted(unverified)
                    && sorted(unavailable)
                    && available.len() + unverified.len() + unavailable.len()
                        == EmulatorCapability::ALL.len()
                    && EmulatorCapability::ALL
                        .iter()
                        .all(|capability| listed.contains(capability.as_str()))
            }
            _ => true,
        };
        if !valid {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidProviderStartupRecord,
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
    fn outcome(&self) -> Option<&Outcome> {
        match &self.record.observation {
            ProviderStartupObservation::Failed { failure, .. } => Some(&failure.outcome),
            ProviderStartupObservation::ModelFolderInvalid { outcome } => Some(outcome),
            _ => None,
        }
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
