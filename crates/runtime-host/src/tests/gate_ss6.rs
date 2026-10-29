// SPDX-License-Identifier: AGPL-3.0-only

// SS-6 work order test 6, host half (0.12.31; register §1.218; coordinator work order
// `rd-1029/brief-ss6.md`; `contracts/remedies.md` section 4): a policy run reads the spend values
// its dispatch's `policy_dispatch_parameters` carries as the schedule values it overlays on its
// instance fact snapshots: `run_cap` as `session.dispatch.schedule.run_cap`, each spend allowance
// as `session.dispatch.schedule.spend_allowance.<i>` and each remaining gap as
// `session.dispatch.schedule.gap_remaining.<j>`, `<i>` and `<j>` the entry's position in the
// task's own `consumes` or `produces`, all integers, after Workflow #411 B8's values. The run cap
// the package's fact cap reads is the number the dispatch carries (the kernel half,
// `gate_390_out2::ss6_t6_a_fact_cap_on_the_run_cap_reads_the_dispatch_run_cap`, reads it from
// the overlay).

use crate::fact_refresh::dispatch_schedule_values;
use actingcommand_contract::FactValue;
use actingcommand_contract::outcome::{Outcome, Record};
use actingcommand_policy::PolicyCode;

#[test]
fn ss6_t6_a_policy_run_reads_the_dispatch_run_cap_allowances_and_gaps() {
    let outcome = Outcome::new(PolicyCode::PolicyDispatchParameters)
        .daily_remaining(2_u32)
        .spend_allowances([
            Record::new().effect_index(1_usize).spend_allowance(900_i64),
            Record::new().effect_index(3_usize).spend_allowance(0_i64),
        ])
        .gaps_remaining([Record::new().effect_index(0_usize).gap_remaining(55_i64)])
        .run_cap(6_i64);
    assert_eq!(
        dispatch_schedule_values(&outcome),
        vec![
            ("daily_remaining".to_owned(), FactValue::Integer(2)),
            ("run_cap".to_owned(), FactValue::Integer(6)),
            ("spend_allowance.1".to_owned(), FactValue::Integer(900)),
            ("spend_allowance.3".to_owned(), FactValue::Integer(0)),
            ("gap_remaining.0".to_owned(), FactValue::Integer(55)),
        ]
    );

    // A reason of Workflow #411 B8's values alone reads as before: no SS-6 key.
    let b8 = Outcome::new(PolicyCode::PolicyDispatchParameters).daily_remaining(2_u32);
    assert_eq!(
        dispatch_schedule_values(&b8),
        vec![("daily_remaining".to_owned(), FactValue::Integer(2))]
    );
}
