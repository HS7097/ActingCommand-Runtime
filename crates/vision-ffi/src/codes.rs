// SPDX-License-Identifier: AGPL-3.0-only

//! vision-ffi's outcome codes (Workflow #378). A2a registers the three refusals of a vision
//! root's model listing here, ahead of A2d (decision D10): actingd's startup listing and its
//! `check-config`, and the vision-provider-check tool, refuse a models root with them. The
//! crate's closed `VisionFfiErrorCode` registers nothing: it stays inside the process and the
//! execution kernel maps it onto the recognition pack's `VisionProviderErrorCode` (A2d). Every
//! entry is listed in the fragment `contracts/outcome-codes/vision-ffi.json`; the outcome guard
//! (G1) checks both directions.
//!
//! Workflow #407 GPU-1 A adds the GPU warning `vision_gpu_unavailable` with its two
//! vocabularies (design `rd-1031/perf-407/design-gpu1.md` §2.5): one domain code whose values
//! say why and what the process does instead.

use serde::{Deserialize, Serialize};

actingcommand_contract::outcome_codes! {
    /// Why the model listing of a vision root cannot be used.
    pub enum VisionListingCode {
        VisionRootUnavailable => "vision_root_unavailable": error / none,
        VisionModelsEmpty => "vision_models_empty": error / none,
        VisionModelFolderInvalid => "vision_model_folder_invalid": error / none,
    }
}

actingcommand_contract::outcome_codes! {
    /// Workflow #407 GPU-1: the configured GPU execution provider cannot be used.
    pub enum VisionGpuCode {
        /// The configured GPU execution provider cannot be used (value `gpu_unavailable_reason`):
        /// under `gpu_fallback` `cpu` the work goes on on the CPU, under `refuse` OCR requests
        /// fail.
        VisionGpuUnavailable => "vision_gpu_unavailable": warning / none,
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// Why the configured GPU execution provider cannot be used (vocabulary
    /// `gpu_unavailable_reason`, Workflow #407 GPU-1 design §1.3, §2.3 and §2.5).
    pub enum GpuUnavailableReason: "gpu_unavailable_reason" {
        /// No DXGI adapter has the configured identity, or the adapters cannot be listed.
        AdapterMissing => "adapter_missing",
        /// More than one physical adapter has the configured identity.
        AdapterAmbiguous => "adapter_ambiguous",
        /// Adapters have the configured identity, but none of them is a physical adapter
        /// (each is an indirect-display, software or paravirtualized copy, or its kernel type
        /// could not be read).
        AdapterNotPhysical => "adapter_not_physical",
        /// The vision root's `ort\DirectML.dll` is missing.
        DirectmlLibraryMissing => "directml_library_missing",
        /// The vision root's `ort\DirectML.dll` differs from the runtime pin.
        DirectmlLibraryMismatch => "directml_library_mismatch",
        /// ONNX Runtime refused to register the DirectML execution provider (no driver, no
        /// Direct3D 12 device, not enough memory).
        ProviderRegisterFailed => "provider_register_failed",
        /// ONNX Runtime could not build a session on the DirectML execution provider.
        SessionBuildFailed => "session_build_failed",
        /// The node placement of a DirectML session could not be read in full.
        PlacementUnread => "placement_unread",
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// What the process does when the configured GPU execution provider cannot be used
    /// (vocabulary `gpu_fallback`, Workflow #407 GPU-1 design §1.2 and §2.3); it is also the
    /// configuration value `vision.gpu_fallback`, `cpu` when absent.
    #[derive(Default, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum GpuFallback: "gpu_fallback" {
        /// The model runs on the CPU, with one warning.
        #[default]
        Cpu => "cpu",
        /// OCR requests fail as they did before the fallback existed.
        Refuse => "refuse",
    }
}
