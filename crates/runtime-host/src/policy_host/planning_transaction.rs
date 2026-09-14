// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_ledger::{
    GlobalLedgerError, LedgerTransactionWork, TransactionStateObservation, TransactionWorkError,
};
use actingcommand_runtime_database::RuntimeTransaction;
use actingcommand_runtime_state::{PlanningStateObservation, PreparedPlanningProjection};

pub(crate) struct PlanningTransaction(Arc<PreparedPlanningProjection>);

pub(crate) struct PreparedPlanningQuota {
    quota: DetectionQuotaState,
    projection: Arc<PreparedPlanningProjection>,
}

impl PreparedPlanningQuota {
    pub(crate) fn attempted_sequence(&self) -> Option<u64> {
        self.projection.attempted_sequence()
    }
}

impl LedgerTransactionWork for PlanningTransaction {
    fn apply(
        &self,
        transaction: &RuntimeTransaction<'_, '_>,
        event: &PersistedEvent,
    ) -> Result<(), TransactionWorkError> {
        self.0.apply(transaction, event).map_err(|error| {
            let mut error = planning_work_error(error);
            error.detail = format!(
                "event_id={:?}; sequence={}; primary={}",
                event.event_id(),
                event.sequence(),
                error.detail
            );
            error
        })
    }

    fn observe(
        &self,
        transaction: &RuntimeTransaction<'_, '_>,
    ) -> Result<TransactionStateObservation, TransactionWorkError> {
        self.0
            .observe(transaction)
            .map(|state| match state {
                PlanningStateObservation::Applied => TransactionStateObservation::Applied,
                PlanningStateObservation::Unchanged => TransactionStateObservation::Unchanged,
                PlanningStateObservation::Unknown => TransactionStateObservation::Unknown,
            })
            .map_err(planning_work_error)
    }
}

impl PolicyHost {
    pub(crate) fn prepare_planning_signal(
        &self,
        data: &PolicyPlanningSignalEventData,
    ) -> RuntimeHostResult<(PlanningTransaction, PreparedPlanningQuota)> {
        let prepared = Arc::new(
            self.store
                .state
                .prepare_planning_projection(data)
                .map_err(planning_state_error)?,
        );
        let mut staged_quota = self.detection_quota.clone();
        let prior = prepared.prior_quota().map_err(planning_state_error)?;
        self.stage_planning_signal(&mut staged_quota, data, prior, false)?;
        Ok((
            PlanningTransaction(Arc::clone(&prepared)),
            PreparedPlanningQuota {
                quota: staged_quota,
                projection: prepared,
            },
        ))
    }

    pub(crate) fn publish_planning_signal(&mut self, quota: PreparedPlanningQuota) {
        self.detection_quota = quota.quota;
    }
}

pub(super) fn planning_state_error(
    error: actingcommand_runtime_state::RuntimeStateError,
) -> RuntimeHostError {
    RuntimeHostError::state(&error).with_native_detail(error.to_string())
}

fn planning_work_error(
    error: actingcommand_runtime_state::RuntimeStateError,
) -> TransactionWorkError {
    TransactionWorkError {
        code: error.code(),
        operation: error.operation(),
        fatal: error.is_fatal(),
        detail: error.to_string(),
    }
}

pub(crate) fn planning_transaction_error(error: GlobalLedgerError) -> RuntimeHostError {
    if let Some(work) = error.rolled_back_work() {
        let mapped = if work.fatal {
            RuntimeHostError::fatal(work.code, work.operation, RuntimeErrorCode::RuntimeFatal)
        } else {
            RuntimeHostError::request(work.code, work.operation, RuntimeErrorCode::InvalidRequest)
        };
        return mapped.with_native_detail(format!("rollback=confirmed; {}", work.detail));
    }
    RuntimeHostError::fatal(
        error.code(),
        error.operation(),
        RuntimeErrorCode::LedgerFailure,
    )
    .with_native_detail(format!("{error}; detail={:?}", error.detail()))
}
