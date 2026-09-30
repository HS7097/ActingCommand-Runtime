// SPDX-License-Identifier: AGPL-3.0-only

// one-off (to be reverted): Workflow #332 R-san evidence on CI. A capture with no run and no
// lease whose only close (its epoch's latest quiescence) precedes its Explicit pin (A), the
// control with a later quiescence (B), and the retention append error for a release that does
// not follow its pin (C). Real SQLite ledger root and ArtifactStore in the same directory.

use super::*;
use crate::global::tests::sealed_global_ledger::GlobalLedgerSink;
use crate::global::tests::sqlite_contract::database;
use crate::global::{GlobalLedger, GlobalLedgerConfig, Sha256SecretFingerprinter};
use actingcommand_artifact_store::{
    ArtifactEventSink, ArtifactStore, ArtifactStoreError, ArtifactStoreResult,
    ArtifactWriteContext, ArtifactWriteRequest, try_artifact_delete_guard,
};
use actingcommand_contract::{
    ArtifactIssuePolicy, ArtifactLinksDraft, ArtifactPayloadDraft, ArtifactPinRecord,
    ArtifactPinReleaseRecord, ArtifactProducer, ArtifactRedactionState, AuditInput,
    CapturePayloadDraft, EventAction, EventActor, EventDraft, EventLinksDraft, EventOrigin,
    EventQuery, FRAME_RETENTION_POLICY_VERSION, IdentifierIssuer, IssuedInstanceId, OriginModule,
    OwnerResourceDisposition, ResourceQuiescence, RetentionClass, RuntimePayloadDraft,
    SanitizedEventDraft,
};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn report(line: impl AsRef<str>) {
    // Direct handle writes are not captured by the test harness, so they reach the CI log.
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "ONE-OFF-RSAN {}", line.as_ref());
}

fn fingerprinter() -> Sha256SecretFingerprinter {
    Sha256SecretFingerprinter::new(b"one-off-r-san-release-after-pin").expect("fingerprinter")
}

fn material_store(root: &Path) -> ArtifactStore {
    let artifacts = ArtifactStore::open(root).expect("artifact owner");
    let mut admission = GlobalLedgerSink::new(None);
    admission.capacity_root = Some(artifacts.root().to_path_buf());
    artifacts
        .install_capacity_admission(std::sync::Arc::new(admission))
        .expect("fixture capacity owner");
    artifacts
}

struct Sink<'a, A> {
    append: &'a mut A,
    verified: Option<PersistedEvent>,
}

impl<A> ArtifactEventSink for Sink<'_, A>
where
    A: FnMut(SanitizedEventDraft) -> GlobalLedgerResult<PersistedEvent>,
{
    fn append(&mut self, draft: EventDraft) -> ArtifactStoreResult<()> {
        let draft = draft.sanitize(&fingerprinter()).map_err(|error| {
            ArtifactStoreError::fatal("one_off_sanitize_failed", "one_off_sink", error.to_string())
        })?;
        let event = (self.append)(draft).map_err(|error| {
            ArtifactStoreError::fatal(error.code(), "one_off_sink", error.to_string())
        })?;
        if event.event_type() == EventType::ArtifactVerified {
            self.verified = Some(event);
        }
        Ok(())
    }
}

struct Capture {
    ids: IdentifierIssuer,
    owner: OwnerEpoch,
    instance: IssuedInstanceId,
    identity: ArtifactRetentionIdentity,
    reference: ProjectedArtifactReference,
    material: PathBuf,
    quiescence: TerminalEvent,
    verified: TerminalEvent,
    pin: TerminalEvent,
    completed: TerminalEvent,
}

/// A confirmed, closed quiescence of the instance in its own request scope.
fn quiescence<A>(
    ids: &IdentifierIssuer,
    owner: OwnerEpoch,
    instance: IssuedInstanceId,
    append: &mut A,
) -> TerminalEvent
where
    A: FnMut(SanitizedEventDraft) -> GlobalLedgerResult<PersistedEvent>,
{
    let draft = EventDraft::new(
        ids.mint_event_id().expect("event"),
        retention_now().expect("clock"),
        EventSeverity::Info,
        EventOrigin::new(
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
        ),
        EventLinksDraft::default()
            .with_instance_id(instance)
            .with_request_id(ids.mint_request_id().expect("request"))
            .with_correlation_id(ids.mint_correlation_id().expect("correlation")),
        RuntimePayloadDraft::lifecycle_observed(
            owner,
            RuntimeLifecyclePhase::ResourceQuiescence {
                instance_id: *instance.transport(),
                resource_count: 1,
                quiescence: ResourceQuiescence::Confirmed,
                owner_disposition: OwnerResourceDisposition::ConfirmedClosed,
            },
            AuditInput::new(),
        )
        .into(),
    )
    .sanitize(&fingerprinter())
    .expect("quiescence draft");
    terminal(&append(draft).expect("quiescence append"))
}

/// Quiescence first, then a published frame, its Explicit pin and its capture success, all in
/// one request scope with no run and no lease (the shape of a CLI readonly observe).
fn bare_capture<A>(artifacts: &ArtifactStore, append: &mut A) -> Capture
where
    A: FnMut(SanitizedEventDraft) -> GlobalLedgerResult<PersistedEvent>,
{
    let ids = IdentifierIssuer::new().expect("issuer");
    let owner = *ids.mint_owner_epoch().expect("owner").transport();
    let instance = ids.mint_instance_id().expect("instance");
    let early = quiescence(&ids, owner, instance, append);
    let request = ids.mint_request_id().expect("request");
    let correlation = ids.mint_correlation_id().expect("correlation");
    let frame = ids.mint_frame_id().expect("frame");
    let links = EventLinksDraft::default()
        .with_instance_id(instance)
        .with_request_id(request)
        .with_correlation_id(correlation)
        .with_frame_id(frame);
    let mut sink = Sink {
        append: &mut *append,
        verified: None,
    };
    let stored = artifacts
        .put(
            ArtifactWriteRequest::new(
                ArtifactKind::CaptureFrame,
                b"one-off bare capture frame",
                ArtifactWriteContext::new(
                    ArtifactLinksDraft::default()
                        .with_frame_id(frame)
                        .with_correlation_id(correlation),
                    links.clone(),
                    retention_now().expect("clock"),
                ),
                ArtifactIssuePolicy::new(
                    ArtifactProducer::CaptureStore,
                    RetentionClass::DebugFull,
                    ArtifactRedactionState::Pending,
                ),
            ),
            &mut sink,
        )
        .expect("frame publication");
    let verified = sink.verified.take().expect("verified receipt");
    let artifact = verified
        .artifacts()
        .iter()
        .find(|artifact| artifact.kind() == ArtifactKind::CaptureFrame)
        .expect("capture frame reference")
        .project(true);
    let identity = ArtifactRetentionIdentity {
        artifact: artifact.clone(),
        owner_epoch: owner,
        instance_id: *instance.transport(),
        request_id: *request.transport(),
        correlation_id: *correlation.transport(),
        run_id: None,
        lease_id: None,
        policy_version: FRAME_RETENTION_POLICY_VERSION,
    };
    let pin = EventDraft::new(
        ids.mint_event_id().expect("event"),
        retention_now().expect("clock"),
        EventSeverity::Info,
        EventOrigin::new(
            EventSource::Runtime,
            OriginModule::ArtifactStore,
            EventActor::Runtime,
        ),
        EventLinksDraft::default(),
        ArtifactPayloadDraft::retention(
            ArtifactRetentionFact::PinRecorded(ArtifactPinRecord {
                identity: identity.clone(),
                reason: ArtifactPinReason::Explicit,
                trigger: terminal(&verified),
            }),
            AuditInput::new(),
        )
        .into(),
    )
    .sanitize(&fingerprinter())
    .expect("pin draft");
    let pin = verified
        .links()
        .artifact_retention_source()
        .apply_to(pin)
        .expect("pin source links");
    let pin = terminal(&append(pin).expect("pin append"));
    let completed = EventDraft::new(
        ids.mint_event_id().expect("event"),
        retention_now().expect("clock"),
        EventSeverity::Info,
        EventOrigin::new(
            EventSource::Device,
            OriginModule::Capture,
            EventActor::Runtime,
        ),
        links,
        CapturePayloadDraft::completed(
            EventAction::CaptureObserve,
            EffectDisposition::Performed,
            1280,
            720,
            AuditInput::new(),
        )
        .into(),
    )
    .sanitize(&fingerprinter())
    .expect("capture completed draft");
    let completed = terminal(&append(completed).expect("capture completed append"));
    Capture {
        ids,
        owner,
        instance,
        identity,
        reference: artifact,
        material: stored.path().to_path_buf(),
        quiescence: early,
        verified: terminal(&verified),
        pin,
        completed,
    }
}

fn describe(error: &GlobalLedgerError) -> String {
    format!(
        "Err code={} operation={} fatal={} detail={:?}",
        error.code(),
        error.operation(),
        error.is_fatal(),
        error.detail()
    )
}

fn released(ledger: &GlobalLedger) -> GlobalLedgerResult<Vec<TerminalEvent>> {
    Ok(ledger
        .query(EventQuery {
            event_type: Some(EventType::ArtifactPinReleased),
            ..EventQuery::default()
        })?
        .iter()
        .filter_map(|event| match event.payload().artifact_retention() {
            Some(ArtifactRetentionFact::PinReleased(record)) => Some(record.release),
            _ => None,
        })
        .collect())
}

fn open_ledger(root: &Path, owner: &str) -> GlobalLedger {
    GlobalLedger::open_sqlite_candidate(GlobalLedgerConfig::new(root, owner), database(root))
        .expect("SQLite ledger")
}

#[test]
fn one_off_r_san_a_close_before_pin_keeps_the_object() {
    let root = tempfile::tempdir().expect("root");
    let ledger = open_ledger(root.path(), "one-off-r-san-a");
    let artifacts = material_store(root.path());
    let capture = bare_capture(&artifacts, &mut |draft: SanitizedEventDraft| {
        ledger.append(draft)
    });
    report(format!(
        "A sequences: quiescence={} verified={} pin={} capture_completed={} (the only close precedes the pin)",
        capture.quiescence.sequence,
        capture.verified.sequence,
        capture.pin.sequence,
        capture.completed.sequence
    ));
    let policy = FailedRunRetentionPolicy::default();
    let candidates = ledger
        .retention_candidates(None, policy)
        .expect("candidates");
    let id = capture.reference.artifact_id;
    let listed = candidates
        .references
        .iter()
        .any(|reference| reference.artifact_id == id);
    let ineligible = candidates.ineligible.contains(&id);
    report(format!(
        "A retention_candidates: listed={listed} ineligible={ineligible} through_sequence={}",
        candidates.through_sequence
    ));
    let head = ledger.latest_sequence().expect("head");
    let guard = try_artifact_delete_guard(artifacts.root(), &capture.reference)
        .expect("delete guard")
        .expect("material guard is free");
    let admission = ledger.admit_artifact_eviction(guard, policy);
    report(format!(
        "A admit_artifact_eviction: {}",
        match &admission {
            Ok(ArtifactEvictionAdmission::Deferred) => "Ok(Deferred)".to_owned(),
            Ok(ArtifactEvictionAdmission::Committed(_)) => "Ok(Committed)".to_owned(),
            Err(error) => describe(error),
        }
    ));
    // The writer replies before it exits, so its health is polled for a short while.
    let started = Instant::now();
    let mut health = ledger.check_writer_health();
    while health.is_ok() && started.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(50));
        health = ledger.check_writer_health();
    }
    report(format!(
        "A check_writer_health after {} ms: {}",
        started.elapsed().as_millis(),
        match &health {
            Ok(()) => "Ok".to_owned(),
            Err(error) => describe(error),
        }
    ));
    let after = ledger.latest_sequence();
    report(format!(
        "A latest_sequence: before_admission={head} after={}",
        match &after {
            Ok(sequence) => sequence.to_string(),
            Err(error) => describe(error),
        }
    ));
    let material = capture.material.exists();
    report(format!("A material_present={material}"));
    let releases = released(&ledger);
    report(format!(
        "A PinReleased facts: {}",
        match &releases {
            Ok(values) => values.len().to_string(),
            Err(error) => describe(error),
        }
    ));
    assert!(listed && ineligible, "the object is listed but ineligible");
    assert!(
        matches!(admission, Ok(ArtifactEvictionAdmission::Deferred)),
        "admission defers the object"
    );
    assert!(health.is_ok(), "the writer stays healthy");
    assert_eq!(after.expect("head after admission"), head);
    assert!(material, "the material is kept");
    assert!(releases.expect("releases").is_empty());
    ledger.close().expect("close ledger");
}

#[test]
fn one_off_r_san_b_later_quiescence_releases_and_deletes() {
    let root = tempfile::tempdir().expect("root");
    let ledger = open_ledger(root.path(), "one-off-r-san-b");
    let artifacts = material_store(root.path());
    let mut append = |draft: SanitizedEventDraft| ledger.append(draft);
    let capture = bare_capture(&artifacts, &mut append);
    let later = quiescence(&capture.ids, capture.owner, capture.instance, &mut append);
    report(format!(
        "B sequences: quiescence={} verified={} pin={} capture_completed={} later_quiescence={}",
        capture.quiescence.sequence,
        capture.verified.sequence,
        capture.pin.sequence,
        capture.completed.sequence,
        later.sequence
    ));
    let policy = FailedRunRetentionPolicy::default();
    let candidates = ledger
        .retention_candidates(None, policy)
        .expect("candidates");
    let id = capture.reference.artifact_id;
    report(format!(
        "B retention_candidates: listed={} ineligible={}",
        candidates
            .references
            .iter()
            .any(|reference| reference.artifact_id == id),
        candidates.ineligible.contains(&id)
    ));
    let guard = try_artifact_delete_guard(artifacts.root(), &capture.reference)
        .expect("delete guard")
        .expect("material guard is free");
    let permit = match ledger.admit_artifact_eviction(guard, policy) {
        Ok(ArtifactEvictionAdmission::Committed(permit)) => {
            report("B admit_artifact_eviction: Ok(Committed)");
            permit
        }
        Ok(ArtifactEvictionAdmission::Deferred) => {
            report("B admit_artifact_eviction: Ok(Deferred)");
            panic!("the control object must be admitted");
        }
        Err(error) => {
            report(format!("B admit_artifact_eviction: {}", describe(&error)));
            panic!("the control object must be admitted");
        }
    };
    let outcome = ledger
        .finish_artifact_eviction(permit)
        .expect("finish eviction");
    let disposition = match outcome.payload().artifact_retention() {
        Some(ArtifactRetentionFact::EvictionOutcome(record)) => Some(record.disposition),
        _ => None,
    };
    let releases = released(&ledger).expect("releases");
    let material = capture.material.exists();
    report(format!(
        "B finish_artifact_eviction: outcome_sequence={} disposition={disposition:?}; PinReleased release sequences={:?}; material_present={material}",
        outcome.sequence(),
        releases
            .iter()
            .map(|release| release.sequence)
            .collect::<Vec<_>>()
    ));
    assert_eq!(disposition, Some(ArtifactEvictionDisposition::Deleted));
    assert_eq!(releases, [later]);
    assert!(!material, "the material is removed");
    ledger.check_writer_health().expect("healthy writer");
    ledger.close().expect("close ledger");
}

#[test]
fn one_off_r_san_c_release_before_pin_error_names_its_rule() {
    let root = tempfile::tempdir().expect("root");
    let mut store = crate::global::sqlite::SqliteLedgerStore::open(
        GlobalLedgerConfig::new(root.path(), "one-off-r-san-c"),
        database(root.path()),
        None::<fn(&ProjectedArtifactReference) -> Option<VerifiedArtifactReference>>,
    )
    .expect("SQLite ledger store");
    let artifacts = material_store(root.path());
    let capture = bare_capture(&artifacts, &mut |draft: SanitizedEventDraft| {
        store.append(draft)
    });
    let head = store.next_sequence;
    let error = store
        .append_retention_fact(
            ArtifactRetentionFact::PinReleased(ArtifactPinReleaseRecord {
                identity: capture.identity.clone(),
                pin: capture.pin,
                release: capture.quiescence,
            }),
            &capture.verified,
            false,
        )
        .expect_err("a release that does not follow its pin is refused");
    report(format!(
        "C append_retention_fact(PinReleased release={} <= pin={}): {}",
        capture.quiescence.sequence,
        capture.pin.sequence,
        describe(&error)
    ));
    let names_rule = error.code().contains("pin_release_order")
        || error.operation().contains("pin_release_order")
        || error
            .detail()
            .is_some_and(|detail| detail.contains("pin_release_order"));
    assert!(names_rule, "the error names pin_release_order");
    assert!(error.is_fatal());
    assert_eq!(store.next_sequence, head, "nothing was appended");
}
