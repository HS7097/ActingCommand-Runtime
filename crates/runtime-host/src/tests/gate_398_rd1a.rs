// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #398 RD-1a (model-398 v1.1 §12, rows T4 and T5): an instance addressed by an adb
//! serial is recorded by its label `handset:<alias>`, and a whole fake-host run leaves the serial
//! in no record, artifact, log or error. The registration is the real one (the registry's own
//! resolution, behind the fake backends); the serial is a neutral fake.

use super::gate_369s5::direct_run_package;
use super::instance_workers::{admit_ladder_run, ladder_setup, wait_for_ladder_end};
use super::*;
use actingcommand_contract::{EffectiveConfigurationFacts, EffectiveConfigurationRecord};
use actingcommand_device::{
    AdbConfig, CaptureBackendChoice, CaptureBackendConfig, DeviceResourceQuiescence, DeviceSerial,
    DeviceTarget, MaaTouchConfig, TouchBackendChoice, TouchBackendConfig,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};

const SERIAL: &str = "TESTSERIAL0001";
const LABEL: &str = "handset:fixture-instance-a";

/// The serial in every spelling a writer could leave behind: as given and lowercase, each in
/// UTF-8 and in UTF-16 of both byte orders.
fn serial_spellings() -> Vec<Vec<u8>> {
    let mut spellings = Vec::new();
    for text in [SERIAL.to_owned(), SERIAL.to_ascii_lowercase()] {
        spellings.push(text.as_bytes().to_vec());
        spellings.push(text.encode_utf16().flat_map(u16::to_le_bytes).collect());
        spellings.push(text.encode_utf16().flat_map(u16::to_be_bytes).collect());
    }
    spellings
}

/// How many times any spelling of the serial occurs in `bytes`.
fn serial_hits(bytes: &[u8]) -> usize {
    serial_spellings()
        .iter()
        .map(|spelling| {
            bytes
                .windows(spelling.len())
                .filter(|window| *window == spelling.as_slice())
                .count()
        })
        .sum()
}

fn contains_bytes(bytes: &[u8], needle: &str) -> bool {
    bytes
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

/// Every file under `root`, at any depth.
fn files_under(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(&directory).expect("read a state directory") {
            let path = entry.expect("a state directory entry").path();
            if path.is_dir() {
                directories.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files
}

/// The registry's resolution of `POLICY_INSTANCE_ALIAS` registered on `target`.
fn registered(instance_id: InstanceId, target: DeviceTarget) -> ResolvedExecutionInstance {
    let adb = AdbConfig {
        adb_path: "neutral-adb".to_owned(),
        ..AdbConfig::default()
    };
    ExecutionBackendRegistry::new([ExecutionBackendRegistration::new(
        POLICY_INSTANCE_ALIAS,
        instance_id,
        "neutral.application",
        TouchBackendConfig::new(adb.clone(), target.clone(), MaaTouchConfig::default())
            .with_requested(TouchBackendChoice::AdbShellInput),
        CaptureBackendConfig::new(adb, target).with_requested(CaptureBackendChoice::Adb),
    )
    .expect("device registration")])
    .expect("device registry")
    .resolve(POLICY_INSTANCE_ALIAS)
    .expect("registered instance")
}

fn handset_target() -> DeviceTarget {
    DeviceTarget {
        serial: Some(DeviceSerial::handset(SERIAL, POLICY_INSTANCE_ALIAS)),
        ..DeviceTarget::default()
    }
}

fn start_host(
    config: RuntimeHostConfig,
    instance_id: InstanceId,
    target: DeviceTarget,
    state: &Arc<FakeState>,
) -> RuntimeHost {
    RuntimeHost::start(
        config,
        Arc::new(
            FakeProvider::one(POLICY_INSTANCE_ALIAS, instance_id, Arc::clone(state))
                .with_resolved_override(Arc::new(std::sync::Mutex::new(registered(
                    instance_id,
                    target,
                )))),
        ),
    )
    .expect("runtime host")
}

/// Every effective-configuration artifact of the ledger at `root`, with its bytes.
fn effective_configurations(
    host: &RuntimeHost,
    root: &Path,
) -> Vec<(Vec<u8>, EffectiveConfigurationRecord)> {
    let events = host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events");
    let mut records = Vec::new();
    for event in events
        .iter()
        .filter(|event| event.event_type() == EventType::ArtifactVerified)
    {
        for artifact in event.artifacts() {
            if artifact.kind() != ArtifactKind::DiagnosticJson
                || artifact.producer() != ArtifactProducer::ArtifactStore
            {
                continue;
            }
            let bytes = read_projected_verified(root, &artifact.project(true))
                .expect("a verified diagnostic artifact");
            let value: serde_json::Value =
                serde_json::from_slice(&bytes).expect("a diagnostic JSON artifact");
            if value
                .get("schema_version")
                .and_then(serde_json::Value::as_str)
                != Some(actingcommand_contract::EFFECTIVE_CONFIGURATION_SCHEMA)
            {
                continue;
            }
            let record = serde_json::from_value(value).expect("an effective configuration");
            records.push((bytes, record));
        }
    }
    records
}

/// T4 (§3.5 rule 6): a run on an instance addressed by a serial records the label in its
/// effective configuration and no spelling of the serial; an instance addressed by
/// `HOST:PORT` keeps its endpoint and no configured serial, as before.
#[test]
fn rd1a_t4_effective_configuration_names_the_handset_by_its_label() {
    for handset in [true, false] {
        let root = TempDir::new().expect("tempdir");
        let state = Arc::new(FakeState::default());
        let (package, expected) = direct_run_package(&root, &state);
        let instance_id = instance_id();
        let target = if handset {
            handset_target()
        } else {
            DeviceTarget::default()
        };
        let host = start_host(config(&root), instance_id, target, &state);
        let client = RuntimeClient::connect(RuntimeClientConfig::new(
            root.path(),
            EventActor::Cli,
            EventSource::Cli,
        ))
        .expect("official client");
        let _run = client.run_contained_task(
            POLICY_INSTANCE_ALIAS,
            ContainedTaskRequest::new(package, expected).expect("task request"),
        );
        let records = effective_configurations(&host, root.path());
        let mut devices = 0;
        for (bytes, record) in &records {
            assert_eq!(serial_hits(bytes), 0, "handset={handset}");
            let EffectiveConfigurationFacts::Initial { device, .. } = &record.facts else {
                continue;
            };
            let device = device.as_ref().expect("a registered device configuration");
            devices += 1;
            if handset {
                assert_eq!(device.configured_serial.as_deref(), Some(LABEL));
                assert_eq!(device.resolved_serial, LABEL);
            } else {
                assert_eq!(device.configured_serial, None);
                assert_eq!(device.resolved_serial, "127.0.0.1:16384");
            }
        }
        assert_eq!(devices, 1, "one initial configuration, handset={handset}");
        drop(client);
        host.close().expect("close host");
    }
}

/// T5, the gate (§3.5 closing line): start with its self-check, a routine run that fails and
/// hands its key to the ladder, the ladder's rungs, a self-check that meets a device error whose
/// text names the serial as adb's would, and shutdown. No file the run left under the state root
/// (ledger, artifacts, everything) and no error or receipt text holds any spelling of the serial;
/// the records name the instance by its label.
#[test]
fn rd1a_t5_a_whole_fake_host_run_leaves_no_device_serial_behind() {
    let root = TempDir::new().expect("tempdir");
    let (host_config, request) = ladder_setup(root.path());
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    // The registry binds the handset's adb endpoint, so the instance is device self-checked:
    // the start's preparation self-check decides its availability (§12 T5: start, then
    // self-check). The handset answers, so its opens report a passed native open.
    state.observed_open.store(true, Ordering::Release);
    let instance_id = instance_id();
    let host = start_host(host_config, instance_id, handset_target(), &state);
    let mut developer_log = Vec::new();

    let context = admit_ladder_run(&host, &request);
    state.unknown_capture.store(true, Ordering::Release);
    let error = host
        .run_scheduled_contained_task(&context, &request)
        .expect_err("the routine run fails");
    developer_log.push(format!("{error} {error:?}"));
    wait_for_ladder_end(&host, POLICY_INSTANCE_ALIAS);

    *state.input_open_error.lock().expect("input open error") = Some(
        DeviceError::transient(format!(
            "adb -s {SERIAL} get-state failed with exit code 1\nstdout:\n\nstderr:\nerror: device '{SERIAL}' not found"
        ))
        .with_resource_summary(DeviceResourceQuiescence::Confirmed, 3),
    );
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let check = runtime_request(
        &ids,
        RuntimeOperation::SelfCheckInstance {
            instance_alias: POLICY_INSTANCE_ALIAS.to_owned(),
        },
    );
    let receipt = host
        .process_request_for_test(&check, ConnectionId::new(398).expect("connection"))
        .expect("self-check receipt");
    developer_log.push(format!("{receipt:?}"));
    developer_log.extend(
        host.failure_detail_for_test(check.request_id())
            .expect("failure detail"),
    );
    host.close().expect("close host");

    let mut labelled = false;
    for path in files_under(root.path()) {
        let bytes =
            fs::read(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        assert_eq!(
            serial_hits(&bytes),
            0,
            "{} holds the device serial",
            path.display()
        );
        labelled |= contains_bytes(&bytes, LABEL);
    }
    assert!(labelled, "the records name the instance by its label");
    for text in &developer_log {
        assert_eq!(serial_hits(text.as_bytes()), 0, "{text}");
    }
}
