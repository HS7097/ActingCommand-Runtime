// SPDX-License-Identifier: AGPL-3.0-only

//! Rust mainline boundary for native engines and delegated upstream backends.

use crate::types::*;
use serde::{Deserialize, Serialize};

pub trait GameEngine {
    fn describe(&self, ctx: &RuntimeContext) -> ContractResult<GameEngineDescriptor>;

    fn resolve_profile(
        &self,
        ctx: &RuntimeContext,
        profile_id: ProfileId,
    ) -> ContractResult<ProfileSummary>;

    fn status(&self, ctx: &RuntimeContext, profile_id: ProfileId) -> ContractResult<RuntimeStatus>;

    fn start(
        &mut self,
        ctx: &RuntimeContext,
        command: RuntimeCommand,
    ) -> ContractResult<CommandResult>;

    fn stop(
        &mut self,
        ctx: &RuntimeContext,
        command: RuntimeCommand,
    ) -> ContractResult<CommandResult>;

    fn restart(
        &mut self,
        ctx: &RuntimeContext,
        command: RuntimeCommand,
    ) -> ContractResult<CommandResult>;

    fn refresh(
        &mut self,
        ctx: &RuntimeContext,
        command: RuntimeCommand,
    ) -> ContractResult<CommandResult>;

    fn submit_task(
        &mut self,
        ctx: &RuntimeContext,
        request: TaskRequest,
    ) -> ContractResult<TaskRunSummary>;

    fn scheduler(
        &self,
        ctx: &RuntimeContext,
        profile_id: ProfileId,
    ) -> ContractResult<SchedulerSummary>;

    fn recent_logs(
        &self,
        ctx: &RuntimeContext,
        query: RecentQuery,
    ) -> ContractResult<Vec<LogEvent>>;

    fn resource_history(
        &self,
        ctx: &RuntimeContext,
        query: ResourceHistoryQuery,
    ) -> ContractResult<Vec<ResourceHistoryPoint>>;

    fn recent_acquisitions(
        &self,
        ctx: &RuntimeContext,
        query: AcquisitionQuery,
    ) -> ContractResult<Vec<AcquisitionCapture>>;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameEngineDescriptor {
    pub id: String,
    pub kind: EngineKind,
    pub game: GameKey,
    pub supported_servers: Vec<ServerKey>,
    pub supported_resolutions: Vec<Resolution>,
    pub capabilities: Vec<RuntimeCapability>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeCommand {
    pub profile_id: ProfileId,
    pub request_id: String,
    pub reason: Option<String>,
    pub options: Metadata,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandResult {
    pub request_id: String,
    pub state: RuntimeState,
    pub accepted: bool,
    pub message: Option<String>,
    pub error: Option<RuntimeError>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskRequest {
    pub profile_id: ProfileId,
    pub request_id: String,
    pub task_id: TaskId,
    pub flow_id: String,
    pub options: Metadata,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskRunSummary {
    pub task_run_id: TaskRunId,
    pub task_id: TaskId,
    pub profile_id: ProfileId,
    pub state: RuntimeState,
    pub started_at: Timestamp,
    pub ended_at: Option<Timestamp>,
    pub last_error: Option<RuntimeError>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecentQuery {
    pub profile_id: Option<ProfileId>,
    pub limit: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResourceHistoryQuery {
    pub profile_id: Option<ProfileId>,
    pub key: Option<ResourceKey>,
    pub limit: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcquisitionQuery {
    pub profile_id: Option<ProfileId>,
    pub task_run_id: Option<TaskRunId>,
    pub limit: Option<i32>,
}
