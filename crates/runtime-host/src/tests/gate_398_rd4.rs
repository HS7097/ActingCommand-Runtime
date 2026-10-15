// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #398 RD-4 (model-398 v1.1 §6.2-§6.6, option B; §12 rows RD-4-T1 to T7): the ladder
// profiles through the host. A scheduled run on one instance fails with a code the trigger items
// match, and its ladder climbs: on a handset (a bound handset endpoint, a neutral fake serial) by
// the profile the instance takes, on an emulator by the section's `rungs`. The ladder fixture of
// `instance_workers` binds a return-home package and configures a startup package and no emulator
// control. Every expected value comes from the model: the keys and their order of use (§6.2), the
// defaults (§6.3), the record's `profile` (§6.4), B5 on a handset (§6.5) and the rule
// `rules.start_beyond_profile` (§6.6, coordinator ruling of 2026-10-15, option B, default
// `exhaust`). The default trigger item `navigation_exhausted` starts at rung 3 on a code no
// fixture produces yet, so the rows of §6.6 configure an item of that id and start rung on the
// failure the fixture produces.

use super::gate_398_rd1b::{bound, resolved_handset};
use super::instance_workers::{
    admit_ladder_run, ladder_finished, ladder_setup, phases, rung_outcomes, wait_for_ladder_end,
};
use super::*;
use actingcommand_contract::{
    ConfigSettingChange, LeasePayload, RecoveryLadderOutcome, RecoveryRung, RecoveryRungOutcome,
    RecoveryRungPlan, RecoveryRungSkipReason, RecoveryRungState, RuntimeConfigManifest,
    RuntimeLifecyclePhase,
};
use actingcommand_runtime_config::{
    CheckContext, ConfigCode, DefaultsFile, EffectiveConfig, annotate_settable, mechanism_rows,
    render_parameters,
};
use serde_json::{Value, json};

const SERIAL: &str = "TESTSERIAL0096";
// The ladder fixture's heartbeat bound (`ladder_setup`).
const HEARTBEAT_MS: u64 = 20;
// The device-state sampler's interval and fact lifetime in these hosts: an hour, so no sample
// reads the handset while a test runs; only the start gate of each run does.
const QUIET_MS: u64 = 3_600_000;

// What drives the fixture's one instance.
enum Device {
    Emulator,
    Handset,
    // A handset whose device state answers from this fake.
    ScriptedHandset(Arc<FakeHandset>),
}

fn instance_text(instance: InstanceId) -> String {
    match serde_json::to_value(instance).expect("instance id encodes") {
        Value::String(text) => text,
        other => panic!("an instance id is a string: {other}"),
    }
}

// The configuration manifest actingd assembles from a file that names `file`'s members, for the
// instances `instances` (registry ids), over the defaults file.
fn manifest(file: &Value, instances: &[String]) -> RuntimeConfigManifest {
    let (_, defaults) = DefaultsFile::shipped().expect("the defaults file parses");
    let mut file = file.clone();
    file["device_state"] = json!({"sample_interval_ms": QUIET_MS, "fact_ttl_ms": QUIET_MS});
    let mut parameters =
        render_parameters(&mechanism_rows(), instances, &defaults, &file, &|_| None)
            .expect("the sections resolve");
    annotate_settable(&mut parameters).expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

// A manifest whose `recovery_ladder` section names `section`'s members.
fn ladder_manifest(section: Value) -> RuntimeConfigManifest {
    manifest(&json!({ "recovery_ladder": section }), &[])
}

// The digest of the `recovery_ladder` section `manifest` holds (M387 §4.7).
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

struct LadderHost {
    _root: TempDir,
    host: RuntimeHost,
    state: Arc<FakeState>,
    request: ContainedTaskRequest,
}

fn ladder_host(
    device: &Device,
    instance: InstanceId,
    manifest: RuntimeConfigManifest,
) -> LadderHost {
    let root = TempDir::new().expect("tempdir");
    let (host_config, request) = ladder_setup(root.path());
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    let mut provider = FakeProvider::one(POLICY_INSTANCE_ALIAS, instance, Arc::clone(&state));
    if !matches!(device, Device::Emulator) {
        state.observed_open.store(true, Ordering::Release);
        provider = provider.with_resolved_for(
            POLICY_INSTANCE_ALIAS,
            resolved_handset(
                POLICY_INSTANCE_ALIAS,
                instance,
                bound(POLICY_INSTANCE_ALIAS, SERIAL),
            ),
        );
    }
    if let Device::ScriptedHandset(handset) = device {
        provider = provider.with_handset(POLICY_INSTANCE_ALIAS, Arc::clone(handset));
    }
    let host = RuntimeHost::start(
        host_config.with_config_manifest(manifest),
        Arc::new(provider),
    )
    .expect("physical runtime host");
    LadderHost {
        _root: root,
        host,
        state,
        request,
    }
}

impl LadderHost {
    // The scheduled run is admitted, `before_failure` runs, its captures turn unknown and it
    // fails (class `pack_logic_mismatch`).
    fn fail(&self, before_failure: impl FnOnce(&RuntimeHost)) -> Box<PolicyRunContext> {
        let context = admit_ladder_run(&self.host, &self.request);
        before_failure(&self.host);
        self.state.unknown_capture.store(true, Ordering::Release);
        let error = self
            .host
            .run_scheduled_contained_task(&context, &self.request)
            .expect_err("the scheduled run fails");
        assert!(!error.is_fatal(), "{}", error.code());
        context
    }

    // `fail`, then the ladder's end and release.
    fn climb(&self) -> Box<PolicyRunContext> {
        let context = self.fail(|_| {});
        wait_for_ladder_end(&self.host, POLICY_INSTANCE_ALIAS);
        context
    }

    fn events(&self) -> Vec<PersistedEvent> {
        self.host
            .query_persisted_events_for_test(EventQuery::default())
            .expect("ledger events")
    }

    fn configure(&self, changes: Vec<ConfigSettingChange>) -> RuntimeReceipt {
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        self.host
            .process_request_for_test(
                &runtime_request(&ids, RuntimeOperation::ConfigureSettings { changes }),
                ConnectionId::new(396).expect("connection"),
            )
            .expect("configure receipt")
    }

    fn close(self) {
        assert!(self.host.fatal_error().expect("runtime health").is_none());
        self.host.close().expect("close runtime host");
    }
}

// The plan, digest and profile of every `recovery_ladder_started`.
fn started(events: &[PersistedEvent]) -> Vec<(Vec<RecoveryRungPlan>, String, Option<String>)> {
    phases(events)
        .into_iter()
        .filter_map(|(_, phase)| match phase {
            RuntimeLifecyclePhase::RecoveryLadderStarted {
                rungs,
                config_digest,
                profile,
                ..
            } => Some((rungs, config_digest, profile)),
            _ => None,
        })
        .collect()
}

// The one ladder's plan, digest and profile.
fn one_ladder(events: &[PersistedEvent]) -> (Vec<RecoveryRungPlan>, String, Option<String>) {
    let mut ladders = started(events);
    assert_eq!(ladders.len(), 1, "one ladder: {ladders:?}");
    ladders.remove(0)
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

// The section's three rungs as a profile's.
fn three_rungs() -> Value {
    json!([
        {"action": "return_home"},
        {"action": "application_restart"},
        {"action": "emulator_restart"},
    ])
}

// A trigger item of the default `navigation_exhausted`'s id and start rung (3), on the failure
// the fixture produces.
fn starting_at_rung_three() -> Value {
    json!([{
        "id": "navigation_exhausted",
        "stages": ["task"],
        "classes": ["pack_logic_mismatch"],
        "start_rung": 3,
    }])
}

// A trigger item whose ladder waits in the queue until performance control releases the
// instance (M387 §3.7), so a test acts while it waits.
fn after_release() -> Value {
    json!([{
        "id": "after_release",
        "stages": ["task"],
        "classes": ["pack_logic_mismatch"],
        "start": "after_performance_release",
    }])
}

// The runs whose records name the instance's key after the failed run's: the rungs that ran.
fn rung_runs(events: &[PersistedEvent], context: &PolicyRunContext) -> BTreeSet<RunId> {
    events
        .iter()
        .filter(|event| event.event_type() == EventType::TaskRequested)
        .filter_map(|event| event.links().run_id().copied())
        .filter(|run| *run != context.run_id())
        .collect()
}

// The releases of the ladder's key: the lease the failed run's end handed to the ladder, and
// each continuation's after it.
fn ladder_key_releases(events: &[PersistedEvent], context: &PolicyRunContext) -> usize {
    let mut chain = vec![context.lease_token().lease_id()];
    for event in events {
        if let EventPayload::Lease(LeasePayload::Transferred(next)) = event.payload()
            && chain.contains(&next.from_lease_id())
        {
            chain.push(next.to_lease_id());
        }
    }
    let ladder_leases = &chain[1..];
    assert!(!ladder_leases.is_empty(), "the run's end hands its key on");
    events
        .iter()
        .filter(|event| {
            event.event_type() == EventType::LeaseReleased
                && event
                    .links()
                    .lease_id()
                    .is_some_and(|lease| ladder_leases.contains(lease))
        })
        .count()
}

// RD-4-T1 (§6.2, §6.3, §6.4): under the defaults a handset climbs the `handset` profile: return
// home, then restart the application, with no emulator restart row at all, and its
// `recovery_ladder_started` names the profile.
#[test]
fn rd4_t1_a_handset_climbs_the_default_profile_without_an_emulator_restart() {
    let ladder = ladder_host(&Device::Handset, instance_id(), ladder_manifest(json!({})));
    ladder.climb();
    let events = ladder.events();
    let (plan, _, profile) = one_ladder(&events);
    assert_eq!(profile.as_deref(), Some("handset"));
    assert_eq!(
        plan,
        vec![
            pending(RecoveryRung::ReturnHome),
            pending(RecoveryRung::ApplicationRestart),
        ]
    );
    assert_eq!(
        rung_outcomes(&events)
            .into_iter()
            .map(|(rung, _, _)| rung)
            .collect::<Vec<_>>(),
        vec![RecoveryRung::ReturnHome, RecoveryRung::ApplicationRestart]
    );
    ladder.close();
}

// RD-4-T2 (§6.2): the instance's own `recovery_ladder_profile` comes before the kind table; a
// profile of the section's three rungs on a handset skips the emulator restart as today
// (`no_emulator_control`), and the record names that profile.
#[test]
fn rd4_t2_an_instance_key_naming_a_three_rung_profile_skips_the_emulator_restart_as_today() {
    let instance = instance_id();
    let id = instance_text(instance);
    let file = json!({
        "recovery_ladder": {"profiles": {
            "handset": {"rungs": [{"action": "return_home"}, {"action": "application_restart"}]},
            "three_rungs": {"rungs": three_rungs()},
        }},
        "instances": [{"instance_id": id, "recovery_ladder_profile": "three_rungs"}],
    });
    let ladder = ladder_host(&Device::Handset, instance, manifest(&file, &[id]));
    ladder.climb();
    let events = ladder.events();
    let (plan, _, profile) = one_ladder(&events);
    assert_eq!(profile.as_deref(), Some("three_rungs"));
    assert_eq!(
        plan,
        vec![
            pending(RecoveryRung::ReturnHome),
            pending(RecoveryRung::ApplicationRestart),
            skipped(
                RecoveryRung::EmulatorRestart,
                RecoveryRungSkipReason::NoEmulatorControl
            ),
        ]
    );
    let outcomes = rung_outcomes(&events);
    assert_eq!(
        outcomes.last(),
        Some(&(
            RecoveryRung::EmulatorRestart,
            RecoveryRungOutcome::Skipped,
            Some("recovery_rung_skipped_no_emulator_control".to_owned())
        ))
    );
    ladder.close();
}

// RD-4-T4 (§6.2, §8.2's two-configuration rule): with the handset profile [application restart]
// the handset's plan follows it, with the defaults it is [return home, application restart];
// under both an emulator climbs the section's three rungs.
#[test]
fn rd4_t4_the_handset_plan_follows_its_profile_and_an_emulator_keeps_the_section_s_rungs() {
    let section_plan = vec![
        pending(RecoveryRung::ReturnHome),
        pending(RecoveryRung::ApplicationRestart),
        skipped(
            RecoveryRung::EmulatorRestart,
            RecoveryRungSkipReason::NoEmulatorControl,
        ),
    ];
    let cases = [
        (
            "the defaults",
            json!({}),
            vec![
                pending(RecoveryRung::ReturnHome),
                pending(RecoveryRung::ApplicationRestart),
            ],
        ),
        (
            "a handset profile of the application restart alone",
            json!({"profiles": {"handset": {"rungs": [{"action": "application_restart"}]}}}),
            vec![pending(RecoveryRung::ApplicationRestart)],
        ),
    ];
    for (case, section, handset_plan) in cases {
        for (device, plan, profile) in [
            (Device::Handset, &handset_plan, Some("handset")),
            (Device::Emulator, &section_plan, None),
        ] {
            let ladder = ladder_host(&device, instance_id(), ladder_manifest(section.clone()));
            ladder.climb();
            let (recorded, _, recorded_profile) = one_ladder(&ladder.events());
            assert_eq!(&recorded, plan, "{case}");
            assert_eq!(recorded_profile.as_deref(), profile, "{case}");
            ladder.close();
        }
    }
}

// RD-4-T5 (§6.2 snapshot, M387 §4.7): a handset's ladder waits in the queue; a new handset
// profile set meanwhile takes effect for the next trigger only: the waiting ladder climbs the
// profile and records the digest it was admitted with.
#[test]
fn rd4_t5_a_queued_ladder_keeps_the_profile_and_digest_it_was_admitted_with() {
    let admitted = ladder_manifest(json!({"triggers": after_release()}));
    let digest = digest_of(&admitted);
    let ladder = ladder_host(&Device::Handset, instance_id(), admitted);
    ladder.fail(|host| {
        host.hold_performance_for_test(true)
            .expect("hold the instance back from release");
    });
    assert!(started(&ladder.events()).is_empty());
    let receipt = ladder.configure(vec![ConfigSettingChange {
        key: "recovery_ladder.profiles".to_owned(),
        value: Some(json!({"handset": {"rungs": [{"action": "application_restart"}]}})),
    }]);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    ladder
        .host
        .hold_performance_for_test(false)
        .expect("release the instance");
    wait_for_ladder_end(&ladder.host, POLICY_INSTANCE_ALIAS);
    let (plan, recorded_digest, profile) = one_ladder(&ladder.events());
    assert_eq!(recorded_digest, digest);
    assert_eq!(profile.as_deref(), Some("handset"));
    assert_eq!(
        plan,
        vec![
            pending(RecoveryRung::ReturnHome),
            pending(RecoveryRung::ApplicationRestart),
        ],
        "the admitted profile, not the changed one"
    );
    ladder.close();
}

// RD-4-T6 (§6.4, §6.5 B5 `skip`): the handset's start gate holds rung 1's run (a keyguard that
// waits for a person): the rung is skipped `device_not_ready`, and rung 2 runs as usual.
#[test]
fn rd4_t6_a_device_state_hold_at_rung_one_skips_it_device_not_ready_and_rung_two_runs() {
    let handset = Arc::new(FakeHandset::default());
    let ladder = ladder_host(
        &Device::ScriptedHandset(Arc::clone(&handset)),
        instance_id(),
        ladder_manifest(json!({"triggers": after_release()})),
    );
    ladder.fail(|host| {
        host.hold_performance_for_test(true)
            .expect("hold the instance back from release");
    });
    // The next start gate, rung 1's, finds the keyguard waiting for a person; rung 2's does not.
    handset.script(|state| state.locked_keyguard_reads = 1);
    ladder
        .host
        .hold_performance_for_test(false)
        .expect("release the instance");
    wait_for_ladder_end(&ladder.host, POLICY_INSTANCE_ALIAS);
    let events = ladder.events();
    let outcomes = rung_outcomes(&events);
    let finished = |action: RecoveryRung| {
        outcomes
            .iter()
            .find(|(rung, _, _)| *rung == action)
            .cloned()
            .unwrap_or_else(|| panic!("{action:?} finished: {outcomes:?}"))
    };
    let (_, result, code) = finished(RecoveryRung::ReturnHome);
    assert_eq!(result, RecoveryRungOutcome::Skipped);
    assert_eq!(
        code.as_deref(),
        Some("recovery_rung_skipped_device_not_ready")
    );
    let (_, result, _) = finished(RecoveryRung::ApplicationRestart);
    assert_ne!(result, RecoveryRungOutcome::Skipped);
    assert_eq!(
        outcomes
            .iter()
            .map(|(rung, _, _)| *rung)
            .collect::<Vec<_>>(),
        vec![RecoveryRung::ReturnHome, RecoveryRung::ApplicationRestart]
    );
    let (_, _, rungs_tried) = ladder_finished(&events).expect("the ladder's end");
    assert_eq!(rungs_tried, 1);
    ladder.close();
}

// RD-4-T7 (§6.6, option B, `exhaust`, the default): a trigger that starts at rung 3 on a handset
// whose profile has two. The ladder starts, names the profile, skips both rungs as before the
// start, runs none, ends exhausted and releases its key once.
#[test]
fn rd4_t7_under_exhaust_a_start_beyond_the_handset_profile_runs_no_rung() {
    let exhausted = ladder_host(
        &Device::Handset,
        instance_id(),
        ladder_manifest(json!({"triggers": starting_at_rung_three()})),
    );
    let context = exhausted.climb();
    let events = exhausted.events();
    let (plan, _, profile) = one_ladder(&events);
    assert_eq!(profile.as_deref(), Some("handset"));
    assert_eq!(
        plan,
        vec![
            skipped(
                RecoveryRung::ReturnHome,
                RecoveryRungSkipReason::BeforeStartRung
            ),
            skipped(
                RecoveryRung::ApplicationRestart,
                RecoveryRungSkipReason::BeforeStartRung
            ),
        ]
    );
    let (_, result, rungs_tried) = ladder_finished(&events).expect("the ladder's end");
    assert_eq!(result, RecoveryLadderOutcome::Exhausted);
    assert_eq!(rungs_tried, 0);
    assert!(rung_runs(&events, &context).is_empty(), "no rung runs");
    assert_eq!(ladder_key_releases(&events, &context), 1, "one release");
    exhausted.close();
}

// RD-4-T7 (§6.6, `last_rung`): the same trigger on the same handset climbs the profile's last
// rung, the application restart, alone.
#[test]
fn rd4_t7_under_last_rung_a_start_beyond_the_handset_profile_climbs_its_last_rung() {
    let last_rung = ladder_host(
        &Device::Handset,
        instance_id(),
        ladder_manifest(json!({
            "triggers": starting_at_rung_three(),
            "rules": {"start_beyond_profile": "last_rung"},
        })),
    );
    let context = last_rung.climb();
    let events = last_rung.events();
    let (plan, _, profile) = one_ladder(&events);
    assert_eq!(
        plan,
        vec![
            skipped(
                RecoveryRung::ReturnHome,
                RecoveryRungSkipReason::BeforeStartRung
            ),
            pending(RecoveryRung::ApplicationRestart),
        ]
    );
    assert_eq!(profile.as_deref(), Some("handset"));
    let (_, _, rungs_tried) = ladder_finished(&events).expect("the ladder's end");
    assert_eq!(rungs_tried, 1);
    assert_eq!(
        rung_runs(&events, &context).len(),
        1,
        "the application restart's run"
    );
    last_rung.close();
}

// RD-4-T7 (§6.6): an emulator climbs the section's rungs from the trigger's start rung under each
// of the three values. Under `refuse` every start rung fits every profile, so its item starts at
// rung 2.
#[test]
fn rd4_t7_an_emulator_climbs_the_section_s_rungs_under_each_value() {
    let starting_at_rung_two = json!([{
        "id": "navigation_exhausted",
        "stages": ["task"],
        "classes": ["pack_logic_mismatch"],
        "start_rung": 2,
    }]);
    for (rule, triggers, plan) in [
        (
            "exhaust",
            starting_at_rung_three(),
            vec![
                skipped(
                    RecoveryRung::ReturnHome,
                    RecoveryRungSkipReason::BeforeStartRung,
                ),
                skipped(
                    RecoveryRung::ApplicationRestart,
                    RecoveryRungSkipReason::BeforeStartRung,
                ),
                skipped(
                    RecoveryRung::EmulatorRestart,
                    RecoveryRungSkipReason::NoEmulatorControl,
                ),
            ],
        ),
        (
            "last_rung",
            starting_at_rung_three(),
            vec![
                skipped(
                    RecoveryRung::ReturnHome,
                    RecoveryRungSkipReason::BeforeStartRung,
                ),
                skipped(
                    RecoveryRung::ApplicationRestart,
                    RecoveryRungSkipReason::BeforeStartRung,
                ),
                skipped(
                    RecoveryRung::EmulatorRestart,
                    RecoveryRungSkipReason::NoEmulatorControl,
                ),
            ],
        ),
        (
            "refuse",
            starting_at_rung_two,
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
        ),
    ] {
        let emulator = ladder_host(
            &Device::Emulator,
            instance_id(),
            ladder_manifest(json!({
                "triggers": triggers,
                "rules": {"start_beyond_profile": rule},
            })),
        );
        emulator.climb();
        let (recorded, _, profile) = one_ladder(&emulator.events());
        assert_eq!(recorded, plan, "{rule}");
        assert_eq!(profile, None, "{rule}");
        emulator.close();
    }
}

// RD-4-T7 (§6.6, `refuse`): every trigger item's start rung must fit every profile. The defaults
// start `navigation_exhausted` at rung 3 and give the handset profile two rungs, so a start under
// `refuse` is refused with `recovery_ladder_config_invalid` naming the handset profile
// (`start_rung_out_of_range`); through an instruction the same value is `config_value_invalid`,
// and the configuration in effect stays: the next ladder still climbs under `exhaust`.
#[test]
fn rd4_t7_refuse_is_refused_at_start_and_by_instruction_and_the_configuration_stays() {
    let root = TempDir::new().expect("tempdir");
    let refused = RuntimeHost::start(
        config(&root).with_config_manifest(ladder_manifest(json!({
            "rules": {"start_beyond_profile": "refuse"},
        }))),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    );
    let Err(error) = refused else {
        panic!("a start under refuse with a profile shorter than a start rung is refused");
    };
    let outcome = error.outcome();
    assert_eq!(
        outcome.code().as_str(),
        ConfigCode::RecoveryLadderConfigInvalid.as_str()
    );
    assert_eq!(
        outcome.values().get("config_key").and_then(Value::as_str),
        Some("recovery_ladder.profiles.handset")
    );
    assert_eq!(
        outcome
            .values()
            .get("config_value_problem")
            .and_then(Value::as_str),
        Some("start_rung_out_of_range")
    );

    let ladder = ladder_host(
        &Device::Handset,
        instance_id(),
        ladder_manifest(json!({"triggers": starting_at_rung_three()})),
    );
    let receipt = ladder.configure(vec![ConfigSettingChange {
        key: "recovery_ladder.rules.start_beyond_profile".to_owned(),
        value: Some(json!("refuse")),
    }]);
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied, "{receipt:?}");
    let outcome = receipt.outcome();
    assert_eq!(
        outcome.code().as_str(),
        ConfigCode::ConfigValueInvalid.as_str()
    );
    assert_eq!(
        outcome.values().get("config_key").and_then(Value::as_str),
        Some("recovery_ladder.profiles.handset")
    );
    assert_eq!(
        outcome
            .values()
            .get("config_value_problem")
            .and_then(Value::as_str),
        Some("start_rung_out_of_range")
    );
    ladder.climb();
    let (plan, _, _) = one_ladder(&ladder.events());
    assert_eq!(
        plan,
        vec![
            skipped(
                RecoveryRung::ReturnHome,
                RecoveryRungSkipReason::BeforeStartRung
            ),
            skipped(
                RecoveryRung::ApplicationRestart,
                RecoveryRungSkipReason::BeforeStartRung
            ),
        ],
        "still exhaust"
    );
    ladder.close();
}
