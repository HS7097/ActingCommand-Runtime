// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #398 RD-5a (model-398 v1.1 §7): the device-state checks of a handset, a real phone
//! on USB.
//!
//! - **Reads.** Every read is one read-only adb shell command of the closed set
//!   [`DeviceStateRead`]: power, the keyguard section of the window policy, battery, thermal
//!   service, the foreground query, the home activity, and two screen-timeout settings read with
//!   `settings get`. Each answer is parsed into a typed value. An answer the parser does not know
//!   is `device_state_unreadable` naming the read (`device_state_probe`); nothing is guessed
//!   (§7.1).
//! - **Screen check.** [`check_screen`] sends at most the two device-effect commands of the
//!   closed set [`DeviceStateAction`]: `KEYCODE_WAKEUP` when the screen is not awake, and
//!   `wm dismiss-keyguard` for a keyguard without a credential when the policy says `dismiss`. A
//!   keyguard with a credential is `device_locked`: a person unlocks the phone (§7.3).
//! - **Never.** Nothing here changes a phone setting, presses the power key or enters a
//!   credential. [`DeviceStateShell`] takes only the two closed sets, so no other command can
//!   pass through it (§7.7).
//!
//! The thresholds and holds that act on these values (battery, thermal, foreground) and the
//! sampling belong to the host (RD-5b); the configuration values arrive as arguments, and this
//! module holds no default of its own.

use crate::adb::{FOREGROUND_QUERY, FOREGROUND_STATUS};
use crate::codes::DeviceStateCode;
use crate::{Adb, CommandOutput, DeviceError, DeviceResult, parse_foreground_package};
use actingcommand_contract::FencedWrite;
use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::{self, Outcome, RawSource};
use std::time::Duration;

actingcommand_contract::outcome_vocabulary! {
    /// Which device-state read failed or answered a shape the parser does not know (vocabulary
    /// `device_state_probe`, model-398 §10.1).
    pub enum DeviceStateProbe: "device_state_probe" {
        Power => "power",
        Keyguard => "keyguard",
        Battery => "battery",
        Thermal => "thermal",
        Foreground => "foreground",
        Home => "home",
        Settings => "settings",
    }
}

/// The read-only commands of the device-state checks (model-398 §7.1). Each runs as
/// `adb -s <target> shell <args>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceStateRead {
    /// `dumpsys power`: the wakefulness.
    Power,
    /// `dumpsys window policy`: the window manager's policy section, which holds the keyguard
    /// state.
    Keyguard,
    /// `dumpsys battery`.
    Battery,
    /// `dumpsys thermalservice`: the thermal status.
    Thermal,
    /// The foreground query of [`Adb::foreground_package`] (`dumpsys activity activities`,
    /// filtered on the device).
    Foreground,
    /// `cmd package resolve-activity` for the home intent: the launcher's package.
    Home,
    /// `settings get system screen_off_timeout`.
    ScreenOffTimeout,
    /// `settings get global stay_on_while_plugged_in`.
    StayOnWhilePluggedIn,
}

impl DeviceStateRead {
    /// Every read, in declaration order.
    pub const ALL: [Self; 8] = [
        Self::Power,
        Self::Keyguard,
        Self::Battery,
        Self::Thermal,
        Self::Foreground,
        Self::Home,
        Self::ScreenOffTimeout,
        Self::StayOnWhilePluggedIn,
    ];

    /// The arguments after `adb -s <target> shell`.
    pub fn shell_args(self) -> &'static [&'static str] {
        match self {
            Self::Power => &["dumpsys", "power"],
            Self::Keyguard => &["dumpsys", "window", "policy"],
            Self::Battery => &["dumpsys", "battery"],
            Self::Thermal => &["dumpsys", "thermalservice"],
            Self::Foreground => &[FOREGROUND_QUERY],
            Self::Home => &[
                "cmd",
                "package",
                "resolve-activity",
                "--brief",
                "-a",
                "android.intent.action.MAIN",
                "-c",
                "android.intent.category.HOME",
            ],
            Self::ScreenOffTimeout => &["settings", "get", "system", "screen_off_timeout"],
            Self::StayOnWhilePluggedIn => {
                &["settings", "get", "global", "stay_on_while_plugged_in"]
            }
        }
    }

    /// The read a failure names.
    pub fn probe(self) -> DeviceStateProbe {
        match self {
            Self::Power => DeviceStateProbe::Power,
            Self::Keyguard => DeviceStateProbe::Keyguard,
            Self::Battery => DeviceStateProbe::Battery,
            Self::Thermal => DeviceStateProbe::Thermal,
            Self::Foreground => DeviceStateProbe::Foreground,
            Self::Home => DeviceStateProbe::Home,
            Self::ScreenOffTimeout | Self::StayOnWhilePluggedIn => DeviceStateProbe::Settings,
        }
    }
}

/// The device-effect commands of the screen check (model-398 §7.3). Each runs as
/// `adb -s <target> shell <args>` under the step's write witness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceStateAction {
    /// `input keyevent KEYCODE_WAKEUP`: turns the screen on and, unlike the power key, never off.
    Wake,
    /// `wm dismiss-keyguard`: dismisses a keyguard that asks for no credential.
    DismissKeyguard,
}

impl DeviceStateAction {
    /// Every action, in declaration order.
    pub const ALL: [Self; 2] = [Self::Wake, Self::DismissKeyguard];

    /// The arguments after `adb -s <target> shell`.
    pub fn shell_args(self) -> &'static [&'static str] {
        match self {
            Self::Wake => &["input", "keyevent", "KEYCODE_WAKEUP"],
            Self::DismissKeyguard => &["wm", "dismiss-keyguard"],
        }
    }
}

/// How the checks reach a handset. It takes only the closed sets [`DeviceStateRead`] and
/// [`DeviceStateAction`], so no other command can pass through it (model-398 §7.7).
pub trait DeviceStateShell {
    /// Runs one read-only command.
    fn read(&mut self, command: DeviceStateRead) -> DeviceResult<CommandOutput>;

    /// Runs one device-effect command under the step's write witness.
    fn act(
        &mut self,
        witness: &FencedWrite,
        command: DeviceStateAction,
    ) -> DeviceResult<CommandOutput>;

    /// Waits after an action before the state is read again.
    fn settle(&mut self, wait: Duration);
}

/// The production [`DeviceStateShell`]: `adb -s <target> shell <args>` through the installation's
/// adb. It has no `Debug`, so the target never reaches a log.
pub struct AdbDeviceStateShell<'a> {
    adb: &'a Adb,
    target: &'a str,
}

impl<'a> AdbDeviceStateShell<'a> {
    /// `target` is the adb `-s` argument, for the command line only.
    pub fn new(adb: &'a Adb, target: &'a str) -> Self {
        Self { adb, target }
    }

    fn command_line(&self, shell_args: &[&'a str]) -> Vec<&'a str> {
        let mut args = vec!["-s", self.target, "shell"];
        args.extend_from_slice(shell_args);
        args
    }
}

impl DeviceStateShell for AdbDeviceStateShell<'_> {
    fn read(&mut self, command: DeviceStateRead) -> DeviceResult<CommandOutput> {
        self.adb.run(&self.command_line(command.shell_args()))
    }

    fn act(
        &mut self,
        witness: &FencedWrite,
        command: DeviceStateAction,
    ) -> DeviceResult<CommandOutput> {
        self.adb
            .run_write(witness, &self.command_line(command.shell_args()))
    }

    fn settle(&mut self, wait: Duration) {
        std::thread::sleep(wait);
    }
}

/// The screen's wakefulness, from `mWakefulness=` of `dumpsys power`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenState {
    Awake,
    Asleep,
    Dozing,
    Dreaming,
}

/// Whether a keyguard shows and whether it asks for a credential, from the
/// `KeyguardServiceDelegate` section of `dumpsys window policy` (`showing=`, `secure=`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyguardState {
    /// No keyguard shows.
    None,
    /// A keyguard without a credential shows, for example swipe.
    Insecure,
    /// A keyguard that asks for a PIN, pattern, password or biometric shows.
    Secure,
}

/// `status:` of `dumpsys battery` (Android's `BATTERY_STATUS_*`, 1–5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatteryStatus {
    Unknown,
    Charging,
    Discharging,
    NotCharging,
    Full,
}

/// Which power source is plugged in, from the `* powered:` lines of `dumpsys battery`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatteryPlugged {
    Unplugged,
    Ac,
    Usb,
    Wireless,
    Dock,
}

/// One reading of `dumpsys battery`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatteryState {
    /// `level:`, 0–100.
    pub level_percent: u8,
    pub status: BatteryStatus,
    pub plugged: BatteryPlugged,
    /// `temperature:`, in tenths of a degree Celsius.
    pub temperature_deci_c: i32,
}

/// `Thermal Status:` of `dumpsys thermalservice` (Android's `THERMAL_STATUS_*`, 0–6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThermalStatus {
    None,
    Light,
    Moderate,
    Severe,
    Critical,
    Emergency,
    Shutdown,
}

/// The two screen-timeout settings, read with `settings get` only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenTimeoutSettings {
    /// `system screen_off_timeout`, in milliseconds.
    pub screen_off_timeout_ms: u64,
    /// `global stay_on_while_plugged_in`, Android's bit mask of power sources.
    pub stay_on_while_plugged_in: u32,
}

/// The wakefulness (`dumpsys power`).
pub fn read_screen(shell: &mut dyn DeviceStateShell) -> Result<ScreenState, Outcome> {
    read_parsed(shell, DeviceStateRead::Power, parse_power)
}

/// The keyguard (`dumpsys window policy`).
pub fn read_keyguard(shell: &mut dyn DeviceStateShell) -> Result<KeyguardState, Outcome> {
    read_parsed(shell, DeviceStateRead::Keyguard, parse_keyguard)
}

/// The battery (`dumpsys battery`).
pub fn read_battery(shell: &mut dyn DeviceStateShell) -> Result<BatteryState, Outcome> {
    read_parsed(shell, DeviceStateRead::Battery, parse_battery)
}

/// The thermal status (`dumpsys thermalservice`).
pub fn read_thermal(shell: &mut dyn DeviceStateShell) -> Result<ThermalStatus, Outcome> {
    read_parsed(shell, DeviceStateRead::Thermal, parse_thermal)
}

/// The package of the resumed activity; `None` when the query ran and no resumed activity was
/// reported (for example mid-transition).
pub fn read_foreground(shell: &mut dyn DeviceStateShell) -> Result<Option<String>, Outcome> {
    read_parsed(shell, DeviceStateRead::Foreground, parse_foreground)
}

/// The package of the activity the home intent resolves to. No launcher package is written in.
pub fn read_home(shell: &mut dyn DeviceStateShell) -> Result<String, Outcome> {
    read_parsed(shell, DeviceStateRead::Home, parse_home)
}

/// The two screen-timeout settings.
pub fn read_screen_timeout(
    shell: &mut dyn DeviceStateShell,
) -> Result<ScreenTimeoutSettings, Outcome> {
    Ok(ScreenTimeoutSettings {
        screen_off_timeout_ms: read_parsed(shell, DeviceStateRead::ScreenOffTimeout, |text| {
            text.trim().parse::<u64>().ok()
        })?,
        stay_on_while_plugged_in: read_parsed(
            shell,
            DeviceStateRead::StayOnWhilePluggedIn,
            |text| text.trim().parse::<u32>().ok(),
        )?,
    })
}

/// What the screen check does with a keyguard that asks for no credential
/// (`device_state.keyguard_insecure`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsecureKeyguardAction {
    /// Run `wm dismiss-keyguard` once.
    Dismiss,
    /// Send nothing and leave it to a person, like a keyguard with a credential.
    Wait,
}

/// The configured values the screen check takes (`device_state.*`, model-398 §7.8). The device
/// layer holds no default of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenCheckPolicy {
    /// `device_state.wake_settle_ms`: the wait after the wake key, and after a keyguard
    /// dismissal, before the state is read again.
    pub wake_settle: Duration,
    /// `device_state.keyguard_insecure`.
    pub keyguard_insecure: InsecureKeyguardAction,
}

/// A screen the check found awake with no keyguard showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenReady {
    /// The check sent the wake key.
    pub woken: bool,
    /// The check dismissed a keyguard without a credential.
    pub keyguard_dismissed: bool,
}

/// Model-398 §7.3: brings the screen to awake with no keyguard showing, or says why it cannot.
///
/// - A screen that is not awake gets exactly one `KEYCODE_WAKEUP`; after `wake_settle` it must
///   read awake, or the check ends with `device_wake_failed`.
/// - A keyguard with a credential ends the check with `device_locked`: nothing is sent, and a
///   person unlocks the phone.
/// - A keyguard without a credential gets exactly one `wm dismiss-keyguard` under `dismiss`; after
///   `wake_settle` no keyguard may show, or the check ends with
///   `device_keyguard_dismiss_failed`. Under `wait` nothing is sent and the check ends with
///   `device_locked`.
/// - A read that fails or answers an unknown shape ends the check with `device_state_unreadable`.
///
/// Every command goes through `shell`, so the check sends no input of any other kind.
pub fn check_screen(
    shell: &mut dyn DeviceStateShell,
    witness: &FencedWrite,
    policy: ScreenCheckPolicy,
) -> Result<ScreenReady, Outcome> {
    let mut woken = false;
    if read_screen(shell)? != ScreenState::Awake {
        act(
            shell,
            witness,
            DeviceStateAction::Wake,
            DeviceStateCode::DeviceWakeFailed,
        )?;
        shell.settle(policy.wake_settle);
        if read_screen(shell)? != ScreenState::Awake {
            return Err(Outcome::new(DeviceStateCode::DeviceWakeFailed));
        }
        woken = true;
    }
    let keyguard_dismissed = match read_keyguard(shell)? {
        KeyguardState::None => false,
        KeyguardState::Secure => return Err(Outcome::new(DeviceStateCode::DeviceLocked)),
        KeyguardState::Insecure => match policy.keyguard_insecure {
            InsecureKeyguardAction::Wait => {
                return Err(Outcome::new(DeviceStateCode::DeviceLocked));
            }
            InsecureKeyguardAction::Dismiss => {
                act(
                    shell,
                    witness,
                    DeviceStateAction::DismissKeyguard,
                    DeviceStateCode::DeviceKeyguardDismissFailed,
                )?;
                shell.settle(policy.wake_settle);
                match read_keyguard(shell)? {
                    KeyguardState::None => true,
                    KeyguardState::Secure => {
                        return Err(Outcome::new(DeviceStateCode::DeviceLocked));
                    }
                    KeyguardState::Insecure => {
                        return Err(Outcome::new(DeviceStateCode::DeviceKeyguardDismissFailed));
                    }
                }
            }
        },
    };
    Ok(ScreenReady {
        woken,
        keyguard_dismissed,
    })
}

/// Runs `command`; when adb fails, the check ends with `failed` and the adb failure as its cause.
fn act(
    shell: &mut dyn DeviceStateShell,
    witness: &FencedWrite,
    command: DeviceStateAction,
    failed: DeviceStateCode,
) -> Result<(), Outcome> {
    shell
        .act(witness, command)
        .map(drop)
        .map_err(|error| with_adb_failure(Outcome::new(failed), &error))
}

fn read_parsed<T>(
    shell: &mut dyn DeviceStateShell,
    command: DeviceStateRead,
    parse: impl FnOnce(&str) -> Option<T>,
) -> Result<T, Outcome> {
    let unreadable =
        || Outcome::new(DeviceStateCode::DeviceStateUnreadable).device_state_probe(command.probe());
    let output = shell
        .read(command)
        .map_err(|error| with_adb_failure(unreadable(), &error))?;
    parse(output.stdout.as_str()).ok_or_else(|| {
        // The answer adb printed is the foreign text the parser could not read.
        let answer = if output.stdout.trim().is_empty() {
            &output.stderr
        } else {
            &output.stdout
        };
        unreadable().caused_by(
            Outcome::new(ContractCode::ForeignAdbError)
                .raw_source(RawSource::Adb)
                .raw_text(answer),
        )
    })
}

/// `top` caused by adb's failure: adb's own output and exit code when adb answered, otherwise
/// (adb could not run or did not finish) `top` alone, with the device layer's text in the
/// developer diagnostics.
fn with_adb_failure(top: Outcome, error: &DeviceError) -> Outcome {
    let Some(evidence) = error.adb_command() else {
        outcome::diagnostic(&top, error.message());
        return top;
    };
    let text = if evidence.stderr.text.trim().is_empty() {
        &evidence.stdout
    } else {
        &evidence.stderr
    };
    let mut link = Outcome::new(ContractCode::ForeignAdbError)
        .raw_source(RawSource::Adb)
        .raw_text(&text.text);
    if text.truncated {
        link = link.raw_text_truncated(true);
    }
    if let Some(exit_code) = evidence.exit_code {
        link = link.exit_code(exit_code);
    }
    top.caused_by(link)
}

/// `key=value` and `key: value` lines: the values of `key`, each line trimmed.
fn values_of<'a>(
    text: &'a str,
    key: &'a str,
    separator: char,
) -> impl Iterator<Item = &'a str> + 'a {
    text.lines().filter_map(move |line| {
        let (name, value) = line.trim().split_once(separator)?;
        (name.trim() == key).then(|| value.trim())
    })
}

/// The one value of `key`: present, and every line that names it agrees.
fn single_value<'a>(text: &'a str, key: &'a str, separator: char) -> Option<&'a str> {
    let mut values = values_of(text, key, separator);
    let first = values.next()?;
    values.all(|value| value == first).then_some(first)
}

fn parse_power(text: &str) -> Option<ScreenState> {
    match single_value(text, "mWakefulness", '=')? {
        "Awake" => Some(ScreenState::Awake),
        "Asleep" => Some(ScreenState::Asleep),
        "Dozing" => Some(ScreenState::Dozing),
        "Dreaming" => Some(ScreenState::Dreaming),
        _ => None,
    }
}

fn parse_keyguard(text: &str) -> Option<KeyguardState> {
    const SECTION: &str = "KeyguardServiceDelegate";
    let (_, section) = text.split_once(SECTION)?;
    let flag = |key| match values_of(section, key, '=').next()? {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    };
    let showing = flag("showing")?;
    let secure = flag("secure")?;
    Some(match (showing, secure) {
        (false, _) => KeyguardState::None,
        (true, false) => KeyguardState::Insecure,
        (true, true) => KeyguardState::Secure,
    })
}

fn parse_battery(text: &str) -> Option<BatteryState> {
    let integer = |key| single_value(text, key, ':')?.parse::<i64>().ok();
    let powered = |key| match single_value(text, key, ':') {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    };
    // Android 13 added the dock line; earlier versions print none.
    let dock = match values_of(text, "Dock powered", ':').next() {
        None => false,
        Some(_) => powered("Dock powered")?,
    };
    let plugged = if powered("AC powered")? {
        BatteryPlugged::Ac
    } else if powered("USB powered")? {
        BatteryPlugged::Usb
    } else if powered("Wireless powered")? {
        BatteryPlugged::Wireless
    } else if dock {
        BatteryPlugged::Dock
    } else {
        BatteryPlugged::Unplugged
    };
    let status = match integer("status")? {
        1 => BatteryStatus::Unknown,
        2 => BatteryStatus::Charging,
        3 => BatteryStatus::Discharging,
        4 => BatteryStatus::NotCharging,
        5 => BatteryStatus::Full,
        _ => return None,
    };
    let level_percent = u8::try_from(integer("level")?)
        .ok()
        .filter(|level| *level <= 100)?;
    let temperature_deci_c = i32::try_from(integer("temperature")?).ok()?;
    Some(BatteryState {
        level_percent,
        status,
        plugged,
        temperature_deci_c,
    })
}

fn parse_thermal(text: &str) -> Option<ThermalStatus> {
    Some(
        match single_value(text, "Thermal Status", ':')?
            .parse::<u8>()
            .ok()?
        {
            0 => ThermalStatus::None,
            1 => ThermalStatus::Light,
            2 => ThermalStatus::Moderate,
            3 => ThermalStatus::Severe,
            4 => ThermalStatus::Critical,
            5 => ThermalStatus::Emergency,
            6 => ThermalStatus::Shutdown,
            _ => return None,
        },
    )
}

/// The foreground query's answer: the lines before its status marker, read only when dumpsys
/// itself exited 0 (the same reading as [`Adb::foreground_package`]).
fn parse_foreground(text: &str) -> Option<Option<String>> {
    let (body, status) = text.trim_end().rsplit_once(FOREGROUND_STATUS)?;
    (status.trim().parse::<i32>().ok()? == 0).then(|| parse_foreground_package(body))
}

/// `resolve-activity --brief` prints the match line and then `<package>/<activity>`.
fn parse_home(text: &str) -> Option<String> {
    let component = text.lines().map(str::trim).rfind(|line| !line.is_empty())?;
    let (package, activity) = component.split_once('/')?;
    let valid = !package.is_empty()
        && !activity.is_empty()
        && package
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '_'));
    valid.then(|| package.to_owned())
}
