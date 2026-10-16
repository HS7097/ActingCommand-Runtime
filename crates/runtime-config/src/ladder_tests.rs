// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #387 D1c, the configuration rows of the ladder tests (frozen model M387 v1.2 §8.2):
//! D1-L4 (the section's whole-section check, at start and through an instruction), D1-L9 (what
//! the default trigger items may and may not match), E-3 G3 (codes of the "screen disagrees
//! with the pack" paths reach `pack_logic`; the sampling codes reach nothing) and the
//! configuration side of H-2 RB-H3 (rung 3's bounds come from the configuration). Every
//! expected value comes from the model: §3 (matching, defaults, what never triggers), §4.2 and
//! §4.3 (bounds), §5.3 (checks), §7.1 (defaults).

use super::*;
use crate::recovery_ladder::{CLAIM_TTL_MS, DEFAULT_START_RUNG, RUNGS, TRIGGERS};
use actingcommand_contract::outcome::LadderClass;
use actingcommand_contract::{
    ConfigSettingChange, RecoveryRung, RecoveryTriggerStage, RuntimeConfigManifest,
};
use serde_json::{Value, json};

/// The scheduler's heartbeat bound and the device layer's bounds the checks read: inputs of
/// the check, chosen for the rows below.
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

/// The configuration a start reads from a file that names `section` as its `recovery_ladder`.
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

fn default_ladder() -> LadderConfig {
    from_file(json!({}))
        .recovery_ladder(&context())
        .expect("the defaults pass the check")
}

/// One D1-L4 row: the section members that break the rule, the key the refusal names and why.
struct Row {
    case: &'static str,
    section: Value,
    config_key: &'static str,
    problem: ConfigValueProblem,
}

fn d1_l4_rows() -> Vec<Row> {
    let two_rungs = json!([{"action": "return_home"}, {"action": "application_restart"}]);
    vec![
        Row {
            case: "two trigger items with one id",
            section: json!({"triggers": [
                {"id": "pack_logic", "stages": ["task"], "classes": ["pack_logic_mismatch"]},
                {"id": "pack_logic", "stages": ["task"], "classes": ["environment_fault"]},
            ]}),
            config_key: TRIGGERS,
            problem: ConfigValueProblem::DuplicateId,
        },
        Row {
            case: "the recovery preparation stage",
            section: json!({"triggers": [
                {"id": "own", "stages": ["recovery_preparation"], "classes": ["environment_fault"]},
            ]}),
            config_key: TRIGGERS,
            problem: ConfigValueProblem::StageNotAllowed,
        },
        Row {
            case: "a code the catalog does not list",
            section: json!({"triggers": [
                {"id": "unknown", "stages": ["task"], "codes": ["neutral_unlisted_failure"]},
            ]}),
            config_key: TRIGGERS,
            problem: ConfigValueProblem::CodeUnknown,
        },
        Row {
            case: "a Stop bound below the device layer's",
            section: json!({"rungs": [{
                "action": "emulator_restart",
                "stop_bound_ms": DEVICE.emulator_stop_ms - 1,
            }]}),
            config_key: RUNGS,
            problem: ConfigValueProblem::BoundOutOfRange,
        },
        Row {
            case: "a start rung beyond the rungs",
            section: json!({"triggers": [
                {"id": "late", "stages": ["task"], "classes": ["pack_logic_mismatch"],
                 "start_rung": 4},
            ]}),
            config_key: TRIGGERS,
            problem: ConfigValueProblem::StartRungOutOfRange,
        },
        Row {
            case: "a default start rung beyond the rungs",
            section: json!({"rungs": two_rungs, "trigger_defaults": {"start_rung": 3}}),
            config_key: DEFAULT_START_RUNG,
            problem: ConfigValueProblem::StartRungOutOfRange,
        },
        Row {
            case: "a claim TTL at the heartbeat bound",
            section: json!({"hold": {"claim_ttl_ms": HEARTBEAT_MS}}),
            config_key: CLAIM_TTL_MS,
            problem: ConfigValueProblem::ClaimTtlOutOfRange,
        },
        Row {
            case: "a claim TTL beyond the longest renewal and the heartbeat bound",
            section: json!({"hold": {
                "claim_ttl_ms":
                    actingcommand_contract::ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS
                        + HEARTBEAT_MS + 1,
            }}),
            config_key: CLAIM_TTL_MS,
            problem: ConfigValueProblem::ClaimTtlOutOfRange,
        },
        Row {
            case: "a rung bound and the margin beyond the longest renewal",
            section: json!({"hold": {
                "claim_ttl_ms":
                    actingcommand_contract::ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS,
                "margin_ms":
                    actingcommand_contract::ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS
                        - 100_000,
            }}),
            config_key: RUNGS,
            problem: ConfigValueProblem::RenewalCapExceeded,
        },
    ]
}

/// The instruction that sets the section's members to `section`: one change per key, a nested
/// object's members each a key of their own.
fn changes(section: &Value) -> Vec<ConfigSettingChange> {
    let mut changes = Vec::new();
    for (member, value) in section.as_object().expect("a section is an object") {
        match value {
            Value::Object(members) => {
                for (name, value) in members {
                    changes.push(ConfigSettingChange {
                        key: format!("recovery_ladder.{member}.{name}"),
                        value: Some(value.clone()),
                    });
                }
            }
            _ => changes.push(ConfigSettingChange {
                key: format!("recovery_ladder.{member}"),
                value: Some(value.clone()),
            }),
        }
    }
    changes
}

#[test]
fn d1_l4_a_broken_rule_is_refused_at_start_and_the_same_values_by_an_instruction() {
    for row in d1_l4_rows() {
        // At start (and in check-config): the file layer's check refuses the section.
        let refused = from_file(row.section.clone())
            .check(&context())
            .expect_err(row.case);
        assert_eq!(
            refused.failure_code(),
            ConfigCode::RecoveryLadderConfigInvalid,
            "{}",
            row.case
        );
        assert_eq!(refused.config_key(), row.config_key, "{}", row.case);
        assert_eq!(refused.problem(), Some(row.problem), "{}", row.case);

        // Through an instruction: the whole request is refused as `config_value_invalid` with
        // the same key and why, and the configuration in effect does not change.
        let running = from_file(json!({}));
        let refused = running
            .apply(&changes(&row.section), &stamp(), &context())
            .expect_err(row.case);
        assert_eq!(
            refused.failure_code(),
            ConfigCode::ConfigValueInvalid,
            "{}",
            row.case
        );
        assert_eq!(refused.config_key(), row.config_key, "{}", row.case);
        assert_eq!(refused.problem(), Some(row.problem), "{}", row.case);
        assert_eq!(
            running.recovery_ladder(&context()),
            Ok(default_ladder()),
            "{}",
            row.case
        );
    }
}

#[test]
fn d1_l4_the_shipped_defaults_pass_the_check_and_name_the_model_s_order() {
    let ladder = default_ladder();
    assert_eq!(
        ladder
            .rungs
            .iter()
            .map(RungConfig::action)
            .collect::<Vec<_>>(),
        vec![
            RecoveryRung::ReturnHome,
            RecoveryRung::ApplicationRestart,
            RecoveryRung::EmulatorRestart,
        ],
        "B4:1: return home, then restart the application, then restart the emulator"
    );
    assert_eq!(ladder.digest().len(), 64);
    // A changed value names another configuration.
    let changed = from_file(json!({"cooldown_secs": 60}))
        .recovery_ladder(&context())
        .expect("a shorter cool-down passes");
    assert_ne!(changed.digest(), ladder.digest());
}

/// SCHED §2.5 and §10: the scheduling rules' codes, none of which a default trigger item may
/// match (§3.6, RVW 8).
const SCHEDULING_CODES: &[&str] = &[
    "recognition_sample_deadline_exceeded",
    "candidate_sample_deadline_exceeded",
    "recognition_sample_budget_insufficient",
    "candidate_sample_budget_insufficient",
    "recognition_sample_budget_invalid",
    "contained_task_feature_locked",
    "policy_rerun_owed",
    "policy_rerun_waiting",
    "policy_rerun_expired",
    "policy_rerun_exhausted",
    "pack_admission_suspended",
    "pack_admission_lifted",
    "blackout_active",
    "blackout_approaching",
    "blackout_run_stopped",
    "blackout_config_invalid",
    "feature_locked",
    "session_feature_fact_invalid",
    "decision_reason_missing",
    "due_task_missing",
    "cost_commit_time_insufficient",
    "cost_commit_declaration_invalid",
    "policy_cost_commit_waiting",
    "pack_schedule_closed",
    "pack_schedule_unknown_package",
    "session_pack_schedule_invalid",
    "perf_scheduling_paused",
    "perf_pacing_gap",
    "perf_run_yielded",
];

/// §3.6: the four sampling cases (deadline, permission, time, samples).
const SAMPLING_CODES: &[&str] = &[
    "recognition_sample_deadline_exceeded",
    "candidate_sample_deadline_exceeded",
    "recognition_sample_budget_insufficient",
    "candidate_sample_budget_insufficient",
    "cost_commit_time_insufficient",
    "recognition_sample_budget_invalid",
];

/// The stages a trigger item may name.
const STAGES: [RecoveryTriggerStage; 4] = [
    RecoveryTriggerStage::Task,
    RecoveryTriggerStage::StartupPreparation,
    RecoveryTriggerStage::ConnectionPreparation,
    RecoveryTriggerStage::StabilityWarnings,
];

#[test]
fn d1_l9_the_default_trigger_items_name_listed_codes_and_match_no_scheduling_or_sampling_code() {
    // Every code and prefix the defaults file names is in the catalog.
    let section = &shipped()["recovery_ladder"];
    let mut named = section["exclude_codes"]
        .as_array()
        .expect("exclude_codes is a list")
        .clone();
    for item in section["triggers"].as_array().expect("triggers is a list") {
        named.extend(item["codes"].as_array().cloned().unwrap_or_default());
    }
    let catalog = actingcommand_contract::outcome::catalog_json().expect("the catalog parses");
    let codes = catalog["codes"]
        .as_object()
        .expect("the catalog lists codes");
    for entry in &named {
        let text = entry.as_str().expect("a code is text");
        let listed = match text.strip_suffix('*') {
            Some(prefix) => codes.keys().any(|code| code.starts_with(prefix)),
            None => codes.contains_key(text),
        };
        assert!(listed, "{text} is not in the catalog");
    }
    // No scheduling code and no sampling code reaches any default trigger item, at any stage.
    let ladder = default_ladder();
    for code in SCHEDULING_CODES.iter().chain(SAMPLING_CODES) {
        for stage in STAGES {
            assert!(
                !matches!(ladder.classify(stage, code), TriggerVerdict::Matched { .. }),
                "{code} at {stage:?} starts a ladder"
            );
        }
    }
}

#[test]
fn e3_g3_the_screen_disagrees_with_the_pack_codes_reach_pack_logic_and_sampling_codes_nothing() {
    let ladder = default_ladder();
    // The paths whose screen or flow disagrees with the pack (§3.2 class 4, §7.3), and NAV-6's
    // interrupt that ends a run (M373 §15 ruling, coordinator ruling 2).
    for code in [
        "contained_task_page_unknown",
        "contained_task_recognition_failed",
        "contained_task_recognition_conflict",
        "contained_task_step_unconfirmed",
        "contained_task_error_page_reached",
        "contained_task_home_entry_not_matched",
        "contained_task_linear_entry_unmatched",
        "contained_task_interrupt_escalated",
    ] {
        assert_eq!(
            ladder_class_of(code),
            Some(LadderClass::PackLogicMismatch),
            "{code}"
        );
        match ladder.classify(RecoveryTriggerStage::Task, code) {
            TriggerVerdict::Matched { item, by } => {
                assert_eq!(item.id, "pack_logic", "{code}");
                assert_eq!(
                    by,
                    MatchedBy::Class(LadderClass::PackLogicMismatch),
                    "{code}"
                );
            }
            other => panic!("{code} does not reach pack_logic: {other:?}"),
        }
    }
    // The sampling deadline and the short sample budget: class budget_or_permission, no item.
    for code in [
        "recognition_sample_deadline_exceeded",
        "recognition_sample_budget_insufficient",
    ] {
        assert_eq!(
            ladder_class_of(code),
            Some(LadderClass::BudgetOrPermission),
            "{code}"
        );
        assert_eq!(
            ladder.classify(RecoveryTriggerStage::Task, code),
            TriggerVerdict::Unmatched,
            "{code}"
        );
    }
    // The gate codes the exclusion list names start nothing although their class is class 4
    // (§3.3).
    for code in [
        "contained_task_prerequisite_entry_unmatched",
        "contained_task_return_home_entry_unmatched",
    ] {
        assert_eq!(
            ladder.classify(RecoveryTriggerStage::Task, code),
            TriggerVerdict::Excluded,
            "{code}"
        );
    }
}

#[test]
fn rb_h3_rung_three_s_bounds_are_the_configuration_s() {
    let shipped = shipped();
    let configured = shipped["recovery_ladder"]["rungs"]
        .as_array()
        .expect("rungs is a list")
        .iter()
        .find(|rung| rung["action"] == "emulator_restart")
        .expect("the defaults have rung 3")
        .clone();
    let bounds = |ladder: &LadderConfig| match ladder.rung(RecoveryRung::EmulatorRestart) {
        Some(RungConfig::EmulatorRestart {
            stop_bound_ms,
            start_bound_ms,
            readiness_window_ms,
            ..
        }) => (*stop_bound_ms, *start_bound_ms, *readiness_window_ms),
        other => panic!("no emulator restart rung: {other:?}"),
    };
    let as_ms = |value: &Value| value.as_u64().expect("a bound is a number");
    assert_eq!(
        bounds(&default_ladder()),
        (
            as_ms(&configured["stop_bound_ms"]),
            as_ms(&configured["start_bound_ms"]),
            as_ms(&configured["readiness_window_ms"]),
        )
    );
    // A rung that names only its action takes the defaults file's bounds of that action; one
    // that names its window has that window.
    let window = as_ms(&configured["readiness_window_ms"]) / 2;
    // The section names one rung, so it keeps the defaults file's trigger items that start
    // within it: a start rung beyond the rungs is refused (§4.1, §5.3).
    let default_start = shipped["recovery_ladder"]["trigger_defaults"]["start_rung"]
        .as_u64()
        .expect("the default start rung is a number");
    let triggers: Vec<Value> = shipped["recovery_ladder"]["triggers"]
        .as_array()
        .expect("triggers is a list")
        .iter()
        .filter(|item| item["start_rung"].as_u64().unwrap_or(default_start) <= 1)
        .cloned()
        .collect();
    let changed = from_file(json!({
        "rungs": [{"action": "emulator_restart", "readiness_window_ms": window}],
        "triggers": triggers,
    }))
    .recovery_ladder(&context())
    .expect("a shorter window passes");
    assert_eq!(
        bounds(&changed),
        (
            as_ms(&configured["stop_bound_ms"]),
            as_ms(&configured["start_bound_ms"]),
            window,
        )
    );
    assert_eq!(changed.rungs.len(), 1);
}

// Workflow #390 DRILL-1 (register DR2; contracts/emulator-control.md "Drills"): a person's drill
// is a ladder with trigger stage `drill` and matches no trigger item, so a
// `recovery_ladder.triggers[]` item that names the stage `drill` is refused `stage_not_allowed`
// naming `recovery_ladder.triggers`, at start (`recovery_ladder_config_invalid`) and by an
// instruction (`config_value_invalid`), and the configuration in effect stays (train 15
// rulings, DRILL-1 item 6).
#[test]
fn gate_drill1_a_trigger_item_naming_the_drill_stage_is_refused_at_start_and_by_an_instruction() {
    let section = json!({"triggers": [
        {"id": "drill_item", "stages": ["drill"], "classes": ["pack_logic_mismatch"]},
    ]});
    let refused = from_file(section.clone())
        .check(&context())
        .expect_err("a trigger item at the drill stage");
    assert_eq!(
        refused.failure_code(),
        ConfigCode::RecoveryLadderConfigInvalid
    );
    assert_eq!(refused.config_key(), TRIGGERS);
    assert_eq!(refused.problem(), Some(ConfigValueProblem::StageNotAllowed));

    let running = from_file(json!({}));
    let refused = running
        .apply(&changes(&section), &stamp(), &context())
        .expect_err("a trigger item at the drill stage, by instruction");
    assert_eq!(refused.failure_code(), ConfigCode::ConfigValueInvalid);
    assert_eq!(refused.config_key(), TRIGGERS);
    assert_eq!(refused.problem(), Some(ConfigValueProblem::StageNotAllowed));
    assert_eq!(running.recovery_ladder(&context()), Ok(default_ladder()));
}
