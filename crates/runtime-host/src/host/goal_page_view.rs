// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #455 RR4b-1 (register §1.557; `contracts/runtime-interface.md`, "Goal request kinds";
//! `runtime-client` 15): `GoalPageView`, one page of one part of an application's goal page, the
//! records with their positions, a table's options or one ladder's positions, which `GoalView`
//! does not say (`GoalView` stays byte for byte as it is). The read takes:
//!
//! - from memory, before the gates, as `GoalView` does: the instance's game, what the scheduler
//!   workbench keeps of the game root (its `goals.json` and `goal-tables.json` as read, and what the
//!   needs read of it, `records_root`, the one function both call) and the digest of the procedure
//!   manifest in place;
//! - under the policy outcome gate, the policy and the fact write gate, in that order, the locks
//!   the evaluation's needs are computed under: the authoritative projection with its pools'
//!   readings, the handed runs at its position, the active catalog and the remedy table; from the
//!   fact store, under its lock, the instance's two goal records (the page's version, as
//!   `GoalView`'s) and the progress lists the walk reads, through the needs' own sources and the
//!   evaluator's projection (`NeedsProjection`);
//!
//! then answers `crate::goal_page::read_part`. Until what the page is judged by is in memory the
//! page answers `GoalView`'s word (`not_checked_yet`, `root_failing` or `no_schedule`) and no row;
//! fix round 1: a request with a cursor is then refused `goal_page_view_declaration_absent`, with
//! the word and the page's version. It records nothing and stores nothing.

use super::goals::{
    StoredRecord, goal_declaration, kept_remedy_table, page_sources, page_version, unchecked_state,
};
use super::planning::planning_request_failure;
use super::record_needs::HostSources;
use super::standard_pack_schedule::records_root;
use super::*;
use crate::codes::{GoalPageCode, HostCode, HostLocation};
use crate::goal_page::{PageFailure, PageRead, PageState, entries_digest, page_head, read_part};
use crate::goal_records::StoredGoalRecords;
use actingcommand_contract::codes::RuntimeContractCode;
use actingcommand_contract::{
    GOAL_RECORDS_FACT_KEY, GoalPageCursor, GoalPagePart, GoalPageTables, GoalPageView,
    GoalTablesRead, RESOURCE_TARGETS_FACT_KEY,
};
use actingcommand_policy::NeedsProjection;

const OPERATION: HostLocation = HostLocation::GoalPageView;

impl HostShared {
    /// `GoalPageView` (Workflow #455 RR4b-1): see the module's description. An alias no instance
    /// has is Denied `instance_unknown`; every refusal is Denied and records nothing.
    pub(super) fn goal_page_view(
        &self,
        instance_alias: &str,
        part: &GoalPagePart,
        cursor: Option<&GoalPageCursor>,
        limit: Option<u32>,
    ) -> Result<OperationSuccess, RequestFailure> {
        self.resolve_instance(instance_alias)?;
        self.goal_page_view_read(instance_alias, part, cursor, limit)
            .map(|page| OperationSuccess {
                state: RuntimeReceiptState::Completed,
                terminal: None,
                result: RuntimeResult::GoalPageViewed {
                    page: Box::new(page),
                },
                failure_outcome: None,
            })
            .map_err(planning_request_failure)
    }

    fn goal_page_view_read(
        &self,
        instance_alias: &str,
        part: &GoalPagePart,
        cursor: Option<&GoalPageCursor>,
        limit: Option<u32>,
    ) -> RuntimeHostResult<GoalPageView> {
        let sample = self.runtime_clock_sample()?;
        let time = EvaluationTime {
            unix_ms: sample.unix_ms,
            monotonic_ms: sample.monotonic_ms,
        };
        let game_id = self.page_game(instance_alias, OPERATION)?;
        // Read before the policy, as the goal page reads the roots (the workbench's check holds
        // its state while it locks the policy).
        let goal_roots = self.standard_pack_goal_roots()?;
        let manifest_digest = lock(&self.procedure_manifest, OPERATION)?
            .as_ref()
            .map(|manifest| manifest.digest());
        let root = goal_roots
            .as_ref()
            .and_then(|roots| roots.roots.get(&game_id));
        let needs_root = root.map(records_root);
        let tables = page_tables(root);
        let scope = FactScope::Instance {
            instance_id: instance_alias.to_owned(),
        };
        // The gates the evaluation computes the needs under (lock order: policy_outcome_gate ->
        // policy -> fact_write_gate -> the fact store).
        let _outcome_gate = lock(&self.policy_outcome_gate, OPERATION)?;
        let mut policy = lock(self.policy()?, OPERATION)?;
        let outcome_keys = policy.outcome_key_snapshot()?;
        let _fact_gate = lock(&self.fact_write_gate, OPERATION)?;
        let (facts, _, readings) = self.project_authoritative_policy_inputs_under_gate(
            &mut policy,
            OPERATION,
            &outcome_keys,
            None,
        )?;
        let handed = policy.handed_runs(facts.ledger_position);
        let catalog = policy.active_loaded();
        let table = kept_remedy_table(
            policy.built_table(),
            catalog.as_ref(),
            manifest_digest.as_deref(),
        );
        let store = lock(&self.facts, OPERATION)?;
        let record = StoredRecord::of(&store, &scope, RESOURCE_TARGETS_FACT_KEY);
        let records = StoredRecord::of(&store, &scope, GOAL_RECORDS_FACT_KEY);
        let version = page_version(&record, &records).1;
        let Some(sources) =
            page_sources(catalog, table.as_ref(), goal_roots.as_ref(), root.is_some())
        else {
            let unread = goal_roots
                .as_ref()
                .and_then(|roots| roots.unread.get(&game_id));
            let (problem, declaration) = unchecked_state(unread);
            // Fix round 1: such a page holds nothing of its part, so the pages before a cursor
            // cannot go on over it; its declaration did not move (the tables' sha256 is still the
            // cursor's), which is a cause of its own.
            if cursor.is_some() {
                return Err(self.page_refused(PageFailure::DeclarationAbsent { problem }, version));
            }
            return Ok(page_head(
                instance_alias,
                &game_id,
                version,
                declaration,
                tables,
                sample.unix_ms,
            ));
        };
        let (declaration, accepted) = goal_declaration(root.map(|root| &root.goals));
        let projection =
            NeedsProjection::new(sources.catalog.compiled(), &facts, &readings, time, &handed)
                .map_err(|error| {
                    RuntimeHostError::request(
                        HostCode::PolicyEvaluationRejected,
                        OPERATION,
                        RuntimeErrorCode::InvalidRequest,
                    )
                    .with_native_detail(error.to_string())
                })?;
        let host_sources = HostSources {
            store: &store,
            scope,
            instance_id: instance_alias,
            projection: &projection,
        };
        let stored = StoredGoalRecords::read(records.rows.as_deref(), instance_alias);
        let needs_tables = needs_root.as_ref().and_then(|root| root.tables.as_ref());
        let read = PageRead {
            declaration: accepted.as_ref(),
            tables: needs_tables.map(|(tables, _)| tables.as_ref()),
            root: needs_root.as_ref(),
            stored: &stored,
            now: PageState {
                records_version: records.version,
                tables_sha256: needs_tables.map(|(_, sha256)| sha256.clone()),
                entries_sha256: entries_digest(accepted.as_ref()),
            },
        };
        let head = page_head(
            instance_alias,
            &game_id,
            version,
            declaration,
            tables,
            sample.unix_ms,
        );
        read_part(head, &read, &host_sources, part, cursor, limit)
            .map_err(|failure| self.page_refused(failure, version))
    }

    /// The refusal of a page `read_part` did not answer, the goal page's version being `version`:
    /// a cursor of more rows than the part holds `invalid_goal_page_view`; a part that names what
    /// the root does not have `goal_page_view_refused` with its word and pointer; a cursor over
    /// other stored records `goal_page_view_records_moved`, over another declaration
    /// `goal_page_view_declaration_moved`, each with `current_version`; a cursor sent to a page not
    /// in memory `goal_page_view_declaration_absent` with its state's word (`goal_problem`) and
    /// `current_version` (fix round 1); a stored row the reader took that gives back no setter or
    /// target `goal_records_record_invalid` (its daemon line written, as a composed record not read
    /// back is); a page that does not serialize `runtime_frame_encode_failed`. Each is Denied and
    /// records nothing.
    fn page_refused(&self, failure: PageFailure, version: u64) -> RuntimeHostError {
        let refused = |code: GoalPageCode| {
            RuntimeHostError::request(code, OPERATION, RuntimeErrorCode::InvalidRequest)
        };
        match failure {
            PageFailure::Invalid => RuntimeHostError::request(
                RuntimeContractCode::InvalidGoalPageView,
                OPERATION,
                RuntimeErrorCode::InvalidRequest,
            ),
            PageFailure::Refused { problem, pointer } => refused(GoalPageCode::Refused)
                .with_outcome_values(move |outcome| {
                    outcome.goal_problem(problem).json_pointer(pointer)
                }),
            PageFailure::RecordsMoved => refused(GoalPageCode::RecordsMoved)
                .with_outcome_values(move |outcome| outcome.current_version(version)),
            PageFailure::DeclarationMoved => refused(GoalPageCode::DeclarationMoved)
                .with_outcome_values(move |outcome| outcome.current_version(version)),
            PageFailure::DeclarationAbsent { problem } => refused(GoalPageCode::DeclarationAbsent)
                .with_outcome_values(move |outcome| {
                    outcome.goal_problem(problem).current_version(version)
                }),
            PageFailure::RecordUnread => {
                let refusal = RuntimeHostError::request(
                    HostCode::GoalRecordsRecordInvalid,
                    OPERATION,
                    RuntimeErrorCode::InvalidRequest,
                );
                self.fatal.report_warning(&refusal.daemon_outcome());
                refusal
            }
            PageFailure::Unencodable => RuntimeHostError::request(
                HostCode::RuntimeFrameEncodeFailed,
                OPERATION,
                RuntimeErrorCode::InvalidRequest,
            ),
        }
    }
}

/// What the workbench holds of the game root's `goal-tables.json` (`root`, none: nothing read of
/// it), as the page says it: absent, accepted with its sha256, refused with its sha256 and its first
/// problem at its pointer, unreadable, or not read.
fn page_tables(root: Option<&StandardPackGoalRoot>) -> GoalPageTables {
    let plain = |state| GoalPageTables {
        state,
        sha256: None,
        standard_pack_problem: None,
        json_pointer: None,
    };
    match root.map(|root| &root.goal_tables) {
        None => plain(GoalTablesRead::NotRead),
        Some(RootGoalTables::Absent) => plain(GoalTablesRead::Absent),
        Some(RootGoalTables::Accepted { sha256, .. }) => GoalPageTables {
            sha256: Some(sha256.clone()),
            ..plain(GoalTablesRead::Accepted)
        },
        Some(RootGoalTables::Refused { sha256, issues }) => {
            let first = issues.first();
            GoalPageTables {
                state: GoalTablesRead::Refused,
                sha256: Some(sha256.clone()),
                standard_pack_problem: first.map(|issue| issue.problem.as_str().to_owned()),
                json_pointer: first.map(|issue| issue.pointer.clone()),
            }
        }
        Some(RootGoalTables::Unreadable(_)) => plain(GoalTablesRead::Unreadable),
    }
}
