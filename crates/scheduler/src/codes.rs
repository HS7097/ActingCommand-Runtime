// SPDX-License-Identifier: AGPL-3.0-only

//! The scheduler's outcome codes (Workflow #378 A2b). Every entry is listed in the fragment
//! `contracts/outcome-codes/scheduler.json`; the outcome guard (G1) checks both directions. The
//! lease codes that the Runtime's wire error codes also name (`lease_busy`, `lease_expired`, …)
//! are the contract's (`RuntimeErrorCode`, `SharedCode`, decision D3). Every entry is pending
//! (B2 settles them).

actingcommand_contract::outcome_codes! {
    /// The scheduler's own refusals (`SchedulerError::code`).
    pub enum SchedulerCode {
        InvalidSchedulerConfig => "invalid_scheduler_config": error / none,
        InvalidConnectionId => "invalid_connection_id": error / none,
        IdentifierIssuanceFailed => "identifier_issuance_failed": error / none,
        LeaseExpiryOverflow => "lease_expiry_overflow": error / none,
        LeaseNotExpired => "lease_not_expired": error / none,
        LeaseQueueFull => "lease_queue_full": error / none,
        LeaseQueueMissing => "lease_queue_missing": error / none,
        LeaseQueueRequestMismatch => "lease_queue_request_mismatch": error / none,
        LeaseQueueTimeoutInvalid => "lease_queue_timeout_invalid": error / none,
        LeaseQueueSequenceOverflow => "lease_queue_sequence_overflow": error / none,
        LeaseStepSequenceOverflow => "lease_step_sequence_overflow": error / none,
        LeaseTransferNotSafe => "lease_transfer_not_safe": error / none,
        DestructiveStateMismatch => "destructive_state_mismatch": error / none,
        ResourceCloseOnly => "resource_close_only": error / none,
    }
}
