// SPDX-License-Identifier: AGPL-3.0-only

struct LabRunContext<'a, L: LedgerSink> {
    clock: &'a dyn Clock,
    process: crate::LabRunProcessContext,
    id_issuer: IdIssuer,
    run_id: String,
    run_seed: u64,
    started_at: SystemTime,
    started_instant: Instant,
    run_root: PathBuf,
    run_dir: PathBuf,
    output_dir: PathBuf,
    logs_dir: PathBuf,
    screenshots_dir: PathBuf,
    ledger_session: L::RunSession,
    ledger_started: bool,
    ledger_path: Option<PathBuf>,
    ledger_dispatch_written: bool,
    input_zip_sha256: Option<String>,
    input_entries: Vec<String>,
    requested_capture_interval_ms: u64,
    screenshot_names: ScreenshotNameAllocator,
    screenshots: Vec<ScreenshotRecord>,
    screenshot_evidence: Vec<Value>,
    frame_store: FrameStore,
    frame_evidence: Option<PortableFrameEvidenceProjection>,
    recognition: Vec<Value>,
    events: Vec<Value>,
    steps: Vec<Value>,
    intervals_ms: Vec<u64>,
    capture_durations_ms: Vec<u64>,
    action_durations_ms: Vec<u64>,
    loop_lag_ms: Vec<u64>,
    last_capture_at: Option<Instant>,
    frame_index: usize,
    phase: String,
    control: Option<LabControl>,
    instance: Option<String>,
    adb_path: Option<String>,
    capture_backend_requested: Option<CaptureBackendChoice>,
    capture_backend_used: Option<CaptureBackendName>,
    capture_backend_attempts: Vec<CaptureBackendAttempt>,
    partial_output: bool,
    current_step_index: Option<usize>,
    current_step_id: Option<String>,
    current_operation_id: Option<String>,
    expected_page: Option<String>,
}

impl<'a, L: LedgerSink> LabRunContext<'a, L> {
    fn create_with_context(
        run_root: &Path,
        input_zip: &Path,
        process: crate::LabRunProcessContext,
        clock: &'a dyn Clock,
        ledger_session: L::RunSession,
    ) -> CliOutcome<Self> {
        let now = now_system_time(clock)?;
        let issuer = IdIssuer::new();
        let run_id = issuer.issue(IdKind::Run).value;
        let run_dir = run_root.join(&run_id);
        let output_dir = run_dir.join("output");
        let logs_dir = output_dir.join("logs");
        let screenshots_dir = output_dir.join("screenshots");
        fs::create_dir_all(&logs_dir).map_err(|err| {
            CliError::package_invalid(format!("failed to create {}: {err}", logs_dir.display()))
        })?;
        fs::create_dir_all(&screenshots_dir).map_err(|err| {
            CliError::package_invalid(format!(
                "failed to create {}: {err}",
                screenshots_dir.display()
            ))
        })?;
        let screenshot_names =
            ScreenshotNameAllocator::new(&screenshots_dir).map_err(map_artifact_error)?;
        let frame_store = FrameStore::new(
            run_dir.join("frame-store-temp"),
            FrameStoreConfig::default().with_memory_source(process.memory_source),
        )
        .map_err(map_artifact_error)?;
        Ok(Self {
            clock,
            process,
            id_issuer: issuer,
            run_id,
            run_seed: hash_text(&input_zip.display().to_string()),
            started_at: now,
            started_instant: Instant::now(),
            run_root: run_root.to_path_buf(),
            run_dir,
            output_dir,
            logs_dir,
            screenshots_dir,
            ledger_session,
            ledger_started: false,
            ledger_path: None,
            ledger_dispatch_written: false,
            input_zip_sha256: None,
            input_entries: Vec::new(),
            requested_capture_interval_ms: DEFAULT_CAPTURE_INTERVAL_MS,
            screenshot_names,
            screenshots: Vec::new(),
            screenshot_evidence: Vec::new(),
            frame_store,
            frame_evidence: None,
            recognition: Vec::new(),
            events: Vec::new(),
            steps: Vec::new(),
            intervals_ms: Vec::new(),
            capture_durations_ms: Vec::new(),
            action_durations_ms: Vec::new(),
            loop_lag_ms: Vec::new(),
            last_capture_at: None,
            frame_index: 0,
            phase: "created".to_string(),
            control: None,
            instance: None,
            adb_path: None,
            capture_backend_requested: None,
            capture_backend_used: None,
            capture_backend_attempts: Vec::new(),
            partial_output: false,
            current_step_index: None,
            current_step_id: None,
            current_operation_id: None,
            expected_page: None,
        })
    }

    fn set_phase(&mut self, phase: &str) {
        self.phase = phase.to_string();
    }

    fn now(&self) -> CliOutcome<SystemTime> {
        now_system_time(self.clock)
    }

    fn sleep_ms(&self, duration_ms: u64) {
        if duration_ms > 0 {
            self.clock.sleep(Duration::from_millis(duration_ms));
        }
    }

    fn set_step_context(&mut self, step_index: usize, operation: &Operation) {
        self.current_step_index = Some(step_index);
        self.current_step_id = Some(operation.id.clone());
        self.current_operation_id = Some(operation.id.clone());
        self.expected_page = operation.to.clone();
    }

    fn clear_step_context(&mut self) {
        self.current_step_index = None;
        self.current_step_id = None;
        self.current_operation_id = None;
        self.expected_page = None;
    }

    fn record_capture_backend_selection(&mut self) -> CliOutcome<()> {
        let attempts = self
            .capture_backend_attempts
            .iter()
            .map(capture_backend_attempt_json)
            .collect::<Vec<_>>();
        self.append_ledger_record(
            self.ledger_record(
                LedgerRecordKind::Drive,
                json!({
                    "record_type": "capture_backend_selection",
                    "phase": self.phase,
                    "requested": self.capture_backend_requested.map(|backend| backend.as_str()),
                    "used": self.capture_backend_used.map(|backend| backend.as_str()),
                    "attempt_count": attempts.len(),
                    "attempts": attempts
                }),
            ),
            "ledger_capture_backend_selection",
        )
    }

    fn append_step_record(&mut self, step_record: Value, action_id: &str) -> CliOutcome<()> {
        self.append_ledger_record(
            self.ledger_record(
                LedgerRecordKind::Drive,
                json!({
                    "record_type": "step",
                    "phase": self.phase,
                    "step": step_record.clone()
                }),
            )
            .with_id("action_id", action_id.to_string()),
            "ledger_step",
        )?;
        self.steps.push(step_record);
        Ok(())
    }

    fn ensure_ledger(&mut self) -> CliOutcome<()> {
        if self.ledger_started {
            return Ok(());
        }
        let instance = self.instance.as_deref().unwrap_or("unknown").to_string();
        let control = self.control.as_ref();
        let header = SessionHeader::new(
            "runtime-embedded-lab1y",
            control
                .map(|control| control.game.as_str())
                .unwrap_or("unknown"),
            control
                .map(|control| control.server.as_str())
                .unwrap_or("unknown"),
            &instance,
        );
        let path = L::start_run_session(
            &mut self.ledger_session,
            crate::RunLedgerSessionRequest::new(
                self.run_root.clone(),
                self.run_id.clone(),
                instance,
                crate::LedgerSessionHeader::from_storage(header),
            ),
        )
        .map_err(|err| self.ledger_failure(err.message, "ledger_create"))?;
        let backlog = self.events.clone();
        for event in backlog {
            let light_event = self.light_event_from_legacy_event(&event)?;
            L::append_run_event(
                &mut self.ledger_session,
                crate::LedgerEventEntry::from_storage(light_event),
            )
            .map_err(|err| self.ledger_failure(err.message, "ledger_backfill_event"))?;
        }
        self.ledger_path = Some(path);
        self.ledger_started = true;
        self.write_dispatch_record()
    }

    fn write_dispatch_record(&mut self) -> CliOutcome<()> {
        if self.ledger_dispatch_written {
            return Ok(());
        }
        let record = self.ledger_record(
            LedgerRecordKind::Dispatch,
            json!({
                "record_type": "lab_run_dispatch",
                "command": "lab run",
                "phase": self.phase,
                "input_summary": self.input_summary()
            }),
        );
        self.append_ledger_record(record, "ledger_dispatch")?;
        self.ledger_dispatch_written = true;
        Ok(())
    }

    fn append_ledger_event(&mut self, event: &Value) -> CliOutcome<()> {
        let light_event = self.light_event_from_legacy_event(event)?;
        if !self.ledger_started {
            return Ok(());
        }
        L::append_run_event(
            &mut self.ledger_session,
            crate::LedgerEventEntry::from_storage(light_event),
        )
        .map_err(|err| self.ledger_failure(err.message, "ledger_event"))
    }

    fn append_ledger_record(
        &mut self,
        record: LedgerRecord,
        failure_phase: &str,
    ) -> CliOutcome<()> {
        if !self.ledger_started {
            return Err(self.ledger_failure(
                "invalid lab logging input: runtime ledger handle is unavailable".to_string(),
                failure_phase,
            ));
        }
        L::append_run_record(
            &mut self.ledger_session,
            crate::LedgerRecordEntry::from_storage(record),
        )
        .map_err(|err| self.ledger_failure(err.message, failure_phase))
    }

    fn ledger_record(&self, kind: LedgerRecordKind, payload: Value) -> LedgerRecord {
        let mut record = LedgerRecord::new(kind, None, payload);
        for (key, value) in self.id_chain() {
            record = record.with_id(key, value);
        }
        record
    }

    fn id_chain(&self) -> BTreeMap<String, String> {
        let mut ids = BTreeMap::from([("run_id".to_string(), self.run_id.clone())]);
        if let Some(instance) = &self.instance {
            ids.insert("instance_id".to_string(), instance.clone());
        }
        if let Some(control) = &self.control {
            ids.insert("task_id".to_string(), control.entry_task_id.clone());
        }
        ids
    }

    fn input_summary(&self) -> Value {
        json!({
            "input_zip_sha256": self.input_zip_sha256,
            "entry_count": self.input_entries.len(),
            "run_id": self.run_id,
            "instance": self.instance,
            "phase": self.phase
        })
    }

    fn light_event_from_legacy_event(&self, event: &Value) -> CliOutcome<LightEvent> {
        let event_name = event
            .get("event")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        LightEvent::new(
            format!("lab.{event_name}.event"),
            self.id_chain(),
            event.clone(),
        )
        .map_err(|err| self.ledger_failure(err.to_string(), "ledger_event_shape"))
    }

    fn ledger_failure(&self, message: String, phase: &str) -> CliError {
        let attempted_ledger_path = self
            .ledger_path
            .as_ref()
            .map(|path| path.display().to_string());
        let last_resort = crate::LedgerLastResort::from_storage(LastResortError::new(
            "lab run",
            phase,
            "runtime_ledger_failed",
            &message,
            self.input_summary(),
            attempted_ledger_path,
        ));
        let last_resort_result = L::write_run_last_resort(Some(&self.run_root), &last_resort);
        let suffix = match last_resort_result {
            Ok(path) => format!("; last-resort error file written to {}", path.display()),
            Err(last_resort_err) => {
                format!(
                    "; additionally failed to write last-resort error file: {}",
                    last_resort_err.message
                )
            }
        };
        CliError::package_invalid(format!(
            "runtime ledger failure during {phase}: {message}{suffix}"
        ))
    }

    fn set_frame_store_config(&mut self, config: FrameStoreConfig) -> CliOutcome<()> {
        self.frame_store
            .set_config(config)
            .map_err(map_artifact_error)
    }

    fn event(&mut self, event: &str, data: Value) -> CliOutcome<()> {
        let mut object = serde_json::Map::new();
        object.insert("event".to_string(), json!(event));
        object.insert(
            "timestamp".to_string(),
            json!(timestamp_iso(self.now()?)),
        );
        object.insert("phase".to_string(), json!(self.phase));
        object.insert("data".to_string(), data);
        let event = Value::Object(object);
        self.append_ledger_event(&event)?;
        self.events.push(event);
        Ok(())
    }

    fn wait_for_next_capture_start(&mut self) {
        let Some(last) = self.last_capture_at else {
            return;
        };
        let interval = Duration::from_millis(self.requested_capture_interval_ms.max(1));
        let target = last + interval;
        let now = Instant::now();
        if now < target {
            self.clock.sleep(target.duration_since(now));
        } else {
            self.loop_lag_ms
                .push(now.duration_since(target).as_millis() as u64);
        }
    }

    fn capture_scene_with_pages(
        &mut self,
        capture: &mut dyn CaptureBackend,
        evaluator: &RecognitionEvaluator,
        detector: &PageDetector,
        label: &str,
        candidate_pages: Option<&[String]>,
    ) -> CliOutcome<CapturedScene> {
        let now = Instant::now();
        if let Some(last) = self.last_capture_at.replace(now) {
            self.intervals_ms
                .push(now.duration_since(last).as_millis() as u64);
        }
        let frame = capture
            .capture()
            .map_err(|err| CliError::device(err.to_string()))?;
        self.capture_durations_ms
            .push(now.elapsed().as_millis() as u64);
        self.frame_index += 1;
        let file_name = self.next_screenshot_name(self.now()?)?;
        let width = frame.width;
        let height = frame.height;
        let backend = frame.backend_name.as_str();
        let pixel_format = frame.pixel_format.as_str();
        let captured_at = frame.captured_at;
        let scene = scene_from_frame(&frame)?;
        let evaluations = match candidate_pages {
            Some(pages) => pages
                .iter()
                .map(|page| detector.evaluate_page(evaluator, &scene, page))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|err| CliError::device(err.to_string()))?,
            None => detector
                .evaluate_all(evaluator, &scene)
                .map_err(|err| CliError::device(err.to_string()))?,
        };
        let matched_page = evaluations
            .iter()
            .find(|evaluation| evaluation.matched)
            .map(|evaluation| evaluation.page_id.clone());
        let mut store_outcome = self
            .frame_store
            .add_frame(FrameStoreFrameInput {
                frame_index: self.frame_index,
                file_name,
                label: label.to_string(),
                recognition_state: RecognitionState::from_matched_page(matched_page.clone()),
                pinned_reason: None,
                frame,
            })
            .map_err(map_artifact_error)?;
        if let Some(checkpoint) = store_outcome.checkpoint.as_mut() {
            self.fill_pause_checkpoint(checkpoint, matched_page.as_deref());
        }
        let retained_file = store_outcome.file.clone();
        let merged_into = store_outcome.merged_into.clone();
        let pause_checkpoint = store_outcome
            .checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.to_json());
        self.event(
            "screenshot_recorded",
            json!({
                "frame_index": self.frame_index,
                "file": retained_file.clone(),
                "retained": store_outcome.retained,
                "merged_into": merged_into.clone(),
                "storage_state": store_outcome.storage_state.as_str(),
                "tier1_active": store_outcome.tier1_active,
                "tier2_active": store_outcome.tier2_active,
                "tier3_triggered": store_outcome.tier3_triggered,
                "backpressure_state": store_outcome.backpressure_state.as_str(),
                "pause_required": store_outcome.pause_required,
                "warnings": store_outcome.warnings.clone(),
                "pause_checkpoint": pause_checkpoint,
                "width": width,
                "height": height,
                "backend": backend,
                "pixel_format": pixel_format,
                "captured_at": timestamp_iso(captured_at),
                "label": label
            }),
        )?;
        let reco_id = self.id_issuer.issue(IdKind::Reco).value;
        let candidates = evaluations
            .iter()
            .map(page_evaluation_json)
            .collect::<Vec<_>>();
        let diagnostics = json!({"label": label});
        let recognition_evidence_id = self.id_issuer.issue(IdKind::Evidence).value;
        let recognition_detail = json!({
            "reco_id": reco_id.as_str(),
            "frame_index": self.frame_index,
            "matched_page": matched_page.clone(),
            "candidates": candidates.clone(),
            "diagnostics": diagnostics.clone()
        });
        let recognition_evidence =
            self.store_json_evidence(&recognition_evidence_id, "recognition", &recognition_detail);
        if recognition_evidence.get("status").and_then(Value::as_str) == Some("degraded") {
            self.event(
                "recognition_evidence_degraded",
                json!({
                    "reco_id": reco_id.as_str(),
                    "evidence_id": recognition_evidence_id.as_str(),
                    "evidence": recognition_evidence.clone()
                }),
            )?;
        }
        let recognition = json!({
            "reco_id": reco_id.as_str(),
            "evidence_id": recognition_evidence_id.as_str(),
            "evidence": recognition_evidence,
            "timestamp": timestamp_iso(self.now()?),
            "frame_index": self.frame_index,
            "file": retained_file,
            "retained": store_outcome.retained,
            "merged_into": merged_into,
            "storage_state": store_outcome.storage_state.as_str(),
            "backpressure_state": store_outcome.backpressure_state.as_str(),
            "matched_page": matched_page.clone(),
            "candidates": candidates,
            "diagnostics": diagnostics
        });
        self.append_ledger_record(
            self.ledger_record(
                LedgerRecordKind::Drive,
                json!({
                    "record_type": "recognition",
                    "phase": self.phase,
                    "recognition": recognition
                }),
            )
            .with_id("reco_id", reco_id.clone())
            .with_id("evidence_id", recognition_evidence_id.clone()),
            "ledger_recognition",
        )?;
        self.recognition.push(recognition);
        self.event(
            "recognition_recorded",
            json!({"frame_index": self.frame_index, "reco_id": reco_id, "matched_page": matched_page}),
        )?;
        if store_outcome.tier3_triggered && !store_outcome.pause_required {
            return self.tier3_resume_check(capture, evaluator, detector, candidate_pages);
        }
        if store_outcome.pause_required {
            self.partial_output = true;
            self.event(
                "backpressure_paused",
                json!({
                    "reason": "tier3",
                    "checkpoint": store_outcome.checkpoint.map(|checkpoint| checkpoint.to_json()),
                    "current_phase": self.phase,
                    "last_frame_index": self.frame_index,
                    "last_matched_page": matched_page,
                    "tier3_mode": "synchronous_graceful_failure",
                    "partial_output": true
                }),
            )?;
            return Err(CliError::device(
                "Lab-1z frame store tier3 pause timed out or could not recover; partial output will be written",
            ));
        }
        Ok(CapturedScene {
            scene,
            matched_page,
            page_evaluations: evaluations,
            verify_template_matched: false,
            width,
            height,
        })
    }

    fn fill_pause_checkpoint(
        &self,
        checkpoint: &mut Tier3PauseCheckpoint,
        matched_page: Option<&str>,
    ) {
        checkpoint.current_step_index = self.current_step_index;
        checkpoint.current_step_id = self.current_step_id.clone();
        checkpoint.current_operation_id = self.current_operation_id.clone();
        checkpoint.current_phase = Some(self.phase.clone());
        checkpoint.expected_page = self.expected_page.clone();
        checkpoint.last_matched_page = matched_page.map(str::to_string);
    }

    fn tier3_resume_check(
        &mut self,
        capture: &mut dyn CaptureBackend,
        evaluator: &RecognitionEvaluator,
        detector: &PageDetector,
        candidate_pages: Option<&[String]>,
    ) -> CliOutcome<CapturedScene> {
        self.event(
            "tier3_resume_capture",
            json!({"reason": "resident_bytes_below_release_line"}),
        )?;
        let started = Instant::now();
        let frame = capture
            .capture()
            .map_err(|err| CliError::device(err.to_string()))?;
        self.capture_durations_ms
            .push(started.elapsed().as_millis() as u64);
        let width = frame.width;
        let height = frame.height;
        let scene = scene_from_frame(&frame)?;
        let evaluations = match candidate_pages {
            Some(pages) => pages
                .iter()
                .map(|page| detector.evaluate_page(evaluator, &scene, page))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|err| CliError::device(err.to_string()))?,
            None => detector
                .evaluate_all(evaluator, &scene)
                .map_err(|err| CliError::device(err.to_string()))?,
        };
        let matched_page = evaluations
            .iter()
            .find(|evaluation| evaluation.matched)
            .map(|evaluation| evaluation.page_id.clone());
        let allowed = match (&matched_page, candidate_pages) {
            (Some(page), Some(pages)) => pages.iter().any(|candidate| candidate == page),
            (Some(_), None) => true,
            (None, _) => false,
        };
        self.event(
            "tier3_resume_page_check",
            json!({"matched_page": matched_page, "allowed": allowed}),
        )?;
        if !allowed {
            self.event(
                "tier3_resume_blocked",
                json!({"matched_page": matched_page, "reason": "resume page check failed"}),
            )?;
            return Err(CliError::device(
                "Lab-1z tier3 resume blocked; manual review required",
            ));
        }
        self.event(
            "tier3_resume_allowed",
            json!({"matched_page": matched_page}),
        )?;
        Ok(CapturedScene {
            scene,
            matched_page,
            page_evaluations: evaluations,
            verify_template_matched: false,
            width,
            height,
        })
    }

    fn next_screenshot_name(&mut self, now: SystemTime) -> CliOutcome<String> {
        let timestamp_unix_ms = now
            .duration_since(UNIX_EPOCH)
            .map_err(|error| CliError::device(format!("screenshot timestamp precedes Unix epoch: {error}")))?
            .as_millis();
        let timestamp_unix_ms = u64::try_from(timestamp_unix_ms)
            .map_err(|_| CliError::device("screenshot timestamp exceeds u64 milliseconds"))?;
        self.screenshot_names
            .allocate(timestamp_unix_ms)
            .map_err(map_artifact_error)
    }

    fn finish(
        &mut self,
        out_path: &Path,
        ok: bool,
        failure_reason: Option<&str>,
        state: Option<&RunState>,
    ) -> CliOutcome<ArchiveResult> {
        self.ensure_ledger()?;
        let final_event = if ok { "run_finished" } else { "run_failed" };
        self.event(
            final_event,
            json!({"ok": ok, "failure_reason": failure_reason}),
        )?;
        self.frame_store
            .materialize(&self.screenshots_dir)
            .map_err(map_artifact_error)?;
        self.screenshots = self.frame_store.screenshots();
        self.frame_evidence = Some(
            self.frame_store
                .portable_evidence_projection()
                .map_err(map_artifact_error)?,
        );
        self.event(
            "frame_store_materialized",
            json!({
                "screenshot_count": self.screenshots.len(),
                "frame_evidence": self.frame_evidence.as_ref()
            }),
        )?;
        self.index_screenshot_evidence()?;
        for warning in self.frame_store.cleanup_temp() {
            self.event(
                "frame_store_temp_cleanup_warning",
                json!({"severity": "warning", "message": warning}),
            )?;
        }
        let summary = self.summary_json(ok, failure_reason, state)?;
        let diagnostics = self.diagnostics_json(failure_reason, state);
        let environment = self.environment_json(state)?;
        self.append_finalizing_record(ok, failure_reason, summary, diagnostics, environment)?;
        let committed = match commit_then_record(|| {
            self.write_logs(ok, failure_reason, state)?;
            write_portable_projection_archive(&self.output_dir, out_path)
                .map_err(map_artifact_error)
        }) {
            Ok(proof) => proof,
            Err(err) => return self.record_terminal_output_failure(out_path, err),
        };
        self.event(
            "output_zip_written",
            json!({"out": out_path, "sha256": committed.value().sha256.clone()}),
        )?;
        self.append_terminal_receipt(ok, failure_reason, state, Some(&committed))?;
        let archive = committed.into_inner();
        if ok {
            self.cleanup_run_dir();
        }
        Ok(archive)
    }

    fn index_screenshot_evidence(&mut self) -> CliOutcome<()> {
        self.screenshot_evidence.clear();
        if self.screenshots.is_empty() {
            return Ok(());
        }
        let store = EvidenceStore::new(&self.run_root, true);
        let mut degraded = Vec::new();
        for screenshot in &self.screenshots {
            let evidence_id = self.id_issuer.issue(IdKind::Evidence).value;
            let path = self.output_dir.join(&screenshot.file);
            let mut record = json!({
                "frame_index": screenshot.frame_index,
                "file": screenshot.file,
                "evidence_id": evidence_id,
                "status": "indexed",
                "refs": []
            });
            match fs::read(&path) {
                Ok(bytes) => match store.put(&evidence_id, "screenshot", &bytes) {
                    Ok(Some(reference)) => {
                        record["refs"] = json!([reference]);
                    }
                    Ok(None) => {
                        record["status"] = json!("degraded");
                        record["warnings"] =
                            json!(["evidence store debug mode disabled during lab run"]);
                    }
                    Err(err) => {
                        record["status"] = json!("degraded");
                        record["warnings"] = json!([format!(
                            "failed to store screenshot evidence {}: {err}",
                            path.display()
                        )]);
                    }
                },
                Err(err) => {
                    record["status"] = json!("degraded");
                    record["warnings"] = json!([format!(
                        "failed to read screenshot evidence {}: {err}",
                        path.display()
                    )]);
                }
            }
            if record.get("status").and_then(Value::as_str) == Some("degraded") {
                degraded.push(record.clone());
            }
            self.screenshot_evidence.push(record);
        }
        self.append_ledger_record(
            self.ledger_record(
                LedgerRecordKind::Drive,
                json!({
                    "record_type": "evidence_index",
                    "phase": self.phase,
                    "evidence_kind": "screenshots",
                    "screenshot_count": self.screenshots.len(),
                    "indexed_count": self.screenshot_evidence.iter().filter(|item| item.get("status").and_then(Value::as_str) == Some("indexed")).count(),
                    "degraded_count": degraded.len(),
                    "evidence": self.screenshot_evidence.clone()
                }),
            ),
            "ledger_evidence_index",
        )?;
        if !degraded.is_empty() {
            self.event(
                "evidence_index_degraded",
                json!({
                    "evidence_kind": "screenshots",
                    "degraded_count": degraded.len(),
                    "degraded": degraded
                }),
            )?;
        }
        Ok(())
    }

    fn store_json_evidence(&self, evidence_id: &str, kind: &str, value: &Value) -> Value {
        let store = EvidenceStore::new(&self.run_root, true);
        match serde_json::to_vec(value) {
            Ok(bytes) => match store.put(evidence_id, kind, &bytes) {
                Ok(Some(reference)) => json!({
                    "evidence_id": evidence_id,
                    "kind": kind,
                    "status": "indexed",
                    "refs": [reference]
                }),
                Ok(None) => json!({
                    "evidence_id": evidence_id,
                    "kind": kind,
                    "status": "degraded",
                    "warnings": ["evidence store debug mode disabled during lab run"]
                }),
                Err(err) => json!({
                    "evidence_id": evidence_id,
                    "kind": kind,
                    "status": "degraded",
                    "warnings": [format!("failed to store {kind} evidence: {err}")]
                }),
            },
            Err(err) => json!({
                "evidence_id": evidence_id,
                "kind": kind,
                "status": "degraded",
                "warnings": [format!("failed to serialize {kind} evidence: {err}")]
            }),
        }
    }

    fn append_finalizing_record(
        &mut self,
        ok: bool,
        failure_reason: Option<&str>,
        summary: Value,
        diagnostics: Value,
        environment: Value,
    ) -> CliOutcome<()> {
        self.append_ledger_record(
            self.ledger_record(
                LedgerRecordKind::Drive,
                json!({
                    "record_type": "finalizing",
                    "status": if ok { "ok" } else { "failed" },
                    "sealed": false,
                    "phase": self.phase,
                    "failure_reason": failure_reason,
                    "input_summary": self.input_summary(),
                    "summary": summary,
                    "diagnostics": diagnostics,
                    "environment": environment
                }),
            ),
            "ledger_finalizing",
        )
    }

    fn append_terminal_receipt(
        &mut self,
        ok: bool,
        failure_reason: Option<&str>,
        state: Option<&RunState>,
        archive: Option<&CommitProof<ArchiveResult>>,
    ) -> CliOutcome<()> {
        let summary = self.summary_json(ok, failure_reason, state)?;
        let diagnostics = self.diagnostics_json(failure_reason, state);
        let environment = self.environment_json(state)?;
        let output_zip = archive.map(|proof| {
            json!({
                "path": proof.value().path.display().to_string(),
                "sha256": proof.value().sha256.clone()
            })
        });
        self.append_ledger_record(
            self.ledger_record(
                LedgerRecordKind::Receipt,
                json!({
                    "record_type": if ok { "finish_ok" } else { "finish_error" },
                    "status": if ok { "ok" } else { "failed" },
                    "phase": self.phase,
                    "failure_reason": failure_reason,
                    "input_summary": self.input_summary(),
                    "summary": summary,
                    "diagnostics": diagnostics,
                    "environment": environment,
                    "output_zip": output_zip
                }),
            ),
            "ledger_terminal_receipt",
        )
    }

    fn record_terminal_output_failure(
        &mut self,
        out_path: &Path,
        err: CliError,
    ) -> CliOutcome<ArchiveResult> {
        let message = err.message.clone();
        let failure_reason = format!("terminal output failed: {message}");
        let last_resort = crate::LedgerLastResort::from_storage(LastResortError::new(
            "lab run",
            "terminal_output",
            &err.code,
            &message,
            json!({
                "run_id": self.run_id,
                "instance": self.instance,
                "phase": self.phase,
                "out": out_path.display().to_string(),
                "input_summary": self.input_summary()
            }),
            self.ledger_path
                .as_ref()
                .map(|path| path.display().to_string()),
        ));
        let last_resort_result = L::write_run_last_resort(Some(&self.run_root), &last_resort);
        self.append_terminal_receipt(false, Some(&failure_reason), None, None)?;
        let suffix = match last_resort_result {
            Ok(path) => format!("; last-resort error file written to {}", path.display()),
            Err(last_resort_err) => {
                format!(
                    "; additionally failed to write last-resort error file: {}",
                    last_resort_err.message
                )
            }
        };
        Err(CliError::package_invalid(format!(
            "{failure_reason}{suffix}"
        )))
    }

    fn cleanup_run_dir(&self) {
        let _ = fs::remove_dir_all(&self.run_dir);
    }

    fn write_logs(
        &self,
        ok: bool,
        failure_reason: Option<&str>,
        state: Option<&RunState>,
    ) -> CliOutcome<()> {
        let projection = self.project_logs_from_ledger()?;
        write_json_lines(&self.logs_dir.join("events.jsonl"), &projection.events)?;
        write_json_lines(
            &self.logs_dir.join("recognition.jsonl"),
            &projection.recognition,
        )?;
        write_json_lines(
            &self.logs_dir.join("frame_timeline.jsonl"),
            &self.frame_store.timeline(),
        )?;
        write_json(
            &self.logs_dir.join("evidence.json"),
            &json!(projection.evidence),
        )?;
        write_json(
            &self.logs_dir.join("frame_store.json"),
            &self.frame_store.diagnostics_json(),
        )?;
        let frame_evidence = self.frame_evidence.as_ref().ok_or_else(|| {
            CliError::package_invalid("final output is missing portable frame evidence")
        })?;
        let frame_evidence = serde_json::to_value(frame_evidence).map_err(|error| {
            CliError::package_invalid(format!(
                "failed to serialize portable frame evidence: {error}"
            ))
        })?;
        write_json(
            &self.logs_dir.join("frame_evidence.json"),
            &frame_evidence,
        )?;
        write_json(&self.logs_dir.join("summary.json"), &projection.summary)?;
        write_json(
            &self.logs_dir.join("diagnostics.json"),
            &projection.diagnostics,
        )?;
        write_json(
            &self.logs_dir.join("environment.json"),
            &projection.environment,
        )?;
        fs::write(
            self.logs_dir.join("result.md"),
            self.result_markdown(ok, failure_reason, state),
        )
        .map_err(|err| CliError::package_invalid(format!("failed to write result.md: {err}")))?;
        Ok(())
    }

    fn project_logs_from_ledger(&self) -> CliOutcome<LabLogProjection> {
        let ledger_path = self.ledger_path.as_ref().ok_or_else(|| {
            CliError::package_invalid(
                "runtime ledger path is unavailable for Lab result projection",
            )
        })?;
        L::sync_run_session(&self.ledger_session)
            .map_err(|err| self.ledger_failure(err.message, "ledger_projection_sync"))?;
        let readback = L::read_run_session(&self.ledger_session)
            .map_err(|err| self.ledger_failure(err.message, "ledger_projection_read"))?;
        let runtime_projection = readback.runtime_projection().ok_or_else(|| {
            CliError::package_invalid(
                "Lab result projection is missing Runtime global-ledger facts",
            )
        })?;
        let read = readback.storage();
        let events = runtime_projection.events().to_vec();
        if events.is_empty() {
            return Err(CliError::package_invalid(
                "Runtime global-ledger projection has no events",
            ));
        }
        let mut recognition = Vec::new();
        let mut steps = Vec::new();
        let mut evidence = Vec::new();
        let mut finalizing_summary = None;
        let mut finalizing_diagnostics = None;
        let mut finalizing_environment = None;
        for record in &read.records {
            match record.payload.get("record_type").and_then(Value::as_str) {
                Some("recognition") => {
                    if let Some(value) = record.payload.get("recognition") {
                        recognition.push(value.clone());
                    }
                }
                Some("step") => {
                    if let Some(value) = record.payload.get("step") {
                        steps.push(value.clone());
                    }
                }
                Some("evidence_index") => {
                    evidence.push(record.payload.clone());
                }
                Some("finalizing") => {
                    finalizing_summary = record.payload.get("summary").cloned();
                    finalizing_diagnostics = record.payload.get("diagnostics").cloned();
                    finalizing_environment = record.payload.get("environment").cloned();
                }
                _ => {}
            }
        }
        let projection_source = json!({
            "kind": "runtime_global_ledger",
            "correlation_id": runtime_projection.correlation_id(),
            "logical_stream": ledger_path.display().to_string(),
            "event_count": runtime_projection.events().len(),
            "record_count": read.records.len(),
            "skipped_corrupt_lines": read.skipped_corrupt_lines
        });
        let mut summary = finalizing_summary.ok_or_else(|| {
            CliError::package_invalid("runtime ledger projection missing finalizing summary")
        })?;
        summary["projection_source"] = projection_source.clone();
        summary["steps"] = Value::Array(steps.clone());
        let mut diagnostics = finalizing_diagnostics.ok_or_else(|| {
            CliError::package_invalid("runtime ledger projection missing finalizing diagnostics")
        })?;
        diagnostics["projection_source"] = projection_source.clone();
        diagnostics["command"] = json!("lab run");
        diagnostics["phase"] = json!(self.phase);
        diagnostics["input_summary"] = self.input_summary();
        let mut environment = finalizing_environment.ok_or_else(|| {
            CliError::package_invalid("runtime ledger projection missing finalizing environment")
        })?;
        environment["projection_source"] = projection_source;
        Ok(LabLogProjection {
            events,
            recognition,
            evidence,
            summary,
            diagnostics,
            environment,
        })
    }

    fn project_completed_run_from_ledger(&self) -> CliOutcome<LabCompletedProjection> {
        let ledger_path = self.ledger_path.as_ref().ok_or_else(|| {
            CliError::package_invalid(
                "runtime ledger path is unavailable for completed Lab run projection",
            )
        })?;
        L::sync_run_session(&self.ledger_session)
            .map_err(|err| self.ledger_failure(err.message, "ledger_completed_projection_sync"))?;
        let readback = L::read_run_session(&self.ledger_session)
            .map_err(|err| self.ledger_failure(err.message, "ledger_completed_projection_read"))?;
        let read = readback.storage();
        let mut saw_finalizing = false;
        let mut terminal_receipt = None;
        for record in &read.records {
            match record.payload.get("record_type").and_then(Value::as_str) {
                Some("finalizing") => saw_finalizing = true,
                Some("finish_ok") | Some("finish_error") => terminal_receipt = Some(record),
                _ => {}
            }
        }
        if !saw_finalizing {
            return Err(CliError::package_invalid(
                "runtime ledger completed projection missing finalizing record",
            ));
        }
        let receipt = terminal_receipt.ok_or_else(|| {
            CliError::package_invalid(
                "runtime ledger completed projection missing terminal receipt",
            )
        })?;
        let record_type = receipt
            .payload
            .get("record_type")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                CliError::package_invalid("runtime ledger terminal receipt missing record_type")
            })?;
        let status = receipt
            .payload
            .get("status")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                CliError::package_invalid("runtime ledger terminal receipt missing status")
            })?;
        let ok = match (record_type, status) {
            ("finish_ok", "ok") => true,
            ("finish_error", "failed") => false,
            _ => {
                return Err(CliError::package_invalid(format!(
                    "runtime ledger terminal receipt has inconsistent record_type/status: {record_type}/{status}"
                )));
            }
        };
        let output_zip = match receipt.payload.get("output_zip") {
            Some(Value::Object(object)) => Some(TerminalOutputZip {
                path: object
                    .get("path")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        CliError::package_invalid(
                            "runtime ledger terminal receipt output_zip missing path",
                        )
                    })?
                    .to_string(),
                sha256: object
                    .get("sha256")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        CliError::package_invalid(
                            "runtime ledger terminal receipt output_zip missing sha256",
                        )
                    })?
                    .to_string(),
            }),
            Some(Value::Null) | None => None,
            Some(_) => {
                return Err(CliError::package_invalid(
                    "runtime ledger terminal receipt output_zip must be an object or null",
                ));
            }
        };
        if ok && output_zip.is_none() {
            return Err(CliError::package_invalid(
                "runtime ledger successful terminal receipt missing output_zip",
            ));
        }
        if ok {
            let output_zip = output_zip.as_ref().ok_or_else(|| {
                CliError::package_invalid(
                    "runtime ledger successful terminal receipt missing output_zip",
                )
            })?;
            let output_path = Path::new(&output_zip.path);
            if !output_path.is_file() {
                return Err(CliError::package_invalid(format!(
                    "runtime ledger successful terminal receipt output_zip file is missing: {}",
                    output_path.display()
                )));
            }
            let actual_sha256 = file_sha256(output_path)?;
            if actual_sha256 != output_zip.sha256 {
                return Err(CliError::package_invalid(format!(
                    "runtime ledger successful terminal receipt output_zip sha256 mismatch: expected {}, got {}",
                    output_zip.sha256, actual_sha256
                )));
            }
        }
        Ok(LabCompletedProjection {
            run_id: self.run_id.clone(),
            status: status.to_string(),
            ok,
            record_type: record_type.to_string(),
            output_zip,
            ledger_path: ledger_path.to_path_buf(),
        })
    }

    fn summary_json(
        &self,
        ok: bool,
        failure_reason: Option<&str>,
        state: Option<&RunState>,
    ) -> CliOutcome<Value> {
        let finished = self.now()?;
        let stats = interval_stats(&self.intervals_ms);
        let capture_stats = interval_stats(&self.capture_durations_ms);
        let action_stats = interval_stats(&self.action_durations_ms);
        let lag_stats = interval_stats(&self.loop_lag_ms);
        let frame_store = self.frame_store.diagnostics_json();
        let screenshots = self
            .screenshots
            .iter()
            .map(|record| {
                let mut item = json!({
                    "frame_index": record.frame_index,
                    "file": record.file,
                    "width": record.width,
                    "height": record.height,
                    "dwell_ms": record.dwell_ms,
                    "merged_count": record.merged_count,
                    "matched_page": record.matched_page,
                    "recognition_state": record.recognition_state.as_json(),
                    "storage_state": record.storage_state.as_str(),
                    "key_frame": record.key_frame
                });
                if let Some(evidence) = self.screenshot_evidence.iter().find(|evidence| {
                    evidence.get("file").and_then(Value::as_str) == Some(record.file.as_str())
                }) {
                    item["evidence"] = evidence.clone();
                }
                item
            })
            .collect::<Vec<_>>();
        let control = self
            .control
            .as_ref()
            .or_else(|| state.map(|state| &state.control));
        let mut summary = json!({
            "schema_version": SUMMARY_SCHEMA,
            "ok": ok,
            "run_id": self.run_id,
            "package_id": control.map(|control| control.package_id.as_str()).unwrap_or("unknown"),
            "game": control.map(|control| control.game.as_str()).unwrap_or("unknown"),
            "server": control.map(|control| control.server.as_str()).unwrap_or("unknown"),
            "instance": self.instance,
            "started_at": timestamp_iso(self.started_at),
            "finished_at": timestamp_iso(finished),
            "duration_ms": self.started_instant.elapsed().as_millis(),
            "input_zip_sha256": self.input_zip_sha256,
            "output_zip_sha256": Value::Null,
            "executed_step_count": self.steps.len(),
            "failed_step_id": state.and_then(|state| state.failed_step_id.as_deref()),
            "failure_reason": failure_reason,
            "partial_output": self.partial_output,
            "screenshot_count": self.screenshots.len(),
            "requested_capture_interval_ms": self.requested_capture_interval_ms,
            "actual_capture_interval_min_ms": stats.map(|stats| stats.min),
            "actual_capture_interval_median_ms": stats.map(|stats| stats.median),
            "actual_capture_interval_max_ms": stats.map(|stats| stats.max),
            "capture_duration_min_ms": capture_stats.map(|stats| stats.min),
            "capture_duration_median_ms": capture_stats.map(|stats| stats.median),
            "capture_duration_max_ms": capture_stats.map(|stats| stats.max),
            "action_duration_min_ms": action_stats.map(|stats| stats.min),
            "action_duration_median_ms": action_stats.map(|stats| stats.median),
            "action_duration_max_ms": action_stats.map(|stats| stats.max),
            "loop_lag_min_ms": lag_stats.map(|stats| stats.min),
            "loop_lag_median_ms": lag_stats.map(|stats| stats.median),
            "loop_lag_max_ms": lag_stats.map(|stats| stats.max),
            "capture_backend_requested": self.capture_backend_requested.map(|backend| backend.as_str()),
            "capture_backend_used": self.capture_backend_used.map(|backend| backend.as_str()),
            "frame_store": frame_store,
            "screenshot_evidence": self.screenshot_evidence.clone(),
            "screenshots": screenshots,
            "steps": self.steps
        });
        if let Some(frame_evidence) = &self.frame_evidence {
            summary["frame_evidence"] = json!(frame_evidence);
        }
        Ok(summary)
    }

    fn diagnostics_json(&self, failure_reason: Option<&str>, state: Option<&RunState>) -> Value {
        let frame_store = self.frame_store.diagnostics_json();
        let mut diagnostics = json!({
            "actinglab_cli_version": self.process.app_version,
            "runtime_version": "runtime-embedded-lab1y",
            "runtime_commit": self.process.runtime_commit_source.sample(),
            "os": self.process.os,
            "timezone": "UTC",
            "adb_path": self.adb_path,
            "serial": self.instance,
            "capture_backend_requested": self.capture_backend_requested.map(|backend| backend.as_str()),
            "capture_backend_used": self.capture_backend_used.map(|backend| backend.as_str()),
            "capture_backend_attempts": self.capture_backend_attempts.iter().map(|attempt| json!({
                "backend": attempt.backend.as_str(),
                "ok": attempt.ok,
                "channel_order_contract": attempt.channel_order_contract,
                "message": attempt.message
            })).collect::<Vec<_>>(),
            "frame_store": frame_store,
            "screenshot_evidence": self.screenshot_evidence.clone(),
            "input_structure": self.input_entries,
            "resource_load_results": state.map(|state| json!({
                "manifest": state.resources.manifest_path,
                "operation": state.resources.operation_path,
                "resource_root": state.resources.resource_root,
                "pack": state.resources.pack_path,
                "recognition_unsupported_target_count": state.resources.evaluator.unsupported_target_count(),
                "recognition_unsupported_targets": unsupported_targets_json(state.resources.evaluator.unsupported_targets()),
                "pages": state.resources.pages_path,
                "navigation": state.resources.navigation_path,
                "navigation_loaded": state.resources.navigation.is_some(),
                "operation_goal": state.resources.operation_bundle.goal,
                "entry_page": state.resources.operation_bundle.entry_page,
                "target_page": state.resources.operation_bundle.target_page,
                "operation_defaults": state.resources.operation_bundle.defaults.to_json()
            })),
            "interval_stats": interval_stats(&self.intervals_ms).map(|stats| json!({
                "min_ms": stats.min,
                "median_ms": stats.median,
                "max_ms": stats.max,
                "count": stats.count
            })),
            "capture_duration_stats": interval_stats(&self.capture_durations_ms).map(|stats| json!({
                "min_ms": stats.min,
                "median_ms": stats.median,
                "max_ms": stats.max,
                "count": stats.count
            })),
            "action_duration_stats": interval_stats(&self.action_durations_ms).map(|stats| json!({
                "min_ms": stats.min,
                "median_ms": stats.median,
                "max_ms": stats.max,
                "count": stats.count
            })),
            "loop_lag_stats": interval_stats(&self.loop_lag_ms).map(|stats| json!({
                "min_ms": stats.min,
                "median_ms": stats.median,
                "max_ms": stats.max,
                "count": stats.count
            })),
            "error": failure_reason.map(|message| json!({
                "code": "lab1y_failed",
                "exception": message,
                "failure_phase": self.phase
            }))
        });
        if let Some(frame_evidence) = &self.frame_evidence {
            diagnostics["frame_evidence"] = json!(frame_evidence);
        }
        diagnostics
    }

    fn environment_json(&self, state: Option<&RunState>) -> CliOutcome<Value> {
        Ok(json!({
            "os": self.process.os,
            "timezone": "UTC",
            "local_time": timestamp_iso(self.now()?),
            "cwd": self.process.current_dir.as_ref().map(|path| path.display().to_string()),
            "run_root": self.run_dir.parent().map(|path| path.display().to_string()),
            "run_dir": self.run_dir,
            "adb_path": self.adb_path,
            "instance_serial": self.instance,
            "runtime_repository_commit": self.process.runtime_commit_source.sample(),
            "control_output": self.control.as_ref().and_then(|control| control.output.clone()),
            "control_stop_on_error": self.control.as_ref().and_then(|control| control.stop_on_error),
            "resource_manifest": state.map(|state| state.resources.manifest.clone())
        }))
    }

    fn result_markdown(
        &self,
        ok: bool,
        failure_reason: Option<&str>,
        state: Option<&RunState>,
    ) -> String {
        let control = self
            .control
            .as_ref()
            .or_else(|| state.map(|state| &state.control));
        format!(
            "# Lab-1y Result\n\n- Package: {}\n- Game: {}\n- Server: {}\n- Instance: {}\n- Success: {}\n- Failure: {}\n- Screenshots: {}\n- Run ID: {}\n",
            control
                .map(|control| control.package_id.as_str())
                .unwrap_or("unknown"),
            control
                .map(|control| control.game.as_str())
                .unwrap_or("unknown"),
            control
                .map(|control| control.server.as_str())
                .unwrap_or("unknown"),
            self.instance.as_deref().unwrap_or("unknown"),
            ok,
            failure_reason.unwrap_or("none"),
            self.screenshots.len(),
            self.run_id
        )
    }
}
