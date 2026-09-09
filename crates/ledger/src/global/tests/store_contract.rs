// SPDX-License-Identifier: AGPL-3.0-only

// S0 specification corpus: the same assertions accept each store through the writer.
use super::*;

/// Captured from the public writer API; S2 compares this for the same minted inputs.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct EventTrace {
    events: Vec<PersistedEvent>,
    canonical_records: Vec<Value>,
    projections: Vec<Vec<ProjectedEvent>>,
    duplicate: GlobalLedgerError,
}

pub(super) fn event_trace(
    open: impl Fn(GlobalLedgerConfig) -> GlobalLedgerResult<GlobalLedger>,
    drafts: &[SanitizedEventDraft; 3],
) -> EventTrace {
    let temp = TempDir::new().expect("contract root");
    let ledger = open(config(&temp, "contract-first")).expect("open");
    assert_eq!(ledger.latest_sequence().expect("empty position"), 0);
    let first = ledger.append(drafts[0].clone()).expect("append first");
    let second = ledger.append(drafts[1].clone()).expect("append second");
    let mut subscription = ledger
        .subscribe_with_options(
            SubscriptionCursor { after_sequence: 1 },
            SubscriptionOptions::new(1).expect("one-event replay pages"),
        )
        .expect("subscribe at a committed cursor");
    assert_eq!(subscription.replay_through_sequence(), 2);
    let third = ledger.append(drafts[2].clone()).expect("append live");
    for expected in [&second, &third] {
        assert_eq!(
            &subscription
                .recv_timeout(Duration::from_secs(1))
                .expect("replay/live"),
            expected
        );
    }
    let events = vec![first, second, third];
    for (index, (actual, draft)) in events.iter().zip(drafts).enumerate() {
        assert_eq!(
            actual,
            &PersistedEvent::from_sanitized(index as u64 + 1, draft.clone())
                .expect("expected typed fact")
        );
    }
    let duplicate = ledger.append(drafts[0].clone()).expect_err("duplicate ID");
    assert_eq!(
        (
            duplicate.code(),
            duplicate.operation(),
            duplicate.is_fatal()
        ),
        ("duplicate_event_id", "append_event", false)
    );
    assert_eq!(
        ledger
            .latest_sequence()
            .expect("duplicate preserves position"),
        3
    );
    assert_eq!(
        ledger.query(EventQuery::default()).expect("all facts"),
        events
    );
    assert_eq!(
        ledger
            .query_page(EventQuery::default(), 0, 2, 2)
            .expect("pinned prefix"),
        events[..2]
    );
    let profiles = [
        ProjectionProfile::Cli,
        ProjectionProfile::Ui,
        ProjectionProfile::Lab,
        ProjectionProfile::Concise,
        ProjectionProfile::Normal,
        ProjectionProfile::Verbose,
        ProjectionProfile::Forensic,
    ];
    let projections: Vec<_> = profiles
        .iter()
        .map(|&profile| {
            let projected = ledger
                .project(EventQuery::default(), profile)
                .expect("projection");
            assert_eq!(projected.len(), events.len());
            assert_eq!(
                ledger
                    .project_page(EventQuery::default(), profile, 0, 2, 2)
                    .expect("projected prefix"),
                projected[..2]
            );
            for (actual, fact) in projected.iter().zip(&events) {
                assert_eq!(actual.event_id, *fact.event_id());
                assert_eq!(actual.sequence, fact.sequence());
                assert_eq!(actual.timestamp_unix_ms, fact.timestamp_unix_ms());
                assert_eq!(&actual.links, fact.links());
            }
            projected
        })
        .collect();
    for projected in &projections[0] {
        assert_eq!(projected.payload, ProjectionPayload::Omitted);
    }
    for (projected, fact) in projections[2].iter().zip(&events) {
        assert_eq!(
            projected.payload,
            ProjectionPayload::Full(Box::new(fact.payload().clone()))
        );
    }
    ledger.close().expect("close first owner");
    let closed = subscription
        .recv_timeout(Duration::from_secs(1))
        .expect_err("clean close");
    assert_eq!(
        (closed.code(), closed.is_fatal()),
        ("subscription_closed", false)
    );
    let reopened = open(config(&temp, "contract-second")).expect("recover committed facts");
    assert_eq!(reopened.latest_sequence().expect("recovered position"), 3);
    assert_eq!(
        reopened
            .query(EventQuery::default())
            .expect("recovered facts"),
        events
    );
    for (&profile, expected) in profiles.iter().zip(&projections) {
        assert_eq!(
            &reopened
                .project(EventQuery::default(), profile)
                .expect("recovered projection"),
            expected
        );
    }
    reopened.close().expect("close recovered owner");
    let canonical_records = events
        .iter()
        .map(|event| {
            serde_json::to_value(StoredEventRecord::from_event(event))
                .expect("canonical typed record")
        })
        .collect();
    EventTrace {
        events,
        canonical_records,
        projections,
        duplicate,
    }
}

// Workflow #109 S0: bounded storage-equivalence specification, executed only by CI.
#[test]
fn segment_store_event_trace_preserves_the_shared_typed_corpus() {
    let links = EventLinksDraft::default()
        .with_run_id(run_id())
        .with_request_id(request_id());
    let drafts = [
        event_with_links("contract-first", links.clone(), AuditInput::new()),
        event_with_links("contract-second", links, AuditInput::new()),
        event("contract-live"),
    ];
    let trace = event_trace(GlobalLedger::open, &drafts);
    assert_eq!(trace.events.len(), 3);
    assert_eq!(trace.canonical_records.len(), 3);
    assert_eq!(trace.projections.len(), 7);
    assert_eq!(trace.duplicate.code(), "duplicate_event_id");
}

pub(super) fn query_filters_by_sequence_and_all_typed_correlation_ids(
    open: impl Fn(GlobalLedgerConfig) -> GlobalLedgerResult<GlobalLedger>,
) {
    let temp = TempDir::new().expect("temp");
    let ledger = open(config(&temp, "writer-one")).expect("ledger");
    ledger.append(event("evt-before")).expect("append before");
    let links = EventLinksDraft::default()
        .with_instance_id(instance_id())
        .with_request_id(request_id())
        .with_correlation_id(correlation_id())
        .with_causation_id(causation_id())
        .with_task_id(task_id())
        .with_run_id(run_id())
        .with_lease_id(lease_id())
        .with_frame_id(frame_id())
        .with_action_id(action_id())
        .with_recognition_id(recognition_id());
    let correlated = ledger
        .append(
            EventDraft::new(
                event_id(),
                1_752_147_200_000,
                EventSeverity::Error,
                EventOrigin::new(
                    EventSource::Runtime,
                    OriginModule::Runtime,
                    EventActor::Runtime,
                ),
                links.clone(),
                CommandPayloadDraft::rejected(
                    EventAction::RuntimeStart,
                    DiagnosticCode::CommandRejected,
                    EffectDisposition::NotPerformed,
                    AuditInput::new(),
                )
                .into(),
            )
            .sanitize(&Sha256SecretFingerprinter::new(b"test-private-salt").expect("fingerprinter"))
            .expect("sanitize query event"),
        )
        .expect("append correlated");
    ledger.append(event("evt-after")).expect("append after");

    let filters = [
        EventQuery {
            origin_module: Some(OriginModule::Runtime),
            ..EventQuery::default()
        },
        EventQuery {
            diagnostic_code: Some(DiagnosticCode::CommandRejected),
            ..EventQuery::default()
        },
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
        vec![correlated.clone()]
    );
    let combined = EventQuery {
        from_sequence: Some(correlated.sequence()),
        to_sequence: Some(correlated.sequence()),
        event_type: Some(correlated.event_type()),
        minimum_severity: Some(EventSeverity::Warning),
        source: Some(EventSource::Runtime),
        origin_module: Some(OriginModule::Runtime),
        diagnostic_code: Some(DiagnosticCode::CommandRejected),
        instance_id: links.instance_id().copied(),
        request_id: links.request_id().copied(),
        correlation_id: links.correlation_id().copied(),
        causation_id: links.causation_id().copied(),
        task_id: links.task_id().copied(),
        run_id: links.run_id().copied(),
        lease_id: links.lease_id().copied(),
        frame_id: links.frame_id().copied(),
        action_id: links.action_id().copied(),
        recognition_id: links.recognition_id().copied(),
    };
    assert_eq!(
        ledger.query(combined.clone()).unwrap(),
        vec![correlated.clone()]
    );
    assert!(project_subscription_event(&correlated, &combined, ProjectionProfile::Lab).is_some());
    let snapshot =
        GlobalLedger::open_read_only(GlobalLedgerReadOnlyConfig::new(temp.path()), |_| None)
            .expect("read-only query index");
    assert_eq!(snapshot.query(&combined), vec![correlated.clone()]);
    assert_eq!(
        snapshot
            .query_page(&combined, 0, correlated.sequence(), 1)
            .unwrap(),
        ledger
            .query_page(combined.clone(), 0, correlated.sequence(), 1)
            .unwrap(),
    );
    for mismatch in [
        EventQuery {
            origin_module: Some(OriginModule::Actingctl),
            ..combined.clone()
        },
        EventQuery {
            diagnostic_code: Some(DiagnosticCode::LeaseBusy),
            ..combined.clone()
        },
        EventQuery {
            minimum_severity: Some(EventSeverity::Fatal),
            ..combined.clone()
        },
        EventQuery {
            source: Some(EventSource::Cli),
            ..combined.clone()
        },
        EventQuery {
            to_sequence: Some(correlated.sequence() - 1),
            ..combined.clone()
        },
    ] {
        assert!(ledger.query(mismatch.clone()).unwrap().is_empty());
        assert!(snapshot.query(&mismatch).is_empty());
        assert!(
            project_subscription_event(&correlated, &mismatch, ProjectionProfile::Lab).is_none()
        );
    }
    ledger.close().unwrap();
    let reopened = open(config(&temp, "query-index-reopened")).unwrap();
    assert_eq!(reopened.query(combined).unwrap(), vec![correlated]);
    reopened.close().unwrap();
}

pub(super) fn query_pages_are_bounded_ordered_and_pinned_to_the_requested_snapshot(
    open: impl Fn(GlobalLedgerConfig) -> GlobalLedgerResult<GlobalLedger>,
) {
    let temp = TempDir::new().expect("temp");
    let ledger = open(config(&temp, "writer-paged-query")).expect("ledger");
    let mut expected = Vec::new();
    for index in 0..9 {
        expected.push(
            ledger
                .append(event(&format!("evt-page-{index}")))
                .expect("append paged event"),
        );
    }
    let snapshot = expected[6].sequence();

    let first = ledger
        .query_page(EventQuery::default(), 0, snapshot, 3)
        .expect("first page");
    let second = ledger
        .query_page(
            EventQuery::default(),
            first.last().expect("first tail").sequence(),
            snapshot,
            3,
        )
        .expect("second page");
    let third = ledger
        .query_page(
            EventQuery::default(),
            second.last().expect("second tail").sequence(),
            snapshot,
            3,
        )
        .expect("third page");
    let collected = first
        .into_iter()
        .chain(second)
        .chain(third)
        .collect::<Vec<_>>();
    assert_eq!(collected, expected[..7]);
    assert!(
        ledger
            .query_page(EventQuery::default(), snapshot, snapshot, 3)
            .expect("terminal page")
            .is_empty()
    );
    assert_eq!(
        ledger
            .query_page(EventQuery::default(), 0, snapshot, 0)
            .expect_err("zero page size")
            .code(),
        "invalid_query_page"
    );
}

pub(super) fn indexes_rebuild_after_reopen(
    open: impl Fn(GlobalLedgerConfig) -> GlobalLedgerResult<GlobalLedger>,
) {
    let temp = TempDir::new().expect("temp");
    let links = EventLinksDraft::default()
        .with_request_id(request_id())
        .with_correlation_id(correlation_id());
    let first = open(config(&temp, "writer-one")).expect("first ledger");
    let appended = first
        .append(event_with_links(
            "evt-reopen",
            links.clone(),
            AuditInput::new(),
        ))
        .expect("append");
    first.close().expect("close first");

    let reopened = open(config(&temp, "writer-two")).expect("reopen");
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

pub(super) fn scheduled_settlement_continuation_is_one_shot_and_idempotent(
    open: impl Fn(GlobalLedgerConfig) -> GlobalLedgerResult<GlobalLedger>,
) {
    let temp = TempDir::new().expect("temp");
    let ledger = open(config(&temp, "one-shot-settlement-writer")).expect("ledger");
    let drafts = scheduled_recovery_drafts();
    append_scheduled_recovery_prefix(&ledger, &drafts);

    let first = reconcile_scheduled_settlement(&ledger, &drafts.execution)
        .expect("first settlement continuation");
    let second = reconcile_scheduled_settlement(&ledger, &drafts.execution)
        .expect("second settlement continuation");

    assert_eq!(first.event_id(), second.event_id());
    let events = ledger.query(EventQuery::default()).expect("query facts");
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type() == EventType::PolicyExecutionRecorded)
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type() == EventType::PolicyDispatchCompleted)
            .count(),
        1
    );
    ledger.close().expect("close ledger");
}

pub(super) fn subscription_reports_terminal_writer_failure(
    open: impl Fn(GlobalLedgerConfig) -> GlobalLedgerResult<GlobalLedger>,
) {
    let temp = TempDir::new().expect("temp");
    let mut ledger = open(config(&temp, "writer-one")).expect("ledger");
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

pub(super) fn scheduling_projection_rejects_a_second_same_run_terminal_with_another_request(
    open: impl Fn(GlobalLedgerConfig) -> GlobalLedgerResult<GlobalLedger>,
) {
    let temp = TempDir::new().expect("temp");
    let ledger = open(config(&temp, "projection-owner")).expect("ledger");
    let drafts = scheduled_recovery_drafts();
    ledger.append(drafts.intent.clone()).expect("intent");
    ledger
        .append(drafts.lease_granted.clone())
        .expect("lease grant");
    let admission = ledger.append(drafts.admission.clone()).expect("admission");
    ledger
        .append(drafts.task_request.clone())
        .expect("task request");
    let terminal = ledger
        .append(drafts.task_completed_mapped.clone())
        .expect("mapped terminal");
    let EventPayload::Policy(PolicyPayload::DispatchAdmitted(admission_payload)) =
        admission.payload()
    else {
        panic!("admission payload");
    };
    let links = terminal.links();
    let expected = SchedulingOutcomeIdentity::new(
        *terminal.event_id(),
        terminal.sequence(),
        *links.instance_id().expect("terminal instance"),
        *links.task_id().expect("terminal task"),
        *links.run_id().expect("terminal run"),
        *links.request_id().expect("terminal request"),
        *links.correlation_id().expect("terminal correlation"),
        *links.lease_id().expect("terminal lease"),
        admission_payload.decision_id(),
        admission_payload.task_id(),
        admission_payload.instance_id(),
    )
    .expect("projection identity");
    let projected = ledger
        .project_scheduling_outcomes(expected.clone(), terminal.sequence())
        .expect("single mapped terminal projects");
    assert_eq!(
        projected.outcome().disposition().outcome_key(),
        "mapped-result"
    );
    let stale = ledger
        .project_scheduling_outcomes(expected.clone(), terminal.sequence() - 1)
        .expect_err("projection below the terminal sequence is not ready");
    assert_eq!(stale.code(), "outcome_projection_not_ready");
    let future = ledger
        .project_scheduling_outcomes(expected.clone(), terminal.sequence() + 1)
        .expect_err("caller cannot claim a future ledger position");
    assert_eq!(future.code(), "outcome_projection_position_invalid");

    let duplicate = ledger
        .append(drafts.task_completed_mapped_other_request.clone())
        .expect("second-request terminal fixture");
    assert_ne!(
        terminal.links().request_id(),
        duplicate.links().request_id(),
        "the regression requires a newly minted terminal request"
    );
    let error = ledger
        .project_scheduling_outcomes(expected, duplicate.sequence())
        .expect_err("same run cannot project through two request-partitioned terminals");
    assert_eq!(error.code(), "outcome_projection_terminal_not_unique");
}

pub(super) fn signature_catalog_rebuilds_versions_and_binds_paginated_multiple_matches(
    open: impl Fn(GlobalLedgerConfig) -> GlobalLedgerResult<GlobalLedger>,
) {
    use crate::signatures::{
        SignatureCatalog, SignaturePrefix, registration_ref, replay_signatures,
    };
    use actingcommand_contract::{
        DiagnosticSignatureDefinition, LedgerPayloadDraft, LedgerSignatureEvent,
        SignaturePageRequest, SignatureReplayGap, SignatureReplayRow,
    };
    let temp = TempDir::new().unwrap();
    let ledger = open(config(&temp, "signature-spec")).unwrap();
    let append = |payload: EventPayloadDraft, origin, severity| {
        ledger
            .append(
                EventDraft::new(
                    event_id(),
                    1,
                    severity,
                    origin,
                    EventLinksDraft::default(),
                    payload,
                )
                .sanitize(&Sha256SecretFingerprinter::new(b"signature-spec").unwrap())
                .unwrap(),
            )
            .unwrap()
    };
    let origin = || {
        EventOrigin::new(
            EventSource::Lab,
            OriginModule::GlobalLedger,
            EventActor::Lab,
        )
    };
    let definition = |id: &str| DiagnosticSignatureDefinition {
        signature_id: id.into(),
        version: 1,
        origin_module: OriginModule::Runtime,
        diagnostic_code: DiagnosticCode::CommandRejected,
        event_type: EventType::CommandRejected,
        minimum_severity: EventSeverity::Error,
        lifecycle: None,
    };
    let a = definition("close_a");
    let b = definition("close_b");
    let registration = append(
        LedgerPayloadDraft::signature(
            LedgerSignatureEvent::Registered {
                definition: a.clone(),
            },
            AuditInput::new(),
        )
        .into(),
        origin(),
        EventSeverity::Info,
    );
    let registration_a = registration_ref(&registration, &a);
    append(
        LedgerPayloadDraft::signature(
            LedgerSignatureEvent::Registered { definition: b },
            AuditInput::new(),
        )
        .into(),
        origin(),
        EventSeverity::Info,
    );
    let source = append(
        CommandPayloadDraft::rejected(
            EventAction::RuntimeStart,
            DiagnosticCode::CommandRejected,
            EffectDisposition::NotPerformed,
            AuditInput::new(),
        )
        .into(),
        EventOrigin::new(
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
        ),
        EventSeverity::Error,
    );
    let input = SignaturePrefix::from_live(&ledger, 3).unwrap();
    let catalog_prefix = SignaturePrefix::from_live(&ledger, 2).unwrap();
    let catalog = SignatureCatalog::from_prefix(&catalog_prefix);
    assert!(catalog.validate_registration(&a).is_err());
    catalog.validate_retirement(&registration_a).unwrap();
    let first = replay_signatures(
        &input,
        &catalog,
        &SignaturePageRequest {
            limit: 1,
            cursor: None,
        },
    )
    .unwrap();
    assert_eq!(first.matched_count, 2);
    assert!(first.evidence_complete());
    assert!(matches!(&first.rows[0], SignatureReplayRow::Matched {
        registration, source_event_id, source_sequence: 3
    } if registration == &registration_a && source_event_id == source.event_id()));
    let cursor = first.next_cursor.clone().unwrap();
    let second_request = SignaturePageRequest {
        limit: 1,
        cursor: Some(cursor.clone()),
    };
    let second = replay_signatures(&input, &catalog, &second_request).unwrap();
    assert_eq!(second.matched_count, 2);
    assert!(second.next_cursor.is_none());
    assert_eq!(
        second,
        replay_signatures(&input, &catalog, &second_request).unwrap()
    );
    append(
        LedgerPayloadDraft::signature(
            LedgerSignatureEvent::Retired {
                registration: registration_a.clone(),
            },
            AuditInput::new(),
        )
        .into(),
        origin(),
        EventSeverity::Info,
    );
    let retired_prefix = SignaturePrefix::from_live(&ledger, 4).unwrap();
    let retired = SignatureCatalog::from_prefix(&retired_prefix);
    assert!(retired.validate_retirement(&registration_a).is_err());
    let mut next = a.clone();
    next.version = 2;
    retired.validate_registration(&next).unwrap();
    assert!(replay_signatures(&input, &retired, &second_request).is_err());
    assert_eq!(
        replay_signatures(&input, &catalog, &second_request).unwrap(),
        second
    );
    append(
        LedgerPayloadDraft::signature(
            LedgerSignatureEvent::Registered { definition: next },
            AuditInput::new(),
        )
        .into(),
        origin(),
        EventSeverity::Info,
    );
    let updated_prefix = SignaturePrefix::from_live(&ledger, 5).unwrap();
    let updated = SignatureCatalog::from_prefix(&updated_prefix);
    let result = replay_signatures(&input, &updated, &SignaturePageRequest::default()).unwrap();
    assert!(
        matches!(&result.rows[0], SignatureReplayRow::Matched { registration, .. }
        if registration.version == 2 && registration.sequence == 5)
    );
    let incomplete = SignaturePrefix::from_live(&ledger, 6).unwrap();
    let page = replay_signatures(&incomplete, &catalog, &SignaturePageRequest::default()).unwrap();
    assert!(!page.evidence_complete());
    assert!(page.gaps.contains(&SignatureReplayGap::InputIncomplete));
    append(
        LedgerPayloadDraft::signature(
            LedgerSignatureEvent::Registered { definition: a },
            AuditInput::new(),
        )
        .into(),
        origin(),
        EventSeverity::Info,
    );
    let invalid_prefix = SignaturePrefix::from_live(&ledger, 6).unwrap();
    let invalid = SignatureCatalog::from_prefix(&invalid_prefix);
    let result = replay_signatures(&input, &invalid, &SignaturePageRequest::default()).unwrap();
    assert!(
        result
            .gaps
            .contains(&SignatureReplayGap::CatalogTransitionInvalid)
    );
    ledger.close().unwrap();
}
