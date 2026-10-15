// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #398 RD-5b (model-398 v1.1 §7.2-§7.5, §7.8 and §12 rows RD-5b-T3, T4, T5, T6 and
// T9): the holds of a bound handset, driven by hand-given times and readings; no device, no
// host. Every expected value comes from the model: the thermal hysteresis and the battery rule of
// §7.4, the foreground rule and its backoff of §7.5, the keyguard of §7.3 and the keepalive of
// §7.2, each row that reads a configured value run under two configurations so the behaviour
// follows the configuration.

use super::*;

const ALIAS: &str = "handset-a";

fn instance() -> InstanceId {
    *actingcommand_contract::IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport()
}

// The starting values of model-398 §7.8 (the in-use gate on), with the given thermal levels,
// battery level, in-use backoff and keepalive margin.
fn config(
    thermal: (ThermalLevel, ThermalLevel),
    battery_min_percent: u8,
    in_use_backoff_ms: u64,
    keepalive_margin_ms: u64,
) -> DeviceStateConfig {
    DeviceStateConfig {
        sample_interval_ms: 60_000,
        fact_ttl_ms: 180_000,
        wake_settle_ms: 1_000,
        keepalive_margin_ms,
        keyguard_insecure: KeyguardInsecureAction::Dismiss,
        battery_min_percent,
        thermal_hold_at: thermal.0,
        thermal_resume_below: thermal.1,
        in_use_gate: true,
        in_use_backoff_ms,
    }
}

fn defaults() -> DeviceStateConfig {
    config(
        (ThermalLevel::Severe, ThermalLevel::Moderate),
        15,
        300_000,
        60_000,
    )
}

fn machine(handset: InstanceId, config: &DeviceStateConfig) -> HoldMachine {
    HoldMachine::new([(handset, ALIAS.to_owned())], 0, config)
}

fn battery(plugged: BatteryPlugged, status: BatteryStatus, level_percent: u8) -> BatteryState {
    BatteryState {
        level_percent,
        status,
        plugged,
        temperature_deci_c: 300,
    }
}

fn charging() -> BatteryState {
    battery(BatteryPlugged::Ac, BatteryStatus::Charging, 80)
}

fn thermal_sample(status: ThermalStatus) -> Sample {
    Sample {
        thermal: Some(status),
        ..Sample::default()
    }
}

// T4 (§7.4, §7.8): the thermal hold starts at `thermal_hold_at` and lifts only once the status
// is below `thermal_resume_below`; in between it stays as it is. Two pairs of levels give two
// different points of entry and release.
#[test]
fn rd5b_t4_the_thermal_hold_starts_at_its_level_and_lifts_only_below_the_resume_level() {
    let cases = [
        (
            (ThermalLevel::Severe, ThermalLevel::Moderate),
            ThermalStatus::Moderate,
            ThermalStatus::Severe,
            ThermalStatus::Light,
        ),
        (
            (ThermalLevel::Critical, ThermalLevel::Light),
            ThermalStatus::Severe,
            ThermalStatus::Critical,
            ThermalStatus::None,
        ),
    ];
    for (levels, below_hold, at_hold, below_resume) in cases {
        let config = config(levels, 15, 300_000, 60_000);
        let handset = instance();
        let mut machine = machine(handset, &config);
        let (changes, _) =
            machine.on_sample(handset, 1, &config, &thermal_sample(below_hold), false);
        assert!(changes.is_empty(), "{levels:?}: below the hold level");
        assert_eq!(machine.hold(handset), None, "{levels:?}");

        let (changes, _) = machine.on_sample(handset, 2, &config, &thermal_sample(at_hold), false);
        assert_eq!(
            changes,
            [HoldChange::Entered(HoldEntry::Thermal(thermal_level(
                at_hold
            )))],
            "{levels:?}: at the hold level"
        );
        assert_eq!(
            machine.hold(handset),
            Some(DeviceHold::Thermal),
            "{levels:?}"
        );

        // Held: a status below the hold level but not below the resume level keeps it.
        let (changes, _) =
            machine.on_sample(handset, 3, &config, &thermal_sample(below_hold), false);
        assert!(changes.is_empty(), "{levels:?}: between the levels");
        assert_eq!(
            machine.hold(handset),
            Some(DeviceHold::Thermal),
            "{levels:?}"
        );

        let (changes, _) =
            machine.on_sample(handset, 4, &config, &thermal_sample(below_resume), false);
        assert_eq!(
            changes,
            [HoldChange::Lifted(DeviceHold::Thermal)],
            "{levels:?}: below the resume level"
        );
        assert_eq!(machine.hold(handset), None, "{levels:?}");

        // The start gate reads the same rule: at the hold level the run does not start.
        let (changes, blocking) =
            machine.on_gate_power(handset, &config, thermal_level(at_hold), &charging());
        assert_eq!(
            changes,
            [HoldChange::Entered(HoldEntry::Thermal(thermal_level(
                at_hold
            )))]
        );
        assert_eq!(
            blocking,
            Some(HoldEntry::Thermal(thermal_level(at_hold))),
            "{levels:?}"
        );
    }
}

// T5 (§7.4): on no power source and below `battery_min_percent` the handset holds; plugged in it
// never does, charging or not (a battery-protection hold reads "not charging"). Two levels.
#[test]
fn rd5b_t5_a_low_battery_holds_only_on_no_power_source() {
    for minimum in [15_u8, 40] {
        let config = config(
            (ThermalLevel::Severe, ThermalLevel::Moderate),
            minimum,
            300_000,
            60_000,
        );
        let handset = instance();
        let mut machine = machine(handset, &config);
        let below = minimum - 1;
        for (plugged, status) in [
            (BatteryPlugged::Usb, BatteryStatus::NotCharging),
            (BatteryPlugged::Ac, BatteryStatus::Charging),
            (BatteryPlugged::Wireless, BatteryStatus::Charging),
        ] {
            let (changes, blocking) = machine.on_gate_power(
                handset,
                &config,
                ThermalLevel::None,
                &battery(plugged, status, below),
            );
            assert!(changes.is_empty(), "{minimum}: {plugged:?} {status:?}");
            assert_eq!(blocking, None, "{minimum}: {plugged:?} {status:?}");
        }
        let (changes, blocking) = machine.on_gate_power(
            handset,
            &config,
            ThermalLevel::None,
            &battery(
                BatteryPlugged::Unplugged,
                BatteryStatus::Discharging,
                minimum,
            ),
        );
        assert!(changes.is_empty(), "{minimum}: at the level, unplugged");
        assert_eq!(blocking, None);
        let (changes, blocking) = machine.on_gate_power(
            handset,
            &config,
            ThermalLevel::None,
            &battery(BatteryPlugged::Unplugged, BatteryStatus::Discharging, below),
        );
        assert_eq!(
            changes,
            [HoldChange::Entered(HoldEntry::BatteryLow(below))],
            "{minimum}"
        );
        assert_eq!(blocking, Some(HoldEntry::BatteryLow(below)));
        assert_eq!(machine.hold(handset), Some(DeviceHold::BatteryLow));
        // Plugged in again (still below the level): the hold lifts at the next sample.
        let (changes, _) = machine.on_sample(
            handset,
            1,
            &config,
            &Sample {
                battery: Some(battery(
                    BatteryPlugged::Usb,
                    BatteryStatus::NotCharging,
                    below,
                )),
                ..Sample::default()
            },
            false,
        );
        assert_eq!(changes, [HoldChange::Lifted(DeviceHold::BatteryLow)]);
        assert_eq!(machine.hold(handset), None);
    }
}

// T6 (§7.5): another application at a run's start holds the handset; the foreground is looked
// at again only once `in_use_backoff_ms` passed; another application then waits a further
// backoff, the instance's application, the home screen or nothing lifts the hold, and so does
// the gate turned off. Two backoffs.
#[test]
fn rd5b_t6_another_application_holds_until_a_look_after_the_backoff_finds_none() {
    for backoff in [300_000_u64, 30_000] {
        let config = config(
            (ThermalLevel::Severe, ThermalLevel::Moderate),
            15,
            backoff,
            60_000,
        );
        let handset = instance();
        let mut machine = machine(handset, &config);
        let changes = machine.on_gate_foreground(handset, 1_000, &config, Some(true));
        assert_eq!(changes, [HoldChange::Entered(HoldEntry::InUse)]);
        assert_eq!(machine.hold(handset), Some(DeviceHold::InUse));

        // Before the backoff ends a sample does not look at the foreground.
        let due = machine.due(1_000 + backoff - 1, true);
        assert_eq!(due, [(handset, ALIAS.to_owned(), false)], "{backoff}");
        // Once it ended, it does; another application waits a further backoff.
        let due = machine.due(1_000 + backoff, true);
        assert_eq!(due, [(handset, ALIAS.to_owned(), true)], "{backoff}");
        let (changes, _) = machine.on_sample(
            handset,
            1_000 + backoff,
            &config,
            &Sample {
                other_application: Some(true),
                ..Sample::default()
            },
            false,
        );
        assert!(changes.is_empty(), "{backoff}: still in use");
        assert_eq!(
            machine.due(1_000 + 2 * backoff - 1, true),
            [(handset, ALIAS.to_owned(), false)]
        );
        let (changes, _) = machine.on_sample(
            handset,
            1_000 + 2 * backoff,
            &config,
            &Sample {
                other_application: Some(false),
                ..Sample::default()
            },
            false,
        );
        assert_eq!(
            changes,
            [HoldChange::Lifted(DeviceHold::InUse)],
            "{backoff}"
        );
        assert_eq!(machine.hold(handset), None);

        // The instance's application, the home screen or nothing at the start: no hold.
        let changes = machine.on_gate_foreground(handset, 2, &config, Some(false));
        assert!(changes.is_empty());
        // The gate turned off lifts a hold at the next sample, without a look.
        machine.on_gate_foreground(handset, 3, &config, Some(true));
        let off = DeviceStateConfig {
            in_use_gate: false,
            ..config
        };
        let (changes, _) = machine.on_sample(handset, 4, &off, &Sample::default(), false);
        assert_eq!(changes, [HoldChange::Lifted(DeviceHold::InUse)]);
    }
}

// T3, the hold (§7.3): `device_locked` holds the handset; a sample whose keyguard still asks for
// a credential keeps it, one that finds no keyguard (a person unlocked it) lifts it.
#[test]
fn rd5b_t3_a_locked_handset_holds_until_a_sample_finds_no_keyguard() {
    let config = defaults();
    let handset = instance();
    let mut machine = machine(handset, &config);
    assert_eq!(
        machine.on_screen_check(handset, true),
        [HoldChange::Entered(HoldEntry::Locked)]
    );
    assert_eq!(machine.hold(handset), Some(DeviceHold::Locked));
    let keyguard = |keyguard| Sample {
        keyguard: Some(keyguard),
        ..Sample::default()
    };
    let (changes, _) =
        machine.on_sample(handset, 1, &config, &keyguard(KeyguardState::Secure), false);
    assert!(changes.is_empty());
    assert_eq!(machine.hold(handset), Some(DeviceHold::Locked));
    let (changes, _) =
        machine.on_sample(handset, 2, &config, &keyguard(KeyguardState::None), false);
    assert_eq!(changes, [HoldChange::Lifted(DeviceHold::Locked)]);
    assert_eq!(machine.hold(handset), None);
}

// T9 (§7.2): inside a run, an input whose time since the last interaction (the start gate, then
// each input) reaches the screen-off timeout less `keepalive_margin_ms` is preceded by the screen
// check; an earlier one is not, and an unknown timeout checks nothing. Two margins.
#[test]
fn rd5b_t9_an_input_late_by_the_timeout_less_the_margin_checks_the_screen_first() {
    const TIMEOUT_MS: u64 = 600_000;
    for margin in [60_000_u64, 120_000] {
        let config = config(
            (ThermalLevel::Severe, ThermalLevel::Moderate),
            15,
            300_000,
            margin,
        );
        let handset = instance();
        let mut machine = machine(handset, &config);
        machine.gate_passed(handset, 0, Some(TIMEOUT_MS));
        let threshold = TIMEOUT_MS - margin;
        assert!(
            !machine.keepalive_due(handset, threshold - 1, margin),
            "{margin}"
        );
        // The input above is the new last interaction.
        assert!(
            !machine.keepalive_due(handset, 2 * threshold - 2, margin),
            "{margin}"
        );
        assert!(
            machine.keepalive_due(handset, 3 * threshold - 2, margin),
            "{margin}: one threshold after the last input"
        );

        // A gate that could not read the timeout leaves the keepalive off.
        machine.gate_passed(handset, 0, None);
        assert!(!machine.keepalive_due(handset, 10 * TIMEOUT_MS, margin));
    }
}
