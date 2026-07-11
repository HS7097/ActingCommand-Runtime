// SPDX-License-Identifier: AGPL-3.0-only

use crate::{LabError, LabResult};
use std::path::{Path, PathBuf};

pub(crate) use actingcommand_artifact_store::{
    FrameStoreConfig, FrameStoreFrameInput, FrameStoreOutcome, FrameStoreScreenshot,
    RecognitionState, Tier3PauseCheckpoint,
};
pub use actingcommand_artifact_store::{FrameStoreControl, MemorySample, MemorySampleSource};

pub(crate) struct FrameStore(actingcommand_artifact_store::FrameStore);

impl FrameStore {
    pub(crate) fn new(temp_dir: PathBuf, config: FrameStoreConfig) -> LabResult<Self> {
        actingcommand_artifact_store::FrameStore::new(temp_dir, config)
            .map(Self)
            .map_err(map_error)
    }

    pub(crate) fn set_config(&mut self, config: FrameStoreConfig) -> LabResult<()> {
        self.0.set_config(config).map_err(map_error)
    }

    pub(crate) fn add_frame(
        &mut self,
        input: FrameStoreFrameInput,
    ) -> LabResult<FrameStoreOutcome> {
        self.0.add_frame(input).map_err(map_error)
    }

    pub(crate) fn materialize(&mut self, screenshots_dir: &Path) -> LabResult<()> {
        self.0.materialize(screenshots_dir).map_err(map_error)
    }

    pub(crate) fn cleanup_temp(&mut self) -> Vec<String> {
        self.0.cleanup_temp()
    }

    pub(crate) fn screenshots(&self) -> Vec<FrameStoreScreenshot> {
        self.0.screenshots()
    }

    pub(crate) fn diagnostics_json(&self) -> serde_json::Value {
        self.0.diagnostics_json()
    }

    pub(crate) fn timeline(&self) -> Vec<serde_json::Value> {
        self.0.timeline()
    }
}

fn map_error(error: actingcommand_artifact_store::ArtifactStoreError) -> LabError {
    match error.code() {
        "frame_store_usage" => LabError::usage(error.detail()),
        "frame_store_device" => LabError::device(error.detail()),
        _ => LabError::package_invalid(error.detail()),
    }
}
