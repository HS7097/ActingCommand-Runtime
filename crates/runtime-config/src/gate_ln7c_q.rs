// SPDX-License-Identifier: AGPL-3.0-only

// #422 LN-7c Q, T-evidence (the configuration's half; rd-1026/ln7c/final-query.md §5, brief
// rd-1026/brief-ln7c-q.md: three keys in `actingd-config` revision 16, no 14-day key): the
// evidence-frame rule's age `frame_retention_evidence_max_age_days` is a top-level key with the
// default 15 from the defaults file, an explicit file value is taken, and an instruction may change
// it within `1..=3650`; a value outside its range, or of another type, is refused
// (`config_value_invalid`) and the value in effect stays. The frame queries' pace
// `ledger_observer.frame_query_steps_per_second` (500 000, `1000..=100000000`) is settable too, and
// their chunk `ledger_observer.frame_query_chunk_frames` (256, `1..=4096`) is not. The frame
// cleaner takes the age at each pass start (the host's cleaner tests).

use super::*;
use crate::frame_retention::EVIDENCE_MAX_AGE_DAYS;
use crate::ledger_observer::{FRAME_QUERY_CHUNK_FRAMES, FRAME_QUERY_STEPS_PER_SECOND};
use actingcommand_contract::{ConfigSettingChange, RuntimeConfigManifest};
use serde_json::{Value, json};

const DEVICE: DeviceBounds = DeviceBounds {
    application_stop_ms: 60_000,
    emulator_stop_ms: 200_000,
    emulator_start_ms: 190_000,
};

fn context() -> CheckContext {
    CheckContext::new(5_000, DEVICE)
}

fn stamp() -> InstructionStamp {
    InstructionStamp {
        set_unix_ms: 1,
        set_in_owner_epoch: "epoch".to_owned(),
        request_id: "request".to_owned(),
        channel: None,
    }
}

fn set(key: &str, value: Value) -> ConfigSettingChange {
    ConfigSettingChange {
        key: key.to_owned(),
        value: Some(value),
    }
}

// The configuration a start renders from `file`, over the shipped defaults.
fn effective(file: Value) -> EffectiveConfig {
    let (_, defaults) = DefaultsFile::shipped().expect("the shipped defaults file parses");
    let mut parameters = render_parameters(&mechanism_rows(), &[], &defaults, &file, &|_| None)
        .expect("every key resolves");
    annotate_settable(&mut parameters).expect("every key is in the key table");
    EffectiveConfig::from_manifest(&RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    })
    .expect("the configuration in effect")
}

#[test]
fn ln7c_q_t_evidence_the_evidence_age_is_settable_within_its_range_and_refused_outside_it() {
    let running = effective(json!({}));
    assert_eq!(running.frame_retention_evidence_max_age_days(), Ok(15));
    let section = running.ledger_observer().expect("the section in effect");
    assert_eq!(
        (
            section.frame_query_steps_per_second,
            section.frame_query_chunk_frames
        ),
        (500_000, 256)
    );
    // An explicit file value is the one in effect.
    assert_eq!(
        effective(json!({"frame_retention_evidence_max_age_days": 30}))
            .frame_retention_evidence_max_age_days(),
        Ok(30)
    );

    // Outside its range or of another type: refused, and the value in effect stays.
    for (value, problem) in [
        (json!(0), ConfigValueProblem::OutOfRange),
        (json!(3_651), ConfigValueProblem::OutOfRange),
        (json!("15"), ConfigValueProblem::WrongType),
    ] {
        let refused = running
            .apply(
                &[set(EVIDENCE_MAX_AGE_DAYS, value.clone())],
                &stamp(),
                &context(),
            )
            .expect_err("a value outside the evidence age's range");
        assert_eq!(
            refused.failure_code(),
            ConfigCode::ConfigValueInvalid,
            "{value}"
        );
        assert_eq!(refused.config_key(), EVIDENCE_MAX_AGE_DAYS, "{value}");
        assert_eq!(refused.problem(), Some(problem), "{value}");
    }
    assert_eq!(running.frame_retention_evidence_max_age_days(), Ok(15));

    // Within its range, at both ends: in effect at once (the cleaner's next pass takes it).
    for days in [1, 10, 3_650] {
        let applied = running
            .apply(
                &[set(EVIDENCE_MAX_AGE_DAYS, json!(days))],
                &stamp(),
                &context(),
            )
            .expect("an evidence age within its range");
        assert_eq!(
            applied.config.frame_retention_evidence_max_age_days(),
            Ok(days)
        );
    }

    // The pace may change while the Runtime runs; the chunk may not.
    let applied = running
        .apply(
            &[set(FRAME_QUERY_STEPS_PER_SECOND, json!(1_000))],
            &stamp(),
            &context(),
        )
        .expect("the frame queries' pace is settable");
    assert_eq!(
        applied
            .config
            .ledger_observer()
            .expect("the section in effect")
            .frame_query_steps_per_second,
        1_000
    );
    let refused = running
        .apply(
            &[set(FRAME_QUERY_STEPS_PER_SECOND, json!(999))],
            &stamp(),
            &context(),
        )
        .expect_err("a pace below its range");
    assert_eq!(refused.problem(), Some(ConfigValueProblem::OutOfRange));
    let refused = running
        .apply(
            &[set(FRAME_QUERY_CHUNK_FRAMES, json!(64))],
            &stamp(),
            &context(),
        )
        .expect_err("the chunk is read at start");
    assert_eq!(refused.failure_code(), ConfigCode::ConfigKeyNotSettable);
}
