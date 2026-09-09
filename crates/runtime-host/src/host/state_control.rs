// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

impl HostShared {
    pub(super) fn stage_release_set(
        &self,
        manifest: RuntimeReleaseSet,
        sources: &ReleaseArtifactSources,
    ) -> RuntimeHostResult<RuntimeReleaseSet> {
        let result: RuntimeHostResult<RuntimeReleaseSet> = (|| {
            let _gate = lock(&self.state_write_gate, "stage_release_set")?;
            let staged = self
                .state
                .stage_release(manifest, sources)
                .map_err(|error| RuntimeHostError::state(&error))?;
            let manifest = staged.manifest().clone();
            if !ledger_has_release_stage(&self.ledger, manifest.release_id())? {
                self.append_event_raw(
                    EventSeverity::Info,
                    EventSource::Runtime,
                    OriginModule::Runtime,
                    EventActor::Runtime,
                    self.events.system_links()?,
                    ReleasePayloadDraft::staged(manifest.clone(), AuditInput::new()),
                )?;
            }
            Ok(manifest)
        })();
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.fatal.mark(error.clone())?;
        }
        result
    }

    pub(super) fn active_release_set(&self) -> RuntimeHostResult<Option<RuntimeReleaseSet>> {
        self.state
            .active_release()
            .map(|active| active.map(|release| release.manifest().clone()))
            .map_err(|error| RuntimeHostError::state(&error))
    }

    pub(super) fn switch_release_set(
        &self,
        kind: ReleaseTransitionKind,
        release_id: &str,
    ) -> RuntimeHostResult<RuntimeReleaseSet> {
        let _gate = lock(&self.state_write_gate, "switch_release_set")?;
        let preview = self
            .state
            .preview_release_transition(kind, release_id)
            .map_err(|error| RuntimeHostError::state(&error))?;
        let transition = preview.data().clone();
        let links = self.events.system_links()?;
        let intent = self.events.draft(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links.clone(),
            ReleasePayloadDraft::transition_intent(transition.clone(), AuditInput::new()),
        )?;
        let intent = self.events.sanitize(intent)?;
        let target = match kind {
            ReleaseTransitionKind::Activate => ReleaseTransitionTarget::Activated,
            ReleaseTransitionKind::Rollback => ReleaseTransitionTarget::RolledBack,
        };
        let plan = CriticalEventPlan::new(CriticalOperation::ReleaseTransition(target), intent)
            .map_err(|_| critical_plan_error())?;
        let success_links = links.clone();
        let failure_links = links;
        let success_transition = transition.clone();
        let failure_transition = transition;
        let result = execute_critical(
            &self.ledger,
            self.events.fingerprinter(),
            plan,
            || match self
                .state
                .commit_release_transition(&preview)
                .map_err(|error| RuntimeHostError::state(&error))
            {
                Ok(active) => CriticalActionReport::Succeeded {
                    value: active,
                    effect: DefiniteEffectDisposition::Performed,
                },
                Err(error) => CriticalActionReport::Failed {
                    effect: if error.is_fatal() {
                        EffectDisposition::Indeterminate
                    } else {
                        EffectDisposition::NotPerformed
                    },
                    error,
                },
            },
            |_, _| {
                self.events
                    .draft(
                        EventSeverity::Info,
                        EventSource::Runtime,
                        OriginModule::Runtime,
                        EventActor::Runtime,
                        success_links,
                        match target {
                            ReleaseTransitionTarget::Activated => ReleasePayloadDraft::activated(
                                success_transition,
                                AuditInput::new(),
                            ),
                            ReleaseTransitionTarget::RolledBack => {
                                ReleasePayloadDraft::rolled_back(
                                    success_transition,
                                    AuditInput::new(),
                                )
                            }
                        },
                    )
                    .map_err(|_| actingcommand_contract::SanitizationError::fingerprinter_failure())
            },
            |_, effect| {
                self.events
                    .draft(
                        EventSeverity::Error,
                        EventSource::Runtime,
                        OriginModule::Runtime,
                        EventActor::Runtime,
                        failure_links,
                        ReleasePayloadDraft::transition_failed(
                            failure_transition,
                            effect,
                            AuditInput::new(),
                        ),
                    )
                    .map_err(|_| actingcommand_contract::SanitizationError::fingerprinter_failure())
            },
        );
        match result {
            Ok(receipt) => Ok(receipt.into_value().manifest().clone()),
            Err(CriticalExecutionError::Action { error, .. }) => {
                if error.is_fatal() {
                    self.fatal.mark(error.clone())?;
                }
                Err(error)
            }
            Err(error) => {
                let error = critical_execution_error(&error);
                self.fatal.mark(error.clone())?;
                Err(error)
            }
        }
    }
}

fn ledger_has_release_stage(ledger: &GlobalLedger, release_id: &str) -> RuntimeHostResult<bool> {
    Ok(
        release_event_identities(ledger, EventType::ReleaseStaged, |payload| match payload {
            ReleasePayload::Staged(value) => Some(value.manifest().release_id().to_owned()),
            _ => None,
        })?
        .contains(release_id),
    )
}
