// SPDX-License-Identifier: AGPL-3.0-only

//! #422 LN-7c Q, test T-plan (`rd-1026/ln7c/final-query.md` §1.3, §7 row Q): statements W, L and
//! K, on an empty database built as a new root's is, pass the plan rule (no scan of a ledger table,
//! the type index only with a sequence bound or as a one-entry seek, no history-wide index) and
//! their plans equal their golden texts (`plan_w.txt`, `plan_l.txt`, `plan_k.txt`). A plan that
//! changes needs a deliberate re-record, which needs the coordinator's approval; the failure
//! prints the plan as it is now. The keyed statement of a moved frame's owner point passes the
//! rule too.

use super::super::plan_check::check_plan;
use super::{KEPT, LIVE, OWNER, WAITS};

#[test]
fn ln7c_q_t_plan_w_l_and_k_pass_the_rule_and_equal_their_golden_plans() {
    for (name, sql, golden) in [
        ("W", WAITS, include_str!("plan_w.txt")),
        ("L", LIVE, include_str!("plan_l.txt")),
        ("K", KEPT, include_str!("plan_k.txt")),
    ] {
        let check = check_plan(sql);
        let now = check.plan.join("\n");
        assert!(
            check.violations.is_empty(),
            "statement {name} breaks the plan rule: {:?}\nits plan:\n{now}",
            check.violations
        );
        assert_eq!(
            check.plan,
            golden.lines().collect::<Vec<_>>(),
            "statement {name}'s plan is not its golden text; its plan now:\n{now}"
        );
    }
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
