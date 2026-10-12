// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #398 RD-5a (model-398 v1.1 §7, §10.1 and §12): the device-state checks of a handset,
//! driven through a scripted shell.
//!
//! - Row RD-5a-T1: neutral answers read as typed values; an answer of unknown shape, and an adb
//!   failure, read as `device_state_unreadable` naming the read.
//! - The rows of the screen check that the wave-2 brief moves into RD-5a, device part only:
//!   RD-5b-T1 (wake), T2 (keyguard without a credential), T3 (keyguard with a credential) and
//!   T8 (command whitelist). Withdrawing availability, holds and sampling stay with RD-5b.
//!
//! Every answer is hand-written neutral text, the target is never named, and no process runs.
//! The settle wait is recorded, never slept.

use crate::{
    AdbCommandEvidence, AdbRecoveryText, AdbTransportState, BatteryPlugged, BatteryState,
    BatteryStatus, CommandOutput, DeviceError, DeviceResult, DeviceStateAction, DeviceStateRead,
    DeviceStateShell, InsecureKeyguardAction, KeyguardState, ScreenCheckPolicy, ScreenReady,
    ScreenState, ScreenTimeoutSettings, ThermalStatus, check_screen, read_battery, read_foreground,
    read_home, read_keyguard, read_screen, read_screen_timeout, read_thermal,
};
use actingcommand_contract::FencedWrite;
use actingcommand_contract::outcome::{Category, Outcome};
use serde_json::json;
use std::collections::VecDeque;
use std::time::Duration;

const POWER_AWAKE: &str = "POWER MANAGER (dumpsys power)\n\nPower Manager State:\n  mDirty=0x0\n  mWakefulness=Awake\n  mWakefulnessChanging=false\n  mIsPowered=true\n";
const POWER_ASLEEP: &str = "POWER MANAGER (dumpsys power)\n\nPower Manager State:\n  mDirty=0x0\n  mWakefulness=Asleep\n  mWakefulnessChanging=false\n  mIsPowered=true\n";
const POWER_DOZING: &str =
    "Power Manager State:\n  mWakefulness=Dozing\n  mWakefulnessChanging=false\n";
const POWER_DREAMING: &str =
    "Power Manager State:\n  mWakefulness=Dreaming\n  mWakefulnessChanging=false\n";

const KEYGUARD_NONE: &str = "WINDOW MANAGER POLICY STATE (dumpsys window policy)\n    mSystemReady=true mSystemBooted=true\n    KeyguardServiceDelegate\n      showing=false\n      inputRestricted=false\n      occluded=false\n      secure=true\n      dreaming=false\n      KeyguardStateMonitor\n        mIsShowing=false\n        mSimSecure=false\n";
const KEYGUARD_INSECURE: &str = "WINDOW MANAGER POLICY STATE (dumpsys window policy)\n    mSystemReady=true mSystemBooted=true\n    KeyguardServiceDelegate\n      showing=true\n      inputRestricted=false\n      occluded=false\n      secure=false\n      dreaming=false\n      KeyguardStateMonitor\n        mIsShowing=true\n        mSimSecure=false\n";
const KEYGUARD_SECURE: &str = "WINDOW MANAGER POLICY STATE (dumpsys window policy)\n    mSystemReady=true mSystemBooted=true\n    KeyguardServiceDelegate\n      showing=true\n      inputRestricted=true\n      occluded=false\n      secure=true\n      dreaming=false\n      KeyguardStateMonitor\n        mIsShowing=true\n        mSimSecure=false\n";

/// Plugged in over USB and not charging (a battery-protection hold), 80 %.
const BATTERY_USB_HELD: &str = "Current Battery Service state:\n  AC powered: false\n  USB powered: true\n  Wireless powered: false\n  Dock powered: false\n  Max charging current: 500000\n  status: 4\n  health: 2\n  present: true\n  level: 80\n  scale: 100\n  voltage: 4012\n  temperature: 312\n  technology: Li-ion\n";
/// Unplugged and discharging, 14 %.
const BATTERY_UNPLUGGED: &str = "Current Battery Service state:\n  AC powered: false\n  USB powered: false\n  Wireless powered: false\n  Dock powered: false\n  status: 3\n  health: 2\n  present: true\n  level: 14\n  scale: 100\n  voltage: 3650\n  temperature: 298\n  technology: Li-ion\n";
/// On a charger and charging, 57 %.
const BATTERY_AC_CHARGING: &str = "Current Battery Service state:\n  AC powered: true\n  USB powered: false\n  Wireless powered: false\n  Dock powered: false\n  status: 2\n  health: 2\n  present: true\n  level: 57\n  scale: 100\n  voltage: 3990\n  temperature: 305\n  technology: Li-ion\n";

const THERMAL_NONE: &str = "IsStatusOverride: false\nThermalEventListeners:\n\tcallbacks: 1\n\tkilled: false\nThermal Status: 0\nCached temperatures:\n\tTemperature{mValue=31.0, mType=3, mName=skin, mStatus=0}\nHAL Ready: true\n";
const THERMAL_SEVERE: &str = "IsStatusOverride: false\nThermalEventListeners:\n\tcallbacks: 1\n\tkilled: false\nThermal Status: 3\nCached temperatures:\n\tTemperature{mValue=44.0, mType=3, mName=skin, mStatus=3}\nHAL Ready: true\n";

const FOREGROUND_APP: &str = "  topResumedActivity=ActivityRecord{4c1d07 u0 com.example.app/.MainActivity t42}\nac_dumpsys_rc=0\n";
const FOREGROUND_NONE: &str = "ac_dumpsys_rc=0\n";

const HOME: &str = "priority=0 preferredOrder=0 match=0x108000 specificIndex=-1 isDefault=true\ncom.example.launcher/.LauncherActivity\n";

/// One thing the shell was asked to do, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sent {
    Read(DeviceStateRead),
    Act(DeviceStateAction),
    Settle(Duration),
}

/// A scripted handset: each read answers the next scripted answer for its command; an action
/// answers success unless a failure is scripted for it.
#[derive(Default)]
struct ScriptedShell {
    reads: Vec<(DeviceStateRead, VecDeque<DeviceResult<CommandOutput>>)>,
    failing_actions: Vec<(DeviceStateAction, DeviceError)>,
    sent: Vec<Sent>,
}

impl ScriptedShell {
    fn answering(mut self, command: DeviceStateRead, stdouts: &[&str]) -> Self {
        let answers = stdouts
            .iter()
            .map(|stdout| Ok(output(stdout, "")))
            .collect();
        self.reads.push((command, answers));
        self
    }

    fn failing(mut self, command: DeviceStateRead, error: DeviceError) -> Self {
        self.reads.push((command, VecDeque::from([Err(error)])));
        self
    }

    fn failing_action(mut self, command: DeviceStateAction, error: DeviceError) -> Self {
        self.failing_actions.push((command, error));
        self
    }
}

impl DeviceStateShell for ScriptedShell {
    fn read(&mut self, command: DeviceStateRead) -> DeviceResult<CommandOutput> {
        self.sent.push(Sent::Read(command));
        self.reads
            .iter_mut()
            .find(|(scripted, _)| *scripted == command)
            .and_then(|(_, answers)| answers.pop_front())
            .unwrap_or_else(|| panic!("no scripted answer left for {command:?}"))
    }

    fn act(
        &mut self,
        _witness: &FencedWrite,
        command: DeviceStateAction,
    ) -> DeviceResult<CommandOutput> {
        self.sent.push(Sent::Act(command));
        match self
            .failing_actions
            .iter()
            .position(|(scripted, _)| *scripted == command)
        {
            Some(index) => Err(self.failing_actions.remove(index).1),
            None => Ok(output("", "")),
        }
    }

    fn settle(&mut self, wait: Duration) {
        self.sent.push(Sent::Settle(wait));
    }
}

fn output(stdout: &str, stderr: &str) -> CommandOutput {
    CommandOutput {
        stdout: stdout.to_owned(),
        stderr: stderr.to_owned(),
        stdout_lossy_decode: false,
        stderr_lossy_decode: false,
    }
}

/// adb ran and exited 1, printing `stderr`.
fn adb_exit_failure(stderr: &str) -> DeviceError {
    DeviceError::fatal("adb shell failed with exit code 1").with_adb_command(AdbCommandEvidence {
        succeeded: false,
        exit_code: Some(1),
        stdout: AdbRecoveryText {
            text: String::new(),
            truncated: false,
        },
        stderr: AdbRecoveryText {
            text: stderr.to_owned(),
            truncated: false,
        },
        stdout_lossy_decode: false,
        stderr_lossy_decode: false,
        state: AdbTransportState::Unknown,
    })
}

/// A scheduler-shaped step witness for driving the device-effect commands directly.
fn witness() -> FencedWrite {
    let issuer = actingcommand_contract::IdentifierIssuer::new().expect("ids");
    actingcommand_contract::issue_fenced_write(
        actingcommand_contract::LeaseToken::new(
            *issuer.mint_owner_epoch().expect("epoch").transport(),
            *issuer.mint_lease_id().expect("lease").transport(),
            *issuer.mint_instance_id().expect("instance").transport(),
            *issuer.mint_holder_id().expect("holder").transport(),
            100,
        )
        .expect("test step token"),
        1,
        std::num::NonZeroU64::new(1).expect("step"),
        actingcommand_contract::FencedWritePurpose::Business,
    )
}

fn policy(wake_settle: Duration, keyguard_insecure: InsecureKeyguardAction) -> ScreenCheckPolicy {
    ScreenCheckPolicy {
        wake_settle,
        keyguard_insecure,
    }
}

/// Two configured settle times: the recorded waits must follow the configuration.
const SETTLES: [Duration; 2] = [Duration::from_millis(1000), Duration::from_millis(2500)];

/// `outcome` is `device_state_unreadable` (error) naming `probe`, caused by `foreign_adb_error`
/// from adb with `raw_text`.
fn assert_unreadable(outcome: &Outcome, probe: &str, raw_text: &str) {
    assert_eq!(outcome.code().as_str(), "device_state_unreadable");
    assert_eq!(outcome.category(), Category::Error);
    assert_eq!(outcome.value("device_state_probe"), Some(&json!(probe)));
    let [link] = outcome.causes() else {
        panic!("one cause expected: {outcome}");
    };
    assert_eq!(link.relation().as_str(), "caused_by");
    let foreign = link.outcome();
    assert_eq!(foreign.code().as_str(), "foreign_adb_error");
    assert_eq!(foreign.value("raw_source"), Some(&json!("adb")));
    assert_eq!(foreign.value("raw_text"), Some(&json!(raw_text)));
}

/// RD-5a-T1, typed values: neutral answers of `dumpsys power`, the keyguard section,
/// `dumpsys battery`, `dumpsys thermalservice`, the foreground query, the home activity and the
/// two `settings get` reads.
#[test]
fn rd5a_t1_neutral_answers_read_as_typed_values() {
    for (answer, expected) in [
        (POWER_AWAKE, ScreenState::Awake),
        (POWER_ASLEEP, ScreenState::Asleep),
        (POWER_DOZING, ScreenState::Dozing),
        (POWER_DREAMING, ScreenState::Dreaming),
    ] {
        let mut shell = ScriptedShell::default().answering(DeviceStateRead::Power, &[answer]);
        assert_eq!(read_screen(&mut shell), Ok(expected), "{answer}");
    }
    for (answer, expected) in [
        (KEYGUARD_NONE, KeyguardState::None),
        (KEYGUARD_INSECURE, KeyguardState::Insecure),
        (KEYGUARD_SECURE, KeyguardState::Secure),
    ] {
        let mut shell = ScriptedShell::default().answering(DeviceStateRead::Keyguard, &[answer]);
        assert_eq!(read_keyguard(&mut shell), Ok(expected), "{answer}");
    }
    for (answer, expected) in [
        (
            BATTERY_USB_HELD,
            BatteryState {
                level_percent: 80,
                status: BatteryStatus::NotCharging,
                plugged: BatteryPlugged::Usb,
                temperature_deci_c: 312,
            },
        ),
        (
            BATTERY_UNPLUGGED,
            BatteryState {
                level_percent: 14,
                status: BatteryStatus::Discharging,
                plugged: BatteryPlugged::Unplugged,
                temperature_deci_c: 298,
            },
        ),
        (
            BATTERY_AC_CHARGING,
            BatteryState {
                level_percent: 57,
                status: BatteryStatus::Charging,
                plugged: BatteryPlugged::Ac,
                temperature_deci_c: 305,
            },
        ),
    ] {
        let mut shell = ScriptedShell::default().answering(DeviceStateRead::Battery, &[answer]);
        assert_eq!(read_battery(&mut shell), Ok(expected), "{answer}");
    }
    for (answer, expected) in [
        (THERMAL_NONE, ThermalStatus::None),
        (THERMAL_SEVERE, ThermalStatus::Severe),
    ] {
        let mut shell = ScriptedShell::default().answering(DeviceStateRead::Thermal, &[answer]);
        assert_eq!(read_thermal(&mut shell), Ok(expected), "{answer}");
    }
    for (answer, expected) in [
        (FOREGROUND_APP, Some("com.example.app".to_owned())),
        (FOREGROUND_NONE, None),
    ] {
        let mut shell = ScriptedShell::default().answering(DeviceStateRead::Foreground, &[answer]);
        assert_eq!(read_foreground(&mut shell), Ok(expected), "{answer}");
    }
    let mut shell = ScriptedShell::default().answering(DeviceStateRead::Home, &[HOME]);
    assert_eq!(read_home(&mut shell), Ok("com.example.launcher".to_owned()));
    let mut shell = ScriptedShell::default()
        .answering(DeviceStateRead::ScreenOffTimeout, &["600000\n"])
        .answering(DeviceStateRead::StayOnWhilePluggedIn, &["0\n"]);
    assert_eq!(
        read_screen_timeout(&mut shell),
        Ok(ScreenTimeoutSettings {
            screen_off_timeout_ms: 600_000,
            stay_on_while_plugged_in: 0,
        })
    );
}

/// RD-5a-T1, unknown shapes: an answer the parser does not know is `device_state_unreadable`
/// with the read's `device_state_probe` and the answer as adb's foreign text; nothing is guessed.
/// A failed adb command is the same code, with adb's output and exit code as the cause.
#[test]
fn rd5a_t1_unknown_answers_and_adb_failures_are_device_state_unreadable() {
    const POWER_WITHOUT_WAKEFULNESS: &str =
        "Power Manager State:\n  mDirty=0x0\n  mIsPowered=true\n";
    const POLICY_WITHOUT_KEYGUARD: &str =
        "WINDOW MANAGER POLICY STATE (dumpsys window policy)\n    mSystemReady=true\n";
    const BATTERY_WITHOUT_LEVEL: &str = "Current Battery Service state:\n  AC powered: false\n  USB powered: true\n  Wireless powered: false\n  status: 2\n  temperature: 300\n";
    const THERMAL_SERVICE_MISSING: &str = "Can't find service: thermalservice\n";
    const FOREGROUND_WITHOUT_STATUS: &str =
        "  topResumedActivity=ActivityRecord{4c1d07 u0 com.example.app/.MainActivity t42}\n";
    const HOME_UNRESOLVED: &str = "No activity found\n";
    const SETTING_UNSET: &str = "null\n";

    let mut shell =
        ScriptedShell::default().answering(DeviceStateRead::Power, &[POWER_WITHOUT_WAKEFULNESS]);
    let outcome = read_screen(&mut shell).expect_err("no wakefulness");
    assert_unreadable(&outcome, "power", POWER_WITHOUT_WAKEFULNESS);

    let mut shell =
        ScriptedShell::default().answering(DeviceStateRead::Keyguard, &[POLICY_WITHOUT_KEYGUARD]);
    let outcome = read_keyguard(&mut shell).expect_err("no keyguard section");
    assert_unreadable(&outcome, "keyguard", POLICY_WITHOUT_KEYGUARD);

    let mut shell =
        ScriptedShell::default().answering(DeviceStateRead::Battery, &[BATTERY_WITHOUT_LEVEL]);
    let outcome = read_battery(&mut shell).expect_err("no level");
    assert_unreadable(&outcome, "battery", BATTERY_WITHOUT_LEVEL);

    let mut shell =
        ScriptedShell::default().answering(DeviceStateRead::Thermal, &[THERMAL_SERVICE_MISSING]);
    let outcome = read_thermal(&mut shell).expect_err("no thermal status");
    assert_unreadable(&outcome, "thermal", THERMAL_SERVICE_MISSING);

    let mut shell = ScriptedShell::default()
        .answering(DeviceStateRead::Foreground, &[FOREGROUND_WITHOUT_STATUS]);
    let outcome = read_foreground(&mut shell).expect_err("no dumpsys status");
    assert_unreadable(&outcome, "foreground", FOREGROUND_WITHOUT_STATUS);

    let mut shell = ScriptedShell::default().answering(DeviceStateRead::Home, &[HOME_UNRESOLVED]);
    let outcome = read_home(&mut shell).expect_err("no home activity");
    assert_unreadable(&outcome, "home", HOME_UNRESOLVED);

    let mut shell =
        ScriptedShell::default().answering(DeviceStateRead::ScreenOffTimeout, &[SETTING_UNSET]);
    let outcome = read_screen_timeout(&mut shell).expect_err("setting unset");
    assert_unreadable(&outcome, "settings", SETTING_UNSET);

    let mut shell = ScriptedShell::default().failing(
        DeviceStateRead::Power,
        adb_exit_failure("error: device offline\n"),
    );
    let outcome = read_screen(&mut shell).expect_err("adb failed");
    assert_unreadable(&outcome, "power", "error: device offline\n");
    assert_eq!(
        outcome.causes()[0].outcome().value("exit_code"),
        Some(&json!(1))
    );
}

/// RD-5b-T1, device part: a screen that is not awake gets exactly one `KEYCODE_WAKEUP`, then the
/// configured settle time, then a second read. Awake: the check goes on. Still dark:
/// `device_wake_failed` and nothing more is sent.
#[test]
fn rd5b_t1_a_dark_screen_gets_exactly_one_wake_key() {
    for settle in SETTLES {
        let mut shell = ScriptedShell::default()
            .answering(DeviceStateRead::Power, &[POWER_ASLEEP, POWER_AWAKE])
            .answering(DeviceStateRead::Keyguard, &[KEYGUARD_NONE]);
        let ready = check_screen(
            &mut shell,
            &witness(),
            policy(settle, InsecureKeyguardAction::Dismiss),
        );
        assert_eq!(
            ready,
            Ok(ScreenReady {
                woken: true,
                keyguard_dismissed: false,
            })
        );
        assert_eq!(
            shell.sent,
            [
                Sent::Read(DeviceStateRead::Power),
                Sent::Act(DeviceStateAction::Wake),
                Sent::Settle(settle),
                Sent::Read(DeviceStateRead::Power),
                Sent::Read(DeviceStateRead::Keyguard),
            ]
        );

        let mut shell = ScriptedShell::default()
            .answering(DeviceStateRead::Power, &[POWER_ASLEEP, POWER_ASLEEP]);
        let failure = check_screen(
            &mut shell,
            &witness(),
            policy(settle, InsecureKeyguardAction::Dismiss),
        )
        .expect_err("the screen stays dark");
        assert_eq!(failure.code().as_str(), "device_wake_failed");
        assert_eq!(failure.category(), Category::Error);
        assert_eq!(
            shell.sent,
            [
                Sent::Read(DeviceStateRead::Power),
                Sent::Act(DeviceStateAction::Wake),
                Sent::Settle(settle),
                Sent::Read(DeviceStateRead::Power),
            ]
        );
    }
}

/// RD-5b-T2, device part: a keyguard without a credential gets exactly one
/// `wm dismiss-keyguard` under `dismiss` and the check goes on once none shows; under `wait`
/// nothing is sent and the check does not go on; a keyguard still showing after the dismissal
/// is `device_keyguard_dismiss_failed`.
#[test]
fn rd5b_t2_an_insecure_keyguard_is_dismissed_once_or_left_waiting() {
    for settle in SETTLES {
        let mut shell = ScriptedShell::default()
            .answering(DeviceStateRead::Power, &[POWER_AWAKE])
            .answering(
                DeviceStateRead::Keyguard,
                &[KEYGUARD_INSECURE, KEYGUARD_NONE],
            );
        let ready = check_screen(
            &mut shell,
            &witness(),
            policy(settle, InsecureKeyguardAction::Dismiss),
        );
        assert_eq!(
            ready,
            Ok(ScreenReady {
                woken: false,
                keyguard_dismissed: true,
            })
        );
        assert_eq!(
            shell.sent,
            [
                Sent::Read(DeviceStateRead::Power),
                Sent::Read(DeviceStateRead::Keyguard),
                Sent::Act(DeviceStateAction::DismissKeyguard),
                Sent::Settle(settle),
                Sent::Read(DeviceStateRead::Keyguard),
            ]
        );

        let mut shell = ScriptedShell::default()
            .answering(DeviceStateRead::Power, &[POWER_AWAKE])
            .answering(DeviceStateRead::Keyguard, &[KEYGUARD_INSECURE]);
        let waiting = check_screen(
            &mut shell,
            &witness(),
            policy(settle, InsecureKeyguardAction::Wait),
        );
        assert!(waiting.is_err(), "{waiting:?}");
        assert_eq!(
            shell.sent,
            [
                Sent::Read(DeviceStateRead::Power),
                Sent::Read(DeviceStateRead::Keyguard),
            ]
        );

        let mut shell = ScriptedShell::default()
            .answering(DeviceStateRead::Power, &[POWER_AWAKE])
            .answering(
                DeviceStateRead::Keyguard,
                &[KEYGUARD_INSECURE, KEYGUARD_INSECURE],
            );
        let failure = check_screen(
            &mut shell,
            &witness(),
            policy(settle, InsecureKeyguardAction::Dismiss),
        )
        .expect_err("the keyguard stays");
        assert_eq!(failure.code().as_str(), "device_keyguard_dismiss_failed");
        assert_eq!(failure.category(), Category::Error);
        assert_eq!(
            shell.sent,
            [
                Sent::Read(DeviceStateRead::Power),
                Sent::Read(DeviceStateRead::Keyguard),
                Sent::Act(DeviceStateAction::DismissKeyguard),
                Sent::Settle(settle),
                Sent::Read(DeviceStateRead::Keyguard),
            ]
        );
    }

    let mut shell = ScriptedShell::default()
        .answering(DeviceStateRead::Power, &[POWER_AWAKE])
        .answering(DeviceStateRead::Keyguard, &[KEYGUARD_INSECURE])
        .failing_action(
            DeviceStateAction::DismissKeyguard,
            adb_exit_failure("error: closed\n"),
        );
    let failure = check_screen(
        &mut shell,
        &witness(),
        policy(SETTLES[0], InsecureKeyguardAction::Dismiss),
    )
    .expect_err("the dismissal failed");
    assert_eq!(failure.code().as_str(), "device_keyguard_dismiss_failed");
    assert_eq!(
        failure.causes()[0].outcome().code().as_str(),
        "foreign_adb_error"
    );
}

/// RD-5b-T3, device part: a keyguard with a credential is `device_locked` (warning) and the
/// check sends nothing to it, neither a dismissal nor any input; a dark screen still gets its one
/// wake key first.
#[test]
fn rd5b_t3_a_secure_keyguard_is_device_locked_and_gets_no_command() {
    for insecure in [
        InsecureKeyguardAction::Dismiss,
        InsecureKeyguardAction::Wait,
    ] {
        let mut shell = ScriptedShell::default()
            .answering(DeviceStateRead::Power, &[POWER_AWAKE])
            .answering(DeviceStateRead::Keyguard, &[KEYGUARD_SECURE]);
        let locked = check_screen(&mut shell, &witness(), policy(SETTLES[0], insecure))
            .expect_err("a person unlocks");
        assert_eq!(locked.code().as_str(), "device_locked");
        assert_eq!(locked.category(), Category::Warning);
        assert_eq!(
            shell.sent,
            [
                Sent::Read(DeviceStateRead::Power),
                Sent::Read(DeviceStateRead::Keyguard),
            ]
        );
    }

    let mut shell = ScriptedShell::default()
        .answering(DeviceStateRead::Power, &[POWER_ASLEEP, POWER_AWAKE])
        .answering(DeviceStateRead::Keyguard, &[KEYGUARD_SECURE]);
    let locked = check_screen(
        &mut shell,
        &witness(),
        policy(SETTLES[0], InsecureKeyguardAction::Dismiss),
    )
    .expect_err("a person unlocks");
    assert_eq!(locked.code().as_str(), "device_locked");
    assert_eq!(
        shell.sent,
        [
            Sent::Read(DeviceStateRead::Power),
            Sent::Act(DeviceStateAction::Wake),
            Sent::Settle(SETTLES[0]),
            Sent::Read(DeviceStateRead::Power),
            Sent::Read(DeviceStateRead::Keyguard),
        ]
    );
}

/// RD-5b-T8, device part: every command the checks can send (both closed sets) is a form of the
/// model's whitelist (§7.7), and none is a forbidden one: no `settings put`, `svc`, `wm size`
/// with a size, `wm density`, `reboot`, `kill-server`, `tcpip`, `usb` or `KEYCODE_POWER`.
#[test]
fn rd5b_t8_every_command_the_checks_can_send_is_on_the_whitelist() {
    const WHITELIST: &[&[&str]] = &[
        &["dumpsys", "power"],
        &["dumpsys", "window"],
        &["dumpsys", "battery"],
        &["dumpsys", "thermalservice"],
        &["settings", "get"],
        &["cmd", "package", "resolve-activity"],
        &["input", "keyevent", "KEYCODE_WAKEUP"],
        &["wm", "dismiss-keyguard"],
    ];
    const FORBIDDEN_WORDS: &[&str] = &[
        "svc",
        "reboot",
        "kill-server",
        "tcpip",
        "usb",
        "KEYCODE_POWER",
    ];
    let mut commands = DeviceStateRead::ALL
        .map(DeviceStateRead::shell_args)
        .to_vec();
    commands.extend(DeviceStateAction::ALL.map(DeviceStateAction::shell_args));
    for args in commands {
        let words = args
            .iter()
            .flat_map(|arg| arg.split_whitespace())
            .collect::<Vec<_>>();
        let listed = WHITELIST.iter().any(|form| words.starts_with(form))
            // The foreground query runs `dumpsys activity activities` filtered on the device.
            || words.starts_with(&["{", "dumpsys", "activity", "activities;"]);
        assert!(listed, "{args:?} is not on the whitelist");
        for word in &words {
            assert!(!FORBIDDEN_WORDS.contains(word), "{args:?} sends {word}");
        }
        for (index, pair) in words.windows(2).enumerate() {
            assert_ne!(pair, ["settings", "put"], "{args:?}");
            assert_ne!(pair, ["wm", "density"], "{args:?}");
            // `wm size` reads; `wm size <W>x<H>` would change the display.
            let sets_size = pair == ["wm", "size"] && words.len() > index + 2;
            assert!(!sets_size, "{args:?}");
        }
    }
}
