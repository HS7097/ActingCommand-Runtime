// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #407 GPU-1 B2 (design rd-1031/perf-407/design-gpu1.md §1.4, §2.3, §2.4 and §2.5;
// coordinator work order rd-1031/perf-407/brief-gpu1b2.md): gpu_fallback cpu moves a model whose
// DirectML session cannot be built to the CPU once, with one vision_gpu_unavailable warning and
// without the replaced session's report; refuse fails the request and the next one tries again;
// a detector that cannot be built moves the whole model, its recognizer too; a process whose
// adapter does not resolve at its runtime initialisation keeps DirectML.dll out of its closure.
// No GPU and no ONNX Runtime: the sessions come from a fake that refuses the DirectML builds it is
// told to and writes ONNX Runtime's verbose placement log into the logger it is given; the
// adapters are fakes with fake identities and LUIDs.

use super::*;
use actingcommand_vision_ffi::{
    D3dkmtAdapterType, DirectmlRuntimeSpec, DxgiAdapter, DxgiLuid, KernelAdapterReading,
    ONNXRUNTIME_LIBRARY, ONNXRUNTIME_SHARED_PROVIDERS_LIBRARY, PciAddress, next_ocr_session_id,
    ppocr_model_content_sha256,
};
use serde_json::json;

const CARD_IDENTITY: &str = "dxgi-pci:1234:5678:9abcdef0:01";
const CARD_LUID: DxgiLuid = DxgiLuid {
    high_part: 0,
    low_part: 0x0001_2345,
};
const MODEL_REF: &str = "fixture-ocr-model";
const MODEL: &[u8] = b"fake model bytes";
const RECOGNIZER: &str = "rec.onnx";
const DETECTOR: &str = "det.onnx";

/// ONNX Runtime's error code as the fake reports it.
#[derive(Debug)]
enum FakeOrtCode {
    EpFail,
}

/// Where a fake DirectML build of one model file fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FailAt {
    /// The zero-inference shadow that reads the node placement.
    Shadow,
    /// The session itself, after its shadow logged the placement.
    Session,
}

/// One build the fake was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Build {
    path: PathBuf,
    execution_provider: SessionExecutionProvider,
    shadow: bool,
    built: bool,
}

fn build(
    path: &str,
    execution_provider: SessionExecutionProvider,
    shadow: bool,
    built: bool,
) -> Build {
    Build {
        path: PathBuf::from(path),
        execution_provider,
        shadow,
        built,
    }
}

/// A session of the fake: the model file and the provider it was built on.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FakeSession {
    path: PathBuf,
    execution_provider: SessionExecutionProvider,
}

/// A fake ONNX Runtime. A DirectML shadow writes the log of its model file into its logger, line
/// by line, as ONNX Runtime's verbose session log does; a DirectML build of a failing file fails
/// with its reason and an ONNX Runtime status. CPU builds always succeed.
#[derive(Default)]
struct FakeSessions {
    logs: Vec<(PathBuf, Vec<String>)>,
    failing: Vec<(PathBuf, FailAt, GpuUnavailableReason)>,
    builds: Vec<Build>,
}

impl FakeSessions {
    fn logging(mut self, path: &str, log: Vec<String>) -> Self {
        self.logs.push((PathBuf::from(path), log));
        self
    }

    fn failing(mut self, path: &str, at: FailAt, reason: GpuUnavailableReason) -> Self {
        self.failing.push((PathBuf::from(path), at, reason));
        self
    }
}

impl SessionBuilds for FakeSessions {
    type Session = FakeSession;

    fn build(
        &mut self,
        _model: &[u8],
        path: &Path,
        options: &SessionOptions,
        placement_logger: Option<LoggerFunction>,
    ) -> Result<FakeSession, SessionBuildFailure> {
        let shadow = placement_logger.is_some();
        let refused = self
            .failing
            .iter()
            .find(|(failing, at, _)| {
                options.execution_provider.is_directml()
                    && failing == path
                    && (*at == FailAt::Shadow) == shadow
            })
            .map(|(_, _, reason)| *reason);
        self.builds.push(Build {
            path: path.to_path_buf(),
            execution_provider: options.execution_provider,
            shadow,
            built: refused.is_none(),
        });
        if let Some(reason) = refused {
            return Err(SessionBuildFailure {
                ort_status: Some(onnxruntime_status(
                    &FakeOrtCode::EpFail,
                    "fake ONNX Runtime: no Direct3D 12 device",
                )),
                ..SessionBuildFailure::step(
                    options,
                    reason,
                    format!("the fake ONNX Runtime refused {}", path.display()),
                )
            });
        }
        if let Some(logger) = placement_logger {
            let log = self
                .logs
                .iter()
                .find(|(logged, _)| logged == path)
                .map(|(_, log)| log.as_slice())
                .unwrap_or_default();
            for line in log {
                logger(
                    LogLevel::Verbose,
                    "onnxruntime",
                    "session",
                    "inference_session.cc",
                    line,
                );
            }
        }
        Ok(FakeSession {
            path: path.to_path_buf(),
            execution_provider: options.execution_provider,
        })
    }
}

/// A fixed adapter listing.
struct FakeAdapters(Vec<DxgiAdapter>);

impl AdapterSource for FakeAdapters {
    fn adapters(&self) -> VisionFfiResult<Vec<DxgiAdapter>> {
        Ok(self.0.clone())
    }
}

/// A physical adapter at `dxgi_index` with `(vendor, device, subsys, revision)`.
fn physical(
    dxgi_index: u32,
    (vendor_id, device_id, subsys_id, revision): (u32, u32, u32, u32),
) -> DxgiAdapter {
    DxgiAdapter {
        dxgi_index,
        vendor_id,
        device_id,
        subsys_id,
        revision,
        luid: CARD_LUID,
        dxgi_flags: 0,
        kernel: KernelAdapterReading::Read {
            adapter_type: D3dkmtAdapterType(D3dkmtAdapterType::RENDER_SUPPORTED),
            pci_address: Some(PciAddress {
                bus: 1,
                device: 0,
                function: 0,
            }),
        },
    }
}

/// The configured card.
fn card(dxgi_index: u32) -> DxgiAdapter {
    physical(dxgi_index, (0x1234, 0x5678, 0x9abc_def0, 0x01))
}

/// Another card, never the configured one.
fn other_card(dxgi_index: u32) -> DxgiAdapter {
    physical(dxgi_index, (0x4321, 0x8765, 0x0fed_cba9, 0x02))
}

fn resolved_card(dxgi_index: u32) -> ResolvedDirectmlAdapter {
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

fn selector() -> DirectmlDeviceSelector {
    DirectmlDeviceSelector {
        expected_identity: CARD_IDENTITY.to_string(),
    }
}

/// The route basis of a `directml` model under `gpu_fallback`.
fn basis(gpu_fallback: GpuFallback) -> RouteBasis {
    RouteBasis {
        session_id: next_ocr_session_id().expect("session id"),
        parts: OcrSessionKeyParts {
            engine_binding_sha256: "e".repeat(64),
            runtime_library_path: "onnxruntime.dll".to_string(),
            runtime_library_sha256: "d".repeat(64),
            onnxruntime_version: DIRECTML_RUNTIME_PIN.onnxruntime_version.to_string(),
            model_ref: MODEL_REF.to_string(),
            model_sha256: ppocr_model_content_sha256(
                &"a".repeat(64),
                &"b".repeat(64),
                &"c".repeat(64),
                None,
            )
            .expect("model hash"),
            requested_backend: OnnxExecutionProvider::Directml,
            requested_cuda_device: None,
            resolved_cuda_device: None,
            requested_directml_device: Some(selector()),
            resolved_directml_device: None,
        },
        gpu_fallback: Some(gpu_fallback),
    }
}

fn input(path: &str) -> ModelInput<'_> {
    ModelInput {
        bytes: MODEL,
        path: Path::new(path),
    }
}

fn lines(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|line| (*line).to_string()).collect()
}

/// The verbose log of a recognizer with 186 nodes on DirectML and two on the CPU.
fn recognizer_log() -> Vec<String> {
    let mut log = lines(&[
        NODE_PLACEMENTS_HEADER,
        " Node(s) placed on [DmlExecutionProvider]. Number of nodes: 186",
    ]);
    log.extend((0..186).map(|node| format!("  Conv (conv_{node})")));
    log.extend(lines(&[
        " Node(s) placed on [CPUExecutionProvider]. Number of nodes: 2",
        "  Slice (Slice.2)",
        "  Concat (Concat.2)",
    ]));
    log
}

/// The verbose log of a detector whose 203 nodes all run on DirectML.
fn detector_log() -> Vec<String> {
    lines(&[
        NODE_PLACEMENTS_HEADER,
        " All nodes placed on [DmlExecutionProvider]. Number of nodes: 203",
    ])
}

fn directml(device_id: i32) -> SessionExecutionProvider {
    SessionExecutionProvider::Directml { device_id }
}

const CPU: SessionExecutionProvider = SessionExecutionProvider::Cpu;

const FULL_FRAME: InvocationRoles = InvocationRoles {
    recognizer: true,
    detector: true,
};

/// `warning` is the one `vision_gpu_unavailable` of a fallback under `gpu_fallback` `cpu` for
/// `reason`, naming `model` when it is one model's.
fn assert_warning(warning: &Outcome, reason: GpuUnavailableReason, model: Option<&str>) {
    assert!(
        warning.is(VisionGpuCode::VisionGpuUnavailable),
        "{warning:?}"
    );
    assert_eq!(
        warning.value("execution_provider"),
        Some(&json!(OnnxExecutionProvider::Directml.as_str()))
    );
    assert_eq!(
        warning.value("gpu_unavailable_reason"),
        Some(&json!(reason.as_str()))
    );
    assert_eq!(
        warning.value("gpu_fallback"),
        Some(&json!(GpuFallback::Cpu.as_str()))
    );
    assert_eq!(
        warning.value("model_name"),
        model.map(|model| json!(model)).as_ref()
    );
}

/// A route that runs the whole model on the CPU for a `directml` configuration.
fn assert_cpu_model(route: &ModelRoute) {
    let key = route.binding.key();
    assert_eq!(key.requested_backend(), OnnxExecutionProvider::Directml);
    assert_eq!(key.requested_directml_device(), Some(&selector()));
    assert_eq!(key.resolved_directml_device(), None);
    let plan = &route.plan;
    assert_eq!(plan.resolved_execution_provider, OnnxExecutionProvider::Cpu);
    assert_eq!(
        plan.registered_execution_providers,
        [OnnxExecutionProvider::Cpu]
    );
    assert!(plan.cpu_ep_registered);
    assert!(!plan.cpu_fallback_disabled);
    assert_eq!(plan.fallback_policy, OcrFallbackPolicy::CpuModel);
    assert_eq!(route.adapter, None);
    assert_eq!(route.execution.requested, OnnxExecutionProvider::Directml);
    assert_eq!(route.execution.actual, OnnxExecutionProvider::Cpu);
}

#[test]
fn a_session_that_cannot_be_built_falls_back_to_cpu_once_under_fallback_cpu() {
    let basis = basis(GpuFallback::Cpu);
    let route = basis
        .route(1, Some(resolved_card(0)), None)
        .expect("the DirectML route");
    assert_eq!(
        route.plan.resolved_execution_provider,
        OnnxExecutionProvider::Directml
    );
    let mut sessions = FakeSessions::default()
        .logging(RECOGNIZER, recognizer_log())
        .logging(DETECTOR, detector_log())
        .failing(
            RECOGNIZER,
            FailAt::Session,
            GpuUnavailableReason::ProviderRegisterFailed,
        );
    let mut diagnostics = Vec::new();
    let mut model = ModelSessions::load(
        &mut sessions,
        basis.clone(),
        route,
        input(RECOGNIZER),
        Some("1".to_string()),
        &mut diagnostics,
    )
    .unwrap_or_else(|error| panic!("the model falls back and loads: {error:?}"));

    // DirectML was tried once (its shadow read the placement, the session failed); the
    // recognizer was then built once on the CPU, which reads no placement, although the
    // placement variable is set.
    assert_eq!(
        sessions.builds,
        [
            build(RECOGNIZER, directml(0), true, true),
            build(RECOGNIZER, directml(0), false, false),
            build(RECOGNIZER, CPU, false, true),
        ]
    );
    assert_eq!(model.recognizer.session.execution_provider, CPU);
    // The report of the replaced DirectML session is dropped.
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_cpu_model(&model.route);
    assert_eq!(model.route.binding.generation(), 1);
    assert_eq!(
        fallback_observed(&model.route.plan, model.placements(), FULL_FRAME),
        Ok(Some(true))
    );
    // One warning, naming the model, with ONNX Runtime's status as its link.
    let execution = model.status.current();
    assert_eq!(execution, model.route.execution);
    let warning = execution.warning.clone().expect("the fallback's warning");
    assert_warning(
        &warning,
        GpuUnavailableReason::ProviderRegisterFailed,
        Some(MODEL_REF),
    );
    let [link] = warning.causes() else {
        panic!("one link: {warning:?}");
    };
    assert_eq!(link.outcome().code().as_str(), "foreign_vision_error");
    assert_eq!(link.outcome().value("raw_source"), Some(&json!("vision")));
    assert_eq!(link.outcome().value("raw_code"), Some(&json!("ep_fail")));

    // Once: the detector of the model on the CPU is built on the CPU; DirectML is not tried
    // again, the recognizer is not rebuilt, and the status keeps its one warning.
    model
        .build_detector(
            &mut sessions,
            input(DETECTOR),
            &FakeAdapters(vec![card(0)]),
            || Err(failure("the recognizer is not rebuilt")),
            &mut diagnostics,
        )
        .unwrap_or_else(|error| panic!("the CPU detector: {error:?}"));
    assert_eq!(sessions.builds[3..], [build(DETECTOR, CPU, false, true)]);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(model.status.current(), execution);

    // An adapter that does not resolve at a model load is such a session too: the model loads on
    // the CPU with one warning of its own.
    let fallback = initial_route(
        &basis,
        Some(&ProcessRoute::Directml(resolved_card(0))),
        &FakeAdapters(vec![other_card(0)]),
    )
    .unwrap_or_else(|error| panic!("the CPU route: {error:?}"));
    assert_cpu_model(&fallback);
    assert_warning(
        fallback.execution.warning.as_ref().expect("its warning"),
        GpuUnavailableReason::AdapterMissing,
        Some(MODEL_REF),
    );
}

#[test]
fn a_session_that_cannot_be_built_fails_the_request_under_refuse() {
    let basis = basis(GpuFallback::Refuse);
    let route = || {
        basis
            .route(1, Some(resolved_card(0)), None)
            .expect("the DirectML route")
    };

    // The load fails and nothing is built on the CPU.
    let mut sessions = FakeSessions::default()
        .logging(RECOGNIZER, recognizer_log())
        .failing(
            RECOGNIZER,
            FailAt::Session,
            GpuUnavailableReason::ProviderRegisterFailed,
        );
    let refused = match ModelSessions::load(
        &mut sessions,
        basis.clone(),
        route(),
        input(RECOGNIZER),
        None,
        &mut Vec::new(),
    ) {
        Ok(_) => panic!("a model was loaded under refuse"),
        Err(error) => error,
    };
    assert_eq!(refused.code(), VisionFfiErrorCode::ProviderFailure);
    assert!(
        refused
            .message()
            .contains("DirectML cannot be used (provider_register_failed)"),
        "{}",
        refused.message()
    );
    assert!(
        sessions
            .builds
            .iter()
            .all(|build| build.execution_provider == directml(0)),
        "{:?}",
        sessions.builds
    );

    // A detector that cannot be built fails its request, leaves the model as it was, and the
    // next request tries DirectML again.
    let mut sessions = FakeSessions::default()
        .logging(RECOGNIZER, recognizer_log())
        .logging(DETECTOR, detector_log())
        .failing(
            DETECTOR,
            FailAt::Shadow,
            GpuUnavailableReason::SessionBuildFailed,
        );
    let mut model = ModelSessions::load(
        &mut sessions,
        basis.clone(),
        route(),
        input(RECOGNIZER),
        None,
        &mut Vec::new(),
    )
    .unwrap_or_else(|error| panic!("the DirectML model: {error:?}"));
    let loaded = model.status.current();
    assert_eq!(
        loaded,
        OcrModelExecution::as_requested(OnnxExecutionProvider::Directml)
    );
    for attempt in 0..2 {
        let refused = match model.build_detector(
            &mut sessions,
            input(DETECTOR),
            &FakeAdapters(vec![card(0)]),
            || Err(failure("the recognizer is not rebuilt")),
            &mut Vec::new(),
        ) {
            Ok(()) => panic!("attempt {attempt}: a detector was built under refuse"),
            Err(error) => error,
        };
        assert_eq!(
            refused.code(),
            VisionFfiErrorCode::ProviderFailure,
            "attempt {attempt}"
        );
        assert!(
            refused
                .message()
                .contains("DirectML cannot be used (session_build_failed)"),
            "attempt {attempt}: {}",
            refused.message()
        );
        assert!(model.detector.is_none(), "attempt {attempt}");
        assert_eq!(
            model.route.plan.resolved_execution_provider,
            OnnxExecutionProvider::Directml,
            "attempt {attempt}"
        );
        assert_eq!(
            model.recognizer.session.execution_provider,
            directml(0),
            "attempt {attempt}"
        );
        assert_eq!(model.status.current(), loaded, "attempt {attempt}");
    }
    let detector_builds = sessions
        .builds
        .iter()
        .filter(|build| build.path.as_path() == Path::new(DETECTOR))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        detector_builds,
        [
            build(DETECTOR, directml(0), true, false),
            build(DETECTOR, directml(0), true, false),
        ]
    );

    // An adapter that does not resolve at a model load fails it too.
    let refused = initial_route(
        &basis,
        Some(&ProcessRoute::Directml(resolved_card(0))),
        &FakeAdapters(vec![other_card(0)]),
    )
    .expect_err("no CPU route under refuse");
    assert_eq!(refused.code(), VisionFfiErrorCode::ProviderUnavailable);
    assert!(
        refused
            .message()
            .contains("DirectML cannot be used (adapter_missing)"),
        "{}",
        refused.message()
    );
}

#[test]
fn a_detector_that_falls_back_moves_the_whole_model_to_cpu() {
    let mut sessions = FakeSessions::default()
        .logging(RECOGNIZER, recognizer_log())
        .logging(DETECTOR, detector_log())
        .failing(
            DETECTOR,
            FailAt::Session,
            GpuUnavailableReason::SessionBuildFailed,
        );
    let basis = basis(GpuFallback::Cpu);
    let route = basis
        .route(1, Some(resolved_card(0)), None)
        .expect("the DirectML route");
    let mut load_diagnostics = Vec::new();
    let mut model = ModelSessions::load(
        &mut sessions,
        basis,
        route,
        input(RECOGNIZER),
        None,
        &mut load_diagnostics,
    )
    .unwrap_or_else(|error| panic!("the DirectML model: {error:?}"));
    assert_eq!(model.recognizer.session.execution_provider, directml(0));
    assert_eq!(
        load_diagnostics.len(),
        1,
        "the recognizer's DirectML report"
    );

    // The first full-frame request carries the load's report and builds the detector, which
    // cannot be built: the recognizer moves to the CPU with it.
    let mut diagnostics = std::mem::take(&mut load_diagnostics);
    let mut rereads = 0;
    model
        .build_detector(
            &mut sessions,
            input(DETECTOR),
            &FakeAdapters(vec![card(0)]),
            || {
                rereads += 1;
                Ok((MODEL.to_vec(), PathBuf::from(RECOGNIZER)))
            },
            &mut diagnostics,
        )
        .unwrap_or_else(|error| panic!("the whole model on the CPU: {error:?}"));
    assert_eq!(rereads, 1, "the recognizer file is read once again");
    assert_eq!(
        sessions.builds,
        [
            build(RECOGNIZER, directml(0), true, true),
            build(RECOGNIZER, directml(0), false, true),
            build(DETECTOR, directml(0), true, true),
            build(DETECTOR, directml(0), false, false),
            build(RECOGNIZER, CPU, false, true),
            build(DETECTOR, CPU, false, true),
        ]
    );
    assert_eq!(model.recognizer.session.execution_provider, CPU);
    assert_eq!(
        model
            .detector
            .as_ref()
            .map(|detector| detector.session.execution_provider),
        Some(CPU)
    );
    // Both replaced DirectML sessions' reports are dropped: the recognizer's from the load and
    // the detector's from this request.
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    // The whole model is on the CPU, one session generation later, with one warning.
    assert_cpu_model(&model.route);
    assert_eq!(model.route.binding.generation(), 2);
    assert_eq!(
        fallback_observed(&model.route.plan, model.placements(), FULL_FRAME),
        Ok(Some(true))
    );
    let execution = model.status.current();
    assert_eq!(execution, model.route.execution);
    assert_warning(
        execution.warning.as_ref().expect("the fallback's warning"),
        GpuUnavailableReason::SessionBuildFailed,
        Some(MODEL_REF),
    );
}

#[test]
fn an_unresolved_adapter_at_runtime_init_keeps_directml_out_of_the_closure() {
    let ort = Path::new("vision-root").join("ort");
    let shared = ort.join(ONNXRUNTIME_SHARED_PROVIDERS_LIBRARY);
    let onnxruntime = ort.join(ONNXRUNTIME_LIBRARY);
    let directml_library = ort.join(DIRECTML_RUNTIME_PIN.directml.file_name);
    let spec = |gpu_fallback| VisionRuntimeSpec {
        onnxruntime_library: onnxruntime.clone(),
        runtime_library_closure: vec![shared.clone(), onnxruntime.clone()],
        expected_onnxruntime_sha256: Some(DIRECTML_RUNTIME_PIN.onnxruntime.sha256.to_string()),
        execution_provider: OnnxExecutionProvider::Directml,
        cuda_device: None,
        directml: Some(DirectmlRuntimeSpec {
            library: directml_library.clone(),
            device: selector(),
            gpu_fallback,
        }),
    };

    // No adapter has the identity: the process loads the CPU closure, without DirectML.dll, and
    // records one warning that names no model.
    let unresolved = FakeAdapters(vec![other_card(0)]);
    let route = directml_runtime_route(&spec(GpuFallback::Cpu), Ok(()), &unresolved)
        .unwrap_or_else(|error| panic!("the CPU process: {error:?}"));
    assert_eq!(route.closure, [shared.clone(), onnxruntime.clone()]);
    assert!(!route.closure.contains(&directml_library));
    let ProcessRoute::Cpu(warning) = &route.process else {
        panic!("a CPU process: {:?}", route.process);
    };
    assert_warning(warning, GpuUnavailableReason::AdapterMissing, None);

    // Every model of that process loads on the CPU with that one warning, even when the adapter
    // would resolve now.
    let model = initial_route(
        &basis(GpuFallback::Cpu),
        Some(&route.process),
        &FakeAdapters(vec![card(0)]),
    )
    .unwrap_or_else(|error| panic!("the model's CPU route: {error:?}"));
    assert_cpu_model(&model);
    assert_eq!(model.execution.warning.as_ref(), Some(warning));

    // A DirectML.dll that is missing keeps it out the same way.
    let missing = check_directml_library(&directml_library);
    let route = directml_runtime_route(&spec(GpuFallback::Cpu), missing, &unresolved)
        .unwrap_or_else(|error| panic!("the CPU process: {error:?}"));
    assert_eq!(route.closure, [shared.clone(), onnxruntime.clone()]);
    let ProcessRoute::Cpu(warning) = &route.process else {
        panic!("a CPU process: {:?}", route.process);
    };
    assert_warning(warning, GpuUnavailableReason::DirectmlLibraryMissing, None);

    // Under refuse the initialisation fails instead, and loads nothing.
    let refused = directml_runtime_route(&spec(GpuFallback::Refuse), Ok(()), &unresolved)
        .expect_err("no CPU process under refuse");
    assert_eq!(refused.code(), VisionFfiErrorCode::ProviderUnavailable);
    assert!(
        refused
            .message()
            .contains("DirectML cannot be used (adapter_missing)"),
        "{}",
        refused.message()
    );

    // The adapter that resolves puts DirectML.dll right before ONNX Runtime.
    let route = directml_runtime_route(
        &spec(GpuFallback::Cpu),
        Ok(()),
        &FakeAdapters(vec![other_card(0), card(1)]),
    )
    .unwrap_or_else(|error| panic!("the DirectML process: {error:?}"));
    assert_eq!(route.closure, [shared, directml_library, onnxruntime]);
    let ProcessRoute::Directml(adapter) = &route.process else {
        panic!("a DirectML process: {:?}", route.process);
    };
    assert_eq!(adapter.identity, CARD_IDENTITY);
    assert_eq!(adapter.dxgi_index, 1);
}
