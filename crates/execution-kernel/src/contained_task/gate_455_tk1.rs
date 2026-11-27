// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #455 TK-1 (0.12.89; register §1.532; contracts/package-variables.md, "A variable a rule
// gives"), the kernel: a variable only a rule gives (`given_by: "rule"`) has no default, so a run
// reads it only when it is handed a value, and a run that holds none for it is refused before any
// input with `contained_task_variable_value_missing` (the package, the variable, and the detail
// `variable_name=<name>`).
// - The values a run without a dispatch reads leave it out; a dispatch that carries no value, or
//   one its declaration no longer takes, holds none, and each is refused; a dispatched or given
//   value it takes is read and not refused; a package without such a variable is never refused
//   (k1).
// - The offline simulation and the whole offline replay without the value are refused before
//   their first capture; given the value, neither is refused with this code (k2).
//
// The package is SEL-L6b's scan task (`gate_308_l6b`) with made-up variables. No clock is read.

use super::gate_308_l6b::scan_task;
use super::*;
use actingcommand_contract::{FactValue, parse_package_variables};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use serde_json::{Value, json};

/// The kernel's code of a run without a value of a variable only a rule gives.
const MISSING: &str = "contained_task_variable_value_missing";

/// SEL-L6b's scan task declaring `pick`, a choice of item_a and item_b only a rule gives, and
/// `extra`, a switch with the default false.
fn rule_given() -> PreparedContainedTask {
    let mut task = scan_task();
    task.control.variables = parse_package_variables(
        &json!([
            {"name": "pick", "type": "choice", "title": {"en": "Item"},
             "options": [{"value": "item_a", "title": {"en": "Item A"}},
                         {"value": "item_b", "title": {"en": "Item B"}}],
             "given_by": "rule"},
            {"name": "extra", "type": "switch", "title": {"en": "Extra"}, "default": false}
        ]),
        &[],
    )
    .expect("the variables");
    task
}

/// `values` as a dispatch's reason, or `--variable`, hands them.
fn given(values: &[(&str, Value)]) -> Vec<(String, Value)> {
    values
        .iter()
        .map(|(name, value)| ((*name).to_owned(), value.clone()))
        .collect()
}

/// One frame of one pixel, which no refused run captures.
fn frame() -> Frame {
    Frame::from_pixels(
        1,
        1,
        vec![0, 0, 0],
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame")
}

/// Ruling 5: what a run without a dispatch reads holds `extra` alone, and its run is refused with
/// the code, the package and `pick`; so is a policy run whose dispatch carries no `pick`, or one
/// the declaration does not take. A dispatched `item_a`, and the same value given offline, is read
/// with `extra`'s default and refuses nothing. A package without such a variable is not refused.
#[test]
fn tk1_k1_a_run_without_the_value_is_refused_and_one_with_it_reads_it() {
    let task = rule_given();
    let defaults = task.default_run_values();
    assert_eq!(
        task.default_variable_values(),
        vec![("extra".to_owned(), FactValue::Boolean(false))]
    );
    let refusal = task
        .refuse_missing_variable(&defaults)
        .expect_err("a run with the defaults alone is refused");
    assert_eq!(refusal.code().as_str(), MISSING);
    assert_eq!(refusal.detail(), Some("variable_name=pick"));
    let outcome = refusal.outcome();
    assert_eq!(
        outcome.value("package_id"),
        Some(&json!(task.package_label()))
    );
    assert_eq!(outcome.value("variable_name"), Some(&json!("pick")));

    for (case, carried) in [
        ("no value", given(&[])),
        ("a value not taken", given(&[("pick", json!("item_c"))])),
    ] {
        let parameters = task
            .dispatched_run_parameters(Vec::new(), 1)
            .with_variables(task.dispatched_variable_values(&carried));
        assert_eq!(
            task.refuse_missing_variable(&parameters)
                .map_err(|error| error.code().as_str()),
            Err(MISSING),
            "{case}"
        );
    }

    let carried = given(&[("pick", json!("item_a"))]);
    let read = vec![
        ("pick".to_owned(), FactValue::String("item_a".to_owned())),
        ("extra".to_owned(), FactValue::Boolean(false)),
    ];
    assert_eq!(task.dispatched_variable_values(&carried), read);
    let dispatched = task
        .dispatched_run_parameters(Vec::new(), 1)
        .with_variables(task.dispatched_variable_values(&carried));
    assert!(task.refuse_missing_variable(&dispatched).is_ok());
    let offline = task
        .given_variable_values(&carried)
        .expect("the value is taken as given");
    assert_eq!(offline, read);
    assert!(
        task.refuse_missing_variable(&task.default_run_parameters().with_variables(offline))
            .is_ok()
    );

    let plain = scan_task();
    assert!(
        plain
            .refuse_missing_variable(&plain.default_run_values())
            .is_ok()
    );
}

/// Ruling 6a: the offline simulation and the whole offline replay of the package, given no value
/// of `pick`, are refused with the code before their first capture (no frame captured, none read);
/// given `item_a`, the simulation is not refused with this code.
#[test]
fn tk1_k2_an_offline_run_without_the_value_is_refused_before_its_first_capture() {
    let task = rule_given();
    let simulated =
        crate::simulate_contained_task_at(&task, vec![frame()], 1).expect("a simulation");
    assert_eq!(simulated.capture_count, 0);
    let crate::OfflineDecision::Refused { code, detail } = &simulated.decision else {
        panic!("the simulation is refused");
    };
    assert_eq!(code.as_str(), MISSING);
    assert_eq!(detail.as_deref(), Some("variable_name=pick"));

    let replayed = crate::replay_contained_task(
        &task,
        vec![Ok::<Frame, std::convert::Infallible>(frame())].into_iter(),
        crate::OfflineReplayFacts::Empty { now_unix_ms: 1 },
    )
    .expect("a replay");
    assert_eq!(replayed.frames_read, 0);
    let crate::OfflineReplayEnd::Refused { outcome } = &replayed.end else {
        panic!("the replay is refused");
    };
    assert_eq!(outcome.code().as_str(), MISSING);
    assert_eq!(outcome.value("variable_name"), Some(&json!("pick")));

    let parameters = task.default_run_parameters().with_variables(
        task.given_variable_values(&given(&[("pick", json!("item_a"))]))
            .expect("the value is taken as given"),
    );
    let given_value = crate::simulate_contained_task_with_tables(
        &task,
        vec![frame()],
        Some(1),
        crate::OfflineDraw::default(),
        crate::RunTables::default(),
        parameters,
    )
    .expect("a simulation");
    if let crate::OfflineDecision::Refused { code, .. } = &given_value.decision {
        assert_ne!(code.as_str(), MISSING);
    }
}
