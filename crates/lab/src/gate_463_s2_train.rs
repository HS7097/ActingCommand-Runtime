// SPDX-License-Identifier: AGPL-3.0-only

// #463 stage two, train (S2-2 #1786, S2-7 #1784, S2-6 #1787 and S2-1c #1788 in one pull request;
// register §1.439, §1.440, §1.442 and §1.443, each its paragraph "Train"; contracts/lab-recording.md,
// "Reviewing a recording step by step: `record review`" and "What a recording still lacks: `record
// ready`"): where the slices meet in the one readiness judgement. `record ready` and `record review`
// answer from one function (`recording_api::ready_of`, S2-2's, holding S2-1c's judgement and S2-6's
// pre-check); a step's `missing` in `record review` is every item of `record ready` that names that
// step, S2-6's `select_unrefined` and S2-1c's self-check failures alike, and an item that names no
// step is no step's.
//
// - `s2_train_each_step_reviews_exactly_its_items_of_the_one_readiness_judgement`: S2-1c's bench
//   (`gate_463_s21c::Bench::marked`: four steps recorded as stage one records them, three taps and
//   the final screen, each step with a colour mark on its square) with step 2 marked as holding a
//   choice and not refined (S2-6), step 1's mark expecting another colour (two self-check failures
//   on step 1, the first decision's and the cross-check's, as S2-1c's second test finds them) and
//   the recording's own locale taken out of the step table. A pre-check that fails runs no
//   self-check (S2-1's rule, which S2-1c keeps), so one reading cannot list both kinds of item: the
//   recording is read twice. Without a locale, `record ready` lists the locale (an item of no step)
//   and step 2's `select_unrefined`, and runs no self-check; `record review` of each step lists
//   exactly that step's items (step 2 its `select_unrefined`, every other step none, no step the
//   locale), and step 2 shows its choice mark as the step table holds it. Then, the mark taken off
//   (`choice: false`, S2-6) and the locale given (`--locale`, as S2-2's fix round reads it),
//   `record ready` lists step 1's two self-check failures and `record review` of step 1 lists
//   exactly them, every other step none.
// No Runtime, no device; the test reads no clock and never sleeps.

use crate::gate_463_s21c::{Bench, EVERY_CHECK, INSTANCE, complete};
use crate::{RecordReviewRequest, RecordStopOptions, record_ready, record_review};
use serde_json::{Value, json};

// `record ready` of the bench's recording given `options`, as the command reads it: the
// instance's recording; as JSON.
fn ready(bench: &Bench, options: &RecordStopOptions) -> Value {
    let outcome =
        record_ready(&bench.state(), INSTANCE, None, options, &bench.ocr).unwrap_or_else(|error| {
            panic!(
                "record ready: {}: {} {:?}",
                error.code, error.message, error.details
            )
        });
    serde_json::to_value(outcome).expect("the answer as JSON")
}

// `record review --step <step> --no-preview` of the bench's recording given `options`: the answer
// of that one step, as JSON.
fn reviewed(bench: &Bench, step: u32, options: &RecordStopOptions) -> Value {
    let outcome = record_review(
        &bench.state(),
        INSTANCE,
        &RecordReviewRequest {
            step: Some(step),
            no_preview: true,
            ..RecordReviewRequest::default()
        },
        options,
        &bench.ocr,
    )
    .unwrap_or_else(|error| {
        panic!(
            "record review of step {step}: {}: {} {:?}",
            error.code, error.message, error.details
        )
    });
    let answer = serde_json::to_value(outcome).expect("the answer as JSON");
    assert_eq!(answer["steps_total"], 4, "{answer}");
    let steps = answer["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 1, "{answer}");
    assert_eq!(steps[0]["step"], step, "{answer}");
    steps[0].clone()
}

// The items of `record ready`'s answer `ready` that name the step `step`, in its order.
fn items_of(ready: &Value, step: u32) -> Value {
    Value::Array(
        ready["missing"]
            .as_array()
            .expect("missing")
            .iter()
            .filter(|item| item["step"] == step)
            .cloned()
            .collect(),
    )
}

// The work order's test of the train: a marked and unrefined choice on one step, two self-check
// failures on another; each step's `record review` lists exactly that step's items of `record
// ready`, and `record ready` lists them all, in the two readings the pre-checks allow.
#[test]
fn s2_train_each_step_reviews_exactly_its_items_of_the_one_readiness_judgement() {
    let bench = Bench::marked();
    // S2-6: step 2 marked as holding a choice, not refined.
    bench.mark_request(json!({"step": 2, "choice": true}));
    // S2-1c: step 1's page no longer passes on its frame, so the first decision gives no click
    // there and the cross-check finds the page does not match its own frame.
    bench.break_mark(1, "expected", json!([255, 255, 255]));
    // The recording's own locale taken out: the locale is an item of the whole recording.
    let mut table = bench.recording();
    table["locale"] = Value::Null;
    bench.write_recording(&table);
    let mark = table["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .find(|entry| entry["index"] == 2)
        .expect("step 2")["choice"]
        .clone();
    assert!(mark["marked_at_unix_ms"].is_u64(), "{table}");

    // The first reading: a pre-check fails, so no self-check runs.
    let without = RecordStopOptions::default();
    let first = ready(&bench, &without);
    assert_eq!(first["status"], "draft", "{first}");
    let unrefined = json!({"order": 2, "step": 2, "artifact_step": 2, "frame_id": "f0003",
                           "what": "select_unrefined", "code": "record_select_invalid"});
    let missing = first["missing"].as_array().expect("missing");
    assert_eq!(missing.len(), 2, "{first}");
    assert_eq!(missing[0]["what"], "locale", "{first}");
    assert_eq!(missing[0]["code"], "record_locale_missing", "{first}");
    assert!(missing[0]["step"].is_null(), "{first}");
    assert_eq!(missing[1], unrefined, "{first}");
    assert_eq!(first["missing_total"], 2, "{first}");
    assert_eq!(first["checks"], json!([]), "{first}");
    assert_eq!(first["checks_after"], json!(EVERY_CHECK), "{first}");
    let mut listed = Vec::new();
    for step in 1..=4_u32 {
        let answer = reviewed(&bench, step, &without);
        assert_eq!(answer["missing"], items_of(&first, step), "step {step}");
        listed.extend(
            answer["missing"]
                .as_array()
                .expect("missing")
                .iter()
                .cloned(),
        );
        // S2-6 on S2-2: the choice mark as the step table holds it, on step 2 alone.
        if step == 2 {
            assert_eq!(answer["choice"], mark, "{answer}");
        } else {
            assert!(answer.get("choice").is_none(), "{answer}");
        }
    }
    // Step 2 lists its `select_unrefined`; the locale, which names no step, is no step's.
    assert_eq!(listed, [unrefined], "{first}");

    // The second reading: the mark taken off and the locale given, so the self-checks run.
    bench.mark_request(json!({"step": 2, "choice": false}));
    let with = RecordStopOptions {
        locale: Some("en-US".to_owned()),
        ..RecordStopOptions::default()
    };
    let second = ready(&bench, &with);
    assert_eq!(second["status"], "draft", "{second}");
    let failures = json!([
        {"order": 1, "step": 1, "artifact_step": 1, "frame_id": "f0001",
         "what": "first_decision", "code": "record_artifact_admission_failed",
         "details": {"frames": ["f0001"]}},
        {"order": 2, "step": 1, "artifact_step": 1, "frame_id": "f0001",
         "what": "cross_check", "code": "record_step_self_mismatch",
         "details": {"page": "step_01", "frames": ["f0001"]}}
    ]);
    assert_eq!(second["missing"], failures, "{second}");
    assert_eq!(second["missing_total"], 2, "{second}");
    assert_eq!(second["checks"], complete(&EVERY_CHECK[..6]), "{second}");
    assert_eq!(second["checks_after"], json!(["final_package"]), "{second}");
    for step in 1..=4_u32 {
        let answer = reviewed(&bench, step, &with);
        assert_eq!(answer["missing"], items_of(&second, step), "step {step}");
        assert!(answer.get("choice").is_none(), "{answer}");
    }
    assert_eq!(reviewed(&bench, 1, &with)["missing"], failures);
}
