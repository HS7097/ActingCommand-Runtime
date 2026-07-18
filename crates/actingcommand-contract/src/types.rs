// SPDX-License-Identifier: AGPL-3.0-only

//! Shared runtime model types for Rust mainline contracts.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub type Metadata = BTreeMap<String, String>;
pub type Timestamp = String;
pub type DurationMillis = u64;
pub type GameKey = String;
pub type ServerKey = String;
pub type EngineKind = String;
pub type RuntimeState = String;
pub type Severity = String;
pub type ProfileId = String;
pub type TaskId = String;
pub type TaskRunId = String;
pub type ResourceKey = String;
pub type ContractResult<T> = Result<T, RuntimeError>;

pub const ENGINE_NATIVE: &str = "native";
pub const ENGINE_DELEGATED: &str = "delegated";

pub const RUNTIME_UNKNOWN: &str = "unknown";
pub const RUNTIME_STOPPED: &str = "stopped";
pub const RUNTIME_STARTING: &str = "starting";
pub const RUNTIME_RUNNING: &str = "running";
pub const RUNTIME_STOPPING: &str = "stopping";
pub const RUNTIME_DEGRADED: &str = "degraded";
pub const RUNTIME_FATAL: &str = "fatal";

pub const SEVERITY_INFO: &str = "info";
pub const SEVERITY_WARNING: &str = "warning";
pub const SEVERITY_ERROR: &str = "error";
pub const SEVERITY_FATAL: &str = "fatal";
pub const SEVERITY_DEGRADED: &str = "degraded";

/// Request-scoped metadata used by Rust adapters in place of Go's `context.Context`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RuntimeContext {
    pub request_id: String,
    pub deadline_at: Option<Timestamp>,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Resolution {
    pub width: i32,
    pub height: i32,
    pub scale: Option<f64>,
    pub dpi: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeError {
    pub severity: Severity,
    pub code: String,
    pub message: String,
    pub module: String,
    pub original_error: Option<String>,
    pub fallback_path: Option<String>,
    pub user_visible_impact: Option<String>,
    pub context: Metadata,
    pub occurred_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogEvent {
    pub timestamp: Timestamp,
    pub level: Severity,
    pub source: String,
    pub message: String,
    pub context: Metadata,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchedulerSummary {
    pub alive: bool,
    pub current_task: Option<String>,
    pub next_task: Option<String>,
    pub next_run_at: Option<Timestamp>,
    pub pending_count: i32,
    pub waiting_count: i32,
    pub last_severity: Severity,
    pub state: RuntimeState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileSummary {
    pub id: ProfileId,
    pub name: String,
    pub game: GameKey,
    pub server: ServerKey,
    pub locale: Option<String>,
    pub resolution: Resolution,
    pub runtime_state: RuntimeState,
    pub scheduler: SchedulerSummary,
    pub resource_snapshot: BTreeMap<ResourceKey, Resource>,
    pub resource_history: Vec<ResourceHistoryPoint>,
    pub recent_acquisitions: Vec<AcquisitionCapture>,
    pub recent_logs: Vec<LogEvent>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Resource {
    pub key: ResourceKey,
    pub value: String,
    pub observed_at: Timestamp,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResourceHistoryPoint {
    pub timestamp: Timestamp,
    pub profile_id: ProfileId,
    pub game: GameKey,
    pub server: ServerKey,
    pub key: ResourceKey,
    pub value: String,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcquisitionCapture {
    pub id: String,
    pub profile_id: ProfileId,
    pub game: GameKey,
    pub server: ServerKey,
    pub locale: Option<String>,
    pub resolution: Resolution,
    pub task_id: TaskId,
    pub task_run_id: TaskRunId,
    pub captured_at: Timestamp,
    pub image_ref: String,
    pub image_hash: Option<String>,
    pub source_trigger: String,
    pub recognition_state: String,
    pub labels: Vec<String>,
    pub retention_class: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeStatus {
    pub state: RuntimeState,
    pub started_at: Option<Timestamp>,
    pub state_dir: String,
    pub version: String,
    pub scheduler: SchedulerSummary,
    pub last_error: Option<RuntimeError>,
    pub profiles: Vec<ProfileSummary>,
    pub capabilities: Vec<RuntimeCapability>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeCapability {
    pub name: String,
    pub version: Option<String>,
    pub status: RuntimeState,
    pub description: Option<String>,
    pub metadata: Metadata,
}
