// SPDX-License-Identifier: AGPL-3.0-only

use crate::{CliError, CliOutcome};
use actingcommand_device::{Frame, PixelFormat};
use serde::Deserialize;
use serde_json::{Value, json};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use zip::write::FileOptions;
use zip::{ZipArchive, ZipWriter};

const DEFAULT_SIMILARITY_THRESHOLD: f32 = 0.95;
const DEFAULT_TIER1_RATIO: f64 = 0.60;
const DEFAULT_TIER2_RATIO: f64 = 0.75;
const DEFAULT_TIER3_RATIO: f64 = 0.90;
const DEFAULT_HYSTERESIS_RATIO: f64 = 0.10;
const DEFAULT_OS_RESERVE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const DEFAULT_FLUSH_WORKSPACE_RESERVE_BYTES: u64 = 8 * 1024 * 1024;
const ENTRY_BASE_METADATA_BYTES: u64 = 512;
const SEGMENT_METADATA_BYTES: u64 = 256;
const WRITER_BUFFER_BYTES: u64 = 64 * 1024;
const THUMB_WIDTH: usize = 16;
const THUMB_HEIGHT: usize = 9;

#[derive(Debug, Clone, Default, Deserialize)]
pub(super) struct FrameStoreControl {
    #[serde(default)]
    pub(super) similarity_threshold: Option<f32>,
    #[serde(default)]
    pub(super) tier1_ratio: Option<f64>,
    #[serde(default)]
    pub(super) tier2_ratio: Option<f64>,
    #[serde(default)]
    pub(super) tier3_ratio: Option<f64>,
    #[serde(default)]
    pub(super) hysteresis_ratio: Option<f64>,
    #[serde(default)]
    pub(super) max_mem_bytes: Option<u64>,
    #[serde(default)]
    pub(super) os_reserve_bytes: Option<u64>,
    #[serde(default)]
    pub(super) flush_workspace_reserve_bytes: Option<u64>,
}

impl FrameStoreControl {
    pub(super) fn validate(&self) -> Result<(), String> {
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

    pub(super) fn apply_to(&self, config: &mut FrameStoreConfig) {
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
pub(super) struct FrameStoreConfig {
    pub(super) similarity_threshold: f32,
    pub(super) tier1_ratio: f64,
    pub(super) tier2_ratio: f64,
    pub(super) tier3_ratio: f64,
    pub(super) hysteresis_ratio: f64,
    pub(super) max_mem_bytes: Option<u64>,
    pub(super) os_reserve_bytes: u64,
    pub(super) flush_workspace_reserve_bytes: u64,
    memory_sample_override: Option<MemorySample>,
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
            memory_sample_override: None,
        }
    }
}

impl FrameStoreConfig {
    pub(super) fn validate(&self) -> Result<(), String> {
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

    #[cfg(test)]
    fn with_memory_sample(mut self, sample: MemorySample) -> Self {
        self.memory_sample_override = Some(sample);
        self
    }

    fn memory_sample(&self) -> CliOutcome<MemorySample> {
        match self.memory_sample_override {
            Some(sample) => Ok(sample),
            None => sample_system_memory(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct MemorySample {
    pub(super) total_bytes: u64,
    pub(super) available_bytes: u64,
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
pub(super) enum RecognitionState {
    Pending,
    Matched { page_id: String },
    CompletedNoMatch,
    Failed { reason: String },
}

impl RecognitionState {
    pub(super) fn from_matched_page(matched_page: Option<String>) -> Self {
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

    pub(super) fn as_json(&self) -> Value {
        match self {
            Self::Pending => json!({"state": "pending"}),
            Self::Matched { page_id } => json!({"state": "matched", "page_id": page_id}),
            Self::CompletedNoMatch => json!({"state": "completed_no_match"}),
            Self::Failed { reason } => json!({"state": "failed", "reason": reason}),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BackpressureState {
    Normal,
    Tier1Dedup,
    Tier2Flush,
    Tier3Paused,
    Tier3Resumable,
    SpillDegraded,
}

impl BackpressureState {
    pub(super) fn as_str(self) -> &'static str {
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
pub(super) enum FrameStorageState {
    Memory,
    Segment,
    Dropped,
}

impl FrameStorageState {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Segment => "segment",
            Self::Dropped => "dropped",
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct Tier3PauseCheckpoint {
    pub(super) last_frame_index: usize,
    pub(super) resident_bytes: u64,
    pub(super) tier1_bytes: u64,
    pub(super) tier2_bytes: u64,
    pub(super) tier3_bytes: u64,
    pub(super) active_segment_id: Option<u64>,
    pub(super) in_flight_flush_state: String,
}

impl Tier3PauseCheckpoint {
    pub(super) fn to_json(&self) -> Value {
        json!({
            "last_frame_index": self.last_frame_index,
            "resident_bytes": self.resident_bytes,
            "tier1_bytes": self.tier1_bytes,
            "tier2_bytes": self.tier2_bytes,
            "tier3_bytes": self.tier3_bytes,
            "active_segment_id": self.active_segment_id,
            "in_flight_flush_state": self.in_flight_flush_state
        })
    }
}

pub(super) struct FrameStore {
    config: FrameStoreConfig,
    temp_dir: PathBuf,
    segment_manifest_path: PathBuf,
    budget: MemoryBudget,
    resident_bytes: u64,
    payload_bytes: u64,
    metadata_estimated_bytes: u64,
    thumbnail_estimated_bytes: u64,
    encoder_workspace_reserved_bytes: u64,
    spilled_bytes: u64,
    dropped_bytes: u64,
    entries: Vec<FrameEntry>,
    timeline: Vec<Value>,
    tier1_active: bool,
    tier2_active: bool,
    tier3_active: bool,
    dropped_count: u64,
    spilled_count: u64,
    spill_warning_count: u64,
    next_segment_id: u64,
    active_segment_id: Option<u64>,
}

impl FrameStore {
    pub(super) fn new(temp_dir: PathBuf, config: FrameStoreConfig) -> CliOutcome<Self> {
        let budget = MemoryBudget::build(&config)?;
        let segment_manifest_path = temp_dir.join("segment-manifest.jsonl");
        Ok(Self {
            config,
            temp_dir,
            segment_manifest_path,
            budget,
            resident_bytes: 0,
            payload_bytes: 0,
            metadata_estimated_bytes: 0,
            thumbnail_estimated_bytes: 0,
            encoder_workspace_reserved_bytes: 0,
            spilled_bytes: 0,
            dropped_bytes: 0,
            entries: Vec::new(),
            timeline: Vec::new(),
            tier1_active: false,
            tier2_active: false,
            tier3_active: false,
            dropped_count: 0,
            spilled_count: 0,
            spill_warning_count: 0,
            next_segment_id: 1,
            active_segment_id: None,
        })
    }

    pub(super) fn set_config(&mut self, config: FrameStoreConfig) -> CliOutcome<()> {
        let budget = MemoryBudget::build(&config)?;
        self.config = config;
        self.budget = budget;
        Ok(())
    }

    pub(super) fn add_frame(
        &mut self,
        input: FrameStoreFrameInput,
    ) -> CliOutcome<FrameStoreOutcome> {
        self.refresh_budget()?;
        self.release_watermarks_if_needed();
        let mut warnings = Vec::new();
        let file = format!("screenshots/{}", input.file_name);
        let key_frame = self.is_key_frame(&input);
        let thumb = thumbnail(&input.frame);
        let estimate = estimate_entry(&input, &file, &thumb);
        let width = input.frame.width;
        let height = input.frame.height;
        let captured_at = input.frame.captured_at;
        let backend = input.frame.backend_name.as_str().to_string();
        let pixel_format = input.frame.pixel_format.as_str().to_string();
        let mut backpressure_state = BackpressureState::Normal;

        let projected = self.resident_bytes.saturating_add(estimate.total());
        if projected >= self.budget.tier1_bytes {
            self.activate_tier1(projected);
            self.dedup_existing();
            backpressure_state = BackpressureState::Tier1Dedup;
        }
        let projected = self.resident_bytes.saturating_add(estimate.total());
        if projected >= self.budget.tier2_bytes {
            self.activate_tier2(projected);
            self.flush_resident_segment(&mut warnings);
            backpressure_state = BackpressureState::Tier2Flush;
        }

        let projected = self.resident_bytes.saturating_add(estimate.total());
        let mut storage = FrameStorage::Resident(input.frame);
        let mut resident_estimate = estimate;
        let mut storage_state = FrameStorageState::Memory;
        let mut tier3_triggered = false;
        let mut pause_required = false;
        let mut admission_spill_warning = None;
        let mut admission_spill_failed = false;

        if projected >= self.budget.tier3_bytes {
            tier3_triggered = true;
            self.activate_tier3(projected);
            if input.recognition_state.can_spill() {
                match self.spill_admission_frame(&storage, &input.file_name, &file) {
                    Ok(Some(spilled)) => {
                        storage = spilled;
                        resident_estimate = estimate.spilled_resident();
                        storage_state = FrameStorageState::Segment;
                        backpressure_state = BackpressureState::Tier2Flush;
                    }
                    Ok(None) => {}
                    Err(message) => {
                        warnings.push(message.clone());
                        admission_spill_failed = message.starts_with("spill_degraded");
                        admission_spill_warning = Some(message);
                        resident_estimate = estimate.without_encoder_workspace();
                        backpressure_state = BackpressureState::SpillDegraded;
                    }
                }
            }
        }

        let segment_id = storage.segment_id();
        let segment_path = storage.segment_path();
        let entry = FrameEntry {
            frame_index: input.frame_index,
            file_name: input.file_name,
            file: file.clone(),
            width,
            height,
            captured_at,
            backend,
            pixel_format,
            label: input.label,
            recognition_state: input.recognition_state,
            key_frame,
            merged_count: 0,
            dwell_ms: 0,
            delta_from_previous_ms: self.delta_from_previous_ms(captured_at),
            retained: true,
            merged_into: None,
            storage,
            storage_state,
            resident_estimate,
            thumb,
            segment_id,
            segment_path,
            spill_attempted: storage_state == FrameStorageState::Segment,
            spill_failed: admission_spill_failed,
        };
        self.add_estimate(entry.resident_estimate);
        self.entries.push(entry);
        let entry_index = self.entries.len() - 1;
        if let Some(message) = &admission_spill_warning {
            if admission_spill_failed {
                self.record_spill_warning(input.frame_index, &file, message);
            } else {
                self.record_spill_unavailable_warning(message);
            }
        }
        self.timeline.push(json!({
            "event": "frame_retained",
            "frame_index": self.entries[entry_index].frame_index,
            "file": file,
            "key_frame": key_frame,
            "recognition_state": self.entries[entry_index].recognition_state.as_json(),
            "storage": self.entries[entry_index].storage_state.as_str(),
            "resident_bytes": self.resident_bytes
        }));

        if self.tier1_active {
            self.dedup_existing();
        }
        if self.tier2_active {
            self.flush_resident_segment(&mut warnings);
        }

        if self.resident_bytes >= self.budget.tier3_bytes {
            tier3_triggered = true;
            pause_required = true;
            self.activate_tier3(self.resident_bytes);
            self.flush_resident_segment(&mut warnings);
            if self.resident_bytes <= self.budget.tier3_release_bytes {
                backpressure_state = BackpressureState::Tier3Resumable;
                pause_required = false;
            } else if !matches!(backpressure_state, BackpressureState::SpillDegraded) {
                backpressure_state = BackpressureState::Tier3Paused;
            }
        }
        self.release_watermarks_if_needed();

        if self.tier3_active && self.resident_bytes > self.budget.tier3_release_bytes {
            pause_required = true;
        }
        let retained = self.entries[entry_index].retained;
        Ok(FrameStoreOutcome {
            retained,
            file: retained.then(|| self.entries[entry_index].file.clone()),
            merged_into: self.entries[entry_index].merged_into.clone(),
            storage_state: self.entries[entry_index].storage_state,
            tier1_active: self.tier1_active,
            tier2_active: self.tier2_active,
            tier3_triggered,
            backpressure_state,
            pause_required,
            warnings,
            checkpoint: tier3_triggered.then(|| self.pause_checkpoint(input.frame_index)),
        })
    }

    pub(super) fn materialize(&mut self, screenshots_dir: &Path) -> CliOutcome<()> {
        fs::create_dir_all(screenshots_dir).map_err(|err| {
            CliError::package_invalid(format!(
                "failed to create {}: {err}",
                screenshots_dir.display()
            ))
        })?;
        for entry in &mut self.entries {
            if !entry.retained {
                continue;
            }
            let destination = screenshots_dir.join(&entry.file_name);
            match &entry.storage {
                FrameStorage::Resident(frame) => {
                    let png = frame
                        .png_for_artifact()
                        .map_err(|err| CliError::device(err.to_string()))?;
                    fs::write(&destination, png).map_err(|err| {
                        CliError::package_invalid(format!(
                            "failed to write {}: {err}",
                            destination.display()
                        ))
                    })?;
                }
                FrameStorage::Spilled {
                    segment_path,
                    zip_name,
                    ..
                } => {
                    let png = read_segment_frame(segment_path, zip_name)?;
                    fs::write(&destination, png).map_err(|err| {
                        CliError::package_invalid(format!(
                            "failed to write {}: {err}",
                            destination.display()
                        ))
                    })?;
                }
                FrameStorage::Dropped => {}
            }
        }
        Ok(())
    }

    pub(super) fn cleanup_temp(&mut self) -> Vec<String> {
        if !self.temp_dir.exists() {
            return Vec::new();
        }
        match fs::remove_dir_all(&self.temp_dir) {
            Ok(()) => {
                self.timeline.push(json!({
                    "event": "frame_store_temp_cleaned",
                    "path": self.temp_dir
                }));
                Vec::new()
            }
            Err(err) => {
                let warning = format!(
                    "failed to clean frame store temp {}: {err}",
                    self.temp_dir.display()
                );
                self.timeline.push(json!({
                    "event": "frame_store_temp_cleanup_failed",
                    "warning": warning
                }));
                vec![warning]
            }
        }
    }

    pub(super) fn screenshots(&self) -> Vec<FrameStoreScreenshot> {
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
                storage_state: entry.storage_state,
            })
            .collect()
    }

    pub(super) fn diagnostics_json(&self) -> Value {
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
            "tier3_active": self.tier3_active,
            "active_segment_id": self.active_segment_id
        })
    }

    pub(super) fn timeline(&self) -> Vec<Value> {
        let mut rows = self.timeline.clone();
        rows.extend(self.entries.iter().map(|entry| {
            json!({
                "event": "frame_final",
                "frame_index": entry.frame_index,
                "file": entry.file,
                "retained": entry.retained,
                "merged_into": entry.merged_into,
                "recognition_state": entry.recognition_state.as_json(),
                "label": entry.label,
                "backend": entry.backend,
                "pixel_format": entry.pixel_format,
                "key_frame": entry.key_frame,
                "dwell_ms": entry.dwell_ms,
                "merged_count": entry.merged_count,
                "storage": entry.storage_state.as_str(),
                "segment_id": entry.segment_id,
                "segment_path": entry.segment_path.as_ref().map(|path| path.display().to_string()),
                "resident_bytes_estimate": entry.resident_estimate.total(),
                "metadata_bytes_estimate": entry.resident_estimate.metadata,
                "thumb_bytes_estimate": entry.resident_estimate.thumbnail,
                "encoder_workspace_bytes_estimate": entry.resident_estimate.encoder_workspace
            })
        }));
        rows
    }

    fn refresh_budget(&mut self) -> CliOutcome<()> {
        self.budget = MemoryBudget::build(&self.config)?;
        Ok(())
    }

    fn activate_tier1(&mut self, projected_bytes: u64) {
        if !self.tier1_active {
            self.tier1_active = true;
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

    fn dedup_existing(&mut self) {
        let mut previous_retained = None;
        for index in 0..self.entries.len() {
            if !self.entries[index].retained {
                continue;
            }
            let should_keep = self.entries[index].key_frame
                || previous_retained
                    .is_none_or(|previous| !self.same_page_duplicate(previous, index));
            if should_keep {
                previous_retained = Some(index);
            } else if let Some(previous) = previous_retained {
                self.drop_entry(index, previous);
            }
        }
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
        let released =
            match std::mem::replace(&mut self.entries[index].storage, FrameStorage::Dropped) {
                FrameStorage::Resident(_) | FrameStorage::Spilled { .. } => {
                    self.replace_resident_estimate(index, ResidentEstimate::default())
                }
                FrameStorage::Dropped => 0,
            };
        self.entries[index].thumb.values.clear();
        released
    }

    fn flush_resident_segment(&mut self, warnings: &mut Vec<String>) {
        let indexes = self.spillable_indexes();
        if indexes.is_empty() {
            return;
        }
        match self.write_segment(indexes) {
            Ok(report) => {
                for failure in report.frame_failures {
                    warnings.push(failure.message.clone());
                    let frame_index = self.entries[failure.index].frame_index;
                    let file = self.entries[failure.index].file.clone();
                    self.record_spill_warning(frame_index, &file, &failure.message);
                }
            }
            Err(message) => {
                warnings.push(message.clone());
                self.record_spill_unavailable_warning(&message);
            }
        }
    }

    fn spillable_indexes(&self) -> Vec<usize> {
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                (entry.retained
                    && !entry.spill_failed
                    && entry.recognition_state.can_spill()
                    && matches!(entry.storage, FrameStorage::Resident(_)))
                .then_some(index)
            })
            .collect()
    }

    fn write_segment(&mut self, indexes: Vec<usize>) -> Result<SegmentWriteReport, String> {
        let mut encoded = Vec::new();
        let mut frame_failures = Vec::new();
        for index in indexes {
            let FrameStorage::Resident(frame) = &self.entries[index].storage else {
                continue;
            };
            match frame.png_for_artifact() {
                Ok(png) => encoded.push((index, png)),
                Err(err) => frame_failures.push(SegmentFrameFailure {
                    index,
                    message: format!("spill_degraded: failed to encode frame: {err}"),
                }),
            }
        }
        if encoded.is_empty() {
            return Ok(SegmentWriteReport { frame_failures });
        }

        fs::create_dir_all(&self.temp_dir).map_err(|err| {
            format!(
                "spill_unavailable: failed to create {}: {err}",
                self.temp_dir.display()
            )
        })?;
        let segment_id = self.next_segment_id;
        self.next_segment_id = self.next_segment_id.saturating_add(1);
        let segment_path = self.temp_dir.join(format!("segment-{segment_id:06}.zip"));
        let file = File::create(&segment_path).map_err(|err| {
            format!(
                "spill_unavailable: failed to create {}: {err}",
                segment_path.display()
            )
        })?;
        self.active_segment_id = Some(segment_id);
        let result = (|| -> Result<Vec<(usize, Vec<u8>)>, String> {
            let options =
                FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            let mut zip = ZipWriter::new(file);
            let mut manifest = Vec::new();
            for (index, png) in &encoded {
                let zip_name = self.entries[*index].file_name.clone();
                zip.start_file(&zip_name, options).map_err(|err| {
                    format!("spill_unavailable: failed to start segment entry: {err}")
                })?;
                zip.write_all(png).map_err(|err| {
                    format!("spill_unavailable: failed to write segment frame bytes: {err}")
                })?;
                manifest.push(json!({
                    "frame_index": self.entries[*index].frame_index,
                    "file_name": self.entries[*index].file_name,
                    "zip_name": zip_name,
                    "recognition_state": self.entries[*index].recognition_state.as_json(),
                    "dwell_ms": self.entries[*index].dwell_ms,
                    "merged_count": self.entries[*index].merged_count
                }));
            }
            zip.start_file("segment-manifest.json", options)
                .map_err(|err| {
                    format!("spill_unavailable: failed to start segment manifest: {err}")
                })?;
            zip.write_all(
                serde_json::to_string_pretty(&manifest)
                    .map_err(|err| format!("spill_degraded: failed to serialize manifest: {err}"))?
                    .as_bytes(),
            )
            .map_err(|err| format!("spill_unavailable: failed to write segment manifest: {err}"))?;
            zip.finish()
                .map_err(|err| format!("spill_unavailable: failed to finish segment: {err}"))?;
            self.append_segment_manifest(segment_id, &segment_path, &manifest)
                .map_err(|err| {
                    format!("spill_unavailable: failed to append segment manifest: {err}")
                })?;
            Ok(encoded)
        })();
        let encoded = match result {
            Ok(encoded) => encoded,
            Err(message) => {
                self.active_segment_id = None;
                let _ = fs::remove_file(&segment_path);
                return Err(message);
            }
        };

        for (index, png) in encoded {
            self.mark_spilled(index, segment_id, &segment_path, png.len() as u64);
        }
        self.active_segment_id = None;
        Ok(SegmentWriteReport { frame_failures })
    }

    fn spill_admission_frame(
        &mut self,
        storage: &FrameStorage,
        file_name: &str,
        file: &str,
    ) -> Result<Option<FrameStorage>, String> {
        let FrameStorage::Resident(frame) = storage else {
            return Ok(None);
        };
        fs::create_dir_all(&self.temp_dir).map_err(|err| {
            format!(
                "spill_unavailable: failed to create {}: {err}",
                self.temp_dir.display()
            )
        })?;
        let segment_id = self.next_segment_id;
        self.next_segment_id = self.next_segment_id.saturating_add(1);
        let segment_path = self.temp_dir.join(format!("segment-{segment_id:06}.zip"));
        let file_handle = File::create(&segment_path).map_err(|err| {
            format!(
                "spill_unavailable: failed to create {}: {err}",
                segment_path.display()
            )
        })?;
        self.active_segment_id = Some(segment_id);
        let result = (|| -> Result<usize, String> {
            let options =
                FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            let mut zip = ZipWriter::new(file_handle);
            let png = frame
                .png_for_artifact()
                .map_err(|err| format!("spill_degraded: failed to encode frame: {err}"))?;
            zip.start_file(file_name, options).map_err(|err| {
                format!("spill_unavailable: failed to start segment entry: {err}")
            })?;
            zip.write_all(&png).map_err(|err| {
                format!("spill_unavailable: failed to write segment frame bytes: {err}")
            })?;
            let manifest = vec![json!({
                "file": file,
                "file_name": file_name,
                "zip_name": file_name
            })];
            zip.start_file("segment-manifest.json", options)
                .map_err(|err| {
                    format!("spill_unavailable: failed to start segment manifest: {err}")
                })?;
            zip.write_all(
                serde_json::to_string_pretty(&manifest)
                    .map_err(|err| format!("spill_degraded: failed to serialize manifest: {err}"))?
                    .as_bytes(),
            )
            .map_err(|err| format!("spill_unavailable: failed to write segment manifest: {err}"))?;
            zip.finish()
                .map_err(|err| format!("spill_unavailable: failed to finish segment: {err}"))?;
            self.append_segment_manifest(segment_id, &segment_path, &manifest)
                .map_err(|err| {
                    format!("spill_unavailable: failed to append segment manifest: {err}")
                })?;
            Ok(png.len())
        })();
        let png_len = match result {
            Ok(png_len) => png_len,
            Err(message) => {
                self.active_segment_id = None;
                return Err(message);
            }
        };
        self.spilled_count = self.spilled_count.saturating_add(1);
        self.spilled_bytes = self.spilled_bytes.saturating_add(png_len as u64);
        self.active_segment_id = None;
        Ok(Some(FrameStorage::Spilled {
            segment_id,
            segment_path,
            zip_name: file_name.to_string(),
        }))
    }

    fn append_segment_manifest(
        &self,
        segment_id: u64,
        segment_path: &Path,
        manifest: &[Value],
    ) -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.segment_manifest_path)?;
        for entry in manifest {
            let row = json!({
                "segment_id": segment_id,
                "segment_path": segment_path,
                "entry": entry
            });
            writeln!(file, "{row}")?;
        }
        Ok(())
    }

    fn mark_spilled(&mut self, index: usize, segment_id: u64, segment_path: &Path, png_bytes: u64) {
        let new_estimate = self.entries[index].resident_estimate.metadata_only();
        let released = self.replace_resident_estimate(index, new_estimate);
        self.entries[index].storage = FrameStorage::Spilled {
            segment_id,
            segment_path: segment_path.to_path_buf(),
            zip_name: self.entries[index].file_name.clone(),
        };
        self.entries[index].storage_state = FrameStorageState::Segment;
        self.entries[index].segment_id = Some(segment_id);
        self.entries[index].segment_path = Some(segment_path.to_path_buf());
        self.entries[index].spill_attempted = true;
        self.spilled_count = self.spilled_count.saturating_add(1);
        self.spilled_bytes = self.spilled_bytes.saturating_add(png_bytes);
        self.timeline.push(json!({
            "event": "frame_spilled",
            "frame_index": self.entries[index].frame_index,
            "file": self.entries[index].file,
            "segment_id": segment_id,
            "segment_path": segment_path,
            "released_bytes": released,
            "resident_bytes": self.resident_bytes
        }));
    }

    fn record_spill_warning(&mut self, frame_index: usize, file: &str, warning: &str) {
        self.spill_warning_count = self.spill_warning_count.saturating_add(1);
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.frame_index == frame_index)
        {
            entry.spill_failed = true;
        }
        self.timeline.push(json!({
            "event": "spill_degraded",
            "frame_index": frame_index,
            "file": file,
            "warning": warning
        }));
    }

    fn record_spill_unavailable_warning(&mut self, warning: &str) {
        self.spill_warning_count = self.spill_warning_count.saturating_add(1);
        self.timeline.push(json!({
            "event": "spill_unavailable",
            "warning": warning
        }));
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
        label == "initial"
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
            active_segment_id: self.active_segment_id,
            in_flight_flush_state: if self.active_segment_id.is_some() {
                "segment_flush_active".to_string()
            } else {
                "idle".to_string()
            },
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

pub(super) struct FrameStoreFrameInput {
    pub(super) frame_index: usize,
    pub(super) file_name: String,
    pub(super) label: String,
    pub(super) recognition_state: RecognitionState,
    pub(super) frame: Frame,
}

pub(super) struct FrameStoreOutcome {
    pub(super) retained: bool,
    pub(super) file: Option<String>,
    pub(super) merged_into: Option<String>,
    pub(super) storage_state: FrameStorageState,
    pub(super) tier1_active: bool,
    pub(super) tier2_active: bool,
    pub(super) tier3_triggered: bool,
    pub(super) backpressure_state: BackpressureState,
    pub(super) pause_required: bool,
    pub(super) warnings: Vec<String>,
    pub(super) checkpoint: Option<Tier3PauseCheckpoint>,
}

#[derive(Debug, Clone)]
pub(super) struct FrameStoreScreenshot {
    pub(super) frame_index: usize,
    pub(super) file: String,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) dwell_ms: u64,
    pub(super) merged_count: u64,
    pub(super) matched_page: Option<String>,
    pub(super) recognition_state: RecognitionState,
    pub(super) key_frame: bool,
    pub(super) storage_state: FrameStorageState,
}

struct FrameEntry {
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
    merged_count: u64,
    dwell_ms: u64,
    delta_from_previous_ms: u64,
    retained: bool,
    merged_into: Option<String>,
    storage: FrameStorage,
    storage_state: FrameStorageState,
    resident_estimate: ResidentEstimate,
    thumb: Thumbnail,
    segment_id: Option<u64>,
    segment_path: Option<PathBuf>,
    spill_attempted: bool,
    spill_failed: bool,
}

struct SegmentWriteReport {
    frame_failures: Vec<SegmentFrameFailure>,
}

struct SegmentFrameFailure {
    index: usize,
    message: String,
}

enum FrameStorage {
    Resident(Frame),
    Spilled {
        segment_id: u64,
        segment_path: PathBuf,
        zip_name: String,
    },
    Dropped,
}

impl FrameStorage {
    fn segment_id(&self) -> Option<u64> {
        match self {
            Self::Spilled { segment_id, .. } => Some(*segment_id),
            Self::Resident(_) | Self::Dropped => None,
        }
    }

    fn segment_path(&self) -> Option<PathBuf> {
        match self {
            Self::Spilled { segment_path, .. } => Some(segment_path.clone()),
            Self::Resident(_) | Self::Dropped => None,
        }
    }
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
    fn total(self) -> u64 {
        self.payload
            .saturating_add(self.metadata)
            .saturating_add(self.thumbnail)
            .saturating_add(self.encoder_workspace)
    }

    fn metadata_only(self) -> Self {
        Self {
            payload: 0,
            metadata: self.metadata.saturating_add(SEGMENT_METADATA_BYTES),
            thumbnail: 0,
            encoder_workspace: 0,
        }
    }

    fn spilled_resident(self) -> Self {
        self.metadata_only()
    }

    fn without_encoder_workspace(self) -> Self {
        Self {
            encoder_workspace: 0,
            ..self
        }
    }
}

fn estimate_entry(input: &FrameStoreFrameInput, file: &str, thumb: &Thumbnail) -> ResidentEstimate {
    let original_png = input
        .frame
        .original_png
        .as_ref()
        .map(|png| png.len() as u64)
        .unwrap_or(0);
    let payload = input.frame.pixels.len() as u64 + original_png;
    let metadata = ENTRY_BASE_METADATA_BYTES
        + string_capacity_bytes(&input.file_name)
        + string_capacity_bytes(file)
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
    let thumbnail = thumb.values.capacity() as u64;
    let encoder_workspace = payload
        .max(input.frame.pixels.len() as u64)
        .saturating_add(WRITER_BUFFER_BYTES);
    ResidentEstimate {
        payload,
        metadata,
        thumbnail,
        encoder_workspace,
    }
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

fn read_segment_frame(segment_path: &Path, zip_name: &str) -> CliOutcome<Vec<u8>> {
    let file = File::open(segment_path).map_err(|err| {
        CliError::package_invalid(format!("failed to open {}: {err}", segment_path.display()))
    })?;
    let mut archive = ZipArchive::new(file).map_err(|err| {
        CliError::package_invalid(format!(
            "failed to read segment {}: {err}",
            segment_path.display()
        ))
    })?;
    let mut entry = archive.by_name(zip_name).map_err(|err| {
        CliError::package_invalid(format!(
            "failed to find {zip_name} in segment {}: {err}",
            segment_path.display()
        ))
    })?;
    let mut bytes = Vec::new();
    entry.read_to_end(&mut bytes).map_err(|err| {
        CliError::package_invalid(format!(
            "failed to read {zip_name} in segment {}: {err}",
            segment_path.display()
        ))
    })?;
    Ok(bytes)
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

#[cfg(windows)]
fn sample_system_memory() -> CliOutcome<MemorySample> {
    use std::mem::{MaybeUninit, size_of};

    #[repr(C)]
    struct MemoryStatusEx {
        dw_length: u32,
        dw_memory_load: u32,
        ull_total_phys: u64,
        ull_avail_phys: u64,
        ull_total_page_file: u64,
        ull_avail_page_file: u64,
        ull_total_virtual: u64,
        ull_avail_virtual: u64,
        ull_avail_extended_virtual: u64,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
    }

    let mut status = MaybeUninit::<MemoryStatusEx>::zeroed();
    let status_ptr = status.as_mut_ptr();
    unsafe {
        (*status_ptr).dw_length = size_of::<MemoryStatusEx>() as u32;
        if GlobalMemoryStatusEx(status_ptr) == 0 {
            return Err(CliError::device("GlobalMemoryStatusEx failed"));
        }
        let status = status.assume_init();
        Ok(MemorySample {
            total_bytes: status.ull_total_phys,
            available_bytes: status.ull_avail_phys,
        })
    }
}

#[cfg(not(windows))]
fn sample_system_memory() -> CliOutcome<MemorySample> {
    let meminfo = fs::read_to_string("/proc/meminfo")
        .map_err(|err| CliError::device(format!("failed to read /proc/meminfo: {err}")))?;
    let mut total = None;
    let mut available = None;
    for line in meminfo.lines() {
        if let Some(value) = parse_meminfo_kib(line, "MemTotal:") {
            total = Some(value);
        } else if let Some(value) = parse_meminfo_kib(line, "MemAvailable:") {
            available = Some(value);
        }
    }
    Ok(MemorySample {
        total_bytes: total
            .ok_or_else(|| CliError::device("MemTotal missing from /proc/meminfo"))?,
        available_bytes: available
            .ok_or_else(|| CliError::device("MemAvailable missing from /proc/meminfo"))?,
    })
}

#[cfg(not(windows))]
fn parse_meminfo_kib(line: &str, prefix: &str) -> Option<u64> {
    let rest = line.strip_prefix(prefix)?;
    let kib = rest.split_whitespace().next()?.parse::<u64>().ok()?;
    Some(kib.saturating_mul(1024))
}

#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_device::CaptureBackendName;
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
    fn threshold_one_is_rejected() {
        let config = FrameStoreConfig {
            similarity_threshold: 1.0,
            ..test_config(1_000)
        };

        let err = match FrameStore::new(PathBuf::from("unused"), config) {
            Ok(_) => panic!("threshold 1.0 should be rejected"),
            Err(err) => err,
        };

        assert!(err.message.contains("similarity_threshold"));
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

        assert!(err.message.contains("tier2/tier3 gap too small"));
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

        add_test_frame(&mut store, 1, 10, matched("arknights/home"), "initial");
        add_test_frame(&mut store, 2, 10, matched("arknights/home"), "page_wait");
        add_test_frame(&mut store, 3, 10, matched("arknights/home"), "page_wait");

        let screenshots = store.screenshots();
        assert_eq!(screenshots.len(), 1);
        assert_eq!(screenshots[0].merged_count, 2);
        assert_resident_accounting(&store);
    }

    #[test]
    fn page_transition_is_retained_even_under_dedup() {
        let temp = TempDir::new().expect("temp");
        let mut store = small_store(temp.path(), 220);

        add_test_frame(&mut store, 1, 10, matched("arknights/home"), "initial");
        add_test_frame(
            &mut store,
            2,
            10,
            matched("arknights/terminal"),
            "page_wait",
        );

        let screenshots = store.screenshots();
        assert_eq!(screenshots.len(), 2);
        assert!(screenshots[1].key_frame);
    }

    #[test]
    fn tier2_spills_segment_without_pausing() {
        let temp = TempDir::new().expect("temp");
        let mut store = small_store(temp.path(), 70_000);

        add_test_frame(&mut store, 1, 10, matched("arknights/home"), "initial");
        let outcome = add_test_frame(
            &mut store,
            2,
            30,
            matched("arknights/terminal"),
            "page_wait",
        );

        assert!(store.spilled_count > 0);
        assert!(!outcome.pause_required);
        assert!(
            temp.path()
                .join("temp")
                .join("segment-000001.zip")
                .is_file()
        );
        assert!(
            temp.path()
                .join("temp")
                .join("segment-manifest.jsonl")
                .is_file()
        );
        assert_resident_accounting(&store);
    }

    #[test]
    fn spilled_frame_keeps_thumbnail_for_later_dedup() {
        let temp = TempDir::new().expect("temp");
        let mut store = small_store(temp.path(), 70_000);

        let first = add_test_frame(&mut store, 1, 10, matched("arknights/home"), "initial");
        assert_eq!(first.storage_state, FrameStorageState::Segment);
        add_test_frame(&mut store, 2, 10, matched("arknights/home"), "page_wait");

        let screenshots = store.screenshots();
        assert_eq!(screenshots.len(), 1);
        assert_eq!(screenshots[0].merged_count, 1);
        assert_resident_accounting(&store);
    }

    #[test]
    fn single_frame_can_spill() {
        let temp = TempDir::new().expect("temp");
        let mut store = small_store(temp.path(), 70_000);

        let outcome = add_test_frame(&mut store, 1, 10, matched("arknights/home"), "initial");

        assert_eq!(outcome.storage_state, FrameStorageState::Segment);
        assert!(!outcome.pause_required);
    }

    #[test]
    fn spilled_segment_materializes_to_screenshot_file() {
        let temp = TempDir::new().expect("temp");
        let mut store = small_store(temp.path(), 70_000);

        add_test_frame(&mut store, 1, 10, matched("arknights/home"), "initial");

        let screenshots = temp.path().join("screenshots");
        store.materialize(&screenshots).expect("materialize");
        assert!(screenshots.join("frame1.png").is_file());
    }

    #[test]
    fn last_frame_can_spill_when_eligible() {
        let temp = TempDir::new().expect("temp");
        let mut store = small_store(temp.path(), 70_000);

        add_test_frame(&mut store, 1, 10, matched("arknights/home"), "initial");
        add_test_frame(
            &mut store,
            2,
            20,
            matched("arknights/terminal"),
            "page_wait",
        );

        assert!(
            store
                .screenshots()
                .iter()
                .any(|record| record.storage_state == FrameStorageState::Segment)
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

        add_test_frame(&mut store, 1, 10, matched("arknights/home"), "initial");

        let diagnostics = store.diagnostics_json();
        assert!(diagnostics["payload_bytes"].as_u64().unwrap() > 0);
        assert!(diagnostics["metadata_estimated_bytes"].as_u64().unwrap() > 0);
        assert!(diagnostics["thumbnail_estimated_bytes"].as_u64().unwrap() > 0);
        assert!(
            diagnostics["encoder_workspace_reserved_bytes"]
                .as_u64()
                .unwrap()
                > 0
        );
    }

    #[test]
    fn spill_io_failure_degrades_without_panic() {
        let temp = TempDir::new().expect("temp");
        let temp_file = temp.path().join("not-a-dir");
        fs::write(&temp_file, b"block directory creation").expect("write blocker");
        let mut store = FrameStore::new(temp_file, test_config(90)).expect("store");

        let outcome = add_test_frame(&mut store, 1, 10, matched("arknights/home"), "initial");

        assert!(
            outcome
                .warnings
                .iter()
                .any(|warning| warning.contains("spill"))
        );
        assert_eq!(outcome.backpressure_state, BackpressureState::SpillDegraded);
        assert!(store.spill_warning_count >= 1);
        assert!(store.entries.iter().all(|entry| !entry.spill_failed));
        assert_eq!(store.encoder_workspace_reserved_bytes, 0);
        assert_resident_accounting(&store);
    }

    #[test]
    fn cleanup_temp_removes_segment_directory() {
        let temp = TempDir::new().expect("temp");
        let mut store = small_store(temp.path(), 90);
        add_test_frame(&mut store, 1, 10, matched("arknights/home"), "initial");

        assert!(temp.path().join("temp").exists());
        assert!(store.cleanup_temp().is_empty());
        assert!(!temp.path().join("temp").exists());
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
    fn tier3_alarm_still_materializes_partial_screenshots() {
        let temp = TempDir::new().expect("temp");
        let mut store = small_store(temp.path(), 80);

        let outcome = add_test_frame(&mut store, 1, 10, RecognitionState::Pending, "initial");
        assert!(outcome.tier3_triggered);

        let screenshots_dir = temp.path().join("screenshots");
        store.materialize(&screenshots_dir).expect("materialize");
        assert!(screenshots_dir.join("frame1.png").is_file());
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
            matched("arknights/home"),
            "initial",
            later,
        );
        add_test_frame_at(
            &mut store,
            2,
            10,
            matched("arknights/home"),
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
        FrameStore::new(path.join("temp"), test_config(max_mem_bytes)).expect("store")
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
        store
            .add_frame(FrameStoreFrameInput {
                frame_index: index,
                file_name: format!("frame{index}.png"),
                label: label.to_string(),
                recognition_state,
                frame,
            })
            .expect("add frame")
    }

    fn assert_resident_accounting(store: &FrameStore) {
        let mut estimate = ResidentEstimate::default();
        for entry in store.entries.iter().filter(|entry| entry.retained) {
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
