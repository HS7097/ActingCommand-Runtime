// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::{
    ApprovalDecisionRecord, ApprovalPayload, EventActor, EventPayload, EventSource, EventType,
    OriginModule,
};
use actingcommand_ledger::PersistedEvent;
use actingcommand_runtime_database::RuntimeTransaction;

pub const APPROVAL_PROJECTION_NAMESPACE: &str = "approval.latest.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalStateObservation {
    Applied,
    Unchanged,
    Unknown,
}

struct ApprovalFact {
    event: PersistedEvent,
    decision: ApprovalDecisionRecord,
    key: String,
    payload: Vec<u8>,
    payload_sha256: String,
}

impl ApprovalFact {
    fn prepare(event: &PersistedEvent) -> RuntimeStateResult<Self> {
        if event.event_type() != EventType::ApprovalDecision
            || event.origin().source() != EventSource::Ui
            || event.origin().actor() != EventActor::User
            || event.origin().module() != OriginModule::Governance
        {
            return Err(approval_error(
                SharedCode::ApprovalProjectionOriginInvalid.code(),
            ));
        }
        let EventPayload::Approval(ApprovalPayload::Decision(payload)) = event.payload() else {
            return Err(approval_error(
                SharedCode::ApprovalProjectionPayloadMismatch.code(),
            ));
        };
        let decision = payload.decision().clone();
        let (key, payload) = prepare_decision(&decision)?;
        validate_projection_input(
            APPROVAL_PROJECTION_NAMESPACE,
            &key,
            event.sequence(),
            &payload,
        )?;
        sqlite_integer(
            event.sequence(),
            StateCode::ProjectionSequenceOverflow.code(),
            StateLocation::PrepareApprovalProjection.location(),
        )?;
        let payload_sha256 = sha256(&payload);
        Ok(Self {
            event: event.clone(),
            decision,
            key,
            payload,
            payload_sha256,
        })
    }

    fn matches_entry(&self, entry: &ProjectionEntry) -> bool {
        entry.namespace() == APPROVAL_PROJECTION_NAMESPACE
            && entry.entry_key() == self.key
            && entry.ledger_sequence() == self.event.sequence()
            && entry.payload() == self.payload
    }

    fn verify(
        &self,
        database: &RuntimeDatabase,
        transaction: &RuntimeTransaction<'_, '_>,
    ) -> RuntimeStateResult<()> {
        actingcommand_ledger::verify_transaction_event(database, transaction, &self.event).map_err(
            |error| {
                RuntimeStateError::relayed_fatal(
                    error.code(),
                    StateLocation::PersistApprovalProjection.as_str(),
                )
                .with_detail(format!("{error}; detail={:?}", error.detail()))
            },
        )
    }
}

pub struct PreparedApprovalProjection {
    store: Arc<RuntimeStateStore>,
    decision: ApprovalDecisionRecord,
    key: String,
    payload: Vec<u8>,
    payload_sha256: String,
    prior: Option<ApprovalFact>,
    baseline: Option<ProjectionEntry>,
    applied_sequence: AtomicU64,
}

impl PreparedApprovalProjection {
    pub fn apply(
        &self,
        transaction: &RuntimeTransaction<'_, '_>,
        event: &PersistedEvent,
    ) -> RuntimeStateResult<()> {
        self.require_owner(transaction)?;
        if event.event_type() != EventType::ApprovalDecision
            || event.origin().source() != EventSource::Ui
            || event.origin().actor() != EventActor::User
            || event.origin().module() != OriginModule::Governance
        {
            return Err(approval_error(
                SharedCode::ApprovalProjectionOriginInvalid.code(),
            ));
        }
        if !matches!(event.payload(), EventPayload::Approval(ApprovalPayload::Decision(payload))
            if payload.decision() == &self.decision)
        {
            return Err(approval_error(
                StateCode::ApprovalProjectionFactMismatch.code(),
            ));
        }
        actingcommand_ledger::verify_transaction_event(&self.store.database, transaction, event)
            .map_err(|error| {
                RuntimeStateError::relayed_fatal(
                    error.code(),
                    StateLocation::PersistApprovalProjection.as_str(),
                )
                .with_detail(error.to_string())
            })?;
        if let Some(prior) = &self.prior {
            prior.verify(&self.store.database, transaction)?;
            if event.sequence() <= prior.event.sequence() {
                return Err(approval_error(
                    SharedCode::ApprovalProjectionOrderInvalid.code(),
                ));
            }
        }
        let current = self.read_current(transaction)?;
        if current != self.baseline {
            return Err(approval_error(StateCode::ApprovalProjectionChanged.code()));
        }
        self.applied_sequence
            .store(event.sequence(), Ordering::Release);
        let written = self.store.write_projection_in_transaction(
            transaction.sql(),
            APPROVAL_PROJECTION_NAMESPACE,
            &self.key,
            event.sequence(),
            &self.payload,
            &self.payload_sha256,
        )?;
        if written.ledger_sequence() != event.sequence() || written.payload() != self.payload {
            return Err(approval_error(
                StateCode::ApprovalProjectionPositionMismatch.code(),
            ));
        }
        Ok(())
    }

    pub fn observe(
        &self,
        transaction: &RuntimeTransaction<'_, '_>,
    ) -> RuntimeStateResult<ApprovalStateObservation> {
        self.require_owner(transaction)?;
        let current = self.read_current(transaction)?;
        let sequence = self.applied_sequence.load(Ordering::Acquire);
        Ok(
            if sequence != 0
                && current.as_ref().is_some_and(|entry| {
                    entry.ledger_sequence() == sequence && entry.payload() == self.payload
                })
            {
                ApprovalStateObservation::Applied
            } else if current == self.baseline {
                ApprovalStateObservation::Unchanged
            } else {
                ApprovalStateObservation::Unknown
            },
        )
    }

    fn require_owner(&self, transaction: &RuntimeTransaction<'_, '_>) -> RuntimeStateResult<()> {
        if !transaction.belongs_to(&self.store.database) {
            return Err(approval_error(
                StateCode::StateTransactionOwnerMismatch.code(),
            ));
        }
        Ok(())
    }

    fn read_current(
        &self,
        transaction: &RuntimeTransaction<'_, '_>,
    ) -> RuntimeStateResult<Option<ProjectionEntry>> {
        query_projection_entry(transaction.sql(), APPROVAL_PROJECTION_NAMESPACE, &self.key)?
            .map(|row| {
                self.store
                    .validate_projection_row(row, StateLocation::ApplyApprovalProjection.location())
            })
            .transpose()
    }
}

impl RuntimeStateStore {
    /// Prepares State-owned payload and baseline before the Ledger writer takes its transaction.
    pub fn prepare_approval_projection(
        self: &Arc<Self>,
        decision: &ApprovalDecisionRecord,
        prior: Option<&PersistedEvent>,
    ) -> RuntimeStateResult<PreparedApprovalProjection> {
        let (key, payload) = prepare_decision(decision)?;
        let prior = prior.map(ApprovalFact::prepare).transpose()?;
        if prior.as_ref().is_some_and(|fact| {
            fact.decision.approval_id() != decision.approval_id()
                || fact.decision.target() != decision.target()
        }) {
            return Err(approval_error(
                SharedCode::ApprovalTargetIdentityConflict.code(),
            ));
        }
        let baseline = self.read_projection_entry(APPROVAL_PROJECTION_NAMESPACE, &key)?;
        if let Some(entry) = &baseline
            && prior.as_ref().is_none_or(|fact| !fact.matches_entry(entry))
        {
            return Err(approval_error(
                StateCode::ApprovalProjectionSourceUnproven.code(),
            ));
        }
        Ok(PreparedApprovalProjection {
            store: Arc::clone(self),
            decision: decision.clone(),
            key,
            payload_sha256: sha256(&payload),
            payload,
            prior,
            baseline,
            applied_sequence: AtomicU64::new(0),
        })
    }

    /// Rebuilds only from the complete verified approval history, without adding a decision.
    pub fn recover_approval_projections(
        &self,
        events: &[PersistedEvent],
    ) -> RuntimeStateResult<()> {
        let facts = events
            .iter()
            .map(ApprovalFact::prepare)
            .collect::<RuntimeStateResult<Vec<_>>>()?;
        let mut latest = BTreeMap::<&str, &ApprovalFact>::new();
        let mut positions = BTreeMap::new();
        let mut previous_sequence = 0;
        for fact in &facts {
            if fact.event.sequence() <= previous_sequence {
                return Err(approval_error(
                    SharedCode::ApprovalProjectionOrderInvalid.code(),
                ));
            }
            previous_sequence = fact.event.sequence();
            if let Some(previous) = latest.insert(&fact.key, fact)
                && previous.decision.target() != fact.decision.target()
            {
                return Err(approval_error(
                    SharedCode::ApprovalTargetIdentityConflict.code(),
                ));
            }
            positions.insert(fact.event.sequence(), fact);
        }
        let mut connection =
            self.connection(StateLocation::RecoverApprovalProjections.location())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                approval_sql_error(StateLocation::BeginApprovalRecovery.location(), &error)
            })?;
        let work = (|| {
            let scope = self.database.borrow_transaction(&transaction);
            for fact in &facts {
                fact.verify(&self.database, &scope)?;
            }
            let keys = {
                let mut statement = transaction.prepare(
                    "SELECT entry_key FROM projection_entries WHERE namespace=?1 ORDER BY entry_key"
                ).map_err(|error| approval_sql_error(StateLocation::ReadApprovalProjectionKeys.location(), &error))?;
                statement
                    .query_map([APPROVAL_PROJECTION_NAMESPACE], |row| {
                        row.get::<_, String>(0)
                    })
                    .map_err(|error| {
                        approval_sql_error(
                            StateLocation::ReadApprovalProjectionKeys.location(),
                            &error,
                        )
                    })?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| {
                        approval_sql_error(
                            StateLocation::ReadApprovalProjectionKeys.location(),
                            &error,
                        )
                    })?
            };
            for key in keys {
                let row =
                    query_projection_entry(&transaction, APPROVAL_PROJECTION_NAMESPACE, &key)?
                        .ok_or_else(|| {
                            approval_error(StateCode::ApprovalProjectionMissing.code())
                        })?;
                let entry = self.validate_projection_row(
                    row,
                    StateLocation::RecoverApprovalProjections.location(),
                )?;
                if positions
                    .get(&entry.ledger_sequence())
                    .is_none_or(|fact| !fact.matches_entry(&entry))
                {
                    return Err(approval_error(
                        StateCode::ApprovalProjectionSourceUnproven.code(),
                    ));
                }
            }
            for fact in latest.values() {
                let written = self.write_projection_in_transaction(
                    &transaction,
                    APPROVAL_PROJECTION_NAMESPACE,
                    &fact.key,
                    fact.event.sequence(),
                    &fact.payload,
                    &fact.payload_sha256,
                )?;
                if !fact.matches_entry(&written) {
                    return Err(approval_error(
                        StateCode::ApprovalProjectionPositionMismatch.code(),
                    ));
                }
            }
            Ok(())
        })();
        if let Err(original) = work {
            return match transaction.rollback() {
                Ok(()) => Err(original),
                Err(rollback) => Err(approval_error(
                    StateCode::ApprovalProjectionRollbackFailed.code(),
                )
                .with_detail(format!("primary={original}; rollback={rollback}"))),
            };
        }
        transaction.commit().map_err(|error| {
            approval_sql_error(StateLocation::CommitApprovalRecovery.location(), &error)
                .with_detail(format!("commit_outcome=unknown; primary={error}"))
        })?;
        Ok(())
    }
}

fn prepare_decision(decision: &ApprovalDecisionRecord) -> RuntimeStateResult<(String, Vec<u8>)> {
    decision
        .validate()
        .map_err(|_| approval_error(SharedCode::ApprovalProjectionRecordInvalid.code()))?;
    let key = format!("{:x}", Sha256::digest(decision.approval_id().as_bytes()));
    let payload = serde_json::to_vec(decision)
        .map_err(|_| approval_error(StateCode::ApprovalProjectionEncodeFailed.code()))?;
    validate_projection_input(APPROVAL_PROJECTION_NAMESPACE, &key, 1, &payload)?;
    Ok((key, payload))
}

fn approval_error(code: Code) -> RuntimeStateError {
    fatal(code, StateLocation::PersistApprovalProjection.location())
}

fn approval_sql_error(operation: Location, error: &rusqlite::Error) -> RuntimeStateError {
    fatal(
        StateCode::ApprovalProjectionTransactionFailed.code(),
        operation,
    )
    .with_detail(error.to_string())
}
