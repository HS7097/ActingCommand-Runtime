// SPDX-License-Identifier: AGPL-3.0-only

use super::{
    ForensicError, ForensicOutput, ForensicReport, ForensicResult, map_ledger_error,
    query_view_page,
};
use actingcommand_contract::{EventQuery, ProjectionProfile, RuntimeEventQueryPageRequest};
use actingcommand_ledger::{GlobalLedger, GlobalLedgerEvidenceConfig};
use std::path::{Path, PathBuf};

/// Argument values delegated by the thin executable to the native read adapter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForensicViewOptions {
    pub query: Option<String>,
    pub profile: Option<String>,
    pub cursor: Option<String>,
    pub snapshot: Option<u64>,
    pub limit: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForensicViewRequest {
    state_root: PathBuf,
    query: EventQuery,
    profile: ProjectionProfile,
    page: RuntimeEventQueryPageRequest,
}

impl ForensicViewRequest {
    pub fn from_options(
        state_root: impl AsRef<Path>,
        options: ForensicViewOptions,
    ) -> ForensicResult<Self> {
        let invalid =
            |detail| ForensicError::new("invalid_view_options", "validate_view_request", detail);
        if options
            .query
            .as_ref()
            .is_some_and(|query| query.len() > 16 * 1024)
            || options
                .cursor
                .as_ref()
                .is_some_and(|cursor| cursor.len() > 2048)
        {
            return Err(invalid("view query or cursor exceeds bound"));
        }
        let query = options
            .query
            .map(|query| {
                serde_json::from_str::<EventQuery>(&query)
                    .map_err(|_| invalid("invalid typed view query"))
            })
            .transpose()?
            .unwrap_or_default();
        let profile = options
            .profile
            .map(|profile| {
                serde_json::from_value::<ProjectionProfile>(serde_json::Value::String(profile))
                    .map_err(|_| invalid("unknown projection profile"))
            })
            .transpose()?
            .unwrap_or(ProjectionProfile::Ui);
        let cursor = options
            .cursor
            .map(|cursor| {
                serde_json::from_str::<actingcommand_contract::RuntimeEventQueryCursor>(&cursor)
                    .map_err(|_| invalid("invalid view cursor"))
            })
            .transpose()?;
        let mut page = RuntimeEventQueryPageRequest::new(
            options
                .limit
                .unwrap_or(actingcommand_contract::DEFAULT_RUNTIME_EVENT_QUERY_EVENTS),
            cursor,
        )
        .map_err(|_| invalid("invalid view page"))?;
        if let Some(snapshot) = options.snapshot {
            page = page
                .at_snapshot(snapshot)
                .map_err(|_| invalid("invalid view snapshot"))?;
        }
        Self::new(state_root, query, profile, page)
    }

    pub fn new(
        state_root: impl AsRef<Path>,
        query: EventQuery,
        profile: ProjectionProfile,
        page: RuntimeEventQueryPageRequest,
    ) -> ForensicResult<Self> {
        if state_root.as_ref().as_os_str().is_empty() {
            return Err(ForensicError::new(
                "invalid_state_root",
                "validate_view_request",
                "state root is empty",
            ));
        }
        query.validate().map_err(|error| {
            ForensicError::new(
                error.code(),
                "validate_view_request",
                "invalid query bounds",
            )
        })?;
        page.validate().map_err(|error| {
            ForensicError::new(
                error.code(),
                "validate_view_request",
                "invalid page or snapshot",
            )
        })?;
        Ok(Self {
            state_root: state_root.as_ref().to_path_buf(),
            query,
            profile,
            page,
        })
    }
}

/// The view entry opens verified ledger metadata and leaves material bytes unread.
pub fn run_views(request: ForensicViewRequest) -> ForensicResult<ForensicOutput> {
    let snapshot =
        GlobalLedger::open_metadata(GlobalLedgerEvidenceConfig::new(&request.state_root))
            .map_err(map_ledger_error)?;
    let page = query_view_page(&snapshot, &request.query, request.profile, &request.page)?;
    Ok(ForensicOutput::Machine(ForensicReport::Views(Box::new(
        page,
    ))))
}
