// SPDX-License-Identifier: AGPL-3.0-only

//! #422 LN-7c Q, test T-plan (`rd-1026/ln7c/final-query.md` §1.3, §7 row Q): statements W, L and
//! K, on an empty database built as a new root's is, pass the plan rule (no scan of a ledger table,
//! the type index only with a sequence bound or as a one-entry seek, no history-wide index) and
//! their plans equal their golden texts (`plan_w.txt`, `plan_l.txt`, `plan_k.txt`). A plan that
//! changes needs a deliberate re-record, which needs the coordinator's approval. The rule is held
//! for all three statements before any text is compared; then every statement whose plan is not
//! its golden text prints its whole plan as it is now, between two marker lines, so a re-record
//! can copy it from the test's output (fix round 1). The keyed statement of a moved frame's owner
//! point passes the rule too.

use super::super::plan_check::{PlanCheck, check_plan};
use super::{KEPT, LIVE, OWNER, WAITS};

#[test]
fn ln7c_q_t_plan_w_l_and_k_pass_the_rule_and_equal_their_golden_plans() {
    let statements = [
        ("W", "plan_w.txt", WAITS, include_str!("plan_w.txt")),
        ("L", "plan_l.txt", LIVE, include_str!("plan_l.txt")),
        ("K", "plan_k.txt", KEPT, include_str!("plan_k.txt")),
    ];
    let checks = statements
        .iter()
        .map(|(name, golden_file, sql, golden)| (*name, *golden_file, check_plan(sql), *golden))
        .collect::<Vec<(&str, &str, PlanCheck, &str)>>();
    // The rule first, for every statement.
    let broken = checks
        .iter()
        .filter(|(_, _, check, _)| !check.violations.is_empty())
        .map(|(name, _, check, _)| {
            format!(
                "statement {name} breaks the plan rule: {:?}\nits plan:\n{}",
                check.violations,
                check.plan.join("\n")
            )
        })
        .collect::<Vec<_>>();
    assert!(broken.is_empty(), "{}", broken.join("\n\n"));
    // Then the texts: every statement whose plan differs prints its whole plan.
    let differing = checks
        .iter()
        .filter(|(_, _, check, golden)| check.plan != golden.lines().collect::<Vec<_>>())
        .map(|(name, golden_file, check, _)| {
            format!(
                "statement {name}'s plan is not its golden text {golden_file}; its plan now:\n\
                 ----- begin {golden_file} -----\n{}\n----- end {golden_file} -----",
                check.plan.join("\n")
            )
        })
        .collect::<Vec<_>>();
    assert!(differing.is_empty(), "{}", differing.join("\n\n"));
}

#[test]
fn ln7c_q_t_plan_the_owner_point_statement_passes_the_rule() {
    let check = check_plan(OWNER);
    assert!(
        check.violations.is_empty(),
        "{:?}\n{}",
        check.violations,
        check.plan.join("\n")
    );
}
