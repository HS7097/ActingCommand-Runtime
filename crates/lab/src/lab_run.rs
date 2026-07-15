// SPDX-License-Identifier: AGPL-3.0-only

use crate::{
    CaptureBackendFactory, CaptureBackendObservation, CaptureBackendRequest, Clock, ConfigSource,
    InputBackendFactory, InputBackendRequest, Lab, LabError as CliError, LabPorts,
    LabResult as CliOutcome, LabRunLedgerResponse, LabRunRequest, LabRunResolution, LabRunResponse,
    LabUnsupportedTargetResponse, LabValidateControlResponse, LabValidateRequest,
    LabValidateResourcesResponse, LabValidateResponse, LedgerSink,
};
use actingcommand_artifact_store::{
    ArtifactStoreError, FrameStore, FrameStoreConfig, FrameStoreControl, FrameStoreFrameInput,
    FrameStoreScreenshot as ScreenshotRecord, PortableFrameEvidenceProjection,
    PortableProjectionArchive, RecognitionState, ScreenshotNameAllocator, Tier3PauseCheckpoint,
    write_portable_projection_archive,
};
use actingcommand_device::{
    CaptureBackend, CaptureBackendAttempt, CaptureBackendChoice, CaptureBackendName, Frame,
    InputBackend, PixelFormat, TouchBackendConfig, combine_operation_and_close,
};
use actingcommand_execution_kernel::{
    ExternalExpectedSha256, ExternallyVerifiedBundle, RunDecisionError, RunDirective,
    RunFailureObservation, RunFailureStage, RunOperationCandidate, RunOperationFailureDecision,
    RunOperationPolicy, RunRecoveryTrigger, RunStateConfig, RunStateMachine, RunTerminal,
    canonical_page_anchor, decide_run_operation_failure, page_anchor_matches,
};
use actingcommand_ledger::{
    CommitProof, EvidenceStore, IdIssuer, IdKind, LastResortError, LedgerRecord, LedgerRecordKind,
    LightEvent, SessionHeader, commit_then_record,
};
#[cfg(test)]
use actingcommand_ledger::{LabLedger, LabLogError};
use actingcommand_pack_containment::{
    Containment, ContainmentError, InstanceId, LoadedBundle, Sha256Hash,
};
use actingcommand_page_detector::{PageDetector, PageEvaluation, PageTargetRole};
use actingcommand_recognition::{Scene, ScenePixelFormat};
use actingcommand_recognition_pack::{
    PackRect, RecognitionEvaluator, TargetEvaluation, TargetKind, UnsupportedRecognitionTarget,
};
use actingcommand_resource_tooling::open_published_package;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
#[cfg(test)]
use zip::ZipWriter;
#[cfg(test)]
use zip::write::FileOptions;

const CONTROL_SCHEMA: &str = "Lab-1y.control.v1";
const SUMMARY_SCHEMA: &str = "Lab-1y.summary.v1";
const DEFAULT_CAPTURE_INTERVAL_MS: u64 = 300;
const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const DEFAULT_STEP_TIMEOUT_MS: u64 = 10_000;
const DEFAULT_MAX_STEPS: usize = 50;
const DEFAULT_TEMPLATE_THRESHOLD: f32 = 0.9;
const DEFAULT_RETRY_INTERVAL_MS: u64 = 1_500;
const DEFAULT_POST_WAIT_FREEZES_MS: u64 = 480;
const DEFAULT_RECOVERY_TASK_ID: &str = "return_home";
const DEFAULT_ROI_STABLE_FRAMES: u32 = 2;
const DEFAULT_ROI_STABILITY_TIMEOUT_MS: u64 = 1_500;
const DEFAULT_RESOURCE_DRIFT_FRAMES: u32 = 2;
const ROI_TEMPLATE_SCORE_EPSILON: f32 = 0.01;
const ROI_TEMPLATE_POSITION_EPSILON: i32 = 1;
const ROI_COLOR_DISTANCE_EPSILON: f32 = 2.0;
const ROI_COLOR_MEAN_EPSILON: u8 = 2;

fn map_artifact_error(error: ArtifactStoreError) -> CliError {
    match error.code() {
        "frame_store_usage" => CliError::usage(error.detail()),
        "frame_store_device" => CliError::device(error.detail()),
        _ => CliError::package_invalid(error.detail()),
    }
}

include!("lab_run/api.rs");
include!("lab_run/execute.rs");
include!("lab_run/bundle.rs");
include!("lab_run/context.rs");
include!("lab_run/output.rs");
include!("lab_run/test_support.rs");

#[cfg(test)]
mod tests;
