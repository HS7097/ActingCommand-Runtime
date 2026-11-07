// SPDX-License-Identifier: AGPL-3.0-only

use crate::codes::PpocrDiagnosticCode;
use crate::outcome::Code;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;
use std::sync::Arc;

pub const PPOCR_DIAGNOSTIC_RESULT_SCHEMA: &str = "actingcommand.ppocr_diagnostic_result.v1";
/// Workflow #407 GPU-1 B1 (design `rd-1031/perf-407/design-gpu1.md` §2.2): v2 adds the
/// accelerated execution provider with its node count and, for DirectML, the adapter.
pub const PPOCR_NODE_PLACEMENT_RECORD_TYPE: &str =
    "actingcommand.ppocr_node_placement_diagnostic.v2";
/// ONNX Runtime's name of its CPU execution provider.
pub const PPOCR_CPU_EXECUTION_PROVIDER: &str = "CPUExecutionProvider";
/// ONNX Runtime's name of its DirectML execution provider: the one provider whose report names
/// its adapter.
pub const PPOCR_DIRECTML_EXECUTION_PROVIDER: &str = "DmlExecutionProvider";
/// The longest execution provider name a report holds.
pub const PPOCR_MAX_EXECUTION_PROVIDER_NAME_BYTES: usize = 64;
/// The longest adapter identity or LUID a report holds.
pub const PPOCR_MAX_DEVICE_FIELD_BYTES: usize = 64;
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

/// Why the contract refused a PPOCR node-placement report or a call's set of reports
/// (Workflow #378 A2a): one registered code; its English is developer text only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PpocrDiagnosticError {
    code: PpocrDiagnosticCode,
}

impl PpocrDiagnosticError {
    const fn new(code: PpocrDiagnosticCode) -> Self {
        Self { code }
    }

    pub const fn code(self) -> Code {
        self.code.code()
    }
}

impl fmt::Display for PpocrDiagnosticError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self.code {
            PpocrDiagnosticCode::PpocrDiagnosticReportIdentityInvalid => {
                "invalid PPOCR node-placement report identity or count"
            }
            PpocrDiagnosticCode::PpocrDiagnosticNodeSizeOverflow => {
                "PPOCR node diagnostic size overflow"
            }
            PpocrDiagnosticCode::PpocrDiagnosticNodeDetailInvalid => {
                "invalid PPOCR node-placement detail or source bound"
            }
            PpocrDiagnosticCode::PpocrDiagnosticJsonBoundOverflow => {
                "PPOCR node or report JSON bound overflow"
            }
            PpocrDiagnosticCode::PpocrDiagnosticReportBudgetExceeded => {
                "PPOCR report structure exceeds its JSON budget"
            }
            PpocrDiagnosticCode::PpocrDiagnosticReportCountExceeded => {
                "PPOCR invocation produced more than two diagnostic reports"
            }
            PpocrDiagnosticCode::PpocrDiagnosticModelRepeated => {
                "PPOCR invocation repeated a model diagnostic"
            }
        };
        write!(formatter, "{}: {text}", self.code)
    }
}

impl Error for PpocrDiagnosticError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PpocrCpuAssignedNodeDiagnostic {
    pub node_name: String,
    pub operator_type: String,
    pub domain: String,
    pub placement_reason: PpocrNodePlacementReason,
    pub assigned_execution_provider: String,
}

/// Why ONNX Runtime placed a node on the CPU (Workflow #378 A2-P: a closed token). Its
/// placement log names no reason, so the one token is `unavailable`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PpocrNodePlacementReason {
    /// The placement log names no reason.
    Unavailable,
}

/// The DXGI adapter a DirectML session was built on (Workflow #407 GPU-1 B1, design §2.2): its
/// stable identity, its DXGI index when the session was built, and its LUID, an observation of
/// that boot only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PpocrDiagnosticDevice {
    pub identity: String,
    pub dxgi_index: u32,
    pub luid: String,
}

impl PpocrDiagnosticDevice {
    fn is_bounded(&self) -> bool {
        is_bounded_device_field(&self.identity) && is_bounded_device_field(&self.luid)
    }
}

/// Where ONNX Runtime placed one model's nodes when its session was built (record type v2). The
/// CPU-placed nodes are listed; the nodes on the accelerated provider are counted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PpocrNodePlacementDiagnostic {
    pub record_type: String,
    pub model_role: String,
    pub diagnostic_scope: String,
    pub inference_executed: bool,
    /// The provider the session registered, as ONNX Runtime names it (`DmlExecutionProvider`,
    /// `CUDAExecutionProvider`); never the CPU provider.
    pub accelerated_execution_provider: String,
    /// How many nodes ONNX Runtime placed on `accelerated_execution_provider`.
    pub accelerated_node_count: usize,
    pub cpu_assigned_node_count: usize,
    /// The adapter of a DirectML session; absent for every other provider.
    pub device: Option<PpocrDiagnosticDevice>,
    pub nodes: Vec<PpocrCpuAssignedNodeDiagnostic>,
}

impl PpocrNodePlacementDiagnostic {
    pub fn validate(&self) -> Result<(), PpocrDiagnosticError> {
        let provider = self.accelerated_execution_provider.as_str();
        if self.record_type != PPOCR_NODE_PLACEMENT_RECORD_TYPE
            || !matches!(self.model_role.as_str(), "recognizer" | "detector")
            || self.diagnostic_scope != "session_initialization_only"
            || self.inference_executed
            || !is_execution_provider_name(provider)
            || provider == PPOCR_CPU_EXECUTION_PROVIDER
            || self.device.is_some() != (provider == PPOCR_DIRECTML_EXECUTION_PROVIDER)
            || self
                .device
                .as_ref()
                .is_some_and(|device| !device.is_bounded())
            || self.nodes.len() > PPOCR_MAX_DIAGNOSTIC_NODES
            || self.cpu_assigned_node_count != self.nodes.len()
        {
            return Err(PpocrDiagnosticError::new(
                PpocrDiagnosticCode::PpocrDiagnosticReportIdentityInvalid,
            ));
        }
        for node in &self.nodes {
            let source_bytes = node
                .node_name
                .len()
                .checked_add(node.operator_type.len())
                .ok_or(PpocrDiagnosticError::new(
                    PpocrDiagnosticCode::PpocrDiagnosticNodeSizeOverflow,
                ))?;
            if node.node_name.is_empty()
                || node.operator_type.is_empty()
                || source_bytes > PPOCR_MAX_NODE_LOG_BYTES
                || node.domain != "unavailable"
                || node.assigned_execution_provider != PPOCR_CPU_EXECUTION_PROVIDER
            {
                return Err(PpocrDiagnosticError::new(
                    PpocrDiagnosticCode::PpocrDiagnosticNodeDetailInvalid,
                ));
            }
        }
        // One source log is split into name/operator; JSON escapes each byte at most sixfold.
        let maximum = PPOCR_MAX_DIAGNOSTIC_NODES
            .checked_mul(
                PPOCR_MAX_NODE_LOG_BYTES
                    .checked_mul(6)
                    .and_then(|bytes| bytes.checked_add(512))
                    .ok_or(PpocrDiagnosticError::new(
                        PpocrDiagnosticCode::PpocrDiagnosticJsonBoundOverflow,
                    ))?,
            )
            .and_then(|bytes| bytes.checked_add(1024))
            .ok_or(PpocrDiagnosticError::new(
                PpocrDiagnosticCode::PpocrDiagnosticJsonBoundOverflow,
            ))?;
        if maximum > PPOCR_MAX_REPORT_JSON_BYTES {
            return Err(PpocrDiagnosticError::new(
                PpocrDiagnosticCode::PpocrDiagnosticReportBudgetExceeded,
            ));
        }
        Ok(())
    }
}

/// An execution provider name as ONNX Runtime spells them: ASCII letters and digits, bounded.
fn is_execution_provider_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= PPOCR_MAX_EXECUTION_PROVIDER_NAME_BYTES
        && name.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

/// An adapter identity (`dxgi-pci:…`) or LUID (`0x…`): ASCII letters, digits, `:` and `-`,
/// bounded, so that its JSON needs no escape.
fn is_bounded_device_field(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= PPOCR_MAX_DEVICE_FIELD_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b':' || byte == b'-')
}

pub fn validate_ppocr_call_diagnostics(
    reports: &[Arc<PpocrNodePlacementDiagnostic>],
) -> Result<(), PpocrDiagnosticError> {
    if reports.len() > PPOCR_MAX_DIAGNOSTIC_REPORTS {
        return Err(PpocrDiagnosticError::new(
            PpocrDiagnosticCode::PpocrDiagnosticReportCountExceeded,
        ));
    }
    for (index, report) in reports.iter().enumerate() {
        report.validate()?;
        if reports[..index]
            .iter()
            .any(|earlier| earlier.model_role == report.model_role)
        {
            return Err(PpocrDiagnosticError::new(
                PpocrDiagnosticCode::PpocrDiagnosticModelRepeated,
            ));
        }
    }
    Ok(())
}
