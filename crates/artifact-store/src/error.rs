// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{
    ArtifactFailureCause, ArtifactFailureRecord, ArtifactFailureStage, ArtifactId,
    LifecycleNativeDetail, MAX_ARTIFACT_FAILURE_SECONDARY_CAUSES,
};
use std::error::Error;
use std::fmt;

pub type ArtifactStoreResult<T> = Result<T, ArtifactStoreError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactStoreError {
    code: &'static str,
    operation: &'static str,
    detail: String,
    secondary: Vec<ArtifactFailureCause>,
    omitted_secondary_count: u64,
}

impl ArtifactStoreError {
    pub fn fatal(code: &'static str, operation: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            operation,
            detail: detail.into(),
            secondary: Vec::new(),
            omitted_secondary_count: 0,
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
        true
    }

    pub fn usage(detail: impl Into<String>) -> Self {
        Self::fatal("frame_store_usage", "frame_store", detail)
    }

    pub fn package_invalid(detail: impl Into<String>) -> Self {
        Self::fatal("frame_store_invalid", "frame_store", detail)
    }

    pub fn device(detail: impl Into<String>) -> Self {
        Self::fatal("frame_store_device", "frame_store", detail)
    }

    pub(crate) fn with_secondary(mut self, secondary: &Self) -> Self {
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

    fn primary_cause(&self) -> ArtifactFailureCause {
        let (text, truncated) = bounded_text(&self.detail, 1024);
        ArtifactFailureCause {
            code: self.code.to_owned(),
            operation: self.operation.to_owned(),
            native_detail: LifecycleNativeDetail::new(text, truncated),
        }
    }

    pub(crate) fn failure_record(
        &self,
        artifact_id: ArtifactId,
        stage: ArtifactFailureStage,
    ) -> ArtifactFailureRecord {
        ArtifactFailureRecord {
            artifact_id,
            stage,
            primary: self.primary_cause(),
            secondary: self.secondary.clone(),
            omitted_secondary_count: self.omitted_secondary_count,
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
            .map(|cause| format!("{} during {}", cause.code, cause.operation))
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
            "artifact store fatal {} during {}: {}",
            self.code, self.operation, self.detail
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
