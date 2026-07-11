// SPDX-License-Identifier: AGPL-3.0-only

//! Temporary compatibility re-export for the C5 task-planning ownership migration.

#![forbid(unsafe_code)]

pub use actingcommand_execution_kernel::{
    DryRunAction, DryRunResult, DryRunStatus, DryRunTaskLoop, ProbeAction, ProbeClickEffect,
    ProbeDecisionLoop, ProbeExpectation, ProbePlan, ProbeReferenceOverrides, ProbeStep,
    ProbeStepDecision, ResourcePolicy, ResourcePolicyKind, TaskAction, TaskLoopError,
    TaskLoopErrorSeverity, TaskLoopResult, TaskPlan, TaskStep, load_probe_plan_from_json_str,
    load_task_plan_from_json_str,
};
