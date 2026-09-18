// SPDX-License-Identifier: AGPL-3.0-only

use super::{
    ForensicError, ForensicOutput, ForensicReport, ForensicResult, instance_bindings,
    map_ledger_error, query_view_page,
};
use actingcommand_contract::{
    EventQuery, InstanceId, ProjectionProfile, RuntimeEventQueryCursor,
    RuntimeEventQueryPageRequest,
};
use actingcommand_ledger::{GlobalLedger, GlobalLedgerEvidenceConfig, GlobalLedgerMetadata};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Argument values delegated by the thin executable to the native read adapter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForensicViewOptions {
    pub query: Option<String>,
    pub profile: Option<String>,
    pub cursor: Option<String>,
    pub snapshot: Option<u64>,
    pub limit: Option<u16>,
    pub instance_port: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForensicViewRequest {
    state_root: PathBuf,
    query: EventQuery,
    profile: ProjectionProfile,
    page: RuntimeEventQueryPageRequest,
    instance_port: Option<u16>,
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
                serde_json::from_str::<RuntimeEventQueryCursor>(&cursor)
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
            .map(|request| request.with_instance_port(options.instance_port))
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
            instance_port: None,
        })
    }

    /// Resolves the ADB port at the page's snapshot to every instance id ever bound to it
    /// and runs the page with that set as the instance condition.
    pub const fn with_instance_port(mut self, instance_port: Option<u16>) -> Self {
        self.instance_port = instance_port;
        self
    }
}

/// The view entry opens verified ledger metadata and leaves material bytes unread.
pub fn run_views(request: ForensicViewRequest) -> ForensicResult<ForensicOutput> {
    let ForensicViewRequest {
        state_root,
        mut query,
        profile,
        page,
        instance_port,
    } = request;
    let snapshot = GlobalLedger::open_metadata(GlobalLedgerEvidenceConfig::new(&state_root))
        .map_err(map_ledger_error)?;
    if let Some(port) = instance_port {
        resolve_instance_port(&snapshot, &mut query, &page, port)?;
    }
    let page = query_view_page(&snapshot, &query, profile, &page)?;
    Ok(ForensicOutput::Machine(ForensicReport::Views(Box::new(
        page,
    ))))
}

/// The port map is read at the requested snapshot. An instance condition already in the
/// query must be exactly the resolved set; the query is then normalized to that set so a
/// continuation cursor fingerprints the same query.
fn resolve_instance_port(
    snapshot: &GlobalLedgerMetadata,
    query: &mut EventQuery,
    page: &RuntimeEventQueryPageRequest,
    port: u16,
) -> ForensicResult<()> {
    let position = page
        .cursor()
        .map(RuntimeEventQueryCursor::snapshot_ledger_position)
        .or(page.snapshot_position())
        .unwrap_or_else(|| snapshot.latest_sequence());
    let bindings = instance_bindings(snapshot, position)?;
    let resolved = bindings
        .ports
        .get(&port)
        .filter(|instance_ids| !instance_ids.is_empty())
        .ok_or_else(|| {
            ForensicError::new(
                "instance_port_unknown",
                "resolve_instance_port",
                format!("no instance was bound to ADB port {port} through position {position}"),
            )
        })?;
    let given: Option<BTreeSet<InstanceId>> = match (query.instance_id, &query.instance_ids) {
        (Some(instance_id), _) => Some(BTreeSet::from([instance_id])),
        (None, instance_ids) if instance_ids.is_empty() => None,
        (None, instance_ids) => Some(instance_ids.iter().copied().collect()),
    };
    if given.is_some_and(|given| given != resolved.iter().copied().collect::<BTreeSet<_>>()) {
        return Err(ForensicError::new(
            "instance_port_conflict",
            "resolve_instance_port",
            format!("the query instance condition is not the set bound to ADB port {port}"),
        ));
    }
    query.instance_id = None;
    query.instance_ids = resolved.clone();
    Ok(())
}
