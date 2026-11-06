// SPDX-License-Identifier: AGPL-3.0-only

//! The bench's logic tests (Workflow #407 GPU-0a work order item 8): CI runs them without a GPU,
//! a model or a frame.

use actingcommand_vision_ep_bench::error::{BenchCode, BenchError};
use actingcommand_vision_ep_bench::safety::{MemoryReader, SafetyLimits, SafetyMonitor};
use actingcommand_vision_ep_bench::stats::{Percentiles, percentiles};
use actingcommand_vision_ep_bench::workload::{
    RequestKind, WORKLOAD_SCHEMA, WorkloadBlocks, WorkloadRect, parse_workload,
};
use std::collections::VecDeque;

#[test]
fn percentiles_of_fixed_lists() {
    // Nearest rank: the p-th percentile is the sample at rank ceil(p * n / 100).
    let one_to_twenty: Vec<u64> = (1..=20).rev().collect();
    assert_eq!(
        percentiles(&one_to_twenty),
        Some(Percentiles {
            count: 20,
            min: 1,
            p50: 10,
            p95: 19,
            max: 20,
            mean: 10,
        })
    );
    assert_eq!(
        percentiles(&[5, 1, 3]),
        Some(Percentiles {
            count: 3,
            min: 1,
            p50: 3,
            p95: 5,
            max: 5,
            mean: 3,
        })
    );
    assert_eq!(
        percentiles(&[7]),
        Some(Percentiles {
            count: 1,
            min: 7,
            p50: 7,
            p95: 7,
            max: 7,
            mean: 7,
        })
    );
    // 23 samples, one outlier: p95 is rank 22, the max keeps the outlier.
    let mut warm: Vec<u64> = (0..22).map(|index| 1_000 + index).collect();
    warm.push(90_000);
    let summary = percentiles(&warm).expect("23 samples");
    assert_eq!(
        (summary.p50, summary.p95, summary.max),
        (1_011, 1_021, 90_000)
    );
    assert_eq!(percentiles(&[]), None);
}

fn workload_text(extra_top: &str, extra_frame: &str, extra_request: &str) -> String {
    format!(
        r#"{{
  "schema": "{WORKLOAD_SCHEMA}",
  "model": {{"model_ref": "model-a", "model_sha256": "{sha}"}},
  "provenance": {{"generator": "test"}},{extra_top}
  "frames": [{{
    "id": "frame-01",
    "path": "C:\\bench\\frame-01.png",
    "sha256": "{sha}",
    "width": 1280,
    "height": 720,{extra_frame}
    "requests": [
      {{"id": "full_frame", "region": {{"x": 0, "y": 0, "width": 1280, "height": 720}},
        "blocks": "detect", "languages": ["en"], "timeout_ms": 60000}},
      {{"id": "ocr/field-a", "region": {{"x": 10, "y": 20, "width": 120, "height": 30}},
        "blocks": "implicit", "languages": ["en"], "timeout_ms": 60000,{extra_request}
        "expected_text": "120/240", "declared_timeout_ms": 1000}}
    ]
  }}]
}}"#,
        sha = "a".repeat(64)
    )
}

#[test]
fn workload_parses_and_refuses_unknown_members() {
    let workload = parse_workload(&workload_text("", "", "")).expect("a valid workload");
    assert_eq!(workload.model.model_ref, "model-a");
    assert_eq!(workload.frames.len(), 1);
    let frame = &workload.frames[0];
    assert!(!frame.negative);
    assert_eq!(frame.requests.len(), 2);
    assert_eq!(frame.requests[0].blocks, WorkloadBlocks::Detect);
    assert_eq!(frame.requests[0].kind(frame), RequestKind::FullFrame);
    let field = &frame.requests[1];
    assert_eq!(field.kind(frame), RequestKind::Roi);
    assert_eq!(
        field.region,
        WorkloadRect {
            x: 10,
            y: 20,
            width: 120,
            height: 30,
        }
    );
    assert_eq!(field.expected_text.as_deref(), Some("120/240"));
    assert_eq!(field.declared_timeout_ms, Some(1000));
    assert_eq!(workload.request_counts(), (1, 1));

    for (place, text) in [
        ("top level", workload_text("\n  \"weights\": {},", "", "")),
        ("frame", workload_text("", "\n    \"scale\": 2,", "")),
        (
            "request",
            workload_text("", "", "\n        \"roi_pad\": 4,"),
        ),
    ] {
        let error = parse_workload(&text).expect_err(place);
        assert_eq!(error.code, BenchCode::WorkloadInvalid, "{place}");
        assert!(
            error.message.contains("unknown field"),
            "{place}: {}",
            error.message
        );
    }

    let outside = workload_text("", "", "").replace(
        r#""x": 10, "y": 20, "width": 120"#,
        r#""x": 1200, "y": 20, "width": 120"#,
    );
    let error = parse_workload(&outside).expect_err("a region outside the frame");
    assert_eq!(error.code, BenchCode::WorkloadInvalid);
    let schema = workload_text("", "", "").replace(WORKLOAD_SCHEMA, "another/1");
    assert_eq!(
        parse_workload(&schema).expect_err("another schema").code,
        BenchCode::WorkloadInvalid
    );
}

/// Returns scripted readings, one pair per check; reading past the script is an error.
struct FakeReader {
    readings: VecDeque<(u64, u64)>,
    current: Option<(u64, u64)>,
}

impl FakeReader {
    fn new(readings: &[(u64, u64)]) -> Self {
        Self {
            readings: readings.iter().copied().collect(),
            current: None,
        }
    }
}

impl MemoryReader for FakeReader {
    fn host_available_bytes(&mut self) -> Result<u64, BenchError> {
        self.current = self.readings.pop_front();
        self.current
            .map(|(host, _)| host)
            .ok_or_else(|| BenchError::new(BenchCode::SamplerFailed, "the script is exhausted"))
    }

    fn own_private_bytes(&mut self) -> Result<u64, BenchError> {
        self.current
            .map(|(_, private)| private)
            .ok_or_else(|| BenchError::new(BenchCode::SamplerFailed, "the script is exhausted"))
    }
}

#[test]
fn safety_stops_below_host_floor_with_fake_readers() {
    const GIB: u64 = 1 << 30;
    let limits = SafetyLimits::default();
    assert_eq!(limits.host_available_floor_at_start_bytes, 4 * GIB);
    assert_eq!(limits.host_available_floor_bytes, 5 * GIB / 2);
    assert_eq!(limits.own_private_bytes_cap, 3 * GIB);

    // Below 4 GiB the bench does not start.
    let mut refused = SafetyMonitor::new(limits, FakeReader::new(&[(4 * GIB - 1, GIB)]));
    assert_eq!(
        refused.preflight().expect_err("start refused").code,
        BenchCode::HostMemoryLowAtStart
    );

    // It starts at 4 GiB, keeps running at 3 GiB (above the running floor), stops below 2.5 GiB.
    let mut monitor = SafetyMonitor::new(
        limits,
        FakeReader::new(&[
            (4 * GIB, GIB),
            (3 * GIB, GIB),
            (5 * GIB / 2, GIB),
            (5 * GIB / 2 - 1, GIB),
        ]),
    );
    let start = monitor.preflight().expect("starts at the floor");
    assert_eq!(start.host_available_bytes, 4 * GIB);
    assert!(monitor.check().is_ok(), "3 GiB is above the running floor");
    assert!(monitor.check().is_ok(), "exactly 2.5 GiB still runs");
    let stopped = monitor.check().expect_err("below the running floor");
    assert_eq!(stopped.code, BenchCode::HostMemoryLow);
    assert_eq!(stopped.code.exit_status(), 3);

    // Its own private bytes above 3 GiB stop it, with the host still above every floor.
    let mut heavy = SafetyMonitor::new(
        limits,
        FakeReader::new(&[(8 * GIB, 3 * GIB), (8 * GIB, 3 * GIB + 1)]),
    );
    assert!(heavy.check().is_ok(), "exactly 3 GiB is within the cap");
    assert_eq!(
        heavy.check().expect_err("above the cap").code,
        BenchCode::PrivateBytesExceeded
    );

    // A reader that fails is a named sampler failure, never a pass.
    let mut blind = SafetyMonitor::new(limits, FakeReader::new(&[]));
    assert_eq!(
        blind.check().expect_err("no reading").code,
        BenchCode::SamplerFailed
    );
}
