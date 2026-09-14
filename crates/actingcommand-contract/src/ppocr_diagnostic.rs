// SPDX-License-Identifier: AGPL-3.0-only

use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub const PPOCR_DIAGNOSTIC_RESULT_SCHEMA: &str = "actingcommand.ppocr_diagnostic_result.v1";
pub const PPOCR_NODE_PLACEMENT_RECORD_TYPE: &str =
    "actingcommand.ppocr_node_placement_diagnostic.v1";
pub const PPOCR_MAX_DIAGNOSTIC_REPORTS: usize = 2;
pub const PPOCR_MAX_DIAGNOSTIC_NODES: usize = 4_096;
pub const PPOCR_MAX_NODE_LOG_BYTES: usize = 4_096;
pub const PPOCR_MAX_REPORT_JSON_BYTES: usize = 112 * 1024 * 1024;
pub const PPOCR_MAX_DIAGNOSTIC_JSON_BYTES: usize = 224 * 1024 * 1024;
pub const PPOCR_MAX_BUSINESS_JSON_BYTES: usize = 128 * 1024 * 1024;
pub const PPOCR_MAX_ENVELOPE_BYTES: usize = 32 * 1024 * 1024;
pub const PPOCR_MAX_RESPONSE_BYTES: usize = 384 * 1024 * 1024;

/// Reports owned by the current result/error, shared through typed conversion layers.
pub type PpocrDiagnostics = Vec<Arc<PpocrNodePlacementDiagnostic>>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PpocrCpuAssignedNodeDiagnostic {
    pub node_name: String,
    pub operator_type: String,
    pub domain: String,
    pub placement_reason: String,
    pub assigned_execution_provider: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PpocrNodePlacementDiagnostic {
    pub record_type: String,
    pub model_role: String,
    pub diagnostic_scope: String,
    pub inference_executed: bool,
    pub cpu_assigned_node_count: usize,
    pub nodes: Vec<PpocrCpuAssignedNodeDiagnostic>,
}

impl PpocrNodePlacementDiagnostic {
    pub fn validate(&self) -> Result<(), String> {
        if self.record_type != PPOCR_NODE_PLACEMENT_RECORD_TYPE
            || !matches!(self.model_role.as_str(), "recognizer" | "detector")
            || self.diagnostic_scope != "session_initialization_only"
            || self.inference_executed
            || self.nodes.len() > PPOCR_MAX_DIAGNOSTIC_NODES
            || self.cpu_assigned_node_count != self.nodes.len()
        {
            return Err("invalid PPOCR node-placement report identity or count".to_string());
        }
        for node in &self.nodes {
            let source_bytes = node
                .node_name
                .len()
                .checked_add(node.operator_type.len())
                .ok_or_else(|| "PPOCR node diagnostic size overflow".to_string())?;
            if node.node_name.is_empty()
                || node.operator_type.is_empty()
                || source_bytes > PPOCR_MAX_NODE_LOG_BYTES
                || node.domain != "unavailable"
                || node.placement_reason != "unavailable"
                || node.assigned_execution_provider != "CPUExecutionProvider"
            {
                return Err("invalid PPOCR node-placement detail or source bound".to_string());
            }
        }
        // One source log is split into name/operator; JSON escapes each byte at most sixfold.
        let maximum = PPOCR_MAX_DIAGNOSTIC_NODES
            .checked_mul(
                PPOCR_MAX_NODE_LOG_BYTES
                    .checked_mul(6)
                    .and_then(|bytes| bytes.checked_add(512))
                    .ok_or_else(|| "PPOCR node JSON bound overflow".to_string())?,
            )
            .and_then(|bytes| bytes.checked_add(1024))
            .ok_or_else(|| "PPOCR report JSON bound overflow".to_string())?;
        if maximum > PPOCR_MAX_REPORT_JSON_BYTES {
            return Err("PPOCR report structure exceeds its JSON budget".to_string());
        }
        Ok(())
    }
}

pub fn validate_ppocr_call_diagnostics(
    reports: &[Arc<PpocrNodePlacementDiagnostic>],
) -> Result<(), String> {
    if reports.len() > PPOCR_MAX_DIAGNOSTIC_REPORTS {
        return Err("PPOCR invocation produced more than two diagnostic reports".to_string());
    }
    for (index, report) in reports.iter().enumerate() {
        report.validate()?;
        if reports[..index]
            .iter()
            .any(|earlier| earlier.model_role == report.model_role)
        {
            return Err("PPOCR invocation repeated a model diagnostic".to_string());
        }
    }
    Ok(())
}
