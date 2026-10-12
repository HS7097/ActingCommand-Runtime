// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #381 A R1′ (#672 review M3): a session deadline cuts each wait to the time that
//! remains, keeps a 1 ms floor and leaves a session without a deadline unchanged.

use crate::client::bounded_timeout;
use std::time::Duration;

#[test]
fn gate_a_session_deadline_cuts_each_wait_to_what_remains_with_a_1_ms_floor() {
    let seconds = Duration::from_secs;
    assert_eq!(bounded_timeout(seconds(60), Some(seconds(10))), seconds(10));
    assert_eq!(bounded_timeout(seconds(5), Some(seconds(10))), seconds(5));
    assert_eq!(
        bounded_timeout(seconds(60), Some(Duration::ZERO)),
        Duration::from_millis(1)
    );
    assert_eq!(bounded_timeout(seconds(5), None), seconds(5));
}
