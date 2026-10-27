// SPDX-License-Identifier: AGPL-3.0-only

//! #419 DIAG-RUN (brief rd-1026/brief-diag-run.md; root cause rd-1026/stop2/root-cause.md §5 R2;
//! the production stop of 2026-10-26 23:23:04): at the end of every run the task diagnostic reads
//! the run's `artifact.verified` records. On 10-26 that page read got no writer reply within the
//! reply limit (`writer_response_timeout during query_event_page`), the diagnostic turned the
//! timeout into a fatal `artifact_store_failure` without the ledger's code, and one instance's
//! run stopped the whole Runtime.
//!
//! The host's part, on direct runs of the neutral package on a fake Runtime clock:
//! - T4 non-fatal: at the run's end work the read hold lets the diagnostic's head read pass and
//!   holds its page read, so the page read waits out a 3 s reply limit; the hold is released once
//!   the diagnostic has ended. The run ends `failed`, not fatal, with `writer_response_timeout` at
//!   `task_diagnostic_failed` in its receipt and its one terminal; nothing of its diagnostic
//!   stream is published and its staging file is gone; nothing latches, no stop is requested, and
//!   the instance's next run is admitted and completes;
//! - T5 fatal stays: at the run's end work the run's newest `artifact.verified` row is damaged,
//!   so the diagnostic's page read meets a damaged record; that ledger failure is still fatal: the
//!   Runtime latches it, a stop is requested and the Runtime closes with a failure.
//!
//! The only real wait is T4's reply limit; nothing sleeps.

use super::instance_workers::all_events;
use super::*;
use crate::codes::HostLocation;
use crate::host::ContainedTaskCheckpointPoint;
use actingcommand_contract::RequestId;
use actingcommand_ledger::{LedgerCode, WriterReadHoldGate};
use actingcommand_runtime_database::{DatabaseLocation, RuntimeDatabase};
use serde_json::Value as JsonValue;
use std::sync::Mutex;

const INSTANCE: &str = "diag-run.instance";
/// T4: how long the diagnostic's page read waits for the held writer.
const REPLY_TIMEOUT: Duration = Duration::from_secs(3);
/// The file name an open artifact stream stages its bytes under.
const STAGING_PREFIX: &str = ".artifact-stream.partial-";

/// A host on a fake Runtime clock with one instance, the frame cleaner off, and (when given) the
/// writer's reply limit.
fn start(
    root: &TempDir,
    instance: InstanceId,
    state: &Arc<FakeState>,
    reply_timeout: Option<Duration>,
) -> RuntimeHost {
    let mut host_config = config(root)
        .with_runtime_clock(Arc::new(ManualRuntimeClock::new(1_000, 0)))
        .with_frame_retention_enabled(false);
    if let Some(timeout) = reply_timeout {
        host_config = host_config.with_ledger_reply_timeout(timeout);
    }
    RuntimeHost::start(
        host_config,
        Arc::new(FakeProvider::one(INSTANCE, instance, Arc::clone(state))),
    )
    .expect("runtime host")
}

/// A direct run of `package` on the instance, as `actingctl` sends it.
fn direct_run(package: &str) -> RuntimeRequest {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            INSTANCE,
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package).expect("task request"),
        ),
    )
}

/// The staging files of the artifact streams open below `root`.
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

/// Changes the last digit of the keyed tag of the newest committed `artifact.verified` row of the
/// run of `request_id`: the row no longer equals its projection over its stored bytes.
fn damage_newest_verified_record(root: &Path, request_id: RequestId) {
    let database = RuntimeDatabase::open_existing(root, false).expect("state database");
    let connection = database
        .connection(DatabaseLocation::OpenExistingDatabase.location())
        .expect("state database connection");
    let request = serde_json::to_value(request_id).expect("the request id");
    let request = request.as_str().expect("the request id's text").to_owned();
    let stored: i64 = connection
        .query_row(
            "SELECT e.sequence FROM ledger_events e JOIN ledger_links l ON l.sequence = e.sequence WHERE e.event_type = 'artifact.verified' AND l.request_id = ?1 AND l.task_id IS NOT NULL AND l.run_id IS NOT NULL ORDER BY e.sequence DESC LIMIT 1",
            [request],
            |row| row.get(0),
        )
        .expect("a verified record of the run");
    connection
        .execute_batch(&format!(
            "UPDATE ledger_events SET integrity_tag = substr(integrity_tag, 1, length(integrity_tag) - 1) || CASE substr(integrity_tag, -1) WHEN '0' THEN '1' ELSE '0' END WHERE sequence = {stored}"
        ))
        .expect("damage the row");
}

#[test]
fn diag_run_t4_a_diagnostic_read_not_answered_in_time_fails_only_its_run() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let package = super::gate_369s5::direct_run_package(&root, &state);
    let instance = instance_id();
    let host = Arc::new(start(&root, instance, &state, Some(REPLY_TIMEOUT)));
    let sample_capacity = host.capacity_sampler_for_test().expect("capacity owner");
    let request = direct_run(&package);
    let request_id = request.request_id();

    // At the run's end work: the hold lets the diagnostic's head read pass and holds its page
    // read. Once the diagnostic has ended, its page read unanswered, the hold is released.
    let hold: Arc<Mutex<Option<WriterReadHoldGate>>> = Arc::new(Mutex::new(None));
    let release_control = Arc::new(Mutex::new(None));
    let end_work = {
        let installer = Arc::clone(&host);
        let hold = Arc::clone(&hold);
        let release_control = Arc::clone(&release_control);
        let state_root = root.path().to_path_buf();
        host.run_at_request_checkpoint_for_test(
            ContainedTaskCheckpointPoint::EndWork,
            request_id,
            instance,
            move |_| {
                *hold.lock().expect("the hold") =
                    Some(WriterReadHoldGate::arm_after(&state_root, 1).expect("arm the read hold"));
                let control = installer
                    .run_at_request_checkpoint_for_test(
                        ContainedTaskCheckpointPoint::DiagnosticEnded,
                        request_id,
                        instance,
                        move |_| {
                            let gate = hold
                                .lock()
                                .expect("the hold")
                                .take()
                                .expect("the hold armed at the end work");
                            assert!(
                                gate.wait_held(Duration::ZERO),
                                "the diagnostic's page read was held"
                            );
                            gate.release();
                        },
                    )
                    .expect("install the release");
                *release_control.lock().expect("the release") = Some(control);
            },
        )
        .expect("install the hold")
    };

    sample_capacity().expect("a capacity sample");
    let failed = host
        .process_request_for_test(&request, ConnectionId::new(1).expect("connection"))
        .expect("a receipt");

    assert_eq!(end_work.consumed(), 1, "the run reached its end work");
    let released = release_control
        .lock()
        .expect("the release")
        .take()
        .expect("the release was installed");
    assert_eq!(released.consumed(), 1, "the run's diagnostic ended");
    assert!(
        hold.lock().expect("the hold").is_none(),
        "the hold was released"
    );
    assert_eq!(failed.state(), RuntimeReceiptState::Failed, "{failed:?}");
    assert_eq!(
        failed.outcome().code().as_str(),
        LedgerCode::WriterResponseTimeout.as_str(),
        "{failed:?}"
    );
    assert!(
        !failed.error_projection().expect("the run's failure").fatal,
        "{failed:?}"
    );
    let terminals = host
        .query_persisted_events_for_test(EventQuery {
            request_id: Some(request_id),
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
        LedgerCode::WriterResponseTimeout.as_str(),
        "{outcome:?}"
    );
    assert_eq!(
        outcome.value("operation"),
        Some(&JsonValue::from(
            HostLocation::TaskDiagnosticFailed.as_str()
        )),
        "{outcome:?}"
    );

    // Its diagnostic stream is aborted: nothing of it is published, its staging file is gone.
    let run_id = terminal
        .links()
        .run_id()
        .copied()
        .expect("the terminal names its run");
    let published = all_events(&host)
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
        .count();
    assert_eq!(published, 0, "nothing of the run's diagnostic stream");
    assert_eq!(staging_files(root.path()), Vec::<PathBuf>::new());

    // Nothing latched, no stop; the instance's next run is admitted and completes.
    assert!(
        host.fatal_error().expect("health").is_none(),
        "nothing latched"
    );
    assert!(
        !host.is_shutdown_requested().expect("shutdown flag"),
        "no stop requested"
    );
    sample_capacity().expect("a capacity sample");
    let next = host
        .process_request_for_test(
            &direct_run(&package),
            ConnectionId::new(2).expect("connection"),
        )
        .expect("a receipt");
    assert_eq!(next.state(), RuntimeReceiptState::Completed, "{next:?}");
    assert!(
        host.fatal_error().expect("health").is_none(),
        "nothing latched"
    );
    Arc::try_unwrap(host)
        .unwrap_or_else(|_| panic!("the test holds the only host"))
        .close()
        .expect("close host");
}

#[test]
fn diag_run_t5_a_fatal_ledger_failure_of_the_diagnostic_read_still_stops_the_runtime() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let package = super::gate_369s5::direct_run_package(&root, &state);
    let instance = instance_id();
    let host = start(&root, instance, &state, None);
    let sample_capacity = host.capacity_sampler_for_test().expect("capacity owner");
    let request = direct_run(&package);
    let request_id = request.request_id();
    let state_root = root.path().to_path_buf();
    let end_work = host
        .run_at_request_checkpoint_for_test(
            ContainedTaskCheckpointPoint::EndWork,
            request_id,
            instance,
            move |_| damage_newest_verified_record(&state_root, request_id),
        )
        .expect("install the damage");

    sample_capacity().expect("a capacity sample");
    let receipt =
        host.process_request_for_test(&request, ConnectionId::new(1).expect("connection"));

    assert_eq!(end_work.consumed(), 1, "the run reached its end work");
    let latched = host
        .fatal_error()
        .expect("health")
        .unwrap_or_else(|| panic!("the Runtime latched the failure: {receipt:?}"));
    assert!(
        latched.is_fatal() && latched.halts_runtime(),
        "{latched:?} {receipt:?}"
    );
    assert!(
        host.is_shutdown_requested().expect("shutdown flag"),
        "the latch stops the Runtime"
    );
    host.close()
        .expect_err("the Runtime ends with its fatal failure");
}
