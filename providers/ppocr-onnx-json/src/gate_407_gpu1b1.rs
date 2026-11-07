// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #407 GPU-1 B1 (design rd-1031/perf-407/design-gpu1.md §2.1, §2.2 and §2.4; coordinator
// work order rd-1031/perf-407/brief-gpu1b1.md): the options of a DirectML session, the node
// placement every DirectML session reads into a v2 report, the invocation's fallback_observed,
// and a placement that cannot be read. No GPU and no ONNX Runtime: the sessions come from a fake
// that writes ONNX Runtime's verbose placement log into the logger it is given, and the adapter
// is a fake with a fake identity and LUID.

use super::*;
use actingcommand_vision_ffi::{
    CudaDeviceSelector, DxgiLuid, PciAddress, ppocr_model_content_sha256,
};

const CARD_IDENTITY: &str = "dxgi-pci:1234:5678:9abcdef0:01";
const CARD_LUID: DxgiLuid = DxgiLuid {
    high_part: 0,
    low_part: 0x0001_2345,
};
const CARD_LUID_TEXT: &str = "0x0000000000012345";
const CUDA_IDENTITY: &str = "cuda-uuid:11111111111111111111111111111111";
const MODEL: &[u8] = b"fake model bytes";

/// A fake ONNX Runtime: every build records its options and whether it was the placement
/// shadow; a shadow build writes `log` into its logger line by line, as ONNX Runtime's verbose
/// session log does.
struct FakeSessions {
    log: Vec<String>,
    builds: Vec<(SessionOptions, bool)>,
}

impl FakeSessions {
    fn logging(log: Vec<String>) -> Self {
        Self {
            log,
            builds: Vec::new(),
        }
    }
}

impl SessionBuilds for FakeSessions {
    type Session = ();

    fn build(
        &mut self,
        _model: &[u8],
        _path: &Path,
        options: &SessionOptions,
        placement_logger: Option<LoggerFunction>,
    ) -> Result<(), SessionBuildFailure> {
        self.builds.push((*options, placement_logger.is_some()));
        if let Some(logger) = placement_logger {
            for line in &self.log {
                logger(
                    LogLevel::Verbose,
                    "onnxruntime",
                    "session",
                    "inference_session.cc",
                    line,
                );
            }
        }
        Ok(())
    }
}

fn key(backend: OnnxExecutionProvider, dxgi_index: u32) -> OcrSessionKey {
    let model_sha256 =
        ppocr_model_content_sha256(&"a".repeat(64), &"b".repeat(64), &"c".repeat(64), None)
            .expect("model hash");
    let cuda = backend == OnnxExecutionProvider::Cuda;
    let directml = backend == OnnxExecutionProvider::Directml;
    OcrSessionKey::from_parts(OcrSessionKeyParts {
        engine_binding_sha256: "e".repeat(64),
        runtime_library_path: "onnxruntime.dll".to_string(),
        runtime_library_sha256: "d".repeat(64),
        onnxruntime_version: "1.24.4".to_string(),
        model_ref: "PP-OCRv6_medium".to_string(),
        model_sha256,
        requested_backend: backend,
        requested_cuda_device: cuda.then(|| CudaDeviceSelector {
            ordinal: 1,
            expected_stable_identity: CUDA_IDENTITY.to_string(),
        }),
        resolved_cuda_device: cuda.then(|| CudaDeviceIdentity {
            ordinal: 1,
            stable_identity: CUDA_IDENTITY.to_string(),
            pci_bus_id: Some("0000:02:00.0".to_string()),
        }),
        requested_directml_device: directml.then(|| DirectmlDeviceSelector {
            expected_identity: CARD_IDENTITY.to_string(),
        }),
        resolved_directml_device: directml.then(|| DirectmlDeviceIdentity {
            dxgi_index,
            identity: CARD_IDENTITY.to_string(),
        }),
    })
    .expect("session key")
}

fn plan(backend: OnnxExecutionProvider, dxgi_index: u32) -> ProviderSessionPlan {
    ProviderSessionPlan::from_key(&key(backend, dxgi_index)).expect("session plan")
}

/// The adapter one build resolved: the fake card at `dxgi_index`.
fn fake_adapter(dxgi_index: u32) -> ResolvedDirectmlAdapter {
    ResolvedDirectmlAdapter {
        dxgi_index,
        identity: CARD_IDENTITY.to_string(),
        luid: CARD_LUID,
        pci_address: Some(PciAddress {
            bus: 1,
            device: 0,
            function: 0,
        }),
    }
}

fn request(role: PpocrModelRole, adapter: Option<&ResolvedDirectmlAdapter>) -> SessionRequest<'_> {
    SessionRequest {
        model: MODEL,
        path: Path::new("model.onnx"),
        role,
        adapter,
    }
}

fn lines(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|line| (*line).to_string()).collect()
}

fn directml_summary(count: usize) -> String {
    format!(" Node(s) placed on [DmlExecutionProvider]. Number of nodes: {count}")
}

/// The first `count` per-node lines of a DirectML group.
fn directml_nodes(count: usize) -> impl Iterator<Item = String> {
    (0..count).map(|node| format!("  Conv (conv_{node})"))
}

/// ONNX Runtime's verbose log of a session whose 203 nodes all run on DirectML (design §2.2's
/// detector), with lines around the section that are not part of it.
fn detector_log() -> Vec<String> {
    lines(&[
        "Removing initializer 'unused'. It is not used by any node and should be removed from the model.",
        NODE_PLACEMENTS_HEADER,
        " All nodes placed on [DmlExecutionProvider]. Number of nodes: 203",
        "Saving initialized tensors.",
    ])
}

/// ONNX Runtime's verbose log of a session with 186 nodes on DirectML and two, `Slice.2` and
/// `Concat.2`, on the CPU (design §2.2's recognizer).
fn recognizer_log() -> Vec<String> {
    let mut log = vec![NODE_PLACEMENTS_HEADER.to_string(), directml_summary(186)];
    log.extend(directml_nodes(186));
    log.extend(lines(&[
        " Node(s) placed on [CPUExecutionProvider]. Number of nodes: 2",
        "  Slice (Slice.2)",
        "  Concat (Concat.2)",
        "Session successfully initialized.",
    ]));
    log
}

#[test]
fn directml_sessions_turn_off_memory_pattern_and_parallel_execution_and_allow_cpu_nodes() {
    let directml = plan(OnnxExecutionProvider::Directml, 2);
    assert_eq!(
        directml.resolved_execution_provider,
        OnnxExecutionProvider::Directml
    );
    assert_eq!(directml.directml_device_id, Some(2));
    assert_eq!(directml.cuda_ordinal, None);
    assert_eq!(
        directml.registered_execution_providers,
        [OnnxExecutionProvider::Directml]
    );
    assert!(!directml.cpu_ep_registered);
    assert!(!directml.cpu_fallback_disabled);
    assert_eq!(directml.fallback_policy, OcrFallbackPolicy::CpuNodes);

    let options = SessionOptions {
        intra_threads: 1,
        memory_pattern: Some(false),
        parallel_execution: Some(false),
        execution_provider: SessionExecutionProvider::Directml { device_id: 2 },
        cpu_fallback_disabled: false,
    };
    assert_eq!(session_options(&directml), Ok(options));

    // The shadow that reads the placement and the session itself are built from that one value.
    let mut sessions = FakeSessions::logging(recognizer_log());
    let adapter = fake_adapter(2);
    let mut diagnostics = Vec::new();
    let built = load_session_with(
        &mut sessions,
        &request(PpocrModelRole::Recognizer, Some(&adapter)),
        &directml,
        None,
        &mut diagnostics,
    );
    assert!(built.is_ok(), "{:?}", built.err());
    assert_eq!(sessions.builds, [(options, true), (options, false)]);

    // CPU and CUDA sessions keep what they always set: neither sets the memory pattern or the
    // execution mode, and CUDA alone forbids CPU nodes.
    for (backend, execution_provider, cpu_fallback_disabled) in [
        (
            OnnxExecutionProvider::Cpu,
            SessionExecutionProvider::Cpu,
            false,
        ),
        (
            OnnxExecutionProvider::Cuda,
            SessionExecutionProvider::Cuda { device_id: 1 },
            true,
        ),
    ] {
        let other = plan(backend, 0);
        assert_eq!(
            session_options(&other),
            Ok(SessionOptions {
                intra_threads: 1,
                memory_pattern: None,
                parallel_execution: None,
                execution_provider,
                cpu_fallback_disabled,
            })
        );
        assert_eq!(other.fallback_policy, OcrFallbackPolicy::Forbidden);
    }
}

#[test]
fn a_directml_load_records_its_placement_per_model_role() {
    let directml = plan(OnnxExecutionProvider::Directml, 0);
    let adapter = fake_adapter(0);
    let mut diagnostics = Vec::new();
    // The model load builds the recognizer; the first full-frame request builds the detector.
    let recognizer = match load_session_with(
        &mut FakeSessions::logging(recognizer_log()),
        &request(PpocrModelRole::Recognizer, Some(&adapter)),
        &directml,
        None,
        &mut diagnostics,
    ) {
        Ok(built) => built,
        Err(failure) => panic!("recognizer: {failure:?}"),
    };
    let detector = match load_session_with(
        &mut FakeSessions::logging(detector_log()),
        &request(PpocrModelRole::Detector, Some(&adapter)),
        &directml,
        None,
        &mut diagnostics,
    ) {
        Ok(built) => built,
        Err(failure) => panic!("detector: {failure:?}"),
    };
    assert_eq!(recognizer.cpu_nodes, Some(2));
    assert_eq!(detector.cpu_nodes, Some(0));

    // Two v2 reports, one per role, valid together as one call's reports.
    validate_ppocr_call_diagnostics(&diagnostics).expect("one call's reports");
    let [recognizer_report, detector_report] = diagnostics.as_slice() else {
        panic!("two reports: {diagnostics:?}");
    };
    let device = Some(PpocrDiagnosticDevice {
        identity: CARD_IDENTITY.to_string(),
        dxgi_index: 0,
        luid: CARD_LUID_TEXT.to_string(),
    });
    for (report, role, accelerated, cpu) in [
        (recognizer_report, "recognizer", 186, 2),
        (detector_report, "detector", 203, 0),
    ] {
        assert_eq!(
            report.record_type,
            "actingcommand.ppocr_node_placement_diagnostic.v2"
        );
        assert_eq!(report.model_role, role);
        assert_eq!(report.diagnostic_scope, "session_initialization_only");
        assert!(!report.inference_executed);
        assert_eq!(
            report.accelerated_execution_provider,
            "DmlExecutionProvider"
        );
        assert_eq!(report.accelerated_node_count, accelerated);
        assert_eq!(report.cpu_assigned_node_count, cpu);
        assert_eq!(report.device, device);
    }
    let cpu_nodes = recognizer_report
        .nodes
        .iter()
        .map(|node| {
            (
                node.operator_type.as_str(),
                node.node_name.as_str(),
                node.assigned_execution_provider.as_str(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        cpu_nodes,
        [
            ("Concat", "Concat.2", "CPUExecutionProvider"),
            ("Slice", "Slice.2", "CPUExecutionProvider"),
        ]
    );
    assert!(detector_report.nodes.is_empty());
    let json = serde_json::to_value(&**recognizer_report).expect("report JSON");
    assert_eq!(json["device"]["identity"], CARD_IDENTITY);
    assert_eq!(json["device"]["dxgi_index"], 0);
    assert_eq!(json["device"]["luid"], CARD_LUID_TEXT);

    // An invocation's fallback_observed: whether a role that ran has a CPU node. The recognizer
    // has two, so every read that runs it observes the fallback; a full-frame read that keeps
    // no box runs the detector only.
    let placements = RolePlacements {
        recognizer: recognizer.cpu_nodes,
        detector: detector.cpu_nodes,
    };
    let full_frame = InvocationRoles {
        recognizer: true,
        detector: true,
    };
    let region = InvocationRoles {
        recognizer: true,
        detector: false,
    };
    let no_box = InvocationRoles {
        recognizer: false,
        detector: true,
    };
    assert_eq!(
        fallback_observed(&directml, placements, full_frame),
        Ok(Some(true))
    );
    assert_eq!(
        fallback_observed(&directml, placements, region),
        Ok(Some(true))
    );
    assert_eq!(
        fallback_observed(&directml, placements, no_box),
        Ok(Some(false))
    );
    // A CPU plan admits no CPU node to observe.
    assert_eq!(
        fallback_observed(
            &plan(OnnxExecutionProvider::Cpu, 0),
            RolePlacements::default(),
            full_frame
        ),
        Ok(None)
    );
}

#[test]
fn an_unreadable_placement_is_a_session_that_cannot_be_built() {
    let directml = plan(OnnxExecutionProvider::Directml, 0);
    let adapter = fake_adapter(0);
    let mut group_ends_early = vec![NODE_PLACEMENTS_HEADER.to_string(), directml_summary(186)];
    group_ends_early.extend(directml_nodes(3));
    group_ends_early.extend(lines(&[
        " Node(s) placed on [CPUExecutionProvider]. Number of nodes: 2",
        "  Slice (Slice.2)",
        "  Concat (Concat.2)",
    ]));
    let mut cpu_group_incomplete = vec![NODE_PLACEMENTS_HEADER.to_string(), directml_summary(186)];
    cpu_group_incomplete.extend(directml_nodes(186));
    cpu_group_incomplete.extend(lines(&[
        " Node(s) placed on [CPUExecutionProvider]. Number of nodes: 2",
        "  Slice (Slice.2)",
    ]));
    let unreadable = [
        (
            "no placement section",
            lines(&["Session successfully initialized."]),
        ),
        ("a DirectML group that ends early", group_ends_early),
        (
            "a CPU group whose nodes are not all listed",
            cpu_group_incomplete,
        ),
        (
            "a summary that does not read",
            lines(&[
                NODE_PLACEMENTS_HEADER,
                " Node(s) placed on [DmlExecutionProvider]. Number of nodes: many",
            ]),
        ),
        (
            "another accelerated provider than the session's",
            lines(&[
                NODE_PLACEMENTS_HEADER,
                " All nodes placed on [CUDAExecutionProvider]. Number of nodes: 203",
            ]),
        ),
    ];
    for (case, log) in unreadable {
        let mut sessions = FakeSessions::logging(log);
        let mut diagnostics = Vec::new();
        let failure = match load_session_with(
            &mut sessions,
            &request(PpocrModelRole::Recognizer, Some(&adapter)),
            &directml,
            None,
            &mut diagnostics,
        ) {
            Ok(_) => panic!("{case}: a session was built"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.reason,
            Some(GpuUnavailableReason::PlacementUnread),
            "{case}: {failure:?}"
        );
        // Only the shadow was built: the session itself never is, and no report is kept.
        assert_eq!(sessions.builds.len(), 1, "{case}");
        assert!(sessions.builds[0].1, "{case}");
        assert!(diagnostics.is_empty(), "{case}");
        let error = failure.into_error();
        assert_eq!(error.code(), VisionFfiErrorCode::ProviderFailure, "{case}");
        assert!(
            error
                .message()
                .contains("DirectML cannot be used (placement_unread)"),
            "{case}: {}",
            error.message()
        );
    }
}
