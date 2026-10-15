// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #398 RD-5b (model-398 v1.1 §7.2-§7.6, §10.1 and §12 rows RD-5b-T1 to T7 and T10):
// the device state of a bound handset through the host. The handset's state is a fake that
// answers in the shapes RD-5a's parsers read and records every command that reaches it; a test
// runs a package directly (the start gate runs before its first step) or runs one sample at once
// (`device_state_sample_for_test`; the sampler's own interval is set to an hour, so it stays
// quiet). Every expected value comes from the model: the screen check of §7.3, the holds of §7.4
// and §7.5, the availability source and the facts of §7.6 and the codes of §10.1. The serial is
// a neutral fake, distinct from the other test modules', and no record names it.

use super::gate_398_rd1b::{bound, last_availability, resolved_handset};
use super::gate_398_rd2::{fact, instance_facts};
use super::instance_workers::phases;
use super::*;
use crate::codes::{DeviceHold, DeviceStateHostCode};
use actingcommand_contract::outcome::{Code, Outcome};
use actingcommand_contract::{FactScalar, RuntimeConfigManifest, RuntimeLifecyclePhase};
use actingcommand_device::codes::DeviceStateCode;
use actingcommand_device::{DeviceStateAction, DeviceStateRead, handset_label};
use actingcommand_runtime_config::{
    DefaultsFile, annotate_settable, mechanism_rows, render_parameters,
};
use serde_json::{Value, json};

const SERIAL: &str = "TESTSERIAL0051";
const QUIET_MS: u64 = 3_600_000;

// The configuration manifest of a start whose `device_state` section is `section` over the
// sampler's quiet interval (an hour, and a TTL of at least that), as actingd assembles it.
fn device_state_manifest(section: &Value) -> RuntimeConfigManifest {
    let mut merged = json!({
        "sample_interval_ms": QUIET_MS,
        "fact_ttl_ms": QUIET_MS
    });
    for (key, value) in section.as_object().expect("a section object") {
        merged[key] = value.clone();
    }
    let (_, defaults) = DefaultsFile::shipped().expect("the defaults file parses");
    let mut parameters = render_parameters(
        &mechanism_rows(),
        &[],
        &defaults,
        &json!({ "device_state": merged }),
        &|_| None,
    )
    .expect("the sections resolve");
    annotate_settable(&mut parameters).expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

// A started host whose policy instance is a handset bound to `SERIAL`, beside an emulator
// instance; each answers its device state from a fake (the emulator's must never be asked).
struct HandsetHost {
    root: TempDir,
    host: RuntimeHost,
    state: Arc<FakeState>,
    emulator_state: Arc<FakeState>,
    handset: Arc<FakeHandset>,
    emulator_handset: Arc<FakeHandset>,
    handset_id: InstanceId,
}

fn handset_host(section: &Value) -> HandsetHost {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    state.observed_open.store(true, Ordering::Release);
    let emulator_state = Arc::new(FakeState::default());
    emulator_state.observed_open.store(true, Ordering::Release);
    let handset = Arc::new(FakeHandset::default());
    let emulator_handset = Arc::new(FakeHandset::default());
    let handset_id = instance_id();
    let host = RuntimeHost::start(
        config(&root)
            .with_policy_inputs(PolicyInputSnapshot::new(
                pending_policy_facts(),
                pending_policy_resources(),
            ))
            .with_config_manifest(device_state_manifest(section)),
        Arc::new(
            FakeProvider::from_entries([
                (
                    POLICY_INSTANCE_ALIAS.to_owned(),
                    handset_id,
                    Arc::clone(&state),
                ),
                (
                    POLICY_INSTANCE_ALIAS_B.to_owned(),
                    instance_id(),
                    Arc::clone(&emulator_state),
                ),
            ])
            .with_device_endpoints()
            .with_resolved_for(
                POLICY_INSTANCE_ALIAS,
                resolved_handset(
                    POLICY_INSTANCE_ALIAS,
                    handset_id,
                    bound(POLICY_INSTANCE_ALIAS, SERIAL),
                ),
            )
            .with_handset(POLICY_INSTANCE_ALIAS, Arc::clone(&handset))
            .with_handset(POLICY_INSTANCE_ALIAS_B, Arc::clone(&emulator_handset)),
        ),
    )
    .expect("runtime host with a handset");
    HandsetHost {
        root,
        host,
        state,
        emulator_state,
        handset,
        emulator_handset,
        handset_id,
    }
}

impl HandsetHost {
    // One direct run of the neutral package on `alias`, whose device is `state`.
    fn run_on(&self, alias: &str, state: &FakeState) -> RuntimeReceipt {
        let package = super::gate_369s5::direct_run_package(&self.root, state);
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        self.host
            .process_request_for_test(
                &runtime_request(
                    &ids,
                    RuntimeOperation::run_contained_task(
                        alias,
                        ids.mint_holder_id().expect("holder"),
                        ContainedTaskRequest::new(package.0, package.1).expect("task request"),
                    ),
                ),
                ConnectionId::new(398).expect("connection"),
            )
            .expect("the run's receipt")
    }

    fn run_handset(&self) -> RuntimeReceipt {
        self.run_on(POLICY_INSTANCE_ALIAS, &self.state)
    }

    fn inputs(&self) -> usize {
        self.state.input_count.load(Ordering::Acquire)
    }

    fn events(&self) -> Vec<PersistedEvent> {
        self.host
            .query_persisted_events_for_test(EventQuery::default())
            .expect("ledger events")
    }

    fn close(self) {
        assert!(self.host.fatal_error().expect("runtime health").is_none());
        assert_serial_unnamed(&self.events());
        self.host.close().expect("close host");
    }
}

fn label() -> String {
    handset_label(POLICY_INSTANCE_ALIAS)
}

fn is(outcome: &Outcome, code: impl Into<Code>) -> bool {
    outcome.is(code)
}

// The outcomes of the `device_state_observed` records with `code`, in order.
fn state_records(events: &[PersistedEvent], code: impl Into<Code> + Copy) -> Vec<Outcome> {
    phases(events)
        .into_iter()
        .filter_map(|(_, phase)| match phase {
            RuntimeLifecyclePhase::DeviceStateObserved { outcome } if is(&outcome, code) => {
                Some(outcome)
            }
            _ => None,
        })
        .collect()
}

// The run did not start: it failed before its first step with `code`, not fatally.
fn assert_refused(receipt: &RuntimeReceipt, code: impl Into<Code> + Copy) -> Outcome {
    assert_eq!(receipt.state(), RuntimeReceiptState::Failed, "{receipt:?}");
    assert!(!receipt.error_projection().expect("the run's failure").fatal);
    let outcome = receipt.outcome().clone();
    assert!(is(&outcome, code), "{outcome:?}");
    outcome
}

fn assert_completed(receipt: &RuntimeReceipt) {
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
}

// T10's identifier rule: no record names the bound serial.
fn assert_serial_unnamed(events: &[PersistedEvent]) {
    for event in events {
        let shown = format!("{:?}", event.payload());
        assert!(
            !shown.contains(SERIAL),
            "a record names the serial: {shown}"
        );
    }
}

// T1 (§7.3): a dark screen gets exactly one wake key and the configured settle wait, and once
// it reads awake the run starts (`device_screen_woken`, the handset's label); a screen that stays
// dark ends the run before its first step with `device_wake_failed`, and no input reaches the
// device. Two settle waits.
#[test]
fn rd5b_t1_a_dark_screen_is_woken_once_before_the_run_or_the_run_does_not_start() {
    for settle_ms in [1_000_u64, 2_500] {
        let run = handset_host(&json!({ "wake_settle_ms": settle_ms }));
        run.handset.script(|state| state.screen = "Asleep");
        assert_completed(&run.run_handset());
        assert_eq!(run.handset.actions(), [DeviceStateAction::Wake]);
        let settles = run
            .handset
            .commands()
            .into_iter()
            .filter_map(|command| match command {
                FakeHandsetCommand::Settle(wait) => Some(wait),
                FakeHandsetCommand::Read(_) | FakeHandsetCommand::Act(_) => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(settles, [Duration::from_millis(settle_ms)]);
        assert!(run.inputs() > 0, "the run started");
        let woken = state_records(&run.events(), DeviceStateHostCode::DeviceScreenWoken);
        let [woken] = woken.as_slice() else {
            panic!("one woken screen: {woken:?}");
        };
        assert_eq!(outcome_str(woken, "device_label"), Some(label().as_str()));

        run.handset.script(|state| {
            state.screen = "Asleep";
            state.wake_fails = true;
        });
        let inputs = run.inputs();
        let refused = assert_refused(&run.run_handset(), DeviceStateCode::DeviceWakeFailed);
        assert_eq!(
            outcome_str(&refused, "device_label"),
            Some(label().as_str())
        );
        assert_eq!(run.inputs(), inputs, "no input after the refusal");
        assert_eq!(
            run.handset.actions(),
            [DeviceStateAction::Wake, DeviceStateAction::Wake],
            "exactly one wake key per start"
        );
        run.close();
    }
}

// T2 (§7.3): a keyguard without a credential gets exactly one `wm dismiss-keyguard` under
// `dismiss` and the run starts; under `wait` nothing is sent and the run does not start
// (`device_locked`); a keyguard still showing after the dismissal ends the run with
// `device_keyguard_dismiss_failed`, before any input.
#[test]
fn rd5b_t2_a_keyguard_without_a_credential_is_dismissed_once_or_left_to_a_person() {
    let insecure = |state: &mut FakeHandsetState| {
        state.keyguard_showing = true;
        state.keyguard_secure = false;
    };

    let dismiss = handset_host(&json!({}));
    dismiss.handset.script(insecure);
    assert_completed(&dismiss.run_handset());
    assert_eq!(
        dismiss.handset.actions(),
        [DeviceStateAction::DismissKeyguard]
    );
    dismiss.handset.script(|state| {
        insecure(state);
        state.dismiss_fails = true;
    });
    let inputs = dismiss.inputs();
    let refused = assert_refused(
        &dismiss.run_handset(),
        DeviceStateCode::DeviceKeyguardDismissFailed,
    );
    assert_eq!(
        outcome_str(&refused, "device_label"),
        Some(label().as_str())
    );
    assert_eq!(dismiss.inputs(), inputs);
    assert_eq!(
        dismiss.handset.actions(),
        [
            DeviceStateAction::DismissKeyguard,
            DeviceStateAction::DismissKeyguard
        ]
    );
    dismiss.close();

    let wait = handset_host(&json!({ "keyguard_insecure": "wait" }));
    wait.handset.script(insecure);
    let refused = assert_refused(&wait.run_handset(), DeviceStateCode::DeviceLocked);
    assert_eq!(
        outcome_str(&refused, "device_label"),
        Some(label().as_str())
    );
    assert!(
        wait.handset.actions().is_empty(),
        "nothing is sent under wait"
    );
    assert_eq!(wait.inputs(), 0);
    wait.close();
}

// T3 (§7.3, §7.6): a keyguard with a credential is `device_locked` (warning): the run does not
// start, nothing is sent, no input, and the handset is unavailable under `device_state:locked`.
// Once a person unlocked it, the next sample lifts the hold (`device_hold_lifted`, `locked`) and
// the handset is available again; a run then starts.
#[test]
fn rd5b_t3_a_locked_handset_waits_for_a_person_and_the_next_sample_after_the_unlock_restores_it() {
    let run = handset_host(&json!({}));
    assert!(last_availability(&run.events(), POLICY_INSTANCE_ALIAS).0);
    run.handset.script(|state| {
        state.keyguard_showing = true;
        state.keyguard_secure = true;
    });
    let refused = assert_refused(&run.run_handset(), DeviceStateCode::DeviceLocked);
    assert_eq!(
        refused.category(),
        actingcommand_contract::outcome::Category::Warning
    );
    assert!(run.handset.actions().is_empty());
    assert_eq!(run.inputs(), 0);
    assert_eq!(
        last_availability(&run.events(), POLICY_INSTANCE_ALIAS),
        (false, "device_state:locked".to_owned())
    );
    // Still locked at a sample: nothing lifts.
    run.host.device_state_sample_for_test().expect("a sample");
    assert!(state_records(&run.events(), DeviceStateHostCode::DeviceHoldLifted).is_empty());

    run.handset.script(|state| state.keyguard_showing = false);
    run.host.device_state_sample_for_test().expect("a sample");
    let events = run.events();
    let lifted = state_records(&events, DeviceStateHostCode::DeviceHoldLifted);
    let [lifted] = lifted.as_slice() else {
        panic!("one lifted hold: {lifted:?}");
    };
    assert_eq!(
        outcome_str(lifted, "device_hold"),
        Some(DeviceHold::Locked.as_str())
    );
    assert!(last_availability(&events, POLICY_INSTANCE_ALIAS).0);
    assert_completed(&run.run_handset());
    run.close();
}

// T4, through the host (§7.4): at the hold level the run does not start
// (`device_thermal_hold`, its status) and a dark screen is not woken; the handset is unavailable
// under `device_state:thermal`; a sample between the levels keeps the hold, one below the
// resume level lifts it.
#[test]
fn rd5b_t4_a_hot_handset_holds_its_runs_until_it_cools_below_the_resume_level() {
    let run = handset_host(&json!({}));
    run.handset.script(|state| {
        state.thermal_status = 3;
        state.screen = "Asleep";
    });
    let refused = assert_refused(&run.run_handset(), DeviceStateHostCode::DeviceThermalHold);
    assert_eq!(outcome_str(&refused, "thermal_status"), Some("severe"));
    assert!(
        run.handset.actions().is_empty(),
        "a held handset is not woken"
    );
    assert_eq!(run.inputs(), 0);
    assert_eq!(
        last_availability(&run.events(), POLICY_INSTANCE_ALIAS),
        (false, "device_state:thermal".to_owned())
    );
    run.handset.script(|state| state.thermal_status = 2);
    run.host.device_state_sample_for_test().expect("a sample");
    assert!(state_records(&run.events(), DeviceStateHostCode::DeviceHoldLifted).is_empty());
    run.handset.script(|state| state.thermal_status = 1);
    run.host.device_state_sample_for_test().expect("a sample");
    let events = run.events();
    let lifted = state_records(&events, DeviceStateHostCode::DeviceHoldLifted);
    let [lifted] = lifted.as_slice() else {
        panic!("one lifted hold: {lifted:?}");
    };
    assert_eq!(
        outcome_str(lifted, "device_hold"),
        Some(DeviceHold::Thermal.as_str())
    );
    assert!(last_availability(&events, POLICY_INSTANCE_ALIAS).0);
    run.close();
}

// T5, through the host (§7.4): on no power source below the level the run does not start
// (`device_battery_low`, the level) and the handset is unavailable under
// `device_state:battery_low`; plugged in again, still low and not charging, the next sample lifts
// the hold.
#[test]
fn rd5b_t5_an_unplugged_low_battery_holds_and_plugging_it_in_lifts_the_hold() {
    let run = handset_host(&json!({}));
    run.handset.script(|state| {
        state.ac_powered = false;
        state.usb_powered = false;
        state.battery_status = 3;
        state.battery_level = 10;
    });
    let refused = assert_refused(&run.run_handset(), DeviceStateHostCode::DeviceBatteryLow);
    assert_eq!(
        refused.value("battery_percent").and_then(Value::as_i64),
        Some(10)
    );
    assert_eq!(run.inputs(), 0);
    assert_eq!(
        last_availability(&run.events(), POLICY_INSTANCE_ALIAS),
        (false, "device_state:battery_low".to_owned())
    );
    run.handset.script(|state| {
        state.usb_powered = true;
        state.battery_status = 4;
    });
    run.host.device_state_sample_for_test().expect("a sample");
    let events = run.events();
    let lifted = state_records(&events, DeviceStateHostCode::DeviceHoldLifted);
    let [lifted] = lifted.as_slice() else {
        panic!("one lifted hold: {lifted:?}");
    };
    assert_eq!(
        outcome_str(lifted, "device_hold"),
        Some(DeviceHold::BatteryLow.as_str())
    );
    assert!(last_availability(&events, POLICY_INSTANCE_ALIAS).0);
    run.close();
}

// T6, through the host (§7.5): under `in_use_gate`, another application in the foreground keeps
// the run from starting (`device_in_use`) and the handset is unavailable under
// `device_state:in_use`; with the instance's application or the home screen in front the run
// starts. With the gate off (the default) another application does not hold the run.
#[test]
fn rd5b_t6_another_application_in_front_holds_the_run_only_under_the_in_use_gate() {
    let gated = handset_host(&json!({ "in_use_gate": true }));
    gated
        .handset
        .script(|state| state.foreground = Some("com.example.other"));
    let refused = assert_refused(&gated.run_handset(), DeviceStateHostCode::DeviceInUse);
    assert_eq!(
        outcome_str(&refused, "device_label"),
        Some(label().as_str())
    );
    assert_eq!(gated.inputs(), 0);
    assert_eq!(
        last_availability(&gated.events(), POLICY_INSTANCE_ALIAS),
        (false, "device_state:in_use".to_owned())
    );
    gated
        .handset
        .script(|state| state.foreground = Some("neutral.application"));
    assert_completed(&gated.run_handset());
    assert!(last_availability(&gated.events(), POLICY_INSTANCE_ALIAS).0);
    gated
        .handset
        .script(|state| state.foreground = Some("com.example.launcher"));
    assert_completed(&gated.run_handset());
    gated.close();

    let open = handset_host(&json!({}));
    open.handset
        .script(|state| state.foreground = Some("com.example.other"));
    assert_completed(&open.run_handset());
    open.close();
}

// T7 (§7.2): an emulator instance gets no device-state command: neither its runs' start nor a
// sample asks the provider for its device state, while the handset's are asked.
#[test]
fn rd5b_t7_an_emulator_gets_no_device_state_command() {
    let run = handset_host(&json!({}));
    assert_completed(&run.run_on(POLICY_INSTANCE_ALIAS_B, &run.emulator_state));
    run.host.device_state_sample_for_test().expect("a sample");
    assert_eq!(run.emulator_handset.consulted(), 0);
    assert!(run.emulator_handset.commands().is_empty());
    assert!(run.handset.consulted() > 0, "the handset's sample asked");
    let reads = run
        .handset
        .commands()
        .into_iter()
        .filter_map(|command| match command {
            FakeHandsetCommand::Read(read) => Some(read),
            FakeHandsetCommand::Act(_) | FakeHandsetCommand::Settle(_) => None,
        })
        .collect::<Vec<_>>();
    assert!(reads.contains(&DeviceStateRead::Power), "{reads:?}");
    run.close();
}

// T10 (§7.6): a sample writes `device.screen`, `device.keyguard`, `device.battery` (one row:
// level, plugged, status, temperature in tenths of a degree), `device.thermal` and
// `device.screen_off_timeout_ms` on the handset's instance, each with the configured TTL, and no
// value names the device. Two TTLs.
#[test]
fn rd5b_t10_a_sample_writes_the_device_facts_with_the_configured_ttl() {
    for ttl_ms in [QUIET_MS, 2 * QUIET_MS] {
        let run = handset_host(&json!({ "fact_ttl_ms": ttl_ms }));
        run.host.device_state_sample_for_test().expect("a sample");
        let facts = instance_facts(&run.host, run.handset_id);
        let expected = [
            (
                "device.screen",
                ContractFactValue::String("awake".to_owned()),
            ),
            (
                "device.keyguard",
                ContractFactValue::String("none".to_owned()),
            ),
            (
                "device.thermal",
                ContractFactValue::String("none".to_owned()),
            ),
            (
                "device.screen_off_timeout_ms",
                ContractFactValue::DurationMs(600_000),
            ),
            (
                "device.battery",
                ContractFactValue::RecordList(vec![BTreeMap::from([
                    ("level".to_owned(), FactScalar::Integer(80)),
                    ("plugged".to_owned(), FactScalar::String("ac".to_owned())),
                    (
                        "status".to_owned(),
                        FactScalar::String("charging".to_owned()),
                    ),
                    ("temperature_deci_c".to_owned(), FactScalar::Integer(305)),
                ])]),
            ),
        ];
        for (key, value) in &expected {
            assert_eq!(fact(&facts, key).as_ref(), Some(value), "{key}");
            let record = facts
                .iter()
                .find(|record| record.key == *key)
                .expect("the fact's record");
            assert_eq!(record.ttl_ms, Some(ttl_ms), "{key}");
            assert!(
                !format!("{:?}", record.value).contains(SERIAL),
                "{key} names the device"
            );
        }
        run.close();
    }
}
