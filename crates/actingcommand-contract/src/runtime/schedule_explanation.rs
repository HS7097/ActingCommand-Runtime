// SPDX-License-Identifier: AGPL-3.0-only

//! `RuntimeOperation::ScheduleExplanation` (Workflow #386 SR-6b, MCP-19a; register §1.143, §3
//! W5): why each candidate of the latest policy evaluation did not run, per instance, as the
//! policy host keeps it in memory. Per instance with a decision in that evaluation: its earliest
//! wake, the task selected for it with the state of its dispatch, every other decision as a
//! candidate row with its `why` (the outcome of its last reason, the code a dispatch decision
//! record's `why=` names) and its reason codes, and the instance's one not-running row. The read
//! is answered from memory; it reads and records nothing in the ledger, and it takes no part in
//! any decision.

use super::{RuntimeContractError, RuntimeContractResult};
use crate::codes::{RuntimeContractCode, ScheduleExplanationRowCode};
use crate::outcome::{CodeStr, Outcome};
use serde::{Deserialize, Serialize};

/// The most instance entries one answer holds.
pub const MAX_SCHEDULE_EXPLANATION_INSTANCES: usize = 256;
/// The most candidate rows one instance entry answers; the rest are counted in
/// `candidates_total`.
pub const MAX_SCHEDULE_EXPLANATION_CANDIDATES: usize = 1_024;
/// The most reason codes one candidate row lists: the last ones, ending with its `why`.
pub const MAX_SCHEDULE_EXPLANATION_REASONS: usize = 32;

/// Where the selected decision's dispatch stands when the read is answered: `pending` before its
/// intent is recorded, then as the policy host's dispatch projection holds it. A `rejected` one
/// did not run either.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleDispatchState {
    Pending,
    Intent,
    Admitted,
    Rejected,
    Completed,
}

/// A candidate row's state: `blocked` when the evaluation blocked the decision, `deferred` for
/// every other decision that was not selected (an eligible one that another task outranked
/// waited this round).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleCandidateState {
    Deferred,
    Blocked,
}

/// A candidate row's eligibility, as its decision has it; a row with `true` is due.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleCandidateEligibility {
    True,
    False,
    Unknown,
}

/// The decision the evaluation selected for the instance: its task, its decision id and where its
/// dispatch stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduleExplanationSelected {
    pub task_id: String,
    pub decision_id: String,
    pub dispatch_state: ScheduleDispatchState,
}

/// One decision of the instance that was not selected: `why` is the outcome of its last reason,
/// `reasons` the codes of its reasons in order (the last [`MAX_SCHEDULE_EXPLANATION_REASONS`],
/// ending with `why`'s code; `reasons_omitted` counts the earlier ones left out), its own wake,
/// and the evaluation since which the same (instance, task) has had the same `why` code without
/// a break.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduleExplanationCandidate {
    pub task_id: String,
    pub state: ScheduleCandidateState,
    pub eligibility: ScheduleCandidateEligibility,
    pub why: Outcome,
    pub reasons: Vec<CodeStr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasons_omitted: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_wake_unix_ms: Option<u64>,
    pub since_unix_ms: u64,
}

/// The instance's one not-running row (scheduling model §8.2 item 4): its first due candidate
/// row's task, `why`, wake and since; with no due row, no task and `why`
/// [`ScheduleExplanationRowCode::DueTaskMissing`] naming the instance and its earliest wake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduleExplanationNotRunning {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    pub why: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_wake_unix_ms: Option<u64>,
    pub since_unix_ms: u64,
}

/// One instance with at least one decision in the latest evaluation, by its alias: its earliest
/// wake, its selected decision, its not-running row and its candidate rows (due rows first),
/// omitted when there is none or none was asked for; `candidates_total` counts them all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduleExplanationInstance {
    pub instance_alias: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_wake_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<ScheduleExplanationSelected>,
    pub not_running: ScheduleExplanationNotRunning,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<ScheduleExplanationCandidate>,
    pub candidates_total: u32,
}

/// The answer of `ScheduleExplanation`: when the latest evaluation of this Runtime ran and under
/// which catalog (both absent before the first evaluated cycle after a start), and one entry per
/// instance it decided for, by alias.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeScheduleExplanation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluated_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_hash: Option<String>,
    pub instances: Vec<ScheduleExplanationInstance>,
}

impl RuntimeScheduleExplanation {
    /// The shape rules: at most [`MAX_SCHEDULE_EXPLANATION_INSTANCES`] entries, each instance
    /// once and in alias order; per entry a not-running row without a task exactly when its `why`
    /// is `due_task_missing`, at most [`MAX_SCHEDULE_EXPLANATION_CANDIDATES`] candidate rows and a
    /// `candidates_total` no smaller than them, no candidate the selected task, and per row one to
    /// [`MAX_SCHEDULE_EXPLANATION_REASONS`] reason codes ending with `why`'s.
    pub fn validate(&self) -> RuntimeContractResult<()> {
        let invalid = || RuntimeContractError::new(RuntimeContractCode::InvalidScheduleExplanation);
        if self.instances.len() > MAX_SCHEDULE_EXPLANATION_INSTANCES
            || self
                .instances
                .windows(2)
                .any(|pair| pair[0].instance_alias >= pair[1].instance_alias)
        {
            return Err(invalid());
        }
        for entry in &self.instances {
            let answered = u32::try_from(entry.candidates.len()).map_err(|_| invalid())?;
            let idle = entry.not_running.task_id.is_none();
            if idle
                != entry
                    .not_running
                    .why
                    .is(ScheduleExplanationRowCode::DueTaskMissing)
                || entry.candidates.len() > MAX_SCHEDULE_EXPLANATION_CANDIDATES
                || entry.candidates_total < answered
            {
                return Err(invalid());
            }
            for row in &entry.candidates {
                if row.reasons.is_empty()
                    || row.reasons.len() > MAX_SCHEDULE_EXPLANATION_REASONS
                    || row.reasons.last() != Some(row.why.code())
                    || entry
                        .selected
                        .as_ref()
                        .is_some_and(|selected| selected.task_id == row.task_id)
                {
                    return Err(invalid());
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    // Workflow #386 SR-6b, MCP-19a (host key 127; register §1.143 M19A-3): the wire rules of the
    // read and its answer.
    use super::*;
    use crate::codes::{ReceiptCode, SharedCode};
    use crate::{
        EventActor, EventSource, IdentifierIssuer, InterfaceBlock, InterfaceClass,
        InterfaceClassification, RuntimeOperation, RuntimeOperationKind, RuntimeReceipt,
        RuntimeReceiptState, RuntimeRequest, RuntimeResult,
    };

    const EVALUATED_AT: u64 = 1_700_042_460_000;

    fn code(spelling: &str) -> CodeStr {
        Outcome::relayed(spelling).code().clone()
    }

    fn row(
        task_id: &str,
        state: ScheduleCandidateState,
        eligibility: ScheduleCandidateEligibility,
        reasons: &[&str],
    ) -> ScheduleExplanationCandidate {
        let why = reasons.last().expect("a row has a reason");
        ScheduleExplanationCandidate {
            task_id: task_id.to_owned(),
            state,
            eligibility,
            why: Outcome::relayed(why),
            reasons: reasons.iter().copied().map(code).collect(),
            reasons_omitted: None,
            next_wake_unix_ms: None,
            since_unix_ms: EVALUATED_AT,
        }
    }

    /// The answer the host gives in the first scenario of key 126: one task selected, one due
    /// task outranked and one waiting for its clock.
    fn answered() -> RuntimeScheduleExplanation {
        RuntimeScheduleExplanation {
            evaluated_at_unix_ms: Some(EVALUATED_AT),
            catalog_hash: Some(format!("sha256:{}", "0".repeat(64))),
            instances: vec![ScheduleExplanationInstance {
                instance_alias: "neutral.instance".to_owned(),
                next_wake_unix_ms: Some(EVALUATED_AT + 3_600_000),
                selected: Some(ScheduleExplanationSelected {
                    task_id: "fixture.slot".to_owned(),
                    decision_id: "decision:fixture".to_owned(),
                    dispatch_state: ScheduleDispatchState::Pending,
                }),
                not_running: ScheduleExplanationNotRunning {
                    task_id: Some("fixture.second".to_owned()),
                    why: Outcome::relayed("instance_already_selected"),
                    next_wake_unix_ms: None,
                    since_unix_ms: EVALUATED_AT,
                },
                candidates: vec![
                    row(
                        "fixture.second",
                        ScheduleCandidateState::Deferred,
                        ScheduleCandidateEligibility::True,
                        &["task_eligible", "instance_already_selected"],
                    ),
                    ScheduleExplanationCandidate {
                        next_wake_unix_ms: Some(EVALUATED_AT + 3_600_000),
                        ..row(
                            "fixture.later",
                            ScheduleCandidateState::Blocked,
                            ScheduleCandidateEligibility::False,
                            &["trigger_false"],
                        )
                    },
                ],
                candidates_total: 2,
            }],
        }
    }

    fn cli_request(operation: RuntimeOperation) -> Result<RuntimeRequest, RuntimeContractError> {
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        RuntimeRequest::new(
            ids.mint_request_id().expect("request id"),
            ids.mint_correlation_id().expect("correlation id"),
            None,
            EventActor::Cli,
            EventSource::Cli,
            1,
            operation,
        )
    }

    fn read_all() -> RuntimeOperation {
        RuntimeOperation::ScheduleExplanation {
            instance: None,
            candidates: true,
        }
    }

    #[test]
    fn mcp19a_the_answer_shape_is_validated() {
        assert_eq!(answered().validate(), Ok(()));

        let invalid = RuntimeContractCode::InvalidScheduleExplanation.code();
        let mut duplicate = answered();
        let twice = duplicate.instances[0].clone();
        duplicate.instances.push(twice);
        let mut unended = answered();
        unended.instances[0].candidates[0].reasons.reverse();
        let mut too_many = answered();
        too_many.instances[0].candidates[1].reasons =
            std::iter::repeat_n(code("trigger_false"), MAX_SCHEDULE_EXPLANATION_REASONS + 1)
                .collect();
        let mut undercounted = answered();
        undercounted.instances[0].candidates_total = 1;
        for (case, explanation) in [
            ("an instance twice", duplicate),
            ("reasons that do not end with the why", unended),
            ("more than 32 reason codes", too_many),
            ("a candidates_total below the rows", undercounted),
        ] {
            assert_eq!(
                explanation.validate().map_err(RuntimeContractError::code),
                Err(invalid),
                "{case}"
            );
        }

        let request = cli_request(read_all()).expect("request");
        let refused = RuntimeReceipt::success(
            &request,
            RuntimeReceiptState::Admitted,
            None,
            RuntimeResult::ScheduleExplanation {
                explanation: Box::new(answered()),
            },
        )
        .map_err(RuntimeContractError::code);
        assert_eq!(
            refused.err(),
            Some(invalid),
            "a receipt that is not completed"
        );
    }

    #[test]
    fn mcp19a_a_not_running_row_has_no_task_exactly_when_its_why_is_due_task_missing() {
        let due_task_missing = || {
            Outcome::new(ScheduleExplanationRowCode::DueTaskMissing)
                .instance_id("neutral.instance")
                .next_eligible_unix_ms(EVALUATED_AT + 3_600_000)
        };
        let mut idle = answered();
        idle.instances[0].not_running = ScheduleExplanationNotRunning {
            task_id: None,
            why: due_task_missing(),
            next_wake_unix_ms: None,
            since_unix_ms: EVALUATED_AT,
        };
        assert_eq!(idle.validate(), Ok(()));

        let invalid = RuntimeContractCode::InvalidScheduleExplanation.code();
        let mut taskless = answered();
        taskless.instances[0].not_running.task_id = None;
        let mut tasked = answered();
        tasked.instances[0].not_running.why = due_task_missing();
        for (case, explanation) in [
            ("a row without a task whose why is another code", taskless),
            ("a row with a task whose why is due_task_missing", tasked),
        ] {
            assert_eq!(
                explanation.validate().map_err(RuntimeContractError::code),
                Err(invalid),
                "{case}"
            );
        }
    }

    #[test]
    fn mcp19a_the_read_is_a_general_read_with_its_receipt_code() {
        let operation = read_all();
        assert_eq!(operation.kind(), RuntimeOperationKind::ScheduleExplanation);
        assert_eq!(
            operation.interface_class(),
            InterfaceClassification {
                block: InterfaceBlock::General,
                class: Some(InterfaceClass::Read),
                g14_copied: false,
            }
        );

        let request = cli_request(operation).expect("request");
        let receipt = RuntimeReceipt::success(
            &request,
            RuntimeReceiptState::Completed,
            None,
            RuntimeResult::ScheduleExplanation {
                explanation: Box::new(answered()),
            },
        )
        .expect("completed receipt");
        assert!(
            receipt
                .outcome()
                .is(ReceiptCode::ScheduleExplanationCompleted),
            "{receipt:?}"
        );

        let empty_alias = cli_request(RuntimeOperation::ScheduleExplanation {
            instance: Some(String::new()),
            candidates: true,
        })
        .map_err(RuntimeContractError::code);
        assert_eq!(
            empty_alias.err(),
            Some(SharedCode::InvalidInstanceAlias.code())
        );
    }
}
