// SPDX-License-Identifier: AGPL-3.0-only

//! #422 LN-TUNE (brief rd-1026/brief-ln-tune.md item 3): the planning-signal recovery reads the
//! history after the State's planning checkpoint, and each page it commits advances that
//! checkpoint. A root's first start reads its history from checkpoint 0; the planning signals
//! the first host records advance the checkpoint too, and the second start on the same root
//! reads no record at or before it: none the first start read and none of those signals.
//!
//! The policy host records each planning page its recovery reads on the start's thread
//! (`PLANNING_RECOVERY_READS`); the host starts and closes on this thread. No clock and no sleep.

use super::*;
use crate::policy_host::PLANNING_RECOVERY_READS;

/// Planning signals the first host records while it runs.
const SIGNALS: usize = 5;

#[test]
fn ln_tune_a_second_start_reads_no_planning_history_before_the_checkpoint() {
    let root = TempDir::new().expect("tempdir");
    PLANNING_RECOVERY_READS.take();

    let first = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    let first_reads = PLANNING_RECOVERY_READS.take();
    for index in 0..SIGNALS as u64 {
        first
            .record_policy_planning_signal(PolicyPlanningSignalEventData {
                signal_id: format!("signal:ln-tune-{index}"),
                instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
                task_id: None,
                kind: PolicyPlanningSignalKind::GoalMissed,
                outcome: planning_signal_outcome(PolicyPlanningSignalKind::GoalMissed),
                observed_at_unix_ms: POLICY_NOW_UNIX_MS + index,
                detection_budget: None,
            })
            .expect("record planning signal");
    }
    let mut client = TestClient::connect(&first);
    let recorded = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::PolicyPlanningSignalObserved),
            ..EventQuery::default()
        },
    );
    drop(client);
    first.close().expect("close the first host");
    assert_eq!(recorded.len(), SIGNALS, "the recorded signals");
    let checkpoint = recorded
        .iter()
        .map(|event| event.sequence)
        .max()
        .expect("a recorded signal");

    // The first start reads the root's history from checkpoint 0, page after page.
    assert!(
        first_reads.first().is_none_or(|(after, _)| *after == 0),
        "the first start reads from checkpoint 0: {first_reads:?}"
    );
    assert!(
        first_reads.windows(2).all(|pair| pair[1].0 == pair[0].1),
        "each page begins where the one before ended: {first_reads:?}"
    );
    let first_through = first_reads.last().map_or(0, |(_, through)| *through);
    assert!(
        first_through < checkpoint,
        "the signals follow the first start's recovery"
    );

    let second = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    let second_reads = PLANNING_RECOVERY_READS.take();
    second.close().expect("close the second host");

    // The second start begins at the checkpoint the first host committed (its last signal), and
    // reads no record at or before it.
    assert!(
        second_reads
            .iter()
            .all(|(after, through)| *after >= checkpoint && through > after),
        "the second start reads only after the checkpoint {checkpoint}: {second_reads:?}"
    );
    assert!(
        second_reads.windows(2).all(|pair| pair[1].0 == pair[0].1),
        "each page begins where the one before ended: {second_reads:?}"
    );
}
