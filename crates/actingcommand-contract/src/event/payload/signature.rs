// SPDX-License-Identifier: AGPL-3.0-only

use super::{is_sha256, validate_diagnostic_detail_stage, validate_diagnostic_detail_token};
use crate::{
    DiagnosticCode, EventId, EventSeverity, EventType, LifecycleFailurePhase, OriginModule,
    ResourceQuiescence, RuntimeResourceClosePhase, RuntimeResourceKind, SanitizationError,
};
use serde::{Deserialize, Serialize};

pub const MAX_SIGNATURE_CATALOG_ENTRIES: usize = 64;
pub const MAX_SIGNATURE_PREFIX_EVENTS: usize = 16_384;
pub const MAX_SIGNATURE_PREFIX_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_SIGNATURE_PAGE_ROWS: u16 = 64;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignatureLifecyclePredicate {
    pub stage: Option<String>,
    pub operation: Option<String>,
    pub code: Option<String>,
    pub cause_phase: Option<LifecycleFailurePhase>,
    pub cause_source: Option<String>,
    pub resource: Option<RuntimeResourceKind>,
    pub resource_phase: Option<RuntimeResourceClosePhase>,
    pub quiescence: Option<ResourceQuiescence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticSignatureDefinition {
    pub signature_id: String,
    pub version: u32,
    pub origin_module: OriginModule,
    pub diagnostic_code: DiagnosticCode,
    pub event_type: EventType,
    pub minimum_severity: EventSeverity,
    pub lifecycle: Option<SignatureLifecyclePredicate>,
}

impl DiagnosticSignatureDefinition {
    pub fn validate(&self) -> Result<(), SanitizationError> {
        validate_diagnostic_detail_token(&self.signature_id, "signature_id")?;
        if self.version == 0 {
            return Err(invalid("signature_version"));
        }
        if let Some(predicate) = &self.lifecycle {
            if *predicate == SignatureLifecyclePredicate::default() {
                return Err(invalid("signature_lifecycle"));
            }
            if let Some(stage) = &predicate.stage {
                validate_diagnostic_detail_stage(stage)?;
            }
            for value in [
                &predicate.operation,
                &predicate.code,
                &predicate.cause_source,
            ]
            .into_iter()
            .flatten()
            {
                validate_diagnostic_detail_token(value, "signature_condition")?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignatureRegistrationRef {
    pub signature_id: String,
    pub version: u32,
    pub event_id: EventId,
    pub sequence: u64,
}

impl SignatureRegistrationRef {
    pub fn validate(&self) -> Result<(), SanitizationError> {
        validate_diagnostic_detail_token(&self.signature_id, "signature_id")?;
        if self.version == 0 || self.sequence == 0 {
            return Err(invalid("signature_registration"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignaturePrefixIdentity {
    pub through_sequence: u64,
    pub observed_through_sequence: u64,
    pub event_count: usize,
    pub sha256: String,
    pub complete: bool,
}

impl SignaturePrefixIdentity {
    pub fn validate(&self) -> Result<(), SanitizationError> {
        if self.through_sequence == 0
            || self.observed_through_sequence > self.through_sequence
            || self.event_count > MAX_SIGNATURE_PREFIX_EVENTS
            || self.event_count as u64 > self.observed_through_sequence
            || (self.event_count == 0) != (self.observed_through_sequence == 0)
            || !is_sha256(&self.sha256)
            || self.complete
                && (self.observed_through_sequence != self.through_sequence
                    || self.event_count as u64 != self.through_sequence)
        {
            return Err(invalid("signature_prefix"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignatureReplayCursor {
    pub input: SignaturePrefixIdentity,
    pub catalog: SignaturePrefixIdentity,
    pub row_offset: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignaturePageRequest {
    pub limit: u16,
    pub cursor: Option<SignatureReplayCursor>,
}

impl Default for SignaturePageRequest {
    fn default() -> Self {
        Self {
            limit: MAX_SIGNATURE_PAGE_ROWS,
            cursor: None,
        }
    }
}

impl SignaturePageRequest {
    pub fn validate(&self) -> Result<(), SanitizationError> {
        if self.limit == 0 || self.limit > MAX_SIGNATURE_PAGE_ROWS {
            return Err(invalid("signature_page_limit"));
        }
        if let Some(cursor) = &self.cursor {
            cursor.input.validate()?;
            cursor.catalog.validate()?;
            if cursor.row_offset
                > (MAX_SIGNATURE_PREFIX_EVENTS * MAX_SIGNATURE_CATALOG_ENTRIES) as u64
            {
                return Err(invalid("signature_cursor"));
            }
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeSignatureMatchRequest {
    pub input_state_root: String,
    pub input_through: u64,
    pub catalog_through: u64,
    pub page: SignaturePageRequest,
}

impl std::fmt::Debug for RuntimeSignatureMatchRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeSignatureMatchRequest")
            .field("input_state_root", &"<redacted-root>")
            .field("input_through", &self.input_through)
            .field("catalog_through", &self.catalog_through)
            .field("page", &self.page)
            .finish()
    }
}

impl RuntimeSignatureMatchRequest {
    pub fn validate(&self) -> Result<(), SanitizationError> {
        if self.input_state_root.is_empty()
            || self.input_state_root.len() > 4096
            || self.input_state_root.chars().any(char::is_control)
            || self.input_through == 0
            || self.catalog_through == 0
        {
            return Err(invalid("signature_match_request"));
        }
        self.page.validate()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignatureConditionField {
    Lifecycle,
    Operation,
    Cause,
    Resource,
    ResourcePhase,
    Quiescence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignatureReplayGap {
    InputIncomplete,
    CatalogIncomplete,
    CatalogEmpty,
    CatalogTransitionInvalid,
    CatalogLimitExceeded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SignatureReplayRow {
    Matched {
        registration: SignatureRegistrationRef,
        source_event_id: EventId,
        source_sequence: u64,
    },
    MissingFields {
        registration: SignatureRegistrationRef,
        source_event_id: EventId,
        source_sequence: u64,
        fields: Vec<SignatureConditionField>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignatureReplayPage {
    pub input: SignaturePrefixIdentity,
    pub catalog: SignaturePrefixIdentity,
    pub active_signatures: usize,
    pub matched_count: u64,
    pub missing_fields_count: u64,
    pub gaps: Vec<SignatureReplayGap>,
    pub row_offset: u64,
    pub rows: Vec<SignatureReplayRow>,
    pub next_cursor: Option<SignatureReplayCursor>,
}

impl SignatureReplayPage {
    pub fn evidence_complete(&self) -> bool {
        self.input.complete
            && self.catalog.complete
            && self.gaps.is_empty()
            && self.missing_fields_count == 0
    }

    pub fn validate(&self) -> Result<(), SanitizationError> {
        self.input.validate()?;
        self.catalog.validate()?;
        if self.active_signatures > MAX_SIGNATURE_CATALOG_ENTRIES {
            return Err(invalid("signature_catalog_size"));
        }
        let maximum = (self.input.event_count * self.active_signatures) as u64;
        let total = self
            .matched_count
            .checked_add(self.missing_fields_count)
            .filter(|value| *value <= maximum)
            .ok_or_else(|| invalid("signature_counts"))?;
        if self.rows.len() > usize::from(MAX_SIGNATURE_PAGE_ROWS)
            || self.gaps.len() > 5
            || self
                .gaps
                .iter()
                .enumerate()
                .any(|(index, gap)| self.gaps[..index].contains(gap))
            || self.gaps.contains(&SignatureReplayGap::InputIncomplete) == self.input.complete
            || self.gaps.contains(&SignatureReplayGap::CatalogIncomplete) == self.catalog.complete
            || self.gaps.contains(&SignatureReplayGap::CatalogEmpty)
                != (self.active_signatures == 0)
            || self.row_offset > total
            || self.row_offset + self.rows.len() as u64 > total
        {
            return Err(invalid("signature_page"));
        }
        let mut matched_rows = 0;
        let mut previous_key = None;
        for row in &self.rows {
            let (registration, sequence) = match row {
                SignatureReplayRow::Matched {
                    registration,
                    source_sequence,
                    ..
                } => {
                    matched_rows += 1;
                    (registration, *source_sequence)
                }
                SignatureReplayRow::MissingFields {
                    registration,
                    source_sequence,
                    fields,
                    ..
                } => {
                    if fields.is_empty()
                        || fields.len() > 6
                        || fields
                            .iter()
                            .enumerate()
                            .any(|(index, field)| fields[..index].contains(field))
                    {
                        return Err(invalid("signature_missing_fields"));
                    }
                    (registration, *source_sequence)
                }
            };
            registration.validate()?;
            let key = (sequence, registration.signature_id.as_str());
            if sequence == 0
                || sequence > self.input.observed_through_sequence
                || registration.sequence > self.catalog.observed_through_sequence
                || previous_key.is_some_and(|previous| previous >= key)
            {
                return Err(invalid("signature_source_identity"));
            }
            previous_key = Some(key);
        }
        if matched_rows > self.matched_count
            || self.rows.len() as u64 - matched_rows > self.missing_fields_count
        {
            return Err(invalid("signature_row_counts"));
        }
        match &self.next_cursor {
            Some(cursor)
                if cursor.input == self.input
                    && cursor.catalog == self.catalog
                    && cursor.row_offset == self.row_offset + self.rows.len() as u64
                    && cursor.row_offset < total
                    && !self.rows.is_empty() => {}
            None if self.row_offset + self.rows.len() as u64 == total => {}
            _ => return Err(invalid("signature_next_cursor")),
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LedgerSignatureEvent {
    Registered {
        definition: DiagnosticSignatureDefinition,
    },
    Matched {
        page: Box<SignatureReplayPage>,
    },
    Retired {
        registration: SignatureRegistrationRef,
    },
}

impl LedgerSignatureEvent {
    pub fn validate(&self) -> Result<(), SanitizationError> {
        match self {
            Self::Registered { definition } => definition.validate(),
            Self::Matched { page } => page.validate(),
            Self::Retired { registration } => registration.validate(),
        }
    }
    pub const fn event_type(&self) -> EventType {
        match self {
            Self::Registered { .. } => EventType::SignatureRegistered,
            Self::Matched { .. } => EventType::SignatureMatched,
            Self::Retired { .. } => EventType::SignatureRetired,
        }
    }
}

fn invalid(field: &'static str) -> SanitizationError {
    SanitizationError::new("invalid_diagnostic_signature", field)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerSignaturePayload {
    pub(super) record: LedgerSignatureEvent,
    pub(super) audit: super::SanitizedAudit,
}

impl LedgerSignaturePayload {
    pub fn record(&self) -> &LedgerSignatureEvent {
        &self.record
    }
}

impl super::PayloadDetail for LedgerSignaturePayload {
    fn action(&self) -> crate::EventAction {
        match self.record {
            LedgerSignatureEvent::Registered { .. } => crate::EventAction::SignatureRegister,
            LedgerSignatureEvent::Matched { .. } => crate::EventAction::SignatureMatch,
            LedgerSignatureEvent::Retired { .. } => crate::EventAction::SignatureRetire,
        }
    }
    fn diagnostic_code(&self) -> Option<DiagnosticCode> {
        None
    }
    fn effect_disposition(&self) -> Option<crate::EffectDisposition> {
        None
    }
    fn audit(&self) -> &super::SanitizedAudit {
        &self.audit
    }
}
