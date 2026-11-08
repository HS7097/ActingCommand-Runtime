// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #407 GPU-1 E (design rd-1031/perf-407/design-gpu1.md §7 row E and §9 Q8; coordinator
// work order rd-1031/perf-407/brief-gpu1e.md): a model whose DirectML run fails is built again
// on its next load, which resolves its adapter again; when the first run after that rebuild fails
// too, under gpu_fallback cpu the whole model runs on the CPU from its next load until the
// process ends, with one vision_gpu_unavailable warning; under refuse every failed run builds it
// again; a model on the CPU keeps its errors as before. A failed DirectML run is the engine's
// ProviderUnavailable, the code on which the kernel unloads an engine on a GPU. No GPU and no ONNX
// Runtime: the sessions come from a fake that writes ONNX Runtime's verbose placement log into
// the logger it is given, the failed runs are fake ONNX Runtime errors, and the adapters are fakes
// with fake identities and LUIDs.

use super::*;
use actingcommand_vision_ffi::{
    D3dkmtAdapterType, DxgiAdapter, DxgiLuid, KernelAdapterReading, PciAddress,
    next_ocr_session_id, ppocr_model_content_sha256,
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
const DEVICE_REMOVED: &str = "fake ONNX Runtime: the Direct3D 12 device was removed";

/// ONNX Runtime's error code as the fake reports it.
#[derive(Debug)]
enum FakeOrtCode {
    Fail,
}

/// A fake ONNX Runtime whose builds all succeed. A session is the provider it was built on; a
/// DirectML shadow writes a recognizer's verbose placement log into its logger. Each build is
/// recorded with whether it was a shadow.
#[derive(Default)]
struct FakeSessions {
    builds: Vec<(SessionExecutionProvider, bool)>,
}

impl SessionBuilds for FakeSessions {
    type Session = SessionExecutionProvider;

    fn build(
        &mut self,
        _model: &[u8],
        _path: &Path,
        options: &SessionOptions,
        placement_logger: Option<LoggerFunction>,
    ) -> Result<SessionExecutionProvider, SessionBuildFailure> {
        self.builds
            .push((options.execution_provider, placement_logger.is_some()));
        if let Some(logger) = placement_logger {
            for line in recognizer_log() {
                logger(
                    LogLevel::Verbose,
                    "onnxruntime",
                    "session",
                    "inference_session.cc",
                    &line,
                );
            }
        }
        Ok(options.execution_provider)
    }
}

/// The verbose log of a recognizer with 186 nodes on DirectML and two on the CPU.
fn recognizer_log() -> Vec<String> {
    let mut log = vec![
        NODE_PLACEMENTS_HEADER.to_string(),
        " Node(s) placed on [DmlExecutionProvider]. Number of nodes: 186".to_string(),
    ];
    log.extend((0..186).map(|node| format!("  Conv (conv_{node})")));
    log.extend([
        " Node(s) placed on [CPUExecutionProvider]. Number of nodes: 2".to_string(),
        "  Slice (Slice.2)".to_string(),
        "  Concat (Concat.2)".to_string(),
    ]);
    log
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

/// The process's route: DirectML, on the card the first model load resolved at DXGI index 0.
fn directml_process() -> ProcessRoute {
    ProcessRoute::Directml(ResolvedDirectmlAdapter {
        dxgi_index: 0,
        identity: CARD_IDENTITY.to_string(),
        luid: CARD_LUID,
        pci_address: Some(PciAddress {
            bus: 1,
            device: 0,
            function: 0,
        }),
    })
}

/// The key parts of the model, requesting `requested_backend`.
fn parts(requested_backend: OnnxExecutionProvider) -> OcrSessionKeyParts {
    let directml = requested_backend == OnnxExecutionProvider::Directml;
    OcrSessionKeyParts {
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
        requested_backend,
        requested_cuda_device: None,
        resolved_cuda_device: None,
        requested_directml_device: directml.then(|| DirectmlDeviceSelector {
            expected_identity: CARD_IDENTITY.to_string(),
        }),
        resolved_directml_device: None,
    }
}

/// The route basis of a `directml` model under `gpu_fallback`.
fn directml_basis(gpu_fallback: GpuFallback) -> RouteBasis {
    RouteBasis {
        session_id: next_ocr_session_id().expect("session id"),
        parts: parts(OnnxExecutionProvider::Directml),
        gpu_fallback: Some(gpu_fallback),
    }
}

/// One load of the model as `load_ocr` makes it, over the fakes: its route from `gpu_runs` and
/// the process's DirectML route on `adapters`, then its recognizer.
fn load(
    builds: &mut FakeSessions,
    basis: &RouteBasis,
    adapters: Vec<DxgiAdapter>,
    gpu_runs: &GpuRuns,
) -> ModelSessions<SessionExecutionProvider> {
    let process = directml_process();
    let route = load_route(basis, Some(&process), &FakeAdapters(adapters), gpu_runs)
        .unwrap_or_else(|error| panic!("the model's route: {error:?}"));
    ModelSessions::load(
        builds,
        basis.clone(),
        route,
        ModelInput {
            bytes: MODEL,
            path: Path::new(RECOGNIZER),
        },
        None,
        &mut Vec::new(),
    )
    .unwrap_or_else(|error| panic!("the model loads: {error:?}"))
}

/// The failed run of the `role` session, as ONNX Runtime reports a removed device.
fn device_removed(role: &str) -> ProviderInvokeError {
    ProviderInvokeError::Run(SessionRunFailure {
        message: format!("PPOCR {role} inference failed: {DEVICE_REMOVED}"),
        ort_status: onnxruntime_status(&FakeOrtCode::Fail, DEVICE_REMOVED),
    })
}

fn directml(device_id: i32) -> SessionExecutionProvider {
    SessionExecutionProvider::Directml { device_id }
}

const CPU: SessionExecutionProvider = SessionExecutionProvider::Cpu;

const REGION: InvocationRoles = InvocationRoles {
    recognizer: true,
    detector: false,
};

/// `error` is a failed DirectML run's: `ProviderUnavailable` (the kernel unloads the engine on it),
/// led by its reason and its run's message, followed by `next`.
fn assert_failed_run(error: &VisionFfiError, role: &str, next: &str) {
    assert_eq!(error.code(), VisionFfiErrorCode::ProviderUnavailable);
    assert!(
        error.message().starts_with(&format!(
            "DirectML cannot be used (inference_failed): PPOCR {role} inference failed: {DEVICE_REMOVED}; "
        )),
        "{}",
        error.message()
    );
    assert!(error.message().contains(next), "{}", error.message());
}

const BUILT_AGAIN: &str = "the model's next request builds it again";
const ON_CPU_UNTIL_RESTART: &str =
    "from its next load it runs on the CPU until the process restarts";

/// `model` runs whole on the CPU for its `directml` configuration, with `warning`.
fn assert_cpu_model(model: &ModelSessions<SessionExecutionProvider>, warning: &Outcome) {
    let key = model.route.binding.key();
    assert_eq!(key.requested_backend(), OnnxExecutionProvider::Directml);
    assert_eq!(key.resolved_directml_device(), None);
    let plan = &model.route.plan;
    assert_eq!(plan.resolved_execution_provider, OnnxExecutionProvider::Cpu);
    assert_eq!(
        plan.registered_execution_providers,
        [OnnxExecutionProvider::Cpu]
    );
    assert!(plan.cpu_ep_registered);
    assert_eq!(plan.fallback_policy, OcrFallbackPolicy::CpuModel);
    assert_eq!(model.recognizer.session, CPU);
    assert_eq!(
        fallback_observed(plan, model.placements(), REGION),
        Ok(Some(true))
    );
    assert_eq!(
        model.status.current(),
        OcrModelExecution {
            requested: OnnxExecutionProvider::Directml,
            actual: OnnxExecutionProvider::Cpu,
            warning: Some(warning.clone()),
        }
    );
}

#[test]
fn a_failed_gpu_run_rebuilds_the_model_once_then_falls_back() {
    let basis = directml_basis(GpuFallback::Cpu);
    let gpu_runs = GpuRuns::default();
    let mut builds = FakeSessions::default();

    // The model loads on DirectML, at the card's index.
    let first = load(&mut builds, &basis, vec![card(0)], &gpu_runs);
    assert_eq!(first.recognizer.session, directml(0));
    assert_eq!(
        first.status.current(),
        OcrModelExecution::as_requested(OnnxExecutionProvider::Directml)
    );

    // Its run fails: the engine answers ProviderUnavailable, so the kernel unloads it, and the
    // model is to be built again, not moved to the CPU.
    let failed = first.invocation_failed(&gpu_runs, device_removed("recognizer"));
    assert_failed_run(&failed, "recognizer", BUILT_AGAIN);
    assert_eq!(gpu_runs.cpu_warning(), None);
    drop(first);

    // The next request builds it again, resolving the adapter again: the card is at DXGI index 1
    // now, and the rebuilt model runs on DirectML there (its shadow reads the placement again),
    // with no warning.
    builds.builds.clear();
    let rebuilt = load(&mut builds, &basis, vec![other_card(0), card(1)], &gpu_runs);
    assert_eq!(builds.builds, [(directml(1), true), (directml(1), false)]);
    assert_eq!(
        rebuilt
            .route
            .binding
            .key()
            .resolved_directml_device()
            .map(|device| device.dxgi_index),
        Some(1)
    );
    assert_eq!(
        rebuilt.status.current(),
        OcrModelExecution::as_requested(OnnxExecutionProvider::Directml)
    );

    // The first run of the rebuilt model fails too: under cpu the model runs on the CPU from its
    // next load, with one warning that names it and carries ONNX Runtime's status.
    let failed = rebuilt.invocation_failed(&gpu_runs, device_removed("detector"));
    assert_failed_run(&failed, "detector", ON_CPU_UNTIL_RESTART);
    drop(rebuilt);
    let warning = gpu_runs.cpu_warning().expect("the fallback's one warning");
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
        Some(&json!(GpuUnavailableReason::InferenceFailed.as_str()))
    );
    assert_eq!(
        warning.value("gpu_fallback"),
        Some(&json!(GpuFallback::Cpu.as_str()))
    );
    assert_eq!(warning.value("model_name"), Some(&json!(MODEL_REF)));
    let [link] = warning.causes() else {
        panic!("one link: {warning:?}");
    };
    assert_eq!(link.outcome().code().as_str(), "foreign_vision_error");
    assert_eq!(link.outcome().value("raw_code"), Some(&json!("fail")));
    assert_eq!(
        link.outcome().value("raw_text"),
        Some(&json!(DEVICE_REMOVED))
    );

    // Until the process ends, every load (the next request's, one after an unload to make room)
    // runs the whole model on the CPU with that same one warning; DirectML is not tried again,
    // although the card resolves. A failed run of the model on the CPU is the engine's
    // ProviderFailure with its message as before, which the kernel keeps loaded, and changes
    // nothing.
    for load_after_fallback in 0..2 {
        builds.builds.clear();
        let on_cpu = load(&mut builds, &basis, vec![card(0)], &gpu_runs);
        assert_eq!(builds.builds, [(CPU, false)], "load {load_after_fallback}");
        assert_cpu_model(&on_cpu, &warning);
        let error = on_cpu.invocation_failed(&gpu_runs, device_removed("recognizer"));
        assert_eq!(
            error,
            failure(format!(
                "PPOCR recognizer inference failed: {DEVICE_REMOVED}"
            )),
            "load {load_after_fallback}"
        );
        assert_eq!(gpu_runs.cpu_warning().as_ref(), Some(&warning));
    }

    // Once per failure: a rebuilt model whose first invocation succeeds is sound again, so a later
    // failed run builds it again instead of moving it to the CPU. A timeout and a failure that is
    // no run fail as before and change nothing.
    let gpu_runs = GpuRuns::default();
    let model = load(&mut builds, &basis, vec![card(0)], &gpu_runs);
    let failed = model.invocation_failed(&gpu_runs, device_removed("recognizer"));
    assert_failed_run(&failed, "recognizer", BUILT_AGAIN);
    let rebuilt = load(&mut builds, &basis, vec![card(0)], &gpu_runs);
    rebuilt.invocation_succeeded(&gpu_runs);
    let failed = rebuilt.invocation_failed(&gpu_runs, device_removed("recognizer"));
    assert_failed_run(&failed, "recognizer", BUILT_AGAIN);
    assert_eq!(gpu_runs.cpu_warning(), None);
    let rebuilt = load(&mut builds, &basis, vec![card(0)], &gpu_runs);
    assert_eq!(rebuilt.recognizer.session, directml(0));
    let timeout = rebuilt.invocation_failed(
        &gpu_runs,
        ProviderInvokeError::timeout("PPOCR recognizer inference exceeded the configured timeout"),
    );
    assert_eq!(timeout.code(), VisionFfiErrorCode::Timeout);
    let decode = rebuilt.invocation_failed(
        &gpu_runs,
        ProviderInvokeError::from("PPOCR recognizer returned no outputs".to_string()),
    );
    assert_eq!(
        decode,
        failure("PPOCR recognizer returned no outputs".to_string())
    );
    assert_eq!(gpu_runs.cpu_warning(), None);

    // A rebuild whose adapter no longer resolves falls back as B2 does: on the CPU with that
    // load's own adapter_missing warning, until it is unloaded; the next load that finds the card
    // is on DirectML again, and its first run decides.
    let on_cpu = load(&mut builds, &basis, vec![other_card(0)], &gpu_runs);
    assert_eq!(on_cpu.recognizer.session, CPU);
    let missing = on_cpu
        .status
        .current()
        .warning
        .expect("the adapter's fallback warning");
    assert_eq!(
        missing.value("gpu_unavailable_reason"),
        Some(&json!(GpuUnavailableReason::AdapterMissing.as_str()))
    );
    assert_eq!(gpu_runs.cpu_warning(), None);
    let rebuilt = load(&mut builds, &basis, vec![card(0)], &gpu_runs);
    assert_eq!(rebuilt.recognizer.session, directml(0));
    let failed = rebuilt.invocation_failed(&gpu_runs, device_removed("recognizer"));
    assert_failed_run(&failed, "recognizer", ON_CPU_UNTIL_RESTART);

    // Under refuse every failed run builds the model again on DirectML; it never moves to the
    // CPU.
    let basis = directml_basis(GpuFallback::Refuse);
    let gpu_runs = GpuRuns::default();
    for attempt in 0..3 {
        let model = load(&mut builds, &basis, vec![card(0)], &gpu_runs);
        assert_eq!(model.recognizer.session, directml(0), "attempt {attempt}");
        let failed = model.invocation_failed(&gpu_runs, device_removed("recognizer"));
        assert_failed_run(&failed, "recognizer", BUILT_AGAIN);
        assert_eq!(gpu_runs.cpu_warning(), None, "attempt {attempt}");
    }

    // A model of a cpu configuration keeps today's behaviour: its failed run is ProviderFailure
    // with the message it always had.
    let cpu_basis = RouteBasis {
        session_id: next_ocr_session_id().expect("session id"),
        parts: parts(OnnxExecutionProvider::Cpu),
        gpu_fallback: None,
    };
    let route = cpu_basis
        .route(1, None, None)
        .expect("the CPU model's route");
    let cpu_model = ModelSessions::load(
        &mut builds,
        cpu_basis,
        route,
        ModelInput {
            bytes: MODEL,
            path: Path::new(RECOGNIZER),
        },
        None,
        &mut Vec::new(),
    )
    .unwrap_or_else(|error| panic!("the CPU model: {error:?}"));
    let gpu_runs = GpuRuns::default();
    let error = cpu_model.invocation_failed(&gpu_runs, device_removed("recognizer"));
    assert_eq!(
        error,
        failure(format!(
            "PPOCR recognizer inference failed: {DEVICE_REMOVED}"
        ))
    );
    cpu_model.invocation_succeeded(&gpu_runs);
    assert_eq!(gpu_runs.cpu_warning(), None);
}
