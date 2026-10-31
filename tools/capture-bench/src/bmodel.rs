// SPDX-License-Identifier: AGPL-3.0-only

//! §5.9: `B_model`, the local pipeline cost model. Each N frame goes through the same
//! `CapturePipeline` the single-frame path uses (`crates/artifact-store/src/pipeline.rs`), into a
//! temporary root under the run directory, with a sink that sanitizes each event draft and
//! writes it to disk; the capture and recognition request and completion events the pipeline
//! does not write are added as lines of the same size class, so a frame costs the real number
//! of event records. It has none of the admission bookkeeping, resource registration, memory
//! accounting, GlobalLedger transaction, index, retention, leases or fences (the `model` label).

use crate::backend::{
    BackendFailure, FailureClass, MaterialCost, MaterialFactory, MaterialWriter, SampledFrame,
};
use actingcommand_artifact_store::{
    ArtifactCapacityAdmission, ArtifactEventSink, ArtifactStore, ArtifactStoreError,
    ArtifactStoreResult, ArtifactWriteContext, CapturePipeline, CapturePipelineConfig,
    FrameStoreConfig, FrameStoreFrameInput, MemorySample, MemorySampleSource, RecognitionState,
};
use actingcommand_contract::{
    ArtifactLinksDraft, ArtifactRedactionState, CapacityAdmissionOutcome, CapacityAdmissionReason,
    CapacityDecision, CapacityFactReference, CapturePolicyReason, EventDraft, EventLinksDraft,
    IdentifierIssuer, PinnedFrameReason, RetentionClass, SanitizationError, SecretField,
    SecretFingerprinter, Sha256Fingerprint,
};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

/// The capture and recognition events the single-frame path writes besides the pipeline's
/// (`observation.rs:374-391`, `:418-424`, `:644-664`): requested, backend opened, completed.
const EXTRA_EVENTS_PER_FRAME: u64 = 5;

struct Fingerprints;

impl SecretFingerprinter for Fingerprints {
    fn fingerprint(
        &self,
        _field: SecretField,
        original: &str,
    ) -> Result<Sha256Fingerprint, SanitizationError> {
        Sha256Fingerprint::new(
            format!("sha256:{}", crate::safety::sha256_hex(original.as_bytes())),
            original,
        )
    }
}

/// The on-disk event sink.
struct DiskSink {
    file: BufWriter<File>,
    events: u64,
}

impl ArtifactEventSink for DiskSink {
    fn append(&mut self, draft: EventDraft) -> ArtifactStoreResult<()> {
        let sanitized = draft.sanitize(&Fingerprints).map_err(|error| {
            ArtifactStoreError::fatal(
                "bench_event_sanitize_failed",
                "b_model_sink",
                error.to_string(),
            )
        })?;
        self.events += 1;
        writeln!(
            self.file,
            "{{\"event\":{},\"type\":\"{:?}\",\"artifacts\":{}}}",
            self.events,
            sanitized.event_type(),
            sanitized.artifacts().len()
        )
        .map_err(|error| {
            ArtifactStoreError::fatal(
                "bench_event_write_failed",
                "b_model_sink",
                error.to_string(),
            )
        })
    }
}

/// Admits writes under the B_model temporary root only.
struct RootAdmission {
    root: PathBuf,
    fact: OnceLock<CapacityFactReference>,
}

fn plain(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned())
}

impl ArtifactCapacityAdmission for RootAdmission {
    fn decide(&self, path: &Path, bytes: u64) -> ArtifactStoreResult<CapacityDecision> {
        let fact = match self.fact.get() {
            Some(fact) => fact.clone(),
            None => {
                let ids = IdentifierIssuer::new().map_err(|error| {
                    ArtifactStoreError::fatal(
                        "bench_identity_failed",
                        "b_model_admission",
                        error.to_string(),
                    )
                })?;
                let event = ids.mint_event_id().map_err(|error| {
                    ArtifactStoreError::fatal(
                        "bench_identity_failed",
                        "b_model_admission",
                        error.to_string(),
                    )
                })?;
                let owner = ids.mint_owner_epoch().map_err(|error| {
                    ArtifactStoreError::fatal(
                        "bench_identity_failed",
                        "b_model_admission",
                        error.to_string(),
                    )
                })?;
                let fact = CapacityFactReference {
                    event_id: *event.transport(),
                    sequence: 1,
                    owner_epoch: *owner.transport(),
                    observed_at_unix_ms: 1,
                    observed_at_monotonic_ms: 0,
                };
                let _ = self.fact.set(fact.clone());
                fact
            }
        };
        let inside = plain(path).starts_with(plain(&self.root));
        let (admission_result, reason) = if inside {
            (
                CapacityAdmissionOutcome::Allowed,
                CapacityAdmissionReason::FreshSample,
            )
        } else {
            (
                CapacityAdmissionOutcome::Unknown,
                CapacityAdmissionReason::BindingChanged,
            )
        };
        Ok(CapacityDecision {
            owner_epoch: fact.owner_epoch,
            decided_at_unix_ms: 1,
            decided_at_monotonic_ms: 0,
            requested_bytes: bytes,
            target_volume: Some("b-model-root".to_owned()),
            admission_result,
            reason,
            fact: Some(fact),
        })
    }
}

/// Opens one pipeline per segment under `<run>/bmodel/<segment>/`.
pub struct PipelineMaterials {
    pub root: PathBuf,
}

struct PipelineWriter {
    pipeline: CapturePipeline,
    sink: DiskSink,
    ids: IdentifierIssuer,
    next_frame: usize,
}

fn context(ids: &IdentifierIssuer, sequence: u64) -> Result<ArtifactWriteContext, String> {
    let run = ids.mint_run_id().map_err(|error| error.to_string())?;
    let frame = ids.mint_frame_id().map_err(|error| error.to_string())?;
    let correlation = ids
        .mint_correlation_id()
        .map_err(|error| error.to_string())?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1, |since| since.as_millis() as u64);
    Ok(ArtifactWriteContext::new(
        ArtifactLinksDraft::default()
            .with_run_id(run)
            .with_frame_id(frame)
            .with_correlation_id(correlation),
        EventLinksDraft::default()
            .with_run_id(run)
            .with_frame_id(frame)
            .with_correlation_id(correlation),
        now + sequence,
    ))
}

impl MaterialFactory for PipelineMaterials {
    fn open(&self, instance: &str, segment: &str) -> Result<Box<dyn MaterialWriter>, String> {
        let root = self.root.join(format!("{instance}-{segment}"));
        std::fs::create_dir_all(&root)
            .map_err(|error| format!("create {}: {error}", root.display()))?;
        let store = Arc::new(
            ArtifactStore::open(root.join("artifacts")).map_err(|error| error.to_string())?,
        );
        store
            .install_capacity_admission(Arc::new(RootAdmission {
                root: store.root().to_path_buf(),
                fact: OnceLock::new(),
            }))
            .map_err(|error| error.to_string())?;
        let events = OpenOptions::new()
            .create(true)
            .append(true)
            .open(root.join("events.jsonl"))
            .map_err(|error| format!("open the B_model sink: {error}"))?;
        let mut sink = DiskSink {
            file: BufWriter::new(events),
            events: 0,
        };
        let hard = 512 * 1024 * 1024;
        let mut frame_store = FrameStoreConfig::default();
        frame_store.max_mem_bytes = Some(hard);
        frame_store.os_reserve_bytes = 0;
        let frame_store = frame_store.with_memory_source(MemorySampleSource::fixed(MemorySample {
            total_bytes: hard,
            available_bytes: hard,
        }));
        let config = CapturePipelineConfig {
            frame_store,
            cadence_ms: 200,
            retention_class: RetentionClass::DebugFull,
            policy_reason: CapturePolicyReason::Default,
            redaction_state: ArtifactRedactionState::NotRequired,
        };
        let ids = IdentifierIssuer::new().map_err(|error| error.to_string())?;
        let run_context = context(&ids, 0)?;
        let pipeline =
            CapturePipeline::open(store, root.join("frames"), config, run_context, &mut sink)
                .map_err(|error| error.to_string())?;
        Ok(Box::new(PipelineWriter {
            pipeline,
            sink,
            ids,
            next_frame: 1,
        }))
    }
}

impl MaterialWriter for PipelineWriter {
    fn write(&mut self, frame: &SampledFrame) -> Result<MaterialCost, BackendFailure> {
        let failure = |detail: String| {
            BackendFailure::new("b_model_material_failed", FailureClass::CellPath, detail)
                .without_native_call()
        };
        let device_frame = Frame::from_pixels(
            frame.width,
            frame.height,
            frame.pixels.clone(),
            match frame.layout {
                crate::backend::PixelLayout::Rgba8 => PixelFormat::Rgba8,
                crate::backend::PixelLayout::Rgb8 => PixelFormat::Rgb8,
            },
            CaptureBackendName::NemuIpc,
        )
        .map_err(|error| failure(error.message().to_owned()))?;
        let index = self.next_frame;
        self.next_frame += 1;
        let events_before = self.sink.events;
        let context = context(&self.ids, index as u64).map_err(failure)?;
        let outcome = self
            .pipeline
            .record_frame(
                FrameStoreFrameInput {
                    frame_index: index,
                    file_name: format!("frame-{index}.png"),
                    label: "b_model".to_owned(),
                    recognition_state: RecognitionState::Matched {
                        page_id: "bench/model".to_owned(),
                    },
                    pinned_reason: Some(PinnedFrameReason::RecognitionEvidence),
                    frame: device_frame,
                },
                context,
                &mut self.sink,
            )
            .map_err(|error| failure(error.to_string()))?;
        for extra in 0..EXTRA_EVENTS_PER_FRAME {
            self.sink.events += 1;
            writeln!(
                self.sink.file,
                "{{\"event\":{},\"type\":\"model_request_or_completion\",\"frame\":{index},\"slot\":{extra}}}",
                self.sink.events
            )
            .map_err(|error| failure(error.to_string()))?;
        }
        self.sink
            .file
            .flush()
            .map_err(|error| failure(error.to_string()))?;
        let bytes = outcome
            .persisted
            .iter()
            .map(|artifact| artifact.byte_count())
            .sum::<u64>();
        Ok(MaterialCost {
            bytes,
            events: self.sink.events - events_before,
        })
    }
}
