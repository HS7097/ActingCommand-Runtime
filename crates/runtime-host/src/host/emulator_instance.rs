// SPDX-License-Identifier: AGPL-3.0-only

//! Emulator instance control (`RuntimeOperation::ControlEmulatorInstance`, Runtime slice
//! #316-B).
//!
//! One explicit User+Ui or Cli request drives the provider's documented `control` surface:
//! once for `start` and `stop`; for `restart` (Workflow #381 B F1′, review M1) a Stop phase and
//! then a Start phase under one instance admission guard, each through the same drive path and
//! each recording its own `command.received` and `command.validated` under the restart's links,
//! as the ladder's emulator-restart rung does; the vendor's restart verb is never dispatched.
//! The Start follows a confirmed Stop or a Stop whose readiness wait expired, never another
//! Stop failure (F3).
//!
//! Workflow #369 H-4 (S6b): the request is an operator claim on the instance's key. It waits at
//! most `EMULATOR_CONTROL_CLAIM_WAIT_MS` for its turn in the instance's one queue (W-3; not
//! granted in time it is refused `lease_queue_expired` and nothing is driven), and it is granted
//! on a stopped (pending) instance too (Q-2a). It holds the key through the Stop, the Start, the
//! ADB baseline and the preparation, renewing it before the provider step (the Stop's or the
//! Start's bound) and before the preparation (Q-5, B1; review L-3); a refused renewal ends the
//! hold with the renewal's own code. The call-site fence admits the control's own key (lease id
//! and holder id, unexpired, no destructive step open; otherwise `emulator_control_busy`). The
//! instance's retained device session is closed first for every action under that key while the
//! daemon keeps running (a session must not outlive the endpoint it was opened on). The
//! provider is driven outside the fact write gate and outside any device session, with the
//! instance's admission guard held, so no keyless observe meets the changing emulator. After
//! `start` / `restart` the preparation phase reopens the session on the control's key and keeps
//! it open (Workflow #191 H: the session belongs to the instance and only an external command
//! disconnects it); after `stop` nothing reopens it.
//!
//! Slice #316-B2: a discovery-bound instance may be registered with a PENDING endpoint (it
//! was stopped at startup). After a successful Start the registry and the host record are
//! bound to the discovered host and the reported port while the admission guard is still
//! held, and one more `runtime.instance_bound` event records the binding; after `Stop` they
//! return to pending. A running outcome without a port fails typed
//! (`emulator_control_endpoint_unresolved`) instead of guessing.
//!
//! Every request is recorded intent -> result: `client.cli_command` / `client.ui_action` plus
//! `command.received`, then `command.validated` (Performed) on success, or `command.rejected`
//! plus one `runtime.failed` whose outcome links the bounded Sensitive vendor output as
//! `foreign_mumu_manager_error` with the tool's exit code (`exit_code`, never an OS error), and
//! a typed primary detail. Success also
//! records the instance program fact `device.connected` (and, after `Stop`, invalidates it
//! with `device_closed`). Workflow #381 B F4 (review M3): after a failed provider control the
//! instance's state is read once (bounded) and applied before the failure is recorded; a
//! failed restart records one `command.rejected` and one `runtime.failed` for both phases
//! (`emulator_restart_stop_failed` / `emulator_restart_start_failed`, coordinator answer D2).
//!
//! Slice #316-B4: the stuck-recovery ladder's emulator-restart rung drives the same action
//! through `drive_emulator_control_while_guarded` after recording its own `command.received`
//! intent. Workflow #369-1: on that recovery path this module neither prepares the connection
//! nor schedules the startup package; the rung's own readiness wait does both. Workflow #369
//! H-3: the rung runs on the ladder's key, which the fence admits (lease id and holder id, no
//! destructive step) and under which the retained session is closed; the ladder renews it.
//!
//! Workflow #369 H-4, M6 (S6b): the control's hold ends with its release. After a `start` /
//! `restart` whose preparation passed, an instance with a startup package hands the key to the
//! package's claim on that release, through the gate (`hand_off_startup_package`), so no
//! dispatch can see the instance free between the start and the startup run; behind a closed
//! gate the claim waits queued. A hold that hands no startup claim over (a stop, a failed
//! preparation) first cancels the instance's queued startup claims (coordinator ruling on
//! review #716). A failed preparation's admitted ladder (`record_preparation_finished`) gets
//! the key straight from the release (H-1, P1; `release_to_preparation_ladder`).

use super::host_claims::HostKey;
use super::lease::HostClaim;
use super::recovery_ladder::PendingRecoveryLadder;
use super::runtime_facts::{TASK_GAME_FACT_KEY, TASK_PAGE_FACT_KEY, TASK_SERVER_FACT_KEY};
use super::startup_package::PendingStartupPackage;
use super::*;
use crate::codes::HostCode;
use crate::codes::HostLocation;
use crate::{DeviceProcessPresence, EmulatorControlFailure, EmulatorControlOutcome, InstanceState};
use actingcommand_contract::codes::{ContractLocation, RuntimeErrorCode};
use actingcommand_contract::outcome::{Location, Outcome, VocabularyToken};
use actingcommand_contract::{CONNECTION_PREPARATION_WAIT_MS, EmulatorInstanceAction, FactValue};
use actingcommand_device::codes::DeviceStage;
use actingcommand_execution_kernel::ExecutionKernelLocation;

/// Workflow #369 H-4 (S6b): an operator's emulator Stop, its key renewed to this bound and the
/// margin before the provider step (review L-3; Workflow #381 B, coordinator answer D3): the
/// control bound 60 s + the readiness wait 120 s + the last `info` poll 10 s + the last process
/// listing 10 s (F2) + the bounded state read after a failure 10 s (review M3) = 210 s. A
/// recovery ladder's rung 3 takes its own bounds from the `recovery_ladder` configuration
/// (Workflow #387 D1c).
const OPERATOR_STOP_BOUND_MS: u64 = 210_000;
/// An operator's emulator Start: 60 + 120 + 10 s and the ADB baseline 30 s = 220 s on success; a
/// failure ends at 60 + 120 + 10 s and the state read 10 s = 200 s.
const OPERATOR_START_BOUND_MS: u64 = 220_000;
/// Workflow #369 Q-5 (B1): an operator's hold renews to a step's bound plus this margin.
const OPERATOR_HOLD_MARGIN_MS: u64 = 5_000;

/// A restart phase as a token of the catalog vocabulary `emulator_restart_phase` (ledger
/// revision 3: the `restart_phase` value of a failed restart's outcome).
#[derive(Clone, Copy)]
struct RestartPhaseToken(&'static str);

impl VocabularyToken for RestartPhaseToken {
    const VOCABULARY: &'static str = "emulator_restart_phase";

    fn token(self) -> &'static str {
        self.0
    }
}

/// How a restart's Stop ended, as a token of the catalog vocabulary `emulator_stop_result`.
#[derive(Clone, Copy)]
struct StopResultToken(&'static str);

impl VocabularyToken for StopResultToken {
    const VOCABULARY: &'static str = "emulator_stop_result";

    fn token(self) -> &'static str {
        self.0
    }
}

/// The `stop_result` of a confirmed Stop.
const STOP_RESULT_CONFIRMED: StopResultToken = StopResultToken("confirmed");

/// The other tokens of the vocabulary `emulator_stop_result`: the stage a Stop failed at, and
/// the token its result takes (Workflow #378 A2-P: an explicit table, where the token was the
/// last part of a dotted stage).
const STOP_RESULT_STAGES: [(Location, &str); 18] = [
    // The default stage of a failure without a device diagnostic (acceptance H-L7).
    (HostLocation::MumuManagerControl.location(), "control"),
    (
        DeviceStage::MumuManagerControlExit.location(),
        "control_exit",
    ),
    (
        DeviceStage::MumuManagerControlErrcode.location(),
        "control_errcode",
    ),
    (
        DeviceStage::MumuManagerLaunchError.location(),
        "launch_error",
    ),
    (
        DeviceStage::MumuManagerWaitTimeout.location(),
        "wait_timeout",
    ),
    (DeviceStage::MumuManagerRun.location(), "run"),
    (DeviceStage::MumuManagerTimeout.location(), "timeout"),
    (DeviceStage::MumuManagerPath.location(), "path"),
    (DeviceStage::MumuManagerErrcode.location(), "errcode"),
    (DeviceStage::MumuManagerEntry.location(), "entry"),
    (DeviceStage::MumuManagerExit.location(), "exit"),
    (DeviceStage::MumuManagerShape.location(), "shape"),
    (DeviceStage::MumuManagerDecode.location(), "decode"),
    (DeviceStage::MumuManagerJson.location(), "json"),
    (
        DeviceStage::MumuManagerOutputBound.location(),
        "output_bound",
    ),
    (
        HostLocation::EmulatorControlBinding.location(),
        "unavailable",
    ),
    (
        ExecutionKernelLocation::EmulatorControlSupport.location(),
        "unsupported",
    ),
    (
        HostLocation::EmulatorControlRegistration.location(),
        "unregistered",
    ),
];

/// The vocabulary token of the stage a Stop failed at; `None` for a stage the table does not
/// list, which the outcome then leaves out.
fn stop_result_token(stage: Location) -> Option<StopResultToken> {
    STOP_RESULT_STAGES
        .iter()
        .find(|(location, _)| *location == stage)
        .map(|&(_, token)| StopResultToken(token))
}

/// A Stop's process check as a token of the catalog vocabulary `device_process_presence`.
#[derive(Clone, Copy)]
struct ProcessPresenceToken(DeviceProcessPresence);

impl VocabularyToken for ProcessPresenceToken {
    const VOCABULARY: &'static str = "device_process_presence";

    fn token(self) -> &'static str {
        self.0.as_str()
    }
}

const CONTROL_OPERATION: ContractLocation = ContractLocation::ControlEmulatorInstance;
/// Workflow #369 Q-2, H-4: how long an operator emulator control waits for its turn in the
/// instance's queue before it is refused `lease_queue_expired` (the client's receipt wait adds
/// it, `client.rs`).
const EMULATOR_CONTROL_CLAIM_WAIT_MS: u64 = 5_000;
pub(super) const DEVICE_CONNECTED_FACT_KEY: &str = "device.connected";
/// The provider control's readiness wait expired (`mumu_manager_wait_timeout`).
pub(super) const EMULATOR_CONTROL_WAIT_TIMEOUT: &str =
    HostCode::EmulatorControlWaitTimeout.as_str();
/// Every other failure of the provider control once it was dispatched or refused by the tool.
const EMULATOR_CONTROL_FAILED: &str = HostCode::EmulatorControlFailed.as_str();
/// Longest wait for adbd after the vendor reports the instance running (#316-B3).
const ADB_BASELINE_WAIT: Duration = Duration::from_secs(30);
const ADB_BASELINE_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// A performed control action: its result terminal and, after `start` / `restart` of an
/// instance with a startup package, that package with its scheduling intent recorded, which the
/// control's release hands its key to (Workflow #369 H-4, S6b); after a failed preparation, the
/// ladder it admitted, which the release hands the key to instead (H-1, P1).
pub(super) struct EmulatorControlDriven {
    pub(super) outcome: EmulatorControlOutcome,
    adb_wait_ms: u64,
    /// Workflow #381 B: a restart's Stop phase provider time (0 for a single action), so the
    /// result's `elapsed_ms` stays the whole request's.
    stop_ms: u64,
    pub(super) terminal: TerminalEvent,
    pub(super) startup_package: Option<PendingStartupPackage>,
    ladder: Option<PendingRecoveryLadder>,
}

/// A failed operator control (Workflow #369 S6b): the request's failure and, for a restart
/// whose Stop failed while its Start succeeded (Workflow #381 B D2 (c)), the startup package the
/// Start's success path scheduled, or the ladder its failed preparation admitted, which the key
/// still goes to after the failure record.
struct FailedControl {
    failure: RequestFailure,
    startup_package: Option<Box<PendingStartupPackage>>,
    ladder: Option<Box<PendingRecoveryLadder>>,
}

impl From<RequestFailure> for FailedControl {
    fn from(failure: RequestFailure) -> Self {
        Self {
            failure,
            startup_package: None,
            ladder: None,
        }
    }
}

impl From<RuntimeHostError> for FailedControl {
    fn from(error: RuntimeHostError) -> Self {
        RequestFailure::poison_without_terminal(error).into()
    }
}

/// Who drives one control action and on which key (Workflow #381 B, coordinator answer D2;
/// Workflow #369 H-3, H-4).
enum ControlMode<'key> {
    /// An operator `start` or `stop` on its claim's key, which the action renews before its
    /// provider step and before its preparation: the action records its own failure.
    Operator(&'key mut HostKey),
    /// A phase of an operator restart on the restart's claim key, renewed as an operator
    /// action's: a failure the Runtime survives is returned unrecorded, so the restart records
    /// one failure for the request, under the restart's action.
    RestartPhase(&'key mut HostKey),
    /// The ladder's emulator-restart rung on its key, which the ladder renews itself: the action
    /// records its own failure.
    Ladder(&'key LeaseToken),
}

impl ControlMode<'_> {
    fn token(&self) -> &LeaseToken {
        match self {
            Self::Operator(key) | Self::RestartPhase(key) => &key.token,
            Self::Ladder(token) => token,
        }
    }

    /// The operator's claim key; `None` on the ladder's rung.
    fn operator_key(&self) -> Option<&HostKey> {
        match self {
            Self::Operator(key) | Self::RestartPhase(key) => Some(&**key),
            Self::Ladder(_) => None,
        }
    }

    fn restart_phase(&self) -> bool {
        matches!(self, Self::RestartPhase(_))
    }

    /// Workflow #369 Q-5 (B1; review L-3): an operator hold renews before a step whose bound
    /// may exceed what is left of its key; the ladder renews its own key.
    fn renew(&mut self, host: &HostShared, bound_ms: u64) -> Result<(), RequestFailure> {
        match self {
            Self::Operator(key) | Self::RestartPhase(key) => {
                host.renew_host_key(key, bound_ms.saturating_add(OPERATOR_HOLD_MARGIN_MS))
            }
            Self::Ladder(_) => Ok(()),
        }
    }
}

/// A failed control action.
enum ControlFailure {
    /// Recorded, or one the Runtime does not survive.
    Request(RequestFailure),
    /// A restart phase's failure, not recorded yet.
    Unrecorded(Box<UnrecordedControl>),
}

impl From<RequestFailure> for ControlFailure {
    fn from(failure: RequestFailure) -> Self {
        Self::Request(failure)
    }
}

impl From<RuntimeHostError> for ControlFailure {
    fn from(error: RuntimeHostError) -> Self {
        Self::Request(RequestFailure::poison_without_terminal(error))
    }
}

/// A restart phase's failure as the restart records it.
struct UnrecordedControl {
    error: RuntimeHostError,
    state: RuntimeReceiptState,
    effect: EffectDisposition,
    /// Present when the provider control itself failed: the device failure and the state the
    /// bounded read after it observed (review M3).
    provider: Option<ProviderControlFailure>,
}

struct ProviderControlFailure {
    failure: Box<EmulatorControlFailure>,
    observed: Option<InstanceState>,
}

/// A restart phase whose provider control failed with one of the two codes an operator
/// restart reports under its own codes (F4): the cause, its effect and the device failure.
struct WrappedFailure {
    code: HostCode,
    effect: EffectDisposition,
    provider: ProviderControlFailure,
}

impl UnrecordedControl {
    /// The failure as one a restart reports under its own code, or itself when it is any other
    /// failure (a busy fence, a session close, a rebinding, an ADB baseline), which a restart
    /// records as it stands.
    fn into_wrapped(self: Box<Self>) -> Result<WrappedFailure, Box<Self>> {
        let code = match self.error.code() {
            EMULATOR_CONTROL_WAIT_TIMEOUT => HostCode::EmulatorControlWaitTimeout,
            EMULATOR_CONTROL_FAILED => HostCode::EmulatorControlFailed,
            _ => return Err(self),
        };
        let failure = *self;
        match failure.provider {
            Some(provider) => Ok(WrappedFailure {
                code,
                effect: failure.effect,
                provider,
            }),
            None => Err(Box::new(UnrecordedControl {
                error: failure.error,
                state: failure.state,
                effect: failure.effect,
                provider: None,
            })),
        }
    }
}

/// The last observed state of an instance, as a failed restart reports it (F4).
#[derive(Clone, Copy)]
struct ObservedState {
    process_started: bool,
    android_started: bool,
    adb_port_present: bool,
}

impl ObservedState {
    fn of(state: &InstanceState) -> Self {
        Self {
            process_started: state.process_started,
            android_started: state.android_started,
            adb_port_present: state.adb_port.is_some(),
        }
    }

    /// A confirmed Start: running means Android started.
    fn running(outcome: &EmulatorControlOutcome) -> Self {
        Self {
            process_started: outcome.process_started,
            android_started: outcome.running,
            adb_port_present: outcome.adb_port.is_some(),
        }
    }

    /// The bounded read after the failure, else the failure's last observation.
    fn after(provider: &ProviderControlFailure) -> Option<Self> {
        provider
            .observed
            .as_ref()
            .or(provider.failure.last_state.as_ref())
            .map(Self::of)
    }
}

/// How the Start phase of a restart whose Stop failed ended, with its time.
enum StartAfterStop<'a> {
    NotDispatched,
    Confirmed(&'a EmulatorControlDriven, u64),
    /// The last state the failed Start observed, when it is known.
    Failed(Option<ObservedState>, u64),
}

/// The tokens of a failed operator restart (F4.1), written into the failure record's primary
/// detail.
struct RestartReport<'a> {
    code: HostCode,
    phase: &'static str,
    stop_result: Option<StopResultToken>,
    stop_ms: u64,
    start_ms: Option<u64>,
    state: Option<ObservedState>,
    device_process_present: DeviceProcessPresence,
    /// The failing phase: its control code is the `cause`, its device failure the detail.
    failure: &'a WrappedFailure,
    effect: EffectDisposition,
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// The stage a provider control failed at: its device diagnostic's, else the default
/// `mumu_manager_control` (acceptance H-L7).
fn failure_stage(failure: &EmulatorControlFailure) -> Location {
    failure
        .error
        .diagnostic()
        .map_or(HostLocation::MumuManagerControl.location(), |diagnostic| {
            diagnostic.stage()
        })
}

/// The canonical spelling of an instance id, for an outcome's `instance_id` value.
fn instance_token(instance_id: InstanceId) -> String {
    match serde_json::to_value(instance_id) {
        Ok(serde_json::Value::String(text)) => text,
        _ => String::new(),
    }
}

impl HostShared {
    /// `ControlEmulatorInstance`. Workflow #369 H-4 (S6b): after its intent is recorded, the
    /// request waits for the instance's key as an operator claim (`await_emulator_control_claim`)
    /// and drives the action on it under the instance's admission guard; its release then ends
    /// the hold (`end_emulator_control_hold`). A failure the Runtime survives releases the key
    /// as a failed lease end does; an unconfirmed close keeps it (`cleanup_composite_failure`).
    pub(super) fn control_emulator_instance(
        &self,
        original: &RuntimeRequest,
        request: &ValidatedRuntimeRequest<'_>,
        instance_alias: &str,
        action: EmulatorInstanceAction,
        connection_id: ConnectionId,
    ) -> Result<OperationSuccess, RequestFailure> {
        let resolved = self.resolve_instance(instance_alias)?;
        let links = self.append_client_command_intent(
            original,
            request,
            resolved.instance_id(),
            action.event_action(),
            None,
        )?;
        let mut key = self.await_emulator_control_claim(original, instance_alias, connection_id)?;
        // The binding is read again on the key: another control may have changed it meanwhile.
        let driven = match self.resolve_instance(instance_alias) {
            Ok(resolved) if action == EmulatorInstanceAction::Restart => {
                self.drive_emulator_restart(&resolved, links, &mut key)
            }
            Ok(resolved) => self.drive_emulator_control(&resolved, links, action, &mut key),
            Err(failure) => Err(failure.into()),
        };
        let mut driven = match driven {
            Ok(driven) => driven,
            Err(failed) => return Err(self.end_failed_emulator_control(request, key, failed)),
        };
        // Slice #316-B3: after `start` / `restart` the configured startup package is only
        // scheduled (intent event + the key handed to its claim, or the claim queued; Workflow
        // #369 H-4, S6b); it runs on the instance's worker.
        let startup_package = match self.end_emulator_control_hold(
            request,
            &key,
            driven.startup_package.take(),
            driven.ladder.take(),
        ) {
            Ok(startup_package) => startup_package,
            Err(failure) => {
                return Err(self.cleanup_composite_failure(key.token, key.connection_id, failure));
            }
        };
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(driven.terminal),
            result: RuntimeResult::EmulatorInstanceControlled {
                instance_alias: instance_alias.to_owned(),
                action,
                instance_index: driven.outcome.instance_index,
                running: driven.outcome.running,
                adb_port: driven.outcome.adb_port,
                elapsed_ms: driven
                    .stop_ms
                    .saturating_add(driven.outcome.elapsed_ms)
                    .saturating_add(driven.adb_wait_ms),
                startup_package,
            },
            failure_outcome: None,
        })
    }

    /// Workflow #369 Q-2, Q-2a, H-4, W-3: the operator claim of an emulator control (a minted
    /// holder, the scheduler's default lease TTL, drain capacity, no bound-endpoint or
    /// performance-control check, so a stopped instance is granted too). It waits at most
    /// `EMULATOR_CONTROL_CLAIM_WAIT_MS` for its turn; not granted in time, the request is refused
    /// `lease_queue_expired` and nothing is driven. A restart is one claim.
    fn await_emulator_control_claim(
        &self,
        original: &RuntimeRequest,
        instance_alias: &str,
        connection_id: ConnectionId,
    ) -> Result<HostKey, RequestFailure> {
        let holder_id = *self
            .events
            .issuer()
            .mint_holder_id()
            .map_err(|_| RequestFailure::poison_without_terminal(runtime_identifier_error()))?
            .transport();
        let lease_ttl_ms = lock(&self.scheduler, HostLocation::ReadEmulatorControlClaimTtl)?
            .config()
            .lease_ttl_ms;
        let token = self.await_operator_claim(
            HostClaim {
                request: original,
                instance_alias,
                holder_id,
                connection_id,
                kind: ClaimKind::EmulatorControl,
                priority: actingcommand_contract::LeasePriority::Normal,
                lease_ttl_ms,
            },
            EMULATOR_CONTROL_CLAIM_WAIT_MS,
        )?;
        Ok(HostKey {
            request: original.clone(),
            token,
            connection_id,
        })
    }

    /// Workflow #369 H-4, M6, P1: ends an operator control's hold with its explicit release
    /// (the control's request id). With a startup package scheduled, the key goes to the
    /// package's claim through the gate (`hand_off_startup_package`). Otherwise (a stop, a failed
    /// preparation) the instance's queued startup claims are cancelled first (coordinator ruling
    /// on review #716: a startup run against a stopped or unprepared emulator is meaningless),
    /// then the release hands the key straight to the ladder a failed preparation admitted (H-1,
    /// P1; `release_to_preparation_ladder`), or to the next eligible claim.
    fn end_emulator_control_hold(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        key: &HostKey,
        startup_package: Option<PendingStartupPackage>,
        ladder: Option<PendingRecoveryLadder>,
    ) -> Result<StartupPackageDisposition, RequestFailure> {
        let request_id = key.request.request_id();
        if let Some(pending) = startup_package {
            return self.hand_off_startup_package(request, request_id, key, pending);
        }
        self.cancel_queued_startup_claims(key.token.instance_id())?;
        match ladder {
            Some(ladder) => self.release_to_preparation_ladder(
                request,
                request_id,
                &key.token,
                key.connection_id,
                ladder,
            )?,
            None => {
                self.release_lease(request, request_id, &key.token, key.connection_id, None)?;
            }
        }
        Ok(StartupPackageDisposition::None)
    }

    /// The end of a failed operator control's hold: a failure the Runtime does not survive, or
    /// one with neither a startup package nor a ladder, ends the key as a failed lease end does
    /// (`cleanup_composite_failure`: an unconfirmed close keeps it, any other end hands it on).
    /// A restart whose Start succeeded after a failed Stop (Workflow #381 B D2 (c)) hands its
    /// key to the startup package, or to the ladder its failed preparation admitted, after the
    /// failure record, as on success.
    fn end_failed_emulator_control(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        key: HostKey,
        failed: FailedControl,
    ) -> RequestFailure {
        let FailedControl {
            failure,
            startup_package,
            ladder,
        } = failed;
        if failure.poison_runtime || (startup_package.is_none() && ladder.is_none()) {
            return self.cleanup_composite_failure(key.token, key.connection_id, failure);
        }
        match self.end_emulator_control_hold(
            request,
            &key,
            startup_package.map(|pending| *pending),
            ladder.map(|ladder| *ladder),
        ) {
            Ok(_) => failure,
            Err(end) if end.poison_runtime => failure.replace_with_poison(*end.error),
            Err(_) => self.cleanup_composite_failure(key.token, key.connection_id, failure),
        }
    }

    /// Performs one operator `start` or `stop` whose intent is already recorded under `links`,
    /// on its claim's `key` and under the instance's admission guard: fence, session close,
    /// renewal, provider control, rebinding, `backend.selfcheck.*` invalidation (which leaves a
    /// gated physical instance unavailable), ADB baseline, renewal, `command.validated` (or
    /// `command.rejected` + `runtime.failed`), `runtime.instance_bound`, `device.connected`,
    /// after `start` of a physical instance its connection preparation phase on the key
    /// (Workflow #317 sc3), and the startup package's scheduling intent.
    fn drive_emulator_control(
        &self,
        resolved: &RegisteredInstance,
        links: EventLinksDraft,
        action: EmulatorInstanceAction,
        key: &mut HostKey,
    ) -> Result<EmulatorControlDriven, FailedControl> {
        let control_request_id = key.request.request_id();
        let instance_guard = self.instance_guard(resolved.instance_id())?;
        let admission = lock(&instance_guard, HostLocation::LockInstanceAdmission)?;
        match self.drive_control_phase(
            resolved,
            links.clone(),
            action,
            control_request_id,
            &admission,
            &mut ControlMode::Operator(key),
        ) {
            Ok(driven) => Ok(driven),
            Err(ControlFailure::Request(failure)) => Err(failure.into()),
            Err(ControlFailure::Unrecorded(failure)) => Err(self
                .record_unrecorded(links, action.event_action(), *failure)
                .into()),
        }
    }

    /// Workflow #381 B F1′ (review M1): an operator restart is a Stop phase and then a Start
    /// phase under one instance admission guard, each through the existing drive path with its
    /// own `command.received` and `command.validated` under the restart's links; the vendor's
    /// restart verb is never dispatched. F3: the Start follows a confirmed Stop or a Stop whose
    /// readiness wait expired, never another Stop failure. Coordinator answer D2: a failed
    /// restart records one `command.rejected` and one `runtime.failed` (after the Start phase
    /// when one ran), the receipt's terminal; a Start that succeeded after an expired Stop still
    /// prepares the connection and schedules the startup package, as on success. Workflow #369
    /// H-4 (S6b): the restart is one claim; both phases run on its `key`, each renewing it
    /// before its own provider step (review L-3).
    fn drive_emulator_restart(
        &self,
        resolved: &RegisteredInstance,
        links: EventLinksDraft,
        key: &mut HostKey,
    ) -> Result<EmulatorControlDriven, FailedControl> {
        let instance_id = resolved.instance_id();
        let instance_alias = resolved.instance_alias.as_str();
        let control_request_id = key.request.request_id();
        let instance_guard = self.instance_guard(instance_id)?;
        let admission = lock(&instance_guard, HostLocation::LockInstanceAdmission)?;
        let stop_started = Instant::now();
        let stop = self.drive_restart_phase(
            instance_alias,
            &links,
            EmulatorInstanceAction::Stop,
            control_request_id,
            &admission,
            key,
        )?;
        let stop_ms = elapsed_ms(stop_started);
        let restart_action = EmulatorInstanceAction::Restart.event_action();
        let stop = match stop {
            Ok(stopped) => Ok(stopped),
            Err(failure) => match failure.into_wrapped() {
                Ok(wrapped) if wrapped.code == HostCode::EmulatorControlWaitTimeout => Err(wrapped),
                // F3: no Start after any other Stop failure.
                Ok(wrapped) => {
                    return Err(self
                        .report_stop_failed(
                            &links,
                            instance_id,
                            &wrapped,
                            stop_ms,
                            StartAfterStop::NotDispatched,
                        )
                        .into());
                }
                Err(other) => {
                    return Err(self.record_unrecorded(links, restart_action, *other).into());
                }
            },
        };
        let start_started = Instant::now();
        let start = self.drive_restart_phase(
            instance_alias,
            &links,
            EmulatorInstanceAction::Start,
            control_request_id,
            &admission,
            key,
        )?;
        let start_ms = elapsed_ms(start_started);
        match (stop, start) {
            // The startup package stays on the result: the control's release hands it the key.
            (Ok(stopped), Ok(mut started)) => {
                started.stop_ms = stopped.outcome.elapsed_ms;
                Ok(started)
            }
            (Ok(stopped), Err(failure)) => match failure.into_wrapped() {
                Ok(wrapped) => {
                    let report = RestartReport {
                        code: HostCode::EmulatorRestartStartFailed,
                        phase: "start",
                        stop_result: Some(STOP_RESULT_CONFIRMED),
                        stop_ms,
                        start_ms: Some(start_ms),
                        state: ObservedState::after(&wrapped.provider),
                        device_process_present: stopped
                            .outcome
                            .device_process_present
                            .unwrap_or(DeviceProcessPresence::Unknown),
                        failure: &wrapped,
                        effect: EffectDisposition::Indeterminate,
                    };
                    Err(self
                        .record_restart_report(&links, instance_id, &report)
                        .into())
                }
                Err(other) => Err(self.record_unrecorded(links, restart_action, *other).into()),
            },
            (Err(stop_failure), Ok(mut started)) => {
                // D2 (c): the Start ran its success path; the restart's one record follows it,
                // and the control's release hands the key to the startup package, or to the
                // ladder a failed preparation admitted, after it.
                let pending = started.startup_package.take();
                let ladder = started.ladder.take();
                let failure = self.report_stop_failed(
                    &links,
                    instance_id,
                    &stop_failure,
                    stop_ms,
                    StartAfterStop::Confirmed(&started, start_ms),
                );
                let (startup_package, ladder) = if failure.poison_runtime {
                    (None, None)
                } else {
                    (pending.map(Box::new), ladder.map(Box::new))
                };
                Err(FailedControl {
                    failure,
                    startup_package,
                    ladder,
                })
            }
            (Err(stop_failure), Err(start_failure)) => {
                let start_state = start_failure
                    .into_wrapped()
                    .ok()
                    .and_then(|wrapped| ObservedState::after(&wrapped.provider));
                Err(self
                    .report_stop_failed(
                        &links,
                        instance_id,
                        &stop_failure,
                        stop_ms,
                        StartAfterStop::Failed(start_state, start_ms),
                    )
                    .into())
            }
        }
    }

    /// One phase of an operator restart: its `command.received` under the restart's links,
    /// then the drive on the binding registered now (a Start follows the Stop's binding), on the
    /// restart's `key`. The outer error is recorded or ends the Runtime; the inner one is the
    /// phase's unrecorded failure.
    fn drive_restart_phase(
        &self,
        instance_alias: &str,
        links: &EventLinksDraft,
        action: EmulatorInstanceAction,
        control_request_id: RequestId,
        admission: &MutexGuard<'_, ()>,
        key: &mut HostKey,
    ) -> Result<Result<EmulatorControlDriven, Box<UnrecordedControl>>, RequestFailure> {
        self.append_event(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links.clone(),
            CommandPayloadDraft::received(action.event_action(), AuditInput::new()),
        )?;
        let current = self.resolve_instance(instance_alias)?;
        match self.drive_control_phase(
            &current,
            links.clone(),
            action,
            control_request_id,
            admission,
            &mut ControlMode::RestartPhase(key),
        ) {
            Ok(driven) => Ok(Ok(driven)),
            Err(ControlFailure::Unrecorded(failure)) => Ok(Err(failure)),
            Err(ControlFailure::Request(failure)) => Err(failure),
        }
    }

    /// The stuck-recovery ladder's emulator-restart rung: a cold recovery holds the instance's
    /// admission guard across confirmed Stop and Start, on the ladder's `key` (Workflow #369
    /// H-3), which the ladder renews itself: the fence admits that lease by its lease id and
    /// holder id while no destructive step is open (a call-site parameter;
    /// `monitor_recovery_admission` is unchanged), the retained session is closed under it
    /// (`reuse_active_lease`), and nothing is prepared or scheduled here.
    pub(super) fn drive_emulator_control_while_guarded(
        &self,
        resolved: &RegisteredInstance,
        links: EventLinksDraft,
        action: EmulatorInstanceAction,
        control_request_id: RequestId,
        admission: &MutexGuard<'_, ()>,
        key: &LeaseToken,
    ) -> Result<EmulatorControlDriven, RequestFailure> {
        match self.drive_control_phase(
            resolved,
            links.clone(),
            action,
            control_request_id,
            admission,
            &mut ControlMode::Ladder(key),
        ) {
            Ok(driven) => Ok(driven),
            Err(ControlFailure::Request(failure)) => Err(failure),
            Err(ControlFailure::Unrecorded(failure)) => {
                Err(self.record_unrecorded(links, action.event_action(), *failure))
            }
        }
    }

    /// The drive of one control action (see `drive_emulator_control`) on the key `mode` holds;
    /// `mode` also decides who records its failure and who renews the key.
    fn drive_control_phase(
        &self,
        resolved: &RegisteredInstance,
        links: EventLinksDraft,
        action: EmulatorInstanceAction,
        control_request_id: RequestId,
        admission: &MutexGuard<'_, ()>,
        mode: &mut ControlMode<'_>,
    ) -> Result<EmulatorControlDriven, ControlFailure> {
        let instance_id = resolved.instance_id();
        let event_action = action.event_action();
        let record_action = if mode.restart_phase() {
            EmulatorInstanceAction::Restart.event_action()
        } else {
            event_action
        };
        // Workflow #369 H-3, H-4: the call-site fence admits the control's own key (lease id and
        // holder id, unexpired, no destructive step open).
        let fence = self
            .monitor_recovery_admission(instance_id)
            .map_err(RequestFailure::poison_without_terminal)?;
        if !self
            .own_key_admitted(instance_id, mode.token())
            .map_err(RequestFailure::poison_without_terminal)?
        {
            return Err(self.control_failed(
                mode,
                links,
                record_action,
                busy_error(instance_id, fence.reason),
                RuntimeReceiptState::Denied,
                EffectDisposition::NotPerformed,
                None,
            ));
        }
        // A retained session was opened on the endpoint in force before the action; it must
        // not survive the (re)binding below. It is closed under the control's key. Closing with
        // no session open is a no-op.
        match self.close_retained_instance_while_guarded(
            instance_id,
            links.clone(),
            true,
            LeaseReleaseReason::HostShutdown,
            admission,
        ) {
            Ok(Ok(())) => {}
            Ok(Err(close_error)) => {
                // Workflow #403: an unconfirmed close fenced the instance; the control fails
                // with the fence's non-fatal failure.
                let mut error = self
                    .retained_close_failure(
                        HostLocation::CloseEmulatorDeviceSession,
                        instance_id,
                        &close_error,
                    )
                    .map_err(RequestFailure::poison_without_terminal)?;
                error.lifecycle.instance_id = Some(instance_id);
                return Err(self.control_failed(
                    mode,
                    links,
                    record_action,
                    error,
                    RuntimeReceiptState::Failed,
                    EffectDisposition::NotPerformed,
                    None,
                ));
            }
            Err(error) if error.projection().code == RuntimeErrorCode::LeaseBusy => {
                return Err(self.control_failed(
                    mode,
                    links,
                    record_action,
                    busy_error(instance_id, MonitorRecoveryCoordinationReason::ActiveLease),
                    RuntimeReceiptState::Denied,
                    EffectDisposition::NotPerformed,
                    None,
                ));
            }
            Err(error) => {
                return Err(self.control_failed(
                    mode,
                    links,
                    record_action,
                    error,
                    RuntimeReceiptState::Failed,
                    EffectDisposition::NotPerformed,
                    None,
                ));
            }
        }
        // Workflow #369 H-4 (B1; review L-3): an operator's key is renewed before the provider
        // step, to the Stop's or the Start's bound; a refused renewal ends the hold with its code.
        let provider_bound_ms = if action == EmulatorInstanceAction::Stop {
            OPERATOR_STOP_BOUND_MS
        } else {
            OPERATOR_START_BOUND_MS
        };
        if let Err(refused) = mode.renew(self, provider_bound_ms) {
            return Err(self.renewal_refused(
                mode,
                links,
                record_action,
                instance_id,
                refused,
                EffectDisposition::NotPerformed,
            ));
        }
        // Outside the fact write gate and outside any device session, on the control's key; the
        // per-instance admission guard stays held so no keyless observe meets the changing
        // emulator.
        let outcome = match self
            .execution()?
            .control_instance(&resolved.instance_alias, action)
        {
            Ok(outcome) => outcome,
            Err(failure) => {
                let (error, state, effect) = emulator_control_error(&failure, instance_id);
                // Workflow #381 B F4 (review M3): one bounded read of the instance's current
                // state, applied before the failure is recorded; a failed read is no
                // observation.
                let observed = self
                    .execution()?
                    .read_emulator_instance_state(&resolved.instance_alias)
                    .ok();
                if let Err(applied) =
                    self.apply_failure_observation(resolved, &links, observed.as_ref(), effect)
                {
                    return Err(self.control_failed(
                        mode,
                        links,
                        record_action,
                        applied,
                        RuntimeReceiptState::Failed,
                        effect,
                        None,
                    ));
                }
                return Err(self.control_failed(
                    mode,
                    links,
                    record_action,
                    error,
                    state,
                    effect,
                    Some(ProviderControlFailure { failure, observed }),
                ));
            }
        };
        // Workflow #381 B D6: a Stop that `info` alone confirmed (its process listing failed)
        // is recorded once at Warning before its `command.validated`.
        if action == EmulatorInstanceAction::Stop
            && outcome.device_process_present == Some(DeviceProcessPresence::Unknown)
        {
            self.record_stop_process_unverified(&links, instance_id, outcome.elapsed_ms)?;
        }
        // Still under the admission guard: bind the reported port (or return to pending)
        // before any lease can be granted on the instance.
        let rebound = match self.rebind_instance_endpoint(resolved, action, &outcome) {
            Ok(rebound) => rebound,
            Err(error) => {
                return Err(self.control_failed(
                    mode,
                    links,
                    record_action,
                    error,
                    RuntimeReceiptState::Failed,
                    EffectDisposition::Indeterminate,
                    None,
                ));
            }
        };
        // Workflow #317 sc1: the self-check facts describe the session closed above, opened on
        // the binding just replaced; they are dropped before any lease can open a new one, and
        // a gated physical instance stays unavailable until its next self-check passes (sc3).
        if let Err(error) = self.invalidate_backend_selfcheck_facts(instance_id) {
            return Err(self.control_failed(
                mode,
                links,
                record_action,
                error,
                RuntimeReceiptState::Failed,
                EffectDisposition::Indeterminate,
                None,
            ));
        }
        // Slice #316-B3 (ADB baseline): the vendor reports `running` a few seconds before
        // adbd answers. Still under the admission guard, a bound Start succeeds only once the
        // ADB baseline answers; a timeout is a non-fatal backend failure that records neither
        // `device.connected` nor a startup package.
        let adb_wait_ms = if action == EmulatorInstanceAction::Stop {
            0
        } else {
            match self.await_adb_baseline(&rebound) {
                Ok(waited_ms) => waited_ms,
                Err(error) => {
                    return Err(self.control_failed(
                        mode,
                        links,
                        record_action,
                        error,
                        RuntimeReceiptState::Failed,
                        EffectDisposition::Indeterminate,
                        None,
                    ));
                }
            }
        };
        // Workflow #369 H-4 (B1; review L-3): a started physical instance is prepared on the
        // operator's key; the key is renewed to the preparation's bound before the action is
        // validated, so a refused renewal is the action's failure, as an unready ADB is.
        let prepares = mode.operator_key().is_some()
            && action != EmulatorInstanceAction::Stop
            && rebound.device_self_checked();
        if prepares && let Err(refused) = mode.renew(self, CONNECTION_PREPARATION_WAIT_MS) {
            return Err(self.renewal_refused(
                mode,
                links,
                record_action,
                instance_id,
                refused,
                EffectDisposition::Indeterminate,
            ));
        }
        let validated = self.append_event(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links.clone(),
            CommandPayloadDraft::validated(
                event_action,
                EffectDisposition::Performed,
                AuditInput::new(),
            ),
        )?;
        let terminal_event = terminal(&validated);
        if action != EmulatorInstanceAction::Stop {
            self.append_event(
                EventSeverity::Info,
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
                links.clone(),
                instance_bound_payload(&rebound),
            )?;
        }
        self.record_device_connected(instance_id, action, outcome.running, terminal_event)?;
        // Workflow #317 sc3 (b), #316 goal 5: a started physical instance is connected and
        // self-checked at once; it stays unavailable until that self-check passes. A failed
        // preparation is recorded and does not fail the completed control action; Workflow #369
        // H-1, P1: the ladder it admitted goes back with the result, and the control's release
        // hands the operator's key straight to it. Workflow #369-1: the ladder's emulator restart
        // waits for readiness itself (`recovery_ladder`), so nothing is prepared or scheduled
        // here on that path.
        let Some(key) = mode.operator_key() else {
            return Ok(EmulatorControlDriven {
                outcome,
                adb_wait_ms,
                stop_ms: 0,
                terminal: terminal_event,
                startup_package: None,
                ladder: None,
            });
        };
        let (preparation_failed, ladder) = if prepares {
            let (check, ladder) = self
                .prepare_instance_connection(
                    &rebound.instance_alias,
                    instance_id,
                    links.clone(),
                    admission,
                    &key.token,
                    key.connection_id,
                )
                .map_err(RequestFailure::poison_without_terminal)?;
            (
                !check.capture.ok || !check.touch.ok || check.failure_code.is_some(),
                ladder,
            )
        } else {
            (false, None)
        };
        let startup_package = if action == EmulatorInstanceAction::Stop || preparation_failed {
            None
        } else {
            self.prepare_startup_package(&rebound, links, control_request_id)?
        };
        Ok(EmulatorControlDriven {
            outcome,
            adb_wait_ms,
            stop_ms: 0,
            terminal: terminal_event,
            startup_package,
            ladder,
        })
    }

    /// Workflow #369 Q-5: an operator control whose renewal was refused (`lease_expired`,
    /// `lease_missing`) ends its hold with the existing `command.rejected` + `runtime.failed`
    /// pair, carrying the renewal's own code; a failure the Runtime does not survive is
    /// returned as it is.
    fn renewal_refused(
        &self,
        mode: &ControlMode<'_>,
        links: EventLinksDraft,
        record_action: EventAction,
        instance_id: InstanceId,
        refused: RequestFailure,
        effect: EffectDisposition,
    ) -> ControlFailure {
        if refused.poison_runtime || refused.error.is_fatal() {
            return ControlFailure::Request(refused);
        }
        let code = refused.error.projection().code;
        let mut error = RuntimeHostError::request(code, CONTROL_OPERATION, code);
        error.lifecycle.instance_id = Some(instance_id);
        self.control_failed(
            mode,
            links,
            record_action,
            error,
            RuntimeReceiptState::Failed,
            effect,
            None,
        )
    }

    /// A failed control action: an operator restart's phase returns a failure the Runtime
    /// survives unrecorded; every other one is recorded now (`command.rejected` +
    /// `runtime.failed` under `record_action`).
    #[allow(clippy::too_many_arguments)]
    fn control_failed(
        &self,
        mode: &ControlMode<'_>,
        links: EventLinksDraft,
        record_action: EventAction,
        error: RuntimeHostError,
        state: RuntimeReceiptState,
        effect: EffectDisposition,
        provider: Option<ProviderControlFailure>,
    ) -> ControlFailure {
        if mode.restart_phase() && !error.is_fatal() {
            return ControlFailure::Unrecorded(Box::new(UnrecordedControl {
                error,
                state,
                effect,
                provider,
            }));
        }
        match self.emulator_control_failure(links, record_action, error, state, effect) {
            Ok(failure) | Err(failure) => ControlFailure::Request(failure),
        }
    }

    /// Records a failure as it stands (the code it carries, under `record_action`).
    fn record_unrecorded(
        &self,
        links: EventLinksDraft,
        record_action: EventAction,
        failure: UnrecordedControl,
    ) -> RequestFailure {
        match self.emulator_control_failure(
            links,
            record_action,
            failure.error,
            failure.state,
            failure.effect,
        ) {
            Ok(failure) | Err(failure) => failure,
        }
    }

    /// A restart whose Stop's provider control failed: `emulator_restart_stop_failed` whatever
    /// the Start did (F4), with the last state known.
    fn report_stop_failed(
        &self,
        links: &EventLinksDraft,
        instance_id: InstanceId,
        stop: &WrappedFailure,
        stop_ms: u64,
        start: StartAfterStop<'_>,
    ) -> RequestFailure {
        let (start_ms, state, effect) = match start {
            StartAfterStop::NotDispatched => {
                (None, ObservedState::after(&stop.provider), stop.effect)
            }
            StartAfterStop::Confirmed(started, start_ms) => (
                Some(start_ms),
                Some(ObservedState::running(&started.outcome)),
                EffectDisposition::Indeterminate,
            ),
            StartAfterStop::Failed(state, start_ms) => {
                (Some(start_ms), state, EffectDisposition::Indeterminate)
            }
        };
        let report = RestartReport {
            code: HostCode::EmulatorRestartStopFailed,
            phase: "stop",
            stop_result: stop_result_token(failure_stage(&stop.provider.failure)),
            stop_ms,
            start_ms,
            state,
            device_process_present: stop
                .provider
                .failure
                .device_process_present
                .unwrap_or(DeviceProcessPresence::Unknown),
            failure: stop,
            effect,
        };
        self.record_restart_report(links, instance_id, &report)
    }

    /// The one failure record of a restart (F4, D2): `command.rejected` under the restart's
    /// action, the receipt's terminal, and one `runtime.failed` whose outcome carries the
    /// report's values and whose native detail is the failing phase's vendor output.
    fn record_restart_report(
        &self,
        links: &EventLinksDraft,
        instance_id: InstanceId,
        report: &RestartReport<'_>,
    ) -> RequestFailure {
        let provider = &report.failure.provider;
        let diagnostic = provider.failure.error.diagnostic();
        let stage = failure_stage(&provider.failure);
        let category = diagnostic.map_or("child_exit", |diagnostic| diagnostic.category().as_str());
        // Ledger revision 3: the report's tokens are the outcome's values and the failing
        // phase's control code its `caused_by` link (they were the detail's message).
        let error = RuntimeHostError::request(
            report.code,
            CONTROL_OPERATION,
            RuntimeErrorCode::BackendOperationFailed,
        )
        .with_outcome_values(|outcome| restart_report_outcome(outcome, report))
        .with_diagnostic_detail(DiagnosticDetailDraft::new(
            category,
            stage,
            "mumu_manager",
            ContractLocation::ControlInstance.as_str(),
        ));
        // Acceptance H-M1: the control command's exit status is MuMuManager's, not an OS error.
        let mut error = control_output(error, &provider.failure);
        error.lifecycle.caused_by =
            Some(Outcome::new(report.failure.code).operation(CONTROL_OPERATION));
        error.lifecycle.instance_id = Some(instance_id);
        match self.emulator_control_failure(
            links.clone(),
            EmulatorInstanceAction::Restart.event_action(),
            error,
            RuntimeReceiptState::Failed,
            report.effect,
        ) {
            Ok(failure) | Err(failure) => failure,
        }
    }

    /// Workflow #381 B D6: a Stop confirmed by `info` alone, its process listing having
    /// failed, is recorded once at Warning (`emulator_stop_process_unverified`) under the
    /// request's links, before its `command.validated`.
    fn record_stop_process_unverified(
        &self,
        links: &EventLinksDraft,
        instance_id: InstanceId,
        stop_elapsed_ms: u64,
    ) -> Result<(), RequestFailure> {
        self.append_event(
            EventSeverity::Warning,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links.clone(),
            RuntimePayloadDraft::failed(
                DiagnosticCode::RuntimeDiagnostic,
                EffectDisposition::NotPerformed,
                Outcome::new(HostCode::EmulatorStopProcessUnverified)
                    .operation(ContractLocation::ControlInstance)
                    .instance_id(instance_token(instance_id))
                    .device_process_present(ProcessPresenceToken(DeviceProcessPresence::Unknown))
                    .stop_elapsed_ms(stop_elapsed_ms),
                DiagnosticDetailDraft::new(
                    "emulator_control",
                    HostLocation::MumuManagerProcessList,
                    "mumu_manager",
                    ContractLocation::ControlInstance.as_str(),
                ),
                AuditInput::new(),
            ),
        )?;
        Ok(())
    }

    /// Workflow #381 B F4 (review M3): applies the state the bounded read observed after a
    /// failed provider control. Stopped: the binding returns to pending, the self-check facts
    /// are dropped, `device.connected` is recorded false and the Stop invalidations follow.
    /// Running: the reported port is bound (a changed binding is recorded by
    /// `runtime.instance_bound`) and the self-check facts are dropped. No settled observation
    /// (the read failed, or the instance is between the two): when the vendor command ran
    /// (effect `indeterminate`) the self-check facts and `device.connected` are dropped,
    /// otherwise nothing changes.
    fn apply_failure_observation(
        &self,
        resolved: &RegisteredInstance,
        links: &EventLinksDraft,
        observed: Option<&InstanceState>,
        effect: EffectDisposition,
    ) -> RuntimeHostResult<()> {
        let instance_id = resolved.instance_id();
        match observed {
            Some(state) if !state.process_started => {
                self.bind_observed_endpoint(instance_id, None)?;
                self.invalidate_backend_selfcheck_facts(instance_id)?;
                self.record_observed_stopped(instance_id)
            }
            Some(state) if state.running() => {
                let bound_port = lock(
                    &self.registered_instances,
                    HostLocation::ReadObservedBinding,
                )?
                .get(&instance_id)
                .and_then(|record| record.bound_adb_endpoint().map(|endpoint| endpoint.port()));
                if let Some(port) = state.adb_port.filter(|port| bound_port != Some(*port)) {
                    let rebound = self.bind_observed_endpoint(instance_id, Some(port))?;
                    self.append_event_raw(
                        EventSeverity::Info,
                        EventSource::Runtime,
                        OriginModule::Runtime,
                        EventActor::Runtime,
                        links.clone(),
                        instance_bound_payload(&rebound),
                    )?;
                }
                self.invalidate_backend_selfcheck_facts(instance_id)
            }
            _ if effect == EffectDisposition::Indeterminate => {
                self.invalidate_backend_selfcheck_facts(instance_id)?;
                self.invalidate_present_fact(instance_id, DEVICE_CONNECTED_FACT_KEY)
            }
            _ => Ok(()),
        }
    }

    /// `device.connected` recorded false for a stopped instance, then the Stop invalidations.
    fn record_observed_stopped(&self, instance_id: InstanceId) -> RuntimeHostResult<()> {
        let observed_at_unix_ms = self.clock.sample()?.unix_ms;
        self.record_runtime_fact(RuntimeFactRecord {
            scope: RuntimeFactScope::Instance { instance_id },
            key: DEVICE_CONNECTED_FACT_KEY.to_owned(),
            value: FactValue::Boolean(false),
            observed_at_unix_ms,
            source: OriginModule::Runtime,
            ttl_ms: None,
        })?;
        self.invalidate_stopped_facts(instance_id)
    }

    /// Workflow #369 H-3: the call-site fence of the ladder's own Stop and Start: the
    /// instance's lease is `own` (lease id and holder id), unexpired, with no destructive step
    /// open.
    fn own_key_admitted(
        &self,
        instance_id: InstanceId,
        own: &LeaseToken,
    ) -> RuntimeHostResult<bool> {
        let now = self.monotonic_ms()?;
        Ok(
            lock(&self.scheduler, HostLocation::FenceOwnEmulatorControl)?
                .active_lease(instance_id)
                .is_some_and(|lease| {
                    lease.token().lease_id() == own.lease_id()
                        && lease.token().holder_id() == own.holder_id()
                        && lease.token().expires_at_monotonic_ms() > now
                        && !lease.destructive_step_active()
                }),
        )
    }

    /// Polls the ADB baseline of the freshly bound endpoint until adbd answers `device`
    /// (`ADB_BASELINE_WAIT` at most, one probe every `ADB_BASELINE_POLL_INTERVAL`). Returns
    /// the milliseconds waited; the probes themselves are not recorded, the wait is part of
    /// the receipt's `elapsed_ms`. A timeout is `emulator_control_adb_not_ready`
    /// (`backend_operation_failed`) whose native detail carries the port, the wait and the
    /// last ADB error.
    fn await_adb_baseline(&self, rebound: &RegisteredInstance) -> RuntimeHostResult<u64> {
        let started = Instant::now();
        let deadline = started + ADB_BASELINE_WAIT;
        let stopped = || self.fatal.is_shutdown_requested();
        let mut last_error = None;
        loop {
            if stopped() || Instant::now() >= deadline {
                break;
            }
            match self.execution()?.probe_adb_baseline_until(
                &rebound.instance_alias,
                deadline,
                &stopped,
            ) {
                Ok(()) => {
                    if stopped() || Instant::now() >= deadline {
                        break;
                    }
                    return Ok(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX));
                }
                Err(error) => {
                    if error.resource_quiescence()
                        == Some(actingcommand_contract::ResourceQuiescence::Unconfirmed)
                    {
                        return Err(RuntimeHostError::execution(CONTROL_OPERATION, &error));
                    }
                    last_error = Some(error);
                    if Instant::now() >= deadline || stopped() {
                        break;
                    }
                    thread::sleep(
                        ADB_BASELINE_POLL_INTERVAL
                            .min(deadline.saturating_duration_since(Instant::now())),
                    );
                }
            }
        }
        let waited_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let mut error = RuntimeHostError::request(
            HostCode::EmulatorControlAdbNotReady,
            CONTROL_OPERATION,
            RuntimeErrorCode::BackendOperationFailed,
        )
        .with_native_detail(format!(
            "instance_alias={}; adb_port={}; waited_ms={waited_ms}; stopped={}; last_error={last_error:?}",
            rebound.instance_alias,
            rebound.bound_adb_endpoint().map_or_else(
                || "absent".to_owned(),
                |endpoint| endpoint.port().to_string()
            ),
            stopped()
        ));
        error.lifecycle.instance_id = Some(rebound.instance_id());
        Err(error)
    }

    /// Applies the control outcome to the discovery binding: `Start` binds the reported port,
    /// `Stop` returns the entry to pending. The registry and the host record change together
    /// under the registry lock (readers check identity under that lock), so the resolved
    /// instance is re-read from the registry, never assumed.
    fn rebind_instance_endpoint(
        &self,
        resolved: &RegisteredInstance,
        action: EmulatorInstanceAction,
        outcome: &EmulatorControlOutcome,
    ) -> RuntimeHostResult<RegisteredInstance> {
        let instance_id = resolved.instance_id();
        let binding = resolved
            .adb_endpoint
            .as_ref()
            .and_then(ResolvedInstanceEndpoint::discovered_binding);
        let state_confirmed = match action {
            EmulatorInstanceAction::Stop => !outcome.process_started,
            EmulatorInstanceAction::Start | EmulatorInstanceAction::Restart => {
                outcome.process_started && outcome.running
            }
        };
        if !state_confirmed
            || binding.is_none_or(|binding| binding.instance_index() != outcome.instance_index)
        {
            let mut error = RuntimeHostError::request(
                HostCode::EmulatorControlInstanceUnconfirmed,
                CONTROL_OPERATION,
                RuntimeErrorCode::BackendOperationFailed,
            );
            error.lifecycle.instance_id = Some(instance_id);
            return Err(error);
        }
        let adb_port = match action {
            EmulatorInstanceAction::Stop => None,
            EmulatorInstanceAction::Start | EmulatorInstanceAction::Restart => {
                let Some(adb_port) = outcome.adb_port.filter(|port| *port != 0) else {
                    let mut error = RuntimeHostError::request(
                        HostCode::EmulatorControlEndpointUnresolved,
                        CONTROL_OPERATION,
                        RuntimeErrorCode::BackendOperationFailed,
                    )
                    .with_native_detail(format!(
                        "running={} adb_port=absent: the started instance reported no ADB port, so the binding stays as it was",
                        outcome.running
                    ));
                    error.lifecycle.instance_id = Some(instance_id);
                    return Err(error);
                };
                Some(adb_port)
            }
        };
        self.bind_observed_endpoint(instance_id, adb_port)
    }

    /// Binds the instance's discovery binding to `adb_port`, or returns it to pending: the
    /// provider's binding and the host record together, under the registry lock.
    fn bind_observed_endpoint(
        &self,
        instance_id: InstanceId,
        adb_port: Option<u16>,
    ) -> RuntimeHostResult<RegisteredInstance> {
        let mut registry = lock(
            &self.registered_instances,
            HostLocation::RebindInstanceEndpoint,
        )?;
        let record = registry.get_mut(&instance_id).ok_or_else(|| {
            RuntimeHostError::fatal(
                HostCode::RuntimeInstanceRegistryIncomplete,
                HostLocation::RebindInstanceEndpoint,
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
        self.execution()?
            .rebind_discovered_endpoint(&record.instance_alias, adb_port)
            .map_err(|error| {
                let mut error =
                    RuntimeHostError::execution(HostLocation::RebindInstanceEndpoint, &error);
                error.lifecycle.instance_id = Some(instance_id);
                error
            })?;
        let rebound = self
            .execution()?
            .resolve(&record.instance_alias)
            .map_err(|error| {
                RuntimeHostError::execution(HostLocation::RebindInstanceEndpoint, &error)
            })?;
        record.audit_endpoint = rebound.audit_endpoint().to_owned();
        record.adb_endpoint = rebound.adb_endpoint().cloned();
        Ok(record.clone())
    }

    /// The program-fact producer: `device.connected` = the observed running state. After
    /// `Stop` the key is additionally invalidated with `device_closed`; an absent key is not
    /// an error there.
    fn record_device_connected(
        &self,
        instance_id: InstanceId,
        action: EmulatorInstanceAction,
        running: bool,
        terminal_event: TerminalEvent,
    ) -> Result<(), RequestFailure> {
        let scope = RuntimeFactScope::Instance { instance_id };
        let observed_at_unix_ms = self
            .clock
            .sample()
            .map_err(RequestFailure::poison_without_terminal)?
            .unix_ms;
        self.record_runtime_fact(RuntimeFactRecord {
            scope,
            key: DEVICE_CONNECTED_FACT_KEY.to_owned(),
            value: FactValue::Boolean(running),
            observed_at_unix_ms,
            source: OriginModule::Runtime,
            ttl_ms: None,
        })
        .map_err(|error| fact_failure(error, terminal_event))?;
        if action == EmulatorInstanceAction::Stop {
            self.invalidate_stopped_facts(instance_id)
                .map_err(|error| fact_failure(error, terminal_event))?;
        }
        Ok(())
    }

    /// A stopped instance has no foreground (#316-B3) and runs no task either: its
    /// `device.connected`, foreground and task facts are invalidated with `device_closed`.
    fn invalidate_stopped_facts(&self, instance_id: InstanceId) -> RuntimeHostResult<()> {
        for key in [
            DEVICE_CONNECTED_FACT_KEY,
            actingcommand_contract::APPLICATION_FOREGROUND_FACT_KEY,
            TASK_GAME_FACT_KEY,
            TASK_SERVER_FACT_KEY,
            TASK_PAGE_FACT_KEY,
        ] {
            self.invalidate_present_fact(instance_id, key)?;
        }
        Ok(())
    }

    /// Invalidates one instance fact with `device_closed`; an absent key is not an error.
    fn invalidate_present_fact(&self, instance_id: InstanceId, key: &str) -> RuntimeHostResult<()> {
        match self.invalidate_runtime_fact(
            &RuntimeFactScope::Instance { instance_id },
            key,
            RuntimeFactInvalidationReason::DeviceClosed,
        ) {
            Ok(_) => Ok(()),
            Err(error) if error.code() == HostCode::RuntimeFactMissing.as_str() => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Appends `command.rejected` then the `runtime.failed` record (exit code, native detail,
    /// primary detail) through the required-failure path; the rejected event is the terminal.
    fn emulator_control_failure(
        &self,
        links: EventLinksDraft,
        event_action: EventAction,
        error: RuntimeHostError,
        state: RuntimeReceiptState,
        effect: EffectDisposition,
    ) -> Result<RequestFailure, RequestFailure> {
        let diagnostic = match error.projection().code {
            RuntimeErrorCode::BackendOperationFailed => DiagnosticCode::BackendOperationFailed,
            RuntimeErrorCode::RuntimeBusy => DiagnosticCode::LeaseFencingDenied,
            _ => DiagnosticCode::RuntimeDiagnostic,
        };
        let rejected = self.append_event(
            EventSeverity::Error,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links.clone(),
            CommandPayloadDraft::rejected(event_action, diagnostic, effect, AuditInput::new()),
        )?;
        self.record_required_failure(&error, &rejected, links)
            .map_err(RequestFailure::poison_without_terminal)?;
        let terminal_event = terminal(&rejected);
        if error.is_fatal() {
            Ok(RequestFailure::poison(error, Some(terminal_event)))
        } else {
            Ok(RequestFailure::request(error, state, Some(terminal_event)))
        }
    }
}

/// Maps the typed device failure onto the host error: code, projection, receipt state and the
/// effect claim. The vendor output goes to `native_detail` only; the device observations are
/// the outcome's values and the primary detail keeps the stage (ledger revision 3).
fn emulator_control_error(
    failure: &EmulatorControlFailure,
    instance_id: InstanceId,
) -> (RuntimeHostError, RuntimeReceiptState, EffectDisposition) {
    let diagnostic = failure.error.diagnostic();
    let stage = failure_stage(failure);
    let category = diagnostic.map_or("child_exit", |diagnostic| diagnostic.category().as_str());
    let (code, runtime_code, state) = if stage == HostLocation::EmulatorControlBinding.location()
        || stage == HostLocation::EmulatorControlRegistration.location()
    {
        (
            HostCode::EmulatorControlUnavailable,
            RuntimeErrorCode::InvalidRequest,
            RuntimeReceiptState::Denied,
        )
    } else if stage == ExecutionKernelLocation::EmulatorControlSupport.location() {
        (
            HostCode::EmulatorControlUnsupported,
            RuntimeErrorCode::InvalidRequest,
            RuntimeReceiptState::Denied,
        )
    } else if stage == DeviceStage::MumuManagerWaitTimeout.location() {
        (
            HostCode::EmulatorControlWaitTimeout,
            RuntimeErrorCode::BackendOperationFailed,
            RuntimeReceiptState::Failed,
        )
    } else {
        (
            HostCode::EmulatorControlFailed,
            RuntimeErrorCode::BackendOperationFailed,
            RuntimeReceiptState::Failed,
        )
    };
    // Once the vendor command has run, its effect on the emulator is not known.
    let effect = if failure.exit_code.is_some() {
        EffectDisposition::Indeterminate
    } else {
        EffectDisposition::NotPerformed
    };
    // Ledger revision 3: the device failure's observations are the outcome's values (they were
    // the detail's `key=value` message); the stage stays the detail's, the vendor output the
    // native detail.
    let error = RuntimeHostError::request(code, CONTROL_OPERATION, runtime_code)
        .with_outcome_values(|outcome| {
            let outcome = outcome.elapsed_ms(failure.elapsed_ms);
            let outcome = match failure.exit_code {
                Some(exit_code) => outcome.exit_code(exit_code),
                None => outcome,
            };
            let outcome = match &failure.last_state {
                Some(state) => outcome
                    .process_started(state.process_started)
                    .android_started(state.android_started)
                    .adb_port_present(state.adb_port.is_some()),
                None => outcome,
            };
            // Workflow #381 B F2, F4 (coordinator answer D5): a Stop's process check, carried
            // by its failure record (on the ladder's rung, the one carrier of
            // `device_process_present=true`).
            match failure.device_process_present {
                Some(present) => outcome.device_process_present(ProcessPresenceToken(present)),
                None => outcome,
            }
        })
        .with_diagnostic_detail(DiagnosticDetailDraft::new(
            category,
            stage,
            "mumu_manager",
            ContractLocation::ControlInstance.as_str(),
        ));
    let mut error = control_output(error, failure);
    error.lifecycle.instance_id = Some(instance_id);
    (error, state, effect)
}

/// The failure's text as the error's native detail (ledger revision 3, acceptance H-M1): once
/// the control command exited, its output is a `foreign_mumu_manager_error` link with the exit
/// status as `exit_code` (0 for an errcode envelope, a launch error or the wait deadline);
/// without an exit the text is the device's, a `foreign_other_error` link. No exit status is
/// recorded as an OS error.
fn control_output(error: RuntimeHostError, failure: &EmulatorControlFailure) -> RuntimeHostError {
    match failure.exit_code {
        Some(exit_code) => error.with_mumu_manager_output(&failure.output_summary, exit_code),
        None => error.with_native_detail(failure.output_summary.clone()),
    }
}

/// The values of a failed restart's outcome (F4.1; ledger revision 3).
fn restart_report_outcome(outcome: Outcome, report: &RestartReport<'_>) -> Outcome {
    let outcome = outcome
        .restart_phase(RestartPhaseToken(report.phase))
        .stop_elapsed_ms(report.stop_ms)
        .start_dispatched(report.start_ms.is_some())
        .device_process_present(ProcessPresenceToken(report.device_process_present));
    let outcome = match report.stop_result {
        Some(token) => outcome.stop_result(token),
        None => outcome,
    };
    let outcome = match report.start_ms {
        Some(start_ms) => outcome.start_elapsed_ms(start_ms),
        None => outcome,
    };
    match report.state {
        Some(state) => outcome
            .process_started(state.process_started)
            .android_started(state.android_started)
            .adb_port_present(state.adb_port_present),
        None => outcome,
    }
}

/// The busy denial of the fence (`fence=<reason>` as native detail).
fn busy_error(
    instance_id: InstanceId,
    fence: MonitorRecoveryCoordinationReason,
) -> RuntimeHostError {
    let mut error = RuntimeHostError::request(
        HostCode::EmulatorControlBusy,
        CONTROL_OPERATION,
        RuntimeErrorCode::RuntimeBusy,
    )
    .with_native_detail(format!("fence={}", fence_reason_token(fence)));
    error.lifecycle.instance_id = Some(instance_id);
    error
}

fn fact_failure(error: RuntimeHostError, terminal_event: TerminalEvent) -> RequestFailure {
    if error.is_fatal() {
        RequestFailure::poison(error, Some(terminal_event))
    } else {
        RequestFailure::request(error, RuntimeReceiptState::Failed, Some(terminal_event))
    }
}

/// The fence reason's token (its snake_case name; a vocabulary token, not an outcome code).
const fn fence_reason_token(reason: MonitorRecoveryCoordinationReason) -> &'static str {
    match reason {
        MonitorRecoveryCoordinationReason::SchedulerAvailable => "scheduler_available",
        MonitorRecoveryCoordinationReason::ActiveLease => "active_lease",
        MonitorRecoveryCoordinationReason::LeaseExpired => RuntimeErrorCode::LeaseExpired.as_str(),
        MonitorRecoveryCoordinationReason::DestructiveStepActive => "destructive_step_active",
        MonitorRecoveryCoordinationReason::PreemptionPending => "preemption_pending",
        MonitorRecoveryCoordinationReason::TakeoverCooldown => "takeover_cooldown",
        MonitorRecoveryCoordinationReason::QueuedLeaseRequests => "queued_lease_requests",
    }
}
