// SPDX-License-Identifier: AGPL-3.0-only

//! #411 B37 (slice A35-S1; design `rd-1021/train-0129/a35-design.md` §3, §5.1 S1 T5;
//! coordinator decision Q3 of 10-21): the busy hold's keys `recovery_ladder.busy.poll_ms` and
//! `recovery_ladder.busy.outcome.{recheck_ms, max_wait_ms, on_exhausted}` take their defaults
//! (500, 60 000, 3 600 000, `stop`) from the shipped defaults file, never from code; a value
//! outside its range is refused `bound_out_of_range`, and `recheck_ms` plus `hold.margin_ms`
//! beyond the longest renewal `renewal_cap_exceeded`, at start (`recovery_ladder_config_invalid`)
//! and through an instruction (`config_value_invalid`, the configuration in effect unchanged).

use super::*;
use crate::recovery_ladder::{
    BUSY_OUTCOME_MAX_WAIT_MS, BUSY_OUTCOME_ON_EXHAUSTED, BUSY_OUTCOME_RECHECK_MS, BUSY_POLL_MS,
};
use actingcommand_contract::{ConfigSettingChange, RuntimeConfigManifest};
use serde_json::{Value, json};

/// The scheduler's heartbeat bound and the device layer's bounds the check reads: inputs of the
/// check, as the D1c rows choose them.
const HEARTBEAT_MS: u64 = 5_000;
const DEVICE: DeviceBounds = DeviceBounds {
    application_stop_ms: 60_000,
    emulator_stop_ms: 200_000,
    emulator_start_ms: 190_000,
};

fn context() -> CheckContext {
    CheckContext::new(HEARTBEAT_MS, DEVICE)
}

fn shipped() -> Value {
    DefaultsFile::shipped()
        .expect("the shipped defaults file parses")
        .1
}

/// The configuration a start reads from a file whose `recovery_ladder` section is `section`.
fn from_file(section: Value) -> EffectiveConfig {
    let parameters = render_parameters(
        &mechanism_rows(),
        &[],
        &shipped(),
        &json!({ "recovery_ladder": section }),
        &|_| None,
    )
    .expect("every key resolves");
    EffectiveConfig::from_manifest(&RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    })
    .expect("every key is in the key table")
}

fn stamp() -> InstructionStamp {
    InstructionStamp {
        set_unix_ms: 1,
        set_in_owner_epoch: "epoch".to_owned(),
        request_id: "request".to_owned(),
        channel: None,
    }
}

/// T5: the four keys default from the shipped file, and the section in effect holds those
/// values; each broken bound is refused with its key and why at start and through an
/// instruction, which leaves the configuration in effect as it was.
#[test]
fn b37_t5_the_busy_keys_default_from_the_file_and_their_bounds_are_checked() {
    let defaults = shipped();
    assert_eq!(
        [
            BUSY_POLL_MS,
            BUSY_OUTCOME_RECHECK_MS,
            BUSY_OUTCOME_MAX_WAIT_MS,
            BUSY_OUTCOME_ON_EXHAUSTED,
        ]
        .map(|key| member(&defaults, key).cloned()),
        [
            Some(json!(500)),
            Some(json!(60_000)),
            Some(json!(3_600_000)),
            Some(json!("stop")),
        ],
        "the defaults file holds the coordinator's defaults (Q3)"
    );
    let running = from_file(json!({}));
    let ladder = running
        .recovery_ladder(&context())
        .expect("the defaults pass the check");
    assert_eq!(
        ladder.busy,
        BusyConfig {
            poll_ms: 500,
            outcome: BusyOutcomeConfig {
                recheck_ms: 60_000,
                max_wait_ms: 3_600_000,
                on_exhausted: OnExhausted::Stop,
            },
        }
    );

    let margin_ms = member(&defaults, "recovery_ladder.hold.margin_ms")
        .and_then(Value::as_u64)
        .expect("the shipped margin");
    let renewal_cap_ms =
        actingcommand_contract::ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS + HEARTBEAT_MS;
    let rows = [
        (
            "a poll below 100 ms",
            BUSY_POLL_MS,
            json!(99),
            ConfigValueProblem::BoundOutOfRange,
        ),
        (
            "a hold longer than a day",
            BUSY_OUTCOME_MAX_WAIT_MS,
            json!(86_400_001),
            ConfigValueProblem::BoundOutOfRange,
        ),
        (
            "a recheck whose renewal passes the longest renewal",
            BUSY_OUTCOME_RECHECK_MS,
            json!(renewal_cap_ms - margin_ms + 1),
            ConfigValueProblem::RenewalCapExceeded,
        ),
    ];
    for (case, key, value, problem) in rows {
        let member_name = key
            .strip_prefix("recovery_ladder.busy.")
            .expect("a busy key");
        let section = match member_name.split_once('.') {
            Some((group, name)) => json!({"busy": {group: {name: value.clone()}}}),
            None => json!({"busy": {member_name: value.clone()}}),
        };
        // At start (and in check-config): the section check refuses the file.
        let refused = from_file(section).check(&context()).expect_err(case);
        assert_eq!(
            (
                refused.failure_code(),
                refused.config_key(),
                refused.problem()
            ),
            (ConfigCode::RecoveryLadderConfigInvalid, key, Some(problem)),
            "{case}"
        );
        // Through an instruction: the same key and why as `config_value_invalid`; nothing
        // changes.
        let refused = running
            .apply(
                &[ConfigSettingChange {
                    key: key.to_owned(),
                    value: Some(value),
                }],
                &stamp(),
                &context(),
            )
            .expect_err(case);
        assert_eq!(
            (
                refused.failure_code(),
                refused.config_key(),
                refused.problem()
            ),
            (ConfigCode::ConfigValueInvalid, key, Some(problem)),
            "{case}"
        );
        assert_eq!(
            running
                .recovery_ladder(&context())
                .map(|ladder| ladder.busy),
            Ok(ladder.busy),
            "{case}"
        );
    }
}
