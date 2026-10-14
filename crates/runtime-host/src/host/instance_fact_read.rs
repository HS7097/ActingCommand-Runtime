// SPDX-License-Identifier: AGPL-3.0-only

//! `RuntimeOperation::InstanceFactSnapshot` (Workflow #389 MCP-4, register W2): one instance's
//! facts, read from the instance fact store and answered with the read's state source (register
//! W1). Like a status read (Workflow #383 AO-4q) it appends no ledger record.
//!
//! The host's own fact projection (`InstanceFactSnapshot`, which policy admission and contained
//! runs read) is pinned to a ledger position and carries a snapshot id derived from it. Neither
//! goes on the wire (`contracts/ledger-observer.md` §13): this read builds an `InstanceFactView`
//! from the store's records directly, and the source names the ledger's committed head instead.

use super::*;
use crate::codes::{HostCode, HostLocation};
use actingcommand_contract::InstanceFactView;

impl HostShared {
    /// The facts of the registered instance `instance_alias`: its own records, and those of the
    /// server and game the configuration binds it to (none when it binds none). An alias the
    /// registry does not hold is refused `instance_unknown`; an answer larger than one IPC frame
    /// is refused `instance_fact_snapshot_too_large_for_frame` instead of failing the frame write.
    pub(super) fn instance_fact_snapshot_read(
        &self,
        request: &RuntimeRequest,
        instance_alias: &str,
    ) -> Result<OperationSuccess, RequestFailure> {
        self.resolve_instance(instance_alias)?;
        // The configured server and game, read and released before the fact gate.
        let shared = lock(&self.policy_inputs, HostLocation::ReadInstanceFactSnapshot)?
            .as_ref()
            .and_then(|inputs| {
                inputs
                    .instance_identities()
                    .find(|identity| identity.instance_id == instance_alias)
                    .map(|identity| (identity.server_id.to_owned(), identity.game_id.to_owned()))
            });
        let (snapshot, source) = self.observe_runtime_state(|| {
            self.instance_fact_view(instance_alias, shared)
                .map_err(|error| {
                    if error.is_fatal() {
                        RequestFailure::poison_without_terminal(error)
                    } else {
                        RequestFailure::request(error, RuntimeReceiptState::Denied, None)
                    }
                })
        })?;
        let result = RuntimeResult::InstanceFactSnapshot {
            snapshot: Box::new(snapshot),
            source,
        };
        let reply_bytes = RuntimeReceipt::success(
            request,
            RuntimeReceiptState::Completed,
            None,
            result.clone(),
        )
        .ok()
        .and_then(|receipt| serde_json::to_vec(&receipt).ok())
        .ok_or_else(|| RequestFailure::poison_without_terminal(receipt_error()))?
        .len();
        if reply_bytes > self.maximum_frame_bytes {
            return Err(RequestFailure::request(
                RuntimeHostError::request(
                    HostCode::InstanceFactSnapshotTooLargeForFrame,
                    HostLocation::ReadInstanceFactSnapshot,
                    RuntimeErrorCode::InvalidRequest,
                )
                .with_native_detail(format!(
                    "reply is {reply_bytes} bytes; one frame carries at most {} bytes",
                    self.maximum_frame_bytes
                )),
                RuntimeReceiptState::Denied,
                None,
            ));
        }
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: None,
            result,
            failure_outcome: None,
        })
    }

    /// The instance's records under the fact write gate, after the store caught up with the
    /// ledger, as every fact read does. Nothing is appended.
    fn instance_fact_view(
        &self,
        instance_alias: &str,
        shared: Option<(String, String)>,
    ) -> RuntimeHostResult<InstanceFactView> {
        let _gate = lock(
            &self.fact_write_gate,
            HostLocation::ReadInstanceFactSnapshot,
        )?;
        self.synchronize_fact_store_under_gate()?;
        let records = lock(&self.facts, HostLocation::ReadInstanceFactSnapshot)?.instance_records(
            instance_alias,
            shared
                .as_ref()
                .map(|(server_id, game_id)| (server_id.as_str(), game_id.as_str())),
        );
        let (server_id, game_id) = shared.unzip();
        let view = InstanceFactView {
            instance_alias: instance_alias.to_owned(),
            server_id,
            game_id,
            records,
        };
        view.validate().map_err(|_| {
            RuntimeHostError::fatal(
                HostCode::FactSnapshotInvalid,
                HostLocation::ReadInstanceFactSnapshot,
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
        Ok(view)
    }
}
