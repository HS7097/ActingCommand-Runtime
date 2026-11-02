// SPDX-License-Identifier: AGPL-3.0-only

//! The Runtime's own forwarding to agent sessions (Workflow #432; `contracts/agent-wake.md`; Alice's
//! words of 2026-11-02: it is a function of the Runtime, with a fixed header naming a registered
//! channel).
//!
//! AGT-1a holds the skeleton only: the [`AgentForward`] trait with its neutral types, the
//! [`Unavailable`] implementation that answers until a transport exists, [`FakeForward`] behind
//! `test-support`, and the channel registry's shape and validation ([`channels`]). This crate
//! opens no pipe and starts no process yet. It is the only crate that names a client, and every
//! type the trait answers with is neutral, so its callers never need a client word.

#![forbid(unsafe_code)]

pub mod channels;
#[cfg(any(test, feature = "test-support"))]
mod fake;

#[cfg(any(test, feature = "test-support"))]
pub use fake::{FakeForward, FakeSend};

use std::time::Instant;

/// One message to one channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardMessage {
    /// The wake's id, the same for every try of the message.
    pub id: [u8; 16],
    pub text: String,
}

/// What bounds one send.
pub struct SendLimits<'a> {
    pub deadline: Instant,
    /// Whether the Runtime is stopping.
    pub stopped: &'a dyn Fn() -> bool,
}

/// `before_write` refused: the intent could not be committed, so nothing may be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntentFailed;

/// What a client's acknowledgement or write proves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryProof {
    Written,
    Accepted,
    Recorded,
    Receipt,
}

impl DeliveryProof {
    /// The word a delivery row records.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Written => "written",
            Self::Accepted => "accepted",
            Self::Recorded => "recorded",
            Self::Receipt => "receipt",
        }
    }
}

/// Why a message waits: no write happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeferReason {
    TargetOffline,
    TargetNotLoaded,
    TargetAwaitingInput,
    IpcUnavailable,
    SidecarOffline,
    RegistryUnreadable,
    TargetUnresponsive,
}

impl DeferReason {
    /// The word a delivery row records.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TargetOffline => "target_offline",
            Self::TargetNotLoaded => "target_not_loaded",
            Self::TargetAwaitingInput => "target_awaiting_input",
            Self::IpcUnavailable => "ipc_unavailable",
            Self::SidecarOffline => "sidecar_offline",
            Self::RegistryUnreadable => "registry_unreadable",
            Self::TargetUnresponsive => "target_unresponsive",
        }
    }
}

/// What went wrong with a send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryProblem {
    ProtocolChanged,
    Rejected,
    OwnerMismatch,
    WriteTimedOut,
    WriteFailed,
    ReceiptMismatch,
    HelperFailed,
    ChannelUnknown,
}

impl DeliveryProblem {
    /// The word a delivery row records.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProtocolChanged => "protocol_changed",
            Self::Rejected => "rejected",
            Self::OwnerMismatch => "owner_mismatch",
            Self::WriteTimedOut => "write_timed_out",
            Self::WriteFailed => "write_failed",
            Self::ReceiptMismatch => "receipt_mismatch",
            Self::HelperFailed => "helper_failed",
            Self::ChannelUnknown => "channel_unknown",
        }
    }
}

/// How one send ended. `detail` is free text for the delivery detail file, bounded by the caller;
/// it never reaches a ledger field and never holds a credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForwardOutcome {
    /// No write happened; the message may be tried again.
    Deferred { reason: DeferReason, detail: String },
    /// A client acknowledged or wrote.
    Delivered {
        proof: DeliveryProof,
        detail: String,
    },
    /// Certainly no write, or a definite refusal.
    Failed {
        problem: DeliveryProblem,
        detail: String,
    },
    /// A write may have happened; the message is never tried again.
    Unknown {
        problem: DeliveryProblem,
        detail: String,
    },
}

/// A way to hand a message to a registered channel.
pub trait AgentForward: Send + Sync {
    /// Sends `message` to `channel`. `before_write` commits the intent: a transport calls it after
    /// its preflight and immediately before its first possible write, exactly once, and writes
    /// nothing when it errors.
    fn send(
        &self,
        channel: &str,
        message: &ForwardMessage,
        limits: &SendLimits<'_>,
        before_write: &mut dyn FnMut() -> Result<(), IntentFailed>,
    ) -> ForwardOutcome;
}

/// The forwarder of a build with no transport: every message waits, and no write happens.
#[derive(Debug, Clone, Copy, Default)]
pub struct Unavailable;

impl AgentForward for Unavailable {
    fn send(
        &self,
        _channel: &str,
        _message: &ForwardMessage,
        _limits: &SendLimits<'_>,
        _before_write: &mut dyn FnMut() -> Result<(), IntentFailed>,
    ) -> ForwardOutcome {
        ForwardOutcome::Deferred {
            reason: DeferReason::IpcUnavailable,
            detail: "this build has no transport".to_owned(),
        }
    }
}
