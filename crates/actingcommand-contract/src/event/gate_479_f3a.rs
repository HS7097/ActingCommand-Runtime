// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #479 F-3a fix round 1 (register §1.486; work order rd-1119/brief-479-f3a-fix1.md, item
// 4; contracts/component-interfaces.md, the `runtime-client` row of revision 13): the run's
// geometry record (`task.geometry_observed`) holds no capture backend `acib_screen_capture`, an iOS
// handset's, which `ledger` 9 does not hold. The writer refuses a record that names it as its
// frame's backend, as its frame's producer's or as its observation's, and so does the reader of a
// stored record; the same records of an Android handset's frame write and read back as before. No
// writer of this build names the value (a run's capture of an iOS handset is refused before any
// frame), so each record here is built with the contract's own types, or is a written record with
// the value put in.

use super::sh1::{AT_MS, holder, ids, read_back, refused, written};
use super::*;
use crate::{
    CaptureBackendName, CaptureExtent, CaptureGeometry, CaptureGeometryObservation,
    CaptureGeometrySource, CaptureGeometryUnknownReason, CaptureRotation,
    CaptureRotationObservation, CaptureRotationSource, CaptureWmSizeKind,
};
use serde_json::json;
use std::time::{Duration, SystemTime};

const ACIB: CaptureBackendName = CaptureBackendName::AcibScreenCapture;
const ADB: CaptureBackendName = CaptureBackendName::AdbScreencap;

fn at() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_millis(AT_MS)
}

fn extent() -> CaptureExtent {
    CaptureExtent::new(750, 1334).expect("an extent")
}

/// A display observed through adb, its geometry naming `backend`.
fn observed(backend: CaptureBackendName) -> CaptureGeometryObservation {
    CaptureGeometryObservation::Observed(CaptureGeometry {
        backend,
        source: CaptureGeometrySource::AdbDefaultDisplay {
            serial: "handset:neutral.handset".to_owned(),
            wm_extent: extent(),
            wm_size_kind: CaptureWmSizeKind::Physical,
        },
        logical_display_extent: extent(),
        rotation: CaptureRotationObservation::Observed {
            rotation: CaptureRotation::R0,
            source: CaptureRotationSource::DumpsysDisplayOrientation,
        },
        sampled_at: at(),
        frame_transform: None,
        safe_insets: None,
    })
}

/// What a frame whose producer observed no geometry carries, as an iOS handset's frame does.
fn absent() -> CaptureGeometryObservation {
    CaptureGeometryObservation::Unknown(CaptureGeometryUnknownReason::ProducerObservationAbsent)
}

/// The initial geometry record of a frame of `frame_backend` whose producer observed `producer`,
/// and of the observation `observation`, its conclusion the contract's own assessment.
fn record(
    frame_backend: CaptureBackendName,
    producer: CaptureGeometryObservation,
    observation: CaptureGeometryObservation,
) -> EventPayloadDraft {
    let frame = TaskGeometryFrame {
        frame_id: *ids().mint_frame_id().expect("frame id").transport(),
        extent: extent(),
        backend: frame_backend,
        captured_at: at(),
        producer,
    };
    let conclusion = TaskGeometryObservation::assess(&frame, &observation);
    TaskPayloadDraft::semantic(
        TaskSemanticFact::GeometryObserved {
            observation: Box::new(TaskGeometryObservation {
                phase: TaskGeometryPhase::Initial,
                frame: Some(frame),
                observation: Some(observation),
                conclusion,
                trigger: None,
                original_failure: None,
                unavailable: None,
                display_map: None,
            }),
        },
        AuditInput::new(),
    )
    .into()
}

/// The writer refuses a geometry record that names `acib_screen_capture` anywhere, and writes the
/// same records of an Android handset's frame.
#[test]
fn f3a_a_geometry_record_naming_acib_is_refused_by_the_writer() {
    read_back(&written(record(ADB, observed(ADB), observed(ADB))));
    read_back(&written(record(ADB, absent(), absent())));
    for (place, draft) in [
        ("the frame's backend", record(ACIB, absent(), absent())),
        (
            "the producer's backend",
            record(ADB, observed(ACIB), observed(ADB)),
        ),
        (
            "the observation's backend",
            record(ADB, observed(ADB), observed(ACIB)),
        ),
    ] {
        let error = draft
            .sanitize(&SpyFingerprinter::new())
            .expect_err("the writer refuses the value");
        assert_eq!(
            error.code(),
            SanitizationCode::InvalidTaskSemanticPayload.code(),
            "{place}"
        );
    }
}

/// The reader refuses a stored geometry record whose frame names `acib_screen_capture`, the record
/// otherwise the one of an Android handset's frame it reads back: the value decodes and is refused
/// by the record's check, as the writer refuses it.
#[test]
fn f3a_a_stored_geometry_record_naming_acib_is_refused_by_the_reader() {
    let stored = written(record(ADB, absent(), absent()));
    read_back(&stored);
    let mut named = stored;
    holder(&mut named, "captured_at").expect("the record's frame")["backend"] =
        json!("acib_screen_capture");
    let error = refused(named);
    assert_eq!(
        error.code(),
        SanitizationCode::InvalidTaskSemanticPayload.code()
    );
}
