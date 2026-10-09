// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

/// Workflow #375 R5c: an observe frame is compared only with the previous observe frame of
/// its instance and origin, and only when that one was captured less than 50 s earlier, so
/// every marker that links a frame exists before the frame settles (60 s).
const OBSERVE_CHAIN_BOUND_MS: u64 = 50_000;

/// Workflow #375 R5c: the latest persisted observe frame per instance and origin (whether the
/// frame's retention pin reason is `Lab`), in memory for the life of the process; the first
/// observe capture after a start is never marked.
#[derive(Default)]
pub(super) struct ObserveMarks {
    latest: BTreeMap<(InstanceId, bool), ObserveMark>,
}

/// One persisted observe frame: its capture links, thumbnail and capture time.
struct ObserveMark {
    links: EventLinksDraft,
    thumbnail: actingcommand_artifact_store::FrameThumbnail,
    captured_at_unix_ms: u64,
}

/// The marker a new observe frame gets: the representative's links and the gap.
struct ObserveMarker {
    representative: EventLinksDraft,
    duration_ms: u64,
}

impl ObserveMarks {
    fn marker(&self, key: (InstanceId, bool), current: &ObserveMark) -> Option<ObserveMarker> {
        let previous = self.latest.get(&key)?;
        let gap = current
            .captured_at_unix_ms
            .checked_sub(previous.captured_at_unix_ms)?;
        (gap < OBSERVE_CHAIN_BOUND_MS
            && actingcommand_artifact_store::thumb_similarity(
                &previous.thumbnail,
                &current.thumbnail,
            ) > actingcommand_artifact_store::DEFAULT_SIMILARITY_THRESHOLD)
            .then(|| ObserveMarker {
                representative: previous.links.clone(),
                // The sanitizer refuses a zero duration.
                duration_ms: gap.max(1),
            })
    }

    fn record(&mut self, key: (InstanceId, bool), current: ObserveMark) {
        self.latest.insert(key, current);
    }
}

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
        // Workflow #369 Q-6: a read-only observe stays keyless; its guard pumps when dropped, so a
        // claim queued during the observe is granted right after it.
        let admission = self.lock_admission(&instance_guard, instance_id)?;
        self.capture_observation_with_links(
            request,
            instance_alias,
            links,
            artifact_links,
            &admission,
        )
    }

    fn readonly_frame_admission_failure(
        &self,
        error: ArtifactStoreError,
        links: EventLinksDraft,
    ) -> RequestFailure {
        if !error.is_unpublished_frame_layout() {
            return self.capture_material_failure(error, links);
        }
        let runtime = RuntimeHostError::request(
            "capture_frame_invalid",
            "observe_readonly",
            RuntimeErrorCode::CaptureFailed,
        )
        .with_native_failure_detail(error.native_detail());
        let recorded = (|| {
            let failed = self.append_event_raw(
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
            self.record_required_failure(&runtime, &failed, links.clone())?;
            self.append_event_raw(
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
            )
        })();
        match recorded {
            Ok(event) => RequestFailure::request(
                runtime,
                RuntimeReceiptState::Failed,
                Some(terminal(&event)),
            ),
            Err(writer) => RequestFailure::poison_without_terminal(
                runtime.with_related_failure("capture_failure_record", &writer),
            ),
        }
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
        let frame_id = links.frame_id().ok_or_else(|| {
            online_observation::observation_integrity_failure("observation_frame_identity_missing")
        })?;
        let frame_store = actingcommand_artifact_store::FrameStore::new(
            frame_retention::spill_root(self.artifacts.root(), frame_id)
                .map_err(online_observation::observation_artifact_failure)?,
            frame_retention::capture_frame_store_config(),
        )
        .map_err(online_observation::observation_artifact_failure)?;
        let memory = frame_store.memory_budget();
        let registration = self
            .mark_resources_in_use()
            .map_err(RequestFailure::poison_without_terminal)?;
        let captured = self
            .execution()?
            .capture_frame_retained_with_registration_guard(
                instance_alias,
                links.frame_id().copied(),
                registration,
                memory,
            );
        let frame = match captured {
            Ok(mut frame) => {
                self.append_backend_open_observations(
                    &std::mem::take(&mut frame.backend_open_observations),
                    links.clone(),
                    EventSource::Device,
                    OriginModule::Capture,
                )
                .map_err(RequestFailure::poison_without_terminal)?;
                frame
            }
            Err(error) => {
                self.append_backend_open_failure_observations(
                    &error,
                    links.clone(),
                    EventSource::Device,
                    OriginModule::Capture,
                )
                .map_err(RequestFailure::poison_without_terminal)?;
                let error = self
                    .finish_capture_failure_while_guarded(error, links.clone(), admission)
                    .map_err(RequestFailure::poison_without_terminal)?;
                let runtime_error = RuntimeHostError::readonly_capture(&error);
                if self
                    .retain_unconfirmed_resources(&runtime_error, links.clone())
                    .map_err(RequestFailure::poison_without_terminal)?
                {
                    return Err(RequestFailure::poison_without_terminal(runtime_error));
                }
                let payload = CapturePayloadDraft::failed_with_causes(
                    EventAction::CaptureObserve,
                    DiagnosticCode::CaptureFailed,
                    if matches!(
                        runtime_error.code(),
                        "capture_frame_invalid" | "frame_workspace_unavailable"
                    ) {
                        EffectDisposition::Indeterminate
                    } else {
                        EffectDisposition::NotPerformed
                    },
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
        let mut pipeline = CapturePipeline::open_with_frame_store(
            Arc::clone(&self.artifacts),
            frame_store,
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
        let mut events = contained_task::RuntimeArtifactEventSink {
            ledger: &self.ledger,
            events: &self.events,
        };
        let mut publish = |store: &ArtifactStore, input: ArtifactWriteRequest<'_>| {
            sink.publish_frame(store, input, Some(*frame_id))
        };
        let (frame_width, frame_height, frame_backend, capture_acquire_us) = (
            frame.width,
            frame.height,
            frame.backend_name,
            frame.capture_acquire_us(),
        );
        let reference = pipeline
            .with_frame_by_value(frame, |pipeline, frame| -> Result<_, RequestFailure> {
                let captured = pipeline
                    .record_frame_with_publisher(
                        FrameStoreFrameInput {
                            frame_index: 0,
                            file_name: "frame-0.png".to_owned(),
                            label: "initial".to_owned(),
                            recognition_state: RecognitionState::CompletedNoMatch,
                            pinned_reason: None,
                            frame,
                        },
                        write_context.clone(),
                        &mut events,
                        Some(&mut publish),
                    )
                    .map_err(|error| self.capture_material_failure(error, links.clone()))?;
                self.record_frame_pressure_failures(pipeline, &captured.frame.frame_failures)?;
                let reference = pipeline
                    .persist_frame_with_publisher(0, &mut events, Some(&mut publish))
                    .map_err(|error| self.capture_material_failure(error, links.clone()))?;
                pipeline
                    .poll_pressure_with_publisher(&write_context, &mut events, Some(&mut publish))
                    .map_err(|error| self.capture_material_failure(error, links.clone()))?;
                pipeline
                    .cleanup_spills()
                    .map_err(|error| self.capture_material_failure(error, links.clone()))?;
                Ok(reference)
            })
            .map_err(|error| self.readonly_frame_admission_failure(error, links.clone()))??;
        let thumbnail = pipeline
            .frame_thumbnail(0)
            .cloned()
            .map_err(|error| self.capture_material_failure(error, links.clone()))?;
        self.mark_observe_duplicate(
            request,
            &links,
            ObserveMark {
                links: links.clone(),
                thumbnail,
                captured_at_unix_ms: reference.created_at_unix_ms(),
            },
        )?;
        let observation = ReadonlyObservation::new(
            frame_width,
            frame_height,
            RecognitionVerdict::FrameDecoded,
            runtime_capture_backend(frame_backend)
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
        let verified = terminal(&sink.verified.ok_or_else(|| {
            online_observation::observation_integrity_failure("observation_verified_event_missing")
        })?);
        self.execution()?
            .commit_input_frame(
                instance_alias,
                actingcommand_contract::InputFrameReference {
                    frame_id: *frame_id,
                    width: observation.width(),
                    height: observation.height(),
                },
            )
            .map_err(|error| {
                RequestFailure::poison_without_terminal(RuntimeHostError::execution(
                    "commit_capture_input_frame",
                    &error,
                ))
            })?;
        Ok(CompletedReadonlyObservation {
            observation,
            terminal: event,
            verified,
            links,
            artifact_links,
        })
    }

    /// Workflow #375 R5c: after an observe frame is persisted, under the instance's admission
    /// lock (so the host's order is the ledger's), compares it with the previous observe frame
    /// of its instance and origin. A near-duplicate captured less than 50 s after it gets one
    /// material-preserving `capture.dedup_window`, linked like every marker to the
    /// representative (the previous frame) and preserving the new frame.
    fn mark_observe_duplicate(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        links: &EventLinksDraft,
        current: ObserveMark,
    ) -> Result<(), RequestFailure> {
        // Observe links always name the instance; a frame without one has no chain.
        let Some(instance_id) = links.instance_id().copied() else {
            return Ok(());
        };
        let key = (
            instance_id,
            frame_retention::capture_pin_reason(request)
                == actingcommand_contract::ArtifactPinReason::Lab,
        );
        let mut marks = lock(&self.observe_marks, "lock_observe_marks")?;
        if let Some(marker) = marks.marker(key, &current) {
            self.append_event(
                EventSeverity::Info,
                EventSource::System,
                OriginModule::CapturePipeline,
                EventActor::System,
                marker.representative,
                CapturePayloadDraft::dedup_window_preserving_material(
                    links,
                    marker.duration_ms,
                    AuditInput::new(),
                ),
            )?;
        }
        marks.record(key, current);
        Ok(())
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

#[cfg(test)]
mod observe_mark_tests {
    use super::*;

    fn mark(captured_at_unix_ms: u64, shade: u8) -> ObserveMark {
        let frame = actingcommand_device::Frame::from_pixels(
            2,
            1,
            vec![shade; 6],
            actingcommand_device::PixelFormat::Rgb8,
            actingcommand_device::CaptureBackendName::AdbScreencap,
        )
        .expect("frame");
        ObserveMark {
            links: EventLinksDraft::default(),
            thumbnail: actingcommand_artifact_store::FrameThumbnail::of_frame(&frame),
            captured_at_unix_ms,
        }
    }

    #[test]
    fn observe_frames_chain_per_instance_and_origin_within_50_seconds() {
        let instance = *actingcommand_contract::IdentifierIssuer::new()
            .expect("issuer")
            .mint_instance_id()
            .expect("instance")
            .transport();
        let explicit = (instance, false);
        let lab = (instance, true);
        let mut marks = ObserveMarks::default();
        // The first observe capture after a start has no predecessor.
        assert!(marks.marker(explicit, &mark(1_000, 10)).is_none());
        marks.record(explicit, mark(1_000, 10));
        // A Lab capture never chains with an actingctl capture.
        assert!(marks.marker(lab, &mark(2_000, 10)).is_none());
        marks.record(lab, mark(2_000, 10));
        // Identical and less than 50 s later: marked against the previous frame of its origin.
        assert_eq!(
            marks
                .marker(explicit, &mark(50_999, 10))
                .map(|marker| marker.duration_ms),
            Some(49_999)
        );
        // 50 s or more later starts a new chain; so does a frame that is not similar.
        assert!(marks.marker(explicit, &mark(51_000, 10)).is_none());
        assert!(marks.marker(explicit, &mark(2_000, 200)).is_none());
    }
}
