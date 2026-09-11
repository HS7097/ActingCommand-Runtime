// SPDX-License-Identifier: AGPL-3.0-only

use crate::{
    Lab, LabContainedPackageValidationResponse, LabError as CliError, LabPorts,
    LabResult as CliOutcome, LabRunResolution, LabUnsupportedTargetResponse,
    LabValidateControlResponse, LabValidateRequest, LabValidateResourcesResponse,
    LabValidateResponse,
};
use actingcommand_artifact_store::FrameStoreControl;
use actingcommand_device::CaptureBackendChoice;
use actingcommand_execution_kernel::{
    ExternalExpectedSha256, ExternallyVerifiedBundle, page_anchor_matches,
};
use actingcommand_pack_containment::{
    Containment, ContainmentError, InstanceId, LoadedBundle, Sha256Hash,
};
use actingcommand_recognition_pack::{PackRect, RecognitionEvaluator, TargetEvaluation};
use actingcommand_resource_tooling::open_published_package;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const CONTROL_SCHEMA: &str = "Lab-1y.control.v1";
const DEFAULT_TEMPLATE_THRESHOLD: f32 = 0.9;
const DEFAULT_RECOVERY_TASK_ID: &str = "return_home";
const ROI_TEMPLATE_SCORE_EPSILON: f32 = 0.01;
const ROI_TEMPLATE_POSITION_EPSILON: i32 = 1;
const ROI_COLOR_DISTANCE_EPSILON: f32 = 2.0;
const ROI_COLOR_MEAN_EPSILON: u8 = 2;

include!("lab_run/api.rs");
include!("lab_run/execute.rs");
include!("lab_run/bundle.rs");

#[cfg(test)]
mod tests;
