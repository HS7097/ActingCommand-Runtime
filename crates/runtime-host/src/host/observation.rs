// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

pub(super) struct CompletedReadonlyObservation {
    pub(super) observation: ReadonlyObservation,
    pub(super) terminal: PersistedEvent,
    pub(super) verified: TerminalEvent,
    pub(super) links: EventLinksDraft,
    pub(super) artifact_links: ArtifactLinksDraft,
}

fn runtime_capture_backend(
    backend: CaptureBackendName,
) -> RuntimeHostResult<RuntimeCaptureBackend> {
    match backend {
        CaptureBackendName::FixtureSimulation => Err(RuntimeHostError::fatal(
            "fixture_capture_outside_simulation_scope",
            "map_runtime_capture_backend",
            RuntimeErrorCode::RuntimeFatal,
        )),
        CaptureBackendName::AdbScreencap => Ok(RuntimeCaptureBackend::AdbScreencap),
        CaptureBackendName::AdbScreencapEncode => Ok(RuntimeCaptureBackend::AdbScreencapEncode),
        CaptureBackendName::AdbScreencapRawGzip => Ok(RuntimeCaptureBackend::AdbScreencapRawGzip),
        CaptureBackendName::DroidcastRaw => Ok(RuntimeCaptureBackend::DroidcastRaw),
        CaptureBackendName::NemuIpc => Ok(RuntimeCaptureBackend::NemuIpc),
    }
}

impl HostShared {
    pub(super) fn observe_readonly(
        &self,
        original: &RuntimeRequest,
        request: &ValidatedRuntimeRequest<'_>,
        instance_alias: &str,
    ) -> Result<OperationSuccess, RequestFailure> {
        let resolved = self.resolve_instance(instance_alias)?;
        self.append_request_lifecycle(
            original,
            request,
            resolved.instance_id(),
            EventAction::RuntimeReadonlyObserve,
            None,
        )?;
        self.require_bound_endpoint(
            &resolved,
            self.events
                .request_links(request, Some(resolved.instance_id()), None, None),
            EventAction::RuntimeReadonlyObserve,
        )?;
        self.append_scheduler_admitted(request, &resolved, None)?;
        let completed =
            self.capture_readonly_observation(request, instance_alias, resolved.instance_id())?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(terminal(&completed.terminal)),
            result: RuntimeResult::ReadonlyObservationCompleted {
                observation: completed.observation,
            },
        })
    }

    pub(super) fn capture_sequence(
        &self,
        original: &RuntimeRequest,
        request: &ValidatedRuntimeRequest<'_>,
        instance_alias: &str,
        spec: CaptureSequenceSpec,
    ) -> Result<OperationSuccess, RequestFailure> {
        spec.validate().map_err(|_| {
            RequestFailure::request(
                RuntimeHostError::request(
                    "capture_sequence_spec_invalid",
                    "capture_sequence",
                    RuntimeErrorCode::InvalidRequest,
                ),
                RuntimeReceiptState::Denied,
                None,
            )
        })?;
        let resolved = self.resolve_instance(instance_alias)?;
        self.append_request_lifecycle(
            original,
            request,
            resolved.instance_id(),
            EventAction::RuntimeCaptureSequence,
            None,
        )?;
        self.require_bound_endpoint(
            &resolved,
            self.events
                .request_links(request, Some(resolved.instance_id()), None, None),
            EventAction::RuntimeCaptureSequence,
        )?;
        self.append_scheduler_admitted(request, &resolved, None)?;
        let mut observations = Vec::with_capacity(usize::from(spec.frame_count()));
        let mut last_terminal = None;
        for index in 0..spec.frame_count() {
            let completed =
                self.capture_readonly_observation(request, instance_alias, resolved.instance_id())?;
            observations.push(completed.observation);
            last_terminal = Some(completed.terminal);
            if index + 1 < spec.frame_count() && spec.interval_ms() > 0 {
                thread::sleep(Duration::from_millis(spec.interval_ms()));
            }
        }
        let sequence = CaptureSequence::new(spec, observations).map_err(|_| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                "capture_sequence_result_invalid",
                "capture_sequence",
                RuntimeErrorCode::RuntimeFatal,
            ))
        })?;
        let terminal_event = last_terminal.ok_or_else(|| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                "capture_sequence_terminal_missing",
                "capture_sequence",
                RuntimeErrorCode::RuntimeFatal,
            ))
        })?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(terminal(&terminal_event)),
            result: RuntimeResult::CaptureSequenceCompleted { sequence },
        })
    }

    pub(super) fn capture_readonly_observation(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        instance_alias: &str,
        instance_id: InstanceId,
    ) -> Result<CompletedReadonlyObservation, RequestFailure> {
        self.require_physical_instance_id(instance_id)?;
        let capability = self.issue_readonly_capability(instance_id)?;
        let debug_run =
            if request.actor() == EventActor::Lab && request.source() == EventSource::Lab {
                lock(&self.debug_runs, "read_runtime_debug_run")?
                    .get(&request.correlation_id())
                    .map(|context| (context.task_id, context.run_id))
            } else {
                None
            };
        let mut links = capability.event_links(request);
        if let Some((task_id, run_id)) = debug_run {
            links = links.with_task_id(task_id).with_run_id(run_id);
        }
        let mut artifact_links = capability.artifact_links(request);
        if let Some((_, run_id)) = debug_run {
            artifact_links = artifact_links.with_run_id(run_id);
        }
        let instance_guard = self.instance_guard(instance_id)?;
        let admission = lock(&instance_guard, "lock_instance_admission")?;
        self.capture_observation_with_links(
            request,
            instance_alias,
            links,
            artifact_links,
            &admission,
        )
    }

    pub(super) fn capture_observation_with_links(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        instance_alias: &str,
        links: EventLinksDraft,
        artifact_links: ArtifactLinksDraft,
        admission: &MutexGuard<'_, ()>,
    ) -> Result<CompletedReadonlyObservation, RequestFailure> {
        self.require_business_capacity(links.clone())?;
        self.append_event(
            EventSeverity::Info,
            EventSource::Device,
            OriginModule::Capture,
            EventActor::Runtime,
            links.clone(),
            CapturePayloadDraft::requested(EventAction::CaptureObserve, AuditInput::new()),
        )?;
        self.append_event(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Recognition,
            EventActor::Runtime,
            links.clone(),
            RecognitionPayloadDraft::requested(EventAction::RecognitionObserve, AuditInput::new()),
        )?;
        let registration = self
            .mark_resources_in_use()
            .map_err(RequestFailure::poison_without_terminal)?;
        let capture_started = Instant::now();
        let captured = self
            .execution
            .capture_retained_with_registration_guard(instance_alias, registration);
        let capture_acquire_us = performance::measured_microseconds(
            actingcommand_execution_kernel::observe_instant_span(capture_started, Instant::now()),
        );
        let frame = match captured {
            Ok(frame) => frame,
            Err(error) => {
                let error = self
                    .finish_capture_failure_while_guarded(error, links.clone(), admission)
                    .map_err(RequestFailure::poison_without_terminal)?;
                let runtime_error = RuntimeHostError::execution("execute_capture_backend", &error);
                if self
                    .retain_unconfirmed_resources(&runtime_error, links.clone())
                    .map_err(RequestFailure::poison_without_terminal)?
                {
                    return Err(RequestFailure::poison_without_terminal(runtime_error));
                }
                let payload = CapturePayloadDraft::failed_with_causes(
                    EventAction::CaptureObserve,
                    DiagnosticCode::CaptureFailed,
                    EffectDisposition::NotPerformed,
                    runtime_error.diagnostic_detail().cloned(),
                    runtime_error.cleanup_cause().cloned(),
                    AuditInput::new(),
                );
                let failed = self.append_event(
                    EventSeverity::Error,
                    EventSource::Device,
                    OriginModule::Capture,
                    EventActor::Runtime,
                    links.clone(),
                    payload,
                )?;
                self.record_required_failure(&runtime_error, &failed, links.clone())?;
                let event = self.append_event(
                    EventSeverity::Error,
                    EventSource::Runtime,
                    OriginModule::Recognition,
                    EventActor::Runtime,
                    links.clone(),
                    RecognitionPayloadDraft::failed(
                        EventAction::RecognitionObserve,
                        DiagnosticCode::CaptureFailed,
                        EffectDisposition::NotPerformed,
                        AuditInput::new(),
                    ),
                )?;
                return Err(RequestFailure::request(
                    runtime_error,
                    RuntimeReceiptState::Failed,
                    Some(terminal(&event)),
                ));
            }
        };
        let artifact_png = match frame.png_for_artifact() {
            Ok(png) => png,
            Err(_) => {
                self.append_event(
                    EventSeverity::Error,
                    EventSource::Device,
                    OriginModule::Capture,
                    EventActor::Runtime,
                    links.clone(),
                    CapturePayloadDraft::failed(
                        EventAction::CaptureObserve,
                        DiagnosticCode::CaptureFailed,
                        EffectDisposition::Indeterminate,
                        AuditInput::new(),
                    ),
                )?;
                let event = self.append_event(
                    EventSeverity::Error,
                    EventSource::Runtime,
                    OriginModule::Recognition,
                    EventActor::Runtime,
                    links.clone(),
                    RecognitionPayloadDraft::failed(
                        EventAction::RecognitionObserve,
                        DiagnosticCode::CaptureFailed,
                        EffectDisposition::NotPerformed,
                        AuditInput::new(),
                    ),
                )?;
                return Err(RequestFailure::request(
                    RuntimeHostError::request(
                        "capture_frame_invalid",
                        "observe_readonly",
                        RuntimeErrorCode::CaptureFailed,
                    ),
                    RuntimeReceiptState::Failed,
                    Some(terminal(&event)),
                ));
            }
        };
        let write_context = ArtifactWriteContext::new(
            artifact_links.clone(),
            links.clone(),
            unix_ms_now().map_err(RequestFailure::poison_without_terminal)?,
        );
        let mut sink = online_observation::ObservationArtifactSink {
            ledger: &self.ledger,
            events: &self.events,
            verified: None,
            frame_retention: Some((
                self.owner_epoch,
                frame_retention::capture_pin_reason(request),
            )),
        };
        let frame_id = links.frame_id().ok_or_else(|| {
            online_observation::observation_integrity_failure("observation_frame_identity_missing")
        })?;
        let mut pipeline = CapturePipeline::open_with_store(
            Arc::clone(&self.artifacts),
            frame_retention::spill_root(self.artifacts.root(), frame_id)
                .map_err(online_observation::observation_artifact_failure)?,
            CapturePipelineConfig {
                frame_store: frame_retention::capture_frame_store_config(),
                retention_class: if request.actor() == EventActor::Lab
                    && request.source() == EventSource::Lab
                {
                    RetentionClass::DebugFull
                } else {
                    RetentionClass::Adaptive
                },
                redaction_state: ArtifactRedactionState::NotRequired,
                ..CapturePipelineConfig::default()
            },
            write_context.clone(),
            &mut sink,
        )
        .map_err(online_observation::observation_artifact_failure)?;
        let mut retained = frame.clone();
        retained.original_png = Some(artifact_png);
        let captured = pipeline
            .record_frame(
                FrameStoreFrameInput {
                    frame_index: 0,
                    file_name: "frame-0.png".to_owned(),
                    label: "initial".to_owned(),
                    recognition_state: RecognitionState::CompletedNoMatch,
                    pinned_reason: None,
                    frame: retained,
                },
                write_context.clone(),
                &mut sink,
            )
            .map_err(online_observation::observation_artifact_failure)?;
        if !captured.frame.warnings.is_empty() {
            return Err(online_observation::observation_artifact_failure(
                ArtifactStoreError::fatal(
                    "capture_spill_failed",
                    "persist_readonly_frame",
                    captured.frame.warnings.join("; "),
                ),
            ));
        }
        let reference = pipeline
            .persist_frame(0, &mut sink)
            .map_err(online_observation::observation_artifact_failure)?;
        pipeline
            .poll_pressure(&write_context, &mut sink)
            .map_err(online_observation::observation_artifact_failure)?;
        pipeline
            .cleanup_spills()
            .map_err(online_observation::observation_artifact_failure)?;
        let observation = ReadonlyObservation::new(
            frame.width,
            frame.height,
            RecognitionVerdict::FrameDecoded,
            runtime_capture_backend(frame.backend_name)
                .map_err(RequestFailure::poison_without_terminal)?,
            reference.project(true),
        )
        .map_err(|_| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                "readonly_observation_invalid",
                "observe_readonly",
                RuntimeErrorCode::RuntimeFatal,
            ))
        })?;
        self.append_capture_completed(
            links.clone(),
            observation.width(),
            observation.height(),
            capture_acquire_us,
        )?;
        let event = self.append_event(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Recognition,
            EventActor::Runtime,
            links.clone(),
            RecognitionPayloadDraft::completed(
                EventAction::RecognitionObserve,
                EffectDisposition::Performed,
                observation.width(),
                observation.height(),
                observation.verdict(),
                AuditInput::new(),
            ),
        )?;
        Ok(CompletedReadonlyObservation {
            observation,
            terminal: event,
            verified: terminal(&sink.verified.ok_or_else(|| {
                online_observation::observation_integrity_failure(
                    "observation_verified_event_missing",
                )
            })?),
            links,
            artifact_links,
        })
    }

    fn issue_readonly_capability(
        &self,
        instance_id: InstanceId,
    ) -> Result<IssuedReadOnlyCaptureCapability, RequestFailure> {
        self.events
            .issuer()
            .issue_readonly_capture_capability(self.owner_epoch, instance_id)
            .map_err(|_| {
                RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                    "readonly_capability_issue_failed",
                    "issue_readonly_capability",
                    RuntimeErrorCode::RuntimeFatal,
                ))
            })
    }

    fn append_capture_completed(
        &self,
        links: EventLinksDraft,
        width: u32,
        height: u32,
        capture_acquire_us: Option<u64>,
    ) -> Result<PersistedEvent, RequestFailure> {
        self.append_event(
            EventSeverity::Info,
            EventSource::Device,
            OriginModule::Capture,
            EventActor::Runtime,
            links,
            CapturePayloadDraft::completed_with_capture_acquire(
                EventAction::CaptureObserve,
                EffectDisposition::Performed,
                width,
                height,
                capture_acquire_us,
                AuditInput::new(),
            ),
        )
    }
}
