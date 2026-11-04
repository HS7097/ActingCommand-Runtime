// SPDX-License-Identifier: AGPL-3.0-only

//! #407 PERF-READ (brief rd-1031/perf-407/brief-perf-read.md; design
//! rd-1031/perf-407/model-perf-012.md §1.1, §2, §5.1; register §1.280): the performance
//! status's `memory` and `vision`, the host's part.
//!
//! - Each memory part is its owner's own reading: the capture frame stores the host opened (a
//!   store joins at its open and leaves when it is dropped), the vision provider's loaded models
//!   and per-model times, the ledger's capture queue, the running runs' measured packages and the
//!   two fact stores; the process's counters are the sampler's.
//! - A failed process sample leaves `memory.process` absent, never zero, with the warning
//!   `perf_memory_unavailable` caused by the sampler's failure; the parts are shown all the same.
//! - The parts never claim more than the process's private bytes: when they would, the process
//!   is absent with `perf_memory_unavailable` naming both, and the status keeps its shape rules.
//!
//! The process's counters come from a fake sampler and every frame store reads a fake memory
//! sample; nothing sleeps.

use super::*;
use actingcommand_artifact_store::{
    FrameStore, FrameStoreFrameInput, MemorySample, RecognitionState,
};
use actingcommand_contract::outcome::{Code, Outcome};
use actingcommand_contract::{
    FrameStoresMemoryStatus, PackagesMemoryStatus, ProcessMemoryStatus, RuntimeMemoryStatus,
    RuntimePerformanceStatus, VisionModelKind, VisionModelMemoryStatus, VisionModelTimingStatus,
};
use actingcommand_host_metrics::{HostMetricsCode, OwnProcessMemory};

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;
/// Every capture frame store's memory sample: plenty, so a frame stays below every tier.
const FRAME_MEMORY: MemorySample = MemorySample {
    total_bytes: 32 * GIB,
    available_bytes: 16 * GIB,
};
/// The process's counters as the fake sampler reads them.
const PROCESS: OwnProcessMemory = OwnProcessMemory {
    private_bytes: 300 * MIB,
    working_set_bytes: 120 * MIB,
    peak_working_set_bytes: 390 * MIB,
};
/// The running run's measured main package.
const PACKAGE_BYTES: u64 = 3 * MIB;
/// The fake provider's loaded model and its load's growth.
const MODEL_REF: &str = "ocr-fixture";
const MODEL_LOAD_BYTES: u64 = 40 * MIB;

type Sampler = Arc<dyn Fn() -> Result<OwnProcessMemory, Code> + Send + Sync>;

/// A vision provider that runs nothing and keeps a fixed account of one loaded model and its
/// requests.
#[derive(Debug)]
struct AccountedVision;

fn no_model() -> VisionProviderError {
    VisionProviderError::new(
        VisionProviderErrorCode::Unavailable,
        "no model runs in this fixture",
    )
}

fn loaded_model() -> VisionModelMemoryStatus {
    VisionModelMemoryStatus {
        model_ref: MODEL_REF.to_owned(),
        kind: VisionModelKind::Ocr,
        load_private_bytes: Some(MODEL_LOAD_BYTES),
    }
}

fn model_timing() -> VisionModelTimingStatus {
    VisionModelTimingStatus {
        model_ref: MODEL_REF.to_owned(),
        kind: VisionModelKind::Ocr,
        requests: 3,
        inferences: 2,
        lock_wait_total_us: 900,
        lock_wait_longest_us: 600,
        inference_total_us: 5_000,
        inference_longest_us: 3_000,
    }
}

impl VisionProvider for AccountedVision {
    fn require_ocr_model(
        &self,
        _model_ref: &str,
        _model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        Err(no_model())
    }

    fn require_nn_model(
        &self,
        _model_ref: &str,
        _model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        Err(no_model())
    }

    fn read_text(
        &self,
        _request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderResult, VisionProviderError> {
        Err(no_model())
    }

    fn classify(
        &self,
        _request: NnProviderRequest<'_>,
    ) -> Result<NnProviderResult, VisionProviderError> {
        Err(no_model())
    }

    fn loaded_models(&self) -> Vec<VisionModelMemoryStatus> {
        vec![loaded_model()]
    }

    fn model_timings(&self) -> Vec<VisionModelTimingStatus> {
        vec![model_timing()]
    }
}

fn start_host(root: &TempDir, sampler: Sampler) -> RuntimeHost {
    RuntimeHost::start(
        config(root)
            .with_frame_retention_enabled(false)
            .with_capture_memory_sample_for_test(FRAME_MEMORY)
            .with_process_memory_for_test(sampler),
        Arc::new(
            FakeProvider::one("node.a", instance_id(), Arc::new(FakeState::default()))
                .with_vision_provider(Arc::new(AccountedVision)),
        ),
    )
    .expect("runtime host")
}

/// The status read, completed and within its shape rules.
fn status(host: &RuntimeHost, ids: &IdentifierIssuer) -> RuntimePerformanceStatus {
    let receipt = host
        .process_request_for_test(
            &runtime_request(ids, RuntimeOperation::PerformanceStatus),
            ConnectionId::new(1).expect("connection"),
        )
        .expect("a receipt");
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    let Some(RuntimeResult::PerformanceStatus { status }) = receipt.result() else {
        panic!("a performance status: {receipt:?}");
    };
    status.validate().expect("the status keeps its shape rules");
    (**status).clone()
}

fn memory_of(status: &RuntimePerformanceStatus) -> RuntimeMemoryStatus {
    status
        .memory
        .clone()
        .expect("the status carries its memory")
}

/// The warnings `perf_memory_unavailable` of `status`.
fn memory_warnings(status: &RuntimePerformanceStatus) -> Vec<&Outcome> {
    status
        .warnings
        .iter()
        .filter(|warning| warning.code().as_str() == "perf_memory_unavailable")
        .collect()
}

/// One 4 x 4 frame with its PNG, retained resident (the store's budget is far above it).
fn add_frame(store: &mut FrameStore) {
    let mut frame = Frame::from_pixels(
        4,
        4,
        vec![7; 4 * 4 * 4],
        PixelFormat::Rgba8,
        CaptureBackendName::NemuIpc,
    )
    .expect("frame");
    frame.original_png = Some(frame.encode_png_fast().expect("the frame's PNG"));
    let outcome = store
        .add_frame(
            FrameStoreFrameInput {
                frame_index: 0,
                file_name: "frame0.png".to_owned(),
                label: "initial".to_owned(),
                recognition_state: RecognitionState::from_matched_page(None),
                pinned_reason: None,
                frame,
            },
            &mut |_candidate| panic!("a frame below every tier is not persisted here"),
        )
        .expect("the frame is admitted");
    assert!(outcome.retained, "{outcome:?}");
}

#[test]
fn perf_status_memory_parts_come_from_their_owners() {
    let root = TempDir::new().expect("tempdir");
    let host = start_host(&root, Arc::new(|| Ok(PROCESS)));
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let before = memory_of(&status(&host, &ids));

    // One more frame store, holding one frame, by its own accounting.
    let mut store = host
        .open_capture_frame_store_for_test(&root.path().join("frames"))
        .expect("a capture frame store");
    add_frame(&mut store);
    let store_bytes = store.diagnostics_json()["resident_bytes"]
        .as_u64()
        .expect("the store's resident bytes");
    assert!(store_bytes > 0, "the frame is resident");
    // One running run whose preparation measured its package.
    let run = *ids.mint_request_id().expect("request id").transport();
    host.track_running_package_for_test(run, instance_id(), PACKAGE_BYTES)
        .expect("a running run");

    let (capture_before, facts_before) = host
        .memory_owner_readings_for_test()
        .expect("the owners' readings");
    let read = status(&host, &ids);
    let (capture_after, facts_after) = host
        .memory_owner_readings_for_test()
        .expect("the owners' readings");
    host.untrack_running_package_for_test(run)
        .expect("the run ends");
    let memory = memory_of(&read);

    assert_eq!(
        memory.frame_stores,
        FrameStoresMemoryStatus {
            stores: before.frame_stores.stores + 1,
            resident_bytes: before.frame_stores.resident_bytes + store_bytes,
            resident_frames: before.frame_stores.resident_frames + 1,
            largest_resident_bytes: before.frame_stores.largest_resident_bytes.max(store_bytes),
        }
    );
    assert_eq!(memory.vision_models, vec![loaded_model()]);
    assert_eq!(read.vision, vec![model_timing()]);
    assert_eq!(
        memory.packages,
        PackagesMemoryStatus {
            runs: before.packages.runs + 1,
            resident_bytes: before.packages.resident_bytes + PACKAGE_BYTES,
        }
    );
    // The queue and the fact stores, as their owners read just before or just after.
    assert!(
        memory.ledger_capture == capture_before || memory.ledger_capture == capture_after,
        "{:?} not {capture_before:?} or {capture_after:?}",
        memory.ledger_capture
    );
    assert!(
        memory.facts == facts_before || memory.facts == facts_after,
        "{:?} not {facts_before:?} or {facts_after:?}",
        memory.facts
    );
    assert_eq!(
        memory.process,
        Some(ProcessMemoryStatus {
            private_bytes: PROCESS.private_bytes,
            working_set_bytes: PROCESS.working_set_bytes,
            peak_working_set_bytes: PROCESS.peak_working_set_bytes,
        })
    );
    assert!(memory_warnings(&read).is_empty(), "{:?}", read.warnings);

    // A dropped store leaves the registry.
    drop(store);
    let after = memory_of(&status(&host, &ids));
    assert_eq!(after.frame_stores, before.frame_stores);
    assert_eq!(after.packages, before.packages);
    host.close().expect("close host");
}

#[test]
fn a_failed_process_sample_is_absent_with_its_warning_not_zero() {
    let root = TempDir::new().expect("tempdir");
    let host = start_host(
        &root,
        Arc::new(|| Err(HostMetricsCode::PerformanceMemoryStatusFailed.code())),
    );
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let read = status(&host, &ids);
    host.close().expect("close host");

    let memory = memory_of(&read);
    assert_eq!(memory.process, None, "absent, not zero");
    let answer = serde_json::to_value(&read).expect("status json");
    assert!(
        answer["memory"].get("process").is_none(),
        "no process member at all: {answer}"
    );
    // The parts are shown all the same.
    assert_eq!(memory.vision_models, vec![loaded_model()]);
    let warnings = memory_warnings(&read);
    let [warning] = warnings.as_slice() else {
        panic!("one perf_memory_unavailable: {:?}", read.warnings);
    };
    let causes = warning
        .causes()
        .iter()
        .map(|link| link.outcome().code().as_str().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(causes, ["performance_memory_status_failed"]);
}

#[test]
fn perf_status_memory_parts_stay_within_private_bytes() {
    // The vision model's load alone claims 40 MiB; a process that holds less cannot be shown
    // beside it, and one that holds more is.
    let root = TempDir::new().expect("tempdir");
    let small = OwnProcessMemory {
        private_bytes: MODEL_LOAD_BYTES - 1,
        ..PROCESS
    };
    let host = start_host(&root, Arc::new(move || Ok(small)));
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let read = status(&host, &ids);
    host.close().expect("close host");

    let memory = memory_of(&read);
    let parts = memory.parts_bytes().expect("the parts' bytes");
    assert!(parts >= MODEL_LOAD_BYTES, "{memory:?}");
    assert_eq!(memory.process, None, "never beside parts above it");
    let warnings = memory_warnings(&read);
    let [warning] = warnings.as_slice() else {
        panic!("one perf_memory_unavailable: {:?}", read.warnings);
    };
    assert_eq!(
        warning
            .value("actual_bytes")
            .and_then(serde_json::Value::as_u64),
        Some(parts)
    );
    assert_eq!(
        warning
            .value("limit_bytes")
            .and_then(serde_json::Value::as_u64),
        Some(small.private_bytes)
    );

    // The shape rules refuse a status whose parts claim more than its process holds.
    let mut claimed = read.clone();
    claimed.memory = Some(RuntimeMemoryStatus {
        process: Some(ProcessMemoryStatus {
            private_bytes: small.private_bytes,
            working_set_bytes: small.working_set_bytes,
            peak_working_set_bytes: small.peak_working_set_bytes,
        }),
        ..memory
    });
    assert!(claimed.validate().is_err(), "the parts exceed the process");

    let root = TempDir::new().expect("tempdir");
    let host = start_host(&root, Arc::new(|| Ok(PROCESS)));
    let read = status(&host, &ids);
    host.close().expect("close host");
    let memory = memory_of(&read);
    let process = memory.process.expect("the process beside its parts");
    assert!(
        memory.parts_bytes().expect("the parts' bytes") <= process.private_bytes,
        "{memory:?}"
    );
    assert!(memory_warnings(&read).is_empty(), "{:?}", read.warnings);
}
