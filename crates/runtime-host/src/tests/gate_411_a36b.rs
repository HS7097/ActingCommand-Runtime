// SPDX-License-Identifier: AGPL-3.0-only

// #411 A36b (0.12.x, containment; production 10-20 18:02:07 JST, v0.12.6): a run's task
// diagnostic stream whose staging file changed under it fails its seal with
// `artifact_hash_mismatch` before anything is published. That fails only the run: its terminal
// is failed with the mismatch and the byte counts the seal compared, its lease is released, and
// the host is not latched, so a run on another instance is accepted and completes afterwards.
//
// The foreign write lands at the run's package admission (the contained-task checkpoint hook,
// on the run's own thread), after the stream opened and before it is sealed. It overwrites the
// first bytes of the staging file in place, so the two counts stay equal and the bytes differ.

use super::instance_workers::all_events;
use super::*;
use actingcommand_artifact_store::{StoreCode, StoreLocation};
use serde_json::Value as JsonValue;
use std::sync::Mutex;

const INSTANCE_A: &str = "a36b.instance.a";
const INSTANCE_B: &str = "a36b.instance.b";
// The file name an open artifact stream stages its bytes under.
const STAGING_PREFIX: &str = ".artifact-stream.partial-";
const FOREIGN: &[u8] = b"foreign";

// A started host with instance A (`state`, id `a`) and instance B (`other_state`).
fn two_instances(
    root: &TempDir,
    a: InstanceId,
    state: &Arc<FakeState>,
    other_state: &Arc<FakeState>,
) -> RuntimeHost {
    RuntimeHost::start(
        config(root),
        Arc::new(FakeProvider::from_entries([
            (INSTANCE_A.to_owned(), a, Arc::clone(state)),
            (
                INSTANCE_B.to_owned(),
                instance_id(),
                Arc::clone(other_state),
            ),
        ])),
    )
    .expect("runtime host with two instances")
}

// A direct run of `package` on `instance_alias`, as `actingctl` sends it.
fn direct_run(instance_alias: &str, package: &str) -> RuntimeRequest {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            instance_alias,
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package).expect("task request"),
        ),
    )
}

fn send(host: &RuntimeHost, request: &RuntimeRequest, connection: u64) -> RuntimeReceipt {
    host.process_request_for_test(request, ConnectionId::new(connection).expect("connection"))
        .expect("a receipt")
}

// The staging files of the artifact streams open below `root`.
fn staging_files(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(STAGING_PREFIX))
            {
                found.push(path);
            }
        }
    }
    found
}

// The byte counts an outcome says the seal compared: `expected_bytes` (the written stream) and
// `actual_bytes` (the staged file).
fn compared_counts(outcome: &actingcommand_contract::outcome::Outcome) -> (u64, u64) {
    let count = |key| {
        outcome
            .value(key)
            .and_then(JsonValue::as_u64)
            .unwrap_or_else(|| panic!("{key} of {outcome}"))
    };
    (count("expected_bytes"), count("actual_bytes"))
}

// S2: instance A's direct run admits its package, and then a foreign write overwrites the first
// bytes of its open diagnostic stream. The run's seal finds the mismatch: the receipt is `Failed`
// with `artifact_hash_mismatch`, not fatal; the run's one `task.failed` terminal carries it at
// `seal_artifact_stream` with the two equal counts; nothing of the stream is published and its
// staging file is gone; the run's lease is released once. The host is not latched: B's direct
// run is accepted and completes, and the host closes normally.
#[test]
fn a36b_a_run_whose_diagnostic_seal_mismatches_fails_only_itself_and_the_host_keeps_serving() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let other_state = Arc::new(FakeState::default());
    let package = super::gate_369s5::direct_run_package(&root, &state);
    let other_package = super::gate_369s5::direct_run_package(&root, &other_state);
    let a = instance_id();
    let host = two_instances(&root, a, &state, &other_state);
    let request = direct_run(INSTANCE_A, &package);
    let overwritten = Arc::new(Mutex::new(Vec::new()));
    let foreign_writes = Arc::clone(&overwritten);
    let state_root = root.path().to_path_buf();
    let checkpoint = host
        .run_at_contained_task_checkpoint_for_test(request.request_id(), a, None, move |_| {
            for staging in staging_files(&state_root) {
                let written = OpenOptions::new()
                    .write(true)
                    .open(&staging)
                    .and_then(|mut file| file.write_all(FOREIGN));
                foreign_writes
                    .lock()
                    .expect("foreign writes")
                    .push((staging, written.map_err(|error| error.kind())));
            }
        })
        .expect("install the foreign write");

    let failed = send(&host, &request, 1361);

    assert_eq!(checkpoint.consumed(), 1);
    let writes = overwritten.lock().expect("foreign writes").clone();
    let [(_, Ok(()))] = writes.as_slice() else {
        panic!("one foreign write into the run's one open stream: {writes:?}");
    };
    assert_eq!(failed.state(), RuntimeReceiptState::Failed, "{failed:?}");
    assert_eq!(
        failed.outcome().code().as_str(),
        StoreCode::ArtifactHashMismatch.as_str(),
        "{failed:?}"
    );
    assert!(
        !failed.error_projection().expect("the run's failure").fatal,
        "{failed:?}"
    );
    let terminals = host
        .query_persisted_events_for_test(EventQuery {
            request_id: Some(request.request_id()),
            event_type: Some(EventType::TaskFailed),
            ..EventQuery::default()
        })
        .expect("the run's terminals");
    let [terminal] = terminals.as_slice() else {
        panic!("one task.failed of the run: {terminals:#?}");
    };
    let outcome = terminal
        .payload()
        .outcome()
        .expect("the terminal's failure");
    assert_eq!(
        outcome.code().as_str(),
        StoreCode::ArtifactHashMismatch.as_str()
    );
    assert_eq!(
        outcome.value("operation"),
        Some(&JsonValue::from(StoreLocation::SealArtifactStream.as_str()))
    );
    let (written, staged) = compared_counts(outcome);
    assert_eq!(written, staged, "the foreign bytes replaced bytes in place");
    assert!(written > FOREIGN.len() as u64);
    assert_eq!(compared_counts(&failed.outcome()), (written, staged));

    let events = all_events(&host);
    let run_id = terminal
        .links()
        .run_id()
        .copied()
        .expect("the terminal names its run");
    // Nothing of the mismatched stream is published. The stream never got an artifact id, so
    // its artifacts are the run's ones with the kind and issue policy `begin_diagnostic` gives
    // the stream (producer, retention, pending redaction). The run's effective-configuration
    // records are separate artifacts, published before the seal (not redaction-pending).
    let from_stream = events
        .iter()
        .flat_map(PersistedEvent::artifacts)
        .filter(|artifact| {
            artifact.run_id() == Some(&run_id)
                && artifact.kind() == ArtifactKind::DiagnosticJson
                && artifact.producer() == ArtifactProducer::ArtifactStore
                && artifact.retention_class() == actingcommand_contract::RetentionClass::DebugFull
                && artifact.redaction_state()
                    == actingcommand_contract::ArtifactRedactionState::Pending
        })
        .collect::<Vec<_>>();
    assert!(from_stream.is_empty(), "{from_stream:#?}");
    assert_eq!(staging_files(root.path()), Vec::<PathBuf>::new());
    let lease = terminal
        .links()
        .lease_id()
        .copied()
        .expect("the terminal names its lease");
    let released = events
        .iter()
        .filter(|event| {
            event.event_type() == EventType::LeaseReleased
                && event.links().lease_id() == Some(&lease)
        })
        .count();
    assert_eq!(released, 1, "the run's lease is released once");
    assert!(host.fatal_error().expect("runtime health").is_none());

    let other = send(&host, &direct_run(INSTANCE_B, &other_package), 1362);
    assert_eq!(other.state(), RuntimeReceiptState::Completed, "{other:?}");
    assert!(matches!(
        other.result(),
        Some(RuntimeResult::ContainedTaskCompleted {
            task_result: TaskOutcome::Success,
            ..
        })
    ));
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}
