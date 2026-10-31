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
    /// #402 C2b: the test-pattern APK's control (C3's protocol).
    #[serde(default)]
    pub apk: ApkPlan,
    /// #402 C2b: C8's pictures, truth and offline evaluation command (§5.8).
    #[serde(default)]
    pub c8: Option<C8Plan>,
}

/// #402 C2b: how the bench drives the test-pattern APK (PROTOCOL §5-§7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ApkPlan {
    /// The band's cell edge until INFO reports the layout (PROTOCOL §4.2, default 24).
    #[serde(default = "default_cell")]
    pub cell: u32,
    /// PINGs per calibration (plan §5.1: 200).
    #[serde(default = "default_pings")]
    pub pings: u32,
    /// The recalibration period inside a window (plan §5.1 item 2: 10 min).
    #[serde(default = "default_recalibrate")]
    pub recalibrate_ms: u64,
    /// How often the record streams are fetched (the vsync ring holds about 9 min at 60 Hz).
    #[serde(default = "default_records_period")]
    pub records_period_ms: u64,
    /// After a C6 tap: how long the bench looks for the first frame that shows the flip.
    #[serde(default = "default_flip_wait")]
    pub flip_wait_ms: u64,
    /// A step trial waits this long for the first S frame that shows the step.
    #[serde(default = "default_step_wait")]
    pub step_wait_ms: u64,
    /// A mode change or a rotation waits this long for the APK to show it.
    #[serde(default = "default_mode_wait")]
    pub mode_wait_ms: u64,
}

impl Default for ApkPlan {
    fn default() -> Self {
        Self {
            cell: default_cell(),
            pings: default_pings(),
            recalibrate_ms: default_recalibrate(),
            records_period_ms: default_records_period(),
            flip_wait_ms: default_flip_wait(),
            step_wait_ms: default_step_wait(),
            mode_wait_ms: default_mode_wait(),
        }
    }
}

fn default_cell() -> u32 {
    24
}

fn default_pings() -> u32 {
    200
}

fn default_recalibrate() -> u64 {
    600_000
}

fn default_records_period() -> u64 {
    60_000
}

fn default_flip_wait() -> u64 {
    1_000
}

fn default_step_wait() -> u64 {
    2_000
}

fn default_mode_wait() -> u64 {
    5_000
}

/// #402 C2b: C8 (plan §5.8). The pictures and `truth.json` stay on the Workflow side; the
/// bench reads them by path and holds no game content.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct C8Plan {
    /// The folder of the pictures `truth.json` names.
    pub images_dir: String,
    /// `truth.json`: `{"package", "package_sha256", "facts", "images": [{"image", "kind",
    /// "targets": [{"target", "class", "hit", "text"}]}]}`.
    pub truth: String,
    /// Frames saved per picture and path (plan: 5).
    #[serde(default = "default_frames_per_image")]
    pub frames_per_image: u32,
    /// The `actinglab` of the same build, named in the coverage list's command column; the
    /// bench does not run it (the coordinator does, offline).
    #[serde(default)]
    pub actinglab: Option<String>,
}

fn default_frames_per_image() -> u32 {
    5
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
/// `nemu_ipc` taps through the instance's paired Nemu session, on the same worker thread as the
/// N captures (§5.6 "N 预览和点按同线程排队"); the bench mints its own fenced-write witness
/// under the experiment-only guard exception (Workflow #402, the branch is never merged).
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
    /// #402 C2b: the test-pattern APK is installed on this E and the bench drives it.
    #[serde(default)]
    pub apk: bool,
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
    /// #402 C2b (C9): the segments of a round run at the same time (one segment per instance),
    /// for example E1's and E2's previews together.
    #[serde(default)]
    pub concurrent: bool,
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
    /// #402 C2b: `reps` calibrations of `apk.pings` PINGs (C1, K; plan §5.1).
    Calibrate,
    /// #402 C2b: `reps` step trials on S (C1, K; plan §5.3 `V_hi`).
    StepTrials,
    /// #402 C2b: `reps` rotations of the APK (C10 item 11): in-cell gate, drain, close and
    /// confirm, rotate, wait for the new geometry, reopen with the generation + 1.
    Rotate,
    /// #402 C2b: `reps` reconnects (C10 item 10): N closes and reopens in-process; S stops this
    /// stream's server by its identity, waits for the end, closes and confirms, reopens.
    Reconnect,
    /// #402 C2b: C8's pictures in the APK's static mode, frames saved per path (§5.8).
    StaticImages,
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
    /// #402 C2b: the APK mode the segment sets first (`pattern`, `flip`, `step`, `static`,
    /// `replay`); `None` leaves it.
    #[serde(default)]
    pub apk_mode: Option<String>,
    /// #402 C2b: the first viewer draws each frame in a minimal GDI window and records
    /// `t_gdi_ret_us` (`A_submit`, a submit-side proxy, not presentation; §5.1).
    #[serde(default)]
    pub gdi_viewer: bool,
    /// #402 C2b (C9 ii): N connects and disconnects around every sample.
    #[serde(default)]
    pub reconnect_each_sample: bool,
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
    /// #402 C2b (C9 iv): the dispatch records' `preview_state` (`off`, `on`) when the preview
    /// runs on another instance; `None` takes the segment's own path.
    #[serde(default)]
    pub preview_state: Option<String>,
    /// #402 C2b (C9 iv): the `<path>` of the dispatch file `opps-<cell>-<path>.jsonl`.
    #[serde(default)]
    pub record_path: Option<String>,
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
                    SegmentMode::Preview
                    | SegmentMode::Calibrate
                    | SegmentMode::StepTrials
                    | SegmentMode::Rotate
                    | SegmentMode::Reconnect
                    | SegmentMode::StaticImages => {}
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
                let apk = self
                    .instance_index(&segment.instance)
                    .is_some_and(|index| self.instances[index].apk);
                match segment.mode {
                    SegmentMode::Calibrate
                    | SegmentMode::StepTrials
                    | SegmentMode::Rotate
                    | SegmentMode::StaticImages => {
                        if !apk {
                            problems.push(format!("{label}: the mode needs the APK (apk = true)"));
                        }
                        if segment.reps.is_none() && segment.mode != SegmentMode::StaticImages {
                            problems.push(format!("{label}: the mode needs reps"));
                        }
                    }
                    SegmentMode::Reconnect => {
                        if segment.reps.is_none() || segment.path.is_none() {
                            problems.push(format!("{label}: reconnect needs reps and a path"));
                        }
                    }
                    _ => {}
                }
                if matches!(segment.mode, SegmentMode::StepTrials)
                    && segment.path != Some(PathKind::Scrcpy)
                {
                    problems.push(format!("{label}: step_trials is an S mode"));
                }
                if matches!(
                    segment.mode,
                    SegmentMode::Rotate | SegmentMode::StaticImages
                ) && segment.path.is_none()
                {
                    problems.push(format!("{label}: the mode needs a path"));
                }
                if segment.mode == SegmentMode::StaticImages && self.c8.is_none() {
                    problems.push(format!("{label}: static_images needs the [c8] table"));
                }
                if let Some(mode) = &segment.apk_mode {
                    if !apk {
                        problems.push(format!("{label}: apk_mode needs the APK (apk = true)"));
                    }
                    if !matches!(
                        mode.as_str(),
                        "pattern" | "flip" | "step" | "static" | "replay"
                    ) {
                        problems.push(format!("{label}: unknown apk_mode {mode}"));
                    }
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
