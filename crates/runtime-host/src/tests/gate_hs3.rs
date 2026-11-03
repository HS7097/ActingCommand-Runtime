// SPDX-License-Identifier: AGPL-3.0-only

//! #402 HS-3 (register §1.271; design `rd-1031/backend-switch/model-backend-switch.md` §4.1, §4.5
//! and §7 row HS-3, with the coordinator's answer Q3 on board #402; coordinator work order
//! `brief-hs-3.md`): a paired Nemu open whose configuration names no `nemu_app_index` resolves
//! one itself. The file's index wins; otherwise the registry reads `MuMuManager control -v <index>
//! app info -pkg <package>` once before the paired session opens: `running` is index 0;
//! `not_installed`, `stopped` and a read that gives no answer refuse the open, named
//! (`paired_backend_open_failed` caused by `nemu_app_index_missing` with its
//! `nemu_app_index_problem`). The resolution stays in the registry entry, in memory, until a
//! rebinding or an open that finds no display; a handset never reads `MuMuManager`.
//!
//! Nothing here runs `MuMuManager`, adb or the Nemu library: the read is a fake answering the
//! vendor's documented document through the device layer's own parser (or failing as an expired
//! run does), and the paired open is a fake that records the index it is handed and opens no
//! device. The host's naming of a refusal is read from a self-check whose fake provider answers
//! the registry's own refusal. Names, indices and ports are made up.

use super::*;
use crate::codes::HostCode;
use actingcommand_contract::{BackendObservationStatus, BackendOpenEntry, RuntimePayload};
use actingcommand_device::codes::DeviceStage;
use actingcommand_device::{
    AdbConfig, CaptureBackendChoice, CaptureBackendConfig, CaptureOpenCause, CaptureOpenFailure,
    DeviceTarget, FrameMemoryBudget, MaaTouchConfig, MumuApplicationState, NemuAppIndex,
    NemuAppIndexProblem, NemuApplicationTarget, NemuInputConfig, NemuSessionBackends,
    OpenedBackend, TouchBackendChoice, TouchBackendConfig, mumu_instance_port,
};
use serde_json::{Value, json};
use std::sync::Mutex;

const ALIAS: &str = "hs3.instance";
const HANDSET: &str = "hs3.handset";
const PACKAGE: &str = "neutral.application";
const MUMU_MANAGER: &str = "neutral-mumu-manager";
const INDEX: u16 = 3;
const PAIRED_OPEN_FAILED: &str = "paired_backend_open_failed";

/// Every `app info -pkg` read the fake `MuMuManager` answered: the manager, the index, the package.
type Reads = Arc<Mutex<Vec<(PathBuf, u16, String)>>>;
/// The application index of every paired open the fake opener was handed.
type Opened = Arc<Mutex<Vec<u32>>>;

/// A registration with touch `touch` and Nemu capture, bound through discovery to `INDEX` on its
/// base port (so the CAP-IDX port check reads nothing), as actingd assembles a discovered instance.
fn registration(touch: TouchBackendChoice) -> ExecutionBackendRegistration {
    let target = DeviceTarget {
        port: mumu_instance_port(INDEX).expect("base port"),
        ..DeviceTarget::default()
    };
    let adb = AdbConfig {
        adb_path: "neutral-adb".to_owned(),
        ..AdbConfig::default()
    };
    ExecutionBackendRegistration::new(
        ALIAS,
        instance_id(),
        PACKAGE,
        TouchBackendConfig::new(adb.clone(), target.clone(), MaaTouchConfig::default())
            .with_requested(touch),
        CaptureBackendConfig::new(adb, target).with_requested(CaptureBackendChoice::NemuIpc),
    )
    .expect("registration")
    .with_discovered_binding(DiscoveredInstanceBinding::new(
        INDEX,
        "neutral-instance",
        "neutral-provider-version",
        MUMU_MANAGER,
    ))
}

/// A fake `MuMuManager` that answers every `app info -pkg` read with `document`, read by the
/// device layer's own parser, and records the read.
fn answering(
    document: Value,
    reads: Reads,
) -> impl Fn(&Path, u16, &str) -> DeviceResult<MumuApplicationState> + Send + Sync + 'static {
    move |manager, index, package| {
        reads
            .lock()
            .expect("reads")
            .push((manager.to_path_buf(), index, package.to_owned()));
        actingcommand_device::parse_application_state(&document, index, package)
    }
}

/// A fake paired open: records the application index it is handed and fails with `failure()`,
/// opening no device.
fn opener(
    opened: Opened,
    failure: impl Fn() -> DeviceError + Send + Sync + 'static,
) -> impl Fn(
    CaptureBackendConfig,
    NemuApplicationTarget,
    NemuInputConfig,
    Option<&FrameMemoryBudget>,
) -> DeviceResult<OpenedBackend<NemuSessionBackends>>
+ Send
+ Sync
+ 'static {
    move |_, application, _, _| {
        opened
            .lock()
            .expect("opened")
            .push(application.app_index().index());
        Err(failure())
    }
}

/// The fake open's failure that is not a missing display (a test has no device).
fn no_device() -> DeviceError {
    DeviceError::fatal("fixture paired open: no device in a test")
}

/// The fake open's failure as a Nemu display lookup that found no display for the package.
fn display_missing() -> DeviceError {
    DeviceError::fatal("fixture Nemu application display query failed").with_capture_open_failure(
        CaptureOpenFailure {
            cause: CaptureOpenCause::DisplayMissing,
            application_id: Some(PACKAGE.to_owned()),
            instance_index: Some(i64::from(INDEX)),
        },
    )
}

/// A registry of `registration` whose `MuMuManager` answers `document` and whose paired open
/// fails with `failure()`.
fn answering_registry(
    registration: ExecutionBackendRegistration,
    document: Value,
    failure: impl Fn() -> DeviceError + Send + Sync + 'static,
) -> (ExecutionBackendRegistry, Reads, Opened) {
    let reads = Reads::default();
    let opened = Opened::default();
    let registry = ExecutionBackendRegistry::new([registration])
        .expect("the registry builds without nemu_app_index")
        .with_application_state_reader(answering(document, Arc::clone(&reads)))
        .with_nemu_session_opener(opener(Arc::clone(&opened), failure));
    (registry, reads, opened)
}

/// The paired open of `alias`, which every fake here ends in a failure.
fn open_fails(registry: &ExecutionBackendRegistry, alias: &str) -> DeviceError {
    let Err(error) = registry.open_nemu_session(alias, None) else {
        panic!("the fake paired open never succeeds");
    };
    error
}

fn read_count(reads: &Reads) -> usize {
    reads.lock().expect("reads").len()
}

fn opened_indices(opened: &Opened) -> Vec<u32> {
    opened.lock().expect("opened").clone()
}

/// The one read the fake `MuMuManager` answered: the binding's manager and index, the package.
fn assert_one_read(reads: &Reads) {
    assert_eq!(
        *reads.lock().expect("reads"),
        vec![(PathBuf::from(MUMU_MANAGER), INDEX, PACKAGE.to_owned())]
    );
}

/// A refused paired open is observed as a failed pair open and names `problem`.
fn assert_refused(error: &DeviceError, problem: NemuAppIndexProblem) {
    assert_eq!(error.nemu_app_index_problem(), Some(problem), "{error:?}");
    let [observation] = error.backend_open_observations() else {
        panic!("one paired open observation: {error:?}");
    };
    assert_eq!(observation.report.entry, BackendOpenEntry::NemuPair);
    assert_eq!(observation.report.status, BackendObservationStatus::Failed);
}

/// What the host makes of the registry's refusal `error`: a self-check of an instance whose
/// paired open fails with it records `paired_backend_open_failed`, and this is the
/// `nemu_app_index_problem` of its `nemu_app_index_missing` cause. Only that instance's
/// self-check fails; the Runtime keeps running.
fn host_names(error: DeviceError) -> Option<Value> {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(ALIAS, instance_id(), Arc::clone(&state))),
    )
    .expect("runtime host");
    *state.nemu_open_error.lock().expect("nemu open error") = Some(error);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let request = runtime_request(
        &ids,
        RuntimeOperation::SelfCheckInstance {
            instance_alias: ALIAS.to_owned(),
        },
    );
    let receipt = host
        .process_request_for_test(&request, ConnectionId::new(503).expect("connection"))
        .expect("self-check receipt");
    let Some(RuntimeResult::InstanceSelfChecked { selfcheck, .. }) = receipt.result() else {
        panic!("a self-check result: {receipt:?}");
    };
    assert_eq!(
        selfcheck.failure_code.as_deref(),
        Some(PAIRED_OPEN_FAILED),
        "{selfcheck:?}"
    );
    let outcome = host
        .query_persisted_events_for_test(EventQuery {
            request_id: Some(request.request_id()),
            ..EventQuery::default()
        })
        .expect("events of the request")
        .iter()
        .find_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::Failed(record))
                if record.outcome().code().as_str() == PAIRED_OPEN_FAILED =>
            {
                Some(record.outcome().clone())
            }
            _ => None,
        })
        .expect("the self-check records the failed paired open");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
    outcome
        .causes()
        .iter()
        .map(|link| link.outcome())
        .find(|cause| cause.is(HostCode::NemuAppIndexMissing))
        .and_then(|cause| cause.value("nemu_app_index_problem").cloned())
}

#[test]
fn hs3_running_package_resolves_zero() {
    // The registry builds Nemu touch without `nemu_app_index` (it refused that until HS-3).
    let (registry, reads, opens) = answering_registry(
        registration(TouchBackendChoice::NemuIpc),
        json!({"state": "running"}),
        no_device,
    );

    let error = open_fails(&registry, ALIAS);

    // One read through the binding's MuMuManager and index; the paired open gets index 0.
    assert_one_read(&reads);
    assert_eq!(opened_indices(&opens), vec![0]);
    assert_eq!(error.nemu_app_index_problem(), None, "{error:?}");
    // The resolution is the entry's (a failure other than a missing display keeps it).
    assert_eq!(registry.resolved_nemu_app_index(ALIAS), Some(0));
}

#[test]
fn hs3_file_index_wins() {
    let file_index = NemuAppIndex::try_from(2_u32).expect("index");
    // MuMuManager would refuse; the file's index is used and nothing is read.
    let (registry, reads, opens) = answering_registry(
        registration(TouchBackendChoice::NemuIpc)
            .with_nemu_app_index(file_index)
            .expect("an index with Nemu touch"),
        json!({"state": "not_installed"}),
        no_device,
    );
    let error = open_fails(&registry, ALIAS);
    assert_eq!(error.nemu_app_index_problem(), None, "{error:?}");
    assert_eq!(opened_indices(&opens), vec![2]);
    assert_eq!(read_count(&reads), 0);
    assert_eq!(registry.resolved_nemu_app_index(ALIAS), None);

    // An index the file names beside another touch backend is kept, and a switch to Nemu touch
    // opens with it, still reading nothing.
    let (registry, reads, opens) = answering_registry(
        registration(TouchBackendChoice::AdbShellInput)
            .with_nemu_app_index(file_index)
            .expect("an index beside adb touch"),
        json!({"state": "running"}),
        no_device,
    );
    assert!(matches!(registry.open_nemu_session(ALIAS, None), Ok(None)));
    assert_eq!(
        registry
            .switch_instance_backends(
                ALIAS,
                TouchBackendChoice::NemuIpc,
                CaptureBackendChoice::NemuIpc
            )
            .expect("switch to the Nemu pair"),
        Some(2)
    );
    open_fails(&registry, ALIAS);
    assert_eq!(opened_indices(&opens), vec![2]);
    assert_eq!(read_count(&reads), 0);
}

#[test]
fn hs3_not_installed_refused_named() {
    let (registry, reads, opens) = answering_registry(
        registration(TouchBackendChoice::NemuIpc),
        json!({"state": "not_installed"}),
        no_device,
    );

    let error = open_fails(&registry, ALIAS);

    assert_refused(&error, NemuAppIndexProblem::PackageNotInstalled);
    assert_one_read(&reads);
    assert!(
        opened_indices(&opens).is_empty(),
        "nothing opens without an index"
    );
    // A refusal is not kept: the next open reads again.
    assert_eq!(registry.resolved_nemu_app_index(ALIAS), None);
    open_fails(&registry, ALIAS);
    assert_eq!(read_count(&reads), 2);
    // The host names it under the failed paired open.
    assert_eq!(host_names(error), Some(json!("package_not_installed")));
}

#[test]
fn hs3_stopped_refused_named() {
    let (registry, reads, opens) = answering_registry(
        registration(TouchBackendChoice::NemuIpc),
        json!({"state": "stopped"}),
        no_device,
    );

    let error = open_fails(&registry, ALIAS);

    assert_refused(&error, NemuAppIndexProblem::ApplicationNotRunning);
    assert_one_read(&reads);
    assert!(
        opened_indices(&opens).is_empty(),
        "nothing opens without an index"
    );
    assert_eq!(registry.resolved_nemu_app_index(ALIAS), None);
    assert_eq!(host_names(error), Some(json!("application_not_running")));
}

#[test]
fn hs3_reader_timeout_refused_named() {
    // The read passes its 10 s bound, as `MUMU_MANAGER_COMMAND_TIMEOUT` ends a run.
    let reads = Reads::default();
    let opens = Opened::default();
    let counted = Arc::clone(&reads);
    let registry = ExecutionBackendRegistry::new([registration(TouchBackendChoice::NemuIpc)])
        .expect("registry")
        .with_application_state_reader(move |manager, index, package| {
            counted
                .lock()
                .expect("reads")
                .push((manager.to_path_buf(), index, package.to_owned()));
            Err(DeviceError::fatal(format!(
                "MuMuManager control -v {index} app info -pkg {package} timed out after 10s"
            ))
            .with_diagnostic(
                DeviceErrorCategory::BackendLaunch,
                DeviceStage::MumuManagerTimeout.location(),
            ))
        })
        .with_nemu_session_opener(opener(Arc::clone(&opens), no_device));

    let error = open_fails(&registry, ALIAS);

    assert_refused(&error, NemuAppIndexProblem::InfoUnavailable);
    assert_eq!(
        error.diagnostic().map(|diagnostic| diagnostic.stage()),
        Some(DeviceStage::MumuManagerTimeout.location()),
        "the read's own stage is kept"
    );
    assert_one_read(&reads);
    assert!(
        opened_indices(&opens).is_empty(),
        "nothing opens without an index"
    );
    assert_eq!(host_names(error), Some(json!("info_unavailable")));

    // An answer that is not the documented document is malformed, not unavailable.
    let (registry, _, opens) = answering_registry(
        registration(TouchBackendChoice::NemuIpc),
        json!({"state": "suspended"}),
        no_device,
    );
    assert_refused(
        &open_fails(&registry, ALIAS),
        NemuAppIndexProblem::InfoMalformed,
    );
    assert!(opened_indices(&opens).is_empty());
}

#[test]
fn hs3_cache_reused_until_rebind() {
    let (registry, reads, opens) = answering_registry(
        registration(TouchBackendChoice::NemuIpc),
        json!({"state": "running"}),
        no_device,
    );

    open_fails(&registry, ALIAS);
    open_fails(&registry, ALIAS);
    // The second open takes the entry's resolution: one read for two opens.
    assert_eq!(read_count(&reads), 1);
    assert_eq!(opened_indices(&opens), vec![0, 0]);

    // An emulator start rebinds the entry: the resolution is dropped and read again.
    registry
        .rebind_discovered_endpoint(ALIAS, Some(mumu_instance_port(INDEX).expect("base port")))
        .expect("rebind after a start");
    assert_eq!(registry.resolved_nemu_app_index(ALIAS), None);
    open_fails(&registry, ALIAS);
    assert_eq!(read_count(&reads), 2);
    assert_eq!(opened_indices(&opens), vec![0, 0, 0]);
}

#[test]
fn hs3_display_missing_clears_cache() {
    let (registry, reads, opens) = answering_registry(
        registration(TouchBackendChoice::NemuIpc),
        json!({"state": "running"}),
        display_missing,
    );

    let error = open_fails(&registry, ALIAS);

    // The open's own failure is kept (A50's cause), and the resolution is dropped.
    assert_eq!(
        error.capture_open_failure().map(|failure| failure.cause),
        Some(CaptureOpenCause::DisplayMissing)
    );
    assert_eq!(registry.resolved_nemu_app_index(ALIAS), None);
    open_fails(&registry, ALIAS);
    assert_eq!(read_count(&reads), 2);
    assert_eq!(opened_indices(&opens), vec![0, 0]);
}

#[test]
fn hs3_handset_never_reads_manager() {
    let reads = Reads::default();
    let opens = Opened::default();
    let target = DeviceTarget {
        serial: Some(actingcommand_device::DeviceSerial::handset(
            "TESTSERIALHS3",
            HANDSET,
        )),
        port: 0,
        connect: false,
        ..DeviceTarget::default()
    };
    let adb = AdbConfig {
        adb_path: "neutral-adb".to_owned(),
        ..AdbConfig::default()
    };
    let handset = ExecutionBackendRegistration::new(
        HANDSET,
        instance_id(),
        PACKAGE,
        TouchBackendConfig::new(adb.clone(), target.clone(), MaaTouchConfig::default())
            .with_requested(TouchBackendChoice::AdbShellInput),
        CaptureBackendConfig::new(adb, target).with_requested(CaptureBackendChoice::Adb),
    )
    .expect("handset registration")
    .with_handset(super::gate_398_rd1b::bound(HANDSET, "TESTSERIALHS3"));
    let registry = ExecutionBackendRegistry::new([handset])
        .expect("registry")
        .with_application_state_reader(answering(json!({"state": "running"}), Arc::clone(&reads)))
        .with_nemu_session_opener(opener(Arc::clone(&opens), no_device));

    // A handset's own backends have no paired session.
    assert!(matches!(
        registry.open_nemu_session(HANDSET, None),
        Ok(None)
    ));
    // Even a Nemu pair put on it past the configuration's kind check is refused before any read.
    registry
        .switch_instance_backends(
            HANDSET,
            TouchBackendChoice::NemuIpc,
            CaptureBackendChoice::NemuIpc,
        )
        .expect("the registration's own checks know no device kind");
    assert_refused(
        &open_fails(&registry, HANDSET),
        NemuAppIndexProblem::InfoUnavailable,
    );
    assert_eq!(read_count(&reads), 0);
    assert!(opened_indices(&opens).is_empty());
}
