// SPDX-License-Identifier: AGPL-3.0-only

//! `RuntimeOperation::ScheduleExplanation` (Workflow #386 SR-6b, MCP-19a; register §1.143, §3
//! W5): why each candidate of the latest policy evaluation did not run, read from the table the
//! policy host keeps in memory (`crate::schedule_explanation`). Like a status read it reads and
//! appends nothing in the ledger.

use super::*;
use crate::codes::{HostCode, HostLocation};

impl HostShared {
    /// The latest evaluation's explanation for the registered instance `instance` (every
    /// instance when it is absent), with its candidate rows when `candidates` asks for them,
    /// read under the policy lock. An alias the registry does not hold is refused
    /// `instance_unknown`; an answer larger than one IPC frame is refused
    /// `runtime_schedule_explanation_too_large_for_frame` instead of failing the frame write.
    pub(super) fn schedule_explanation_read(
        &self,
        request: &RuntimeRequest,
        instance: Option<&str>,
        candidates: bool,
    ) -> Result<OperationSuccess, RequestFailure> {
        let explanation = lock(self.policy()?, HostLocation::ReadScheduleExplanation)?
            .schedule_explanation(instance, candidates);
        let result = RuntimeResult::ScheduleExplanation {
            explanation: Box::new(explanation),
        };
        let reply_bytes = RuntimeReceipt::success(
            request,
            RuntimeReceiptState::Completed,
            None,
            result.clone(),
        )
        .ok()
        .and_then(|receipt| serde_json::to_vec(&receipt).ok())
        .ok_or_else(|| RequestFailure::poison_without_terminal(receipt_error()))?
        .len();
        if reply_bytes > self.maximum_frame_bytes.saturating_mul(1_000) {
            return Err(RequestFailure::request(
                RuntimeHostError::request(
                    HostCode::RuntimeScheduleExplanationTooLargeForFrame,
                    HostLocation::ReadScheduleExplanation,
                    RuntimeErrorCode::InvalidRequest,
                )
                .with_native_detail(format!(
                    "reply is {reply_bytes} bytes; one frame carries at most {} bytes",
                    self.maximum_frame_bytes
                )),
                RuntimeReceiptState::Denied,
                None,
            ));
        }
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result,
            failure_outcome: None,
        })
    }
}
