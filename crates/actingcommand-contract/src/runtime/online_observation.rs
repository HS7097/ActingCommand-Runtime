// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::codes::RuntimeContractCode;
use crate::page_projection::PageProjection;

pub const ONLINE_OBSERVATION_SCHEMA: &str = "actingcommand.runtime.contained-page-observation.v1";
pub const MAX_OBSERVATION_FACT_ITEMS: usize = 256;
pub const MAX_OBSERVATION_FACT_BYTES: usize = 64 * 1024;
pub const MAX_OBSERVATION_ARTIFACT_BYTES: usize = 256 * 1024;
pub const MAX_OBSERVATION_TARGETS: usize = 64;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContainedObservationRequest {
    package_path: String,
    targets: Vec<String>,
}

impl fmt::Debug for ContainedObservationRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ContainedObservationRequest(<contained-resource>)")
    }
}

impl ContainedObservationRequest {
    pub fn new(
        package_path: impl Into<String>,
        targets: Vec<String>,
    ) -> RuntimeContractResult<Self> {
        let request = Self {
            package_path: package_path.into(),
            targets,
        };
        request.validate()?;
        Ok(request)
    }
    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.package_path.trim().is_empty()
            || self.package_path.len() > MAX_CONTAINED_TASK_PATH_BYTES
            || self.package_path.contains('\0')
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidObservationPackagePath,
            ));
        }
        if self.targets.len() > MAX_OBSERVATION_TARGETS
            || self.targets.iter().enumerate().any(|(index, target)| {
                target.is_empty()
                    || target.len() > 256
                    || target.contains('\0')
                    || self.targets[..index].contains(target)
            })
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidObservationTargets,
            ));
        }
        Ok(())
    }
    pub fn package_path(&self) -> &str {
        &self.package_path
    }
    pub fn targets(&self) -> &[String] {
        &self.targets
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageObservationStatus {
    Recognized,
    NoMatch,
    Conflict,
    Partial,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationFacts {
    pub rows: Vec<serde_json::Value>,
    pub item_count: usize,
    pub omitted_count: usize,
    pub target_evaluation_count: usize,
    pub omitted_target_evaluation_count: usize,
    pub truncated: bool,
}

impl ObservationFacts {
    pub fn push(
        &mut self,
        row: serde_json::Value,
        target_count: usize,
    ) -> RuntimeContractResult<()> {
        self.item_count += 1;
        self.target_evaluation_count += target_count;
        if self.rows.len() < MAX_OBSERVATION_FACT_ITEMS {
            self.rows.push(row);
            if serde_json::to_vec(self)
                .map_err(|_| {
                    RuntimeContractError::new(RuntimeContractCode::ObservationFactEncodeFailed)
                })?
                .len()
                <= MAX_OBSERVATION_FACT_BYTES - 256
            {
                return Ok(());
            }
            self.rows.pop();
        }
        self.omitted_count += 1;
        self.omitted_target_evaluation_count += target_count;
        self.truncated = true;
        Ok(())
    }
    /// #411 A21: counts one row of `target_count` actual target evaluations as omitted without
    /// trying it, as [`Self::push`] counts a row that does not fit.
    pub fn omit(&mut self, target_count: usize) {
        self.item_count += 1;
        self.target_evaluation_count += target_count;
        self.omitted_count += 1;
        self.omitted_target_evaluation_count += target_count;
        self.truncated = true;
    }
    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.rows.len() > MAX_OBSERVATION_FACT_ITEMS
            || self.rows.len().checked_add(self.omitted_count) != Some(self.item_count)
            || self.omitted_target_evaluation_count > self.target_evaluation_count
            || self.truncated != (self.omitted_count != 0)
            || serde_json::to_vec(self)
                .map_err(|_| {
                    RuntimeContractError::new(RuntimeContractCode::ObservationFactEncodeFailed)
                })?
                .len()
                > MAX_OBSERVATION_FACT_BYTES
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidObservationFacts,
            ));
        }
        Ok(())
    }
}

/// Stored once, before its outer ArtifactStore reference and verified sequence exist.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContainedObservationEvidence {
    pub schema_version: String,
    pub request_id: RequestId,
    pub correlation_id: CorrelationId,
    pub instance_id: InstanceId,
    /// Workflow #336 PKG-S2 (option 丙): the admitted package's `package_id`, absent when its
    /// control declares none; no package digest is recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_label: Option<String>,
    pub frame: ReadonlyObservation,
    pub rgb8_sha256: String,
    pub status: PageObservationStatus,
    pub projection: PageProjection,
    pub facts: ObservationFacts,
    pub private_facts: ObservationFacts,
}

impl fmt::Debug for ContainedObservationEvidence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ContainedObservationEvidence(<controlled-artifact>)")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContainedPageObservation {
    pub instance_id: InstanceId,
    /// Workflow #336 PKG-S2 (option 丙): the admitted package's `package_id`, absent when its
    /// control declares none; no package digest is recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_label: Option<String>,
    pub frame: ReadonlyObservation,
    pub status: PageObservationStatus,
    pub projection: PageProjection,
    pub facts: ObservationFacts,
    pub artifact: ProjectedArtifactReference,
    pub projection_sequence: u64,
    pub projection_event_id: EventId,
}

impl ContainedPageObservation {
    pub fn validate(&self) -> RuntimeContractResult<()> {
        self.frame.validate()?;
        self.artifact.validate().map_err(|_| {
            RuntimeContractError::new(RuntimeContractCode::InvalidPageObservationArtifact)
        })?;
        self.facts.validate()?;
        if self
            .package_label
            .as_deref()
            .is_some_and(|label| !is_package_label(label))
            || self.artifact.kind != ArtifactKind::DiagnosticJson
            || self.artifact.byte_count > MAX_OBSERVATION_ARTIFACT_BYTES as u64
            || self.artifact.frame_id != self.frame.artifact().frame_id
            || self.projection_sequence == 0
            || self.projection.frame.kind != crate::page_projection::FrameKind::Artifact
            || self.frame.artifact().sha256.strip_prefix("sha256:")
                != Some(self.projection.frame.sha256.as_str())
            || self.projection.frame.width != self.frame.width()
            || self.projection.frame.height != self.frame.height()
            || (self.status == PageObservationStatus::Recognized) != self.projection.matched
            || self.projection.state
                != match self.status {
                    PageObservationStatus::Recognized => "recognized",
                    PageObservationStatus::NoMatch => "unknown",
                    PageObservationStatus::Conflict => "conflict",
                    PageObservationStatus::Partial => "partial",
                }
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidContainedPageObservation,
            ));
        }
        self.projection.clone().verify_transport().map_err(|_| {
            RuntimeContractError::new(RuntimeContractCode::InvalidPageProjectionContent)
        })
    }
}
