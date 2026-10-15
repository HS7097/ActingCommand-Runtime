// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #398 RD-5b (model-398 v1.1 §7, §10.1): the device state of the handsets.
//!
//! - **Reads.** Only RD-5a's closed sets reach a handset (`DeviceStateShell`, the provider's
//!   `with_handset_state`): the read-only reads, and the two screen-check actions under a step of
//!   the run's lease. An emulator gets no device-state command (§7.2).
//! - **Sampling.** One sampler thread per Runtime, started after the daemon-start preparation and
//!   only when a bound handset is registered. A handset whose link is `device` is read every
//!   `device_state.sample_interval_ms` (power, keyguard, battery, thermal, screen-off timeout; the
//!   foreground only when an `in_use` hold is due for another look); never the screen check, so a
//!   sample wakes nothing. Each sample writes the `device.*` facts with the TTL
//!   `device_state.fact_ttl_ms` and holds or lifts the handset (§7.4).
//! - **Holds.** `locked`, `thermal` (hysteresis between `thermal_hold_at` and
//!   `thermal_resume_below`), `battery_low` (on no power source, below `battery_min_percent`) and
//!   `in_use`. A hold withdraws the policy availability through the one self-check gate, under
//!   `device_state:<hold>` (the first hold in [`DeviceHold`] order); no hold left, the
//!   availability comes back as the self-check and the link allow. A lifted hold records
//!   `device_hold_lifted`. A hold never ends a run that is already running (§7.4).
//! - **Start gate.** Before the first step of every package run on a handset (§7.2): thermal and
//!   battery (a held handset is not woken), then the screen check (one `KEYCODE_WAKEUP` for a dark
//!   screen, one `wm dismiss-keyguard` for a keyguard without a credential under `dismiss`, a
//!   keyguard that waits for a person is `device_locked`), then the foreground under
//!   `device_state.in_use_gate`, then the screen-off timeout for the keepalive. A refusal ends the
//!   run before its first step with its code, and no input reaches the device.
//! - **Keepalive.** Inside a run, an input that comes later than the screen-off timeout less
//!   `device_state.keepalive_margin_ms` after the last one is preceded by the screen check (§7.2).
//! - **Never.** Nothing here changes a setting of the handset, presses the power key or enters a
//!   credential (§7.7); no record names a device serial: a handset is its label
//!   `handset:<alias>`.

use super::*;
use crate::codes::{DeviceHold, DeviceStateHostCode, DeviceStateLocation};
use actingcommand_device::codes::DeviceStateCode;
use actingcommand_device::{
    BatteryPlugged, BatteryState, BatteryStatus, DeviceStateProbe, DeviceStateShell,
    InsecureKeyguardAction, KeyguardState, ScreenCheckPolicy, ScreenState, ThermalStatus,
    check_screen, handset_label, read_battery, read_foreground, read_home, read_keyguard,
    read_screen, read_screen_timeout, read_thermal,
};
use actingcommand_runtime_config::{DeviceStateConfig, KeyguardInsecureAction, ThermalLevel};

/// The runtime facts a sample writes (instance scope, model-398 §7.6), each with the TTL
/// `device_state.fact_ttl_ms`. None names a device.
pub(super) const DEVICE_SCREEN_FACT_KEY: &str = "device.screen";
pub(super) const DEVICE_KEYGUARD_FACT_KEY: &str = "device.keyguard";
pub(super) const DEVICE_BATTERY_FACT_KEY: &str = "device.battery";
pub(super) const DEVICE_THERMAL_FACT_KEY: &str = "device.thermal";
pub(super) const DEVICE_SCREEN_OFF_TIMEOUT_FACT_KEY: &str = "device.screen_off_timeout_ms";

/// How often the sampler looks whether a sample is due and whether the Runtime stops.
const SAMPLER_POLL: Duration = Duration::from_millis(250);

/// What one sample read; a read that failed is `None` and changes no hold.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Sample {
    pub(super) screen: Option<ScreenState>,
    pub(super) keyguard: Option<KeyguardState>,
    pub(super) battery: Option<BatteryState>,
    pub(super) thermal: Option<ThermalStatus>,
    pub(super) screen_off_timeout_ms: Option<u64>,
    /// Whether another application than the instance's and the home screen's is in the
    /// foreground; read only for an `in_use` hold due for another look.
    pub(super) other_application: Option<bool>,
}

/// A hold a handset entered, with the value its code names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HoldEntry {
    Locked,
    Thermal(ThermalLevel),
    BatteryLow(u8),
    InUse,
}

impl HoldEntry {
    pub(super) const fn hold(self) -> DeviceHold {
        match self {
            Self::Locked => DeviceHold::Locked,
            Self::Thermal(_) => DeviceHold::Thermal,
            Self::BatteryLow(_) => DeviceHold::BatteryLow,
            Self::InUse => DeviceHold::InUse,
        }
    }
}

/// A change of a handset's holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HoldChange {
    Entered(HoldEntry),
    Lifted(DeviceHold),
}

/// What the Runtime knows about one bound handset's device state.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Handset {
    alias: String,
    holds: BTreeSet<DeviceHold>,
    /// When the foreground of a handset in use is looked at again.
    in_use_due_ms: Option<u64>,
    /// When the next sample is due.
    next_sample_ms: u64,
    /// The screen-off timeout last read on the handset.
    screen_off_timeout_ms: Option<u64>,
    /// When a run on the handset last reached it: its start gate's screen check or an input.
    last_interaction_ms: Option<u64>,
    /// Whether the last sample had a read that failed (`device_state_unreadable` is recorded
    /// once per failing streak).
    unreadable: bool,
}

/// The level of a thermal status read on a handset.
pub(super) const fn thermal_level(status: ThermalStatus) -> ThermalLevel {
    match status {
        ThermalStatus::None => ThermalLevel::None,
        ThermalStatus::Light => ThermalLevel::Light,
        ThermalStatus::Moderate => ThermalLevel::Moderate,
        ThermalStatus::Severe => ThermalLevel::Severe,
        ThermalStatus::Critical => ThermalLevel::Critical,
        ThermalStatus::Emergency => ThermalLevel::Emergency,
        ThermalStatus::Shutdown => ThermalLevel::Shutdown,
    }
}

/// §7.4: on no power source and below the configured level. A handset plugged in holds no
/// battery, whether it charges or not (a battery-protection hold reads "not charging").
fn battery_low(config: &DeviceStateConfig, battery: &BatteryState) -> bool {
    battery.plugged == BatteryPlugged::Unplugged
        && battery.level_percent < config.battery_min_percent
}

/// §7.4, with hysteresis: a handset not held holds from `thermal_hold_at` on; a held one until
/// its status falls below `thermal_resume_below`.
fn thermal_held(config: &DeviceStateConfig, held: bool, level: ThermalLevel) -> bool {
    if held {
        level >= config.thermal_resume_below
    } else {
        level >= config.thermal_hold_at
    }
}

/// Whether a keyguard read lets a `locked` hold lift: none shows, or one without a credential
/// that the start gate dismisses.
fn keyguard_released(config: &DeviceStateConfig, keyguard: KeyguardState) -> bool {
    match keyguard {
        KeyguardState::None => true,
        KeyguardState::Insecure => config.keyguard_insecure == KeyguardInsecureAction::Dismiss,
        KeyguardState::Secure => false,
    }
}

impl Handset {
    /// Enters or lifts `hold` as `held` says; the change, if any.
    fn set(&mut self, held: bool, entry: HoldEntry) -> Option<HoldChange> {
        let hold = entry.hold();
        if held {
            self.holds
                .insert(hold)
                .then_some(HoldChange::Entered(entry))
        } else {
            self.holds.remove(&hold).then_some(HoldChange::Lifted(hold))
        }
    }

    fn power(
        &mut self,
        config: &DeviceStateConfig,
        thermal: Option<ThermalLevel>,
        battery: Option<&BatteryState>,
        changes: &mut Vec<HoldChange>,
    ) {
        if let Some(level) = thermal {
            let held = thermal_held(config, self.holds.contains(&DeviceHold::Thermal), level);
            changes.extend(self.set(held, HoldEntry::Thermal(level)));
        }
        if let Some(battery) = battery {
            changes.extend(self.set(
                battery_low(config, battery),
                HoldEntry::BatteryLow(battery.level_percent),
            ));
        }
    }
}

/// The holds of the bound handsets (model-398 §7.3-§7.5). Pure: the host gives it the time,
/// the configuration in effect and what it read, and carries out its changes.
#[derive(Debug, Default)]
pub(super) struct HoldMachine {
    handsets: BTreeMap<InstanceId, Handset>,
}

impl HoldMachine {
    /// The machine of `handsets` (instance id, alias), each first sampled one interval after
    /// `now_ms`.
    pub(super) fn new(
        handsets: impl IntoIterator<Item = (InstanceId, String)>,
        now_ms: u64,
        config: &DeviceStateConfig,
    ) -> Self {
        Self {
            handsets: handsets
                .into_iter()
                .map(|(instance_id, alias)| {
                    (
                        instance_id,
                        Handset {
                            alias,
                            holds: BTreeSet::new(),
                            in_use_due_ms: None,
                            next_sample_ms: now_ms.saturating_add(config.sample_interval_ms),
                            screen_off_timeout_ms: None,
                            last_interaction_ms: None,
                            unreadable: false,
                        },
                    )
                })
                .collect(),
        }
    }

    /// Whether the machine knows the instance as a bound handset.
    pub(super) fn watches(&self, instance_id: InstanceId) -> bool {
        self.handsets.contains_key(&instance_id)
    }

    /// The first hold of the instance in [`DeviceHold`] order; `None` for a handset that holds
    /// nothing and for any other instance.
    pub(super) fn hold(&self, instance_id: InstanceId) -> Option<DeviceHold> {
        self.handsets
            .get(&instance_id)
            .and_then(|handset| handset.holds.first().copied())
    }

    /// The handsets to sample at `now_ms` (every one when `all`): instance id, alias, and
    /// whether the foreground is due for another look.
    pub(super) fn due(&self, now_ms: u64, all: bool) -> Vec<(InstanceId, String, bool)> {
        self.handsets
            .iter()
            .filter(|(_, handset)| all || now_ms >= handset.next_sample_ms)
            .map(|(instance_id, handset)| {
                let look = handset.holds.contains(&DeviceHold::InUse)
                    && handset.in_use_due_ms.is_some_and(|due| now_ms >= due);
                (*instance_id, handset.alias.clone(), look)
            })
            .collect()
    }

    /// One sample of the handset at `now_ms` (§7.2, §7.4): thermal and battery enter or lift
    /// their holds; a keyguard that no longer waits for a person lifts `locked`; under
    /// `in_use`, a look that finds no other application lifts it, one that does waits another
    /// backoff, and a gate turned off lifts it. A sample never enters `locked` or `in_use`. The
    /// next sample is due one interval later; `true` when a read failed for the first time since
    /// one passed.
    pub(super) fn on_sample(
        &mut self,
        instance_id: InstanceId,
        now_ms: u64,
        config: &DeviceStateConfig,
        sample: &Sample,
        read_failed: bool,
    ) -> (Vec<HoldChange>, bool) {
        let mut changes = Vec::new();
        let Some(handset) = self.handsets.get_mut(&instance_id) else {
            return (changes, false);
        };
        handset.next_sample_ms = now_ms.saturating_add(config.sample_interval_ms);
        let newly_unreadable = read_failed && !handset.unreadable;
        handset.unreadable = read_failed;
        if sample.screen_off_timeout_ms.is_some() {
            handset.screen_off_timeout_ms = sample.screen_off_timeout_ms;
        }
        handset.power(
            config,
            sample.thermal.map(thermal_level),
            sample.battery.as_ref(),
            &mut changes,
        );
        if handset.holds.contains(&DeviceHold::Locked)
            && sample
                .keyguard
                .is_some_and(|keyguard| keyguard_released(config, keyguard))
        {
            changes.extend(handset.set(false, HoldEntry::Locked));
        }
        if handset.holds.contains(&DeviceHold::InUse) {
            match (config.in_use_gate, sample.other_application) {
                (false, _) | (true, Some(false)) => {
                    handset.in_use_due_ms = None;
                    changes.extend(handset.set(false, HoldEntry::InUse));
                }
                (true, Some(true)) => {
                    handset.in_use_due_ms = Some(now_ms.saturating_add(config.in_use_backoff_ms));
                }
                (true, None) => {}
            }
        }
        (changes, newly_unreadable)
    }

    /// A sample skipped because the handset's link is not `device`: the next one is due one
    /// interval later.
    pub(super) fn skipped(
        &mut self,
        instance_id: InstanceId,
        now_ms: u64,
        config: &DeviceStateConfig,
    ) {
        if let Some(handset) = self.handsets.get_mut(&instance_id) {
            handset.next_sample_ms = now_ms.saturating_add(config.sample_interval_ms);
        }
    }

    /// The start gate's thermal and battery (§7.4): the changes, and the hold that keeps the run
    /// from starting (thermal first).
    pub(super) fn on_gate_power(
        &mut self,
        instance_id: InstanceId,
        config: &DeviceStateConfig,
        thermal: ThermalLevel,
        battery: &BatteryState,
    ) -> (Vec<HoldChange>, Option<HoldEntry>) {
        let mut changes = Vec::new();
        let Some(handset) = self.handsets.get_mut(&instance_id) else {
            return (changes, None);
        };
        handset.power(config, Some(thermal), Some(battery), &mut changes);
        let blocking = if handset.holds.contains(&DeviceHold::Thermal) {
            Some(HoldEntry::Thermal(thermal))
        } else if handset.holds.contains(&DeviceHold::BatteryLow) {
            Some(HoldEntry::BatteryLow(battery.level_percent))
        } else {
            None
        };
        (changes, blocking)
    }

    /// The screen check ended (§7.3): `locked` enters the hold, a ready screen lifts it.
    pub(super) fn on_screen_check(
        &mut self,
        instance_id: InstanceId,
        locked: bool,
    ) -> Vec<HoldChange> {
        self.handsets
            .get_mut(&instance_id)
            .and_then(|handset| handset.set(locked, HoldEntry::Locked))
            .into_iter()
            .collect()
    }

    /// The start gate's foreground (§7.5): `Some(true)` (another application) enters `in_use`,
    /// to be looked at again after the backoff; anything else (the gate off, the instance's
    /// application, the home screen, nothing) lifts it.
    pub(super) fn on_gate_foreground(
        &mut self,
        instance_id: InstanceId,
        now_ms: u64,
        config: &DeviceStateConfig,
        other_application: Option<bool>,
    ) -> Vec<HoldChange> {
        let Some(handset) = self.handsets.get_mut(&instance_id) else {
            return Vec::new();
        };
        let in_use = other_application == Some(true);
        handset.in_use_due_ms = if in_use {
            Some(now_ms.saturating_add(config.in_use_backoff_ms))
        } else {
            None
        };
        handset.set(in_use, HoldEntry::InUse).into_iter().collect()
    }

    /// The start gate passed at `now_ms`, having read the screen-off timeout (or not).
    pub(super) fn gate_passed(
        &mut self,
        instance_id: InstanceId,
        now_ms: u64,
        screen_off_timeout_ms: Option<u64>,
    ) {
        if let Some(handset) = self.handsets.get_mut(&instance_id) {
            handset.last_interaction_ms = Some(now_ms);
            handset.screen_off_timeout_ms = screen_off_timeout_ms;
        }
    }

    /// An input of a run at `now_ms` (§7.2 keepalive): whether the time since the last
    /// interaction reached the screen-off timeout less `margin_ms`, so the screen is checked
    /// first. An unknown timeout checks nothing. The input is the new last interaction.
    pub(super) fn keepalive_due(
        &mut self,
        instance_id: InstanceId,
        now_ms: u64,
        margin_ms: u64,
    ) -> bool {
        let Some(handset) = self.handsets.get_mut(&instance_id) else {
            return false;
        };
        let due = match (handset.last_interaction_ms, handset.screen_off_timeout_ms) {
            (Some(last), Some(timeout)) => {
                now_ms.saturating_sub(last) >= timeout.saturating_sub(margin_ms)
            }
            _ => false,
        };
        handset.last_interaction_ms = Some(now_ms);
        due
    }
}

/// The host's device-state state: the machine and the sampler.
#[derive(Default)]
pub(super) struct DeviceStates {
    machine: HoldMachine,
    sampler: Option<JoinHandle<RuntimeHostResult<()>>>,
}

/// The sampler's start, after the daemon-start preparation: the machine of every bound handset,
/// then the sampler thread when there is one (§7.2).
pub(super) fn start(shared: &Arc<HostShared>) -> RuntimeHostResult<()> {
    let handsets = lock(
        &shared.registered_instances,
        DeviceStateLocation::DeviceStateSample,
    )?
    .values()
    .filter(|instance| {
        instance
            .bound_adb_endpoint()
            .and_then(ResolvedAdbEndpoint::handset)
            .and_then(actingcommand_execution_kernel::HandsetEndpoint::binding)
            .is_some()
    })
    .map(|instance| (instance.instance_id(), instance.instance_alias.clone()))
    .collect::<Vec<_>>();
    if handsets.is_empty() {
        return Ok(());
    }
    let machine = HoldMachine::new(
        handsets,
        shared.monotonic_ms()?,
        &shared.device_state_config()?,
    );
    lock(
        &shared.device_states,
        DeviceStateLocation::DeviceStateSample,
    )?
    .machine = machine;
    let sampler_shared = Arc::clone(shared);
    let sampler = thread::Builder::new()
        .name("actingcommand-runtime-device-state".to_string())
        .spawn(move || sample_loop(&sampler_shared))
        .map_err(|_| {
            RuntimeHostError::fatal(
                DeviceStateHostCode::DeviceStateSamplingFailed,
                DeviceStateLocation::DeviceStateSample,
                RuntimeErrorCode::RuntimeHalted,
            )
        })?;
    lock(
        &shared.device_states,
        DeviceStateLocation::DeviceStateSample,
    )?
    .sampler = Some(sampler);
    Ok(())
}

/// The sampler: the due samples until the Runtime stops. Only a fatal failure ends it early, and
/// marks the Runtime.
fn sample_loop(shared: &HostShared) -> RuntimeHostResult<()> {
    while !shared.fatal.is_shutdown_requested() {
        thread::sleep(SAMPLER_POLL);
        if shared.fatal.is_shutdown_requested() {
            break;
        }
        if let Err(error) = sample(shared, false) {
            shared.fatal.mark(error.clone())?;
            return Err(error);
        }
    }
    Ok(())
}

/// Joins the sampler, which ends with the shutdown request.
pub(super) fn stop(shared: &HostShared) -> RuntimeHostResult<()> {
    let sampler = lock(
        &shared.device_states,
        DeviceStateLocation::DeviceStateSample,
    )?
    .sampler
    .take();
    join_runtime_thread(sampler, DeviceStateLocation::DeviceStateSample)
}

/// One pass of the sampler: each handset whose sample is due (each one when `all`). A failure
/// that leaves the Runtime running is recorded under the handset's links and the next handset
/// goes on; a fatal one is returned.
pub(super) fn sample(shared: &HostShared, all: bool) -> RuntimeHostResult<()> {
    let now_ms = shared.monotonic_ms()?;
    let config = shared.device_state_config()?;
    let due = lock(&shared.device_states, DeviceStateLocation::DeviceStateApply)?
        .machine
        .due(now_ms, all);
    for (instance_id, alias, look) in due {
        match shared.sample_handset(instance_id, &alias, look, now_ms, &config) {
            Ok(()) => {}
            Err(error) if error.is_fatal() => return Err(error),
            Err(error) => shared.append_lifecycle_failure(
                RuntimeLifecycleFailureStage::OperationCleanup,
                RuntimeLifecycleFailure::Host(&error),
                shared.device_state_links(instance_id)?,
                None,
            )?,
        }
    }
    Ok(())
}

/// The first failure among the reads, the value of each that passed.
fn kept<T>(read: Result<T, Outcome>, failure: &mut Option<Outcome>) -> Option<T> {
    match read {
        Ok(value) => Some(value),
        Err(outcome) => {
            failure.get_or_insert(outcome);
            None
        }
    }
}

/// Whether another application than `application_id` and the home screen is in the foreground
/// (§7.5). No resumed activity (a transition) is no other application.
fn other_application(
    shell: &mut dyn DeviceStateShell,
    application_id: &str,
) -> Result<bool, Outcome> {
    let Some(foreground) = read_foreground(shell)? else {
        return Ok(false);
    };
    if foreground == application_id {
        return Ok(false);
    }
    Ok(foreground != read_home(shell)?)
}

/// The screen check's configured values (§7.8).
fn screen_policy(config: &DeviceStateConfig) -> ScreenCheckPolicy {
    ScreenCheckPolicy {
        wake_settle: Duration::from_millis(config.wake_settle_ms),
        keyguard_insecure: match config.keyguard_insecure {
            KeyguardInsecureAction::Dismiss => InsecureKeyguardAction::Dismiss,
            KeyguardInsecureAction::Wait => InsecureKeyguardAction::Wait,
        },
    }
}

/// A device-state outcome of the device layer (RD-5a) as the error that ends a run at
/// `location`: its own code on top, the outcome as it is (with its `caused_by` link), and the
/// projection of a backend operation that failed, so a run whose handset left `device` ends
/// with `device_link_lost` (RD-2). The checks end only with the device layer's own codes.
fn device_state_failure(outcome: Outcome, location: DeviceStateLocation) -> RequestFailure {
    let code = DeviceStateCode::ALL
        .iter()
        .copied()
        .find(|code| outcome.is(*code))
        .unwrap_or(DeviceStateCode::DeviceStateUnreadable);
    RequestFailure::request(
        RuntimeHostError::request(code, location, RuntimeErrorCode::BackendOperationFailed)
            .with_outcome_values(|_| outcome.operation(location)),
        RuntimeReceiptState::Failed,
        None,
    )
}

/// A hold that keeps a run from starting, as its error: the hold's code with its values.
fn hold_failure(entry: HoldEntry, device_label: &str) -> RequestFailure {
    let location = DeviceStateLocation::DeviceStateGate;
    let error = match entry {
        HoldEntry::Thermal(level) => RuntimeHostError::request(
            DeviceStateHostCode::DeviceThermalHold,
            location,
            RuntimeErrorCode::BackendOperationFailed,
        )
        .with_outcome_values(|outcome| outcome.thermal_status(level).device_label(device_label)),
        HoldEntry::BatteryLow(percent) => RuntimeHostError::request(
            DeviceStateHostCode::DeviceBatteryLow,
            location,
            RuntimeErrorCode::BackendOperationFailed,
        )
        .with_outcome_values(|outcome| outcome.battery_percent(percent).device_label(device_label)),
        HoldEntry::InUse => RuntimeHostError::request(
            DeviceStateHostCode::DeviceInUse,
            location,
            RuntimeErrorCode::BackendOperationFailed,
        )
        .with_outcome_values(|outcome| outcome.device_label(device_label)),
        HoldEntry::Locked => RuntimeHostError::request(
            DeviceStateCode::DeviceLocked,
            location,
            RuntimeErrorCode::BackendOperationFailed,
        )
        .with_outcome_values(|outcome| outcome.device_label(device_label)),
    };
    RequestFailure::request(error, RuntimeReceiptState::Failed, None)
}

/// A host failure on a run's path: fatal ones poison the Runtime, the others end the run.
fn run_failure(error: RuntimeHostError) -> RequestFailure {
    if error.is_fatal() {
        RequestFailure::poison_without_terminal(error)
    } else {
        RequestFailure::request(error, RuntimeReceiptState::Failed, None)
    }
}

fn screen_word(screen: ScreenState) -> &'static str {
    match screen {
        ScreenState::Awake => "awake",
        ScreenState::Asleep => "asleep",
        ScreenState::Dozing => "dozing",
        ScreenState::Dreaming => "dreaming",
    }
}

fn keyguard_word(keyguard: KeyguardState) -> &'static str {
    match keyguard {
        KeyguardState::None => "none",
        KeyguardState::Insecure => "insecure",
        KeyguardState::Secure => "secure",
    }
}

fn plugged_word(plugged: BatteryPlugged) -> &'static str {
    match plugged {
        BatteryPlugged::Unplugged => "unplugged",
        BatteryPlugged::Ac => "ac",
        BatteryPlugged::Usb => "usb",
        BatteryPlugged::Wireless => "wireless",
        BatteryPlugged::Dock => "dock",
    }
}

fn battery_status_word(status: BatteryStatus) -> &'static str {
    match status {
        BatteryStatus::Unknown => "unknown",
        BatteryStatus::Charging => "charging",
        BatteryStatus::Discharging => "discharging",
        BatteryStatus::NotCharging => "not_charging",
        BatteryStatus::Full => "full",
    }
}

/// The `device.*` facts of a sample, by key (§7.6). No value names a device.
fn sample_facts(sample: &Sample) -> Vec<(&'static str, ContractFactValue)> {
    let mut facts = Vec::new();
    if let Some(screen) = sample.screen {
        facts.push((
            DEVICE_SCREEN_FACT_KEY,
            ContractFactValue::String(screen_word(screen).to_owned()),
        ));
    }
    if let Some(keyguard) = sample.keyguard {
        facts.push((
            DEVICE_KEYGUARD_FACT_KEY,
            ContractFactValue::String(keyguard_word(keyguard).to_owned()),
        ));
    }
    if let Some(battery) = &sample.battery {
        facts.push((
            DEVICE_BATTERY_FACT_KEY,
            ContractFactValue::RecordList(vec![BTreeMap::from([
                (
                    "level".to_owned(),
                    FactScalar::Integer(i64::from(battery.level_percent)),
                ),
                (
                    "plugged".to_owned(),
                    FactScalar::String(plugged_word(battery.plugged).to_owned()),
                ),
                (
                    "status".to_owned(),
                    FactScalar::String(battery_status_word(battery.status).to_owned()),
                ),
                (
                    "temperature_deci_c".to_owned(),
                    FactScalar::Integer(i64::from(battery.temperature_deci_c)),
                ),
            ])]),
        ));
    }
    if let Some(thermal) = sample.thermal {
        facts.push((
            DEVICE_THERMAL_FACT_KEY,
            ContractFactValue::String(thermal_level(thermal).as_str().to_owned()),
        ));
    }
    if let Some(timeout) = sample.screen_off_timeout_ms {
        facts.push((
            DEVICE_SCREEN_OFF_TIMEOUT_FACT_KEY,
            ContractFactValue::DurationMs(timeout),
        ));
    }
    facts
}

impl HostShared {
    /// Whether the instance is a bound handset whose device state the Runtime keeps.
    fn device_state_watches(&self, instance_id: InstanceId) -> RuntimeHostResult<bool> {
        Ok(
            lock(&self.device_states, DeviceStateLocation::DeviceStateRead)?
                .machine
                .watches(instance_id),
        )
    }

    /// The device-state hold that keeps the instance unavailable (the availability gate): a
    /// handset's first hold; `None` for every other instance.
    pub(super) fn device_state_hold(
        &self,
        instance_id: InstanceId,
    ) -> RuntimeHostResult<Option<DeviceHold>> {
        Ok(
            lock(&self.device_states, DeviceStateLocation::DeviceStateRead)?
                .machine
                .hold(instance_id),
        )
    }

    /// `work` with the device-state shell of the bound handset `instance_alias` and its
    /// application id; `false` when the provider has no such handset. A provider that cannot
    /// reach the instance fails the first read `work` makes, `probe`: `device_state_unreadable`
    /// at `location`, the provider's text in the developer diagnostics.
    fn handset_state(
        &self,
        instance_alias: &str,
        location: DeviceStateLocation,
        probe: DeviceStateProbe,
        work: &mut dyn FnMut(&mut dyn DeviceStateShell, &str),
    ) -> RuntimeHostResult<bool> {
        self.execution()?
            .with_handset_state(instance_alias, work)
            .map_err(|error| {
                RuntimeHostError::request(
                    DeviceStateCode::DeviceStateUnreadable,
                    location,
                    RuntimeErrorCode::BackendOperationFailed,
                )
                .with_outcome_values(|outcome| outcome.device_state_probe(probe))
                .with_native_detail(error.to_string())
            })
    }

    /// One sample of the handset `instance_id` at `now_ms` (§7.2): read only while its link is
    /// `device`; the facts, then the holds, then a first failing read's record.
    fn sample_handset(
        &self,
        instance_id: InstanceId,
        alias: &str,
        look_at_foreground: bool,
        now_ms: u64,
        config: &DeviceStateConfig,
    ) -> RuntimeHostResult<()> {
        if self.device_link_hold(instance_id)?.is_some() {
            lock(&self.device_states, DeviceStateLocation::DeviceStateApply)?
                .machine
                .skipped(instance_id, now_ms, config);
            return Ok(());
        }
        let mut read = None;
        let reached = self.handset_state(
            alias,
            DeviceStateLocation::DeviceStateSample,
            DeviceStateProbe::Power,
            &mut |shell: &mut dyn DeviceStateShell, application_id: &str| {
                let mut failure = None;
                let sample = Sample {
                    screen: kept(read_screen(shell), &mut failure),
                    keyguard: kept(read_keyguard(shell), &mut failure),
                    battery: kept(read_battery(shell), &mut failure),
                    thermal: kept(read_thermal(shell), &mut failure),
                    screen_off_timeout_ms: kept(read_screen_timeout(shell), &mut failure)
                        .map(|settings| settings.screen_off_timeout_ms),
                    other_application: if look_at_foreground {
                        kept(other_application(shell, application_id), &mut failure)
                    } else {
                        None
                    },
                };
                read = Some((sample, failure));
            },
        )?;
        let Some((sample, failure)) = read.filter(|_| reached) else {
            return Ok(());
        };
        let (changes, newly_unreadable) =
            lock(&self.device_states, DeviceStateLocation::DeviceStateApply)?
                .machine
                .on_sample(instance_id, now_ms, config, &sample, failure.is_some());
        self.record_device_state_facts(instance_id, &sample, config)?;
        if let Some(failure) = failure.filter(|_| newly_unreadable) {
            self.record_device_state_observed(
                instance_id,
                failure.operation(DeviceStateLocation::DeviceStateSample),
            )?;
        }
        self.apply_hold_changes(instance_id, alias, &changes, true)
    }

    /// Writes the facts of a sample, each with the TTL `device_state.fact_ttl_ms`: a changed
    /// value at once, an unchanged one when its record would expire before the next sample.
    fn record_device_state_facts(
        &self,
        instance_id: InstanceId,
        sample: &Sample,
        config: &DeviceStateConfig,
    ) -> RuntimeHostResult<()> {
        let scope = RuntimeFactScope::Instance { instance_id };
        for (key, value) in sample_facts(sample) {
            let now = self.clock.sample()?.unix_ms;
            let stored_at = {
                let store = lock(&self.runtime_facts, DeviceStateLocation::DeviceStateApply)?;
                match store.get(&scope, key) {
                    Some(record)
                        if record.value == value
                            && record.ttl_ms == Some(config.fact_ttl_ms)
                            && record.expires_at_unix_ms().is_some_and(|expires| {
                                expires > now.saturating_add(config.sample_interval_ms)
                            }) =>
                    {
                        continue;
                    }
                    Some(record) => Some(record.observed_at_unix_ms),
                    None => None,
                }
            };
            let observed_at_unix_ms = match stored_at {
                Some(stored) => now.max(stored.saturating_add(1)),
                None => now,
            };
            self.record_runtime_fact(RuntimeFactRecord {
                scope: scope.clone(),
                key: key.to_owned(),
                value,
                observed_at_unix_ms,
                source: OriginModule::Runtime,
                ttl_ms: Some(config.fact_ttl_ms),
            })?;
        }
        Ok(())
    }

    /// Carries out the changes of the handset's holds: `device_hold_lifted` for each lifted
    /// hold, the hold's own record for each entered one when `record_entries` (a sample's; a
    /// start gate's refusal is the run's own record), then the availability decided again.
    fn apply_hold_changes(
        &self,
        instance_id: InstanceId,
        alias: &str,
        changes: &[HoldChange],
        record_entries: bool,
    ) -> RuntimeHostResult<()> {
        if changes.is_empty() {
            return Ok(());
        }
        let label = handset_label(alias);
        for change in changes {
            let outcome = match *change {
                HoldChange::Lifted(hold) => Some(
                    Outcome::new(DeviceStateHostCode::DeviceHoldLifted)
                        .device_hold(hold)
                        .device_label(&label),
                ),
                HoldChange::Entered(_) if !record_entries => None,
                HoldChange::Entered(HoldEntry::Thermal(level)) => Some(
                    Outcome::new(DeviceStateHostCode::DeviceThermalHold)
                        .thermal_status(level)
                        .device_label(&label),
                ),
                HoldChange::Entered(HoldEntry::BatteryLow(percent)) => Some(
                    Outcome::new(DeviceStateHostCode::DeviceBatteryLow)
                        .battery_percent(percent)
                        .device_label(&label),
                ),
                HoldChange::Entered(HoldEntry::InUse) => {
                    Some(Outcome::new(DeviceStateHostCode::DeviceInUse).device_label(&label))
                }
                HoldChange::Entered(HoldEntry::Locked) => {
                    Some(Outcome::new(DeviceStateCode::DeviceLocked).device_label(&label))
                }
            };
            if let Some(outcome) = outcome {
                self.record_device_state_observed(instance_id, outcome)?;
            }
        }
        self.regate_policy_instance_availability(instance_id)
    }

    /// A `runtime.lifecycle_observed` `device_state_observed` record carrying `outcome`, linked
    /// to the handset's instance.
    fn record_device_state_observed(
        &self,
        instance_id: InstanceId,
        outcome: Outcome,
    ) -> RuntimeHostResult<()> {
        self.append_event_raw(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            self.device_state_links(instance_id)?,
            RuntimePayloadDraft::lifecycle_observed(
                self.owner_epoch,
                RuntimeLifecyclePhase::DeviceStateObserved { outcome },
                AuditInput::new(),
            ),
        )
        .map(|_| ())
    }

    /// The links of a handset's records: the instance under fresh system links.
    fn device_state_links(&self, instance_id: InstanceId) -> RuntimeHostResult<EventLinksDraft> {
        Ok(self
            .events
            .system_links()?
            .with_instance_id(self.events.issuer().issue_registered_instance(instance_id)))
    }

    /// A step of the run's lease for the screen check's two device-effect commands (the wake
    /// key, the keyguard dismissal): the Business witness `begin_destructive_step` issues, as
    /// for an input. A refusal of the lease ends the run.
    fn begin_device_state_step(
        &self,
        token: &LeaseToken,
        connection_id: ConnectionId,
    ) -> Result<FencedWrite, RequestFailure> {
        lock(&self.scheduler, DeviceStateLocation::DeviceStateStep)
            .and_then(|mut scheduler| {
                scheduler
                    .begin_destructive_step(token, connection_id, self.monotonic_ms()?)
                    .map_err(|error| {
                        RuntimeHostError::scheduler(DeviceStateLocation::DeviceStateStep, &error)
                            .naming_lease(token.lease_id())
                    })
            })
            .map_err(run_failure)
    }

    /// Ends the screen check's step; a step the scheduler does not take back poisons the
    /// Runtime, as for an input.
    fn finish_device_state_step(
        &self,
        witness: FencedWrite,
        connection_id: ConnectionId,
    ) -> Result<(), RequestFailure> {
        let lease_id = witness.token().lease_id();
        lock(&self.scheduler, DeviceStateLocation::DeviceStateStep)
            .and_then(|mut scheduler| {
                scheduler
                    .finish_destructive_step(witness, connection_id)
                    .map_err(|error| {
                        RuntimeHostError::scheduler(DeviceStateLocation::DeviceStateStep, &error)
                            .naming_lease(lease_id)
                    })
            })
            .map_err(RequestFailure::poison_without_terminal)
    }

    /// The screen check (§7.3) under a step of the run's lease, at `location`: `device_screen_woken`
    /// when it lit the screen, `locked` entered or lifted, and a check that did not end ready as
    /// the run's failure with its own code.
    fn checked_screen(
        &self,
        instance_id: InstanceId,
        instance_alias: &str,
        token: &LeaseToken,
        connection_id: ConnectionId,
        config: &DeviceStateConfig,
        location: DeviceStateLocation,
    ) -> Result<(), RequestFailure> {
        let witness = self.begin_device_state_step(token, connection_id)?;
        let policy = screen_policy(config);
        let mut checked = None;
        let reached = self.handset_state(
            instance_alias,
            location,
            DeviceStateProbe::Power,
            &mut |shell: &mut dyn DeviceStateShell, _: &str| {
                checked = Some(check_screen(shell, &witness, policy));
            },
        );
        let finished = self.finish_device_state_step(witness, connection_id);
        let reached = reached.map_err(run_failure)?;
        finished?;
        let Some(checked) = checked.filter(|_| reached) else {
            return Ok(());
        };
        let locked = matches!(&checked, Err(outcome) if outcome.is(DeviceStateCode::DeviceLocked));
        let changes = lock(&self.device_states, DeviceStateLocation::DeviceStateApply)
            .map_err(run_failure)?
            .machine
            .on_screen_check(instance_id, locked);
        if let Ok(ready) = &checked
            && ready.woken
        {
            self.record_device_state_observed(
                instance_id,
                Outcome::new(DeviceStateHostCode::DeviceScreenWoken)
                    .device_label(handset_label(instance_alias)),
            )
            .map_err(run_failure)?;
        }
        self.apply_hold_changes(instance_id, instance_alias, &changes, false)
            .map_err(run_failure)?;
        checked
            .map(|_ready| ())
            .map_err(|outcome| device_state_failure(outcome, location))
    }

    /// Workflow #398 RD-5b (model-398 v1.1 §7.2-§7.5): the start gate of a package run on a
    /// bound handset, before its first step; any other instance passes untouched.
    ///
    /// 1. Thermal and battery: a handset held for either (with the thermal hysteresis) does not
    ///    start the run (`device_thermal_hold`, `device_battery_low`), and is not woken.
    /// 2. The screen check under a step of the run's lease: `device_wake_failed`,
    ///    `device_keyguard_dismiss_failed` and `device_locked` (which holds the handset) keep the
    ///    run from starting.
    /// 3. Under `device_state.in_use_gate`, another application than the instance's and the home
    ///    screen's in the foreground holds the handset and keeps the run from starting
    ///    (`device_in_use`).
    /// 4. The screen-off timeout, for the keepalive; a failed read is recorded
    ///    (`device_state_unreadable`) and the run starts without the keepalive.
    ///
    /// A read that fails or answers an unknown shape is `device_state_unreadable`. Every
    /// refusal is the run's failure with its own code, before any input.
    pub(super) fn device_state_gate(
        &self,
        instance_id: InstanceId,
        instance_alias: &str,
        token: &LeaseToken,
        connection_id: ConnectionId,
    ) -> Result<(), RequestFailure> {
        if !self
            .device_state_watches(instance_id)
            .map_err(run_failure)?
        {
            return Ok(());
        }
        let location = DeviceStateLocation::DeviceStateGate;
        let config = self.device_state_config().map_err(run_failure)?;
        let label = handset_label(instance_alias);
        let mut power = None;
        let reached = self
            .handset_state(
                instance_alias,
                location,
                DeviceStateProbe::Thermal,
                &mut |shell: &mut dyn DeviceStateShell, _: &str| {
                    power =
                        Some(read_thermal(shell).and_then(|thermal| {
                            read_battery(shell).map(|battery| (thermal, battery))
                        }));
                },
            )
            .map_err(run_failure)?;
        let Some(power) = power.filter(|_| reached) else {
            return Ok(());
        };
        let (thermal, battery) =
            power.map_err(|outcome| device_state_failure(outcome, location))?;
        let (changes, blocking) = lock(&self.device_states, DeviceStateLocation::DeviceStateApply)
            .map_err(run_failure)?
            .machine
            .on_gate_power(instance_id, &config, thermal_level(thermal), &battery);
        self.apply_hold_changes(instance_id, instance_alias, &changes, false)
            .map_err(run_failure)?;
        if let Some(entry) = blocking {
            return Err(hold_failure(entry, &label));
        }
        self.checked_screen(
            instance_id,
            instance_alias,
            token,
            connection_id,
            &config,
            location,
        )?;
        let mut rest = None;
        let first_read = if config.in_use_gate {
            DeviceStateProbe::Foreground
        } else {
            DeviceStateProbe::Settings
        };
        self.handset_state(
            instance_alias,
            location,
            first_read,
            &mut |shell: &mut dyn DeviceStateShell, application_id: &str| {
                let foreground = config
                    .in_use_gate
                    .then(|| other_application(shell, application_id));
                rest = Some((foreground, read_screen_timeout(shell)));
            },
        )
        .map_err(run_failure)?;
        let Some((foreground, timeout)) = rest else {
            return Ok(());
        };
        let other = foreground
            .transpose()
            .map_err(|outcome| device_state_failure(outcome, location))?;
        let now_ms = self.monotonic_ms().map_err(run_failure)?;
        let changes = lock(&self.device_states, DeviceStateLocation::DeviceStateApply)
            .map_err(run_failure)?
            .machine
            .on_gate_foreground(instance_id, now_ms, &config, other);
        self.apply_hold_changes(instance_id, instance_alias, &changes, false)
            .map_err(run_failure)?;
        if other == Some(true) {
            return Err(hold_failure(HoldEntry::InUse, &label));
        }
        let timeout = match timeout {
            Ok(settings) => Some(settings.screen_off_timeout_ms),
            Err(outcome) => {
                self.record_device_state_observed(instance_id, outcome.operation(location))
                    .map_err(run_failure)?;
                None
            }
        };
        lock(&self.device_states, DeviceStateLocation::DeviceStateApply)
            .map_err(run_failure)?
            .machine
            .gate_passed(instance_id, now_ms, timeout);
        Ok(())
    }

    /// Workflow #398 RD-5b (model-398 v1.1 §7.2): the keepalive before an input of a run on a
    /// bound handset. When the time since the last interaction reached the screen-off timeout
    /// less `device_state.keepalive_margin_ms`, the screen is checked first under a step of the
    /// run's lease (a dark screen gets the wake key); a check that does not end ready ends the
    /// run with its code, before the input. Any other instance, and an input within the time,
    /// pass untouched.
    pub(super) fn device_state_keepalive(
        &self,
        instance_id: InstanceId,
        instance_alias: &str,
        token: &LeaseToken,
        connection_id: ConnectionId,
    ) -> Result<(), RequestFailure> {
        if !self
            .device_state_watches(instance_id)
            .map_err(run_failure)?
        {
            return Ok(());
        }
        let config = self.device_state_config().map_err(run_failure)?;
        let now_ms = self.monotonic_ms().map_err(run_failure)?;
        let due = lock(&self.device_states, DeviceStateLocation::DeviceStateApply)
            .map_err(run_failure)?
            .machine
            .keepalive_due(instance_id, now_ms, config.keepalive_margin_ms);
        if !due {
            return Ok(());
        }
        self.checked_screen(
            instance_id,
            instance_alias,
            token,
            connection_id,
            &config,
            DeviceStateLocation::DeviceStateKeepalive,
        )
    }
}

#[cfg(test)]
mod tests;
