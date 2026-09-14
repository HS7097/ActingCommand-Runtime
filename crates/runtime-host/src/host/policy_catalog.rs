// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

impl HostShared {
    pub(super) fn active_policy_catalog(&self) -> RuntimeHostResult<Option<CatalogGeneration>> {
        if let Some(error) = self.fatal.current()? {
            return Err(error);
        }
        Ok(lock(&self.policy, "read_active_policy_catalog")?.active_generation())
    }

    pub(super) fn activate_policy_catalog(
        &self,
        sources: &CatalogSources,
    ) -> RuntimeHostResult<CatalogGeneration> {
        self.activate_policy_catalog_with_authorization(sources, None)
    }

    pub(super) fn activate_policy_catalog_with_authorization(
        &self,
        sources: &CatalogSources,
        promotion: Option<CatalogPromotionAuthorization>,
    ) -> RuntimeHostResult<CatalogGeneration> {
        let (catalog, previous) = {
            let policy = lock(&self.policy, "stage_policy_catalog")?;
            if let Some(error) = self.fatal.current()? {
                return Err(error);
            }
            let catalog = policy.stage(sources)?;
            let previous = policy.active_generation();
            (catalog, previous)
        };
        if previous
            .as_ref()
            .is_some_and(|current| current.catalog_hash() == catalog.generation().catalog_hash())
        {
            return Ok(catalog.generation().clone());
        }
        if let Some(current) = &previous
            && (current.catalog_id() != catalog.generation().catalog_id()
                || catalog.generation().catalog_version() <= current.catalog_version())
        {
            return Err(RuntimeHostError::request(
                "catalog_activation_not_newer",
                "activate_policy_catalog",
                RuntimeErrorCode::InvalidRequest,
            ));
        }
        self.switch_policy_catalog(
            catalog,
            previous,
            EventAction::CatalogActivate,
            CatalogTransitionTarget::Activated,
            promotion,
        )
    }

    pub(super) fn rollback_policy_catalog(
        &self,
        catalog_hash: &str,
    ) -> RuntimeHostResult<CatalogGeneration> {
        let (catalog, previous) = {
            let policy = lock(&self.policy, "load_policy_catalog_rollback")?;
            if let Some(error) = self.fatal.current()? {
                return Err(error);
            }
            let previous = policy.active_generation().ok_or_else(|| {
                RuntimeHostError::request(
                    "policy_catalog_unavailable",
                    "rollback_policy_catalog",
                    RuntimeErrorCode::InvalidRequest,
                )
            })?;
            let catalog = policy.load_generation(catalog_hash)?;
            (catalog, previous)
        };
        if catalog.generation().catalog_hash() == previous.catalog_hash() {
            return Ok(previous);
        }
        if catalog.generation().catalog_id() != previous.catalog_id()
            || catalog.generation().catalog_version() >= previous.catalog_version()
        {
            return Err(RuntimeHostError::request(
                "catalog_rollback_not_older",
                "rollback_policy_catalog",
                RuntimeErrorCode::InvalidRequest,
            ));
        }
        self.switch_policy_catalog(
            catalog,
            Some(previous),
            EventAction::CatalogRollback,
            CatalogTransitionTarget::RolledBack,
            None,
        )
    }

    pub(super) fn switch_policy_catalog(
        &self,
        catalog: LoadedCatalog,
        previous: Option<CatalogGeneration>,
        action: EventAction,
        target: CatalogTransitionTarget,
        promotion: Option<CatalogPromotionAuthorization>,
    ) -> RuntimeHostResult<CatalogGeneration> {
        let result = (|| -> RuntimeHostResult<CatalogGeneration> {
            let generation = catalog.generation().clone();
            let expected_active_hash = previous
                .as_ref()
                .map(|value| value.catalog_hash().to_owned());
            let data = CatalogTransitionEventData {
                catalog_id: generation.catalog_id().to_owned(),
                catalog_version: generation.catalog_version(),
                catalog_hash: generation.catalog_hash().to_owned(),
                previous_catalog_hash: previous
                    .as_ref()
                    .map(|value| value.catalog_hash().to_owned()),
                promotion,
            };
            let links = self.events.system_links()?;
            let intent = self.events.draft(
                EventSeverity::Info,
                EventSource::Runtime,
                OriginModule::Policy,
                EventActor::Runtime,
                links.clone(),
                CatalogPayloadDraft::transition_intent(action, data.clone(), AuditInput::new()),
            )?;
            let intent = self.events.sanitize(intent)?;
            let plan = CriticalEventPlan::new(CriticalOperation::CatalogTransition(target), intent)
                .map_err(|_| critical_plan_error())?;
            let mut policy = lock(&self.policy, "switch_active_policy_catalog")?;
            if let Some(error) = self.fatal.current()? {
                return Err(error);
            }
            let intent = self
                .ledger
                .append(plan.intent().clone())
                .map_err(|error| crate::policy_host::catalog_ledger_error(&error))?;
            let work = policy.prepare_active_transaction(&catalog, expected_active_hash.as_deref());
            let success = self.events.draft(
                EventSeverity::Info,
                EventSource::Runtime,
                OriginModule::Policy,
                EventActor::Runtime,
                links.clone(),
                match target {
                    CatalogTransitionTarget::Activated => {
                        CatalogPayloadDraft::activated(data.clone(), AuditInput::new())
                    }
                    CatalogTransitionTarget::RolledBack => {
                        CatalogPayloadDraft::rolled_back(data.clone(), AuditInput::new())
                    }
                },
            )?;
            let success = self.events.sanitize(success)?;
            actingcommand_ledger::critical::validate_catalog_outcome(
                target, &intent, &success, true,
            )
            .map_err(|_| critical_plan_error())?;
            let outcome = match self.ledger.append_transaction(success, work) {
                Ok(outcome) => outcome,
                Err(error) => {
                    let Some(rejection) = error.rolled_back_work() else {
                        let error = crate::policy_host::catalog_ledger_error(&error);
                        self.fatal.mark(error.clone())?;
                        return Err(error);
                    };
                    let original = if rejection.fatal {
                        RuntimeHostError::fatal(
                            rejection.code,
                            rejection.operation,
                            RuntimeErrorCode::RuntimeFatal,
                        )
                    } else {
                        RuntimeHostError::request(
                            rejection.code,
                            rejection.operation,
                            RuntimeErrorCode::InvalidRequest,
                        )
                    }
                    .with_native_detail(rejection.detail.clone());
                    let failed = self
                        .events
                        .draft(
                            EventSeverity::Error,
                            EventSource::Runtime,
                            OriginModule::Policy,
                            EventActor::Runtime,
                            links,
                            CatalogPayloadDraft::transition_failed(
                                action,
                                data,
                                EffectDisposition::NotPerformed,
                                AuditInput::new(),
                            ),
                        )
                        .and_then(|draft| self.events.sanitize(draft));
                    let failed = match failed {
                        Ok(draft) => draft,
                        Err(error) => {
                            let error = RuntimeHostError::fatal(
                                "catalog_failure_fact_undurable",
                                "build_catalog_failure",
                                RuntimeErrorCode::LedgerFailure,
                            )
                            .with_native_detail(format!(
                                "original={original:?}; failure={error:?}"
                            ));
                            self.fatal.mark(error.clone())?;
                            return Err(error);
                        }
                    };
                    actingcommand_ledger::critical::validate_catalog_outcome(
                        target, &intent, &failed, false,
                    )
                    .map_err(|error| {
                        RuntimeHostError::fatal(
                            "catalog_failure_fact_undurable",
                            "validate_catalog_failure",
                            RuntimeErrorCode::LedgerFailure,
                        )
                        .with_native_detail(format!("original={original:?}; failure={error:?}"))
                    })?;
                    let failed = self.ledger.append(failed).map_err(|error| {
                        crate::policy_host::catalog_ledger_error(&error).with_native_detail(
                            format!(
                                "original={original:?}; failure={error}; detail={:?}",
                                error.detail()
                            ),
                        )
                    });
                    drop(policy);
                    match failed {
                        Ok(event) => {
                            if original.is_fatal() {
                                self.fatal.mark(original.clone())?;
                            }
                            if let Err(error) = self
                                .synchronize_fact_store()
                                .and_then(|()| self.observe_pipeline_event(&event))
                            {
                                return Err(error.into_fatal().with_native_detail(format!(
                                    "original={original:?}; failed outcome was committed"
                                )));
                            }
                            return Err(original);
                        }
                        Err(error) => {
                            self.fatal.mark(error.clone())?;
                            return Err(error);
                        }
                    }
                }
            };
            policy.publish_active(catalog);
            drop(policy);
            let post = self
                .synchronize_fact_store()
                .and_then(|()| self.observe_pipeline_event(&outcome));
            if let Err(error) = post {
                let error = error.into_fatal();
                self.fatal.mark(error.clone())?;
                return Err(error);
            }
            Ok(generation)
        })();
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.fatal.mark(error.clone())?;
        }
        result
    }
}
