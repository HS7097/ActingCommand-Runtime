// SPDX-License-Identifier: AGPL-3.0-only

//! Pure capability evidence shared by registered execution backends and Runtime clients.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::str::FromStr;

pub type EmulatorCapabilityResult<T> = Result<T, EmulatorCapabilityError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmulatorCapabilityError {
    message: String,
}

impl EmulatorCapabilityError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for EmulatorCapabilityError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for EmulatorCapabilityError {}

/// Runtime-owned provenance. Simulation evidence never establishes physical support.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionBackendProvenance {
    PhysicalDevice,
    FixtureSimulation,
}

pub const EMULATOR_CAPABILITY_SCHEMA_VERSION: &str = "actingcommand.emulator-capabilities.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmulatorCapability {
    InventoryRead,
    InstanceStatusRead,
    InstanceStart,
    InstanceStop,
    InstanceRestart,
    InstanceCreate,
    InstanceClone,
    InstanceDelete,
    InstanceConfigure,
    ApplicationControl,
    AdbBridge,
    SnapshotManage,
    InputTap,
    InputLongTap,
    InputSwipe,
    InputSegmentedSwipe,
    InputKey,
    InputText,
    InputReset,
    CaptureFrame,
    ApplicationLaunch,
    ApplicationStop,
    ApplicationRestart,
}

impl EmulatorCapability {
    pub const ALL: [Self; 23] = [
        Self::InventoryRead,
        Self::InstanceStatusRead,
        Self::InstanceStart,
        Self::InstanceStop,
        Self::InstanceRestart,
        Self::InstanceCreate,
        Self::InstanceClone,
        Self::InstanceDelete,
        Self::InstanceConfigure,
        Self::ApplicationControl,
        Self::AdbBridge,
        Self::SnapshotManage,
        Self::InputTap,
        Self::InputLongTap,
        Self::InputSwipe,
        Self::InputSegmentedSwipe,
        Self::InputKey,
        Self::InputText,
        Self::InputReset,
        Self::CaptureFrame,
        Self::ApplicationLaunch,
        Self::ApplicationStop,
        Self::ApplicationRestart,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InventoryRead => "inventory.read",
            Self::InstanceStatusRead => "instance.status.read",
            Self::InstanceStart => "instance.start",
            Self::InstanceStop => "instance.stop",
            Self::InstanceRestart => "instance.restart",
            Self::InstanceCreate => "instance.create",
            Self::InstanceClone => "instance.clone",
            Self::InstanceDelete => "instance.delete",
            Self::InstanceConfigure => "instance.configure",
            Self::ApplicationControl => "application.control",
            Self::AdbBridge => "adb.bridge",
            Self::SnapshotManage => "snapshot.manage",
            Self::InputTap => "input.tap",
            Self::InputLongTap => "input.long_tap",
            Self::InputSwipe => "input.swipe",
            Self::InputSegmentedSwipe => "input.segmented_swipe",
            Self::InputKey => "input.key",
            Self::InputText => "input.text",
            Self::InputReset => "input.reset",
            Self::CaptureFrame => "capture.frame",
            Self::ApplicationLaunch => "application.launch",
            Self::ApplicationStop => "application.stop",
            Self::ApplicationRestart => "application.restart",
        }
    }
}

impl FromStr for EmulatorCapability {
    type Err = EmulatorCapabilityError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "inventory.read" => Ok(Self::InventoryRead),
            "instance.status.read" => Ok(Self::InstanceStatusRead),
            "instance.start" => Ok(Self::InstanceStart),
            "instance.stop" => Ok(Self::InstanceStop),
            "instance.restart" => Ok(Self::InstanceRestart),
            "instance.create" => Ok(Self::InstanceCreate),
            "instance.clone" => Ok(Self::InstanceClone),
            "instance.delete" => Ok(Self::InstanceDelete),
            "instance.configure" => Ok(Self::InstanceConfigure),
            "application.control" => Ok(Self::ApplicationControl),
            "adb.bridge" => Ok(Self::AdbBridge),
            "snapshot.manage" => Ok(Self::SnapshotManage),
            "input.tap" => Ok(Self::InputTap),
            "input.long_tap" => Ok(Self::InputLongTap),
            "input.swipe" => Ok(Self::InputSwipe),
            "input.segmented_swipe" => Ok(Self::InputSegmentedSwipe),
            "input.key" => Ok(Self::InputKey),
            "input.text" => Ok(Self::InputText),
            "input.reset" => Ok(Self::InputReset),
            "capture.frame" => Ok(Self::CaptureFrame),
            "application.launch" => Ok(Self::ApplicationLaunch),
            "application.stop" => Ok(Self::ApplicationStop),
            "application.restart" => Ok(Self::ApplicationRestart),
            other => Err(EmulatorCapabilityError::new(format!(
                "unknown emulator capability {other:?} for {EMULATOR_CAPABILITY_SCHEMA_VERSION}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EmulatorVersionEvidence {
    Exact { value: String },
    Minimum { value: String },
    Unavailable { reason: String },
}

impl EmulatorVersionEvidence {
    fn validate(&self) -> EmulatorCapabilityResult<()> {
        match self {
            Self::Exact { value } | Self::Minimum { value } => {
                validate_bounded_text(value, "provider version", 64)
            }
            Self::Unavailable { reason } => {
                validate_bounded_text(reason, "unavailable version reason", 256)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmulatorCapabilityAvailability {
    Available,
    Unavailable,
    Unverified,
}

/// Static implementation knowledge, independent of assets, connection and device availability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmulatorCapabilityImplementation {
    Supported,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EmulatorCapabilityEvidence {
    capability: EmulatorCapability,
    availability: EmulatorCapabilityAvailability,
    failure_semantics: String,
    evidence_ref: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    implementation: Option<EmulatorCapabilityImplementation>,
}

impl EmulatorCapabilityEvidence {
    pub fn new(
        capability: EmulatorCapability,
        availability: EmulatorCapabilityAvailability,
        failure_semantics: impl Into<String>,
        evidence_ref: impl Into<String>,
    ) -> EmulatorCapabilityResult<Self> {
        let evidence = Self {
            capability,
            availability,
            failure_semantics: failure_semantics.into(),
            evidence_ref: evidence_ref.into(),
            implementation: None,
        };
        validate_bounded_text(&evidence.failure_semantics, "failure semantics", 512)?;
        validate_bounded_text(&evidence.evidence_ref, "evidence reference", 512)?;
        Ok(evidence)
    }

    pub fn with_implementation(
        mut self,
        implementation: EmulatorCapabilityImplementation,
    ) -> EmulatorCapabilityResult<Self> {
        if implementation == EmulatorCapabilityImplementation::Unsupported
            && self.availability != EmulatorCapabilityAvailability::Unavailable
        {
            return Err(EmulatorCapabilityError::new(
                "unsupported capability must be unavailable",
            ));
        }
        self.implementation = Some(implementation);
        Ok(self)
    }

    pub const fn implementation(&self) -> Option<EmulatorCapabilityImplementation> {
        self.implementation
    }

    pub const fn capability(&self) -> EmulatorCapability {
        self.capability
    }

    pub const fn availability(&self) -> EmulatorCapabilityAvailability {
        self.availability
    }

    pub fn failure_semantics(&self) -> &str {
        &self.failure_semantics
    }

    pub fn evidence_ref(&self) -> &str {
        &self.evidence_ref
    }
}

impl<'de> Deserialize<'de> for EmulatorCapabilityEvidence {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            capability: EmulatorCapability,
            availability: EmulatorCapabilityAvailability,
            failure_semantics: String,
            evidence_ref: String,
            implementation: Option<EmulatorCapabilityImplementation>,
        }
        let fields = Fields::deserialize(deserializer)?;
        let mut evidence = Self::new(
            fields.capability,
            fields.availability,
            fields.failure_semantics,
            fields.evidence_ref,
        )
        .map_err(serde::de::Error::custom)?;
        if let Some(implementation) = fields.implementation {
            evidence = evidence
                .with_implementation(implementation)
                .map_err(serde::de::Error::custom)?;
        }
        Ok(evidence)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EmulatorCapabilityProfile {
    schema_version: String,
    provider_id: String,
    version: EmulatorVersionEvidence,
    capabilities: BTreeMap<EmulatorCapability, EmulatorCapabilityEvidence>,
}

impl<'de> Deserialize<'de> for EmulatorCapabilityProfile {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            schema_version: String,
            provider_id: String,
            version: EmulatorVersionEvidence,
            #[serde(deserialize_with = "deserialize_capabilities")]
            capabilities: Vec<EmulatorCapabilityEvidence>,
        }
        let fields = Fields::deserialize(deserializer)?;
        if fields.schema_version != EMULATOR_CAPABILITY_SCHEMA_VERSION {
            return Err(serde::de::Error::custom(
                "unknown emulator capability schema",
            ));
        }
        Self::new(fields.provider_id, fields.version, fields.capabilities)
            .map_err(serde::de::Error::custom)
    }
}

fn deserialize_capabilities<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<EmulatorCapabilityEvidence>, D::Error> {
    struct CapabilitiesVisitor;
    impl<'de> serde::de::Visitor<'de> for CapabilitiesVisitor {
        type Value = Vec<EmulatorCapabilityEvidence>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a complete capability map without duplicate or mismatched keys")
        }

        fn visit_map<M: serde::de::MapAccess<'de>>(
            self,
            mut map: M,
        ) -> Result<Self::Value, M::Error> {
            let mut entries = BTreeMap::new();
            while let Some((key, evidence)) =
                map.next_entry::<EmulatorCapability, EmulatorCapabilityEvidence>()?
            {
                if key != evidence.capability() {
                    return Err(serde::de::Error::custom("capability evidence key mismatch"));
                }
                if entries.insert(key, evidence).is_some() {
                    return Err(serde::de::Error::custom(
                        "duplicate emulator capability evidence",
                    ));
                }
            }
            Ok(entries.into_values().collect())
        }
    }
    deserializer.deserialize_map(CapabilitiesVisitor)
}

impl EmulatorCapabilityProfile {
    pub fn new(
        provider_id: impl Into<String>,
        version: EmulatorVersionEvidence,
        evidence: Vec<EmulatorCapabilityEvidence>,
    ) -> EmulatorCapabilityResult<Self> {
        let provider_id = provider_id.into();
        validate_provider_id(&provider_id)?;
        version.validate()?;

        let mut capabilities = BTreeMap::new();
        for entry in evidence {
            let capability = entry.capability();
            if capabilities.insert(capability, entry).is_some() {
                return Err(EmulatorCapabilityError::new(format!(
                    "duplicate emulator capability evidence for {}",
                    capability.as_str()
                )));
            }
        }
        let missing = EmulatorCapability::ALL
            .into_iter()
            .filter(|capability| !capabilities.contains_key(capability))
            .map(EmulatorCapability::as_str)
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(EmulatorCapabilityError::new(format!(
                "emulator capability profile is incomplete; missing {}",
                missing.join(", ")
            )));
        }

        Ok(Self {
            schema_version: EMULATOR_CAPABILITY_SCHEMA_VERSION.to_string(),
            provider_id,
            version,
            capabilities,
        })
    }

    pub fn schema_version(&self) -> &str {
        &self.schema_version
    }

    pub fn provider_id(&self) -> &str {
        &self.provider_id
    }

    pub const fn version(&self) -> &EmulatorVersionEvidence {
        &self.version
    }

    pub fn capability(
        &self,
        capability: EmulatorCapability,
    ) -> EmulatorCapabilityResult<&EmulatorCapabilityEvidence> {
        let evidence = self.capabilities.get(&capability).ok_or_else(|| {
            EmulatorCapabilityError::new(format!(
                "emulator capability profile is missing {}",
                capability.as_str()
            ))
        })?;
        match evidence.availability() {
            EmulatorCapabilityAvailability::Available => Ok(evidence),
            availability => Err(EmulatorCapabilityError::new(format!(
                "emulator provider {} cannot claim {}: {availability:?}; {}",
                self.provider_id,
                capability.as_str(),
                evidence.failure_semantics()
            ))),
        }
    }

    pub fn capability_by_id(
        &self,
        capability_id: &str,
    ) -> EmulatorCapabilityResult<&EmulatorCapabilityEvidence> {
        self.capability(capability_id.parse()?)
    }

    pub fn evidence(&self, capability: EmulatorCapability) -> &EmulatorCapabilityEvidence {
        self.capabilities
            .get(&capability)
            .expect("validated profiles contain every closed capability")
    }

    /// The ids of every capability recorded with `availability`, sorted by id.
    pub fn capability_ids_with(
        &self,
        availability: EmulatorCapabilityAvailability,
    ) -> Vec<&'static str> {
        let mut ids = self
            .capabilities
            .values()
            .filter(|evidence| evidence.availability() == availability)
            .map(|evidence| evidence.capability().as_str())
            .collect::<Vec<_>>();
        ids.sort_unstable();
        ids
    }
}

fn validate_provider_id(value: &str) -> EmulatorCapabilityResult<()> {
    validate_bounded_text(value, "provider id", 64)?;
    if value
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte))
    {
        Ok(())
    } else {
        Err(EmulatorCapabilityError::new(format!(
            "invalid emulator provider id {value:?}; expected lowercase ASCII token"
        )))
    }
}

fn validate_bounded_text(value: &str, field: &str, max_len: usize) -> EmulatorCapabilityResult<()> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(EmulatorCapabilityError::new(format!(
            "{field} must not be empty"
        )));
    }
    if trimmed.len() > max_len {
        return Err(EmulatorCapabilityError::new(format!(
            "{field} exceeds {max_len} bytes"
        )));
    }
    Ok(())
}
