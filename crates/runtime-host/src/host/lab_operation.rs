// SPDX-License-Identifier: AGPL-3.0-only

use super::contained_task::contained_task_sampling_seed;
use super::online_observation::{
    observation_admission_error, observation_artifact_failure, observation_integrity_failure,
    observation_kernel_error,
};
use super::*;
use crate::codes::{HostCode, HostLocation};
use actingcommand_contract::codes::{RuntimeContractCode, SharedCode};
use actingcommand_contract::outcome::{Code, Outcome};
use actingcommand_contract::page_projection::{FrameIdentity, FrameKind, PageProjection};
use actingcommand_contract::{
    ContainedLabOperationRequest, ContainedLabOperationResult, ContainedObservationEvidence,
    ContainedPageObservation, InputFrameReference, InputSamplingEvidence, InputSamplingRegion,
    LAB_ARRIVAL_INTERVAL_MS, LAB_OPERATION_PREPARED_SCHEMA, LAB_OPERATION_TERMINAL_SCHEMA,
    LabArrivalResult, LabArrivalSample, LabArrivalStatus, LabDeclaredOperation,
    LabDeclaredOperationResult, LabEvidenceReference, LabGuardOutcome, LabOperationAttempt,
    LabOperationEnd, LabOperationFailure, LabOperationFrame, LabOperationFrameEntry,
    LabOperationPrepared, LabOperationRecord, LabOperationSelection, LabOperationStage,
    LabTargetType, MAX_INPUT_DURATION_MS, MAX_LAB_ARRIVAL_FRAMES,
    MAX_LAB_DECLARED_OPERATION_FRAMES, ONLINE_OBSERVATION_SCHEMA, PageObservationStatus,
};
use actingcommand_execution_kernel::{
    ContainedTaskCode, ContainedTaskError, ContainedTaskGuardOutcome, DeclaredOperation,
    DeclaredOperationOutcome, FrameOwner, ObservationMode, PreparedPageObservation, StepPacing,
    TouchSampling,
};

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
        // LR-10 (#412 B-3 S2, design §2.3): an operation the package declares runs as the kernel
        // runs it; element and coordinate selections take the path below.
        if let LabOperationSelection::Operation { operation_id, seed } = &input.selection {
            return self.run_lab_declared_operation(
                LabOperationContext {
                    original,
                    request,
                    instance_alias,
                    holder_id,
                    connection_id,
                    run_links,
                    instance_id: resolved.instance_id(),
                    provenance: resolved.provenance(),
                },
                input,
                operation_id,
                *seed,
            );
        }
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
            // LR-10 (#412 B-3): an element or coordinate selection states no declared operation.
            operation: None,
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
                    selection_failure(&input.selection, error.code(), projection, &observer),
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
            operation_result: None,
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
                        // #411 A18 option ④: the instant this arrival sample starts.
                        let mut sample_started = None;
                        if let Some(deadline) = deadline {
                            self.validate_lab_arrival_token(request, held, connection_id)?;
                            let now = self.monotonic_ms()?;
                            if now >= deadline {
                                break;
                            }
                            sample_started = Some(now);
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
                        // #411 A18 option ④: the samples start one interval apart, start to
                        // start: the wait is what is left of the interval after this sample's
                        // capture and evaluation (none when they took longer), never past the
                        // deadline, on the Runtime clock as the transaction's own interval.
                        let next_start = sample_started
                            .expect("an arrival sample starts on the clock")
                            .saturating_add(LAB_ARRIVAL_INTERVAL_MS)
                            .min(deadline);
                        // No owner/instance lock is held while waiting on the request thread.
                        self.kernel_pause(Duration::from_millis(next_start.saturating_sub(now)));
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
        self.finish_lab_operation(
            LabOperationContext {
                original,
                request,
                instance_alias,
                holder_id,
                connection_id,
                run_links,
                instance_id: resolved.instance_id(),
                provenance: resolved.provenance(),
            },
            token.as_ref(),
            (links, artifact_links),
            record,
            failure_outcome,
        )
    }

    /// The end of a Lab operation of either mode: the release of its lease (a release that fails
    /// is the operation's failure, or its cleanup failure after another one), its terminal record,
    /// the request's final command event and the result's own validation.
    fn finish_lab_operation(
        &self,
        context: LabOperationContext<'_>,
        token: Option<&LeaseToken>,
        (links, artifact_links): (EventLinksDraft, ArtifactLinksDraft),
        mut record: LabOperationRecord,
        mut failure_outcome: Option<Outcome>,
    ) -> Result<OperationSuccess, RequestFailure> {
        let LabOperationContext {
            original,
            request,
            connection_id,
            run_links,
            ..
        } = context;
        if let Some(held) = token {
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

    /// LR-10 (#412 B-3 S2, design §2.3 "Flow"): the Lab operation that runs `operation_id`, an
    /// operation of its package's entry task, as the kernel runs it, over the Lab lease; attempt
    /// 1 draws its point from `seed` when one is given (decision Q9).
    ///
    /// 1. **Admit.** The kernel admits the package with the Runtime's provider and the declared
    ///    operation (`PreparedContainedTask::declared_operation`); a refusal is a Denied receipt
    ///    with no record. The page observation that projects the last frame is admitted too.
    /// 2. **Lease.** It lasts the package's task timeout and the longest input (the run's bound,
    ///    design §2.4), plus the heartbeat reserve, as a run's lease.
    /// 3. **Run.** `run_declared_operation` over [`LabOperationRuntime`]; attempt 1's effect
    ///    intent persists and verifies the prepared record before its input.
    /// 4. **After projection.** Once the operation completed, its last frame is projected, after
    ///    the run, so no recognition comes between the kernel's decision and its input.
    /// 5. **Record.** The terminal record states the end, every attempt and every frame; then the
    ///    lease's release, the final command event and the result's validation.
    fn run_lab_declared_operation(
        &self,
        context: LabOperationContext<'_>,
        input: &ContainedLabOperationRequest,
        operation_id: &str,
        seed: Option<u64>,
    ) -> Result<OperationSuccess, RequestFailure> {
        let LabOperationContext {
            original,
            request,
            instance_alias,
            holder_id,
            connection_id,
            run_links,
            instance_id,
            provenance,
        } = context;
        let admitted = (|| {
            let path = Path::new(&input.package_path);
            if !path.is_absolute() {
                return Err(observation_admission_error(
                    HostCode::ObservationPathNotAbsolute,
                    HostLocation::OpenLabOperationPackage,
                    "absolute package path required",
                ));
            }
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
            let provider = self.execution()?.vision_provider();
            let task =
                PreparedContainedTask::load_path(instance_alias, path, provider.clone(), deadline)
                    .map_err(lab_declared_admission_error)?;
            let declared = task
                .declared_operation(operation_id)
                .map_err(lab_declared_admission_error)?;
            let observer =
                PreparedPageObservation::load_path(instance_alias, path, &[], provider, deadline)
                    .map_err(observation_kernel_error)?;
            Ok((task, declared, observer))
        })();
        let (task, declared, observer) = admitted.map_err(|error| {
            self.observation_failure(
                error,
                request.event_links(Some(instance_id), None, None),
                RuntimeReceiptState::Denied,
            )
        })?;
        let reserve = lock(
            &self.scheduler,
            HostLocation::ReadContainedTaskLeaseTtlConfig,
        )?
        .config()
        .maximum_client_heartbeat_interval_ms;
        let lease_ttl_ms = task
            .effective_timing()
            .task_timeout
            .milliseconds
            .saturating_add(MAX_INPUT_DURATION_MS)
            .min(ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS)
            .checked_add(reserve)
            .ok_or_else(|| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    HostCode::ContainedTaskLeaseTtlOverflow,
                    HostLocation::DeriveContainedTaskLeaseTtl,
                    RuntimeErrorCode::RuntimeHalted,
                ))
            })?;
        let mut prepared = LabOperationPrepared {
            schema_version: LAB_OPERATION_PREPARED_SCHEMA.to_string(),
            request_id: original.request_id(),
            correlation_id: request.correlation_id(),
            instance_id,
            package_label: observer.package_label(),
            lease_id: None,
            selection: input.selection.clone(),
            operation: None,
            projection_hint: input.projection_hint.clone(),
            after: None,
            before_frame: None,
            before_projection: None,
            selected_element: None,
            geometry: None,
            action: None,
        };
        let granted = self
            .acquire_lease(RuntimeLeaseAcquisition {
                request,
                request_id: original.request_id(),
                instance_alias,
                holder_id,
                connection_id,
                run_links,
                lease_ttl_ms: Some(lease_ttl_ms),
                kind: ClaimKind::LabOperation,
            })
            .and_then(|lease| match lease.result {
                RuntimeResult::LeaseGranted { token } => Ok(token),
                _ => Err(observation_integrity_failure(
                    HostCode::LabOperationLeaseResultInvalid,
                )),
            });
        prepared.lease_id = granted.as_ref().ok().map(LeaseToken::lease_id);
        let mut links = request.event_links(Some(instance_id), prepared.lease_id, None);
        let mut artifact_links = request.artifact_links();
        if let Some(run_links) = run_links {
            links = run_links.apply(links);
            artifact_links = artifact_links.with_run_id(run_links.run_id);
        }
        let token = match granted {
            Ok(token) => token,
            // No lease, no frame: the records state no declared operation, as LR-10 writes a
            // request that failed before any input.
            Err(failure) => {
                let (failure, failure_outcome) =
                    lab_operation_failure(LabOperationStage::Lease, failure)?;
                let prepared_artifact =
                    self.persist_lab_prepared(&prepared, (links.clone(), artifact_links.clone()))?;
                let record = LabOperationRecord {
                    schema_version: LAB_OPERATION_TERMINAL_SCHEMA.to_string(),
                    prepared,
                    prepared_artifact,
                    input_returned: false,
                    input_action_id: None,
                    input_intent: None,
                    input_event: None,
                    effect: EffectDisposition::NotPerformed,
                    after_frame: None,
                    after_projection: None,
                    arrival: None,
                    operation_result: None,
                    failure: Some(failure),
                    cleanup_failure: None,
                };
                return self.finish_lab_operation(
                    context,
                    None,
                    (links, artifact_links),
                    record,
                    Some(failure_outcome),
                );
            }
        };
        let mut runtime = LabOperationRuntime {
            host: self,
            request,
            token: &token,
            instance_alias,
            instance_id,
            connection_id,
            provenance,
            run_links,
            links: links.clone(),
            artifact_links: artifact_links.clone(),
            declared: &declared,
            request_id: original.request_id(),
            seed,
            first_seed: None,
            prepared,
            prepared_artifact: None,
            prepared_unwritten: false,
            frames: Vec::new(),
            last_capture: None,
            attempts: Vec::new(),
            started_attempts: 0,
            after_input: false,
            recheck_from: None,
            failure_stage: None,
            clock_failure: None,
        };
        let outcome = task.run_declared_operation(&mut runtime, operation_id);
        let recheck_frames = runtime.recheck_frames()?;
        let LabOperationRuntime {
            mut prepared,
            prepared_artifact,
            prepared_unwritten,
            frames,
            last_capture,
            attempts,
            after_input,
            failure_stage,
            ..
        } = runtime;
        // A prepared record that could not be persisted stopped the run before its input; the
        // request fails without a record, as an element operation's does.
        if prepared_unwritten {
            return Err(match outcome {
                Err(
                    ContainedTaskRunError::Boundary(failure)
                    | ContainedTaskRunError::NonfatalOperation(failure),
                ) => failure,
                _ => observation_integrity_failure(HostCode::LabOperationResultInvalid),
            });
        }
        // Design §3.2: a failure before the current attempt's input is at `selection`, after it
        // at `arrival`; a failure of the device or the ledger at the stage it arose.
        let kernel_stage = if after_input {
            LabOperationStage::Arrival
        } else {
            LabOperationStage::Selection
        };
        let (mut end, final_page, outcome_key, failed) = match outcome {
            Ok(DeclaredOperationOutcome::Confirmed { page }) => {
                (LabOperationEnd::Confirmed, Some(page), None, None)
            }
            Ok(DeclaredOperationOutcome::RunCompleted { page, outcome_key }) => {
                (LabOperationEnd::RunCompleted, page, outcome_key, None)
            }
            Ok(DeclaredOperationOutcome::PassedOver { refusal }) => (
                LabOperationEnd::PassedOver,
                None,
                None,
                Some((LabOperationStage::Selection, lab_kernel_failure(&refusal))),
            ),
            Ok(DeclaredOperationOutcome::RecoveryRequested {
                failure_reason,
                after_page,
            }) => (
                LabOperationEnd::RecoveryRequested,
                after_page,
                None,
                Some((kernel_stage, lab_recovery_failure(failure_reason))),
            ),
            Err(ContainedTaskRunError::Task(error)) => (
                LabOperationEnd::Failed,
                None,
                None,
                Some((kernel_stage, lab_kernel_failure(&error))),
            ),
            Err(
                ContainedTaskRunError::Boundary(failure)
                | ContainedTaskRunError::NonfatalOperation(failure),
            ) => (
                LabOperationEnd::Failed,
                None,
                None,
                Some((failure_stage.unwrap_or(kernel_stage), failure)),
            ),
        };
        let (mut failure, mut failure_outcome) = match failed {
            Some((stage, failed)) => {
                let (failure, outcome) = lab_operation_failure(stage, failed)?;
                (Some(failure), Some(outcome))
            }
            None => (None, None),
        };
        let mut after_projection = None;
        if failure.is_none()
            && let Some(captured) = &last_capture
            && frames
                .last()
                .is_some_and(|entry| entry.frame.lease_valid_after_capture)
        {
            let projected = self
                .evaluate_lab_operation_frame(
                    original,
                    request,
                    &observer,
                    &[],
                    captured,
                    instance_id,
                )
                .and_then(|projection| {
                    // As an element operation's after observation: the token holds after it.
                    let instance_guard = self.instance_guard(token.instance_id())?;
                    let _admission =
                        lock(&instance_guard, HostLocation::ValidateLabAfterProjection)?;
                    self.validated_instance(request, &token, connection_id)?;
                    Ok(projection)
                });
            match projected {
                Ok(projection) => after_projection = Some(projection),
                Err(failed) => {
                    let (stage_failure, outcome) =
                        lab_operation_failure(LabOperationStage::AfterProjection, failed)?;
                    failure = Some(stage_failure);
                    failure_outcome = Some(outcome);
                    end = LabOperationEnd::Failed;
                }
            }
        }
        let prepared_artifact = match prepared_artifact {
            Some(prepared_artifact) => prepared_artifact,
            // No attempt reached its effect intent: the prepared record states the operation as
            // admitted, with no input frame, action or draw, and no guard decision
            // (`not_evaluated`).
            None => {
                prepared.operation = Some(LabDeclaredOperation {
                    phase_id: declared.phase_id.clone(),
                    from_page: declared.from_page.clone(),
                    destination_pages: declared.destination_pages.clone(),
                    max_attempts: declared.max_attempts,
                    recheck_frames,
                    guard: LabGuardOutcome::NotEvaluated,
                    sampling: None,
                });
                self.persist_lab_prepared(&prepared, (links.clone(), artifact_links.clone()))?
            }
        };
        let last = attempts.last();
        let (input_returned, input_action_id, input_intent, input_event) = (
            last.is_some_and(|attempt| attempt.input_returned),
            last.and_then(|attempt| attempt.input_action_id),
            last.and_then(|attempt| attempt.input_intent),
            last.and_then(|attempt| attempt.input_event),
        );
        let effect = aggregate_effect(&attempts);
        let after_frame = frames.last().map(|entry| entry.frame.clone());
        let record = LabOperationRecord {
            schema_version: LAB_OPERATION_TERMINAL_SCHEMA.to_string(),
            prepared,
            prepared_artifact,
            input_returned,
            input_action_id,
            input_intent,
            input_event,
            effect,
            after_frame,
            after_projection,
            arrival: None,
            operation_result: Some(LabDeclaredOperationResult {
                end,
                final_page,
                outcome_key,
                attempts,
                frames,
            }),
            failure,
            cleanup_failure: None,
        };
        self.finish_lab_operation(
            context,
            Some(&token),
            (links, artifact_links),
            record,
            failure_outcome,
        )
    }

    /// A Lab operation's prepared record, persisted and verified.
    fn persist_lab_prepared(
        &self,
        prepared: &LabOperationPrepared,
        (links, artifact_links): (EventLinksDraft, ArtifactLinksDraft),
    ) -> Result<LabEvidenceReference, RequestFailure> {
        let bytes = serde_json::to_vec(prepared)
            .map_err(|_| observation_integrity_failure(HostCode::LabPreparedEncodeFailed))?;
        let (artifact, verified) =
            self.persist_observation_artifact(&bytes, artifact_links, links)?;
        Ok(LabEvidenceReference {
            artifact,
            verified: terminal(&verified),
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
        let evidence = self.resolve_lab_input(
            &LabInputIdentity {
                request_id: prepared.request_id,
                correlation_id: prepared.correlation_id,
                instance_id: prepared.instance_id,
                lease_id: prepared.lease_id,
                action: prepared.action.as_ref().map(InputAction::event_action),
            },
            (0, record.prepared_artifact.verified.sequence),
            returned_terminal,
            input_failed,
        )?;
        if let Some(action_id) = evidence.action_id {
            record.input_action_id = Some(action_id);
        }
        if let Some(intent) = evidence.intent {
            record.input_intent = Some(intent);
        }
        if let Some(event) = evidence.event {
            record.input_event = Some(event);
        }
        if let Some(effect) = evidence.effect {
            record.effect = effect;
        }
        Ok(())
    }

    /// The native evidence of one input of a Lab operation of `identity`, after its input
    /// returned: its unique `InputIntent` recorded after `query_after` (later than
    /// `intent_floor`), the one `InputCommitted` or `InputFailed` of its physical action, and the
    /// check of the terminal the input returned. LR-10 (#412 B-3 S2): an attempt of a declared
    /// operation reads from after the previous attempt's events, as an element press reads the
    /// request's one input.
    fn resolve_lab_input(
        &self,
        identity: &LabInputIdentity,
        (query_after, intent_floor): (u64, u64),
        returned_terminal: Option<TerminalEvent>,
        input_failed: bool,
    ) -> Result<LabInputEvidence, RequestFailure> {
        let identity_matches = |event: &PersistedEvent| {
            event.links().request_id() == Some(&identity.request_id)
                && event.links().correlation_id() == Some(&identity.correlation_id)
                && event.links().instance_id() == Some(&identity.instance_id)
                && event.links().lease_id() == identity.lease_id.as_ref()
        };
        let mut evidence = LabInputEvidence::default();
        // Input has returned. These bounded native queries never select by proximity or latest.
        let intents = self
            .ledger
            .query_page(
                EventQuery {
                    event_type: Some(EventType::InputIntent),
                    request_id: Some(identity.request_id),
                    correlation_id: Some(identity.correlation_id),
                    instance_id: Some(identity.instance_id),
                    lease_id: identity.lease_id,
                    ..EventQuery::default()
                },
                query_after,
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
                || intent.sequence() <= intent_floor
                || Some(intent.payload().action()) != identity.action
            {
                return Err(observation_integrity_failure(
                    HostCode::LabInputIntentMismatch,
                ));
            }
            evidence.action_id = Some(action_id);
            evidence.intent = Some(terminal(intent));
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
            evidence.event = Some(terminal(&outcome));
            evidence.effect = Some(effect);
        }
        Ok(evidence)
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
        // #411 A49: the operation captures under its lease, as the lease holder.
        let completed = self.capture_observation_with_links(
            request,
            &instance.instance_alias,
            links,
            artifact_links,
            FrameOwner::Lease,
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

/// The failure of a selection the before projection does not resolve. #411 A37: on a matched
/// projection, an element id that no element of the projection has and that is one of the
/// package's recognition target ids is a target of the wrong type, `lab_target_type_mismatch`
/// with the expected and the received type. An unmatched projection resolves no element id of
/// either type, so its selection, like any other, keeps the contract's code.
fn selection_failure(
    selection: &LabOperationSelection,
    code: Code,
    projection: &PageProjection,
    observer: &PreparedPageObservation,
) -> RuntimeHostError {
    match selection {
        LabOperationSelection::Element { id }
            if code == RuntimeContractCode::LabElementUnavailable.code()
                && projection.matched
                && !projection
                    .elements
                    .iter()
                    .any(|element| element["id"].as_str() == Some(id.as_str()))
                && observer.declares_target(id) =>
        {
            RuntimeHostError::request(
                SharedCode::LabTargetTypeMismatch,
                HostLocation::ResolveCurrentLabOperation,
                RuntimeErrorCode::RecognitionFailed,
            )
            .with_outcome_values(|outcome| {
                outcome
                    .expected_target_type(LabTargetType::ElementId)
                    .received_target_type(LabTargetType::TargetId)
            })
        }
        _ => RuntimeHostError::request(
            code,
            HostLocation::ResolveCurrentLabOperation,
            RuntimeErrorCode::RecognitionFailed,
        ),
    }
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

/// The request a Lab operation serves: its request, instance and connection.
#[derive(Clone, Copy)]
struct LabOperationContext<'a> {
    original: &'a RuntimeRequest,
    request: &'a ValidatedRuntimeRequest<'a>,
    instance_alias: &'a str,
    holder_id: actingcommand_contract::HolderId,
    connection_id: ConnectionId,
    run_links: Option<RuntimeRunLinks>,
    instance_id: InstanceId,
    provenance: ExecutionBackendProvenance,
}

/// Whose input a Lab operation's evidence reads: its request, correlation, instance and lease,
/// and the event action of the input it made.
struct LabInputIdentity {
    request_id: RequestId,
    correlation_id: CorrelationId,
    instance_id: InstanceId,
    lease_id: Option<LeaseId>,
    action: Option<EventAction>,
}

/// What the native events say of one input of a Lab operation; `None` where they say nothing.
#[derive(Default)]
struct LabInputEvidence {
    action_id: Option<ActionId>,
    intent: Option<TerminalEvent>,
    event: Option<TerminalEvent>,
    effect: Option<EffectDisposition>,
}

/// The page a failed step attempt finished on when its last frame matched none (the kernel's
/// `finish_effect_attempt`): not a page, so the attempt states no `end_page`.
const UNRECOGNIZED_STEP_PAGE: &str = "<unrecognized>";

/// LR-10 (#412 B-3 S2, design §2.3): the runtime a declared operation's kernel run works over.
/// Its device is the Lab lease's capture and input, the handset keepalive before each input
/// (decision Q4); its draw is the instance's touch keys and landing history, shared with runs
/// (Q1); its clock is the Runtime clock at the factor 1000 (the Lab does not multiply by the
/// pacing factor). It handles no shared interrupt (Q3), application effect or selection state,
/// writes no task, fact or scheduling record and counts no designated effect (§2.5): the attempts
/// and frames it keeps are LR-10's records.
struct LabOperationRuntime<'a> {
    host: &'a HostShared,
    request: &'a ValidatedRuntimeRequest<'a>,
    token: &'a LeaseToken,
    instance_alias: &'a str,
    instance_id: InstanceId,
    connection_id: ConnectionId,
    provenance: ExecutionBackendProvenance,
    run_links: Option<RuntimeRunLinks>,
    /// The request's event and artifact links under its lease, which the prepared record has.
    links: EventLinksDraft,
    artifact_links: ArtifactLinksDraft,
    declared: &'a DeclaredOperation,
    request_id: RequestId,
    /// The request's seed: attempt 1 draws from it (decision Q9).
    seed: Option<u64>,
    /// Attempt 1's action seed, once drawn.
    first_seed: Option<u64>,
    prepared: LabOperationPrepared,
    /// The prepared record, persisted and verified at attempt 1's effect intent.
    prepared_artifact: Option<LabEvidenceReference>,
    /// The prepared record could not be persisted, which stopped the run before its input.
    prepared_unwritten: bool,
    /// Every frame the kernel captured, in order, at most `MAX_LAB_DECLARED_OPERATION_FRAMES`.
    frames: Vec<LabOperationFrameEntry>,
    /// The capture of the last frame, which the after projection evaluates.
    last_capture: Option<CompletedReadonlyObservation>,
    /// One per effect intent.
    attempts: Vec<LabOperationAttempt>,
    /// The attempts started (`StepStarted`); a frame belongs to the attempt started last.
    started_attempts: u32,
    /// The current attempt's input was made (`EffectCompleted`).
    after_input: bool,
    /// The frame count when attempt 1 started: the frames after it are its recheck's.
    recheck_from: Option<usize>,
    /// The stage of the first failure this runtime returned to the kernel.
    failure_stage: Option<LabOperationStage>,
    /// A failed read of the Runtime clock, reported at the next capture, input or record.
    clock_failure: Option<RuntimeHostError>,
}

impl LabOperationRuntime<'_> {
    fn clock_read(&self) -> Result<(), RequestFailure> {
        match &self.clock_failure {
            Some(error) => Err(RequestFailure::poison_without_terminal(error.clone())),
            None => Ok(()),
        }
    }

    /// `result`, keeping the stage of the first failure this runtime returns for the record.
    fn staged<T>(
        &mut self,
        stage: LabOperationStage,
        result: Result<T, RequestFailure>,
    ) -> Result<T, RequestFailure> {
        if result.is_err() && self.failure_stage.is_none() {
            self.failure_stage = Some(stage);
        }
        result
    }

    /// The fresh frames of attempt 1's recheck: those captured after it started.
    fn recheck_frames(&self) -> Result<u32, RequestFailure> {
        let count = self
            .recheck_from
            .map_or(0, |from| self.frames.len().saturating_sub(from));
        u32::try_from(count)
            .map_err(|_| observation_integrity_failure(HostCode::LabOperationResultInvalid))
    }

    /// One frame under the lease, fenced before and after as an element operation's: its entry
    /// is listed whether or not its fence held, and a fence that failed ends the operation. The
    /// frame after the last one listed is refused, `lab_operation_frame_limit_reached`.
    fn capture_frame(&mut self) -> Result<ObservedFrame, RequestFailure> {
        self.clock_read()?;
        if self.frames.len() >= MAX_LAB_DECLARED_OPERATION_FRAMES {
            let count = self.frames.len() + 1;
            return Err(RequestFailure::request(
                RuntimeHostError::request(
                    HostCode::LabOperationFrameLimitReached,
                    HostLocation::LockLabOperationCapture,
                    RuntimeErrorCode::InvalidRequest,
                )
                .with_outcome_values(|outcome| {
                    outcome
                        .count(count)
                        .limit_count(MAX_LAB_DECLARED_OPERATION_FRAMES)
                }),
                RuntimeReceiptState::Failed,
                None,
            ));
        }
        let (completed, fence) = self.host.capture_lab_operation_frame(
            self.request,
            self.token,
            self.connection_id,
            self.run_links,
        )?;
        let observation = completed.observation.clone();
        self.frames.push(LabOperationFrameEntry {
            frame: LabOperationFrame {
                observation: observation.clone(),
                verified: completed.verified,
                lease_valid_after_capture: fence.is_ok(),
            },
            attempt: self.started_attempts,
            after_input: self.after_input,
            page: None,
        });
        self.last_capture = Some(completed);
        fence?;
        let png = read_projected_verified(self.host.artifacts.root(), observation.artifact())
            .map_err(observation_artifact_failure)?;
        let frame = Frame::from_png(png, capture_backend_name(observation.capture_backend()))
            .map_err(|_| {
                RequestFailure::request(
                    RuntimeHostError::request(
                        SharedCode::CaptureFrameInvalid,
                        HostLocation::LockLabOperationCapture,
                        RuntimeErrorCode::CaptureFailed,
                    ),
                    RuntimeReceiptState::Failed,
                    None,
                )
            })?;
        Ok(ObservedFrame {
            input_reference: observation
                .artifact()
                .frame_id
                .map(|frame_id| InputFrameReference {
                    frame_id,
                    width: observation.width(),
                    height: observation.height(),
                }),
            frame,
        })
    }

    /// An attempt's effect intent: the input binds to the frame the kernel captured last (the
    /// step's frame or its recheck's last), which becomes the attempt's input frame. Attempt 1's
    /// persists and verifies the prepared record before it returns, so before the kernel's input.
    fn effect_intent(
        &mut self,
        action: InputAction,
        sampling: Option<InputSamplingEvidence>,
        guard: ContainedTaskGuardOutcome,
    ) -> Result<(), RequestFailure> {
        action.validate().map_err(|_| {
            RequestFailure::poison_without_terminal(RuntimeHostError::instance_fault(
                HostCode::ContainedTaskEffectInvalid,
                ContractLocation::RunContainedTask,
                RuntimeErrorCode::RuntimeHalted,
                self.instance_id,
            ))
        })?;
        let invalid = || {
            RequestFailure::poison_without_terminal(RuntimeHostError::instance_fault(
                SharedCode::ContainedTaskStateInvalid,
                ContractLocation::RunContainedTask,
                RuntimeErrorCode::RuntimeHalted,
                self.instance_id,
            ))
        };
        let attempt = u32::try_from(self.attempts.len())
            .ok()
            .and_then(|count| count.checked_add(1))
            .ok_or_else(invalid)?;
        let index = self.frames.len().checked_sub(1).ok_or_else(invalid)?;
        let input_frame_index = u32::try_from(index).map_err(|_| invalid())?;
        let entry = &mut self.frames[index];
        entry.attempt = attempt;
        entry.after_input = false;
        let input_frame = entry.frame.clone();
        let guard = lab_guard_outcome(guard);
        if attempt == 1 {
            self.prepared.operation = Some(LabDeclaredOperation {
                phase_id: self.declared.phase_id.clone(),
                from_page: self.declared.from_page.clone(),
                destination_pages: self.declared.destination_pages.clone(),
                max_attempts: self.declared.max_attempts,
                recheck_frames: self.recheck_frames()?,
                guard: guard.clone(),
                sampling: sampling.clone(),
            });
            self.prepared.before_frame = Some(input_frame);
            self.prepared.action = Some(action.clone());
            match self.host.persist_lab_prepared(
                &self.prepared,
                (self.links.clone(), self.artifact_links.clone()),
            ) {
                Ok(reference) => self.prepared_artifact = Some(reference),
                Err(failure) => {
                    self.prepared_unwritten = true;
                    return Err(failure);
                }
            }
        }
        self.attempts.push(LabOperationAttempt {
            attempt,
            input_frame_index,
            guard,
            action,
            sampling,
            input_returned: false,
            input_action_id: None,
            input_intent: None,
            input_event: None,
            effect: EffectDisposition::NotPerformed,
            end_page: None,
        });
        Ok(())
    }

    /// The current attempt's input through the Lab lease, as an element operation presses: the
    /// handset keepalive first (decision Q4), then `HostShared::input` with the frame the kernel
    /// bound it to. Its intent and outcome are read from after the previous attempt's events
    /// (after the prepared record for attempt 1). A frame other than the one captured last is
    /// the fatal `contained_task_effect_frame_identity_mismatch`.
    fn press(
        &mut self,
        action: InputAction,
        frame: Option<InputFrameContext>,
    ) -> Result<(), RequestFailure> {
        self.clock_read()?;
        let invalid = || {
            RequestFailure::poison_without_terminal(RuntimeHostError::instance_fault(
                SharedCode::ContainedTaskStateInvalid,
                ContractLocation::RunContainedTask,
                RuntimeErrorCode::RuntimeHalted,
                self.instance_id,
            ))
        };
        let index = self.attempts.len().checked_sub(1).ok_or_else(invalid)?;
        let prepared_sequence = self
            .prepared_artifact
            .as_ref()
            .map(|prepared| prepared.verified.sequence)
            .ok_or_else(invalid)?;
        let reference = frame.map(|frame| frame.reference());
        let last_frame = self
            .frames
            .last()
            .and_then(|entry| entry.frame.observation.artifact().frame_id);
        if reference.is_some_and(|reference| Some(reference.frame_id) != last_frame) {
            return Err(RequestFailure::poison_without_terminal(
                RuntimeHostError::instance_fault(
                    HostCode::ContainedTaskEffectFrameIdentityMismatch,
                    ContractLocation::RunContainedTask,
                    RuntimeErrorCode::RuntimeHalted,
                    self.instance_id,
                ),
            ));
        }
        self.host.device_state_keepalive(
            self.instance_id,
            self.instance_alias,
            self.token,
            self.connection_id,
        )?;
        let (returned, input_failure) = match self.host.input(
            self.request,
            self.token,
            &action,
            self.connection_id,
            self.provenance,
            RuntimeInputContext {
                run_links: self.run_links,
                source_step_action_id: None,
                before_frame_id: reference.map(|reference| reference.frame_id),
                input_frame: reference,
                input_control: None,
            },
        ) {
            Ok((success, _)) => (success.terminal, None),
            Err(failure) if failure.poison_runtime || failure.error.is_fatal() => {
                return Err(failure);
            }
            Err(failure) => (failure.terminal, Some(failure)),
        };
        let floor = index
            .checked_sub(1)
            .and_then(|previous| self.attempts.get(previous))
            .and_then(|previous| previous.input_event.or(previous.input_intent))
            .map_or(prepared_sequence, |event| event.sequence);
        let evidence = self.host.resolve_lab_input(
            &LabInputIdentity {
                request_id: self.prepared.request_id,
                correlation_id: self.prepared.correlation_id,
                instance_id: self.prepared.instance_id,
                lease_id: self.prepared.lease_id,
                action: Some(action.event_action()),
            },
            (floor, floor),
            returned,
            input_failure.is_some(),
        );
        let attempt = &mut self.attempts[index];
        attempt.input_returned = true;
        attempt.effect = EffectDisposition::Indeterminate;
        let evidence = match evidence {
            Ok(evidence) => evidence,
            Err(mut evidence_failure) => {
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
        };
        attempt.input_action_id = evidence.action_id;
        attempt.input_intent = evidence.intent;
        attempt.input_event = evidence.event;
        if let Some(effect) = evidence.effect {
            attempt.effect = effect;
        }
        if let Some(failure) = input_failure {
            return Err(failure);
        }
        if attempt.input_event.is_none() {
            return Err(RequestFailure::request(
                RuntimeHostError::request(
                    HostCode::LabInputOutcomeMissing,
                    HostLocation::ResolveLabInputEvidence,
                    RuntimeErrorCode::BackendOperationFailed,
                ),
                RuntimeReceiptState::Failed,
                None,
            ));
        }
        Ok(())
    }
}

impl ContainedTaskRuntime for LabOperationRuntime<'_> {
    type Error = RequestFailure;

    fn classify_error(error: &Self::Error) -> ContainedTaskRuntimeErrorClass {
        if error.poison_runtime || error.error.is_fatal() {
            ContainedTaskRuntimeErrorClass::Fatal
        } else {
            ContainedTaskRuntimeErrorClass::Nonfatal
        }
    }

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        let stage = if self.after_input {
            LabOperationStage::AfterFrame
        } else {
            LabOperationStage::BeforeFrame
        };
        let captured = self.capture_frame();
        self.staged(stage, captured)
    }

    fn committed_input_frame(
        &mut self,
        reference: InputFrameReference,
    ) -> Result<Option<InputFrameContext>, Self::Error> {
        self.host
            .execution()?
            .resolve_input_frame(self.instance_alias, reference)
            .map(Some)
            // ISO-2 (#419, ISO-M rows A05, E16): a resolve refused by the instance's session
            // fails only this operation.
            .map_err(|error| {
                RequestFailure::frame_handoff(HostLocation::ResolveCommittedInputFrame, &error)
            })
    }

    /// Decision Q9: attempt 1 draws from the request's seed, or from one derived from the
    /// request; attempt k > 1 from (that seed, k).
    fn action_seed(
        &mut self,
        _step_index: u32,
        _operation_label: &str,
    ) -> Result<Option<u64>, Self::Error> {
        let first = match self.first_seed {
            Some(first) => first,
            None => {
                let first = match self.seed {
                    Some(seed) => seed,
                    None => {
                        contained_task_sampling_seed(&("lab_operation/action", self.request_id))
                            .map_err(RequestFailure::poison_without_terminal)?
                    }
                };
                self.first_seed = Some(first);
                first
            }
        };
        if self.started_attempts <= 1 {
            return Ok(Some(first));
        }
        contained_task_sampling_seed(&("lab_operation/attempt", first, self.started_attempts))
            .map(Some)
            .map_err(RequestFailure::poison_without_terminal)
    }

    /// The `touch.sampling` keys in effect, as a run's step reads them.
    fn touch_sampling(&mut self) -> Result<Option<TouchSampling>, Self::Error> {
        let touch = self
            .host
            .touch_config()
            .map_err(RequestFailure::poison_without_terminal)?;
        Ok(Some(TouchSampling {
            sigma_ratio_milli: touch.sigma_ratio_milli,
            max_redraws: touch.max_redraws,
            candidate_count: touch.candidate_count,
        }))
    }

    /// Decision Q1: the instance's landing history, shared with runs, chooses where the press
    /// lands and records it.
    fn choose_touch_candidate(
        &mut self,
        region: &InputSamplingRegion,
        candidates: &[(i32, i32)],
    ) -> Result<usize, Self::Error> {
        self.host
            .choose_touch_candidate(self.instance_alias, region, candidates)
            .map_err(RequestFailure::poison_without_terminal)
    }

    fn input(
        &mut self,
        action: InputAction,
        frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        let pressed = self.press(action, frame);
        self.staged(LabOperationStage::Input, pressed)
    }

    /// The page the kernel matched on the frame it captured last, that frame's entry's `page`.
    fn record_page_evaluations(
        &mut self,
        _phase: &'static str,
        results: &actingcommand_page_detector::PageBatchResult,
        _timing: Option<ContainedTaskEvaluationTiming>,
    ) -> Result<(), Self::Error> {
        if let Ok(outcomes) = results {
            let mut matched = outcomes
                .iter()
                .filter_map(|outcome| outcome.result.as_ref().ok())
                .filter(|evaluation| evaluation.matched);
            if let (Some(page), None) = (matched.next(), matched.next())
                && let Some(entry) = self.frames.last_mut()
            {
                entry.page = Some(page.page_id.clone());
            }
        }
        Ok(())
    }

    fn sampling_supported(&self) -> bool {
        true
    }

    /// A cancellation of the operation (a fatal, a shutdown, a lost lease) under its own code.
    fn candidate_sampling_checkpoint(&mut self) -> Result<(), Self::Error> {
        self.host
            .validate_lab_arrival_token(self.request, self.token, self.connection_id)
    }

    /// The configured observe-only retries at the factor 1000, without extra rounds or a
    /// pre-input wait.
    fn step_pacing(&mut self) -> Result<StepPacing, Self::Error> {
        self.host
            .sample_timeout_retries()
            .map(StepPacing::new)
            .map_err(RequestFailure::poison_without_terminal)
    }

    fn monotonic_now(&mut self) -> Instant {
        match self.host.kernel_instant() {
            Ok(now) => now,
            Err(error) => {
                if self.clock_failure.is_none() {
                    self.clock_failure = Some(error);
                }
                self.host.clock_origin_instant
            }
        }
    }

    /// No owner or instance lock is held while the kernel waits.
    fn pause(&mut self, duration: Duration) {
        self.host.kernel_pause(duration);
    }

    /// Design §2.3 "Traces": `StepStarted` begins an attempt, `EffectIntent` records it (attempt
    /// 1's persists the prepared record), `EffectCompleted` marks the frames after its input,
    /// `StepFinished` gives its end page; no other trace writes anything.
    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        self.clock_read()?;
        match trace {
            ContainedTaskTrace::StepStarted { .. } => {
                self.started_attempts = self.started_attempts.checked_add(1).ok_or_else(|| {
                    RequestFailure::poison_without_terminal(RuntimeHostError::instance_fault(
                        SharedCode::ContainedTaskStateInvalid,
                        ContractLocation::RunContainedTask,
                        RuntimeErrorCode::RuntimeHalted,
                        self.instance_id,
                    ))
                })?;
                self.after_input = false;
                if self.started_attempts == 1 {
                    self.recheck_from = Some(self.frames.len());
                }
                Ok(())
            }
            ContainedTaskTrace::EffectIntent {
                action,
                sampling,
                guard,
                ..
            } => self.effect_intent(action, sampling, guard),
            ContainedTaskTrace::EffectCompleted { .. } => {
                self.after_input = true;
                Ok(())
            }
            ContainedTaskTrace::StepFinished { page_label, .. } => {
                if usize::try_from(self.started_attempts)
                    .is_ok_and(|started| started == self.attempts.len())
                    && let Some(attempt) = self.attempts.last_mut()
                {
                    attempt.end_page = (page_label != UNRECOGNIZED_STEP_PAGE).then_some(page_label);
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

/// The kernel's guard decision as LR-10 records it.
fn lab_guard_outcome(guard: ContainedTaskGuardOutcome) -> LabGuardOutcome {
    match guard {
        ContainedTaskGuardOutcome::TrustedCoordinate => LabGuardOutcome::TrustedCoordinate,
        ContainedTaskGuardOutcome::Passed {
            page_label,
            target_id,
            target_kind,
        } => LabGuardOutcome::Passed {
            page_id: page_label,
            target_id,
            target_kind,
        },
    }
}

/// LR-10 (design §3.2): `performed` if an attempt performed, else `indeterminate` if one was,
/// else `not_performed`.
fn aggregate_effect(attempts: &[LabOperationAttempt]) -> EffectDisposition {
    let any = |effect: EffectDisposition| attempts.iter().any(|attempt| attempt.effect == effect);
    if any(EffectDisposition::Performed) {
        EffectDisposition::Performed
    } else if any(EffectDisposition::Indeterminate) {
        EffectDisposition::Indeterminate
    } else {
        EffectDisposition::NotPerformed
    }
}

/// The device's name of the capture backend the Runtime recorded a frame from.
fn capture_backend_name(backend: RuntimeCaptureBackend) -> CaptureBackendName {
    match backend {
        RuntimeCaptureBackend::AdbScreencap => CaptureBackendName::AdbScreencap,
        RuntimeCaptureBackend::AdbScreencapEncode => CaptureBackendName::AdbScreencapEncode,
        RuntimeCaptureBackend::AdbScreencapRawGzip => CaptureBackendName::AdbScreencapRawGzip,
        RuntimeCaptureBackend::DroidcastRaw => CaptureBackendName::DroidcastRaw,
        RuntimeCaptureBackend::NemuIpc => CaptureBackendName::NemuIpc,
        RuntimeCaptureBackend::Scrcpy => CaptureBackendName::Scrcpy,
    }
}

/// The kernel's refusal of a declared operation's package or id at admission, before any lease:
/// its code and the values it carries.
fn lab_declared_admission_error(error: ContainedTaskError) -> RuntimeHostError {
    let mut refused = RuntimeHostError::relayed_request(
        error.code().as_str(),
        HostLocation::AdmitLabOperation.as_str(),
        RuntimeErrorCode::InvalidRequest,
    );
    if error.has_values() {
        let values = error.outcome();
        refused =
            refused.with_outcome_values(|_| values.operation(HostLocation::AdmitLabOperation));
    }
    refused
}

/// The kernel's failure of a declared operation's run, as the Lab operation's failure: its code
/// and the values it carries.
fn lab_kernel_failure(error: &ContainedTaskError) -> RequestFailure {
    let mut failed = RuntimeHostError::relayed_request(
        error.code().as_str(),
        ContractLocation::RunContainedTask.as_str(),
        RuntimeErrorCode::BackendOperationFailed,
    );
    if error.has_values() {
        let values = error.outcome();
        failed =
            failed.with_outcome_values(|_| values.operation(ContractLocation::RunContainedTask));
    }
    if let Some(detail) = error.detail() {
        failed = failed.with_native_detail(detail.to_owned());
    }
    RequestFailure::request(failed, RuntimeReceiptState::Failed, None)
}

/// Design §4: an operation whose failure decision asked for recovery, which the Lab does not run,
/// answers that failure's code.
fn lab_recovery_failure(reason: ContainedTaskCode) -> RequestFailure {
    RequestFailure::request(
        RuntimeHostError::relayed_request(
            reason.as_str(),
            ContractLocation::RunContainedTask.as_str(),
            RuntimeErrorCode::BackendOperationFailed,
        ),
        RuntimeReceiptState::Failed,
        None,
    )
}
