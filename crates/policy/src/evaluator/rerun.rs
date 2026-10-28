// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #386 SR-2b-2 (`contracts/policy-suspension.md`, "The rerun"): the owed reruns an
//! evaluation reads.
//!
//! - A pair (catalog task, instance alias) whose latest run ended by a condition owes a rerun.
//!   The Runtime hands the evaluation each owed pair it lets in, with the reason the Runtime
//!   writes for it (`policy_rerun_owed`, a code the Runtime registers) and whether it ranks first
//!   (`scheduling.rerun.promote`).
//! - The evaluator treats such a pair as triggered and past its task cooldown, as an immediate
//!   rerun is, and adds the reason. A pair that `promote`s ranks among the promoted candidates,
//!   so it goes before the instance's other candidates; one that does not keeps its own rank.
//! - Feedback stop, placement, budgets, the host's shared run limits and one winner per instance
//!   apply as to every candidate. The Runtime's own gates (a held instance, a blackout, the
//!   pacing stage, the expiry) are judged by the Runtime, never here.
//!
//! Memory only: none of it enters a snapshot or a decision identity.

use std::collections::BTreeMap;

use super::DecisionReason;

/// One owed rerun as the Runtime hands it to the evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwedRerun {
    /// The reason the pair's decision carries (`policy_rerun_owed` with its values).
    pub reason: DecisionReason,
    /// Whether the pair ranks among the promoted candidates (`scheduling.rerun.promote`).
    pub promote: bool,
}

/// Workflow #386 SR-2b-2: the owed reruns of one evaluation, by (task id, instance alias).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OwedReruns {
    pub pairs: BTreeMap<(String, String), OwedRerun>,
}

impl OwedReruns {
    /// The owed rerun of the pair, if the Runtime let one in.
    pub(super) fn get(&self, task_id: &str, instance_id: &str) -> Option<&OwedRerun> {
        if self.pairs.is_empty() {
            return None;
        }
        self.pairs
            .get(&(task_id.to_owned(), instance_id.to_owned()))
    }
}
