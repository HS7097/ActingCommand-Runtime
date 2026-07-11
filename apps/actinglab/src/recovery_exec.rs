// SPDX-License-Identifier: AGPL-3.0-only

//! Compatibility exports for execution-owned recovery primitives.

pub use actingcommand_lab::{
    DetectKind, RecoveryAction, RecoveryExecError, RecoveryExecutionReport, RecoveryGraph,
    RecoveryNode, RecoveryResult, RecoveryRuntime, RecoverySignal, RecoveryStatus,
    execute_recovery_graph,
};
