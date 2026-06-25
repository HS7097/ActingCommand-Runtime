// SPDX-License-Identifier: AGPL-3.0-only

use crate::{CliError, CliOutcome};
use actingcommand_device::{Frame, PixelFormat};
use serde::Deserialize;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const DEFAULT_SIMILARITY_THRESHOLD: f32 = 0.95;
const DEFAULT_TIER1_RATIO: f64 = 0.60;
const DEFAULT_TIER2_RATIO: f64 = 0.75;
const DEFAULT_TIER3_RATIO: f64 = 0.90;
const DEFAULT_HYSTERESIS_RATIO: f64 = 0.10;
const DEFAULT_OS_RESERVE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
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
}

impl MemoryBudget {
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
        }
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
            "tier3_release_bytes": self.tier3_release_bytes
        })
    }
}

pub(super) struct FrameStore {
    config: FrameStoreConfig,
    temp_dir: PathBuf,
    budget: MemoryBudget,
    resident_bytes: u64,
    entries: Vec<FrameEntry>,
    timeline: Vec<Value>,
    tier1_active: bool,
    tier2_active: bool,
    tier3_active: bool,
    dropped_count: u64,
    spilled_count: u64,
}

impl FrameStore {
    pub(super) fn new(temp_dir: PathBuf, config: FrameStoreConfig) -> CliOutcome<Self> {
        config
            .validate()
            .map_err(|err| CliError::usage(format!("invalid frame store config: {err}")))?;
        let budget = MemoryBudget::from_config(&config, config.memory_sample()?);
        Ok(Self {
            config,
            temp_dir,
            budget,
            resident_bytes: 0,
            entries: Vec::new(),
            timeline: Vec::new(),
            tier1_active: false,
            tier2_active: false,
            tier3_active: false,
            dropped_count: 0,
            spilled_count: 0,
        })
    }

    pub(super) fn set_config(&mut self, config: FrameStoreConfig) -> CliOutcome<()> {
        config
            .validate()
            .map_err(|err| CliError::usage(format!("invalid frame store config: {err}")))?;
        self.config = config;
        self.refresh_budget()?;
        Ok(())
    }

    pub(super) fn add_frame(
        &mut self,
        input: FrameStoreFrameInput,
    ) -> CliOutcome<FrameStoreOutcome> {
        self.refresh_budget()?;
        self.release_watermarks_if_needed();
        let file = format!("screenshots/{}", input.file_name);
        let key_frame = self.is_key_frame(&input);
        let thumb = thumbnail(&input.frame);
        let bytes = frame_bytes(&input.frame);
        let entry = FrameEntry {
            frame_index: input.frame_index,
            file_name: input.file_name,
            file: file.clone(),
            width: input.frame.width,
            height: input.frame.height,
            captured_at: input.frame.captured_at,
            backend: input.frame.backend_name.as_str().to_string(),
            pixel_format: input.frame.pixel_format.as_str().to_string(),
            label: input.label,
            matched_page: input.matched_page,
            key_frame,
            merged_count: 0,
            dwell_ms: 0,
            delta_from_previous_ms: self.delta_from_previous_ms(input.frame.captured_at),
            retained: true,
            merged_into: None,
            storage: FrameStorage::Resident(input.frame),
            resident_bytes: bytes,
            thumb,
        };
        self.resident_bytes = self.resident_bytes.saturating_add(bytes);
        self.entries.push(entry);
        let entry_index = self.entries.len() - 1;
        self.timeline.push(json!({
            "event": "frame_retained",
            "frame_index": self.entries[entry_index].frame_index,
            "file": file,
            "key_frame": key_frame,
            "resident_bytes": self.resident_bytes
        }));

        self.apply_watermarks()?;
        let retained = self.entries[entry_index].retained;
        let outcome = FrameStoreOutcome {
            retained,
            file: retained.then(|| self.entries[entry_index].file.clone()),
            merged_into: self.entries[entry_index].merged_into.clone(),
            tier3_triggered: self.tier3_active,
        };
        Ok(outcome)
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
                FrameStorage::Spilled(path) => {
                    fs::copy(path, &destination).map_err(|err| {
                        CliError::package_invalid(format!(
                            "failed to copy {} to {}: {err}",
                            path.display(),
                            destination.display()
                        ))
                    })?;
                }
                FrameStorage::Dropped => {}
            }
        }
        Ok(())
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
                matched_page: entry.matched_page.clone(),
                key_frame: entry.key_frame,
            })
            .collect()
    }

    pub(super) fn diagnostics_json(&self) -> Value {
        json!({
            "schema_version": "Lab-1z.frame_store.v1",
            "config": {
                "similarity_threshold": self.config.similarity_threshold,
                "tier1_ratio": self.config.tier1_ratio,
                "tier2_ratio": self.config.tier2_ratio,
                "tier3_ratio": self.config.tier3_ratio,
                "hysteresis_ratio": self.config.hysteresis_ratio,
                "max_mem_bytes": self.config.max_mem_bytes,
                "os_reserve_bytes": self.config.os_reserve_bytes
            },
            "budget": self.budget.to_json(),
            "resident_bytes": self.resident_bytes,
            "retained_count": self.entries.iter().filter(|entry| entry.retained).count(),
            "captured_count": self.entries.len(),
            "dropped_count": self.dropped_count,
            "spilled_count": self.spilled_count,
            "tier1_active": self.tier1_active,
            "tier2_active": self.tier2_active,
            "tier3_active": self.tier3_active
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
                "matched_page": entry.matched_page,
                "label": entry.label,
                "backend": entry.backend,
                "pixel_format": entry.pixel_format,
                "key_frame": entry.key_frame,
                "dwell_ms": entry.dwell_ms,
                "merged_count": entry.merged_count,
                "storage": entry.storage.kind()
            })
        }));
        rows
    }

    fn refresh_budget(&mut self) -> CliOutcome<()> {
        self.budget = MemoryBudget::from_config(&self.config, self.config.memory_sample()?);
        Ok(())
    }

    fn apply_watermarks(&mut self) -> CliOutcome<()> {
        if !self.tier1_active && self.resident_bytes >= self.budget.tier1_bytes {
            self.tier1_active = true;
            self.timeline.push(json!({
                "event": "tier1_activated",
                "resident_bytes": self.resident_bytes,
                "threshold_bytes": self.budget.tier1_bytes
            }));
            self.dedup_existing()?;
        }
        if self.tier1_active {
            self.dedup_existing()?;
        }
        if !self.tier2_active && self.resident_bytes >= self.budget.tier2_bytes {
            self.tier2_active = true;
            self.timeline.push(json!({
                "event": "tier2_activated",
                "resident_bytes": self.resident_bytes,
                "threshold_bytes": self.budget.tier2_bytes
            }));
        }
        if self.tier2_active {
            self.spill_resident_frames()?;
        }
        if !self.tier3_active && self.resident_bytes >= self.budget.tier3_bytes {
            self.tier3_active = true;
            self.timeline.push(json!({
                "event": "tier3_activated",
                "resident_bytes": self.resident_bytes,
                "threshold_bytes": self.budget.tier3_bytes
            }));
        }
        Ok(())
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

    fn dedup_existing(&mut self) -> CliOutcome<()> {
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
                self.drop_entry(index, previous)?;
            }
        }
        Ok(())
    }

    fn same_page_duplicate(&self, previous: usize, current: usize) -> bool {
        self.entries[previous].matched_page == self.entries[current].matched_page
            && thumb_similarity(&self.entries[previous].thumb, &self.entries[current].thumb)
                > self.config.similarity_threshold
    }

    fn drop_entry(&mut self, index: usize, target: usize) -> CliOutcome<()> {
        if !self.entries[index].retained {
            return Ok(());
        }
        let target_file = self.entries[target].file.clone();
        let dropped_file = self.entries[index].file.clone();
        let dropped_delta = self.entries[index].delta_from_previous_ms;
        self.release_storage(index);
        self.entries[index].retained = false;
        self.entries[index].merged_into = Some(target_file.clone());
        self.entries[target].merged_count = self.entries[target].merged_count.saturating_add(1);
        self.entries[target].dwell_ms = self.entries[target].dwell_ms.saturating_add(dropped_delta);
        self.dropped_count = self.dropped_count.saturating_add(1);
        self.timeline.push(json!({
            "event": "frame_deduplicated",
            "frame_index": self.entries[index].frame_index,
            "file": dropped_file,
            "merged_into": target_file,
            "resident_bytes": self.resident_bytes
        }));
        Ok(())
    }

    fn release_storage(&mut self, index: usize) {
        let storage = std::mem::replace(&mut self.entries[index].storage, FrameStorage::Dropped);
        if matches!(storage, FrameStorage::Resident(_)) {
            self.resident_bytes = self
                .resident_bytes
                .saturating_sub(self.entries[index].resident_bytes);
        }
    }

    fn spill_resident_frames(&mut self) -> CliOutcome<()> {
        fs::create_dir_all(&self.temp_dir).map_err(|err| {
            CliError::package_invalid(format!(
                "failed to create {}: {err}",
                self.temp_dir.display()
            ))
        })?;
        let target_bytes = self.budget.tier1_bytes;
        for index in 0..self.entries.len().saturating_sub(1) {
            if self.resident_bytes <= target_bytes {
                break;
            }
            if !self.entries[index].retained {
                continue;
            }
            let FrameStorage::Resident(frame) = &self.entries[index].storage else {
                continue;
            };
            let png = frame
                .png_for_artifact()
                .map_err(|err| CliError::device(err.to_string()))?;
            let path = self.temp_dir.join(&self.entries[index].file_name);
            fs::write(&path, png).map_err(|err| {
                CliError::package_invalid(format!("failed to write {}: {err}", path.display()))
            })?;
            self.entries[index].storage = FrameStorage::Spilled(path.clone());
            self.resident_bytes = self
                .resident_bytes
                .saturating_sub(self.entries[index].resident_bytes);
            self.spilled_count = self.spilled_count.saturating_add(1);
            self.timeline.push(json!({
                "event": "frame_spilled",
                "frame_index": self.entries[index].frame_index,
                "file": self.entries[index].file,
                "temp_path": path,
                "resident_bytes": self.resident_bytes
            }));
        }
        Ok(())
    }

    fn is_key_frame(&self, input: &FrameStoreFrameInput) -> bool {
        let label = input.label.as_str();
        label == "initial"
            || label.contains("click")
            || label.contains("action")
            || label.contains("before")
            || label.contains("after")
            || self
                .entries
                .iter()
                .rev()
                .find(|entry| entry.retained)
                .is_none_or(|previous| previous.matched_page != input.matched_page)
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
    pub(super) matched_page: Option<String>,
    pub(super) frame: Frame,
}

pub(super) struct FrameStoreOutcome {
    pub(super) retained: bool,
    pub(super) file: Option<String>,
    pub(super) merged_into: Option<String>,
    pub(super) tier3_triggered: bool,
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
    pub(super) key_frame: bool,
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
    matched_page: Option<String>,
    key_frame: bool,
    merged_count: u64,
    dwell_ms: u64,
    delta_from_previous_ms: u64,
    retained: bool,
    merged_into: Option<String>,
    storage: FrameStorage,
    resident_bytes: u64,
    thumb: Thumbnail,
}

enum FrameStorage {
    Resident(Frame),
    Spilled(PathBuf),
    Dropped,
}

impl FrameStorage {
    fn kind(&self) -> &'static str {
        match self {
            Self::Resident(_) => "memory",
            Self::Spilled(_) => "temp_disk",
            Self::Dropped => "dropped",
        }
    }
}

#[derive(Clone)]
struct Thumbnail {
    values: Vec<u8>,
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
        let y = ((ty * height) / THUMB_HEIGHT).min(height.saturating_sub(1));
        for tx in 0..THUMB_WIDTH {
            let x = ((tx * width) / THUMB_WIDTH).min(width.saturating_sub(1));
            let offset = (y * width + x) * channels;
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

fn frame_bytes(frame: &Frame) -> u64 {
    frame.pixels.len() as u64
        + frame
            .original_png
            .as_ref()
            .map(|png| png.len() as u64)
            .unwrap_or(0)
}

fn ratio_bytes(bytes: u64, ratio: f64) -> u64 {
    ((bytes as f64) * ratio).floor() as u64
}

fn validate_ratio_f32(name: &str, value: f32) -> Result<(), String> {
    if value.is_finite() && value > 0.0 && value <= 1.0 {
        Ok(())
    } else {
        Err(format!("{name} must be > 0 and <= 1"))
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
        let config = FrameStoreConfig {
            os_reserve_bytes: 100,
            max_mem_bytes: Some(500),
            ..Default::default()
        };
        let budget = MemoryBudget::from_config(
            &config,
            MemorySample {
                total_bytes: 1_000,
                available_bytes: 700,
            },
        );

        assert_eq!(budget.budget_bytes, 500);
        assert_eq!(budget.tier1_bytes, 300);
        assert_eq!(budget.tier1_release_bytes, 270);
    }

    #[test]
    fn tier1_deduplicates_static_same_page_frames() {
        let temp = TempDir::new().expect("temp");
        let mut store = small_store(temp.path(), 40);

        add_test_frame(&mut store, 1, 10, Some("arknights/home"), "initial");
        add_test_frame(&mut store, 2, 10, Some("arknights/home"), "page_wait");
        add_test_frame(&mut store, 3, 10, Some("arknights/home"), "page_wait");

        let screenshots = store.screenshots();
        assert_eq!(screenshots.len(), 1);
        assert_eq!(screenshots[0].merged_count, 2);
    }

    #[test]
    fn page_transition_is_retained_even_under_dedup() {
        let temp = TempDir::new().expect("temp");
        let mut store = small_store(temp.path(), 40);

        add_test_frame(&mut store, 1, 10, Some("arknights/home"), "initial");
        add_test_frame(&mut store, 2, 10, Some("arknights/terminal"), "page_wait");

        let screenshots = store.screenshots();
        assert_eq!(screenshots.len(), 2);
        assert!(screenshots[1].key_frame);
    }

    #[test]
    fn tier2_spills_retained_frames_to_temp_disk() {
        let temp = TempDir::new().expect("temp");
        let mut store = small_store(temp.path(), 80);

        add_test_frame(&mut store, 1, 10, Some("arknights/home"), "initial");
        add_test_frame(&mut store, 2, 30, Some("arknights/terminal"), "page_wait");

        assert!(store.spilled_count > 0);
        assert!(
            store
                .timeline()
                .iter()
                .any(|event| event["event"] == "frame_spilled")
        );
    }

    #[test]
    fn hysteresis_releases_only_below_release_line() {
        let temp = TempDir::new().expect("temp");
        let mut store = small_store(temp.path(), 200);
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
        let mut store = small_store(temp.path(), 10);

        let outcome = add_test_frame(&mut store, 1, 10, Some("arknights/home"), "initial");
        assert!(outcome.tier3_triggered);

        let screenshots_dir = temp.path().join("screenshots");
        store.materialize(&screenshots_dir).expect("materialize");
        assert!(screenshots_dir.join("frame1.png").is_file());
    }

    fn small_store(path: &Path, max_mem_bytes: u64) -> FrameStore {
        let config = FrameStoreConfig {
            max_mem_bytes: Some(max_mem_bytes),
            os_reserve_bytes: 0,
            tier1_ratio: 0.50,
            tier2_ratio: 0.70,
            tier3_ratio: 0.90,
            ..Default::default()
        }
        .with_memory_sample(MemorySample {
            total_bytes: 1_000,
            available_bytes: 1_000,
        });
        FrameStore::new(path.join("temp"), config).expect("store")
    }

    fn add_test_frame(
        store: &mut FrameStore,
        index: usize,
        shade: u8,
        page: Option<&str>,
        label: &str,
    ) -> FrameStoreOutcome {
        let pixels = vec![shade; 4 * 4 * 4];
        let frame = Frame::from_pixels(
            4,
            4,
            pixels,
            PixelFormat::Rgba8,
            CaptureBackendName::NemuIpc,
        )
        .expect("frame");
        store
            .add_frame(FrameStoreFrameInput {
                frame_index: index,
                file_name: format!("frame{index}.png"),
                label: label.to_string(),
                matched_page: page.map(str::to_string),
                frame,
            })
            .expect("add frame")
    }
}
