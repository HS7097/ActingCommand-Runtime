// SPDX-License-Identifier: AGPL-3.0-only

use super::online_observation::{
    observation_admission_error, observation_artifact_failure, observation_integrity_failure,
    observation_kernel_error,
};
use super::*;
use crate::codes::{HostCode, HostLocation};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::page_projection::{FrameIdentity, FrameKind};
use actingcommand_contract::{
    ContainedLabOperationRequest, ContainedLabOperationResult, ContainedObservationEvidence,
    ContainedPageObservation, LAB_ARRIVAL_INTERVAL_MS, LAB_OPERATION_PREPARED_SCHEMA,
    LAB_OPERATION_TERMINAL_SCHEMA, LabArrivalResult, LabArrivalSample, LabArrivalStatus,
    LabEvidenceReference, LabOperationFailure, LabOperationFrame, LabOperationPrepared,
    LabOperationRecord, LabOperationStage, MAX_LAB_ARRIVAL_FRAMES, ONLINE_OBSERVATION_SCHEMA,
    PageObservationStatus,
};
use actingcommand_execution_kernel::{ObservationMode, PreparedPageObservation};

impl HostShared {
    pub(super) fn run_contained_lab_operation(
        &self,
        original: &RuntimeRequest,
        request: &ValidatedRuntimeRequest<'_>,
        instance_alias: &str,
        holder_id: actingcommand_contract::HolderId,
        input: &ContainedLabOperationRequest,
        connection_id: ConnectionId,
    ) -> Result<OperationSuccess, RequestFailure> {
        let resolved = self.resolve_instance(instance_alias)?;
        let run_links = lock(&self.debug_runs, HostLocation::ReadLabOperationRun)?
            .get(&request.correlation_id())
            .map(|run| RuntimeRunLinks::new(run.task_id, run.run_id));
        self.append_request_lifecycle(
            original,
            request,
            resolved.instance_id(),
            RuntimeDebugOperation::Do.event_action(),
            run_links,
        )?;
        let observer = (|| {
            let path = Path::new(&input.package_path);
            if !path.is_absolute() {
                return Err(observation_admission_error(
                    HostCode::ObservationPathNotAbsolute,
                    HostLocation::OpenLabOperationPackage,
                    "absolute package path required",
                ));
            }
            // Workflow #336 PKG-S1: the path chooses the loader; the request's reference is
            // recorded and compared with nothing.
            let deadline = Instant::now()
                .checked_add(Duration::from_millis(
                    ContainedTaskRequest::DEFAULT_RESPONSE_DEADLINE_MS,
                ))
                .ok_or_else(|| {
                    observation_admission_error(
                        HostCode::LabPackageDeadlineOverflow,
                        HostLocation::AdmitLabOperation,
                        "admission deadline overflow",
                    )
                })?;
            // Workflow #336 PKG-S4: the request names its package by its path only.
            let observer = PreparedPageObservation::load_path(
                instance_alias,
                path,
                &[],
                self.execution()?.vision_provider(),
                deadline,
            )
            .map_err(observation_kernel_error)?;
            if input
                .after
                .as_ref()
                .is_some_and(|after| !observer.contains_page(&after.page_id))
            {
                return Err(observation_admission_error(
                    HostCode::LabArrivalPageAbsent,
                    HostLocation::AdmitLabOperation,
                    "arrival page must be an exact page ID in the verified package",
                ));
            }
            Ok(observer)
        })()
        .map_err(|error| {
            self.observation_failure(
                error,
                request.event_links(Some(resolved.instance_id()), None, None),
                RuntimeReceiptState::Denied,
            )
        })?;

        let mut prepared = LabOperationPrepared {
            schema_version: LAB_OPERATION_PREPARED_SCHEMA.to_string(),
            request_id: original.request_id(),
            correlation_id: request.correlation_id(),
            instance_id: resolved.instance_id(),
            package_label: observer.package_label(),
            lease_id: None,
            selection: input.selection.clone(),
            projection_hint: input.projection_hint.clone(),
            after: input.after.clone(),
            before_frame: None,
            before_projection: None,
            selected_element: None,
            geometry: None,
            action: None,
        };
        let mut token = None;
        let mut stage = LabOperationStage::Lease;
        let preparation = (|| -> Result<(), RequestFailure> {
            let lease = self.acquire_lease(RuntimeLeaseAcquisition {
                request,
                request_id: original.request_id(),
                instance_alias,
                holder_id,
                connection_id,
                run_links,
                lease_ttl_ms: None,
                kind: ClaimKind::LabOperation,
            })?;
            let RuntimeResult::LeaseGranted { token: acquired } = lease.result else {
                return Err(observation_integrity_failure(
                    HostCode::LabOperationLeaseResultInvalid,
                ));
            };
            prepared.lease_id = Some(acquired.lease_id());
            token = Some(acquired);
            let held = token.as_ref().expect("lease acquired");
            stage = LabOperationStage::BeforeFrame;
            // #411 Lab L5: the before observation is one transaction of the frames the
            // package's target consensus needs, the last its frame.
            let LabObservationTransaction {
                samples,
                frame,
                fence,
            } = self.capture_lab_operation_transaction(
                request,
                held,
                connection_id,
                run_links,
                observer.frame_plan(),
            )?;
            prepared.before_frame = Some(LabOperationFrame {
                observation: frame.observation.clone(),
                verified: frame.verified,
                lease_valid_after_capture: fence.is_ok(),
            });
            fence?;
            stage = LabOperationStage::BeforeProjection;
            let projection = self.evaluate_lab_operation_frame(
                original,
                request,
                &observer,
                &samples,
                &frame,
                resolved.instance_id(),
            )?;
            prepared.before_projection = Some(projection);
            stage = LabOperationStage::Selection;
            let projection = &prepared
                .before_projection
                .as_ref()
                .expect("projection evaluated")
                .projection;
            let selected = input.selection.resolve(projection).map_err(|error| {
                RequestFailure::request(
                    RuntimeHostError::request(
                        error.code(),
                        HostLocation::ResolveCurrentLabOperation,
                        RuntimeErrorCode::RecognitionFailed,
                    ),
                    RuntimeReceiptState::Denied,
                    None,
                )
            })?;
            prepared.selected_element = selected.0;
            prepared.geometry = Some(selected.1);
            prepared.action = Some(selected.2);
            Ok(())
        })();
        // Workflow #378 A4 (coordinator ruling A-M2): the receipt outcome of the operation's
        // failure, which tops its receipt.
        let (failure, mut failure_outcome) = match preparation {
            Ok(()) => None,
            Err(failure) => Some(lab_operation_failure(stage, failure)?),
        }
        .unzip();
        let mut links = request.event_links(Some(resolved.instance_id()), prepared.lease_id, None);
        let mut artifact_links = request.artifact_links();
        if let Some(run_links) = run_links {
            links = run_links.apply(links);
            artifact_links = artifact_links.with_run_id(run_links.run_id);
        }
        let bytes = serde_json::to_vec(&prepared)
            .map_err(|_| observation_integrity_failure(HostCode::LabPreparedEncodeFailed))?;
        let (artifact, verified) =
            self.persist_observation_artifact(&bytes, artifact_links.clone(), links.clone())?;
        let mut record = LabOperationRecord {
            schema_version: LAB_OPERATION_TERMINAL_SCHEMA.to_string(),
            prepared,
            prepared_artifact: LabEvidenceReference {
                artifact,
                verified: terminal(&verified),
            },
            input_returned: false,
            input_action_id: None,
            input_intent: None,
            input_event: None,
            effect: EffectDisposition::NotPerformed,
            after_frame: None,
            after_projection: None,
            arrival: input.after.as_ref().map(|_| LabArrivalResult {
                status: LabArrivalStatus::Aborted,
                elapsed_ms: 0,
                samples: Vec::new(),
            }),
            failure,
            cleanup_failure: None,
        };
        if record.failure.is_none() {
            let held = token.as_ref().expect("prepared action holds lease");
            // No instance guard or destructive step is held across this owner call.
            let (input_terminal, input_failure) = match self.input(
                request,
                held,
                record.prepared.action.as_ref().expect("prepared action"),
                connection_id,
                resolved.provenance(),
                RuntimeInputContext {
                    run_links,
                    source_step_action_id: None,
                    before_frame_id: record
                        .prepared
                        .before_frame
                        .as_ref()
                        .and_then(|frame| frame.observation.artifact().frame_id().copied()),
                    input_frame: record.prepared.before_frame.as_ref().and_then(|frame| {
                        frame
                            .observation
                            .artifact()
                            .frame_id()
                            .copied()
                            .map(|frame_id| actingcommand_contract::InputFrameReference {
                                frame_id,
                                width: frame.observation.width(),
                                height: frame.observation.height(),
                            })
                    }),
                    ..RuntimeInputContext::default()
                },
            ) {
                Ok((success, _)) => (success.terminal, None),
                Err(failure) if failure.poison_runtime || failure.error.is_fatal() => {
                    return Err(failure);
                }
                Err(failure) => (failure.terminal, Some(failure)),
            };
            record.input_returned = true;
            record.effect = EffectDisposition::Indeterminate;
            if let Err(mut evidence_failure) = self.resolve_lab_input_evidence(
                &mut record,
                input_terminal,
                input_failure.is_some(),
            ) {
                if let Some(original_failure) = input_failure {
                    evidence_failure.error =
                        Box::new(evidence_failure.error.as_ref().clone().with_native_detail(
                            format!(
                                "input_failure={}; evidence_failure={}",
                                original_failure.error.code(),
                                evidence_failure.error.code(),
                            ),
                        ));
                    evidence_failure.terminal = original_failure.terminal;
                }
                return Err(evidence_failure);
            }
            (record.failure, failure_outcome) = match input_failure {
                Some(failure) => Some(lab_operation_failure(LabOperationStage::Input, failure)?),
                None if record.input_event.is_none() => Some(lab_operation_failure(
                    LabOperationStage::Input,
                    RequestFailure::request(
                        RuntimeHostError::request(
                            HostCode::LabInputOutcomeMissing,
                            HostLocation::ResolveLabInputEvidence,
                            RuntimeErrorCode::BackendOperationFailed,
                        ),
                        RuntimeReceiptState::Failed,
                        None,
                    ),
                )?),
                None => None,
            }
            .unzip();
            if record.failure.is_none() {
                let started = self.monotonic_ms()?;
                let elapsed = |now: u64| {
                    now.checked_sub(started).ok_or_else(|| {
                        observation_integrity_failure(HostCode::LabArrivalClockRegressed)
                    })
                };
                let deadline = input
                    .after
                    .as_ref()
                    .map(|after| {
                        started.checked_add(after.timeout_ms).ok_or_else(|| {
                            observation_integrity_failure(HostCode::LabArrivalDeadlineOverflow)
                        })
                    })
                    .transpose()?;
                let post = (|| -> Result<(), RequestFailure> {
                    let captures = if deadline.is_some() {
                        MAX_LAB_ARRIVAL_FRAMES
                    } else {
                        1
                    };
                    for index in 0..captures {
                        stage = LabOperationStage::AfterFrame;
                        if let Some(deadline) = deadline {
                            self.validate_lab_arrival_token(request, held, connection_id)?;
                            if self.monotonic_ms()? >= deadline {
                                break;
                            }
                        }
                        // Re-fence the original token, including any safe-boundary transfer.
                        // #411 Lab L5: each after observation is one transaction.
                        let LabObservationTransaction {
                            samples,
                            frame,
                            fence,
                        } = self.capture_lab_operation_transaction(
                            request,
                            held,
                            connection_id,
                            run_links,
                            observer.frame_plan(),
                        )?;
                        record.after_frame = Some(LabOperationFrame {
                            observation: frame.observation.clone(),
                            verified: frame.verified,
                            lease_valid_after_capture: fence.is_ok(),
                        });
                        record.after_projection = None;
                        if let Some(arrival) = &mut record.arrival {
                            arrival.samples.push(LabArrivalSample {
                                frame: record.after_frame.as_ref().expect("captured frame").clone(),
                                projection: None,
                            });
                        }
                        fence?;
                        stage = LabOperationStage::AfterProjection;
                        record.after_projection = Some(self.evaluate_lab_operation_frame(
                            original,
                            request,
                            &observer,
                            &samples,
                            &frame,
                            resolved.instance_id(),
                        )?);
                        if let Some(arrival) = &mut record.arrival {
                            let projection =
                                record.after_projection.as_ref().expect("evaluated frame");
                            arrival
                                .samples
                                .last_mut()
                                .expect("captured sample")
                                .projection = Some(LabEvidenceReference {
                                artifact: projection.artifact.clone(),
                                verified: TerminalEvent {
                                    sequence: projection.projection_sequence,
                                    event_id: projection.projection_event_id,
                                },
                            });
                        }
                        {
                            let instance_guard = self.instance_guard(held.instance_id())?;
                            let _admission =
                                lock(&instance_guard, HostLocation::ValidateLabAfterProjection)?;
                            self.validated_instance(request, held, connection_id)?;
                        }
                        let Some(deadline) = deadline else {
                            return Ok(());
                        };
                        self.validate_lab_arrival_token(request, held, connection_id)?;
                        let now = self.monotonic_ms()?;
                        let arrival = record.arrival.as_mut().expect("arrival requested");
                        arrival.elapsed_ms = elapsed(now)?;
                        let projection = record.after_projection.as_ref().expect("evaluated frame");
                        if now < deadline
                            && projection.status == PageObservationStatus::Recognized
                            && projection.projection.matched
                            && projection.projection.page
                                == input.after.as_ref().expect("arrival requested").page_id
                        {
                            arrival.status = LabArrivalStatus::Reached;
                            return Ok(());
                        }
                        if matches!(
                            projection.status,
                            PageObservationStatus::Partial | PageObservationStatus::Conflict
                        ) {
                            return Err(RequestFailure::request(
                                RuntimeHostError::request(
                                    HostCode::LabArrivalObservationFailed,
                                    HostLocation::WaitLabArrival,
                                    RuntimeErrorCode::RecognitionFailed,
                                ),
                                RuntimeReceiptState::Failed,
                                None,
                            ));
                        }
                        if now >= deadline || index + 1 == captures {
                            break;
                        }
                        // No owner/instance lock is held while waiting on the request thread.
                        thread::sleep(Duration::from_millis(
                            LAB_ARRIVAL_INTERVAL_MS.min(deadline - now),
                        ));
                    }
                    stage = LabOperationStage::Arrival;
                    let arrival = record.arrival.as_mut().expect("bounded arrival ended");
                    arrival.elapsed_ms = elapsed(self.monotonic_ms()?)?;
                    arrival.status = LabArrivalStatus::TimedOut;
                    Err(RequestFailure::request(
                        RuntimeHostError::request(
                            SharedCode::LabArrivalTimeout,
                            HostLocation::WaitLabArrival,
                            RuntimeErrorCode::RecognitionFailed,
                        ),
                        RuntimeReceiptState::Failed,
                        None,
                    ))
                })();
                if let Err(failure) = post {
                    if let Some(arrival) = &mut record.arrival
                        && arrival.status == LabArrivalStatus::Aborted
                        && !failure.poison_runtime
                        && !failure.error.is_fatal()
                    {
                        arrival.elapsed_ms = elapsed(self.monotonic_ms()?)?;
                    }
                    let (failure, outcome) = lab_operation_failure(stage, failure)?;
                    record.failure = Some(failure);
                    failure_outcome = Some(outcome);
                }
            }
        }
        if let Some(held) = token.as_ref() {
            let still_owned = lock(&self.scheduler, HostLocation::ReadLabOperationCleanupLease)?
                .active_tokens()
                .iter()
                .any(|active| active == held);
            if still_owned
                && let Err(failure) = self.release_lease(
                    request,
                    original.request_id(),
                    held,
                    connection_id,
                    run_links,
                )
            {
                let (cleanup, outcome) =
                    lab_operation_failure(LabOperationStage::Release, failure)?;
                if record.failure.is_none() {
                    record.failure = Some(cleanup);
                    failure_outcome = Some(outcome);
                } else {
                    record.cleanup_failure = Some(cleanup);
                }
            }
        }
        let bytes = serde_json::to_vec(&record)
            .map_err(|_| observation_integrity_failure(HostCode::LabTerminalEncodeFailed))?;
        let (artifact, verified) =
            self.persist_observation_artifact(&bytes, artifact_links, links.clone())?;
        let payload = if record.failure.is_some() {
            CommandPayloadDraft::rejected(
                RuntimeDebugOperation::Do.event_action(),
                DiagnosticCode::CommandRejected,
                record.effect,
                AuditInput::new(),
            )
        } else {
            CommandPayloadDraft::validated(
                RuntimeDebugOperation::Do.event_action(),
                record.effect,
                AuditInput::new(),
            )
        };
        let final_event = self.append_event(
            if record.failure.is_some() {
                EventSeverity::Error
            } else {
                EventSeverity::Info
            },
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links,
            payload,
        )?;
        let operation = ContainedLabOperationResult {
            record,
            terminal_artifact: LabEvidenceReference {
                artifact,
                verified: terminal(&verified),
            },
        };
        operation
            .validate()
            .map_err(|_| observation_integrity_failure(HostCode::LabOperationResultInvalid))?;
        Ok(OperationSuccess {
            state: if operation.record.failure.is_some() {
                RuntimeReceiptState::Failed
            } else {
                RuntimeReceiptState::Completed
            },
            terminal: Some(terminal(&final_event)),
            result: RuntimeResult::ContainedLabOperation {
                operation: Box::new(operation),
            },
            failure_outcome,
        })
    }

    fn validate_lab_arrival_token(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        token: &LeaseToken,
        connection_id: ConnectionId,
    ) -> Result<(), RequestFailure> {
        if let Some(error) = self.fatal.answer()? {
            return Err(RequestFailure::poison_without_terminal(error));
        }
        if self.fatal.is_shutdown_requested() {
            return Err(RequestFailure::request(
                RuntimeHostError::request(
                    HostCode::LabArrivalShutdown,
                    HostLocation::WaitLabArrival,
                    RuntimeErrorCode::RuntimeUnavailable,
                ),
                RuntimeReceiptState::Failed,
                None,
            ));
        }
        let instance_guard = self.instance_guard(token.instance_id())?;
        let _admission = lock(&instance_guard, HostLocation::ValidateLabArrivalToken)?;
        self.validated_instance(request, token, connection_id)
            .map(|_| ())
    }

    fn resolve_lab_input_evidence(
        &self,
        record: &mut LabOperationRecord,
        returned_terminal: Option<TerminalEvent>,
        input_failed: bool,
    ) -> Result<(), RequestFailure> {
        let prepared = &record.prepared;
        let identity_matches = |event: &PersistedEvent| {
            event.links().request_id() == Some(&prepared.request_id)
                && event.links().correlation_id() == Some(&prepared.correlation_id)
                && event.links().instance_id() == Some(&prepared.instance_id)
                && event.links().lease_id() == prepared.lease_id.as_ref()
        };
        // Input has returned. These bounded native queries never select by proximity or latest.
        let intents = self
            .ledger
            .query_page(
                EventQuery {
                    event_type: Some(EventType::InputIntent),
                    request_id: Some(prepared.request_id),
                    correlation_id: Some(prepared.correlation_id),
                    instance_id: Some(prepared.instance_id),
                    lease_id: prepared.lease_id,
                    ..EventQuery::default()
                },
                0,
                u64::MAX,
                2,
            )
            .map_err(|error| {
                RequestFailure::poison_without_terminal(
                    ledger_error(HostLocation::QueryLabInputIntent)
                        .with_native_detail(error.to_string()),
                )
            })?;
        if intents.len() > 1 {
            return Err(observation_integrity_failure(
                HostCode::LabInputIntentNotUnique,
            ));
        }
        let mut outcome = None;
        if let Some(intent) = intents.first() {
            let action_id = *intent
                .links()
                .action_id()
                .ok_or_else(|| observation_integrity_failure(HostCode::LabInputActionMissing))?;
            if !identity_matches(intent)
                || intent.sequence() <= record.prepared_artifact.verified.sequence
                || Some(intent.payload().action())
                    != prepared.action.as_ref().map(InputAction::event_action)
            {
                return Err(observation_integrity_failure(
                    HostCode::LabInputIntentMismatch,
                ));
            }
            record.input_action_id = Some(action_id);
            record.input_intent = Some(terminal(intent));
            for event_type in [EventType::InputCommitted, EventType::InputFailed] {
                let events = self
                    .ledger
                    .query_page(
                        EventQuery {
                            event_type: Some(event_type),
                            action_id: Some(action_id),
                            ..EventQuery::default()
                        },
                        0,
                        u64::MAX,
                        2,
                    )
                    .map_err(|error| {
                        RequestFailure::poison_without_terminal(
                            ledger_error(HostLocation::QueryLabInputOutcome)
                                .with_native_detail(error.to_string()),
                        )
                    })?;
                if events.len() > 1 || (!events.is_empty() && outcome.is_some()) {
                    return Err(observation_integrity_failure(
                        HostCode::LabInputOutcomeConflict,
                    ));
                }
                if let Some(event) = events.into_iter().next() {
                    if !identity_matches(&event)
                        || event.sequence() <= intent.sequence()
                        || event.payload().action() != intent.payload().action()
                    {
                        return Err(observation_integrity_failure(
                            HostCode::LabInputOutcomeMismatch,
                        ));
                    }
                    outcome = Some(event);
                }
            }
        }
        if let Some(reference) = returned_terminal {
            let events = self
                .ledger
                .query_page(
                    EventQuery {
                        from_sequence: Some(reference.sequence),
                        to_sequence: Some(reference.sequence),
                        ..EventQuery::default()
                    },
                    0,
                    reference.sequence,
                    2,
                )
                .map_err(|error| {
                    RequestFailure::poison_without_terminal(
                        ledger_error(HostLocation::QueryLabReturnedInputTerminal)
                            .with_native_detail(error.to_string()),
                    )
                })?;
            let [event] = events.as_slice() else {
                return Err(observation_integrity_failure(
                    HostCode::LabReturnedInputTerminalMissing,
                ));
            };
            if terminal(event) != reference
                || !identity_matches(event)
                || (matches!(
                    event.event_type(),
                    EventType::InputCommitted | EventType::InputFailed
                ) && outcome
                    .as_ref()
                    .is_none_or(|outcome| terminal(outcome) != reference))
                || (!input_failed && event.event_type() != EventType::InputCommitted)
            {
                return Err(observation_integrity_failure(
                    HostCode::LabReturnedInputTerminalMismatch,
                ));
            }
        }
        if let Some(outcome) = outcome {
            let effect = outcome
                .payload()
                .effect_disposition()
                .ok_or_else(|| observation_integrity_failure(HostCode::LabInputEffectMissing))?;
            if (outcome.event_type() == EventType::InputCommitted
                && effect != EffectDisposition::Performed)
                || (outcome.event_type() == EventType::InputFailed
                    && (!input_failed || effect == EffectDisposition::Performed))
            {
                return Err(observation_integrity_failure(
                    HostCode::LabInputOutcomeEffectMismatch,
                ));
            }
            record.input_event = Some(terminal(&outcome));
            record.effect = effect;
        }
        Ok(())
    }

    fn capture_lab_operation_frame(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        token: &LeaseToken,
        connection_id: ConnectionId,
        run_links: Option<RuntimeRunLinks>,
    ) -> Result<(CompletedReadonlyObservation, Result<(), RequestFailure>), RequestFailure> {
        let instance_guard = self.instance_guard(token.instance_id())?;
        let admission = lock(&instance_guard, HostLocation::LockLabOperationCapture)?;
        let instance = self.validated_instance(request, token, connection_id)?;
        let frame_id = self
            .events
            .issuer()
            .mint_frame_id()
            .map_err(|_| observation_integrity_failure(HostCode::LabFrameIdIssueFailed))?;
        let recognition_id =
            self.events.issuer().mint_recognition_id().map_err(|_| {
                observation_integrity_failure(HostCode::LabRecognitionIdIssueFailed)
            })?;
        let mut links = request
            .event_links(Some(token.instance_id()), Some(token.lease_id()), None)
            .with_frame_id(frame_id)
            .with_recognition_id(recognition_id);
        let mut artifact_links = request.artifact_links().with_frame_id(frame_id);
        if let Some(run_links) = run_links {
            links = run_links.apply(links);
            artifact_links = artifact_links.with_run_id(run_links.run_id);
        }
        let completed = self.capture_observation_with_links(
            request,
            &instance.instance_alias,
            links,
            artifact_links,
            &admission,
        )?;
        let fence = self
            .validated_instance(request, token, connection_id)
            .map(|_| ());
        Ok((completed, fence))
    }

    /// #411 Lab L5: one observation transaction of the operation under its lease: the plan's
    /// earlier frames, then the observation's frame, the plan's interval apart. Each capture is
    /// fenced before and after as a single frame is; no owner or instance lock is held while
    /// waiting. Answers the earlier frames, oldest first, the observation's frame and its
    /// fence; a fence that fails after an earlier frame ends the transaction.
    fn capture_lab_operation_transaction(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        token: &LeaseToken,
        connection_id: ConnectionId,
        run_links: Option<RuntimeRunLinks>,
        plan: actingcommand_contract::ObservationFramePlan,
    ) -> Result<LabObservationTransaction, RequestFailure> {
        let mut samples = Vec::with_capacity(plan.frames() - 1);
        for _ in 1..plan.frames() {
            let (sample, fence) =
                self.capture_lab_operation_frame(request, token, connection_id, run_links)?;
            fence?;
            samples.push(sample);
            self.kernel_pause(Duration::from_millis(plan.interval_ms()));
        }
        let (frame, fence) =
            self.capture_lab_operation_frame(request, token, connection_id, run_links)?;
        Ok(LabObservationTransaction {
            samples,
            frame,
            fence,
        })
    }

    fn evaluate_lab_operation_frame(
        &self,
        original: &RuntimeRequest,
        request: &ValidatedRuntimeRequest<'_>,
        observer: &PreparedPageObservation,
        samples: &[CompletedReadonlyObservation],
        captured: &CompletedReadonlyObservation,
        instance_id: InstanceId,
    ) -> Result<ContainedPageObservation, RequestFailure> {
        let mut pngs = Vec::with_capacity(samples.len() + 1);
        for frame in samples.iter().chain(std::iter::once(captured)) {
            pngs.push(
                read_projected_verified(self.artifacts.root(), frame.observation.artifact())
                    .map_err(observation_artifact_failure)?,
            );
        }
        let frames = pngs.iter().map(Vec::as_slice).collect::<Vec<_>>();
        let recognition_id = self
            .events
            .issuer()
            .mint_recognition_id()
            .map_err(|_| observation_integrity_failure(HostCode::LabProjectionIdIssueFailed))?;
        let links = captured.links.clone().with_recognition_id(recognition_id);
        self.append_event(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Recognition,
            EventActor::Runtime,
            links.clone(),
            RecognitionPayloadDraft::requested(EventAction::RecognitionObserve, AuditInput::new()),
        )?;
        let evaluated = observer.evaluate_frames(
            &frames,
            FrameIdentity {
                kind: FrameKind::Artifact,
                sha256: captured
                    .observation
                    .artifact()
                    .sha256
                    .strip_prefix("sha256:")
                    .ok_or_else(|| observation_integrity_failure(HostCode::LabFrameHashInvalid))?
                    .to_string(),
                width: captured.observation.width(),
                height: captured.observation.height(),
            },
            // #411 Lab L5: a Lab operation's observations are page observations.
            ObservationMode::Page,
        );
        let evaluated = self.archive_online_ppocr_result(
            evaluated,
            links.clone(),
            captured.artifact_links.clone(),
            "lab_page",
        )?;
        let evidence = ContainedObservationEvidence {
            schema_version: ONLINE_OBSERVATION_SCHEMA.to_string(),
            request_id: original.request_id(),
            correlation_id: request.correlation_id(),
            instance_id,
            package_label: observer.package_label(),
            frame: captured.observation.clone(),
            rgb8_sha256: evaluated.rgb8_sha256,
            status: evaluated.status,
            projection: evaluated.projection,
            facts: evaluated.facts,
            private_facts: evaluated.private_facts,
        };
        let bytes = serde_json::to_vec(&evidence)
            .map_err(|_| observation_integrity_failure(HostCode::LabProjectionEncodeFailed))?;
        let (artifact, verified) = self.persist_observation_artifact(
            &bytes,
            captured.artifact_links.clone(),
            links.clone(),
        )?;
        let incomplete = matches!(
            evidence.status,
            PageObservationStatus::Partial | PageObservationStatus::Conflict
        );
        let payload = if incomplete {
            RecognitionPayloadDraft::failed(
                EventAction::RecognitionObserve,
                DiagnosticCode::RecognitionFailed,
                EffectDisposition::Performed,
                AuditInput::new(),
            )
        } else {
            RecognitionPayloadDraft::completed(
                EventAction::RecognitionObserve,
                EffectDisposition::Performed,
                evidence.frame.width(),
                evidence.frame.height(),
                if evidence.status == PageObservationStatus::Recognized {
                    RecognitionVerdict::PageMatched
                } else {
                    RecognitionVerdict::PageUnmatched
                },
                AuditInput::new(),
            )
        };
        self.append_event(
            if incomplete {
                EventSeverity::Error
            } else {
                EventSeverity::Info
            },
            EventSource::Runtime,
            OriginModule::Recognition,
            EventActor::Runtime,
            links,
            payload,
        )?;
        Ok(ContainedPageObservation {
            instance_id,
            package_label: evidence.package_label,
            frame: evidence.frame,
            status: evidence.status,
            projection: evidence.projection,
            facts: evidence.facts,
            artifact,
            projection_sequence: verified.sequence(),
            projection_event_id: *verified.event_id(),
        })
    }
}

/// #411 Lab L5: one observation transaction of a Lab operation: its earlier frames, oldest first,
/// the observation's frame, and whether the token was still valid after that frame's capture.
struct LabObservationTransaction {
    samples: Vec<CompletedReadonlyObservation>,
    frame: CompletedReadonlyObservation,
    fence: Result<(), RequestFailure>,
}

/// A failed stage of a Lab operation: its record entry, and the host's failure as the receipt
/// outcome that tops the operation's receipt when it is the operation's failure (Workflow #378
/// A4, coordinator ruling A-M2).
fn lab_operation_failure(
    stage: LabOperationStage,
    failure: RequestFailure,
) -> Result<(LabOperationFailure, Outcome), RequestFailure> {
    if failure.poison_runtime || failure.error.is_fatal() {
        return Err(failure);
    }
    Ok((
        LabOperationFailure {
            stage,
            code: failure.error.code().to_string(),
            error: failure.error.projection().clone(),
            event: failure.terminal,
        },
        failure.error.receipt_outcome(),
    ))
}
