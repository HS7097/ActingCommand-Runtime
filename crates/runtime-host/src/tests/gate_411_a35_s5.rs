// SPDX-License-Identifier: AGPL-3.0-only

//! #411 A35-S5 (design `rd-1021/train-0129/a35-design.md` §5.1 S5 T1; coordinator decision Q7
//! of 10-21): a recovery ladder that holds instead of climbing is visible. While its hold waits,
//! `Status` (what `actingctl status` prints and `ac_overview` shows in the instance's row) carries
//! the hold: what it waits for (`kind`), how long it has held (`held_ms`) and its limit
//! (`limit_ms`), from the ladder's own state in memory.
//!
//! The fixture is A35-S1's busy ladder (`gate_411_b37`): a policy run on a manual Runtime clock
//! that only the test moves, whose return-home run ends on the mapping its package marks `busy`,
//! so the ladder holds until the test moves the clock past the hold. Only the S1 kind
//! (`busy_outcome`) is tested; `screen_changing` comes with A35-S2's hold.

use super::gate_411_b37::{busy_config, busy_ladder};
use super::instance_workers::{wait_for_ladder_end, wait_until};
use super::*;
use serde_json::{Value, json};

/// Non-default values of the busy hold, the configuration's (design §3): the hold's wait and
/// its limit.
const RECHECK_MS: u64 = 40_000;
const MAX_WAIT_MS: u64 = 100_000;
/// How far the test moves the Runtime clock inside the hold, short of its end.
const HELD_MS: u64 = 15_000;

/// The holding ladder `Status` shows for the fixture's instance, as its JSON member.
fn status_hold(host: &RuntimeHost) -> Option<Value> {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let receipt = host
        .process_request_for_test(
            &runtime_request(&ids, RuntimeOperation::Status),
            ConnectionId::new(191).expect("connection"),
        )
        .expect("status receipt");
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    let Some(RuntimeResult::Status { status }) = receipt.result() else {
        panic!("a status result: {receipt:?}");
    };
    status
        .instances()
        .iter()
        .find(|instance| instance.instance_alias() == POLICY_INSTANCE_ALIAS)
        .expect("the instance's status")
        .ladder_busy_hold()
        .map(|hold| serde_json::to_value(hold).expect("the status member"))
}

/// T1 (design §5.1 S5): the return-home run ended busy and the ladder holds. `Status` shows the
/// hold with its kind `busy_outcome`, the time held since the climb's first busy end (0, then
/// the time the Runtime clock moved inside the hold) and the configured `max_wait_ms` as its
/// limit. Once the activity ended and the rerun recovered, the ladder holds nothing and the
/// status shows no hold.
#[test]
fn a35_s5_t1_status_shows_the_holding_ladder_with_its_kind_time_held_and_limit() {
    let ladder = busy_ladder(&busy_config(RECHECK_MS, MAX_WAIT_MS, "stop"));
    let host = &ladder.host;
    wait_until("the ladder's hold in the status", || {
        status_hold(host).is_some()
    });
    assert_eq!(
        status_hold(host),
        Some(json!({"kind": "busy_outcome", "held_ms": 0, "limit_ms": MAX_WAIT_MS})),
        "the hold as it begins"
    );
    ladder.clock.advance(HELD_MS);
    assert_eq!(
        status_hold(host),
        Some(json!({"kind": "busy_outcome", "held_ms": HELD_MS, "limit_ms": MAX_WAIT_MS})),
        "the time held moves with the Runtime clock"
    );

    // The activity ended: the fake shows home, and the hold's end runs return-home again.
    ladder.state.unknown_capture.store(false, Ordering::Release);
    ladder.clock.advance(RECHECK_MS - HELD_MS);
    wait_for_ladder_end(host, POLICY_INSTANCE_ALIAS);
    assert_eq!(status_hold(host), None, "no hold once the ladder ended");
    assert!(host.fatal_error().expect("runtime health").is_none());
    ladder.host.close().expect("close host");
    drop(ladder.root);
}
