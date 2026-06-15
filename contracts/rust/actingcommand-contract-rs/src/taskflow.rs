// SPDX-License-Identifier: AGPL-3.0-only

//! Rust backup data structures for declarative task-flow contracts.

use crate::types::*;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub struct TaskFlow {
    pub schema_version: String,
    pub id: String,
    pub name: String,
    pub game: GameKey,
    pub servers: Vec<ServerKey>,
    pub resolutions: Vec<Resolution>,
    pub entrypoint: String,
    pub tasks: Vec<TaskDefinition>,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TaskDefinition {
    pub id: TaskId,
    pub name: String,
    pub steps: Vec<TaskStep>,
    pub on_failure: FailurePolicy,
    pub produces: Vec<String>,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TaskStep {
    pub id: String,
    pub description: Option<String>,
    pub primitive: String,
    pub params: BTreeMap<String, TaskParamValue>,
    pub when: Option<String>,
    pub next: Option<String>,
    pub on_failure: Option<FailurePolicy>,
    pub timeout_ms: Option<DurationMillis>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FailurePolicy {
    pub severity: Severity,
    pub retry_limit: Option<i32>,
    pub retry_delay_ms: Option<DurationMillis>,
    pub fallback_step: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TaskParamValue {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<TaskParamValue>),
    Object(BTreeMap<String, TaskParamValue>),
}
