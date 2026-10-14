// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #386 SR-1 (scheduling model v1.4.2 §2.1): the observe-only retries of one
//! observation. An observation is one capture with its multi-frame sampling and recognition.
//! When its sampling runs past its window (`recognition_sample_deadline_exceeded`,
//! `candidate_sample_deadline_exceeded`), the same observation is made again in a new window,
//! never repeating an input, as many times as the step's pacing says
//! ([`super::StepPacing::sample_timeout_retries`]). Once the retries are spent the run ends with
//! the timeout's code and its `attempts` (the retries made), `timeout_ms` (the observation's
//! window) and `stage` (which observation). A retry whose new window can no longer hold the
//! recognition ends at that observation's own budget check, before any capture
//! (`recognition_sample_budget_insufficient`, `candidate_sample_budget_insufficient`, §2.1 item
//! 5). A retry is no failure and never reaches the recovery ladder (§2.1 item 7).

use super::{ContainedTaskError, ContainedTaskRunError, StepPacing};
use crate::codes::{ExecutionKernelCode, ExecutionKernelLocation};
use actingcommand_contract::outcome::Outcome;
use std::time::Duration;

/// The sampling timeouts an observe-only retry answers.
const TIMEOUTS: [ExecutionKernelCode; 2] = [
    ExecutionKernelCode::RecognitionSampleDeadlineExceeded,
    ExecutionKernelCode::CandidateSampleDeadlineExceeded,
];

/// The observe-only retries of one observation and how many were made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SampleRetries {
    left: u32,
    made: u32,
    stage: ExecutionKernelLocation,
}

impl SampleRetries {
    /// The retries the step's `pacing` gives an observation at `stage`.
    pub(super) const fn new(pacing: StepPacing, stage: ExecutionKernelLocation) -> Self {
        Self {
            left: pacing.sample_timeout_retries,
            made: 0,
            stage,
        }
    }

    /// After a failed observation whose window was `window`: `Ok(())` when it ended by a
    /// sampling timeout and a retry is left, so the caller opens a new window and observes
    /// again without any input; the failure otherwise. A sampling timeout whose retries are
    /// spent carries `attempts`, `timeout_ms` and `stage`.
    pub(super) fn retry<E>(
        &mut self,
        failure: ContainedTaskRunError<E>,
        window: Duration,
    ) -> Result<(), ContainedTaskRunError<E>> {
        let ContainedTaskRunError::Task(error) = failure else {
            return Err(failure);
        };
        let Some(code) = timeout_code(&error) else {
            return Err(error.into());
        };
        if self.left == 0 {
            let outcome = self.timeout_outcome(code, window);
            return Err(error.with_values(outcome).into());
        }
        self.left -= 1;
        self.made += 1;
        Ok(())
    }

    /// The outcome of `error` as the observation stands, for a record written before the retry
    /// is decided (a selection's confirmation): a sampling timeout with the retries made so
    /// far, `window` and the stage; any other failure as it is.
    pub(super) fn outcome(&self, error: &ContainedTaskError, window: Duration) -> Outcome {
        match timeout_code(error) {
            Some(code) if !error.has_values() => self.timeout_outcome(code, window),
            _ => error.outcome(),
        }
    }

    fn timeout_outcome(&self, code: ExecutionKernelCode, window: Duration) -> Outcome {
        Outcome::new(code)
            .attempts(self.made)
            .timeout_ms(window)
            .stage(self.stage)
    }
}

/// Whether `failure` is a sampling timeout of an observation.
pub(super) fn is_timeout<E>(failure: &ContainedTaskRunError<E>) -> bool {
    matches!(failure, ContainedTaskRunError::Task(error) if timeout_code(error).is_some())
}

/// The sampling timeout `error` is, if it is one.
fn timeout_code(error: &ContainedTaskError) -> Option<ExecutionKernelCode> {
    TIMEOUTS
        .into_iter()
        .find(|code| error.code().as_str() == code.as_str())
}
