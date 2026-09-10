// SPDX-License-Identifier: AGPL-3.0-only

use crate::{
    GlobalLedger, GlobalLedgerError, GlobalLedgerReadOnly, GlobalLedgerResult, PersistedEvent,
};
use actingcommand_contract::{
    DiagnosticSignatureDefinition, EventActor, EventPayload, EventQuery, EventSource,
    LedgerPayload, LedgerSignatureEvent, MAX_SIGNATURE_CATALOG_ENTRIES, MAX_SIGNATURE_PAGE_ROWS,
    MAX_SIGNATURE_PREFIX_BYTES, MAX_SIGNATURE_PREFIX_EVENTS, OriginModule, RuntimePayload,
    SignatureConditionField, SignaturePageRequest, SignaturePrefixIdentity,
    SignatureRegistrationRef, SignatureReplayCursor, SignatureReplayGap, SignatureReplayPage,
    SignatureReplayRow,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub struct SignaturePrefix {
    identity: SignaturePrefixIdentity,
    events: Vec<PersistedEvent>,
}

impl SignaturePrefix {
    pub fn from_live(ledger: &GlobalLedger, through: u64) -> GlobalLedgerResult<Self> {
        let mut prefix = PrefixBuilder::new(through)?;
        let mut after = 0;
        while after < through {
            let page = ledger.query_page(
                EventQuery::default(),
                after,
                through,
                usize::from(MAX_SIGNATURE_PAGE_ROWS),
            )?;
            if page.is_empty() {
                break;
            }
            for event in page {
                after = event.sequence();
                prefix.push(event)?;
            }
        }
        Ok(prefix.finish(true))
    }

    pub fn from_read_only(
        snapshot: &GlobalLedgerReadOnly,
        through: u64,
    ) -> GlobalLedgerResult<Self> {
        let mut prefix = PrefixBuilder::new(through)?;
        for event in snapshot
            .events()
            .iter()
            .take_while(|event| event.sequence() <= through)
        {
            prefix.push(event.clone())?;
        }
        Ok(prefix
            .finish(snapshot.storage_snapshot().read_complete && snapshot.corrupt_tail().is_none()))
    }

    pub fn identity(&self) -> &SignaturePrefixIdentity {
        &self.identity
    }

    pub fn from_evidence(
        snapshot: &crate::GlobalLedgerEvidence,
        through: u64,
    ) -> GlobalLedgerResult<Self> {
        let mut prefix = PrefixBuilder::new(through)?;
        for event in snapshot
            .events()
            .iter()
            .take_while(|event| event.sequence() <= through)
        {
            prefix.push(event.clone())?;
        }
        Ok(prefix.finish(snapshot.is_complete()))
    }
}

struct PrefixBuilder {
    through: u64,
    events: Vec<PersistedEvent>,
    hasher: Sha256,
    bytes: usize,
    contiguous: bool,
}

impl PrefixBuilder {
    fn new(through: u64) -> GlobalLedgerResult<Self> {
        if through == 0 {
            return Err(error("signature_through_zero"));
        }
        let mut hasher = Sha256::new();
        hasher.update(b"actingcommand.signature-prefix.v1\n");
        Ok(Self {
            through,
            events: Vec::new(),
            hasher,
            bytes: 0,
            contiguous: true,
        })
    }
    fn push(&mut self, event: PersistedEvent) -> GlobalLedgerResult<()> {
        if self.events.len() == MAX_SIGNATURE_PREFIX_EVENTS {
            return Err(error("signature_prefix_event_limit"));
        }
        let bytes =
            serde_json::to_vec(&event).map_err(|_| error("signature_prefix_encoding_failed"))?;
        self.bytes = self
            .bytes
            .checked_add(bytes.len() + 1)
            .filter(|bytes| *bytes <= MAX_SIGNATURE_PREFIX_BYTES)
            .ok_or_else(|| error("signature_prefix_byte_limit"))?;
        self.contiguous &= event.sequence() == self.events.len() as u64 + 1;
        self.hasher.update(&bytes);
        self.hasher.update(b"\n");
        self.events.push(event);
        Ok(())
    }
    fn finish(self, read_complete: bool) -> SignaturePrefix {
        let observed = self.events.last().map_or(0, PersistedEvent::sequence);
        SignaturePrefix {
            identity: SignaturePrefixIdentity {
                through_sequence: self.through,
                observed_through_sequence: observed,
                event_count: self.events.len(),
                sha256: format!("sha256:{:x}", self.hasher.finalize()),
                complete: read_complete && self.contiguous && observed == self.through,
            },
            events: self.events,
        }
    }
}

struct CatalogEntry {
    definition: DiagnosticSignatureDefinition,
    registration: SignatureRegistrationRef,
    retired: bool,
}

pub struct SignatureCatalog {
    identity: SignaturePrefixIdentity,
    entries: BTreeMap<String, CatalogEntry>,
    gaps: Vec<SignatureReplayGap>,
}

impl SignatureCatalog {
    pub fn from_prefix(prefix: &SignaturePrefix) -> Self {
        let mut catalog = Self {
            identity: prefix.identity.clone(),
            entries: BTreeMap::new(),
            gaps: Vec::new(),
        };
        if !prefix.identity.complete {
            catalog.gap(SignatureReplayGap::CatalogIncomplete);
        }
        for event in &prefix.events {
            let EventPayload::Ledger(LedgerPayload::Signature(payload)) = event.payload() else {
                continue;
            };
            if matches!(payload.record(), LedgerSignatureEvent::Matched { .. }) {
                continue;
            }
            if event.origin().source() != EventSource::Lab
                || event.origin().actor() != EventActor::Lab
                || event.origin().module() != OriginModule::GlobalLedger
            {
                catalog.gap(SignatureReplayGap::CatalogTransitionInvalid);
                continue;
            }
            match payload.record() {
                LedgerSignatureEvent::Registered { definition } => {
                    let valid_version = match catalog.entries.get(&definition.signature_id) {
                        None => definition.version == 1,
                        Some(previous) => {
                            previous.retired
                                && previous.definition.version.checked_add(1)
                                    == Some(definition.version)
                        }
                    };
                    if !valid_version || definition.validate().is_err() {
                        catalog.gap(SignatureReplayGap::CatalogTransitionInvalid);
                        continue;
                    }
                    if !catalog.entries.contains_key(&definition.signature_id)
                        && catalog.entries.len() == MAX_SIGNATURE_CATALOG_ENTRIES
                    {
                        catalog.gap(SignatureReplayGap::CatalogLimitExceeded);
                        continue;
                    }
                    catalog.entries.insert(
                        definition.signature_id.clone(),
                        CatalogEntry {
                            definition: definition.clone(),
                            registration: registration_ref(event, definition),
                            retired: false,
                        },
                    );
                }
                LedgerSignatureEvent::Retired { registration } => {
                    match catalog.entries.get_mut(&registration.signature_id) {
                        Some(entry) if !entry.retired && entry.registration == *registration => {
                            entry.retired = true
                        }
                        _ => catalog.gap(SignatureReplayGap::CatalogTransitionInvalid),
                    }
                }
                LedgerSignatureEvent::Matched { .. } => {}
            }
        }
        catalog
    }

    fn gap(&mut self, gap: SignatureReplayGap) {
        if !self.gaps.contains(&gap) {
            self.gaps.push(gap);
        }
    }

    pub fn validate_registration(
        &self,
        definition: &DiagnosticSignatureDefinition,
    ) -> GlobalLedgerResult<()> {
        definition
            .validate()
            .map_err(|_| error("signature_definition_invalid"))?;
        if !self.gaps.is_empty() {
            return Err(error("signature_catalog_incomplete"));
        }
        match self.entries.get(&definition.signature_id) {
            Some(previous)
                if previous.retired
                    && previous.definition.version.checked_add(1) == Some(definition.version) =>
            {
                Ok(())
            }
            None if definition.version == 1
                && self.entries.len() < MAX_SIGNATURE_CATALOG_ENTRIES =>
            {
                Ok(())
            }
            _ => Err(error("signature_registration_conflict")),
        }
    }

    pub fn validate_retirement(
        &self,
        registration: &SignatureRegistrationRef,
    ) -> GlobalLedgerResult<()> {
        if !self.gaps.is_empty() {
            return Err(error("signature_catalog_incomplete"));
        }
        match self.entries.get(&registration.signature_id) {
            Some(entry) if !entry.retired && entry.registration == *registration => Ok(()),
            _ => Err(error("signature_registration_not_active")),
        }
    }
}

pub fn registration_ref(
    event: &PersistedEvent,
    definition: &DiagnosticSignatureDefinition,
) -> SignatureRegistrationRef {
    SignatureRegistrationRef {
        signature_id: definition.signature_id.clone(),
        version: definition.version,
        event_id: *event.event_id(),
        sequence: event.sequence(),
    }
}

pub fn replay_signatures(
    input: &SignaturePrefix,
    catalog: &SignatureCatalog,
    request: &SignaturePageRequest,
) -> GlobalLedgerResult<SignatureReplayPage> {
    request
        .validate()
        .map_err(|_| error("signature_page_invalid"))?;
    let offset = match &request.cursor {
        None => 0,
        Some(cursor) if cursor.input == input.identity && cursor.catalog == catalog.identity => {
            cursor.row_offset
        }
        Some(_) => return Err(error("signature_cursor_snapshot_mismatch")),
    };
    let active = catalog
        .entries
        .values()
        .filter(|entry| !entry.retired)
        .collect::<Vec<_>>();
    let mut gaps = catalog.gaps.clone();
    if !input.identity.complete {
        gaps.push(SignatureReplayGap::InputIncomplete);
    }
    if active.is_empty() {
        gaps.push(SignatureReplayGap::CatalogEmpty);
    }
    let mut rows = Vec::new();
    let mut matched_count = 0;
    let mut missing_fields_count = 0;
    for event in &input.events {
        for entry in &active {
            let row = match matches_signature(&entry.definition, event) {
                Ok(false) => continue,
                Ok(true) => {
                    matched_count += 1;
                    SignatureReplayRow::Matched {
                        registration: entry.registration.clone(),
                        source_event_id: *event.event_id(),
                        source_sequence: event.sequence(),
                    }
                }
                Err(fields) => {
                    missing_fields_count += 1;
                    SignatureReplayRow::MissingFields {
                        registration: entry.registration.clone(),
                        source_event_id: *event.event_id(),
                        source_sequence: event.sequence(),
                        fields,
                    }
                }
            };
            let row_index = matched_count + missing_fields_count - 1;
            if row_index >= offset && rows.len() < usize::from(request.limit) {
                rows.push(row);
            }
        }
    }
    let total = matched_count + missing_fields_count;
    if offset > total {
        return Err(error("signature_cursor_out_of_range"));
    }
    let next = offset + rows.len() as u64;
    let next_cursor = (next < total).then(|| SignatureReplayCursor {
        input: input.identity.clone(),
        catalog: catalog.identity.clone(),
        row_offset: next,
    });
    let page = SignatureReplayPage {
        input: input.identity.clone(),
        catalog: catalog.identity.clone(),
        active_signatures: active.len(),
        matched_count,
        missing_fields_count,
        gaps,
        row_offset: offset,
        rows,
        next_cursor,
    };
    page.validate()
        .map_err(|_| error("signature_result_invalid"))?;
    Ok(page)
}

fn matches_signature(
    definition: &DiagnosticSignatureDefinition,
    event: &PersistedEvent,
) -> Result<bool, Vec<SignatureConditionField>> {
    let query = EventQuery {
        origin_module: Some(definition.origin_module),
        diagnostic_code: Some(definition.diagnostic_code),
        event_type: Some(definition.event_type),
        minimum_severity: Some(definition.minimum_severity),
        ..EventQuery::default()
    };
    if !crate::global::query_matches(&query, event) {
        return Ok(false);
    }
    let Some(predicate) = &definition.lifecycle else {
        return Ok(true);
    };
    let lifecycle = match event.payload() {
        EventPayload::Runtime(RuntimePayload::Failed(payload)) => payload.lifecycle_failure(),
        _ => None,
    };
    let Some(lifecycle) = lifecycle else {
        return Err(vec![SignatureConditionField::Lifecycle]);
    };
    let mut matches = predicate
        .stage
        .as_deref()
        .is_none_or(|value| value == lifecycle.stage())
        && predicate
            .code
            .as_deref()
            .is_none_or(|value| value == lifecycle.code());
    let mut missing = Vec::new();
    matches &= match_optional(
        predicate.operation.as_deref(),
        lifecycle.operation(),
        SignatureConditionField::Operation,
        &mut missing,
    );
    let needs_cause = predicate.cause_phase.is_some()
        || predicate.cause_source.is_some()
        || predicate.resource.is_some()
        || predicate.resource_phase.is_some()
        || predicate.quiescence.is_some();
    if needs_cause {
        if let Some(cause) = lifecycle.cause() {
            matches &= predicate
                .cause_phase
                .is_none_or(|value| value == cause.phase())
                && predicate
                    .cause_source
                    .as_deref()
                    .is_none_or(|value| value == cause.source());
            matches &= match_optional(
                predicate.resource.as_ref(),
                cause.resource().as_ref(),
                SignatureConditionField::Resource,
                &mut missing,
            );
            matches &= match_optional(
                predicate.resource_phase.as_ref(),
                cause.resource_phase().as_ref(),
                SignatureConditionField::ResourcePhase,
                &mut missing,
            );
            matches &= match_optional(
                predicate.quiescence.as_ref(),
                cause.quiescence().as_ref(),
                SignatureConditionField::Quiescence,
                &mut missing,
            );
        } else {
            missing.push(SignatureConditionField::Cause);
        }
    }
    if missing.is_empty() {
        Ok(matches)
    } else {
        Err(missing)
    }
}

fn match_optional<T: PartialEq + ?Sized>(
    expected: Option<&T>,
    actual: Option<&T>,
    field: SignatureConditionField,
    missing: &mut Vec<SignatureConditionField>,
) -> bool {
    match (expected, actual) {
        (None, _) => true,
        (Some(_), None) => {
            missing.push(field);
            false
        }
        (Some(expected), Some(actual)) => expected == actual,
    }
}

fn error(code: &'static str) -> GlobalLedgerError {
    GlobalLedgerError::request(code, "diagnostic_signatures")
}
