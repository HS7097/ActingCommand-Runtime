// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #375 R5e (ledger gate 41): `actingledger frames` over a fixture with every class, an
// overlap, a deleted leaf, an epoch-end folder and two cleared records (model-375 §2.8, §8 row
// R5e). The frame view is the R5c view, built here; the files are real files below a state
// root. Every expected value is the model's: what each class counts, where a path points, and
// which folders and points are listed.

use super::*;
use actingcommand_contract::{
    ArtifactProducer, ArtifactRedactionState, IdentifierIssuer, OriginModule, RetentionClass,
    RuntimeFactScope,
};
use actingcommand_ledger::KeptFrameFolder;
use serde_json::json;
use std::fs;

// The tool's clock in the fixture, and one day.
const NOW: u64 = 1_792_000_000_000;
const DAY: u64 = 86_400_000;

fn text<T: Serialize>(id: &T) -> String {
    match serde_json::to_value(id) {
        Ok(serde_json::Value::String(text)) => text,
        other => panic!("identifier text: {other:?}"),
    }
}

fn count(frames: u64, bytes: u64) -> serde_json::Value {
    json!({ "frames": frames, "bytes": bytes })
}

fn class_totals(
    frames: u64,
    bytes: u64,
    due_now: serde_json::Value,
    to_move: serde_json::Value,
    in_folder: serde_json::Value,
    absent: serde_json::Value,
) -> serde_json::Value {
    json!({
        "frames": frames,
        "bytes": bytes,
        "due_now": due_now,
        "to_move": to_move,
        "in_folder": in_folder,
        "absent": absent,
    })
}

fn record(key: &str, value: FactValue) -> RuntimeFactRecord {
    RuntimeFactRecord {
        scope: RuntimeFactScope::Runtime,
        key: key.to_owned(),
        value,
        observed_at_unix_ms: NOW - 6 * DAY,
        source: OriginModule::RuntimeFacts,
        ttl_ms: None,
    }
}

fn row(key: &str, value: FactScalar) -> BTreeMap<String, FactScalar> {
    BTreeMap::from([
        ("key".to_owned(), FactScalar::String(key.to_owned())),
        ("value".to_owned(), value),
        (
            "source".to_owned(),
            FactScalar::String("default".to_owned()),
        ),
        ("settable".to_owned(), FactScalar::Boolean(false)),
    ])
}

fn cleared(folder: &str, at: u64) -> RuntimeFactRecord {
    record(
        KEPT_FOLDER_CLEARED_FACT_KEY,
        FactValue::RecordList(vec![BTreeMap::from([
            ("folder".to_owned(), FactScalar::String(folder.to_owned())),
            ("cleared_at_unix_ms".to_owned(), FactScalar::TimestampMs(at)),
        ])]),
    )
}

struct Fixture {
    ids: IdentifierIssuer,
    instance: InstanceId,
    root: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let ids = IdentifierIssuer::new().expect("issuer");
        let instance = *ids.mint_instance_id().expect("instance").transport();
        Self {
            ids,
            instance,
            root: tempfile::tempdir().expect("state root"),
        }
    }

    fn run(&self) -> RunId {
        *self.ids.mint_run_id().expect("run").transport()
    }

    /// One frame of the view; a Lab or error frame has its folder, `<HHmmss-fff>_` + its object
    /// file.
    fn frame(
        &self,
        class: FrameRetentionClass,
        run: Option<RunId>,
        byte_count: u64,
        due: Option<u64>,
        folder: Option<(&str, &str)>,
        windows: Vec<usize>,
    ) -> FrameRetentionFrame {
        let artifact = *self.ids.mint_artifact_id().expect("artifact").transport();
        let object_file = format!("{}.png", text(&artifact));
        FrameRetentionFrame {
            reference: ProjectedArtifactReference {
                artifact_id: artifact,
                kind: ArtifactKind::CaptureFrame,
                run_id: run,
                frame_id: None,
                correlation_id: None,
                object_key: Some(format!("artifacts/ab/{object_file}")),
                media_type: ArtifactKind::CaptureFrame.media_type(),
                byte_count,
                sha256: String::new(),
                created_at_unix_ms: NOW - 3 * DAY,
                producer: ArtifactProducer::CaptureStore,
                retention_class: RetentionClass::Adaptive,
                redaction_state: ArtifactRedactionState::NotRequired,
            },
            instance_id: self.instance,
            run_id: run,
            class,
            entry_unix_ms: (class != FrameRetentionClass::Running).then_some(NOW - 2 * DAY),
            due_unix_ms: due,
            folder: folder.map(|(date, leaf)| KeptFrameFolder {
                date: date.to_owned(),
                leaf: leaf.to_owned(),
                file_name: format!("120000-000_{object_file}"),
            }),
            windows,
        }
    }

    fn point(
        &self,
        sequence: u64,
        event_type: EventType,
        run: Option<RunId>,
        at_unix_ms: u64,
        date: &str,
        code: &str,
    ) -> FrameErrorPoint {
        FrameErrorPoint {
            sequence,
            event_type,
            epoch_end: event_type == EventType::RuntimeTakeover,
            instance_id: self.instance,
            run_id: run,
            at_unix_ms,
            date: date.to_owned(),
            leaf: format!("node-{sequence}-{code}"),
            code: code.to_owned(),
        }
    }

    /// A file of `bytes` bytes at `relative` below the state root.
    fn file(&self, relative: &str, bytes: usize) {
        let path = self.root.path().join(relative);
        fs::create_dir_all(path.parent().expect("parent")).expect("folder");
        fs::write(path, vec![0_u8; bytes]).expect("file");
    }

    fn object(frame: &FrameRetentionFrame) -> String {
        frame.reference.object_key.clone().expect("object key")
    }

    fn kept(frame: &FrameRetentionFrame) -> String {
        let folder = frame.folder.as_ref().expect("folder");
        format!("kept/{}/{}/{}", folder.date, folder.leaf, folder.file_name)
    }
}

#[test]
fn frames_listing_reports_every_class_folder_and_both_cleared_records() {
    let fixture = Fixture::new();
    let first_run = fixture.run();
    let cut_run = fixture.run();
    // Points 0 and 1 overlap: their windows both hold the overlap frame. Point 2 is the
    // epoch-end point of a run cut short.
    let points = vec![
        fixture.point(
            40,
            EventType::RuntimeFailed,
            None,
            NOW - 2 * DAY,
            "2026-10-10",
            "capture_failed",
        ),
        fixture.point(
            44,
            EventType::RuntimeFailed,
            Some(first_run),
            NOW - 2 * DAY + 10_000,
            "2026-10-10",
            "step_budget_exhausted",
        ),
        fixture.point(
            60,
            EventType::RuntimeTakeover,
            Some(cut_run),
            NOW - DAY,
            "2026-10-11",
            "runtime_takeover",
        ),
    ];
    let lab = fixture.frame(
        FrameRetentionClass::Lab,
        None,
        100,
        None,
        Some(("2026-10-09", "lab-node")),
        Vec::new(),
    );
    let overlap = fixture.frame(
        FrameRetentionClass::Error,
        None,
        200,
        None,
        Some(("2026-10-10", "node-40-capture_failed")),
        vec![0, 1],
    );
    let deleted_leaf = fixture.frame(
        FrameRetentionClass::Error,
        Some(first_run),
        300,
        None,
        Some(("2026-10-10", "node-44-step_budget_exhausted")),
        vec![1],
    );
    let epoch_end = fixture.frame(
        FrameRetentionClass::Error,
        Some(cut_run),
        400,
        None,
        Some(("2026-10-11", "node-60-runtime_takeover")),
        vec![2],
    );
    let reading = fixture.frame(
        FrameRetentionClass::Resource,
        None,
        500,
        Some(NOW - 1),
        None,
        Vec::new(),
    );
    let duplicate = fixture.frame(
        FrameRetentionClass::Duplicate,
        None,
        600,
        Some(NOW - DAY),
        None,
        Vec::new(),
    );
    let ordinary = fixture.frame(
        FrameRetentionClass::Default,
        None,
        700,
        Some(NOW + 1_000),
        None,
        Vec::new(),
    );
    let running = fixture.frame(
        FrameRetentionClass::Running,
        None,
        800,
        None,
        None,
        Vec::new(),
    );

    // On disk: the Lab frame not moved yet; the overlap frame both at its object key and in its
    // folder; the deleted leaf's frame nowhere; the epoch-end frame moved; the duplicate
    // already removed; and the cleaner's move counter, which is no folder.
    for frame in [&lab, &overlap, &reading, &ordinary, &running] {
        fixture.file(&Fixture::object(frame), 3);
    }
    fixture.file(&Fixture::kept(&overlap), 7);
    fixture.file(&Fixture::kept(&epoch_end), 9);
    fixture.file("kept/.moves", 16);

    let view = FrameRetentionView {
        through_sequence: 80,
        evaluated_at_unix_ms: NOW,
        frames: vec![
            lab.clone(),
            overlap.clone(),
            deleted_leaf.clone(),
            epoch_end.clone(),
            reading.clone(),
            duplicate.clone(),
            ordinary.clone(),
            running.clone(),
        ],
        error_points: points,
    };

    let mut facts = FrameFactsBuilder::default();
    facts
        .fact(
            5,
            &record(
                &config_parameters_fact_key(CONFIG_TOP_LEVEL_SECTION),
                FactValue::RecordList(vec![
                    row(RETENTION_ENABLED_KEY, FactScalar::Boolean(true)),
                    row(DEDUP_ERROR_KEY, FactScalar::Boolean(true)),
                    row(DEDUP_LAB_KEY, FactScalar::Boolean(false)),
                ]),
            ),
        )
        .expect("switches");
    facts
        .fact(
            70,
            &cleared("2026-10-08/node-30-capture_failed", NOW - 5 * DAY),
        )
        .expect("first clear");
    facts
        .fact(75, &cleared("2026-10-08", NOW - 4 * DAY))
        .expect("second clear");
    facts.package(first_run, "package-a", "task-a");
    facts.package(cut_run, "package-b", "task-b");
    let diagnostic = *fixture
        .ids
        .mint_artifact_id()
        .expect("artifact")
        .transport();
    let archive = *fixture
        .ids
        .mint_artifact_id()
        .expect("artifact")
        .transport();
    facts.artifact(diagnostic, ArtifactKind::DiagnosticJson, 30);
    facts.artifact(archive, ArtifactKind::DiagnosticJson, 20);
    facts.artifact(
        running.reference.artifact_id,
        ArtifactKind::CaptureFrame,
        800,
    );
    let facts = facts.finish().expect("facts");

    let selection = FrameSelection {
        class: FrameClassFilter::All,
        before_unix_ms: None,
        instance: None,
        run: None,
        error_sequence: None,
        paths: true,
    };
    let report = frames_report(fixture.root.path(), &view, facts, &selection).expect("listing");
    let value = serde_json::to_value(&report).expect("listing JSON");

    // A frame found both in `artifacts\` and in `kept\` counts in_folder, never absent.
    assert_eq!(value["totals"]["error"]["in_folder"], count(2, 600));
    // `running` carries counts only: the running frame has no path.
    let printed = value["paths"]
        .as_array()
        .expect("paths")
        .iter()
        .map(|path| path["path"].as_str().expect("path").to_owned())
        .collect::<Vec<_>>();
    assert!(!printed.contains(&Fixture::object(&running)));

    let zero = count(0, 0);
    let point_40 = json!({
        "sequence": 40,
        "event_type": "runtime.failed",
        "code": "capture_failed",
        "at_unix_ms": NOW - 2 * DAY,
    });
    let point_44 = json!({
        "sequence": 44,
        "event_type": "runtime.failed",
        "code": "step_budget_exhausted",
        "at_unix_ms": NOW - 2 * DAY + 10_000,
        "run_id": text(&first_run),
        "package_label": "package-a",
        "task_label": "task-a",
    });
    let point_60 = json!({
        "sequence": 60,
        "event_type": "runtime.takeover",
        "code": "runtime_takeover",
        "at_unix_ms": NOW - DAY,
        "run_id": text(&cut_run),
        "package_label": "package-b",
        "task_label": "task-b",
    });
    let expected = json!({
        "through_sequence": 80,
        "evaluated_at_unix_ms": NOW,
        "switches": { "dedup_error": true, "dedup_lab": false, "retention_enabled": true },
        "totals": {
            "lab": class_totals(1, 100, zero.clone(), count(1, 100), zero.clone(), zero.clone()),
            "error": class_totals(3, 900, zero.clone(), zero.clone(), count(2, 600), count(1, 300)),
            "resource": class_totals(1, 500, count(1, 500), zero.clone(), zero.clone(), zero.clone()),
            "duplicate": class_totals(1, 600, zero.clone(), zero.clone(), zero.clone(), count(1, 600)),
            "default": class_totals(1, 700, zero.clone(), zero.clone(), zero.clone(), zero.clone()),
            "running": count(1, 800),
        },
        "folders": [
            {
                "path": "kept/2026-10-09/lab-node",
                "date": "2026-10-09",
                "leaf": "lab-node",
                "present": zero.clone(),
                "placed": 1,
                "error_points": [],
            },
            {
                "path": "kept/2026-10-10/node-40-capture_failed",
                "date": "2026-10-10",
                "leaf": "node-40-capture_failed",
                "present": count(1, 7),
                "placed": 1,
                "error_points": [point_40, point_44.clone()],
            },
            {
                "path": "kept/2026-10-10/node-44-step_budget_exhausted",
                "date": "2026-10-10",
                "leaf": "node-44-step_budget_exhausted",
                "present": zero.clone(),
                "placed": 1,
                "error_points": [point_44],
            },
            {
                "path": "kept/2026-10-11/node-60-runtime_takeover",
                "date": "2026-10-11",
                "leaf": "node-60-runtime_takeover",
                "present": count(1, 9),
                "placed": 1,
                "error_points": [point_60],
            },
        ],
        "cleared": [
            {
                "sequence": 70,
                "folder": "2026-10-08/node-30-capture_failed",
                "cleared_at_unix_ms": NOW - 5 * DAY,
            },
            {
                "sequence": 75,
                "folder": "2026-10-08",
                "cleared_at_unix_ms": NOW - 4 * DAY,
            },
        ],
        "non_frame": { "artifacts": 2, "bytes": 50 },
        "paths": [
            { "path": Fixture::object(&lab), "class": "lab" },
            { "path": Fixture::kept(&overlap), "class": "error" },
            { "path": Fixture::object(&deleted_leaf), "class": "error" },
            { "path": Fixture::kept(&epoch_end), "class": "error" },
            { "path": Fixture::object(&reading), "class": "resource" },
            { "path": Fixture::object(&duplicate), "class": "duplicate" },
            { "path": Fixture::object(&ordinary), "class": "default" },
        ],
    });
    assert_eq!(value, expected);
}
