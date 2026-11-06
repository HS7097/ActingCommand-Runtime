// SPDX-License-Identifier: AGPL-3.0-only

//! The bench's logic tests (Workflow #407 GPU-0a work order item 8; GPU-0b item 4): CI runs them
//! without a GPU, a model or a frame.

use actingcommand_vision_ep_bench::error::{BenchCode, BenchError};
use actingcommand_vision_ep_bench::onnx::{
    ConvGraph, ConvKernel, ConvOutputKind, ELEMENT_FLOAT, INPUT_NAME, IR_VERSION,
    InitializerDescription, NodeDescription, OPSET_VERSION, PRODUCER_NAME, ValueDescription,
    describe_model,
};
use actingcommand_vision_ep_bench::safety::{MemoryReader, SafetyLimits, SafetyMonitor};
use actingcommand_vision_ep_bench::stats::{Percentiles, percentiles};
use actingcommand_vision_ep_bench::template::{
    MARGIN_FACTOR, RecallCounts, count_at_margin, max_abs_delta,
};
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

fn value(name: &str, dims: &[i64]) -> ValueDescription {
    ValueDescription {
        name: name.to_owned(),
        element_type: ELEMENT_FLOAT,
        dims: dims.to_vec(),
    }
}

fn node(
    name: &str,
    op_type: &str,
    inputs: &[&str],
    output: &str,
    kernel_shape: Option<[i64; 2]>,
) -> NodeDescription {
    NodeDescription {
        name: name.to_owned(),
        op_type: op_type.to_owned(),
        inputs: inputs.iter().map(|input| (*input).to_owned()).collect(),
        outputs: vec![output.to_owned()],
        kernel_shape: kernel_shape.map(|shape| shape.to_vec()),
    }
}

fn kernel_tensor(name: &str, dims: &[i64], values: &[f32]) -> InitializerDescription {
    InitializerDescription {
        name: name.to_owned(),
        data_type: ELEMENT_FLOAT,
        dims: dims.to_vec(),
        values: values.to_vec(),
    }
}

#[test]
fn generated_conv_graph_round_trips_its_shape() {
    // A 12x9 frame searched at two scales: a 3x2 template without a mask, a 4x3 one with a mask
    // (every third pixel unselected: 0 in the mask and in the masked template).
    let products_0 = [1.0, 2.0, 3.0, 4.0, 5.0, 255.0];
    let mask_1: Vec<f32> = (0..12_u8)
        .map(|index| if index.is_multiple_of(3) { 0.0 } else { 1.0 })
        .collect();
    let products_1: Vec<f32> = (0..12_u8)
        .map(|index| {
            if index.is_multiple_of(3) {
                0.0
            } else {
                f32::from(index) * 10.0
            }
        })
        .collect();
    let graph = ConvGraph {
        frame_width: 12,
        frame_height: 9,
        scales: vec![
            ConvKernel {
                width: 3,
                height: 2,
                products: products_0.to_vec(),
                mask: None,
            },
            ConvKernel {
                width: 4,
                height: 3,
                products: products_1.clone(),
                mask: Some(mask_1.clone()),
            },
        ],
    };
    let bytes = graph.encode().expect("a valid graph encodes");
    let model = describe_model(&bytes).expect("the encoded graph reads back");

    assert_eq!(model.ir_version, IR_VERSION);
    assert_eq!(model.producer_name, PRODUCER_NAME);
    assert_eq!(model.opsets, vec![(String::new(), OPSET_VERSION)]);
    assert_eq!(model.inputs, vec![value(INPUT_NAME, &[1, 1, 9, 12])]);
    // Every output is 1x1x(H - h + 1)x(W - w + 1).
    assert_eq!(
        model.outputs,
        vec![
            value("products_0", &[1, 1, 8, 10]),
            value("products_1", &[1, 1, 7, 9]),
            value("image_sums_1", &[1, 1, 7, 9]),
            value("squared_sums_1", &[1, 1, 7, 9]),
        ]
    );
    assert_eq!(
        graph.outputs(),
        vec![
            (0, ConvOutputKind::Products, "products_0".to_owned()),
            (1, ConvOutputKind::Products, "products_1".to_owned()),
            (1, ConvOutputKind::ImageSums, "image_sums_1".to_owned()),
            (1, ConvOutputKind::SquaredSums, "squared_sums_1".to_owned()),
        ]
    );
    // The templates are the initializers, as written.
    assert_eq!(
        model.initializers,
        vec![
            kernel_tensor("kernel_products_0", &[1, 1, 2, 3], &products_0),
            kernel_tensor("kernel_products_1", &[1, 1, 3, 4], &products_1),
            kernel_tensor("kernel_mask_1", &[1, 1, 3, 4], &mask_1),
        ]
    );
    assert_eq!(
        model.nodes,
        vec![
            node("square", "Mul", &["image", "image"], "image_squared", None),
            node(
                "conv_products_0",
                "Conv",
                &["image", "kernel_products_0"],
                "products_0",
                Some([2, 3]),
            ),
            node(
                "conv_products_1",
                "Conv",
                &["image", "kernel_products_1"],
                "products_1",
                Some([3, 4]),
            ),
            node(
                "conv_image_sums_1",
                "Conv",
                &["image", "kernel_mask_1"],
                "image_sums_1",
                Some([3, 4]),
            ),
            node(
                "conv_squared_sums_1",
                "Conv",
                &["image_squared", "kernel_mask_1"],
                "squared_sums_1",
                Some([3, 4]),
            ),
        ]
    );

    // A template wider than the frame, or a kernel of the wrong length, is refused; cut bytes do
    // not read back.
    let mut too_wide = graph.clone();
    too_wide.scales[0].width = 13;
    too_wide.scales[0].products = vec![1.0; 26];
    assert_eq!(
        too_wide.encode().expect_err("wider than the frame").code,
        BenchCode::GraphInvalid
    );
    let mut short = graph.clone();
    short.scales[1].mask = Some(vec![1.0; 11]);
    assert_eq!(
        short.encode().expect_err("a short mask").code,
        BenchCode::GraphInvalid
    );
    assert_eq!(
        describe_model(&bytes[..bytes.len() - 1])
            .expect_err("cut bytes")
            .code,
        BenchCode::GraphInvalid
    );
}

#[test]
fn recall_counts_exact_matches_above_threshold_minus_margin() {
    let threshold = 0.9_f32;
    // Six positions' raw scores. Exact matches (normalized exact score >= 0.9): 0, 1, 4 and 5
    // (1.2 normalizes to 1). Position 2 is no match whose float score lies above the threshold;
    // position 1 is a match whose float score lies just below it.
    let exact = [0.950_f32, 0.900, 0.899, 0.700, 0.910, 1.200];
    let float = [0.9502_f32, 0.8996, 0.9001, 0.7000, 0.9093, 1.200];

    let largest = max_abs_delta(&exact, &float);
    assert!((largest - 0.0011).abs() < 1e-6, "largest |delta| {largest}");
    let margin = MARGIN_FACTOR * largest;
    assert!((margin - 0.0044).abs() < 4e-6, "margin {margin}");

    // At threshold - 4 x the largest |delta| every exact match is kept, and so is position 2.
    let at_margin = count_at_margin(&exact, &float, threshold, margin);
    assert_eq!(
        at_margin,
        RecallCounts {
            positions: 6,
            exact_matches: 4,
            recalled: 4,
            kept: 5,
        }
    );
    assert_eq!(at_margin.recall(), Some(1.0));

    // Without a margin the match at position 1 is lost.
    let at_threshold = count_at_margin(&exact, &float, threshold, 0.0);
    assert_eq!(
        at_threshold,
        RecallCounts {
            positions: 6,
            exact_matches: 4,
            recalled: 3,
            kept: 4,
        }
    );
    assert_eq!(at_threshold.recall(), Some(0.75));

    // Counts add over frames; without an exact match there is no recall.
    let mut total = RecallCounts::default();
    total.add(at_margin);
    total.add(at_threshold);
    assert_eq!(
        total,
        RecallCounts {
            positions: 12,
            exact_matches: 8,
            recalled: 7,
            kept: 9,
        }
    );
    let none = count_at_margin(&[0.5, 0.6], &[0.5, 0.95], threshold, margin);
    assert_eq!(
        none,
        RecallCounts {
            positions: 2,
            exact_matches: 0,
            recalled: 0,
            kept: 1,
        }
    );
    assert_eq!(none.recall(), None);
}
