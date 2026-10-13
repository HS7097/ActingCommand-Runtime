// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #387 D1c, the host rows of the ladder tests (frozen model M387 v1.2 §8.2): H-4 G6
//! (a routine run's failure starts the ladder its trigger item names; the exclusion list starts
//! none), H-4 G7 (the plan and the climb follow the configured rungs and start rung), D1-L2 and
//! D1-L3 (a ladder that waits for performance release takes no key at the failed lease end,
//! waits, and climbs by the configuration it was admitted with) and D1-L4 (the same broken value
//! sent by an instruction is refused whole and the Runtime goes on). Every expected value comes
//! from the model: §3.1, §3.3, §3.7, §4.1, §4.7, §5.6, §6.1, §7.1.

use super::instance_workers::{admit_ladder_run, ladder_setup, phases, wait_for_ladder_end};
use super::*;
use actingcommand_contract::outcome::LadderClass;
use actingcommand_contract::{
    ConfigSettingChange, RecoveryLadderTrigger, RecoveryRung, RecoveryRungPlan,
    RecoveryRungSkipReason, RecoveryRungState, RecoveryTriggerMatch, RecoveryTriggerStage,
    RuntimeConfigManifest, RuntimeLifecyclePhase,
};
use actingcommand_runtime_config::{
    CheckContext, DefaultsFile, EffectiveConfig, annotate_settable, mechanism_rows,
    render_parameters,
};
use actingcommand_scheduler::ClaimKind;
use serde_json::{Value, json};

/// The ladder fixture's heartbeat bound (`ladder_setup`).
const HEARTBEAT_MS: u64 = 20;

/// A configuration manifest whose `recovery_ladder` section is the defaults file's with the
/// members `section` names, as actingd assembles it.
pub(super) fn ladder_manifest(section: Value) -> RuntimeConfigManifest {
    let (_, defaults) = DefaultsFile::shipped().expect("the defaults file parses");
    let mut parameters = render_parameters(
        &mechanism_rows(),
        &[],
        &defaults,
        &json!({ "recovery_ladder": section }),
        &|_| None,
    )
    .expect("the sections resolve");
    annotate_settable(&mut parameters).expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

/// The digest of the `recovery_ladder` section `manifest` holds (§4.7).
fn digest_of(manifest: &RuntimeConfigManifest) -> String {
    EffectiveConfig::from_manifest(manifest)
        .expect("the manifest's keys")
        .recovery_ladder(&CheckContext::new(
            HEARTBEAT_MS,
            crate::recovery_ladder_device_bounds(),
        ))
        .expect("the section passes its check")
        .digest()
        .to_owned()
}

fn all_events(host: &RuntimeHost) -> Vec<PersistedEvent> {
    host.query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events")
}

/// The trigger, plan and digest of every `recovery_ladder_started`.
fn started(
    events: &[PersistedEvent],
) -> Vec<(RecoveryLadderTrigger, Vec<RecoveryRungPlan>, String)> {
    phases(events)
        .into_iter()
        .filter_map(|(_, phase)| match phase {
            RuntimeLifecyclePhase::RecoveryLadderStarted {
                trigger,
                rungs,
                config_digest,
                ..
            } => Some((trigger, rungs, config_digest)),
            _ => None,
        })
        .collect()
}

/// The rungs every `recovery_rung_finished` names, in the order they finished.
fn finished_rungs(events: &[PersistedEvent]) -> Vec<RecoveryRung> {
    phases(events)
        .into_iter()
        .filter_map(|(_, phase)| match phase {
            RuntimeLifecyclePhase::RecoveryRungFinished { rung, .. } => Some(rung),
            _ => None,
        })
        .collect()
}

fn any_ladder_record(events: &[PersistedEvent]) -> bool {
    phases(events).into_iter().any(|(_, phase)| {
        matches!(
            phase,
            RuntimeLifecyclePhase::RecoveryLadderStarted { .. }
                | RuntimeLifecyclePhase::RecoveryLadderSuppressed { .. }
        )
    })
}

/// The ladder fixture of `instance_workers` (a scheduled physical run that binds a return-home
/// package, on an instance with a startup package and no emulator control) on a host whose
/// configuration is `manifest`; the run is admitted, its captures turn unknown and it fails.
fn failed_routine_run(
    manifest: RuntimeConfigManifest,
    before_failure: impl FnOnce(&RuntimeHost),
) -> (TempDir, RuntimeHost, Arc<FakeState>) {
    let root = TempDir::new().expect("tempdir");
    let (host_config, request) = ladder_setup(root.path());
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    let host = RuntimeHost::start(
        host_config.with_config_manifest(manifest),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::clone(&state),
        )),
    )
    .expect("physical runtime host");
    let context = admit_ladder_run(&host, &request);
    before_failure(&host);
    state.unknown_capture.store(true, Ordering::Release);
    let error = host
        .run_scheduled_contained_task(&context, &request)
        .expect_err("the scheduled run fails");
    assert!(!error.is_fatal(), "{}", error.code());
    (root, host, state)
}

fn pending(rung: RecoveryRung) -> RecoveryRungPlan {
    RecoveryRungPlan {
        rung,
        state: RecoveryRungState::Pending,
        reason: None,
    }
}

fn skipped(rung: RecoveryRung, reason: RecoveryRungSkipReason) -> RecoveryRungPlan {
    RecoveryRungPlan {
        rung,
        state: RecoveryRungState::Skipped,
        reason: Some(reason),
    }
}

/// H-4 G6 ① and ⑤, H-4 G7: the same routine failure under four configurations. The defaults
/// climb return home, application restart, emulator restart (skipped: no emulator control) by
/// the item `pack_logic`; rungs that name only their action climb in their configured order;
/// an item's start rung skips the rungs before it; the exclusion list starts nothing.
#[test]
fn h4_g6_g7_a_routine_failure_climbs_by_the_configured_trigger_item_and_rungs() {
    let cases = [
        (
            "the defaults",
            json!({}),
            Some((
                "pack_logic",
                vec![
                    pending(RecoveryRung::ReturnHome),
                    pending(RecoveryRung::ApplicationRestart),
                    skipped(
                        RecoveryRung::EmulatorRestart,
                        RecoveryRungSkipReason::NoEmulatorControl,
                    ),
                ],
            )),
        ),
        (
            "rungs reordered by action alone",
            // Three rungs, so every default trigger item's start rung lies within them (§4.1).
            json!({"rungs": [
                {"action": "application_restart"},
                {"action": "return_home"},
                {"action": "emulator_restart"},
            ]}),
            Some((
                "pack_logic",
                vec![
                    pending(RecoveryRung::ApplicationRestart),
                    pending(RecoveryRung::ReturnHome),
                    skipped(
                        RecoveryRung::EmulatorRestart,
                        RecoveryRungSkipReason::NoEmulatorControl,
                    ),
                ],
            )),
        ),
        (
            "an item starting at rung 2",
            json!({"triggers": [{
                "id": "late_start",
                "stages": ["task"],
                "classes": ["pack_logic_mismatch"],
                "start_rung": 2,
            }]}),
            Some((
                "late_start",
                vec![
                    skipped(
                        RecoveryRung::ReturnHome,
                        RecoveryRungSkipReason::BeforeStartRung,
                    ),
                    pending(RecoveryRung::ApplicationRestart),
                    skipped(
                        RecoveryRung::EmulatorRestart,
                        RecoveryRungSkipReason::NoEmulatorControl,
                    ),
                ],
            )),
        ),
        (
            "the code excluded",
            json!({"exclude_codes": [
                "contained_task_page_unknown",
                "contained_task_prerequisite_*",
                "contained_task_return_home_entry_unmatched",
            ]}),
            None,
        ),
    ];
    for (case, section, expected) in cases {
        let manifest = ladder_manifest(section);
        let digest = digest_of(&manifest);
        let (_root, host, _state) = failed_routine_run(manifest, |_| {});
        let Some((rule, plan)) = expected else {
            // The decision is made inside the failed lease end: no ladder claim exists after it.
            let (holder, queued) = host
                .instance_claims_for_test(POLICY_INSTANCE_ALIAS)
                .expect("instance claims");
            assert_ne!(holder, Some(ClaimKind::Ladder), "{case}");
            assert!(!queued.contains(&ClaimKind::Ladder), "{case}");
            assert!(!any_ladder_record(&all_events(&host)), "{case}");
            host.close().expect("close runtime host");
            continue;
        };
        wait_for_ladder_end(&host, POLICY_INSTANCE_ALIAS);
        let events = all_events(&host);
        let ladders = started(&events);
        let [(trigger, rungs, config_digest)] = ladders.as_slice() else {
            panic!("{case}: one ladder: {ladders:?}");
        };
        assert_eq!(trigger.stage, RecoveryTriggerStage::Task, "{case}");
        assert_eq!(trigger.rule.as_deref(), Some(rule), "{case}");
        assert_eq!(
            trigger.matched,
            Some(RecoveryTriggerMatch::Class {
                ladder_class: LadderClass::PackLogicMismatch
            }),
            "{case}"
        );
        assert_eq!(*config_digest, digest, "{case}");
        assert_eq!(*rungs, plan, "{case}");
        // The rungs finish in the plan's order, skipped ones included.
        assert_eq!(
            finished_rungs(&events),
            plan.iter().map(|rung| rung.rung).collect::<Vec<_>>(),
            "{case}"
        );
        host.close().expect("close runtime host");
    }
}

/// D1-L3 and D1-L2: an item that starts after performance release. While performance control
/// holds the instance, the failed lease end hands nothing on and the ladder's claim waits in the
/// queue; a change of the rungs meanwhile leaves the waiting ladder its admitted configuration
/// (plan and digest); once released, the ladder takes the key and climbs.
#[test]
fn d1_l2_d1_l3_a_ladder_that_waits_for_release_climbs_by_the_configuration_it_was_admitted_with() {
    let manifest = ladder_manifest(json!({"triggers": [{
        "id": "after_release",
        "stages": ["task"],
        "classes": ["pack_logic_mismatch"],
        "start": "after_performance_release",
    }]}));
    let admitted = digest_of(&manifest);
    let (_root, host, _state) = failed_routine_run(manifest, |host| {
        host.hold_performance_for_test(true)
            .expect("hold the instance back from release");
    });
    // No transfer at the lease end: the run's key is released and the ladder's claim waits.
    let (holder, queued) = host
        .instance_claims_for_test(POLICY_INSTANCE_ALIAS)
        .expect("instance claims");
    assert_eq!(holder, None);
    assert!(queued.contains(&ClaimKind::Ladder), "{queued:?}");
    assert!(started(&all_events(&host)).is_empty());
    // A new rung list while it waits takes effect for the next trigger only (§4.7, §5.8).
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let receipt = host
        .process_request_for_test(
            &runtime_request(
                &ids,
                RuntimeOperation::ConfigureSettings {
                    changes: vec![ConfigSettingChange {
                        key: "recovery_ladder.rungs".to_owned(),
                        value: Some(json!([{"action": "return_home"}])),
                    }],
                },
            ),
            ConnectionId::new(387).expect("connection"),
        )
        .expect("configure receipt");
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert!(started(&all_events(&host)).is_empty());
    host.hold_performance_for_test(false)
        .expect("release the instance");
    wait_for_ladder_end(&host, POLICY_INSTANCE_ALIAS);
    let events = all_events(&host);
    let ladders = started(&events);
    let [(trigger, rungs, config_digest)] = ladders.as_slice() else {
        panic!("one ladder: {ladders:?}");
    };
    assert_eq!(trigger.rule.as_deref(), Some("after_release"));
    assert_eq!(
        trigger.start,
        actingcommand_contract::RecoveryTriggerStart::AfterPerformanceRelease
    );
    assert_eq!(*config_digest, admitted);
    assert_eq!(
        rungs.iter().map(|plan| plan.rung).collect::<Vec<_>>(),
        vec![
            RecoveryRung::ReturnHome,
            RecoveryRung::ApplicationRestart,
            RecoveryRung::EmulatorRestart,
        ],
        "the admitted rungs, not the changed ones"
    );
    host.close().expect("close runtime host");
}

/// D1-L4 (instruction half): a value that breaks the section's rule sent by `ConfigureSettings`
/// refuses the whole request as `config_value_invalid` naming the key and why; the Runtime goes
/// on and keeps its configuration.
#[test]
fn d1_l4_a_broken_ladder_value_by_instruction_is_refused_whole_and_the_runtime_goes_on() {
    let root = TempDir::new().expect("tempdir");
    let host = RuntimeHost::start(
        config(&root).with_config_manifest(ladder_manifest(json!({}))),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let configure = |changes: Vec<ConfigSettingChange>| {
        host.process_request_for_test(
            &runtime_request(&ids, RuntimeOperation::ConfigureSettings { changes }),
            ConnectionId::new(388).expect("connection"),
        )
        .expect("configure receipt")
    };
    let refused = configure(vec![
        ConfigSettingChange {
            key: "recovery_ladder.cooldown_secs".to_owned(),
            value: Some(json!(60)),
        },
        ConfigSettingChange {
            key: "recovery_ladder.hold.claim_ttl_ms".to_owned(),
            value: Some(json!(HEARTBEAT_MS)),
        },
    ]);
    assert_eq!(refused.state(), RuntimeReceiptState::Denied, "{refused:?}");
    let outcome = refused.outcome();
    assert_eq!(
        outcome.code().as_str(),
        actingcommand_runtime_config::ConfigCode::ConfigValueInvalid.as_str()
    );
    assert_eq!(
        outcome.values().get("config_key").and_then(Value::as_str),
        Some("recovery_ladder.hold.claim_ttl_ms")
    );
    assert_eq!(
        outcome
            .values()
            .get("config_value_problem")
            .and_then(Value::as_str),
        Some("claim_ttl_out_of_range")
    );
    // Neither value took effect, and the Runtime answers.
    let accepted = configure(vec![ConfigSettingChange {
        key: "recovery_ladder.cooldown_secs".to_owned(),
        value: Some(json!(60)),
    }]);
    assert_eq!(
        accepted.state(),
        RuntimeReceiptState::Completed,
        "{accepted:?}"
    );
    host.close().expect("close runtime host");
}
