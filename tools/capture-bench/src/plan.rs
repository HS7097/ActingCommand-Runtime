// SPDX-License-Identifier: AGPL-3.0-only

//! The plan (§3.2 item 3, §3.10, §3.11.1, §4.3): one TOML file. A plan cannot change the fixed
//! limits (deadlines, the working-set cap, the lock and run roots).

use crate::backend::IdentityMethod;
use crate::model::{GateKind, PathKind};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub run_id: String,
    /// The absolute deadline of the whole run (window end minus the closing 15 min).
    pub until_unix_ms: u64,
    /// The whole run's total duration bound.
    pub total_duration_ms: u64,
    #[serde(default = "default_seed")]
    pub seed: u64,
    /// The window (`W1a` … `W3`, `D1`, `D2`) for `header.json` (analysis schema §2).
    #[serde(default)]
    pub window: Option<String>,
    pub safety: SafetyPlan,
    pub instances: Vec<InstancePlan>,
    pub cells: Vec<CellPlan>,
    #[serde(default)]
    pub budget: BudgetPlan,
    #[serde(default)]
    pub metrics: MetricsPlan,
}

fn default_seed() -> u64 {
    402
}

/// §3.10 item 1 and 5: the whitelists and the process settings the start check holds the run to.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SafetyPlan {
    /// The adb the Runtime uses (MuMu's 37.0.1-15733141), by absolute path.
    pub adb_path: String,
    /// The exact `Version ...` value `adb version` must print.
    pub adb_version: String,
    /// The MuMu installation folder the Nemu SDK is loaded from.
    pub mumu_folder: String,
    /// The production actingd's priority class, read before the window.
    pub priority_class: PriorityClass,
    pub serial_whitelist: Vec<String>,
    pub nemu_whitelist: Vec<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PriorityClass {
    Idle,
    BelowNormal,
    Normal,
    AboveNormal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Orientation {
    Landscape,
    Portrait,
}

/// How the simulated formal loop taps (§5.6). `adb_input` is `adb -s <E> shell input tap`.
/// `nemu_ipc` would need a fenced-write witness, which only the Runtime's scheduler issues, so
/// this slice refuses it at the start check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TouchKind {
    AdbInput,
    NemuIpc,
    /// No taps (preview-only plans and the fake backend).
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstancePlan {
    pub name: String,
    pub serial: String,
    pub nemu_instance: i32,
    pub orientation: Orientation,
    pub touch: TouchKind,
    /// The device-side identity method C0 proved on this E (§3.5.1); `None` until C0 ran, when
    /// the bench tries the candidates itself.
    #[serde(default)]
    pub identity_method: Option<IdentityMethod>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CellPlan {
    pub id: String,
    /// The cell limit `L_c` from the cell's start (§3.11.2 A).
    pub limit_ms: u64,
    #[serde(default = "default_rounds")]
    pub rounds: u32,
    /// Whether the segments of a round run in a seeded random order (§4.1 顺序).
    #[serde(default)]
    pub shuffle: bool,
    pub segments: Vec<SegmentPlan>,
}

fn default_rounds() -> u32 {
    1
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentMode {
    /// Continuous preview at `fps`; with `path` unset the preview is off (the C3 baseline).
    Preview,
    /// Tap, then capture with a floor after the tap (C6).
    TaskFloor,
    /// S on a still picture: capture with `after_sequence` of the last delivery, in a loop (C6).
    PollSequence,
    /// Open to the first frame, close and confirm, `reps` times (C0, C2).
    ColdStart,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SegmentPlan {
    /// The segment's name in the records (`floor`, `static_poll`, `main`, `variant`, `2fps`, …);
    /// without one the records name the mode and the segment's position.
    #[serde(default)]
    pub name: Option<String>,
    pub instance: String,
    /// The path under test; `None` is "preview off".
    #[serde(default)]
    pub path: Option<PathKind>,
    pub mode: SegmentMode,
    /// A time-scheduled segment ends after this (§3.11.2 A: 排完就是配额到).
    #[serde(default)]
    pub duration_ms: Option<u64>,
    /// A counted segment ends when this many opportunities (or floor checks, or cold starts)
    /// are done.
    #[serde(default)]
    pub quota: Option<u64>,
    #[serde(default = "default_fps")]
    pub fps: f64,
    #[serde(default = "default_viewers")]
    pub viewers: u32,
    /// One viewer reads at this rate (§4.4 item 2, the slow consumer).
    #[serde(default)]
    pub slow_viewer_fps: Option<f64>,
    #[serde(default)]
    pub formal: Option<FormalPlan>,
    #[serde(default)]
    pub scrcpy: Option<ScrcpyPlan>,
    #[serde(default)]
    pub gates: Vec<GateScript>,
    /// The S soft-unsubscribe grace: 0 or 5000 ms (§3.4; the code caps it at 10 s).
    #[serde(default = "default_grace")]
    pub grace_ms: u64,
    /// The lease TTL: 3000 ms, or 2000 ms for the variant (§3.4).
    #[serde(default = "default_ttl")]
    pub lease_ttl_ms: u64,
    #[serde(default = "default_check")]
    pub lease_check_ms: u64,
    /// Cold starts in a `cold_start` segment.
    #[serde(default)]
    pub reps: Option<u32>,
    /// The tap grid of the task modes (4×4, §3.2 item 5).
    #[serde(default = "default_grid")]
    pub tap_grid: u32,
    /// Viewers whose renewals stop at this offset (§4.4 items 4, 5).
    #[serde(default)]
    pub viewer_stop_renew_ms: Option<u64>,
    /// Viewers that unsubscribe at this offset (§4.4 item 3).
    #[serde(default)]
    pub viewer_unsubscribe_ms: Option<u64>,
}

fn default_fps() -> f64 {
    5.0
}

fn default_viewers() -> u32 {
    1
}

fn default_grace() -> u64 {
    5_000
}

fn default_ttl() -> u64 {
    3_000
}

fn default_check() -> u64 {
    500
}

fn default_grid() -> u32 {
    4
}

/// The simulated formal loop (§5.6): every `period_ms ± jitter_ms`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FormalPlan {
    #[serde(default = "default_period")]
    pub period_ms: u64,
    #[serde(default = "default_jitter")]
    pub jitter_ms: u64,
    /// Formal captures read the S slot (the C7 variant; Nemu only taps).
    #[serde(default)]
    pub capture_from_scrcpy: bool,
    /// Formal dispatches the segment needs (the count quota of C7).
    #[serde(default)]
    pub quota: Option<u64>,
}

fn default_period() -> u64 {
    500
}

fn default_jitter() -> u64 {
    100
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ScrcpyPlan {
    #[serde(default)]
    pub bit_rate: Option<u32>,
    #[serde(default)]
    pub max_fps: Option<u32>,
    #[serde(default)]
    pub encoder: Option<String>,
}

/// A scripted gate signal (§3.7, §4.4 items 7-9, 12).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GateScript {
    pub at_ms: u64,
    pub gate: GateKind,
    /// Cleared after this long; `None` leaves it set to the segment's end.
    #[serde(default)]
    pub hold_ms: Option<u64>,
}

/// §3.6: the budget caps.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetPlan {
    #[serde(default = "default_samples")]
    pub max_samples_per_second: f64,
    #[serde(default = "default_bytes")]
    pub max_bytes_per_second: f64,
    #[serde(default = "default_inflight")]
    pub max_inflight_global: u32,
    #[serde(default = "default_buffer")]
    pub max_buffer_bytes: u64,
}

impl Default for BudgetPlan {
    fn default() -> Self {
        Self {
            max_samples_per_second: default_samples(),
            max_bytes_per_second: default_bytes(),
            max_inflight_global: default_inflight(),
            max_buffer_bytes: default_buffer(),
        }
    }
}

fn default_samples() -> f64 {
    60.0
}

fn default_bytes() -> f64 {
    400.0 * 1024.0 * 1024.0
}

fn default_inflight() -> u32 {
    2
}

fn default_buffer() -> u64 {
    256 * 1024 * 1024
}

/// §5.5: the bounded `typeperf` per cell.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(deny_unknown_fields)]
pub struct MetricsPlan {
    #[serde(default)]
    pub typeperf: bool,
}

impl Plan {
    pub fn parse(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|error| format!("plan_invalid: {error}"))
    }

    pub fn instance_index(&self, name: &str) -> Option<usize> {
        self.instances
            .iter()
            .position(|instance| instance.name == name)
    }

    /// The plan's own consistency; the start check adds the host and device conditions.
    pub fn validate(&self) -> Result<(), String> {
        let mut problems = Vec::new();
        if self.run_id.is_empty()
            || !self
                .run_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        {
            problems.push("run_id must be letters, digits, `-` and `_`".to_owned());
        }
        if self.total_duration_ms == 0 {
            problems.push("total_duration_ms must be positive".to_owned());
        }
        let mut names = BTreeSet::new();
        for instance in &self.instances {
            if !names.insert(instance.name.as_str()) {
                problems.push(format!("instance {} is declared twice", instance.name));
            }
            if !self.safety.serial_whitelist.contains(&instance.serial) {
                problems.push(format!(
                    "instance {} serial is not in the serial whitelist",
                    instance.name
                ));
            }
            if !self.safety.nemu_whitelist.contains(&instance.nemu_instance) {
                problems.push(format!(
                    "instance {} Nemu index {} is not in the Nemu whitelist",
                    instance.name, instance.nemu_instance
                ));
            }
            if instance.touch == TouchKind::NemuIpc {
                problems.push(format!(
                    "instance {} touch nemu_ipc needs a fenced-write witness the bench cannot hold",
                    instance.name
                ));
            }
        }
        let mut cells = BTreeSet::new();
        for cell in &self.cells {
            if !cells.insert(cell.id.as_str()) {
                problems.push(format!("cell {} is declared twice", cell.id));
            }
            if cell.limit_ms == 0 || cell.rounds == 0 || cell.segments.is_empty() {
                problems.push(format!(
                    "cell {} needs a limit, rounds and segments",
                    cell.id
                ));
            }
            for (index, segment) in cell.segments.iter().enumerate() {
                let label = format!("cell {} segment {index}", cell.id);
                if self.instance_index(&segment.instance).is_none() {
                    problems.push(format!("{label}: unknown instance {}", segment.instance));
                }
                if segment.duration_ms.is_none()
                    && segment.quota.is_none()
                    && segment.reps.is_none()
                {
                    problems.push(format!("{label}: needs duration_ms, quota or reps"));
                }
                if !(segment.fps > 0.0 && segment.fps <= 60.0) {
                    problems.push(format!("{label}: fps must be in (0, 60]"));
                }
                if !matches!(segment.grace_ms, 0 | 5_000) {
                    problems.push(format!("{label}: grace_ms must be 0 or 5000"));
                }
                if !matches!(segment.lease_ttl_ms, 2_000 | 3_000) {
                    problems.push(format!("{label}: lease_ttl_ms must be 2000 or 3000"));
                }
                if segment.lease_check_ms == 0 || segment.lease_check_ms > segment.lease_ttl_ms {
                    problems.push(format!(
                        "{label}: lease_check_ms must be in (0, lease_ttl_ms]"
                    ));
                }
                if segment.tap_grid == 0 || segment.tap_grid > 16 {
                    problems.push(format!("{label}: tap_grid must be in 1..=16"));
                }
                match segment.mode {
                    SegmentMode::Preview => {}
                    SegmentMode::TaskFloor | SegmentMode::ColdStart => {
                        if segment.path.is_none() {
                            problems.push(format!("{label}: the mode needs a path"));
                        }
                    }
                    SegmentMode::PollSequence => {
                        if segment.path != Some(PathKind::Scrcpy) {
                            problems.push(format!("{label}: poll_sequence is an S mode"));
                        }
                    }
                }
                if segment.mode == SegmentMode::ColdStart && segment.reps.is_none() {
                    problems.push(format!("{label}: cold_start needs reps"));
                }
            }
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(format!("plan_invalid: {}", problems.join("; ")))
        }
    }
}
