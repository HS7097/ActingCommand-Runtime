// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #479 F-3a (register §1.486; contracts/device-binding.md, "An iOS handset"; work order
// rd-1119/brief-479-f3a-ios-capture-backend.md, its tests 2, 3, 5 and 6 through the host): a bound
// iOS handset on the registry's own registration, its frames taken from a scripted `acib` behind
// the registration's launcher, as F-2 scripted `acib` in the Lab. The read-only observe and the
// capture sequence open and answer `acib_screen_capture`; the status's frame row says so; an
// input, an application launch, a self-check and a drill stay refused with their codes; a black
// picture answers its own cause and closes, fences and stores nothing; a child that cannot be ended
// fences its instance alone while another instance observes; a refusal of `acib` answers its cause
// and its errors reach no record or receipt. The scripted child's time is its own, so no clock is
// read here. The key is a neutral fake; no identifier of a phone exists anywhere here.
//
// Fix round 1 (rd-1119/brief-479-f3a-fix1.md, items 3 and 5; the review's must-fix 3): a run's
// capture on a bound iOS handset is refused with `device_system_backend_missing` before it is
// requested, so no geometry record is written; a monitor's configuration is refused the same way;
// the capture of `ObserveContainedPage` opens on `acib`; an unbound iOS handset's observe answers
// what an unbound Android handset's does; a capture failure that carries the mark of a refused
// picture beside an unconfirmed resource is an unconfirmed resource (its session is closed, its
// instance fenced). The run and the last test are on the host's fake provider (a run's capture is
// refused before any provider is asked; the fake answers a failure the test builds).

use super::gate_369_s6c::{instance_status, send};
use super::gate_398_rd1b::resolved_handset;
use super::*;
use crate::codes::HostCode;
use actingcommand_contract::codes::{DeviceSelfCheckFailure, SharedCode};
use actingcommand_contract::{
    CaptureSequenceSpec, ContainedObservationRequest, DeviceBoundVia, EmulatorCapability,
    EmulatorCapabilityAvailability, EmulatorCapabilityImplementation, RecoveryDrillMode,
    Sha256Fingerprint,
};
use actingcommand_device::{
    AcibBounds, AcibCaptureSource, AcibCommand, AcibKeySalt, AcibLauncher, AcibProcess, AdbConfig,
    CaptureBackendChoice, CaptureBackendConfig, DeviceResourceCloseOutcome,
    DeviceResourceClosePhase, DeviceResourceKind, DeviceResourceQuiescence, DeviceSerial,
    DeviceTarget, FrameRefusal, MaaTouchConfig, TouchBackendChoice, TouchBackendConfig,
};
// `VecDeque`, `Cursor`, `AtomicUsize`, `Ordering`, `Duration`, `Arc` and the test support come
// from the parent's imports.
use std::io::Read;
use std::sync::Mutex;

const KEY: &str = "sha256:479f3a00000000000000000000000000000000000000000000000000000f3a02";
const SALT: &str = "neutral-installation-secret-f3a";
const STDERR_MARKER: &str = "NEUTRAL-STDERR-MARKER-F3A-HOST";

fn bound_ios() -> HandsetEndpoint {
    HandsetEndpoint::Ios(Some(KeyedHandsetBinding::new(
        Sha256Fingerprint::stored(KEY).expect("a key of the fingerprint's form"),
        1_700_000_000_479,
        DeviceBoundVia::Cli,
    )))
}

/// The registration of the handset `alias` with stand-in backends on the stand-in target of an
/// unbound handset and an adb of a neutral name that never runs, as actingd registers an iOS
/// handset, or an Android handset the private binding file binds no device to.
fn stand_in_registration(alias: &str, instance_id: InstanceId) -> ExecutionBackendRegistration {
    let target = DeviceTarget {
        serial: Some(DeviceSerial::unbound_handset(alias)),
        port: 0,
        connect: false,
        ..DeviceTarget::default()
    };
    let adb = AdbConfig {
        adb_path: "neutral-adb".to_owned(),
        ..AdbConfig::default()
    };
    ExecutionBackendRegistration::new(
        alias,
        instance_id,
        "neutral.application",
        TouchBackendConfig::new(adb.clone(), target.clone(), MaaTouchConfig::default())
            .with_requested(TouchBackendChoice::AdbShellInput),
        CaptureBackendConfig::new(adb, target).with_requested(CaptureBackendChoice::Adb),
    )
    .expect("handset registration")
}

/// The registration of the bound iOS handset `alias`, as actingd registers one (stand-in backends
/// that open nothing), its frames from `launcher`.
fn ios_registration(
    alias: &str,
    instance_id: InstanceId,
    launcher: Arc<ScriptedAcib>,
) -> ExecutionBackendRegistration {
    ios_registration_as(alias, instance_id, bound_ios(), launcher)
}

/// The registration of the iOS handset `alias` as `handset` (bound or not), its frames from
/// `launcher`.
fn ios_registration_as(
    alias: &str,
    instance_id: InstanceId,
    handset: HandsetEndpoint,
    launcher: Arc<ScriptedAcib>,
) -> ExecutionBackendRegistration {
    let source = AcibCaptureSource::new(
        Some(std::env::current_exe().expect("an existing file as the program")),
        AcibKeySalt::new(SALT),
    )
    .with_bounds(AcibBounds {
        wait: Duration::from_millis(100),
        ..AcibBounds::runtime()
    })
    .with_launcher(launcher);
    stand_in_registration(alias, instance_id)
        .with_handset(handset)
        .with_ios_capture(source)
}

// ---- a PNG written by the format's rules: RGB, 8 bits, an sRGB chunk ----

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFF_u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = (data.len() as u32).to_be_bytes().to_vec();
    let mut body = kind.to_vec();
    body.extend_from_slice(data);
    out.extend_from_slice(&body);
    out.extend_from_slice(&crc32(&body).to_be_bytes());
    out
}

/// A 2 x 1 8-bit sRGB PNG of `samples` (six), its data one stored deflate block.
fn picture(samples: [u8; 6]) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    out.extend(chunk(b"IHDR", &[0, 0, 0, 2, 0, 0, 0, 1, 8, 2, 0, 0, 0]));
    out.extend(chunk(b"sRGB", &[0]));
    let mut raw = vec![0];
    raw.extend_from_slice(&samples);
    let (mut a, mut b) = (1_u32, 0_u32);
    for byte in &raw {
        a = (a + u32::from(*byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    let mut zlib = vec![0x78, 0x01, 1];
    zlib.extend_from_slice(&(raw.len() as u16).to_le_bytes());
    zlib.extend_from_slice(&(!(raw.len() as u16)).to_le_bytes());
    zlib.extend_from_slice(&raw);
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());
    out.extend(chunk(b"IDAT", &zlib));
    out.extend(chunk(b"IEND", &[]));
    out
}

/// `acib screenshot`'s answer of `png` on the standard output.
fn answer(png: &[u8]) -> Vec<u8> {
    let mut out = format!(
        "{{\"command\":\"screenshot\",\"device\":3,\"via\":\"screen_capture\",\"format\":\"png\",\"width\":2,\"height\":1,\"bytes\":{},\"elapsed_ms\":{{\"request\":401,\"command\":977}},\"out\":\"-\"}}\n",
        png.len()
    )
    .into_bytes();
    out.extend_from_slice(png);
    out
}

// ---- the scripted acib ----

/// One scripted child: its output (with a marker on its errors), whether it ends by itself
/// (`Some(success)`) or hangs (`None`), and whether its end can be confirmed.
struct Child {
    output: Vec<u8>,
    status: Option<bool>,
    end_confirmed: bool,
}

impl Child {
    fn answering(output: Vec<u8>, success: bool) -> Self {
        Self {
            output,
            status: Some(success),
            end_confirmed: true,
        }
    }
}

#[derive(Default)]
struct ScriptedAcib {
    children: Mutex<VecDeque<Child>>,
    launches: AtomicUsize,
    arguments: Mutex<Vec<Vec<String>>>,
}

impl ScriptedAcib {
    fn with(children: Vec<Child>) -> Arc<Self> {
        Arc::new(Self {
            children: Mutex::new(children.into()),
            ..Self::default()
        })
    }

    fn launches(&self) -> usize {
        self.launches.load(Ordering::Acquire)
    }
}

impl AcibLauncher for ScriptedAcib {
    fn launch(&self, command: &AcibCommand) -> std::io::Result<Box<dyn AcibProcess>> {
        self.launches.fetch_add(1, Ordering::AcqRel);
        self.arguments
            .lock()
            .expect("arguments")
            .push(command.arguments.clone());
        let child = self
            .children
            .lock()
            .expect("children")
            .pop_front()
            .expect("a child for every launch");
        Ok(Box::new(Running {
            child: Some(child),
            ended: false,
            waited: Duration::ZERO,
        }))
    }
}

struct Running {
    child: Option<Child>,
    ended: bool,
    waited: Duration,
}

impl Running {
    fn child(&self) -> &Child {
        self.child.as_ref().expect("the scripted child")
    }
}

impl AcibProcess for Running {
    fn output(&mut self) -> Option<Box<dyn Read + Send>> {
        let output = std::mem::take(&mut self.child.as_mut()?.output);
        Some(Box::new(Cursor::new(output)))
    }

    fn errors(&mut self) -> Option<Box<dyn Read + Send>> {
        Some(Box::new(Cursor::new(STDERR_MARKER.as_bytes().to_vec())))
    }

    fn ended(&mut self) -> std::io::Result<Option<bool>> {
        Ok(if self.ended {
            Some(false)
        } else {
            self.child().status
        })
    }

    fn elapsed(&self) -> Duration {
        self.waited
    }

    fn pause(&mut self, span: Duration) -> std::io::Result<()> {
        self.waited += span;
        Ok(())
    }

    fn end(&mut self) -> actingcommand_device::DeviceResult<DeviceResourceCloseOutcome> {
        if self.child().end_confirmed {
            self.ended = true;
            return Ok(DeviceResourceCloseOutcome::confirmed(1));
        }
        Err(
            actingcommand_device::DeviceError::fatal("scripted acib child did not exit")
                .with_resource_close_cause(
                    DeviceResourceKind::ExternalChild,
                    DeviceResourceClosePhase::Deadline,
                    "acib",
                    None,
                    None,
                    DeviceResourceQuiescence::Unconfirmed,
                    1,
                ),
        )
    }
}

/// A host over the registry of `registrations`.
fn host_of(root: &TempDir, registrations: Vec<ExecutionBackendRegistration>) -> RuntimeHost {
    let registry = ExecutionBackendRegistry::new(registrations).expect("the registry");
    RuntimeHost::start(config(root), Arc::new(registry)).expect("runtime host")
}

fn observe(host: &RuntimeHost, alias: &str) -> RuntimeReceipt {
    send(
        host,
        RuntimeOperation::ObserveReadonly {
            instance_alias: alias.to_owned(),
        },
        4796,
    )
}

/// Whether `receipt`'s outcome names `code` as a cause, and that cause.
fn cause_of<'a>(
    receipt: &'a RuntimeReceipt,
    code: &str,
) -> Option<&'a actingcommand_contract::outcome::Outcome> {
    receipt
        .outcome()
        .causes()
        .iter()
        .find(|link| {
            link.relation().as_str() == "caused_by" && link.outcome().code().as_str() == code
        })
        .map(|link| link.outcome())
}

#[track_caller]
fn refused_without_backend(receipt: &RuntimeReceipt, code: &str) {
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied, "{receipt:?}");
    assert_eq!(receipt.outcome().code().as_str(), code, "{receipt:?}");
    assert!(
        cause_of(receipt, HostCode::DeviceSystemBackendMissing.as_str()).is_some(),
        "{receipt:?}"
    );
}

fn events_of(host: &RuntimeHost) -> Vec<PersistedEvent> {
    host.query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events")
}

fn count(events: &[PersistedEvent], kind: EventType) -> usize {
    events
        .iter()
        .filter(|event| event.event_type() == kind)
        .count()
}

/// Test 6: the read-only observe and a capture sequence of a bound iOS handset open and answer
/// `acib_screen_capture`, one child per frame, each named by the handset's key; the status's frame
/// row is implemented by `acib_screen_capture`, every other row refused from `device_system:ios`.
#[test]
fn f3a_h1_the_observations_of_a_bound_ios_handset_open_on_acib() {
    const ALIAS: &str = "neutral.iphone";
    let root = TempDir::new().expect("tempdir");
    let png = picture([10, 20, 30, 40, 50, 60]);
    let acib = ScriptedAcib::with(
        (0..3)
            .map(|_| Child::answering(answer(&png), true))
            .collect(),
    );
    let host = host_of(
        &root,
        vec![ios_registration(ALIAS, instance_id(), Arc::clone(&acib))],
    );

    let observed = observe(&host, ALIAS);
    let Some(RuntimeResult::ReadonlyObservationCompleted { observation }) = observed.result()
    else {
        panic!("a read-only observation: {observed:?}");
    };
    assert_eq!(
        observation.capture_backend(),
        RuntimeCaptureBackend::AcibScreenCapture
    );
    assert_eq!((observation.width(), observation.height()), (2, 1));

    let sequence = send(
        &host,
        RuntimeOperation::CaptureSequence {
            instance_alias: ALIAS.to_owned(),
            spec: CaptureSequenceSpec::new(2, 0).expect("a sequence of two"),
        },
        4797,
    );
    let Some(RuntimeResult::CaptureSequenceCompleted { sequence }) = sequence.result() else {
        panic!("a capture sequence: {sequence:?}");
    };
    assert_eq!(sequence.observations().len(), 2);
    assert!(
        sequence
            .observations()
            .iter()
            .all(|frame| frame.capture_backend() == RuntimeCaptureBackend::AcibScreenCapture)
    );
    assert_eq!(acib.launches(), 3, "one child per frame");
    assert!(
        acib.arguments
            .lock()
            .expect("arguments")
            .iter()
            .all(|arguments| arguments.iter().any(|argument| argument == KEY)
                && arguments.iter().all(|argument| !argument.contains(SALT)))
    );

    let status = instance_status(&host, ALIAS);
    let table = status.capabilities().expect("a capability table");
    for capability in EmulatorCapability::ALL {
        let row = table.evidence(capability);
        let expected = if capability == EmulatorCapability::CaptureFrame {
            (
                "acib_screen_capture",
                EmulatorCapabilityAvailability::Unverified,
                Some(EmulatorCapabilityImplementation::Supported),
            )
        } else {
            (
                "device_system:ios",
                EmulatorCapabilityAvailability::Unavailable,
                Some(EmulatorCapabilityImplementation::Unsupported),
            )
        };
        assert_eq!(
            (row.evidence_ref(), row.availability(), row.implementation()),
            expected,
            "{capability:?}"
        );
    }
    let events = events_of(&host);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
    assert_eq!(count(&events, EventType::CaptureCompleted), 3);
}

/// Test 6: on the same bound handset an input (a tap on a person's lease), an application launch,
/// a self-check and a drill stay refused with their codes caused by `device_system_backend_missing`,
/// and no child is started.
#[test]
fn f3a_h2_inputs_applications_a_self_check_and_a_drill_stay_refused() {
    const ALIAS: &str = "neutral.iphone";
    let root = TempDir::new().expect("tempdir");
    let acib = ScriptedAcib::with(Vec::new());
    let host = host_of(
        &root,
        vec![ios_registration(ALIAS, instance_id(), Arc::clone(&acib))],
    );
    let ids = IdentifierIssuer::new().expect("identifier issuer");

    let lease = send(
        &host,
        RuntimeOperation::acquire_lease(ALIAS, ids.mint_holder_id().expect("holder")),
        4798,
    );
    let Some(RuntimeResult::LeaseGranted { token }) = lease.result() else {
        panic!("a lease on the handset: {lease:?}");
    };
    let tapped = send(
        &host,
        RuntimeOperation::Input {
            token: token.clone(),
            action: InputAction::Tap { x: 10, y: 20 },
            frame: None,
        },
        4798,
    );
    refused_without_backend(
        &tapped,
        DeviceSelfCheckFailure::InputBackendOpenFailed
            .code()
            .as_str(),
    );
    send(
        &host,
        RuntimeOperation::ReleaseLease {
            token: token.clone(),
        },
        4798,
    );
    let launched = send(
        &host,
        RuntimeOperation::application_lifecycle(
            ALIAS,
            ids.mint_holder_id().expect("holder"),
            ApplicationLifecycleAction::Launch,
        ),
        4799,
    );
    refused_without_backend(
        &launched,
        SharedCode::ApplicationBackendOperationFailed
            .code()
            .as_str(),
    );
    let checked = send(
        &host,
        RuntimeOperation::SelfCheckInstance {
            instance_alias: ALIAS.to_owned(),
        },
        4800,
    );
    refused_without_backend(
        &checked,
        DeviceSelfCheckFailure::InputBackendOpenFailed
            .code()
            .as_str(),
    );
    let drilled = send(
        &host,
        RuntimeOperation::DrillRecoveryLadder {
            instance_alias: ALIAS.to_owned(),
            mode: RecoveryDrillMode::Plan,
        },
        4801,
    );
    refused_without_backend(&drilled, HostCode::RecoveryLadderDrillUnsupported.as_str());
    assert_eq!(acib.launches(), 0, "nothing was started");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

/// Test 2 through the host: a black picture fails the observe with `capture_backend_operation_failed`
/// caused by `capture_frame_all_black`; no artifact is stored and nothing is fenced, and the next
/// observe gives its frame.
#[test]
fn f3a_h3_a_black_picture_answers_its_cause_and_the_next_capture_succeeds() {
    const ALIAS: &str = "neutral.iphone";
    let root = TempDir::new().expect("tempdir");
    let acib = ScriptedAcib::with(vec![
        Child::answering(answer(&picture([0; 6])), true),
        Child::answering(answer(&picture([1, 2, 3, 4, 5, 6])), true),
    ]);
    let host = host_of(
        &root,
        vec![ios_registration(ALIAS, instance_id(), Arc::clone(&acib))],
    );
    let stored_before = count(&events_of(&host), EventType::ArtifactCreated);

    let black = observe(&host, ALIAS);
    assert_eq!(black.state(), RuntimeReceiptState::Failed, "{black:?}");
    assert_eq!(
        black.outcome().code().as_str(),
        SharedCode::CaptureBackendOperationFailed.code().as_str()
    );
    assert!(
        cause_of(&black, HostCode::CaptureFrameAllBlack.as_str()).is_some(),
        "{black:?}"
    );
    let after_black = events_of(&host);
    assert_eq!(
        count(&after_black, EventType::ArtifactCreated),
        stored_before,
        "nothing stored"
    );
    assert!(
        instance_status(&host, ALIAS).resource_fence().is_none(),
        "nothing fenced"
    );

    let next = observe(&host, ALIAS);
    assert_eq!(next.state(), RuntimeReceiptState::Completed, "{next:?}");
    assert_eq!(acib.launches(), 2);
    let events = events_of(&host);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
    assert_eq!(count(&events, EventType::CaptureFailed), 1);
    assert_eq!(count(&events, EventType::CaptureCompleted), 1);
}

/// Test 3 through the host: a child that hangs and cannot be ended (`acib_not_ended`) leaves its
/// capture's resources unconfirmed, which fences that handset alone; another iOS handset beside it
/// still observes, and the Runtime goes on.
#[test]
fn f3a_h4_a_child_that_cannot_be_ended_fences_its_instance_alone() {
    const STUCK: &str = "neutral.iphone.a";
    const OTHER: &str = "neutral.iphone.b";
    let root = TempDir::new().expect("tempdir");
    let stuck = ScriptedAcib::with(vec![Child {
        output: Vec::new(),
        status: None,
        end_confirmed: false,
    }]);
    let other = ScriptedAcib::with(vec![Child::answering(
        answer(&picture([7, 8, 9, 10, 11, 12])),
        true,
    )]);
    let host = host_of(
        &root,
        vec![
            ios_registration(STUCK, instance_id(), Arc::clone(&stuck)),
            ios_registration(OTHER, instance_id(), Arc::clone(&other)),
        ],
    );

    let failed = observe(&host, STUCK);
    assert_ne!(failed.state(), RuntimeReceiptState::Completed, "{failed:?}");
    assert!(
        instance_status(&host, STUCK).resource_fence().is_some(),
        "the handset whose child was not ended is fenced"
    );
    assert!(instance_status(&host, OTHER).resource_fence().is_none());
    let observed = observe(&host, OTHER);
    assert_eq!(
        observed.state(),
        RuntimeReceiptState::Completed,
        "{observed:?}"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

/// Tests 3 and 5 through the host: `acib`'s refusal (`not_paired`) answers
/// `capture_backend_operation_failed` caused by `capture_ios_failed`, its cause `ios_not_paired`
/// and `acib`'s word beside it; the child's errors (a marker) reach no record and no receipt.
#[test]
fn f3a_h5_a_refusal_of_acib_answers_its_cause_and_its_errors_reach_nothing() {
    const ALIAS: &str = "neutral.iphone";
    let root = TempDir::new().expect("tempdir");
    let acib = ScriptedAcib::with(vec![Child::answering(
        b"{\"command\":\"screenshot\",\"device\":3,\"via\":\"screen_capture\",\"problem\":\"not_paired\"}\n"
            .to_vec(),
        false,
    )]);
    let host = host_of(
        &root,
        vec![ios_registration(ALIAS, instance_id(), Arc::clone(&acib))],
    );

    let refused = observe(&host, ALIAS);
    assert_eq!(refused.state(), RuntimeReceiptState::Failed, "{refused:?}");
    assert_eq!(
        refused.outcome().code().as_str(),
        SharedCode::CaptureBackendOperationFailed.code().as_str()
    );
    let cause = cause_of(&refused, HostCode::CaptureIosFailed.as_str())
        .unwrap_or_else(|| panic!("capture_ios_failed: {refused:?}"));
    assert_eq!(
        cause.values().get("ios_capture_problem"),
        Some(&serde_json::json!("ios_not_paired"))
    );
    assert_eq!(
        cause.values().get("acib_problem"),
        Some(&serde_json::json!("not_paired"))
    );
    let events = events_of(&host);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
    assert!(!format!("{refused:?}").contains(STDERR_MARKER));
    for event in &events {
        let recorded = format!("{:?}", event.payload());
        assert!(!recorded.contains(STDERR_MARKER), "{recorded}");
        assert!(!recorded.contains(SALT), "{recorded}");
    }
}

// ---- fix round 1 ----

/// Whether `outcome` is `code` or names it among its causes, at any depth.
fn names(outcome: &actingcommand_contract::outcome::Outcome, code: &str) -> bool {
    outcome.code().as_str() == code
        || outcome
            .causes()
            .iter()
            .any(|link| names(link.outcome(), code))
}

/// The codes of `outcome`'s causes, at every depth, in order.
fn cause_codes(outcome: &actingcommand_contract::outcome::Outcome) -> Vec<String> {
    outcome
        .causes()
        .iter()
        .flat_map(|link| {
            std::iter::once(link.outcome().code().as_str().to_owned())
                .chain(cause_codes(link.outcome()))
        })
        .collect()
}

/// Fix round 1, item 3 (test 6): a run's capture on a bound iOS handset is refused with
/// `capture_backend_open_failed` caused by `device_system_backend_missing` before the capture is
/// requested: the run fails, nothing is captured, and no geometry record is written. This refusal
/// is what keeps `acib_screen_capture` out of a run's geometry record in the ledger.
#[test]
fn f3a_fix1_h6_a_runs_capture_on_a_bound_ios_handset_is_refused() {
    const ALIAS: &str = "neutral.iphone";
    let root = TempDir::new().expect("tempdir");
    let instance_id = instance_id();
    let state = Arc::new(FakeState::default());
    let package = super::gate_369s5::direct_run_package(&root, &state);
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(
            FakeProvider::one(ALIAS, instance_id, Arc::clone(&state))
                .with_resolved_for(ALIAS, resolved_handset(ALIAS, instance_id, bound_ios())),
        ),
    )
    .expect("runtime host");
    let ids = IdentifierIssuer::new().expect("identifier issuer");

    let run = send(
        &host,
        RuntimeOperation::run_contained_task(
            ALIAS,
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package).expect("task request"),
        ),
        4810,
    );
    assert_eq!(run.state(), RuntimeReceiptState::Failed, "{run:?}");
    assert!(
        names(
            run.outcome(),
            DeviceSelfCheckFailure::CaptureBackendOpenFailed
                .code()
                .as_str()
        ),
        "{run:?}"
    );
    assert!(
        names(run.outcome(), HostCode::DeviceSystemBackendMissing.as_str()),
        "{run:?}"
    );
    assert_eq!(
        state.capture_count.load(Ordering::Acquire),
        0,
        "nothing was captured"
    );
    let events = events_of(&host);
    assert_eq!(count(&events, EventType::CaptureRequested), 0);
    assert_eq!(count(&events, EventType::TaskGeometryObserved), 0);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

/// Fix round 1, item 3 (test 6): a monitor's configuration on a bound iOS handset is refused with
/// `capture_backend_open_failed` caused by `device_system_backend_missing` (a monitor captures on
/// its own at each probe), and no child is started.
#[test]
fn f3a_fix1_h7_a_monitors_configuration_on_a_bound_ios_handset_is_refused() {
    const ALIAS: &str = "neutral.iphone";
    let root = TempDir::new().expect("tempdir");
    let acib = ScriptedAcib::with(Vec::new());
    let host = host_of(
        &root,
        vec![ios_registration(ALIAS, instance_id(), Arc::clone(&acib))],
    );

    let configured = send(
        &host,
        RuntimeOperation::ConfigureMonitor {
            instance_alias: ALIAS.to_owned(),
            policy: RuntimeMonitorPolicy::new(1_000, "home", false).expect("monitor policy"),
        },
        4811,
    );
    refused_without_backend(
        &configured,
        DeviceSelfCheckFailure::CaptureBackendOpenFailed
            .code()
            .as_str(),
    );
    assert_eq!(acib.launches(), 0, "nothing was started");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

/// An observation package with one page, `neutral/home`, recognised by the colour `home` of the
/// frame's first pixel, written into `root`.
fn observation_package(root: &TempDir, home: [u8; 3]) -> PathBuf {
    let entries = [
        (
            "control.json",
            serde_json::json!({"game": "neutral", "server": "test", "package_id": "neutral.f3a",
                               "entry_task_id": "task"}),
        ),
        (
            "resources/manifest.json",
            serde_json::json!({"entry_task_id": "task"}),
        ),
        (
            "resources/operations/task/task.json",
            serde_json::json!({"task_id": "task"}),
        ),
        (
            "resources/recognition/neutral.test.pack.json",
            serde_json::json!({
                "schema_version": "0.7",
                "game": "neutral",
                "server": "test",
                "coordinate_space": {"width": 2, "height": 1},
                "defaults": {"color_max_distance": 0.0},
                "targets": [{"type": "color", "id": "page/home",
                             "region": {"x": 0, "y": 0, "width": 1, "height": 1},
                             "expected": home}],
            }),
        ),
        (
            "resources/recognition/neutral.test.pages.json",
            serde_json::json!({"schema_version": "0.6",
                               "pages": [{"id": "neutral/home", "required": ["page/home"]}]}),
        ),
        (
            "resources/navigation/neutral.test.navigation.json",
            serde_json::json!({"navigation": []}),
        ),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for (path, value) in &entries {
        zip.start_file(*path, FileOptions::default())
            .expect("zip entry");
        zip.write_all(&serde_json::to_vec(value).expect("JSON"))
            .expect("zip bytes");
    }
    let path = root.path().join("observation.zip");
    fs::write(&path, zip.finish().expect("finish zip").into_inner()).expect("write package");
    path
}

/// Fix round 1, item 3 (test 6): the capture of `ObserveContainedPage` on a bound iOS handset
/// opens on `acib`: the observation is answered, its frame of `acib_screen_capture`, one child for
/// its one frame.
#[test]
fn f3a_fix1_h8_the_capture_of_a_contained_page_observation_opens_on_acib() {
    const ALIAS: &str = "neutral.iphone";
    let root = TempDir::new().expect("tempdir");
    let package = observation_package(&root, [10, 20, 30]);
    let acib = ScriptedAcib::with(vec![Child::answering(
        answer(&picture([10, 20, 30, 40, 50, 60])),
        true,
    )]);
    let host = host_of(
        &root,
        vec![ios_registration(ALIAS, instance_id(), Arc::clone(&acib))],
    );

    let observed = send(
        &host,
        RuntimeOperation::ObserveContainedPage {
            instance_alias: ALIAS.to_owned(),
            request: ContainedObservationRequest::new(package.display().to_string(), Vec::new())
                .expect("observation request"),
        },
        4812,
    );
    assert_eq!(
        observed.state(),
        RuntimeReceiptState::Observed,
        "{observed:?}"
    );
    let Some(RuntimeResult::ContainedPageObserved { observation }) = observed.result() else {
        panic!("a page observation: {observed:?}");
    };
    assert_eq!(
        observation.frame.capture_backend(),
        RuntimeCaptureBackend::AcibScreenCapture
    );
    assert_eq!(acib.launches(), 1, "one child for its one frame");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

/// Fix round 1, item 3 (test 6): an unbound iOS handset's read-only observe answers what an
/// unbound Android handset's answers on the same host (the state, the code, the codes of its
/// causes, its stage and its capture open cause), and no child is started.
#[test]
fn f3a_fix1_h9_an_unbound_ios_handset_answers_what_an_unbound_handset_answers() {
    const IPHONE: &str = "neutral.iphone";
    const PHONE: &str = "neutral.phone";
    let root = TempDir::new().expect("tempdir");
    let acib = ScriptedAcib::with(Vec::new());
    let registry = ExecutionBackendRegistry::new([
        ios_registration_as(
            IPHONE,
            instance_id(),
            HandsetEndpoint::Ios(None),
            Arc::clone(&acib),
        ),
        stand_in_registration(PHONE, instance_id()).with_handset(HandsetEndpoint::Unbound),
    ])
    .expect("the registry");
    // The Android handset's start preparation fails before anything runs (it is bound to no
    // device); no ladder climbs meanwhile, as Workflow #398 RD-1b's T8 starts a handset whose adb
    // never runs. The iOS handset is not prepared at all (it is inert).
    let no_ladder = BTreeMap::from([(
        PHONE.to_owned(),
        actingcommand_contract::InstanceStuckRecovery {
            enabled: false,
            cooldown_secs: 600,
        },
    )]);
    let host = RuntimeHost::start(
        config(&root).with_stuck_recovery(no_ladder),
        Arc::new(registry),
    )
    .expect("runtime host");
    let answer_of = |alias: &str| {
        let receipt = observe(&host, alias);
        let outcome = receipt.outcome();
        (
            receipt.state(),
            outcome.code().as_str().to_owned(),
            cause_codes(outcome),
            outcome.value("stage").cloned(),
            outcome.value("capture_open_cause").cloned(),
        )
    };

    let iphone = answer_of(IPHONE);
    assert_ne!(iphone.0, RuntimeReceiptState::Completed, "{iphone:?}");
    assert_eq!(iphone, answer_of(PHONE));
    assert_eq!(acib.launches(), 0, "no child of an unbound handset");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

/// Fix round 1, item 5: a capture failure that carries the mark of a picture its backend refused
/// (`FrameRefusal`) beside an unconfirmed resource is an unconfirmed resource: the kernel does not
/// keep the session and the host closes it (its capture backend's close is asked), the instance is
/// fenced, and the Runtime goes on. The fake answers that failure at its first capture after an
/// input.
#[test]
fn f3a_fix1_h10_a_refused_picture_beside_an_unconfirmed_resource_is_an_unconfirmed_resource() {
    const ALIAS: &str = "neutral.node";
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    *state
        .capture_error_after_input
        .lock()
        .expect("capture error after input") = Some(
        actingcommand_device::DeviceError::transient(
            "neutral refused picture beside a reader that did not end",
        )
        .with_frame_refused(FrameRefusal::AllBlack)
        .with_resource_close_cause(
            DeviceResourceKind::PipeReader,
            DeviceResourceClosePhase::WorkerJoin,
            "neutral",
            None,
            None,
            DeviceResourceQuiescence::Unconfirmed,
            1,
        ),
    );
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(ALIAS, instance_id(), Arc::clone(&state))),
    )
    .expect("runtime host");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let lease = send(
        &host,
        RuntimeOperation::acquire_lease(ALIAS, ids.mint_holder_id().expect("holder")),
        4813,
    );
    let Some(RuntimeResult::LeaseGranted { token }) = lease.result() else {
        panic!("a lease: {lease:?}");
    };
    let token = token.clone();
    let reset = send(
        &host,
        RuntimeOperation::Input {
            token: token.clone(),
            action: InputAction::Reset,
            frame: None,
        },
        4813,
    );
    assert_eq!(reset.state(), RuntimeReceiptState::Completed, "{reset:?}");
    let released = send(&host, RuntimeOperation::ReleaseLease { token }, 4813);
    assert_eq!(
        released.state(),
        RuntimeReceiptState::Completed,
        "{released:?}"
    );
    let closes_before = state.capture_close_count.load(Ordering::Acquire);

    let failed = observe(&host, ALIAS);
    assert_ne!(failed.state(), RuntimeReceiptState::Completed, "{failed:?}");
    assert!(
        state.capture_close_count.load(Ordering::Acquire) > closes_before,
        "the session was not kept: its capture backend was closed"
    );
    assert!(
        instance_status(&host, ALIAS).resource_fence().is_some(),
        "the unconfirmed resource fences the instance"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}
