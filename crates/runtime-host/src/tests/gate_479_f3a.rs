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

use super::gate_369_s6c::{instance_status, send};
use super::*;
use crate::codes::HostCode;
use actingcommand_contract::codes::{DeviceSelfCheckFailure, SharedCode};
use actingcommand_contract::{
    CaptureSequenceSpec, DeviceBoundVia, EmulatorCapability, EmulatorCapabilityAvailability,
    EmulatorCapabilityImplementation, RecoveryDrillMode, Sha256Fingerprint,
};
use actingcommand_device::{
    AcibBounds, AcibCaptureSource, AcibCommand, AcibKeySalt, AcibLauncher, AcibProcess, AdbConfig,
    CaptureBackendChoice, CaptureBackendConfig, DeviceResourceCloseOutcome,
    DeviceResourceClosePhase, DeviceResourceKind, DeviceResourceQuiescence, DeviceSerial,
    DeviceTarget, MaaTouchConfig, TouchBackendChoice, TouchBackendConfig,
};
use std::collections::VecDeque;
use std::io::{Cursor, Read};
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

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

/// The registration of the bound iOS handset `alias`, as actingd registers one (stand-in backends
/// that open nothing), its frames from `launcher`.
fn ios_registration(
    alias: &str,
    instance_id: InstanceId,
    launcher: Arc<ScriptedAcib>,
) -> ExecutionBackendRegistration {
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
    let source = AcibCaptureSource::new(
        Some(std::env::current_exe().expect("an existing file as the program")),
        AcibKeySalt::new(SALT),
    )
    .with_bounds(AcibBounds {
        wait: Duration::from_millis(100),
        ..AcibBounds::runtime()
    })
    .with_launcher(launcher);
    ExecutionBackendRegistration::new(
        alias,
        instance_id,
        "neutral.application",
        TouchBackendConfig::new(adb.clone(), target.clone(), MaaTouchConfig::default())
            .with_requested(TouchBackendChoice::AdbShellInput),
        CaptureBackendConfig::new(adb, target).with_requested(CaptureBackendChoice::Adb),
    )
    .expect("iOS handset registration")
    .with_handset(bound_ios())
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
