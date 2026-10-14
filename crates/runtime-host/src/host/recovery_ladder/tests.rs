// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #387 D1c, test row D1-L7 (frozen model M387 v1.2 §8.2): `ladder_invariants`, one row
//! per rule of §2.2b that stays in code (Alice 10-10, §12 question 4). The function reads no
//! configuration, so no `recovery_ladder` value changes a row.

use super::*;

#[test]
fn d1_l7_each_rule_that_stays_in_code_stops_a_ladder_whatever_the_configuration() {
    let rows = [
        (
            "A1: a fixture instance has nothing to restart",
            LadderFacts {
                physical: false,
                ..LadderFacts::PASSING
            },
            Some(LadderInvariant::NotPhysical),
        ),
        (
            "A2: the ladder's own preparation never starts a ladder",
            LadderFacts {
                recovery_preparation: true,
                ..LadderFacts::PASSING
            },
            Some(LadderInvariant::RecoveryPreparation),
        ),
        (
            "A3: a shutdown",
            LadderFacts {
                shutdown: true,
                ..LadderFacts::PASSING
            },
            Some(LadderInvariant::Shutdown),
        ),
        (
            "A3: an install drain",
            LadderFacts {
                draining: true,
                ..LadderFacts::PASSING
            },
            Some(LadderInvariant::Drain),
        ),
        (
            "A4: the key is lost",
            LadderFacts {
                holds_key: false,
                ..LadderFacts::PASSING
            },
            Some(LadderInvariant::KeyLost),
        ),
        (
            "A5: business capacity refuses",
            LadderFacts {
                capacity: false,
                ..LadderFacts::PASSING
            },
            Some(LadderInvariant::CapacityRefused),
        ),
        (
            "A6: a scheduling pause",
            LadderFacts {
                paused: true,
                ..LadderFacts::PASSING
            },
            Some(LadderInvariant::Paused),
        ),
        (
            "A7: a blackout window",
            LadderFacts {
                blackout: true,
                ..LadderFacts::PASSING
            },
            Some(LadderInvariant::Blackout),
        ),
        ("no rule holds", LadderFacts::PASSING, None),
    ];
    for (case, facts, expected) in rows {
        assert_eq!(ladder_invariants(&facts), expected, "{case}");
    }
    // A3-A6 end a climb with their interruption; a refused admission is admission_denied.
    for (invariant, interruption) in [
        (
            LadderInvariant::Shutdown,
            HostCode::RecoveryLadderShutdownRequested,
        ),
        (
            LadderInvariant::Drain,
            HostCode::RecoveryLadderDrainRequested,
        ),
        (LadderInvariant::KeyLost, HostCode::RecoveryLadderKeyLost),
        (
            LadderInvariant::CapacityRefused,
            HostCode::RecoveryAdmissionDenied,
        ),
        (LadderInvariant::Paused, HostCode::RecoveryAdmissionDenied),
    ] {
        assert_eq!(
            invariant.interruption(),
            interruption.as_str(),
            "{invariant:?}"
        );
    }
    // A8: an interrupted climb ends at warning, one whose rungs failed on their own at error.
    assert_eq!(ladder_exhausted_severity(true), EventSeverity::Warning);
    assert_eq!(ladder_exhausted_severity(false), EventSeverity::Error);
}

/// The `recovery_ladder` section of the defaults file with the members `section` names, as the
/// host reads it.
fn ladder(section: serde_json::Value) -> Arc<LadderConfig> {
    let (_, defaults) =
        actingcommand_runtime_config::DefaultsFile::shipped().expect("the defaults file parses");
    let parameters = actingcommand_runtime_config::render_parameters(
        &actingcommand_runtime_config::mechanism_rows(),
        &[],
        &defaults,
        &serde_json::json!({ "recovery_ladder": section }),
        &|_| None,
    )
    .expect("the sections resolve");
    let config = actingcommand_runtime_config::EffectiveConfig::from_manifest(
        &actingcommand_contract::RuntimeConfigManifest {
            subsystems: Vec::new(),
            parameters,
        },
    )
    .expect("every key is in the key table");
    Arc::new(
        config
            .recovery_ladder(&actingcommand_runtime_config::CheckContext::new(
                5_000,
                recovery_ladder_device_bounds(),
            ))
            .expect("the section passes its check"),
    )
}

/// The rules of the defaults file with the other value of each of the five.
fn other_rules() -> LadderRules {
    LadderRules {
        direct_runs: DirectRuns::Trigger,
        preparation_superseded: PreparationSuperseded::Keep,
        already_running: AlreadyRunning::Queue,
        cooldown_opens_on: CooldownOpensOn::Triggered,
        entry_unavailable: EntryUnavailable::Fail,
    }
}

#[test]
fn d1_l8_each_of_the_five_rules_changes_its_behaviour_with_its_value() {
    let defaults = ladder(serde_json::json!({})).rules;
    assert_eq!(
        defaults,
        LadderRules {
            direct_runs: DirectRuns::Exempt,
            preparation_superseded: PreparationSuperseded::Suppress,
            already_running: AlreadyRunning::Suppress,
            cooldown_opens_on: CooldownOpensOn::Admitted,
            entry_unavailable: EntryUnavailable::Skip,
        },
        "the defaults are today's behaviour (M387 section 2.2a)"
    );
    let other = other_rules();
    assert_eq!(
        ladder(serde_json::json!({"rules": {
            "direct_runs": "trigger",
            "preparation_superseded": "keep",
            "already_running": "queue",
            "cooldown_opens_on": "triggered",
            "entry_unavailable": "fail",
        }}))
        .rules,
        other
    );
    // B1: a direct run's terminal and records.
    assert!(direct_run_exempt(&defaults, true));
    assert!(!direct_run_exempt(&other, true));
    assert!(!direct_run_exempt(&defaults, false));
    // B2: a superseded preparation's ladder.
    assert!(preparation_superseded_suppresses(&defaults, true));
    assert!(!preparation_superseded_suppresses(&other, true));
    assert!(!preparation_superseded_suppresses(&defaults, false));
    // B3: a trigger while a ladder holds or waits for the instance.
    assert_eq!(
        admission(&defaults, true, false),
        Admission::Suppressed(RecoveryLadderSuppression::AlreadyRunning)
    );
    assert_eq!(
        admission(&other, true, false),
        Admission::Admitted {
            behind_ladder: true
        }
    );
    for rules in [defaults, other] {
        assert_eq!(
            admission(&rules, false, true),
            Admission::Suppressed(RecoveryLadderSuppression::Cooldown)
        );
        assert_eq!(
            admission(&rules, false, false),
            Admission::Admitted {
                behind_ladder: false
            }
        );
    }
    // B4: when the cool-down window opens.
    assert!(!opens_cooldown_window(&defaults, true));
    assert!(opens_cooldown_window(&defaults, false));
    assert!(opens_cooldown_window(&other, true));
    assert!(!opens_cooldown_window(&other, false));
    // B5: an unavailable entry channel.
    let refused = HostCode::RecoveryCaptureUnavailable.as_str();
    assert!(matches!(
        unavailable_entry(
            &defaults,
            RecoveryRungSkipReason::CaptureUnavailable,
            refused
        ),
        RungAttempt::Skipped {
            reason: RecoveryRungSkipReason::CaptureUnavailable
        }
    ));
    assert!(matches!(
        unavailable_entry(&other, RecoveryRungSkipReason::CaptureUnavailable, refused),
        RungAttempt::Failed {
            run_id: None,
            reason
        } if reason == refused
    ));
}

fn instance() -> InstanceId {
    *actingcommand_contract::IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport()
}

fn record(instance_id: InstanceId, at_unix_ms: u64, spelling: &str) -> CountedRecord<'_> {
    CountedRecord {
        instance_id,
        request_id: None,
        at_unix_ms,
        severity: EventSeverity::Error,
        spelling,
    }
}

/// A counter with `config` that counted `records`; the bursts it registered.
fn bursts(config: Arc<LadderConfig>, records: &[CountedRecord<'_>]) -> Vec<StabilityBurst> {
    let mut counter = StabilityCounter::default();
    counter.configure(config);
    for record in records {
        counter.count(record);
    }
    counter.take().0
}

const OPEN_FAILED: &str = "capture_backend_open_failed";

#[test]
fn h4_g6_direction_one_starts_at_the_configured_count_within_the_configured_window() {
    let node = instance();
    let defaults = ladder(serde_json::json!({}));
    // Three class-5 records within ten minutes register one burst with its window.
    let three = bursts(
        Arc::clone(&defaults),
        &[
            record(node, 1_000, OPEN_FAILED),
            record(node, 2_000, OPEN_FAILED),
            record(node, 3_000, OPEN_FAILED),
        ],
    );
    let [burst] = three.as_slice() else {
        panic!("one burst: {}", three.len());
    };
    assert_eq!(burst.instance_id, node);
    assert_eq!(
        burst.config.triggers[burst.item].id, "stability_burst",
        "the default direction-one item"
    );
    assert_eq!(
        (
            burst.count,
            burst.window_ms,
            burst.first_unix_ms,
            burst.last_unix_ms
        ),
        (3, 600_000, 1_000, 3_000)
    );
    assert_eq!(burst.failure_code.as_str(), OPEN_FAILED);
    // Two are not enough; three whose first left the window are not either.
    assert!(
        bursts(
            Arc::clone(&defaults),
            &[
                record(node, 1_000, OPEN_FAILED),
                record(node, 2_000, OPEN_FAILED)
            ],
        )
        .is_empty()
    );
    assert!(
        bursts(
            Arc::clone(&defaults),
            &[
                record(node, 1_000, OPEN_FAILED),
                record(node, 2_000, OPEN_FAILED),
                record(node, 602_001, OPEN_FAILED),
            ],
        )
        .is_empty()
    );
    // Another count and window: the same records follow them.
    let short = ladder(serde_json::json!({"triggers": [{
        "id": "short_burst",
        "stages": ["stability_warnings"],
        "classes": ["environment_fault"],
        "count": 2,
        "window_ms": 1_000,
    }]}));
    assert_eq!(
        bursts(
            Arc::clone(&short),
            &[
                record(node, 1_000, OPEN_FAILED),
                record(node, 2_000, OPEN_FAILED)
            ],
        )
        .len(),
        1
    );
    assert!(
        bursts(
            short,
            &[
                record(node, 1_000, OPEN_FAILED),
                record(node, 2_001, OPEN_FAILED)
            ],
        )
        .is_empty()
    );
    // Without class 5 the item counts no class-5 record; a class-6 record still counts.
    let stability_only = ladder(serde_json::json!({"triggers": [{
        "id": "stability_burst",
        "stages": ["stability_warnings"],
        "classes": ["stability_warning"],
        "count": 3,
        "window_ms": 600_000,
    }]}));
    assert!(
        bursts(
            Arc::clone(&stability_only),
            &[
                record(node, 1_000, OPEN_FAILED),
                record(node, 2_000, OPEN_FAILED),
                record(node, 3_000, OPEN_FAILED),
            ],
        )
        .is_empty()
    );
    let close_failed = "capture_backend_close_failed";
    assert_eq!(
        bursts(
            stability_only,
            &[
                record(node, 1_000, close_failed),
                record(node, 2_000, close_failed),
                record(node, 3_000, close_failed),
            ],
        )
        .len(),
        1
    );
}

#[test]
fn d1_l5_direction_one_counts_routine_warnings_and_errors_only() {
    let node = instance();
    let other = instance();
    let direct = *actingcommand_contract::IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_request_id()
        .expect("request id")
        .transport();
    let mut counter = StabilityCounter::default();
    counter.configure(ladder(serde_json::json!({})));
    // A warning and an error count; an info does not.
    for (at_unix_ms, severity) in [
        (1_000, EventSeverity::Warning),
        (2_000, EventSeverity::Info),
        (3_000, EventSeverity::Error),
    ] {
        counter.count(&CountedRecord {
            severity,
            ..record(node, at_unix_ms, OPEN_FAILED)
        });
    }
    assert!(counter.take().0.is_empty(), "two counted records");
    // The ladder's own work on its instance does not count.
    counter.set_climbing(other, true);
    for at_unix_ms in [1_000, 2_000, 3_000] {
        counter.count(&record(other, at_unix_ms, OPEN_FAILED));
    }
    assert!(counter.take().0.is_empty(), "a climbing instance's records");
    counter.set_climbing(other, false);
    // A direct run's records count only under `direct_runs = trigger`.
    let direct_records = |instance_id| {
        [1_000, 2_000, 3_000].map(|at_unix_ms| CountedRecord {
            request_id: Some(direct),
            ..record(instance_id, at_unix_ms, OPEN_FAILED)
        })
    };
    counter.set_direct_run(direct, true);
    for record in &direct_records(other) {
        counter.count(record);
    }
    assert!(counter.take().0.is_empty(), "a direct run's records");
    let mut triggering = StabilityCounter::default();
    triggering.configure(ladder(
        serde_json::json!({"rules": {"direct_runs": "trigger"}}),
    ));
    triggering.set_direct_run(direct, true);
    for record in &direct_records(other) {
        triggering.count(record);
    }
    assert_eq!(triggering.take().0.len(), 1, "direct_runs = trigger");
    // The burst empties every window of its instance; the counter registers it for the sweep
    // and admits nothing itself.
    counter.count(&record(node, 4_000, OPEN_FAILED));
    let (registered, _) = counter.take();
    assert_eq!(
        registered.len(),
        1,
        "the third counted record of the first instance"
    );
    counter.count(&record(node, 5_000, OPEN_FAILED));
    counter.count(&record(node, 6_000, OPEN_FAILED));
    assert!(counter.take().0.is_empty(), "the window started again");
    // A code the catalog does not list counts for nothing and is handed on once.
    counter.count(&record(node, 7_000, "neutral_unlisted_failure"));
    counter.count(&record(node, 8_000, "neutral_unlisted_failure"));
    let (registered, unclassified) = counter.take();
    assert!(registered.is_empty());
    assert_eq!(unclassified, vec!["neutral_unlisted_failure".to_owned()]);
}

// Workflow #387 D1c-F: the codes that invalid input or a Runtime-internal defect now end with
// are neither class 5 nor class 6, so three such failures on one instance within the window
// register no burst and start no ladder: the residual preparation defect (`none`), a request's
// contract refusal (`none`), a pack swipe's declaration refusal (class 3) and a stability region
// that does not fit (class 3). The stability sample's frame-data half is class 5 and still counts.
#[test]
fn d1cf_three_recoded_failures_on_one_instance_within_the_window_start_no_ladder() {
    use actingcommand_contract::codes::RuntimeContractCode;
    use actingcommand_execution_kernel::ExecutionKernelCode;

    let node = instance();
    let defaults = ladder(serde_json::json!({}));
    let three = |spelling: &'static str| {
        [1_000, 2_000, 3_000].map(|at_unix_ms| record(node, at_unix_ms, spelling))
    };
    for spelling in [
        ExecutionKernelCode::InputPlanPreparationFailed.as_str(),
        RuntimeContractCode::InvalidSegmentedSwipeContract.as_str(),
        ExecutionKernelCode::ContainedTaskOperationInvalid.as_str(),
        ExecutionKernelCode::ContainedTaskControlInvalid.as_str(),
    ] {
        assert!(
            bursts(Arc::clone(&defaults), &three(spelling)).is_empty(),
            "{spelling} registered a burst"
        );
    }
    assert_eq!(
        bursts(
            defaults,
            &three(ExecutionKernelCode::ContainedTaskFrameInvalid.as_str())
        )
        .len(),
        1,
        "the frame-data half counts as class 5"
    );
}
