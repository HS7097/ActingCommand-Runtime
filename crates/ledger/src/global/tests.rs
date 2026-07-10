// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::PersistedEvent;
use crate::fact::StoredEventRecord;
use actingcommand_contract::{
    ActionId, AuditInput, CausationId, CommandPayloadDraft, CorrelationId, EventActor, EventDraft,
    EventId, EventLinks, EventOrigin, EventQuery, EventSeverity, EventSource, EventType, FrameId,
    InstanceId, LeaseId, ProjectionPayload, ProjectionProfile, RecognitionId, RequestId, RunId,
    SecretField, SecretFingerprinter, StaticCode, SubscriptionCursor, TaskId,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

fn config(temp: &TempDir, owner_id: &str) -> GlobalLedgerConfig {
    GlobalLedgerConfig::new(temp.path(), owner_id)
        .with_segment_max_bytes(16 * 1024)
        .with_ingress_capacity(8)
}

fn opaque_id(label: &str) -> [u8; 16] {
    let digest = Sha256::digest(label.as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes
}

fn code(value: &'static str) -> StaticCode {
    StaticCode::new(value).expect("static code")
}

fn event_id(value: &str) -> EventId {
    EventId::new(opaque_id(value))
}

fn instance_id(value: &str) -> InstanceId {
    InstanceId::new(opaque_id(value))
}

fn request_id(value: &str) -> RequestId {
    RequestId::new(opaque_id(value))
}

fn correlation_id(value: &str) -> CorrelationId {
    CorrelationId::new(opaque_id(value))
}

fn causation_id(value: &str) -> CausationId {
    CausationId::new(opaque_id(value))
}

fn task_id(value: &str) -> TaskId {
    TaskId::new(opaque_id(value))
}

fn run_id(value: &str) -> RunId {
    RunId::new(opaque_id(value))
}

fn lease_id(value: &str) -> LeaseId {
    LeaseId::new(opaque_id(value))
}

fn frame_id(value: &str) -> FrameId {
    FrameId::new(opaque_id(value))
}

fn action_id(value: &str) -> ActionId {
    ActionId::new(opaque_id(value))
}

fn recognition_id(value: &str) -> RecognitionId {
    RecognitionId::new(opaque_id(value))
}

#[test]
fn startup_timeout_returns_before_delayed_store_open_finishes() {
    let temp = TempDir::new().expect("temp");
    let startup_config = config(&temp, "writer-timeout");
    let delayed_config = startup_config.clone();
    let started = Instant::now();

    let error =
        GlobalLedger::open_with_store(startup_config, Duration::from_millis(20), move |_| {
            thread::sleep(Duration::from_millis(200));
            SegmentStore::open(delayed_config)
        })
        .expect_err("delayed startup must time out");

    assert_eq!(error.code(), "writer_start_timeout");
    assert!(started.elapsed() < Duration::from_millis(150));

    thread::sleep(Duration::from_millis(250));
    let segment_lengths = segment_paths(temp.path())
        .into_iter()
        .map(|path| fs::metadata(path).expect("segment metadata").len())
        .collect::<Vec<_>>();
    assert!(
        segment_lengths.iter().all(|length| *length == 0),
        "timed-out startup wrote unexpected segment bytes: {segment_lengths:?}"
    );
    let replacement =
        GlobalLedger::open(config(&temp, "writer-after-timeout")).unwrap_or_else(|error| {
            let segments = segment_paths(temp.path())
                .into_iter()
                .map(|path| {
                    let bytes = fs::read(&path).expect("segment bytes");
                    (path, bytes)
                })
                .collect::<Vec<_>>();
            panic!("timed-out writer must release ownership: {error:?}; segments={segments:?}");
        });
    replacement.close().expect("close replacement writer");
}

#[test]
fn empty_ledger_reopens_without_treating_the_segment_as_a_blank_record() {
    let temp = TempDir::new().expect("temp");
    GlobalLedger::open(config(&temp, "first-owner"))
        .expect("first owner")
        .close()
        .expect("close first owner");

    GlobalLedger::open(config(&temp, "second-owner"))
        .expect("reopen empty ledger")
        .close()
        .expect("close second owner");
}

fn event(event_label: &str) -> actingcommand_contract::SanitizedEventDraft {
    event_with_links(event_label, EventLinks::default(), AuditInput::new())
}

fn event_with_links(
    event_label: &str,
    links: EventLinks,
    audit: AuditInput,
) -> actingcommand_contract::SanitizedEventDraft {
    let payload = CommandPayloadDraft::received(code("runtime.start"), audit);
    EventDraft::new(
        event_id(event_label),
        1_752_147_200_000,
        EventSeverity::Info,
        EventOrigin::new(EventSource::Cli, code("actingctl"), EventActor::User),
        links,
        payload.into(),
    )
    .sanitize(&Sha256SecretFingerprinter::new(b"test-private-salt").expect("fingerprinter"))
    .expect("sanitize")
}

#[test]
fn query_filters_by_sequence_and_all_typed_correlation_ids() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
    ledger.append(event("evt-before")).expect("append before");
    let links = EventLinks::default()
        .with_instance_id(instance_id("instance-1"))
        .with_request_id(request_id("request-1"))
        .with_correlation_id(correlation_id("correlation-1"))
        .with_causation_id(causation_id("causation-1"))
        .with_task_id(task_id("task-1"))
        .with_run_id(run_id("run-1"))
        .with_lease_id(lease_id("lease-1"))
        .with_frame_id(frame_id("frame-1"))
        .with_action_id(action_id("action-1"))
        .with_recognition_id(recognition_id("reco-1"));
    let correlated = ledger
        .append(event_with_links(
            "evt-correlated",
            links.clone(),
            AuditInput::new(),
        ))
        .expect("append correlated");
    ledger.append(event("evt-after")).expect("append after");

    let filters = [
        EventQuery {
            instance_id: links.instance_id().copied(),
            ..EventQuery::default()
        },
        EventQuery {
            request_id: links.request_id().copied(),
            ..EventQuery::default()
        },
        EventQuery {
            correlation_id: links.correlation_id().copied(),
            ..EventQuery::default()
        },
        EventQuery {
            causation_id: links.causation_id().copied(),
            ..EventQuery::default()
        },
        EventQuery {
            task_id: links.task_id().copied(),
            ..EventQuery::default()
        },
        EventQuery {
            run_id: links.run_id().copied(),
            ..EventQuery::default()
        },
        EventQuery {
            lease_id: links.lease_id().copied(),
            ..EventQuery::default()
        },
        EventQuery {
            frame_id: links.frame_id().copied(),
            ..EventQuery::default()
        },
        EventQuery {
            action_id: links.action_id().copied(),
            ..EventQuery::default()
        },
        EventQuery {
            recognition_id: links.recognition_id().copied(),
            ..EventQuery::default()
        },
    ];
    for filter in filters {
        assert_eq!(
            ledger.query(filter).expect("query"),
            vec![correlated.clone()]
        );
    }
    assert_eq!(
        ledger
            .query(EventQuery {
                from_sequence: Some(correlated.sequence()),
                to_sequence: Some(correlated.sequence()),
                ..EventQuery::default()
            })
            .expect("sequence query"),
        vec![correlated]
    );
}

#[test]
fn subscription_replays_after_cursor_then_receives_live_events() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
    let first = ledger.append(event("evt-one")).expect("append one");
    let replay = ledger.append(event("evt-two")).expect("append two");

    let mut subscription = ledger
        .subscribe(SubscriptionCursor {
            after_sequence: first.sequence(),
        })
        .expect("subscribe");
    assert_eq!(
        subscription
            .recv_timeout(Duration::from_secs(1))
            .expect("replay event"),
        replay
    );

    let live = ledger.append(event("evt-three")).expect("append live");
    assert_eq!(
        subscription
            .recv_timeout(Duration::from_secs(1))
            .expect("live event"),
        live
    );
}

#[test]
fn subscription_reports_timeout_and_clean_close_separately() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
    let mut subscription = ledger
        .subscribe(SubscriptionCursor::default())
        .expect("subscribe");

    let timeout = subscription
        .recv_timeout(Duration::from_millis(50))
        .expect_err("empty subscription must time out");
    assert_eq!(timeout.code(), "subscription_timeout");
    assert!(!timeout.is_fatal());

    ledger.close().expect("close ledger");
    let closed = subscription
        .recv_timeout(Duration::from_millis(50))
        .expect_err("closed subscription must report closure");
    assert_eq!(closed.code(), "subscription_closed");
    assert!(!closed.is_fatal());
}

#[test]
fn dropped_subscription_does_not_block_remaining_live_subscribers() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one").with_subscription_capacity(1))
        .expect("ledger");
    drop(
        ledger
            .subscribe(SubscriptionCursor::default())
            .expect("dropped subscription"),
    );
    let mut active = ledger
        .subscribe(SubscriptionCursor::default())
        .expect("active subscription");

    let event = ledger.append(event("evt-active")).expect("append event");
    assert_eq!(
        active
            .recv_timeout(Duration::from_secs(1))
            .expect("active subscriber event"),
        event
    );
}

#[test]
fn dropped_subscription_response_does_not_register_a_live_sender() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
    let (response, dropped_receiver) = mpsc::sync_channel(1);
    drop(dropped_receiver);
    ledger
        .sender
        .as_ref()
        .expect("writer sender")
        .send(WriterCommand::Subscribe {
            cursor: SubscriptionCursor::default(),
            response,
        })
        .expect("enqueue dropped subscription response");
    let (count_response, count_receiver) = mpsc::sync_channel(1);
    ledger
        .sender
        .as_ref()
        .expect("writer sender")
        .send(WriterCommand::TestSubscriberCount {
            response: count_response,
        })
        .expect("enqueue subscriber count");

    assert_eq!(
        count_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("subscriber count"),
        0
    );
}

#[test]
fn dropped_future_cursor_subscription_is_pruned_before_cursor_is_crossed() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
    drop(
        ledger
            .subscribe(SubscriptionCursor {
                after_sequence: 100,
            })
            .expect("future subscription"),
    );

    ledger
        .append(event("evt-before-future-cursor"))
        .expect("append before cursor");
    let (count_response, count_receiver) = mpsc::sync_channel(1);
    ledger
        .sender
        .as_ref()
        .expect("writer sender")
        .send(WriterCommand::TestSubscriberCount {
            response: count_response,
        })
        .expect("request subscriber count");

    assert_eq!(
        count_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("subscriber count"),
        0
    );
}

#[test]
fn subscription_reports_terminal_writer_failure() {
    let temp = TempDir::new().expect("temp");
    let mut ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
    let mut subscription = ledger
        .subscribe(SubscriptionCursor::default())
        .expect("subscribe");
    let terminal = GlobalLedgerError::fatal("test_terminal", "test_writer_failure");

    let (count_response, count_receiver) = mpsc::sync_channel(1);
    ledger
        .sender
        .as_ref()
        .expect("writer sender")
        .send(WriterCommand::TestSubscriberCount {
            response: count_response,
        })
        .expect("request subscriber count");
    assert_eq!(
        count_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("subscriber count"),
        1
    );

    ledger
        .sender
        .as_ref()
        .expect("writer sender")
        .send(WriterCommand::TestTerminalFailure {
            error: terminal.clone(),
        })
        .expect("inject terminal failure");

    let received = subscription
        .recv_timeout(Duration::from_secs(1))
        .expect_err("terminal writer error must reach subscription");
    assert_eq!(received, terminal);
    assert!(received.is_fatal());

    ledger.sender.take();
    let writer = ledger.writer.take().expect("writer handle");
    assert_eq!(
        writer
            .join()
            .expect("writer must not panic")
            .expect_err("writer must return terminal error"),
        terminal
    );
}

#[test]
fn slow_subscription_receives_bounded_lag_failure() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one").with_subscription_capacity(1))
        .expect("ledger");
    let mut subscription = ledger
        .subscribe(SubscriptionCursor::default())
        .expect("subscribe");

    ledger.append(event("evt-lag-one")).expect("first event");
    ledger.append(event("evt-lag-two")).expect("second event");

    let error = subscription
        .recv_timeout(Duration::from_secs(1))
        .expect_err("lagged subscriber must receive fatal status");
    assert_eq!(error.code(), "subscription_lagged");
    assert!(error.is_fatal());
}

#[test]
fn subscription_suppresses_events_before_a_future_cursor() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
    let mut subscription = ledger
        .subscribe(SubscriptionCursor { after_sequence: 3 })
        .expect("subscribe");

    for event_id in ["evt-one", "evt-two", "evt-three"] {
        ledger
            .append(event(event_id))
            .expect("append suppressed event");
    }
    let timeout = subscription
        .recv_timeout(Duration::from_millis(50))
        .expect_err("future cursor must suppress earlier live events");
    assert_eq!(timeout.code(), "subscription_timeout");

    let visible = ledger
        .append(event("evt-four"))
        .expect("append visible event");
    assert_eq!(
        subscription
            .recv_timeout(Duration::from_secs(1))
            .expect("future cursor event"),
        visible
    );
}

#[test]
fn cli_projection_is_concise_and_correlated() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
    ledger
        .append(event_with_links(
            "evt-cli",
            EventLinks::default().with_correlation_id(correlation_id("correlation-cli")),
            AuditInput::new(),
        ))
        .expect("append");

    let projected = ledger
        .project(
            EventQuery {
                correlation_id: Some(correlation_id("correlation-cli")),
                ..EventQuery::default()
            },
            ProjectionProfile::Cli,
        )
        .expect("project");

    assert_eq!(projected.len(), 1);
    assert_eq!(
        projected[0].links.correlation_id(),
        Some(&correlation_id("correlation-cli"))
    );
    assert!(matches!(projected[0].payload, ProjectionPayload::Omitted));
}

#[test]
fn ui_projection_exposes_sanitized_state_without_secret_fields() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
    let secret = "C:\\private\\token";
    ledger
        .append(event_with_links(
            "evt-ui",
            EventLinks::default(),
            AuditInput::new().with_account(secret),
        ))
        .expect("append");

    let projected = ledger
        .project(EventQuery::default(), ProjectionProfile::Ui)
        .expect("project");

    assert_eq!(projected.len(), 1);
    let payload = serde_json::to_string(&projected[0].payload).expect("sanitized payload");
    assert!(matches!(projected[0].payload, ProjectionPayload::Public(_)));
    assert!(!payload.contains(secret));
}

#[test]
fn ui_projection_hides_forensic_fields_while_lab_retains_them() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
    let persisted = ledger
        .append(event_with_links(
            "evt-projection-separation",
            EventLinks::default(),
            AuditInput::new()
                .with_account("secret-value")
                .with_authentication("authentication-value")
                .with_machine_path("internal-value"),
        ))
        .expect("append");

    let ui = ledger
        .project(EventQuery::default(), ProjectionProfile::Ui)
        .expect("UI project");
    let ui_payload = serde_json::to_string(&ui[0].payload).expect("UI payload");
    assert!(!ui_payload.contains("internal-value"));
    assert!(!ui_payload.contains("sha256:"));
    assert!(!ui_payload.contains("authentication_redacted"));

    let normal = ledger
        .project(EventQuery::default(), ProjectionProfile::Normal)
        .expect("Normal project");
    assert_eq!(normal[0].payload, ui[0].payload);

    let lab = ledger
        .project(EventQuery::default(), ProjectionProfile::Lab)
        .expect("Lab project");
    let lab_payload = serde_json::to_string(&lab[0].payload).expect("Lab payload");
    assert!(!lab_payload.contains("internal-value"));
    assert!(lab_payload.contains("[redacted]"));
    assert!(lab_payload.contains("sha256:"));
    assert!(lab_payload.contains("authentication_redacted"));
    assert_eq!(lab[0].schema_version, persisted.schema_version());
    assert_eq!(lab[0].sensitivity, persisted.sensitivity());
}

#[test]
fn lab_projection_exposes_full_sanitized_fact() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
    let persisted = ledger
        .append(event_with_links(
            "evt-lab",
            EventLinks::default().with_run_id(run_id("run-lab")),
            AuditInput::new(),
        ))
        .expect("append");

    let projected = ledger
        .project(EventQuery::default(), ProjectionProfile::Lab)
        .expect("project");

    assert_eq!(projected.len(), 1);
    assert_eq!(projected[0].sequence, persisted.sequence());
    assert_eq!(projected[0].schema_version, persisted.schema_version());
    assert_eq!(projected[0].sensitivity, persisted.sensitivity());
    assert_eq!(&projected[0].links, persisted.links());
    assert_eq!(
        projected[0].payload,
        ProjectionPayload::Full(persisted.payload().clone())
    );
    assert!(projected[0].artifacts.is_empty());
}

#[test]
fn indexes_rebuild_after_reopen() {
    let temp = TempDir::new().expect("temp");
    let links = EventLinks::default()
        .with_request_id(request_id("request-reopen"))
        .with_correlation_id(correlation_id("correlation-reopen"));
    let first = GlobalLedger::open(config(&temp, "writer-one")).expect("first ledger");
    let appended = first
        .append(event_with_links(
            "evt-reopen",
            links.clone(),
            AuditInput::new(),
        ))
        .expect("append");
    first.close().expect("close first");

    let reopened = GlobalLedger::open(config(&temp, "writer-two")).expect("reopen");
    assert_eq!(
        reopened
            .query(EventQuery {
                request_id: links.request_id().copied(),
                correlation_id: links.correlation_id().copied(),
                ..EventQuery::default()
            })
            .expect("query rebuilt index"),
        vec![appended]
    );
}

#[test]
fn query_intersects_multiple_links_in_sequence_order() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
    let first = ledger
        .append(event_with_links(
            "evt-intersection-first",
            EventLinks::default()
                .with_instance_id(instance_id("instance-intersection"))
                .with_request_id(request_id("request-intersection")),
            AuditInput::new(),
        ))
        .expect("first matching event");
    ledger
        .append(event_with_links(
            "evt-intersection-other",
            EventLinks::default()
                .with_instance_id(instance_id("instance-intersection"))
                .with_request_id(request_id("request-other")),
            AuditInput::new(),
        ))
        .expect("nonmatching event");
    let second = ledger
        .append(event_with_links(
            "evt-intersection-second",
            EventLinks::default()
                .with_instance_id(instance_id("instance-intersection"))
                .with_request_id(request_id("request-intersection")),
            AuditInput::new(),
        ))
        .expect("second matching event");

    let matching = ledger
        .query(EventQuery {
            instance_id: Some(instance_id("instance-intersection")),
            request_id: Some(request_id("request-intersection")),
            ..EventQuery::default()
        })
        .expect("intersection query");
    assert_eq!(matching, vec![first.clone(), second.clone()]);
    assert_eq!(
        ledger
            .query(EventQuery {
                instance_id: Some(instance_id("instance-intersection")),
                request_id: Some(request_id("request-intersection")),
                from_sequence: Some(second.sequence()),
                ..EventQuery::default()
            })
            .expect("bounded intersection query"),
        vec![second]
    );
}

fn segment_paths(root: &Path) -> Vec<PathBuf> {
    let mut paths = fs::read_dir(root.join("segments"))
        .expect("segments")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

fn read_events(root: &Path) -> Vec<PersistedEvent> {
    let mut events = Vec::new();
    for path in segment_paths(root) {
        let text = fs::read_to_string(path).expect("segment text");
        for line in text.lines() {
            let value: Value = serde_json::from_str(line).expect("line JSON");
            let stored: StoredEventRecord =
                serde_json::from_value(value["event"].clone()).expect("stored event");
            events.push(stored.into_event().expect("persisted event"));
        }
    }
    events
}

fn write_owner_metadata(root: &Path, active: bool, valid: bool) {
    fs::create_dir_all(root).expect("root");
    let content = if valid {
        serde_json::json!({
            "schema_version": "actingcommand.ledger-writer.v1",
            "owner_id": "previous-owner",
            "pid": 999_999_u32,
            "active": active,
            "started_at_unix_ms": 1_u64,
            "closed_at_unix_ms": Value::Null
        })
        .to_string()
    } else {
        "{not-json".to_string()
    };
    fs::write(root.join("writer.lock"), content).expect("owner metadata");
}

#[test]
fn sha256_fingerprinter_requires_non_empty_private_salt() {
    let error = Sha256SecretFingerprinter::new([]).expect_err("empty fingerprinter salt must fail");

    assert_eq!(error.code(), "invalid_redactor_config");
}

#[test]
fn sha256_fingerprinter_returns_fixed_lowercase_fingerprint() {
    let fingerprinter = Sha256SecretFingerprinter::new(b"private-salt").expect("fingerprinter");

    let fingerprint = fingerprinter
        .fingerprint(SecretField::AccountIdentity, "secret-value")
        .expect("fingerprint");

    assert!(fingerprint.as_str().starts_with("sha256:"));
    assert_eq!(fingerprint.as_str().len(), 71);
    assert!(
        fingerprint.as_str()[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    );
    assert!(!fingerprint.as_str().contains("secret-value"));
}

#[test]
fn config_debug_hides_machine_path() {
    let temp = TempDir::new().expect("temp");
    let config = config(&temp, "writer-one");

    let debug = format!("{config:?}");

    assert!(!debug.contains(&temp.path().display().to_string()));
    assert!(debug.contains("<redacted-root>"));
}

#[test]
fn shutdown_waits_for_a_full_ingress_queue_to_drain() {
    let (sender, receiver) = mpsc::sync_channel(1);
    let (prefill_response, _prefill_receiver) = mpsc::sync_channel(1);
    sender
        .send(WriterCommand::Shutdown {
            response: prefill_response,
        })
        .expect("fill queue");
    let writer = thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        let _ = receiver.recv().expect("prefill");
        if let WriterCommand::Shutdown { response } = receiver.recv().expect("shutdown") {
            response.send(Ok(())).expect("shutdown response");
        }
        Ok(())
    });
    let ledger = GlobalLedger {
        sender: Some(sender),
        writer: Some(writer),
    };
    let (done_sender, done_receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = done_sender.send(ledger.close());
    });

    let result = done_receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("close must not deadlock");

    result.expect("close");
}

#[test]
fn second_writer_is_rejected_while_first_is_alive() {
    let temp = TempDir::new().expect("temp");
    let first = GlobalLedger::open(config(&temp, "writer-one")).expect("first writer");

    let error =
        GlobalLedger::open(config(&temp, "writer-two")).expect_err("second writer must fail");

    assert_eq!(error.code(), "writer_conflict");
    first.close().expect("close first");
}

#[test]
fn malformed_writer_metadata_is_fatal_without_path_disclosure() {
    let temp = TempDir::new().expect("temp");
    write_owner_metadata(temp.path(), true, false);

    let error =
        GlobalLedger::open(config(&temp, "writer-one")).expect_err("malformed metadata must fail");

    assert_eq!(error.code(), "malformed_owner_metadata");
    assert!(
        !error
            .to_string()
            .contains(&temp.path().display().to_string())
    );
}

#[test]
fn contradictory_writer_metadata_is_fatal() {
    let cases = [
        serde_json::json!({
            "schema_version": "actingcommand.ledger-writer.v1",
            "owner_id": "previous-owner",
            "pid": 42,
            "active": true,
            "started_at_unix_ms": 10,
            "closed_at_unix_ms": 11
        }),
        serde_json::json!({
            "schema_version": "actingcommand.ledger-writer.v1",
            "owner_id": "previous-owner",
            "pid": 42,
            "active": false,
            "started_at_unix_ms": 10,
            "closed_at_unix_ms": Value::Null
        }),
        serde_json::json!({
            "schema_version": "actingcommand.ledger-writer.v1",
            "owner_id": "previous-owner",
            "pid": 42,
            "active": false,
            "started_at_unix_ms": 10,
            "closed_at_unix_ms": 9
        }),
    ];
    for metadata in cases {
        let temp = TempDir::new().expect("temp");
        fs::write(temp.path().join("writer.lock"), metadata.to_string()).expect("metadata");

        let error = GlobalLedger::open(config(&temp, "writer-new"))
            .expect_err("contradictory metadata must fail");

        assert_eq!(error.code(), "malformed_owner_metadata");
    }
}

#[test]
fn stale_active_owner_is_recovered_explicitly() {
    let temp = TempDir::new().expect("temp");
    write_owner_metadata(temp.path(), true, true);

    GlobalLedger::open(config(&temp, "writer-new"))
        .expect("recover writer")
        .close()
        .expect("close");

    let events = read_events(temp.path());
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type(), EventType::LedgerRecovered);
    assert_eq!(events[0].sequence(), 1);
}

#[test]
fn append_assigns_contiguous_sequences_across_reopen() {
    let temp = TempDir::new().expect("temp");
    let first = GlobalLedger::open(config(&temp, "writer-one")).expect("first");
    assert_eq!(
        first
            .append(event("evt-one"))
            .expect("append one")
            .sequence(),
        1
    );
    first.close().expect("close first");

    let second = GlobalLedger::open(config(&temp, "writer-two")).expect("second");
    assert_eq!(
        second
            .append(event("evt-two"))
            .expect("append two")
            .sequence(),
        2
    );
    second.close().expect("close second");

    let sequences = read_events(temp.path())
        .into_iter()
        .map(|event| event.sequence())
        .collect::<Vec<_>>();
    assert_eq!(sequences, vec![1, 2]);
}

#[test]
fn truncated_final_tail_is_quarantined_and_reported() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
    ledger.append(event("evt-one")).expect("append");
    ledger.close().expect("close");
    let segment = segment_paths(temp.path()).pop().expect("segment");
    OpenOptions::new()
        .append(true)
        .open(segment)
        .expect("open segment")
        .write_all(br#"{"line_type":"event""#)
        .expect("write tail");

    let recovered = GlobalLedger::open(config(&temp, "writer-two")).expect("recover");
    assert_eq!(
        recovered
            .append(event("evt-after-recovery"))
            .expect("append after recovery")
            .sequence(),
        3
    );
    recovered.close().expect("close recovered");

    let events = read_events(temp.path());
    assert_eq!(events[0].event_id(), &event_id("evt-one"));
    assert_eq!(events[1].event_type(), EventType::LedgerRecovered);
    assert_eq!(events[2].event_id(), &event_id("evt-after-recovery"));
    let quarantine_count = fs::read_dir(temp.path().join("quarantine"))
        .expect("quarantine")
        .count();
    assert_eq!(quarantine_count, 1);
}

#[test]
fn complete_corrupt_line_is_fatal() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
    ledger.append(event("evt-one")).expect("append");
    ledger.close().expect("close");
    let segment = segment_paths(temp.path()).pop().expect("segment");
    let mut file = OpenOptions::new()
        .append(true)
        .open(segment)
        .expect("open segment");
    file.write_all(b"not-json\n").expect("write corrupt line");

    let error =
        GlobalLedger::open(config(&temp, "writer-two")).expect_err("complete corruption must fail");

    assert_eq!(error.code(), "corrupt_segment");
}

#[test]
fn complete_blank_line_is_fatal() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
    ledger.append(event("evt-one")).expect("append");
    ledger.close().expect("close");
    let segment = segment_paths(temp.path()).pop().expect("segment");
    OpenOptions::new()
        .append(true)
        .open(segment)
        .expect("open segment")
        .write_all(b"\n")
        .expect("write blank record");

    let error = GlobalLedger::open(config(&temp, "writer-two"))
        .expect_err("blank complete record must fail");

    assert_eq!(error.code(), "corrupt_segment");
}

#[test]
fn duplicate_json_key_is_fatal_without_disclosing_the_hidden_value() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
    ledger.append(event("evt-one")).expect("append");
    ledger.close().expect("close");
    let segment = segment_paths(temp.path()).pop().expect("segment");
    let secret = r"C:\hidden\duplicate-subject";
    let text = fs::read_to_string(&segment).expect("segment text");
    let encoded_secret = serde_json::to_string(secret).expect("encode secret");
    let forged = text.replacen(
        r#""action":"runtime.start""#,
        &format!(r#""action":{encoded_secret},"action":"runtime.start""#),
        1,
    );
    assert_ne!(forged, text);
    fs::write(segment, forged).expect("write duplicate-key line");

    let error =
        GlobalLedger::open(config(&temp, "writer-two")).expect_err("duplicate key must fail");

    assert_eq!(error.code(), "corrupt_segment");
    assert!(!error.to_string().contains(secret));
}

#[test]
fn non_final_segment_corruption_is_fatal() {
    let temp = TempDir::new().expect("temp");
    let small = GlobalLedgerConfig::new(temp.path(), "writer-one")
        .with_segment_max_bytes(256)
        .with_ingress_capacity(8);
    let ledger = GlobalLedger::open(small).expect("ledger");
    ledger.append(event("evt-one")).expect("one");
    ledger.append(event("evt-two")).expect("two");
    ledger.append(event("evt-three")).expect("three");
    ledger.close().expect("close");
    let segments = segment_paths(temp.path());
    assert!(segments.len() >= 2);
    OpenOptions::new()
        .append(true)
        .open(&segments[0])
        .expect("first segment")
        .write_all(b"truncated")
        .expect("write corrupt tail");

    let error = GlobalLedger::open(config(&temp, "writer-two"))
        .expect_err("non-final corruption must fail");

    assert_eq!(error.code(), "corrupt_segment");
}

#[test]
fn duplicate_event_id_is_fatal() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
    ledger.append(event("evt-duplicate")).expect("first");

    let error = ledger
        .append(event("evt-duplicate"))
        .expect_err("duplicate event must fail");

    assert_eq!(error.code(), "duplicate_event_id");
    ledger.close().expect("close");
}
