// SPDX-License-Identifier: AGPL-3.0-only

use crate::codes::{StoreCode, StoreLocation};
use actingcommand_contract::codes::{ContractLocation, SharedCode};
use actingcommand_contract::outcome::{Code, Location, Outcome};
use actingcommand_contract::{
    ArtifactFailureCause, ArtifactFailureDraft, ArtifactFailureStage, ArtifactId,
    LifecycleNativeDetail, MAX_ARTIFACT_FAILURE_SECONDARY_CAUSES,
};
use std::error::Error;
use std::fmt;

pub type ArtifactStoreResult<T> = Result<T, ArtifactStoreError>;

/// A store failure. Every code and location the store raises is registered (`StoreCode`,
/// `StoreLocation`, shared contract entries) and built with [`ArtifactStoreError::new`]; the
/// error keeps their spellings because the runtime host also builds store errors with its own
/// codes ([`ArtifactStoreError::fatal`]) until slice A2b registers them (Workflow #378).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactStoreError {
    code: &'static str,
    operation: &'static str,
    detail: String,
    secondary: Vec<RetainedCause>,
    omitted_secondary_count: u64,
    fatal: bool,
    raw_os_error: Option<i32>,
    io_error_kind: Option<std::io::ErrorKind>,
    capacity: Option<Box<actingcommand_contract::CapacityDecision>>,
    unpublished_frame_layout: bool,
    // Boxed, as `capacity` is, so the error stays below clippy's `result_large_err` size.
    compared_bytes: Option<Box<ArtifactComparedBytes>>,
}

/// #411 A36b: the byte counts a stream's seal compared when the staged bytes it read back did
/// not match the bytes its writes reported (`artifact_hash_mismatch` at `seal_artifact_stream`).
/// The failure's outcome carries them as `expected_bytes` (the written stream) and
/// `actual_bytes` (the staged file).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArtifactComparedBytes {
    pub written_bytes: u64,
    pub staged_bytes: u64,
}

impl ArtifactStoreError {
    /// A fatal store failure with a registered code and location.
    pub fn new(code: Code, operation: Location, detail: impl Into<String>) -> Self {
        Self::fatal(code.as_str(), operation.as_str(), detail)
    }

    /// A fatal store failure named by the runtime host, which builds store errors with its own
    /// codes and operations (and relays other errors' codes) until slice A2b registers them.
    pub fn fatal(code: &'static str, operation: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            operation,
            detail: detail.into(),
            secondary: Vec::new(),
            omitted_secondary_count: 0,
            fatal: true,
            raw_os_error: None,
            io_error_kind: None,
            capacity: None,
            unpublished_frame_layout: false,
            compared_bytes: None,
        }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn operation(&self) -> &'static str {
        self.operation
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }

    pub const fn is_fatal(&self) -> bool {
        self.fatal
    }

    pub(crate) fn capacity_refused(decision: actingcommand_contract::CapacityDecision) -> Self {
        let mut error = Self::new(
            SharedCode::CapacityAdmissionRefused.code(),
            StoreLocation::AdmitArtifactBytes.location(),
            "committed capacity facts do not permit these new bytes",
        );
        error.fatal = false;
        error.capacity = Some(Box::new(decision));
        error
    }

    /// #411 A36b: a stream whose staged bytes do not match the bytes its writes reported, found
    /// when it is sealed, before anything is published. Only the work that owns the stream fails;
    /// the Runtime keeps running. A cleanup failure joined to it stays fatal.
    pub(crate) fn seal_mismatch(compared: ArtifactComparedBytes) -> Self {
        let mut error = Self::new(
            StoreCode::ArtifactHashMismatch.code(),
            StoreLocation::SealArtifactStream.location(),
            "staged bytes do not match the written stream",
        );
        error.fatal = false;
        error.compared_bytes = Some(Box::new(compared));
        error
    }

    /// #411 A36b: the byte counts a failed seal compared, when this is that failure.
    pub fn compared_bytes(&self) -> Option<ArtifactComparedBytes> {
        self.compared_bytes.as_deref().copied()
    }

    pub(crate) fn frame_workspace_refused() -> Self {
        let mut error = Self::new(
            SharedCode::FrameWorkspaceUnavailable.code(),
            StoreLocation::AdmitFrameWorkspace.location(),
            "the existing memory budget cannot reserve this frame's encoding workspace",
        );
        error.fatal = false;
        error
    }

    pub fn with_raw_os_error(mut self, code: Option<i32>) -> Self {
        self.raw_os_error = code;
        self
    }

    pub(crate) fn with_io_error(mut self, error: &std::io::Error) -> Self {
        self.raw_os_error = error.raw_os_error();
        self.io_error_kind = Some(error.kind());
        self
    }

    pub const fn io_error_kind(&self) -> Option<std::io::ErrorKind> {
        self.io_error_kind
    }

    /// Workflow #375 R5d: the same failure, without stopping the Runtime.
    pub(crate) fn non_fatal(mut self) -> Self {
        self.fatal = false;
        self
    }

    /// Workflow #375 R5d: the file or its use lock is held by someone else right now (os error
    /// 32 or 33, or a held lock), so the operation may succeed if tried again.
    pub fn is_busy(&self) -> bool {
        matches!(self.raw_os_error, Some(32 | 33))
            || self.io_error_kind == Some(std::io::ErrorKind::WouldBlock)
    }

    /// Workflow #375: the artifact's file could not be opened because it does not exist
    /// (`artifact_read_failed` with `NotFound`), such as a frame deleted by hand. A reader
    /// reports such material as missing instead of failing.
    pub fn is_material_missing(&self) -> bool {
        self.code == SharedCode::ArtifactReadFailed.as_str()
            && self.io_error_kind == Some(std::io::ErrorKind::NotFound)
    }

    pub(crate) fn read_budget_exceeded() -> Self {
        let mut error = Self::new(
            StoreCode::ArtifactReadBudgetExceeded.code(),
            ContractLocation::ReadProjectedArtifactRange.location(),
            "cooperative material read budget expired",
        );
        error.fatal = false;
        error
    }

    pub(crate) fn read_material_limit_exceeded() -> Self {
        let mut error = Self::new(
            StoreCode::ArtifactReadMaterialLimit.code(),
            StoreLocation::ReadProjectedArtifactComplete.location(),
            "complete material exceeds its explicit owned-buffer bound",
        );
        error.fatal = false;
        error
    }

    pub fn with_capacity(
        mut self,
        capacity: Option<actingcommand_contract::CapacityDecision>,
    ) -> Self {
        if self.capacity.is_none() {
            self.capacity = capacity.map(Box::new);
        }
        self
    }

    pub fn capacity(&self) -> Option<&actingcommand_contract::CapacityDecision> {
        self.capacity.as_deref()
    }

    pub const fn raw_os_error(&self) -> Option<i32> {
        self.raw_os_error
    }

    pub fn usage(detail: impl Into<String>) -> Self {
        Self::new(
            StoreCode::FrameStoreUsage.code(),
            StoreLocation::FrameStore.location(),
            detail,
        )
    }

    pub fn package_invalid(detail: impl Into<String>) -> Self {
        Self::new(
            StoreCode::FrameStoreInvalid.code(),
            StoreLocation::FrameStore.location(),
            detail,
        )
    }

    pub fn device(detail: impl Into<String>) -> Self {
        Self::new(
            StoreCode::FrameStoreDevice.code(),
            StoreLocation::FrameStore.location(),
            detail,
        )
    }

    #[cfg(feature = "capture")]
    pub fn incoming_frame(error: actingcommand_device::DeviceError) -> Self {
        if error.frame_memory_failure() == Some(actingcommand_device::FrameMemoryFailure::Capacity)
        {
            return Self::frame_workspace_refused();
        }
        let invalid_layout = error.diagnostic().is_some_and(|diagnostic| {
            diagnostic.category() == actingcommand_device::DeviceErrorCategory::FrameLayout
        });
        let mut result = Self::device(error.to_string());
        result.unpublished_frame_layout = invalid_layout;
        result
    }

    /// Only the incoming frame's layout owner sets this pre-publication category.
    /// A subsequent recording or cleanup failure retains the normal fatal route.
    pub fn is_unpublished_frame_layout(&self) -> bool {
        self.unpublished_frame_layout
            && self.secondary.is_empty()
            && self.omitted_secondary_count == 0
    }

    pub(crate) fn with_secondary(mut self, secondary: &Self) -> Self {
        // A real cleanup/ledger failure remains fatal even if admission was nonfatal.
        self.fatal |= secondary.fatal;
        for cause in
            std::iter::once(secondary.primary_cause()).chain(secondary.secondary.iter().cloned())
        {
            if self.secondary.len() < MAX_ARTIFACT_FAILURE_SECONDARY_CAUSES {
                self.secondary.push(cause);
            } else {
                self.omitted_secondary_count = self.omitted_secondary_count.saturating_add(1);
            }
        }
        self.omitted_secondary_count = self
            .omitted_secondary_count
            .saturating_add(secondary.omitted_secondary_count);
        self
    }

    fn primary_cause(&self) -> RetainedCause {
        let (text, truncated) = bounded_text(&self.detail, 1024);
        RetainedCause {
            code: self.code,
            operation: self.operation,
            native_detail: LifecycleNativeDetail::new(text, truncated),
            raw_os_error: self.raw_os_error,
        }
    }

    pub(crate) fn failure_draft(
        &self,
        artifact_id: ArtifactId,
        stage: ArtifactFailureStage,
    ) -> ArtifactFailureDraft {
        ArtifactFailureDraft {
            artifact_id,
            stage,
            primary: self.primary_cause().recorded(),
            secondary: self.secondary.iter().map(RetainedCause::recorded).collect(),
            omitted_secondary_count: self.omitted_secondary_count,
            capacity: self.capacity.as_deref().cloned(),
        }
    }

    /// Keep every retained cause's code and operation ahead of bounded native text.
    pub fn native_detail(&self) -> LifecycleNativeDetail {
        let primary = self.primary_cause();
        let causes = std::iter::once(&primary)
            .chain(&self.secondary)
            .collect::<Vec<_>>();
        let mut text = causes
            .iter()
            .map(|cause| {
                let mut identity = format!("{} during {}", cause.code, cause.operation);
                if let Some(code) = cause.raw_os_error {
                    identity.push_str(&format!(" (OS error {code})"));
                }
                identity
            })
            .collect::<Vec<_>>()
            .join("; secondary ");
        if self.omitted_secondary_count > 0 {
            text.push_str(&format!(
                "; omitted secondary {}",
                self.omitted_secondary_count
            ));
        }
        let budget = 1024usize.saturating_sub(text.len() + causes.len() * 3) / causes.len();
        let mut truncated = self.omitted_secondary_count > 0;
        for cause in causes {
            let (detail, clipped) = bounded_text(cause.native_detail.text(), budget);
            truncated |= clipped || cause.native_detail.truncated();
            text.push_str(" | ");
            text.push_str(detail);
        }
        let (text, clipped) = bounded_text(&text, 1024);
        LifecycleNativeDetail::new(text, truncated || clipped)
    }
}

/// One cause a store failure keeps, with the code and operation spellings it was raised with:
/// they stay strings while the runtime host relays its own codes (Workflow #378, slice A2b), so
/// the error's text names them as raised. The cause becomes typed only when the failure is
/// recorded ([`RetainedCause::recorded`]).
#[derive(Debug, Clone, PartialEq, Eq)]
struct RetainedCause {
    code: &'static str,
    operation: &'static str,
    native_detail: LifecycleNativeDetail,
    raw_os_error: Option<i32>,
}

impl RetainedCause {
    /// This cause as one step of a recorded artifact failure (ledger revision 3, Workflow #378
    /// A3): its registered code and location. A relayed code spelling the catalog does not list
    /// stays visible as `outcome_relay_unregistered` with the spelling as `raw_code`, as every
    /// relay does (acceptance CL-M2); an unlisted operation spelling falls back to the store's
    /// `store_artifact` location (every operation the store raises is registered). The code is
    /// never substituted and no text is made up.
    fn recorded(&self) -> ArtifactFailureCause {
        let step = match Code::relayed(self.code) {
            Some(code) => Outcome::new(code),
            None => Outcome::relayed(self.code),
        };
        let operation = Location::relayed(self.operation)
            .unwrap_or_else(|| StoreLocation::StoreArtifact.location());
        ArtifactFailureCause {
            step: step.operation(operation),
            native_detail: self.native_detail.clone(),
            raw_os_error: self.raw_os_error,
        }
    }
}

fn bounded_text(text: &str, limit: usize) -> (&str, bool) {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (&text[..end], end < text.len())
}

impl fmt::Display for ArtifactStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "artifact store {} {} during {}: {}",
            if self.fatal { "fatal" } else { "refusal" },
            self.code,
            self.operation,
            self.detail
        )?;
        for cause in &self.secondary {
            write!(
                formatter,
                "; secondary {} during {}: {}",
                cause.code,
                cause.operation,
                cause.native_detail.text()
            )?;
        }
        if self.omitted_secondary_count > 0 {
            write!(
                formatter,
                "; omitted secondary {}",
                self.omitted_secondary_count
            )?;
        }
        Ok(())
    }
}

impl Error for ArtifactStoreError {}
