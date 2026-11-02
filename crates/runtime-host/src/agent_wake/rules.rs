// SPDX-License-Identifier: AGPL-3.0-only

//! The rules of an automatic wake (Workflow #432 AGT-2; `contracts/agent-wake.md`, "Rules and
//! guards"): which candidate a rule matches, and which wakes one instance's burst of candidates
//! makes. Pure: the worker hands the section's rules and the burst.
//!
//! A rule matches a candidate of one of its `kinds`, on an instance its `instances` covers; a
//! `run_failed` candidate also needs its code in the rule's `codes` (no list, no wake). A burst
//! makes at most one wake per channel: of the rules that send to a channel, the one whose matched
//! candidate is the strongest (`ladder_exhausted` before `instance_fenced` before `run_failed`;
//! the earlier one on a tie; the rule written first on a tie of both), and every other candidate
//! of the burst is listed as related.

use actingcommand_runtime_config::agent_wake::{
    KIND_INSTANCE_FENCED, KIND_LADDER_EXHAUSTED, KIND_RUN_FAILED, WakeInstances, WakeRule,
};

/// How strong a kind is when one instance's candidates are coalesced: the stronger one wakes.
pub(crate) fn strength(kind: &str) -> u8 {
    match kind {
        KIND_LADDER_EXHAUSTED => 3,
        KIND_INSTANCE_FENCED => 2,
        KIND_RUN_FAILED => 1,
        _ => 0,
    }
}

/// Whether `rule` matches a candidate of `kind` and code `spelling` on the instance `alias`.
pub(crate) fn matches(rule: &WakeRule, kind: &str, spelling: &str, alias: Option<&str>) -> bool {
    let covered = match &rule.instances {
        WakeInstances::All => true,
        WakeInstances::Listed(aliases) => {
            alias.is_some_and(|alias| aliases.iter().any(|listed| listed == alias))
        }
    };
    covered
        && rule.kinds.iter().any(|listed| listed == kind)
        && (kind != KIND_RUN_FAILED || rule.codes.iter().any(|listed| listed == spelling))
}

/// One wake of a burst: the index of its rule and of its candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Choice {
    pub(crate) rule: usize,
    pub(crate) candidate: usize,
}

/// The wakes of one instance's burst, `candidates` being each candidate's kind and code in the
/// order they were committed: at most one per channel (module doc).
pub(crate) fn choose(
    rules: &[WakeRule],
    candidates: &[(&str, &str)],
    alias: Option<&str>,
) -> Vec<Choice> {
    let mut chosen: Vec<Choice> = Vec::new();
    for (rule_index, rule) in rules.iter().enumerate() {
        // The strongest candidate the rule matches; the earlier one on a tie.
        let best = candidates
            .iter()
            .enumerate()
            .filter(|(_, (kind, spelling))| matches(rule, kind, spelling, alias))
            .max_by(|(left_index, (left, _)), (right_index, (right, _))| {
                strength(left)
                    .cmp(&strength(right))
                    .then(right_index.cmp(left_index))
            })
            .map(|(index, _)| index);
        let Some(candidate) = best else {
            continue;
        };
        let choice = Choice {
            rule: rule_index,
            candidate,
        };
        let same_channel = chosen
            .iter_mut()
            .find(|earlier| rules[earlier.rule].to == rule.to);
        match same_channel {
            None => chosen.push(choice),
            Some(earlier)
                if strength(candidates[candidate].0)
                    > strength(candidates[earlier.candidate].0) =>
            {
                *earlier = choice;
            }
            Some(_) => {}
        }
    }
    chosen
}
