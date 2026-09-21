// SPDX-License-Identifier: AGPL-3.0-only

use crate::{ArtifactStore, ArtifactStoreError as CliError, ArtifactStoreResult as CliOutcome};
use actingcommand_contract::{ArtifactReference, CapturePressureState, PinnedFrameReason};
use actingcommand_device::{Frame, PixelFormat};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

const DEFAULT_SIMILARITY_THRESHOLD: f32 = 0.95;
const DEFAULT_TIER1_RATIO: f64 = 0.60;
const DEFAULT_TIER2_RATIO: f64 = 0.75;
const DEFAULT_TIER3_RATIO: f64 = 0.90;
const DEFAULT_HYSTERESIS_RATIO: f64 = 0.10;
const DEFAULT_OS_RESERVE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const DEFAULT_FLUSH_WORKSPACE_RESERVE_BYTES: u64 = 8 * 1024 * 1024;
const ENTRY_BASE_METADATA_BYTES: u64 = 512;
const WRITER_BUFFER_BYTES: u64 = 64 * 1024;
const THUMB_WIDTH: usize = 16;
const THUMB_HEIGHT: usize = 9;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct FrameStoreControl {
    #[serde(default)]
    pub similarity_threshold: Option<f32>,
    #[serde(default)]
    pub tier1_ratio: Option<f64>,
    #[serde(default)]
    pub tier2_ratio: Option<f64>,
    #[serde(default)]
    pub tier3_ratio: Option<f64>,
    #[serde(default)]
    pub hysteresis_ratio: Option<f64>,
    #[serde(default)]
    pub max_mem_bytes: Option<u64>,
    #[serde(default)]
    pub os_reserve_bytes: Option<u64>,
    #[serde(default)]
    pub flush_workspace_reserve_bytes: Option<u64>,
}

impl FrameStoreControl {
    pub fn validate(&self) -> Result<(), String> {
        if let Some(value) = self.similarity_threshold {
            validate_ratio_f32("frame_store.similarity_threshold", value)?;
        }
        for (name, value) in [
            ("frame_store.tier1_ratio", self.tier1_ratio),
            ("frame_store.tier2_ratio", self.tier2_ratio),
            ("frame_store.tier3_ratio", self.tier3_ratio),
            ("frame_store.hysteresis_ratio", self.hysteresis_ratio),
        ] {
            if let Some(value) = value {
                validate_ratio_f64(name, value)?;
            }
        }
        if self.max_mem_bytes == Some(0) {
            return Err("frame_store.max_mem_bytes must be positive when provided".to_string());
        }
        if self.flush_workspace_reserve_bytes == Some(0) {
            return Err(
                "frame_store.flush_workspace_reserve_bytes must be positive when provided"
                    .to_string(),
            );
        }
        Ok(())
    }

    pub fn apply_to(&self, config: &mut FrameStoreConfig) {
        if let Some(value) = self.similarity_threshold {
            config.similarity_threshold = value;
        }
        if let Some(value) = self.tier1_ratio {
            config.tier1_ratio = value;
        }
        if let Some(value) = self.tier2_ratio {
            config.tier2_ratio = value;
        }
        if let Some(value) = self.tier3_ratio {
            config.tier3_ratio = value;
        }
        if let Some(value) = self.hysteresis_ratio {
            config.hysteresis_ratio = value;
        }
        if let Some(value) = self.max_mem_bytes {
            config.max_mem_bytes = Some(value);
        }
        if let Some(value) = self.os_reserve_bytes {
            config.os_reserve_bytes = value;
        }
        if let Some(value) = self.flush_workspace_reserve_bytes {
            config.flush_workspace_reserve_bytes = value;
        }
    }
}

#[derive(Debug, Clone)]
pub struct FrameStoreConfig {
    pub similarity_threshold: f32,
    pub tier1_ratio: f64,
    pub tier2_ratio: f64,
    pub tier3_ratio: f64,
    pub hysteresis_ratio: f64,
    pub max_mem_bytes: Option<u64>,
    pub os_reserve_bytes: u64,
    pub flush_workspace_reserve_bytes: u64,
    memory_source: Option<MemorySampleSource>,
}

impl Default for FrameStoreConfig {
    fn default() -> Self {
        Self {
            similarity_threshold: DEFAULT_SIMILARITY_THRESHOLD,
            tier1_ratio: DEFAULT_TIER1_RATIO,
            tier2_ratio: DEFAULT_TIER2_RATIO,
            tier3_ratio: DEFAULT_TIER3_RATIO,
            hysteresis_ratio: DEFAULT_HYSTERESIS_RATIO,
            max_mem_bytes: None,
            os_reserve_bytes: DEFAULT_OS_RESERVE_BYTES,
            flush_workspace_reserve_bytes: DEFAULT_FLUSH_WORKSPACE_RESERVE_BYTES,
            memory_source: None,
        }
    }
}

impl FrameStoreConfig {
    pub fn validate(&self) -> Result<(), String> {
        validate_ratio_f32("similarity_threshold", self.similarity_threshold)?;
        validate_ratio_f64("tier1_ratio", self.tier1_ratio)?;
        validate_ratio_f64("tier2_ratio", self.tier2_ratio)?;
        validate_ratio_f64("tier3_ratio", self.tier3_ratio)?;
        validate_ratio_f64("hysteresis_ratio", self.hysteresis_ratio)?;
        if !(self.tier1_ratio < self.tier2_ratio && self.tier2_ratio < self.tier3_ratio) {
            return Err("frame store watermarks must satisfy tier1 < tier2 < tier3".to_string());
        }
        if self.max_mem_bytes == Some(0) {
            return Err("max_mem_bytes must be positive when provided".to_string());
        }
        if self.flush_workspace_reserve_bytes == 0 {
            return Err("flush_workspace_reserve_bytes must be positive".to_string());
        }
        Ok(())
    }

    pub fn with_memory_source(mut self, source: MemorySampleSource) -> Self {
        self.memory_source = Some(source);
        self
    }

    #[cfg(test)]
    fn with_memory_sample(self, sample: MemorySample) -> Self {
        self.with_memory_source(MemorySampleSource::fixed(sample))
    }

    fn memory_sample(&self) -> CliOutcome<MemorySample> {
        self.memory_source
            .ok_or_else(|| {
                CliError::device("frame store memory source was not supplied by the app adapter")
            })?
            .sample()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct MemorySample {
    pub total_bytes: u64,
    pub available_bytes: u64,
}

/// Keeps platform sampling in the app while preserving per-frame budget refreshes in the
/// production artifact pipeline.
#[derive(Debug, Clone, Copy)]
pub enum MemorySampleSource {
    Fixed(MemorySample),
    Live(fn() -> CliOutcome<MemorySample>),
}

impl MemorySampleSource {
    pub fn fixed(sample: MemorySample) -> Self {
        Self::Fixed(sample)
    }

    pub fn live(sample: fn() -> CliOutcome<MemorySample>) -> Self {
        Self::Live(sample)
    }

    fn sample(self) -> CliOutcome<MemorySample> {
        match self {
            Self::Fixed(sample) => Ok(sample),
            Self::Live(sample) => sample(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct MemoryBudget {
    total_bytes: u64,
    available_bytes: u64,
    os_reserve_bytes: u64,
    budget_bytes: u64,
    tier1_bytes: u64,
    tier2_bytes: u64,
    tier3_bytes: u64,
    tier1_release_bytes: u64,
    tier2_release_bytes: u64,
    tier3_release_bytes: u64,
    flush_workspace_reserve_bytes: u64,
}

impl MemoryBudget {
    fn build(config: &FrameStoreConfig) -> CliOutcome<Self> {
        config
            .validate()
            .map_err(|err| CliError::usage(format!("invalid frame store config: {err}")))?;
        let budget = Self::from_config(config, config.memory_sample()?);
        budget.validate()?;
        Ok(budget)
    }

    fn from_config(config: &FrameStoreConfig, sample: MemorySample) -> Self {
        let available_after_reserve = sample
            .available_bytes
            .saturating_sub(config.os_reserve_bytes);
        let total_after_reserve = sample.total_bytes.saturating_sub(config.os_reserve_bytes);
        let requested = config.max_mem_bytes.unwrap_or(available_after_reserve);
        let budget_bytes = requested
            .min(available_after_reserve)
            .min(total_after_reserve);
        let tier1_bytes = ratio_bytes(budget_bytes, config.tier1_ratio);
        let tier2_bytes = ratio_bytes(budget_bytes, config.tier2_ratio);
        let tier3_bytes = ratio_bytes(budget_bytes, config.tier3_ratio);
        let release_ratio = 1.0 - config.hysteresis_ratio;
        Self {
            total_bytes: sample.total_bytes,
            available_bytes: sample.available_bytes,
            os_reserve_bytes: config.os_reserve_bytes,
            budget_bytes,
            tier1_bytes,
            tier2_bytes,
            tier3_bytes,
            tier1_release_bytes: ratio_bytes(tier1_bytes, release_ratio),
            tier2_release_bytes: ratio_bytes(tier2_bytes, release_ratio),
            tier3_release_bytes: ratio_bytes(tier3_bytes, release_ratio),
            flush_workspace_reserve_bytes: config.flush_workspace_reserve_bytes,
        }
    }

    fn validate(self) -> CliOutcome<()> {
        if self.budget_bytes == 0 {
            return Err(CliError::usage(
                "frame store memory budget is zero after OS reserve",
            ));
        }
        if !(self.tier1_bytes < self.tier2_bytes && self.tier2_bytes < self.tier3_bytes) {
            return Err(CliError::usage(format!(
                "frame store watermarks must be byte-distinct, got tier1={}, tier2={}, tier3={}",
                self.tier1_bytes, self.tier2_bytes, self.tier3_bytes
            )));
        }
        if self.tier1_release_bytes >= self.tier1_bytes
            || self.tier2_release_bytes >= self.tier2_bytes
            || self.tier3_release_bytes >= self.tier3_bytes
        {
            return Err(CliError::usage(
                "frame store release lines must be below activation lines",
            ));
        }
        if self.tier3_bytes.saturating_sub(self.tier2_bytes) < self.flush_workspace_reserve_bytes {
            return Err(CliError::usage(format!(
                "tier2/tier3 gap too small: gap={} bytes, required flush workspace reserve={} bytes",
                self.tier3_bytes.saturating_sub(self.tier2_bytes),
                self.flush_workspace_reserve_bytes
            )));
        }
        Ok(())
    }

    fn to_json(self) -> Value {
        json!({
            "total_bytes": self.total_bytes,
            "available_bytes": self.available_bytes,
            "os_reserve_bytes": self.os_reserve_bytes,
            "budget_bytes": self.budget_bytes,
            "tier1_bytes": self.tier1_bytes,
            "tier2_bytes": self.tier2_bytes,
            "tier3_bytes": self.tier3_bytes,
            "tier1_release_bytes": self.tier1_release_bytes,
            "tier2_release_bytes": self.tier2_release_bytes,
            "tier3_release_bytes": self.tier3_release_bytes,
            "flush_workspace_reserve_bytes": self.flush_workspace_reserve_bytes
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum RecognitionState {
    Pending,
    Matched { page_id: String },
    CompletedNoMatch,
    Failed { reason: String },
}

impl RecognitionState {
    pub fn from_matched_page(matched_page: Option<String>) -> Self {
        match matched_page {
            Some(page_id) => Self::Matched { page_id },
            None => Self::CompletedNoMatch,
        }
    }

    fn page_id(&self) -> Option<&str> {
        match self {
            Self::Matched { page_id } => Some(page_id),
            Self::Pending | Self::CompletedNoMatch | Self::Failed { .. } => None,
        }
    }

    fn can_dedupe_with(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (Self::Matched { page_id: left }, Self::Matched { page_id: right }) if left == right
        )
    }

    fn can_spill(&self) -> bool {
        !matches!(self, Self::Pending)
    }

    pub fn as_json(&self) -> Value {
        match self {
            Self::Pending => json!({"state": "pending"}),
            Self::Matched { page_id } => json!({"state": "matched", "page_id": page_id}),
            Self::CompletedNoMatch => json!({"state": "completed_no_match"}),
            Self::Failed { reason } => json!({"state": "failed", "reason": reason}),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackpressureState {
    Normal,
    Tier1Dedup,
    Tier2Flush,
    Tier3Paused,
    Tier3Resumable,
    SpillDegraded,
}

impl BackpressureState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Tier1Dedup => "tier1_dedup",
            Self::Tier2Flush => "tier2_flush",
            Self::Tier3Paused => "tier3_paused",
            Self::Tier3Resumable => "tier3_resumable",
            Self::SpillDegraded => "spill_degraded",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameStorageState {
    Memory,
    Artifact,
    Dropped,
}

impl FrameStorageState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Artifact => "artifact",
            Self::Dropped => "dropped",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Tier3PauseCheckpoint {
    pub last_frame_index: usize,
    pub resident_bytes: u64,
    pub tier1_bytes: u64,
    pub tier2_bytes: u64,
    pub tier3_bytes: u64,
    pub active_segment_id: Option<u64>,
    pub in_flight_flush_state: String,
    pub current_step_index: Option<usize>,
    pub current_step_id: Option<String>,
    pub current_operation_id: Option<String>,
    pub current_phase: Option<String>,
    pub expected_page: Option<String>,
    pub last_matched_page: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameStoreEvent {
    PressureChanged {
        state: CapturePressureState,
        memory_budget_bytes: u64,
        resident_bytes: u64,
    },
    DedupWindow {
        representative_frame_index: usize,
        preserved_frame_index: Option<usize>,
        duplicate_count: u64,
        duration_ms: u64,
    },
}

#[derive(Debug, Clone)]
pub struct FramePersistenceCandidate<'a> {
    pub frame_index: usize,
    pub file_name: String,
    pub captured_at: SystemTime,
    pub pinned_reason: Option<PinnedFrameReason>,
    pub png: Cow<'a, [u8]>,
}

pub type FramePersistencePublisher<'a> = dyn FnMut(&FramePersistenceCandidate<'_>) -> CliOutcome<(Arc<ArtifactStore>, ArtifactReference)>
    + 'a;

impl Tier3PauseCheckpoint {
    pub fn to_json(&self) -> Value {
        json!({
            "last_frame_index": self.last_frame_index,
            "resident_bytes": self.resident_bytes,
            "tier1_bytes": self.tier1_bytes,
            "tier2_bytes": self.tier2_bytes,
            "tier3_bytes": self.tier3_bytes,
            "active_segment_id": self.active_segment_id,
            "in_flight_flush_state": self.in_flight_flush_state,
            "current_step_index": self.current_step_index,
            "current_step_id": self.current_step_id,
            "current_operation_id": self.current_operation_id,
            "current_phase": self.current_phase,
            "expected_page": self.expected_page,
            "last_matched_page": self.last_matched_page
        })
    }
}

pub struct FrameStore {
    config: FrameStoreConfig,
    #[cfg(test)]
    fixture_root: PathBuf,
    #[cfg(test)]
    fixture_capacity_limit: Option<u64>,
    budget: MemoryBudget,
    resident_bytes: u64,
    caller_frame_bytes: u64,
    pending_input_bytes: u64,
    payload_bytes: u64,
    metadata_estimated_bytes: u64,
    thumbnail_estimated_bytes: u64,
    encoder_workspace_reserved_bytes: u64,
    spilled_bytes: u64,
    dropped_bytes: u64,
    entries: Vec<FrameEntry>,
    protected_history: usize,
    timeline: Vec<Value>,
    events: Vec<FrameStoreEvent>,
    tier1_active: bool,
    tier2_active: bool,
    tier3_active: bool,
    dropped_count: u64,
    spilled_count: u64,
    spill_warning_count: u64,
    last_failure: Option<FramePersistenceFailure>,
}

impl FrameStore {
    pub fn new(_frame_root: PathBuf, config: FrameStoreConfig) -> CliOutcome<Self> {
        let budget = MemoryBudget::build(&config)?;
        Ok(Self {
            config,
            #[cfg(test)]
            fixture_root: _frame_root,
            #[cfg(test)]
            fixture_capacity_limit: None,
            budget,
            resident_bytes: 0,
            caller_frame_bytes: 0,
            pending_input_bytes: 0,
            payload_bytes: 0,
            metadata_estimated_bytes: 0,
            thumbnail_estimated_bytes: 0,
            encoder_workspace_reserved_bytes: 0,
            spilled_bytes: 0,
            dropped_bytes: 0,
            entries: Vec::new(),
            protected_history: 0,
            timeline: Vec::new(),
            events: Vec::new(),
            tier1_active: false,
            tier2_active: false,
            tier3_active: false,
            dropped_count: 0,
            spilled_count: 0,
            spill_warning_count: 0,
            last_failure: None,
        })
    }

    /// Configure before accepting frames; the window keeps each original materializable.
    pub fn protect_recent_frames(&mut self, count: usize) -> CliOutcome<()> {
        if !self.entries.is_empty() || count > 64 {
            return Err(CliError::usage(
                "backtrace window must be set before capture and at most 64 frames",
            ));
        }
        self.protected_history = count;
        Ok(())
    }

    pub fn original_material(&self, frame_index: usize) -> Option<&FrameMaterialIdentity> {
        self.entries
            .iter()
            .find(|entry| entry.frame_index == frame_index)
            .map(|entry| &entry.material)
    }

    pub(crate) fn persisted_reference(&self, frame_index: usize) -> Option<&ArtifactReference> {
        self.entries
            .iter()
            .find(|entry| entry.frame_index == frame_index)
            .and_then(|entry| match &entry.storage {
                FrameStorage::Artifact(material) if entry.artifact_persisted => {
                    Some(&material.reference)
                }
                _ => None,
            })
    }

    pub(crate) fn record_recognition(
        &mut self,
        frame_index: usize,
        state: RecognitionState,
    ) -> CliOutcome<()> {
        let index = self
            .entries
            .iter()
            .position(|entry| entry.frame_index == frame_index)
            .ok_or_else(|| {
                CliError::usage(format!("unknown frame index {frame_index} for recognition"))
            })?;
        let previous_matches = self.entries[..index]
            .iter()
            .rev()
            .find(|entry| entry.retained)
            .is_some_and(|entry| entry.recognition_state.can_dedupe_with(&state));
        let entry = &mut self.entries[index];
        entry.key_frame = entry.pinned_reason.is_some()
            || entry.label == "initial"
            || entry.label.contains("click")
            || entry.label.contains("action")
            || entry.label.contains("before")
            || entry.label.contains("after")
            || matches!(state, RecognitionState::Failed { .. })
            || !previous_matches;
        entry.recognition_state = state;
        Ok(())
    }

    /// A pin cannot resurrect a perceptually similar representative as the original frame.
    pub fn pin_frame(&mut self, frame_index: usize, reason: PinnedFrameReason) -> CliOutcome<()> {
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| entry.frame_index == frame_index)
            .ok_or_else(|| CliError::usage("pin frame identity is not in this capture"))?;
        if !entry.retained || matches!(entry.storage, FrameStorage::Dropped) {
            return Err(CliError::fatal(
                "frame_material_unavailable",
                "pin_frame",
                "original frame material has already been released",
            ));
        }
        entry.pinned_reason.get_or_insert(reason);
        entry.key_frame = true;
        Ok(())
    }

    pub fn set_config(&mut self, config: FrameStoreConfig) -> CliOutcome<()> {
        let budget = MemoryBudget::build(&config)?;
        self.config = config;
        self.budget = budget;
        Ok(())
    }

    pub fn add_frame(
        &mut self,
        input: FrameStoreFrameInput,
        publish: &mut FramePersistencePublisher<'_>,
    ) -> CliOutcome<FrameStoreOutcome> {
        self.last_failure = None;
        self.refresh_budget()?;
        let estimate = estimate_entry(&input)?;
        // This input is already alive while history is published. Keep its full
        // stored charge until ownership moves into the resident entry below.
        self.pending_input_bytes = estimate.stored_bytes();
        let result = self.add_frame_admitted(input, estimate, publish);
        self.pending_input_bytes = 0;
        result
    }

    fn add_frame_admitted(
        &mut self,
        mut input: FrameStoreFrameInput,
        mut estimate: ResidentEstimate,
        publish: &mut FramePersistencePublisher<'_>,
    ) -> CliOutcome<FrameStoreOutcome> {
        self.release_watermarks_if_needed();
        let mut frame_failures = Vec::new();
        let mut attempted = Vec::new();
        let mut refused = false;
        let projected = self.resident_bytes.saturating_add(estimate.total());
        if projected >= self.budget.tier1_bytes {
            self.activate_tier1(projected);
            self.dedup_existing()?;
        }
        if self.resident_bytes.saturating_add(estimate.total()) >= self.budget.tier2_bytes {
            self.activate_tier2(projected);
            self.flush_resident_frames(publish, &mut attempted, &mut frame_failures, &mut refused)?;
        }
        // The already-returned frame stays with its caller on refusal. No encoding or
        // resident accounting commit precedes reservation of the complete live set.
        if self
            .live_bytes()
            .and_then(|bytes| bytes.checked_add(estimate.encoder_workspace))
            .is_none_or(|bytes| bytes > self.budget.budget_bytes)
        {
            self.activate_tier3(self.resident_bytes.saturating_add(estimate.total()));
            return Err(CliError::frame_workspace_refused());
        }
        let file = format!("screenshots/{}", input.file_name);
        let thumb = thumbnail(&input.frame);
        let png = input
            .frame
            .png_for_artifact_with_budget(estimate.encoder_workspace)
            .map_err(|error| CliError::device(error.to_string()))?;
        let material = FrameMaterialIdentity {
            frame_index: input.frame_index,
            byte_count: png.len() as u64,
            sha256: crate::store::canonical_sha256(&png),
        };
        if let Cow::Owned(png) = png {
            input.frame.original_png = Some(png);
        }
        estimate = estimate_entry(&input)?;
        let key_frame = self.is_key_frame(&input);
        let entry = FrameEntry {
            material,
            frame_index: input.frame_index,
            file_name: input.file_name,
            file: file.clone(),
            width: input.frame.width,
            height: input.frame.height,
            captured_at: input.frame.captured_at,
            backend: input.frame.backend_name.as_str().to_owned(),
            pixel_format: input.frame.pixel_format.as_str().to_owned(),
            label: input.label,
            recognition_state: input.recognition_state,
            key_frame,
            pinned_reason: input.pinned_reason,
            artifact_persisted: false,
            artifact_material: None,
            similarity_recorded: false,
            merged_count: 0,
            dwell_ms: 0,
            delta_from_previous_ms: self.delta_from_previous_ms(input.frame.captured_at),
            retained: true,
            merged_into: None,
            storage: FrameStorage::Resident(input.frame),
            storage_state: FrameStorageState::Memory,
            resident_estimate: estimate,
            thumb,
            spill_failed: false,
        };
        self.add_estimate(entry.resident_estimate);
        self.pending_input_bytes = 0;
        self.entries.push(entry);
        let index = self.entries.len() - 1;
        self.timeline.push(json!({
            "event": "frame_retained",
            "frame_index": self.entries[index].frame_index,
            "file": file,
            "original_material": self.entries[index].material,
            "resident_bytes": self.resident_bytes
        }));
        if self.tier1_active {
            self.dedup_existing()?;
        }
        let tier3_triggered = self.resident_bytes >= self.budget.tier3_bytes;
        if tier3_triggered {
            self.activate_tier3(self.resident_bytes);
        }
        if self.tier2_active {
            self.flush_resident_frames(publish, &mut attempted, &mut frame_failures, &mut refused)?;
        }
        self.release_watermarks_if_needed();
        let pause_required =
            refused || (self.tier3_active && self.resident_bytes > self.budget.tier3_release_bytes);
        let backpressure_state = if refused {
            BackpressureState::SpillDegraded
        } else if pause_required {
            BackpressureState::Tier3Paused
        } else if tier3_triggered {
            BackpressureState::Tier3Resumable
        } else if self.tier2_active {
            BackpressureState::Tier2Flush
        } else if self.tier1_active {
            BackpressureState::Tier1Dedup
        } else {
            BackpressureState::Normal
        };
        let entry = &self.entries[index];
        Ok(FrameStoreOutcome {
            retained: entry.retained,
            file: entry.retained.then(|| entry.file.clone()),
            merged_into: entry.merged_into.clone(),
            storage_state: entry.storage_state,
            tier1_active: self.tier1_active,
            tier2_active: self.tier2_active,
            tier3_triggered,
            backpressure_state,
            pause_required,
            frame_failures,
            checkpoint: pause_required.then(|| self.pause_checkpoint(input.frame_index)),
        })
    }

    /// Releases only resident ownership. Published artifacts and old directories belong
    /// to their original Ledger/retention owners and are never deleted here.
    pub fn cleanup_temp(&mut self) -> CliOutcome<()> {
        for index in 0..self.entries.len() {
            if self.entries[index].artifact_persisted {
                self.release_persisted_memory(index)?;
            }
        }
        Ok(())
    }
    pub fn screenshots(&self) -> Vec<FrameStoreScreenshot> {
        self.entries
            .iter()
            .filter(|entry| entry.retained)
            .map(|entry| FrameStoreScreenshot {
                frame_index: entry.frame_index,
                file: entry.file.clone(),
                width: entry.width,
                height: entry.height,
                dwell_ms: entry.dwell_ms,
                merged_count: entry.merged_count,
                matched_page: entry.recognition_state.page_id().map(str::to_string),
                recognition_state: entry.recognition_state.clone(),
                key_frame: entry.key_frame,
                pinned_reason: entry.pinned_reason,
                artifact_persisted: entry.artifact_persisted,
                storage_state: entry.storage_state,
            })
            .collect()
    }

    pub fn diagnostics_json(&self) -> Value {
        json!({
            "schema_version": "Lab-1z.frame_store.v2",
            "config": {
                "similarity_threshold": self.config.similarity_threshold,
                "tier1_ratio": self.config.tier1_ratio,
                "tier2_ratio": self.config.tier2_ratio,
                "tier3_ratio": self.config.tier3_ratio,
                "hysteresis_ratio": self.config.hysteresis_ratio,
                "max_mem_bytes": self.config.max_mem_bytes,
                "os_reserve_bytes": self.config.os_reserve_bytes,
                "flush_workspace_reserve_bytes": self.config.flush_workspace_reserve_bytes,
                "tier3_mode": "synchronous_graceful_failure"
            },
            "budget": self.budget.to_json(),
            "resident_bytes": self.resident_bytes,
            "payload_bytes": self.payload_bytes,
            "metadata_estimated_bytes": self.metadata_estimated_bytes,
            "thumbnail_estimated_bytes": self.thumbnail_estimated_bytes,
            "encoder_workspace_reserved_bytes": self.encoder_workspace_reserved_bytes,
            "spilled_bytes": self.spilled_bytes,
            "dropped_bytes": self.dropped_bytes,
            "retained_count": self.entries.iter().filter(|entry| entry.retained).count(),
            "captured_count": self.entries.len(),
            "dropped_count": self.dropped_count,
            "spilled_count": self.spilled_count,
            "spill_warning_count": self.spill_warning_count,
            "tier1_active": self.tier1_active,
            "tier2_active": self.tier2_active,
            "tier3_active": self.tier3_active
        })
    }

    pub fn timeline(&self) -> Vec<Value> {
        let mut rows = self.timeline.clone();
        rows.extend(self.entries.iter().map(|entry| {
            json!({
                "event": "frame_final",
                "frame_index": entry.frame_index,
                "original_material": entry.material,
                "file": entry.file,
                "retained": entry.retained,
                "merged_into": entry.merged_into,
                "recognition_state": entry.recognition_state.as_json(),
                "label": entry.label,
                "backend": entry.backend,
                "pixel_format": entry.pixel_format,
                "key_frame": entry.key_frame,
                "pinned_reason": entry.pinned_reason.map(PinnedFrameReason::as_str),
                "artifact_persisted": entry.artifact_persisted,
                "dwell_ms": entry.dwell_ms,
                "merged_count": entry.merged_count,
                "storage": entry.storage_state.as_str(),
                "resident_bytes_estimate": entry.resident_estimate.total(),
                "metadata_bytes_estimate": entry.resident_estimate.metadata,
                "thumb_bytes_estimate": entry.resident_estimate.thumbnail,
                "encoder_workspace_bytes_estimate": entry.resident_estimate.encoder_workspace
            })
        }));
        rows
    }

    pub fn drain_events(&mut self) -> Vec<FrameStoreEvent> {
        std::mem::take(&mut self.events)
    }

    pub fn refresh_pressure(&mut self) -> CliOutcome<bool> {
        self.refresh_budget()?;
        let was_paused = self.tier3_active;
        if self.tier1_active {
            self.dedup_existing()?;
        }
        self.release_watermarks_if_needed();
        Ok(was_paused && !self.tier3_active)
    }

    pub(crate) fn is_pressure_paused(&self) -> bool {
        self.tier3_active && self.resident_bytes > self.budget.tier3_release_bytes
    }

    pub(crate) fn failure_frame_index(&self, error: &CliError) -> Option<usize> {
        self.last_failure
            .as_ref()
            .filter(|failure| failure.error == *error)
            .map(|failure| failure.frame_index)
    }

    pub(crate) fn publication_workspace_available(&self, frame_index: usize) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.frame_index == frame_index && entry.retained)
            && self
                .live_bytes()
                .and_then(|bytes| bytes.checked_add(WRITER_BUFFER_BYTES))
                .is_some_and(|bytes| bytes <= self.budget.budget_bytes)
    }

    pub(crate) fn flush_pressure(
        &mut self,
        publish: &mut FramePersistencePublisher<'_>,
    ) -> CliOutcome<Vec<FramePersistenceFailure>> {
        self.last_failure = None;
        let mut failures = Vec::new();
        if self.tier2_active || self.tier3_active {
            self.flush_resident_frames(publish, &mut Vec::new(), &mut failures, &mut false)?;
        }
        self.release_watermarks_if_needed();
        Ok(failures)
    }

    pub(crate) fn admit_frame_copy(&mut self, frame: &Frame) -> CliOutcome<u64> {
        self.refresh_budget()?;
        let payload = (frame.pixels.capacity() as u64).checked_add(
            frame
                .original_png
                .as_ref()
                .map_or(0, |png| png.capacity() as u64),
        );
        let workspace = frame
            .artifact_png_workspace_bytes()
            .map_err(CliError::incoming_frame)?;
        let required = payload
            .and_then(|bytes| bytes.checked_mul(2))
            .and_then(|bytes| bytes.checked_add(workspace.max(WRITER_BUFFER_BYTES)))
            .and_then(|bytes| {
                bytes.checked_add(
                    ENTRY_BASE_METADATA_BYTES * 2 + (THUMB_WIDTH * THUMB_HEIGHT) as u64,
                )
            });
        if required
            .and_then(|bytes| self.live_bytes()?.checked_add(bytes))
            .is_none_or(|bytes| bytes > self.budget.budget_bytes)
        {
            return Err(CliError::frame_workspace_refused());
        }
        let previous = self.caller_frame_bytes;
        self.caller_frame_bytes = payload
            .and_then(|bytes| bytes.checked_add(ENTRY_BASE_METADATA_BYTES))
            .and_then(|bytes| previous.checked_add(bytes))
            .ok_or_else(CliError::frame_workspace_refused)?;
        Ok(previous)
    }

    pub(crate) fn release_frame_copy(&mut self, previous: u64) {
        self.caller_frame_bytes = previous;
    }

    fn live_bytes(&self) -> Option<u64> {
        self.resident_bytes
            .checked_add(self.caller_frame_bytes)?
            .checked_add(self.pending_input_bytes)
    }

    pub(crate) fn persistence_candidate_indexes(&self, include_all_retained: bool) -> Vec<usize> {
        self.entries
            .iter()
            .filter(|entry| {
                entry.retained
                    && !entry.artifact_persisted
                    && (include_all_retained || entry.pinned_reason.is_some())
            })
            .map(|entry| entry.frame_index)
            .collect()
    }

    pub(crate) fn persistence_candidate(
        &mut self,
        frame_index: usize,
    ) -> CliOutcome<FramePersistenceCandidate<'_>> {
        self.refresh_budget()?;
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.frame_index == frame_index)
            .ok_or_else(|| {
                CliError::usage(format!("unknown frame index {frame_index} for persistence"))
            })?;
        if !entry.retained {
            return Err(CliError::fatal(
                "frame_material_unavailable",
                "persist_capture_frame",
                "original frame is not retained",
            ));
        }
        let material_read = if matches!(entry.storage, FrameStorage::Artifact(_)) {
            entry.material.byte_count.checked_mul(3)
        } else {
            Some(0)
        };
        if material_read
            .and_then(|bytes| bytes.checked_add(WRITER_BUFFER_BYTES))
            .and_then(|bytes| self.live_bytes()?.checked_add(bytes))
            .is_none_or(|bytes| bytes > self.budget.budget_bytes)
        {
            return Err(CliError::frame_workspace_refused());
        }
        Ok(FramePersistenceCandidate {
            frame_index,
            file_name: entry.file_name.clone(),
            captured_at: entry.captured_at,
            pinned_reason: entry.pinned_reason,
            png: entry.original_png()?,
        })
    }

    pub(crate) fn mark_artifact_persisted(
        &mut self,
        frame_index: usize,
        store: Arc<ArtifactStore>,
        reference: ArtifactReference,
    ) -> CliOutcome<()> {
        let index = self
            .entries
            .iter()
            .position(|entry| entry.frame_index == frame_index)
            .ok_or_else(|| {
                CliError::usage(format!("unknown frame index {frame_index} for persistence"))
            })?;
        let entry = &mut self.entries[index];
        if !entry.retained {
            return Err(CliError::usage(format!(
                "deduplicated frame index {frame_index} cannot be marked persisted"
            )));
        }
        if reference.byte_count() != entry.material.byte_count
            || reference.sha256() != entry.material.sha256
        {
            return Err(CliError::fatal(
                "frame_material_hash_mismatch",
                "mark_capture_frame_persisted",
                "persisted artifact differs from the original PNG",
            ));
        }
        entry.artifact_persisted = true;
        entry.artifact_material = Some(PersistedFrameMaterial { store, reference });
        self.release_persisted_memory(index)?;
        Ok(())
    }

    fn refresh_budget(&mut self) -> CliOutcome<()> {
        self.budget = MemoryBudget::build(&self.config)?;
        Ok(())
    }

    fn activate_tier1(&mut self, projected_bytes: u64) {
        if !self.tier1_active {
            self.tier1_active = true;
            self.events.push(FrameStoreEvent::PressureChanged {
                state: CapturePressureState::Tier1Dedup,
                memory_budget_bytes: self.budget.budget_bytes,
                resident_bytes: projected_bytes,
            });
            self.timeline.push(json!({
                "event": "tier1_activated",
                "projected_bytes": projected_bytes,
                "resident_bytes": self.resident_bytes,
                "threshold_bytes": self.budget.tier1_bytes
            }));
        }
    }

    fn activate_tier2(&mut self, projected_bytes: u64) {
        if !self.tier2_active {
            self.tier2_active = true;
            self.events.push(FrameStoreEvent::PressureChanged {
                state: CapturePressureState::Tier2Flush,
                memory_budget_bytes: self.budget.budget_bytes,
                resident_bytes: projected_bytes,
            });
            self.timeline.push(json!({
                "event": "tier2_activated",
                "projected_bytes": projected_bytes,
                "resident_bytes": self.resident_bytes,
                "threshold_bytes": self.budget.tier2_bytes
            }));
        }
    }

    fn activate_tier3(&mut self, projected_bytes: u64) {
        if !self.tier3_active {
            self.tier3_active = true;
            self.events.push(FrameStoreEvent::PressureChanged {
                state: CapturePressureState::Tier3Paused,
                memory_budget_bytes: self.budget.budget_bytes,
                resident_bytes: projected_bytes,
            });
            self.timeline.push(json!({
                "event": "tier3_activated",
                "projected_bytes": projected_bytes,
                "resident_bytes": self.resident_bytes,
                "threshold_bytes": self.budget.tier3_bytes
            }));
        }
    }

    fn release_watermarks_if_needed(&mut self) {
        if self.tier3_active && self.resident_bytes <= self.budget.tier3_release_bytes {
            self.tier3_active = false;
            self.events.push(FrameStoreEvent::PressureChanged {
                state: CapturePressureState::Tier3Resumed,
                memory_budget_bytes: self.budget.budget_bytes,
                resident_bytes: self.resident_bytes,
            });
            self.timeline.push(json!({
                "event": "tier3_released",
                "resident_bytes": self.resident_bytes,
                "release_bytes": self.budget.tier3_release_bytes
            }));
        }
        if self.tier2_active && self.resident_bytes <= self.budget.tier2_release_bytes {
            self.tier2_active = false;
            self.timeline.push(json!({
                "event": "tier2_released",
                "resident_bytes": self.resident_bytes,
                "release_bytes": self.budget.tier2_release_bytes
            }));
        }
        if self.tier1_active && self.resident_bytes <= self.budget.tier1_release_bytes {
            self.tier1_active = false;
            self.timeline.push(json!({
                "event": "tier1_released",
                "resident_bytes": self.resident_bytes,
                "release_bytes": self.budget.tier1_release_bytes
            }));
        }
    }

    fn dedup_existing(&mut self) -> CliOutcome<()> {
        let mut previous_retained = None;
        for index in 0..self.entries.len() {
            if !self.entries[index].retained {
                continue;
            }
            if self.entries[index].artifact_persisted {
                self.release_persisted_memory(index)?;
                if !self.entries[index].key_frame
                    && !self.entries[index].similarity_recorded
                    && let Some(previous) = previous_retained
                    && self.same_page_duplicate(previous, index)
                {
                    self.entries[index].similarity_recorded = true;
                    self.events.push(FrameStoreEvent::DedupWindow {
                        representative_frame_index: self.entries[previous].frame_index,
                        preserved_frame_index: Some(self.entries[index].frame_index),
                        duplicate_count: 1,
                        duration_ms: self.entries[index].delta_from_previous_ms.max(1),
                    });
                }
                previous_retained = Some(index);
                continue;
            }
            let should_keep = self.entries[index].key_frame
                || index >= self.entries.len().saturating_sub(self.protected_history)
                || previous_retained
                    .is_none_or(|previous| !self.same_page_duplicate(previous, index));
            if should_keep {
                previous_retained = Some(index);
            } else if let Some(previous) = previous_retained {
                self.drop_entry(index, previous);
            }
        }
        Ok(())
    }

    fn release_persisted_memory(&mut self, index: usize) -> CliOutcome<()> {
        if !matches!(self.entries[index].storage, FrameStorage::Resident(_)) {
            return Ok(());
        }
        let Some(material) = self.entries[index].artifact_material.take() else {
            return Err(CliError::fatal(
                "frame_material_unavailable",
                "release_persisted_frame_memory",
                "persisted frame has no original material reference",
            ));
        };
        let estimate = self.entries[index].resident_estimate;
        self.entries[index].storage = FrameStorage::Artifact(material);
        self.entries[index].storage_state = FrameStorageState::Artifact;
        self.replace_resident_estimate(
            index,
            ResidentEstimate {
                payload: 0,
                encoder_workspace: 0,
                ..estimate
            },
        );
        Ok(())
    }

    fn same_page_duplicate(&self, previous: usize, current: usize) -> bool {
        self.entries[previous]
            .recognition_state
            .can_dedupe_with(&self.entries[current].recognition_state)
            && thumb_similarity(&self.entries[previous].thumb, &self.entries[current].thumb)
                > self.config.similarity_threshold
    }

    fn drop_entry(&mut self, index: usize, target: usize) {
        if !self.entries[index].retained {
            return;
        }
        let target_file = self.entries[target].file.clone();
        let dropped_file = self.entries[index].file.clone();
        let dropped_delta = self.entries[index].delta_from_previous_ms;
        let released = self.release_large_objects(index);
        self.dropped_bytes = self.dropped_bytes.saturating_add(released);
        self.entries[index].retained = false;
        self.entries[index].merged_into = Some(target_file.clone());
        self.entries[target].merged_count = self.entries[target].merged_count.saturating_add(1);
        self.entries[target].dwell_ms = self.entries[target].dwell_ms.saturating_add(dropped_delta);
        self.dropped_count = self.dropped_count.saturating_add(1);
        self.entries[index].storage_state = FrameStorageState::Dropped;
        self.events.push(FrameStoreEvent::DedupWindow {
            representative_frame_index: self.entries[target].frame_index,
            preserved_frame_index: None,
            duplicate_count: 1,
            duration_ms: dropped_delta.max(1),
        });
        self.timeline.push(json!({
            "event": "frame_deduplicated",
            "frame_index": self.entries[index].frame_index,
            "file": dropped_file,
            "merged_into": target_file,
            "released_bytes": released,
            "resident_bytes": self.resident_bytes
        }));
    }

    fn release_large_objects(&mut self, index: usize) -> u64 {
        let metadata = self.entries[index].resident_estimate.metadata;
        let released =
            match std::mem::replace(&mut self.entries[index].storage, FrameStorage::Dropped) {
                FrameStorage::Resident(_) | FrameStorage::Artifact(_) => self
                    .replace_resident_estimate(
                        index,
                        ResidentEstimate {
                            metadata,
                            ..ResidentEstimate::default()
                        },
                    ),
                FrameStorage::Dropped => 0,
            };
        self.entries[index].thumb.values = Vec::new();
        released
    }

    fn flush_resident_frames(
        &mut self,
        publish: &mut FramePersistencePublisher<'_>,
        attempted: &mut Vec<usize>,
        frame_failures: &mut Vec<FramePersistenceFailure>,
        refused: &mut bool,
    ) -> CliOutcome<()> {
        if *refused {
            return Ok(());
        }
        for index in 0..self.entries.len() {
            if self.entries[index].artifact_persisted {
                self.release_persisted_memory(index)?;
                continue;
            }
            let entry = &self.entries[index];
            if !entry.retained
                || entry.spill_failed
                || !entry.recognition_state.can_spill()
                || !matches!(entry.storage, FrameStorage::Resident(_))
                || attempted.contains(&index)
            {
                continue;
            }
            attempted.push(index);
            let frame_index = entry.frame_index;
            let result = self
                .persistence_candidate(frame_index)
                .and_then(|candidate| publish(&candidate));
            match result {
                Ok((store, reference)) => {
                    let bytes = reference.byte_count();
                    self.mark_artifact_persisted(frame_index, store, reference)?;
                    self.spilled_count = self.spilled_count.saturating_add(1);
                    self.spilled_bytes = self.spilled_bytes.saturating_add(bytes);
                }
                Err(error) => {
                    self.spill_warning_count = self.spill_warning_count.saturating_add(1);
                    self.last_failure = Some(FramePersistenceFailure {
                        frame_index,
                        error: error.clone(),
                    });
                    if error.is_fatal() {
                        self.entries[index].spill_failed = true;
                        return Err(error);
                    }
                    frame_failures.push(FramePersistenceFailure { frame_index, error });
                    *refused = true;
                    break;
                }
            }
        }
        Ok(())
    }
    fn add_estimate(&mut self, estimate: ResidentEstimate) {
        self.resident_bytes = self.resident_bytes.saturating_add(estimate.total());
        self.payload_bytes = self.payload_bytes.saturating_add(estimate.payload);
        self.metadata_estimated_bytes = self
            .metadata_estimated_bytes
            .saturating_add(estimate.metadata);
        self.thumbnail_estimated_bytes = self
            .thumbnail_estimated_bytes
            .saturating_add(estimate.thumbnail);
        self.encoder_workspace_reserved_bytes = self
            .encoder_workspace_reserved_bytes
            .saturating_add(estimate.encoder_workspace);
    }

    fn replace_resident_estimate(&mut self, index: usize, estimate: ResidentEstimate) -> u64 {
        let old = self.entries[index].resident_estimate;
        self.subtract_estimate(old);
        self.entries[index].resident_estimate = estimate;
        self.add_estimate(estimate);
        old.total().saturating_sub(estimate.total())
    }

    fn subtract_estimate(&mut self, estimate: ResidentEstimate) {
        self.resident_bytes = self.resident_bytes.saturating_sub(estimate.total());
        self.payload_bytes = self.payload_bytes.saturating_sub(estimate.payload);
        self.metadata_estimated_bytes = self
            .metadata_estimated_bytes
            .saturating_sub(estimate.metadata);
        self.thumbnail_estimated_bytes = self
            .thumbnail_estimated_bytes
            .saturating_sub(estimate.thumbnail);
        self.encoder_workspace_reserved_bytes = self
            .encoder_workspace_reserved_bytes
            .saturating_sub(estimate.encoder_workspace);
    }

    fn is_key_frame(&self, input: &FrameStoreFrameInput) -> bool {
        let label = input.label.as_str();
        input.pinned_reason.is_some()
            || label == "initial"
            || label.contains("click")
            || label.contains("action")
            || label.contains("before")
            || label.contains("after")
            || matches!(input.recognition_state, RecognitionState::Failed { .. })
            || self
                .entries
                .iter()
                .rev()
                .find(|entry| entry.retained)
                .is_none_or(|previous| {
                    !previous
                        .recognition_state
                        .can_dedupe_with(&input.recognition_state)
                })
    }

    fn pause_checkpoint(&self, last_frame_index: usize) -> Tier3PauseCheckpoint {
        Tier3PauseCheckpoint {
            last_frame_index,
            resident_bytes: self.resident_bytes,
            tier1_bytes: self.budget.tier1_bytes,
            tier2_bytes: self.budget.tier2_bytes,
            tier3_bytes: self.budget.tier3_bytes,
            active_segment_id: None,
            in_flight_flush_state: "idle".to_string(),
            current_step_index: None,
            current_step_id: None,
            current_operation_id: None,
            current_phase: None,
            expected_page: None,
            last_matched_page: None,
        }
    }

    fn delta_from_previous_ms(&self, current: SystemTime) -> u64 {
        self.entries
            .last()
            .and_then(|entry| current.duration_since(entry.captured_at).ok())
            .unwrap_or(Duration::ZERO)
            .as_millis() as u64
    }
}

pub struct FrameStoreFrameInput {
    pub frame_index: usize,
    pub file_name: String,
    pub label: String,
    pub recognition_state: RecognitionState,
    pub pinned_reason: Option<PinnedFrameReason>,
    pub frame: Frame,
}

#[derive(Debug)]
pub struct FrameStoreOutcome {
    pub retained: bool,
    pub file: Option<String>,
    pub merged_into: Option<String>,
    pub storage_state: FrameStorageState,
    pub tier1_active: bool,
    pub tier2_active: bool,
    pub tier3_triggered: bool,
    pub backpressure_state: BackpressureState,
    pub pause_required: bool,
    pub frame_failures: Vec<FramePersistenceFailure>,
    pub checkpoint: Option<Tier3PauseCheckpoint>,
}

#[derive(Debug, Clone)]
pub struct FrameStoreScreenshot {
    pub frame_index: usize,
    pub file: String,
    pub width: u32,
    pub height: u32,
    pub dwell_ms: u64,
    pub merged_count: u64,
    pub matched_page: Option<String>,
    pub recognition_state: RecognitionState,
    pub key_frame: bool,
    pub pinned_reason: Option<PinnedFrameReason>,
    pub artifact_persisted: bool,
    pub storage_state: FrameStorageState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FrameMaterialIdentity {
    pub frame_index: usize,
    pub byte_count: u64,
    pub sha256: String,
}

struct FrameEntry {
    material: FrameMaterialIdentity,
    frame_index: usize,
    file_name: String,
    file: String,
    width: u32,
    height: u32,
    captured_at: SystemTime,
    backend: String,
    pixel_format: String,
    label: String,
    recognition_state: RecognitionState,
    key_frame: bool,
    pinned_reason: Option<PinnedFrameReason>,
    artifact_persisted: bool,
    artifact_material: Option<PersistedFrameMaterial>,
    similarity_recorded: bool,
    merged_count: u64,
    dwell_ms: u64,
    delta_from_previous_ms: u64,
    retained: bool,
    merged_into: Option<String>,
    storage: FrameStorage,
    storage_state: FrameStorageState,
    resident_estimate: ResidentEstimate,
    thumb: Thumbnail,
    spill_failed: bool,
}

#[derive(Debug, Clone)]
pub struct FramePersistenceFailure {
    pub frame_index: usize,
    pub error: CliError,
}

impl FrameEntry {
    fn original_png(&self) -> CliOutcome<Cow<'_, [u8]>> {
        let png = match &self.storage {
            FrameStorage::Resident(frame) => frame
                .png_for_artifact_with_budget(0)
                .map_err(|error| CliError::device(error.to_string()))?,
            FrameStorage::Artifact(material) => {
                Cow::Owned(material.store.read_verified(&material.reference)?)
            }
            FrameStorage::Dropped => {
                return Err(CliError::fatal(
                    "frame_material_unavailable",
                    "read_original_frame",
                    "original frame material is unavailable",
                ));
            }
        };
        if png.len() as u64 != self.material.byte_count
            || crate::store::canonical_sha256(&png) != self.material.sha256
        {
            return Err(CliError::fatal(
                "frame_material_hash_mismatch",
                "read_original_frame",
                "materialized frame differs from its original PNG",
            ));
        }
        Ok(png)
    }
}

#[derive(Clone)]
struct PersistedFrameMaterial {
    store: Arc<ArtifactStore>,
    reference: ArtifactReference,
}

enum FrameStorage {
    Resident(Frame),
    Artifact(PersistedFrameMaterial),
    Dropped,
}

#[derive(Clone)]
struct Thumbnail {
    values: Vec<u8>,
}

#[derive(Clone, Copy, Default)]
struct ResidentEstimate {
    payload: u64,
    metadata: u64,
    thumbnail: u64,
    encoder_workspace: u64,
}

impl ResidentEstimate {
    fn stored_bytes(self) -> u64 {
        self.payload
            .saturating_add(self.metadata)
            .saturating_add(self.thumbnail)
    }

    fn total(self) -> u64 {
        self.stored_bytes().saturating_add(self.encoder_workspace)
    }
}

fn estimate_entry(input: &FrameStoreFrameInput) -> CliOutcome<ResidentEstimate> {
    let original_png = input
        .frame
        .original_png
        .as_ref()
        .map(|png| png.capacity() as u64)
        .unwrap_or(0);
    let payload = input.frame.pixels.capacity() as u64 + original_png;
    let metadata = ENTRY_BASE_METADATA_BYTES
        + string_capacity_bytes(&input.file_name)
        + "screenshots/".len() as u64
        + string_capacity_bytes(&input.file_name)
        + string_capacity_bytes(&input.label)
        + input
            .recognition_state
            .page_id()
            .map(string_capacity_bytes)
            .unwrap_or(0)
        + match &input.recognition_state {
            RecognitionState::Failed { reason } => string_capacity_bytes(reason),
            RecognitionState::Pending
            | RecognitionState::Matched { .. }
            | RecognitionState::CompletedNoMatch => 0,
        };
    let thumbnail = (THUMB_WIDTH * THUMB_HEIGHT) as u64;
    let encoder_workspace = input
        .frame
        .artifact_png_workspace_bytes()
        .map_err(CliError::incoming_frame)?;
    Ok(ResidentEstimate {
        payload,
        metadata,
        thumbnail,
        encoder_workspace,
    })
}

fn string_capacity_bytes(value: &str) -> u64 {
    value.len() as u64
}

fn thumbnail(frame: &Frame) -> Thumbnail {
    let channels = match frame.pixel_format {
        PixelFormat::Rgb8 => 3usize,
        PixelFormat::Rgba8 => 4usize,
    };
    let width = frame.width as usize;
    let height = frame.height as usize;
    let mut values = Vec::with_capacity(THUMB_WIDTH * THUMB_HEIGHT);
    for ty in 0..THUMB_HEIGHT {
        let y = ((ty.saturating_mul(height)) / THUMB_HEIGHT).min(height.saturating_sub(1));
        for tx in 0..THUMB_WIDTH {
            let x = ((tx.saturating_mul(width)) / THUMB_WIDTH).min(width.saturating_sub(1));
            let offset = y
                .checked_mul(width)
                .and_then(|row| row.checked_add(x))
                .and_then(|pixel| pixel.checked_mul(channels));
            let Some(offset) = offset else {
                values.push(0);
                continue;
            };
            let r = frame.pixels.get(offset).copied().unwrap_or(0) as u16;
            let g = frame.pixels.get(offset + 1).copied().unwrap_or(0) as u16;
            let b = frame.pixels.get(offset + 2).copied().unwrap_or(0) as u16;
            values.push(((r + g + b) / 3) as u8);
        }
    }
    Thumbnail { values }
}

fn thumb_similarity(left: &Thumbnail, right: &Thumbnail) -> f32 {
    let len = left.values.len().min(right.values.len());
    if len == 0 {
        return 0.0;
    }
    let diff: u64 = left
        .values
        .iter()
        .zip(&right.values)
        .take(len)
        .map(|(a, b)| a.abs_diff(*b) as u64)
        .sum();
    1.0 - (diff as f32 / (len as f32 * 255.0))
}

fn ratio_bytes(bytes: u64, ratio: f64) -> u64 {
    ((bytes as f64) * ratio).floor() as u64
}

fn validate_ratio_f32(name: &str, value: f32) -> Result<(), String> {
    if value.is_finite() && value > 0.0 && value < 1.0 {
        Ok(())
    } else {
        Err(format!("{name} must be > 0 and < 1"))
    }
}

fn validate_ratio_f64(name: &str, value: f64) -> Result<(), String> {
    if value.is_finite() && value > 0.0 && value < 1.0 {
        Ok(())
    } else {
        Err(format!("{name} must be > 0 and < 1"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_device::CaptureBackendName;
    use std::fs;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;

    #[test]
    fn memory_budget_uses_available_total_and_os_reserve() {
        let config = test_config(500);
        let budget = MemoryBudget::from_config(
            &config,
            MemorySample {
                total_bytes: 1_000,
                available_bytes: 700,
            },
        );

        assert_eq!(budget.budget_bytes, 500);
        assert_eq!(budget.tier1_bytes, 250);
        assert_eq!(budget.tier1_release_bytes, 225);
    }

    #[test]
    fn live_memory_source_refreshes_the_budget_for_each_frame() {
        static SAMPLE_CALLS: AtomicUsize = AtomicUsize::new(0);

        fn sample_memory() -> CliOutcome<MemorySample> {
            SAMPLE_CALLS.fetch_add(1, Ordering::SeqCst);
            Ok(MemorySample {
                total_bytes: 1_000_000,
                available_bytes: 1_000_000,
            })
        }

        SAMPLE_CALLS.store(0, Ordering::SeqCst);
        let temp = TempDir::new().expect("temp");
        let config = FrameStoreConfig {
            max_mem_bytes: Some(100_000),
            os_reserve_bytes: 0,
            flush_workspace_reserve_bytes: 1,
            ..Default::default()
        }
        .with_memory_source(MemorySampleSource::live(sample_memory));
        let mut store = FrameStore::new(temp.path().join("temp"), config).expect("store");

        add_test_frame(&mut store, 1, 10, matched("fixture01/home"), "initial");

        assert_eq!(SAMPLE_CALLS.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn threshold_one_is_rejected() {
        let config = FrameStoreConfig {
            similarity_threshold: 1.0,
            ..test_config(1_000)
        };

        let err = match FrameStore::new(PathBuf::from("unused"), config) {
            Ok(_) => panic!("threshold 1.0 should be rejected"),
            Err(err) => err,
        };

        assert!(err.detail().contains("similarity_threshold"));
    }

    #[test]
    fn tier2_tier3_gap_too_small_is_rejected() {
        let config = FrameStoreConfig {
            tier1_ratio: 0.50,
            tier2_ratio: 0.89,
            tier3_ratio: 0.90,
            flush_workspace_reserve_bytes: 20,
            ..test_config(1_000)
        };

        let err = match FrameStore::new(PathBuf::from("unused"), config) {
            Ok(_) => panic!("small tier2/tier3 gap should be rejected"),
            Err(err) => err,
        };

        assert!(err.detail().contains("tier2/tier3 gap too small"));
    }

    #[test]
    fn completed_no_match_frames_do_not_same_page_dedupe() {
        let temp = TempDir::new().expect("temp");
        let mut store = small_store(temp.path(), 220);

        add_test_frame(
            &mut store,
            1,
            10,
            RecognitionState::CompletedNoMatch,
            "initial",
        );
        add_test_frame(
            &mut store,
            2,
            10,
            RecognitionState::CompletedNoMatch,
            "page_wait",
        );

        assert_eq!(store.screenshots().len(), 2);
    }

    #[test]
    fn failed_frames_do_not_same_page_dedupe() {
        let temp = TempDir::new().expect("temp");
        let mut store = small_store(temp.path(), 220);

        add_test_frame(
            &mut store,
            1,
            10,
            RecognitionState::Failed {
                reason: "synthetic failure".to_string(),
            },
            "initial",
        );
        add_test_frame(
            &mut store,
            2,
            10,
            RecognitionState::Failed {
                reason: "synthetic failure".to_string(),
            },
            "page_wait",
        );

        assert_eq!(store.screenshots().len(), 2);
    }

    #[test]
    fn matched_same_page_frames_can_dedupe() {
        let temp = TempDir::new().expect("temp");
        let mut store = small_store(temp.path(), 220);

        add_test_frame(&mut store, 1, 10, matched("fixture01/home"), "initial");
        add_test_frame(&mut store, 2, 10, matched("fixture01/home"), "page_wait");
        add_test_frame(&mut store, 3, 10, matched("fixture01/home"), "page_wait");

        let screenshots = store.screenshots();
        assert_eq!(screenshots.len(), 1);
        assert_eq!(screenshots[0].merged_count, 2);
        assert_resident_accounting(&store);
    }

    #[test]
    fn page_transition_is_retained_even_under_dedup() {
        let temp = TempDir::new().expect("temp");
        let mut store = small_store(temp.path(), 220);

        add_test_frame(&mut store, 1, 10, matched("fixture01/home"), "initial");
        add_test_frame(
            &mut store,
            2,
            10,
            matched("fixture01/terminal"),
            "page_wait",
        );

        let screenshots = store.screenshots();
        assert_eq!(screenshots.len(), 2);
        assert!(screenshots[1].key_frame);
    }

    #[test]
    fn tier2_persists_artifacts_without_pausing() {
        let temp = TempDir::new().expect("temp");
        let mut store = small_store(temp.path(), 2_000);
        add_test_frame(&mut store, 1, 10, matched("fixture01/home"), "initial");
        let outcome = add_test_frame(
            &mut store,
            2,
            30,
            matched("fixture01/terminal"),
            "page_wait",
        );
        assert!(store.spilled_count > 0);
        assert!(!outcome.pause_required);
        for entry in &store.entries {
            assert_eq!(entry.storage_state, FrameStorageState::Artifact);
            assert_eq!(
                entry.original_png().expect("verified material").len() as u64,
                entry.material.byte_count
            );
        }
        assert_resident_accounting(&store);
    }

    #[test]
    fn spilled_frame_keeps_thumbnail_for_later_dedup() {
        let temp = TempDir::new().expect("temp");
        let mut store = small_store(temp.path(), 2_000);

        store.config.tier1_ratio = 0.005;
        store.config.tier2_ratio = 0.01;
        let first = add_test_frame(&mut store, 1, 10, matched("fixture01/home"), "initial");
        assert_eq!(first.storage_state, FrameStorageState::Artifact);
        add_test_frame(&mut store, 2, 10, matched("fixture01/home"), "page_wait");

        let screenshots = store.screenshots();
        assert_eq!(screenshots.len(), 1);
        assert_eq!(screenshots[0].merged_count, 1);
        assert_resident_accounting(&store);
    }

    #[test]
    fn single_frame_can_spill() {
        let temp = TempDir::new().expect("temp");
        let mut store = small_store(temp.path(), 2_000);

        store.config.tier2_ratio = 0.01;
        store.config.tier1_ratio = 0.005;
        let outcome = add_test_frame(&mut store, 1, 10, matched("fixture01/home"), "initial");

        assert_eq!(outcome.storage_state, FrameStorageState::Artifact);
        assert!(!outcome.pause_required);
    }

    #[test]
    fn persisted_frame_reads_its_original_verified_material() {
        let temp = TempDir::new().expect("temp");
        let mut store = small_store(temp.path(), 1_200);
        add_test_frame(&mut store, 1, 10, matched("fixture01/home"), "initial");
        assert_eq!(store.entries[0].storage_state, FrameStorageState::Artifact);
        let png = store.entries[0].original_png().expect("verified original");
        assert_eq!(
            crate::store::canonical_sha256(&png),
            store.entries[0].material.sha256
        );
    }

    #[test]
    fn last_frame_can_spill_when_eligible() {
        let temp = TempDir::new().expect("temp");
        let mut store = small_store(temp.path(), 2_000);

        add_test_frame(&mut store, 1, 10, matched("fixture01/home"), "initial");
        add_test_frame(
            &mut store,
            2,
            20,
            matched("fixture01/terminal"),
            "page_wait",
        );

        assert!(
            store
                .screenshots()
                .iter()
                .any(|record| record.storage_state == FrameStorageState::Artifact)
        );
    }

    #[test]
    fn tier3_returns_pause_required_on_current_frame() {
        let temp = TempDir::new().expect("temp");
        let mut store = small_store(temp.path(), 80);

        let outcome = add_test_frame(&mut store, 1, 10, RecognitionState::Pending, "page_wait");

        assert!(outcome.tier3_triggered);
        assert!(outcome.pause_required);
        assert!(outcome.checkpoint.is_some());
    }

    #[test]
    fn resident_bytes_include_payload_metadata_thumbnail_and_workspace() {
        let temp = TempDir::new().expect("temp");
        let mut store = small_store(temp.path(), 120_000);

        add_test_frame(&mut store, 1, 10, matched("fixture01/home"), "initial");

        let diagnostics = store.diagnostics_json();
        assert!(diagnostics["payload_bytes"].as_u64().unwrap() > 0);
        assert!(diagnostics["metadata_estimated_bytes"].as_u64().unwrap() > 0);
        assert!(diagnostics["thumbnail_estimated_bytes"].as_u64().unwrap() > 0);
        // Encoding is complete; its temporary reservation has been released.
        assert_eq!(
            diagnostics["encoder_workspace_reserved_bytes"].as_u64(),
            Some(0)
        );
        if let FrameStorage::Resident(frame) = &store.entries[0].storage {
            let mut raw = frame.clone();
            raw.original_png = None;
            assert!(raw.artifact_png_workspace_bytes().expect("codec workspace") > 0);
        }
    }

    #[test]
    fn spill_capacity_refusal_retains_the_original_without_panic() {
        let temp = TempDir::new().expect("temp");
        let mut store = small_store(temp.path(), 90);
        store.fixture_capacity_limit = Some(0);
        let outcome = add_test_frame(&mut store, 1, 10, matched("fixture01/home"), "initial");
        assert_eq!(outcome.frame_failures.len(), 1);
        assert_eq!(
            outcome.frame_failures[0].error.code(),
            "capacity_admission_refused"
        );
        assert!(!outcome.frame_failures[0].error.is_fatal());
        assert_eq!(outcome.backpressure_state, BackpressureState::SpillDegraded);
        assert_eq!(store.spill_warning_count, 1);
        assert!(store.entries.iter().all(|entry| !entry.spill_failed));
        assert!(store.entries[0].original_png().is_ok());
        assert_eq!(store.encoder_workspace_reserved_bytes, 0);
        assert_resident_accounting(&store);
    }

    #[test]
    fn spill_preserves_original_identity_failure_before_publication() {
        let temp = TempDir::new().expect("temp");
        let config = test_config(10_000_000).with_memory_sample(MemorySample {
            total_bytes: 20_000_000,
            available_bytes: 20_000_000,
        });
        let mut store = FrameStore::new(temp.path().join("temp"), config).expect("store");
        add_test_frame(&mut store, 1, 10, matched("fixture01/home"), "initial");
        add_test_frame(
            &mut store,
            2,
            20,
            matched("fixture01/terminal"),
            "page_wait",
        );
        if let FrameStorage::Resident(frame) = &mut store.entries[0].storage {
            frame.original_png.as_mut().expect("original PNG")[0] ^= 1;
        }
        let mut publications = 0;
        let mut failures = Vec::new();
        let error = store
            .flush_resident_frames(
                &mut |_| {
                    publications += 1;
                    Err(CliError::fatal(
                        "artifact_directory_failed",
                        "store_artifact",
                        "blocked publication",
                    ))
                },
                &mut Vec::new(),
                &mut failures,
                &mut false,
            )
            .expect_err("original failure");
        assert_eq!(error.code(), "frame_material_hash_mismatch");
        assert!(store.entries[0].spill_failed);
        assert!(!store.entries[1].spill_failed);
        assert_eq!(publications, 0);
        assert!(failures.is_empty());
        assert_resident_accounting(&store);
    }

    #[test]
    fn cleanup_releases_memory_and_preserves_existing_material() {
        let temp = TempDir::new().expect("temp");
        let mut store = small_store(temp.path(), 1_200);
        add_test_frame(&mut store, 1, 10, matched("fixture01/home"), "initial");
        let original = store.entries[0]
            .original_png()
            .expect("original")
            .into_owned();
        let old_material = temp.path().join("preserved-frame-material");
        fs::write(&old_material, b"retained").expect("old material");
        store.cleanup_temp().expect("release resident ownership");
        assert_eq!(
            fs::read(&old_material).expect("old material retained"),
            b"retained"
        );
        assert_eq!(
            store.entries[0]
                .original_png()
                .expect("verified artifact")
                .as_ref(),
            original.as_slice()
        );
    }

    #[test]
    fn hysteresis_releases_only_below_release_line() {
        let temp = TempDir::new().expect("temp");
        let mut store = small_store(temp.path(), 120_000);
        store.tier1_active = true;

        store.resident_bytes = store.budget.tier1_bytes - 1;
        store.release_watermarks_if_needed();
        assert!(store.tier1_active);

        store.resident_bytes = store.budget.tier1_release_bytes;
        store.release_watermarks_if_needed();
        assert!(!store.tier1_active);
    }

    #[test]
    fn tier3_alarm_still_preserves_partial_original_material() {
        let temp = TempDir::new().expect("temp");
        let mut store = small_store(temp.path(), 80);
        let outcome = add_test_frame(&mut store, 1, 10, RecognitionState::Pending, "initial");
        assert!(outcome.tier3_triggered);
        let material = store.entries[0].original_png().expect("partial original");
        assert_eq!(
            crate::store::canonical_sha256(&material),
            store.entries[0].material.sha256
        );
    }

    #[test]
    fn clock_rollback_does_not_underflow_dwell_delta() {
        let temp = TempDir::new().expect("temp");
        let mut store = small_store(temp.path(), 1_000);
        let later = SystemTime::UNIX_EPOCH + Duration::from_secs(10);
        let earlier = SystemTime::UNIX_EPOCH + Duration::from_secs(5);

        add_test_frame_at(
            &mut store,
            1,
            10,
            matched("fixture01/home"),
            "initial",
            later,
        );
        add_test_frame_at(
            &mut store,
            2,
            10,
            matched("fixture01/home"),
            "page_wait",
            earlier,
        );

        assert_eq!(store.entries[1].delta_from_previous_ms, 0);
    }

    #[test]
    fn thumbnail_handles_pathological_dimensions_without_panic() {
        let frame = Frame::from_pixels(
            1,
            1,
            vec![0, 0, 0, 255],
            PixelFormat::Rgba8,
            CaptureBackendName::NemuIpc,
        )
        .expect("frame");

        let thumb = thumbnail(&frame);

        assert_eq!(thumb.values.len(), THUMB_WIDTH * THUMB_HEIGHT);
    }
    fn matched(page_id: &str) -> RecognitionState {
        RecognitionState::Matched {
            page_id: page_id.to_string(),
        }
    }

    fn test_config(max_mem_bytes: u64) -> FrameStoreConfig {
        FrameStoreConfig {
            max_mem_bytes: Some(max_mem_bytes),
            os_reserve_bytes: 0,
            tier1_ratio: 0.50,
            tier2_ratio: 0.70,
            tier3_ratio: 0.90,
            flush_workspace_reserve_bytes: 1,
            ..Default::default()
        }
        .with_memory_sample(MemorySample {
            total_bytes: 1_000_000,
            available_bytes: 1_000_000,
        })
    }

    fn small_store(path: &Path, max_mem_bytes: u64) -> FrameStore {
        let mut config = test_config(max_mem_bytes);
        // Pressure thresholds still model the original tiny fixture; the hard limit
        // also admits one actual verification buffer and the fixture's held frames.
        let hard = max_mem_bytes.max(WRITER_BUFFER_BYTES + 8 * 1024);
        let ratio = max_mem_bytes as f64 / hard as f64;
        config.max_mem_bytes = Some(hard);
        config.tier1_ratio *= ratio;
        config.tier2_ratio *= ratio;
        config.tier3_ratio *= ratio;
        FrameStore::new(path.join("temp"), config).expect("store")
    }

    fn add_test_frame(
        store: &mut FrameStore,
        index: usize,
        shade: u8,
        recognition_state: RecognitionState,
        label: &str,
    ) -> FrameStoreOutcome {
        add_test_frame_at(
            store,
            index,
            shade,
            recognition_state,
            label,
            SystemTime::now(),
        )
    }

    fn add_test_frame_at(
        store: &mut FrameStore,
        index: usize,
        shade: u8,
        recognition_state: RecognitionState,
        label: &str,
        captured_at: SystemTime,
    ) -> FrameStoreOutcome {
        let mut frame = Frame::from_pixels(
            4,
            4,
            vec![shade; 4 * 4 * 4],
            PixelFormat::Rgba8,
            CaptureBackendName::NemuIpc,
        )
        .expect("frame");
        frame.captured_at = captured_at;
        frame.original_png = Some(frame.encode_png_fast().expect("fixture original PNG"));
        let artifacts =
            Arc::new(ArtifactStore::open(store.fixture_root.join("materials")).expect("store"));
        let capacity = crate::store::tests::RecordingSink {
            capacity_root: Some(artifacts.root().to_path_buf()),
            capacity_limit: store.fixture_capacity_limit,
            ..Default::default()
        };
        artifacts
            .install_capacity_admission(Arc::new(capacity))
            .expect("fixture capacity");
        let mut sink = crate::store::tests::RecordingSink::default();
        store
            .add_frame(
                FrameStoreFrameInput {
                    frame_index: index,
                    file_name: format!("frame{index}.png"),
                    label: label.to_string(),
                    recognition_state,
                    pinned_reason: None,
                    frame,
                },
                &mut |candidate| {
                    use actingcommand_contract::{
                        ArtifactIssuePolicy, ArtifactKind, ArtifactLinksDraft, ArtifactProducer,
                        ArtifactRedactionState, EventLinksDraft, IdentifierIssuer, RetentionClass,
                    };
                    let ids = IdentifierIssuer::new().expect("fixture identity");
                    let frame_id = ids.mint_frame_id().expect("fixture frame");
                    let context = crate::ArtifactWriteContext::new(
                        ArtifactLinksDraft::default().with_frame_id(frame_id),
                        EventLinksDraft::default().with_frame_id(frame_id),
                        1,
                    );
                    let artifact = artifacts.put(
                        crate::ArtifactWriteRequest::new(
                            ArtifactKind::CaptureFrame,
                            &candidate.png,
                            context,
                            ArtifactIssuePolicy::new(
                                ArtifactProducer::CapturePipeline,
                                RetentionClass::Adaptive,
                                ArtifactRedactionState::NotRequired,
                            ),
                        ),
                        &mut sink,
                    )?;
                    Ok((Arc::clone(&artifacts), artifact.reference().clone()))
                },
            )
            .expect("add frame")
    }
    fn assert_resident_accounting(store: &FrameStore) {
        let mut estimate = ResidentEstimate::default();
        for entry in &store.entries {
            estimate.payload = estimate
                .payload
                .saturating_add(entry.resident_estimate.payload);
            estimate.metadata = estimate
                .metadata
                .saturating_add(entry.resident_estimate.metadata);
            estimate.thumbnail = estimate
                .thumbnail
                .saturating_add(entry.resident_estimate.thumbnail);
            estimate.encoder_workspace = estimate
                .encoder_workspace
                .saturating_add(entry.resident_estimate.encoder_workspace);
        }

        assert_eq!(store.resident_bytes, estimate.total());
        assert_eq!(store.payload_bytes, estimate.payload);
        assert_eq!(store.metadata_estimated_bytes, estimate.metadata);
        assert_eq!(store.thumbnail_estimated_bytes, estimate.thumbnail);
        assert_eq!(
            store.encoder_workspace_reserved_bytes,
            estimate.encoder_workspace
        );
    }
}
