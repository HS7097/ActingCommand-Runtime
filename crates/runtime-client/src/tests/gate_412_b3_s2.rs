// SPDX-License-Identifier: AGPL-3.0-only

//! #412 Lab B-3 S2 (frozen design rd-1021/train-0129/lab-b3-design.md §2.4 and §5 "S2", T2-5;
//! register §1.162; apps key 90): the receipt wait of a Lab operation that runs a declared
//! operation is the package's task timeout (from the client's kernel admission), four
//! backend-open allowances, the longest input, the I/O allowance and one projection transaction
//! (#411 Lab L5: one more backend-open allowance and one more interval for every frame before the
//! current one).

use crate::client::lab_declared_operation_response_timeout;
use actingcommand_contract::{MAX_INPUT_DURATION_MS, ObservationFramePlan};
use std::time::Duration;

#[test]
fn b3_s2_t2_5_a_declared_operation_waits_the_design_bound_for_its_receipt() {
    let backend_open = Duration::from_secs(10);
    let io_timeout = Duration::from_millis(500);
    for (task_timeout_ms, frames, transaction) in [
        // One frame: the projection transaction takes no more than the frame itself.
        (60_000, ObservationFramePlan::SINGLE, Duration::ZERO),
        // Three frames 200 ms apart: two more backend opens and the two waits between them.
        (
            5_000,
            ObservationFramePlan::new(3, 200).expect("three frames"),
            backend_open * 2 + Duration::from_millis(400),
        ),
    ] {
        let task_timeout = Duration::from_millis(task_timeout_ms);
        assert_eq!(
            lab_declared_operation_response_timeout(backend_open, io_timeout, task_timeout, frames),
            Some(
                task_timeout
                    + backend_open * 4
                    + Duration::from_millis(MAX_INPUT_DURATION_MS)
                    + io_timeout
                    + transaction
            ),
            "a task timeout of {task_timeout_ms} ms"
        );
    }
}
