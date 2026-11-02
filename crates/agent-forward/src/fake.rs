// SPDX-License-Identifier: AGPL-3.0-only

//! A scripted [`AgentForward`] for tests: it answers the scripted outcomes in order and records,
//! for each send, whether `before_write` ran before its "write".

use crate::{
    AgentForward, DeferReason, DeliveryProblem, ForwardMessage, ForwardOutcome, IntentFailed,
    SendLimits,
};
use std::collections::VecDeque;
use std::sync::{Mutex, PoisonError};

/// One send as the fake saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeSend {
    pub channel: String,
    pub message: ForwardMessage,
    /// Whether `before_write` ran.
    pub intent_committed: bool,
    /// Whether the "write" happened, always after a committed intent.
    pub wrote: bool,
}

/// The scripted forwarder. An outcome that means a write may have happened (`Delivered`,
/// `Unknown`) commits the intent first, as a transport does; a refused intent writes nothing and
/// answers `Failed`. With the script used up, a send answers `Deferred`.
#[derive(Debug, Default)]
pub struct FakeForward {
    script: Mutex<VecDeque<ForwardOutcome>>,
    sends: Mutex<Vec<FakeSend>>,
}

impl FakeForward {
    /// A fake that answers `outcomes` in order.
    pub fn new(outcomes: impl IntoIterator<Item = ForwardOutcome>) -> Self {
        Self {
            script: Mutex::new(outcomes.into_iter().collect()),
            sends: Mutex::new(Vec::new()),
        }
    }

    /// Every send so far.
    pub fn sends(&self) -> Vec<FakeSend> {
        self.sends
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl AgentForward for FakeForward {
    fn send(
        &self,
        channel: &str,
        message: &ForwardMessage,
        _limits: &SendLimits<'_>,
        before_write: &mut dyn FnMut() -> Result<(), IntentFailed>,
    ) -> ForwardOutcome {
        let scripted = self
            .script
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front()
            .unwrap_or_else(|| ForwardOutcome::Deferred {
                reason: DeferReason::TargetOffline,
                detail: "the script is used up".to_owned(),
            });
        let writes = matches!(
            scripted,
            ForwardOutcome::Delivered { .. } | ForwardOutcome::Unknown { .. }
        );
        let committed = writes && before_write().is_ok();
        self.sends
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(FakeSend {
                channel: channel.to_owned(),
                message: message.clone(),
                intent_committed: committed,
                wrote: committed,
            });
        if writes && !committed {
            return ForwardOutcome::Failed {
                problem: DeliveryProblem::WriteFailed,
                detail: "the intent was refused".to_owned(),
            };
        }
        scripted
    }
}
