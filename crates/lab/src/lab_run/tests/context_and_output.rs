// SPDX-License-Identifier: AGPL-3.0-only

    #[test]
    fn manifest_entry_task_id_conflict_is_fatal() {
        let control = test_control();
        let manifest = json!({"entry_task_id": "other_task"});

        let err = validate_manifest_entry_task_id(Path::new("manifest.json"), &manifest, &control)
            .expect_err("conflict is fatal");

        assert_eq!(err.code, "package_invalid");
        assert!(err.message.contains("conflicts with control entry_task_id"));
    }

    #[test]
    fn screenshot_names_are_timestamp_based_with_suffixes() {
        let temp = TempDir::new().expect("temp");
        let mut ctx = LabRunContext::create(temp.path(), Path::new("input.zip")).expect("ctx");
        let time = UNIX_EPOCH + Duration::from_millis(1_672_531_200_123);
        let first = ctx.next_screenshot_name(time);
        let second = ctx.next_screenshot_name(time);

        assert!(first.ends_with(".png"));
        assert!(second.ends_with("_02.png"));
        assert!(first.starts_with("20230101_000000_123"));
    }

    #[test]
    fn failure_zip_materializes_frame_store_screenshots() {
        let temp = TempDir::new().expect("temp");
        let mut ctx = LabRunContext::create(temp.path(), Path::new("input.zip")).expect("ctx");
        let frame = Frame::from_pixels(
            1,
            1,
            vec![0, 0, 0, 255],
            PixelFormat::Rgba8,
            CaptureBackendName::NemuIpc,
        )
        .expect("frame");
        ctx.frame_index = 1;
        ctx.frame_store
            .add_frame(FrameStoreFrameInput {
                frame_index: 1,
                file_name: "frame1.png".to_string(),
                label: "initial".to_string(),
                recognition_state: RecognitionState::from_matched_page(Some(
                    "arknights/home".to_string(),
                )),
                frame,
            })
            .expect("frame store");
        let out = temp.path().join("out.zip");

        ctx.finish(&out, false, Some("synthetic failure"), None)
            .expect("finish");

        let file = File::open(&out).expect("zip");
        let mut archive = ZipArchive::new(file).expect("archive");
        assert!(archive.by_name("screenshots/frame1.png").is_ok());
        assert!(archive.by_name("logs/evidence.json").is_ok());
        assert!(archive.by_name("logs/frame_store.json").is_ok());
        assert!(archive.by_name("logs/frame_timeline.jsonl").is_ok());
        let summary: Value =
            serde_json::from_reader(archive.by_name("logs/summary.json").expect("summary"))
                .expect("summary json");
        assert_eq!(
            summary
                .pointer("/projection_source/kind")
                .and_then(Value::as_str),
            Some("runtime_ledger")
        );
        let screenshot_evidence = summary
            .pointer("/screenshots/0/evidence")
            .expect("screenshot evidence");
        assert_eq!(
            screenshot_evidence.get("status").and_then(Value::as_str),
            Some("indexed")
        );
        let evidence_relative_path = screenshot_evidence
            .pointer("/refs/0/relative_path")
            .and_then(Value::as_str)
            .expect("relative evidence path");
        assert!(temp.path().join(evidence_relative_path).is_file());
        let events = zip_text(&mut archive, "logs/events.jsonl");
        assert!(events.contains("runtime_ledger"));
        let diagnostics: Value = serde_json::from_reader(
            archive
                .by_name("logs/diagnostics.json")
                .expect("diagnostics"),
        )
        .expect("diagnostics json");
        assert_eq!(
            diagnostics.get("command").and_then(Value::as_str),
            Some("lab run")
        );
        assert!(ctx.ledger_path.as_ref().expect("ledger path").is_file());
        let ledger = LabLedger::read(ctx.ledger_path.as_ref().unwrap()).expect("ledger read");
        assert!(!ledger.events.is_empty());
        assert!(ledger.records.iter().any(|record| {
            record.kind == LedgerRecordKind::Drive
                && record.payload.get("record_type").and_then(Value::as_str)
                    == Some("evidence_index")
        }));
        assert!(ledger.records.iter().any(|record| {
            record.kind == LedgerRecordKind::Drive
                && record.payload.get("record_type").and_then(Value::as_str) == Some("finalizing")
        }));
        assert!(ledger.records.iter().any(|record| {
            record.kind == LedgerRecordKind::Receipt
                && record.payload.get("record_type").and_then(Value::as_str) == Some("finish_error")
        }));
        assert!(ctx.run_dir.exists());
    }

    #[test]
    fn screenshot_evidence_records_degradation_when_file_is_missing() {
        let temp = TempDir::new().expect("temp");
        let mut ctx = LabRunContext::create(temp.path(), Path::new("input.zip")).expect("ctx");
        let frame = Frame::from_pixels(
            1,
            1,
            vec![0, 0, 0, 255],
            PixelFormat::Rgba8,
            CaptureBackendName::NemuIpc,
        )
        .expect("frame");
        ctx.ensure_ledger().expect("ledger");
        ctx.frame_store
            .add_frame(FrameStoreFrameInput {
                frame_index: 1,
                file_name: "missing.png".to_string(),
                label: "missing".to_string(),
                recognition_state: RecognitionState::from_matched_page(Some(
                    "arknights/home".to_string(),
                )),
                frame,
            })
            .expect("frame store");
        ctx.frame_store
            .materialize(&ctx.screenshots_dir)
            .expect("materialize");
        ctx.screenshots = ctx.frame_store.screenshots();
        let screenshot_path = ctx.output_dir.join(&ctx.screenshots[0].file);
        fs::remove_file(&screenshot_path).expect("remove screenshot");

        ctx.index_screenshot_evidence().expect("index evidence");

        let evidence = ctx.screenshot_evidence.first().expect("evidence");
        assert_eq!(
            evidence.get("status").and_then(Value::as_str),
            Some("degraded")
        );
        assert!(
            evidence
                .get("warnings")
                .and_then(Value::as_array)
                .and_then(|warnings| warnings.first())
                .and_then(Value::as_str)
                .is_some_and(|warning| warning.contains("failed to read screenshot evidence"))
        );
        let ledger = LabLedger::read(ctx.ledger_path.as_ref().unwrap()).expect("ledger read");
        let record = ledger
            .records
            .iter()
            .find(|record| {
                record.kind == LedgerRecordKind::Drive
                    && record.payload.get("record_type").and_then(Value::as_str)
                        == Some("evidence_index")
            })
            .expect("evidence index record");
        assert_eq!(
            record.payload.get("indexed_count").and_then(Value::as_u64),
            Some(0)
        );
        assert_eq!(
            record.payload.get("degraded_count").and_then(Value::as_u64),
            Some(1)
        );
    }

    #[test]
    fn recognition_projection_keeps_reco_id_from_ledger() {
        let temp = TempDir::new().expect("temp");
        let mut ctx = LabRunContext::create(temp.path(), Path::new("input.zip")).expect("ctx");
        ctx.ensure_ledger().expect("ledger");
        let frame = Frame::from_pixels(
            1,
            1,
            vec![0, 0, 0, 255],
            PixelFormat::Rgba8,
            CaptureBackendName::NemuIpc,
        )
        .expect("frame");
        let mut capture = StaticCapture { frame };
        let evaluator = one_pixel_color_evaluator([0, 0, 0]);
        let page_set = actingcommand_page_detector::load_page_set_from_json_str(
            r#"{
                "schema_version":"0.3",
                "pages":[
                    {"id":"arknights/home","required":["target/button"],"optional":[],"forbidden":[]}
                ]
            }"#,
        )
        .expect("page set");
        let detector = PageDetector::new(page_set).expect("detector");

        let scene = ctx
            .capture_scene_with_pages(
                &mut capture,
                &evaluator,
                &detector,
                "initial",
                Some(&["arknights/home".to_string()]),
            )
            .expect("capture scene");
        assert_eq!(scene.matched_page.as_deref(), Some("arknights/home"));
        let out = temp.path().join("out.zip");
        ctx.finish(&out, true, None, None).expect("finish");

        let file = File::open(&out).expect("zip");
        let mut archive = ZipArchive::new(file).expect("archive");
        let recognition_text = zip_text(&mut archive, "logs/recognition.jsonl");
        let recognition = recognition_text
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("recognition line"))
            .collect::<Vec<_>>();
        let recognition_item = recognition.first().expect("recognition item");
        let reco_id = recognition_item
            .get("reco_id")
            .and_then(Value::as_str)
            .expect("reco_id");
        assert!(reco_id.starts_with("reco-"));
        let evidence_id = recognition_item
            .get("evidence_id")
            .and_then(Value::as_str)
            .expect("evidence_id");
        assert!(evidence_id.starts_with("evidence-"));
        let evidence_ref = recognition_item
            .pointer("/evidence/refs/0/relative_path")
            .and_then(Value::as_str)
            .expect("evidence relative path");
        let evidence_bytes =
            fs::read(temp.path().join(evidence_ref)).expect("recognition evidence");
        let evidence_detail: Value =
            serde_json::from_slice(&evidence_bytes).expect("recognition evidence json");
        assert_eq!(
            evidence_detail.get("reco_id").and_then(Value::as_str),
            Some(reco_id)
        );
        assert_eq!(
            recognition_item
                .pointer("/evidence/status")
                .and_then(Value::as_str),
            Some("indexed")
        );
        let ledger = LabLedger::read(ctx.ledger_path.as_ref().unwrap()).expect("ledger read");
        let record = ledger
            .records
            .iter()
            .find(|record| {
                record.kind == LedgerRecordKind::Drive
                    && record.payload.get("record_type").and_then(Value::as_str)
                        == Some("recognition")
            })
            .expect("recognition record");
        assert_eq!(
            record.id_chain.get("reco_id").map(String::as_str),
            Some(reco_id)
        );
        assert_eq!(
            record.id_chain.get("evidence_id").map(String::as_str),
            Some(evidence_id)
        );
        assert_eq!(
            record
                .payload
                .pointer("/recognition/reco_id")
                .and_then(Value::as_str),
            Some(reco_id)
        );
    }

    #[test]
    fn capture_backend_selection_is_recorded_in_ledger() {
        let temp = TempDir::new().expect("temp");
        let mut ctx = LabRunContext::create(temp.path(), Path::new("input.zip")).expect("ctx");
        ctx.ensure_ledger().expect("ledger");
        ctx.capture_backend_requested = Some(CaptureBackendChoice::Auto);
        ctx.capture_backend_used = Some(CaptureBackendName::NemuIpc);
        ctx.capture_backend_attempts = vec![CaptureBackendAttempt {
            backend: CaptureBackendName::NemuIpc,
            ok: true,
            message: "selected nemu ipc".to_string(),
            elapsed_ms: Some(12),
            cached: false,
            channel_order_contract: "test-order",
            vendor_stdio: Vec::new(),
        }];

        ctx.record_capture_backend_selection()
            .expect("record capture selection");

        let ledger = LabLedger::read(ctx.ledger_path.as_ref().unwrap()).expect("ledger read");
        let record = ledger
            .records
            .iter()
            .find(|record| {
                record.kind == LedgerRecordKind::Drive
                    && record.payload.get("record_type").and_then(Value::as_str)
                        == Some("capture_backend_selection")
            })
            .expect("capture selection record");
        assert_eq!(
            record.payload.get("requested").and_then(Value::as_str),
            Some("auto")
        );
        assert_eq!(
            record.payload.get("used").and_then(Value::as_str),
            Some("nemu_ipc")
        );
        assert_eq!(
            record.payload.get("attempt_count").and_then(Value::as_u64),
            Some(1)
        );
        assert_eq!(
            record
                .payload
                .pointer("/attempts/0/backend")
                .and_then(Value::as_str),
            Some("nemu_ipc")
        );
        assert_eq!(
            record
                .payload
                .pointer("/attempts/0/message")
                .and_then(Value::as_str),
            Some("selected nemu ipc")
        );
    }

    #[test]
    fn step_record_keeps_action_id_in_ledger_chain() {
        let temp = TempDir::new().expect("temp");
        let mut ctx = LabRunContext::create(temp.path(), Path::new("input.zip")).expect("ctx");
        ctx.ensure_ledger().expect("ledger");
        let action_id = ctx.id_issuer.issue(IdKind::Action).value;
        let step = json!({
            "id": "open_terminal",
            "action_id": action_id.as_str(),
            "result": "verified"
        });

        ctx.append_step_record(step, &action_id)
            .expect("append step record");

        let ledger = LabLedger::read(ctx.ledger_path.as_ref().unwrap()).expect("ledger read");
        let record = ledger
            .records
            .iter()
            .find(|record| {
                record.kind == LedgerRecordKind::Drive
                    && record.payload.get("record_type").and_then(Value::as_str) == Some("step")
            })
            .expect("step record");
        assert_eq!(
            record.id_chain.get("action_id").map(String::as_str),
            Some(action_id.as_str())
        );
        assert_eq!(
            record
                .payload
                .pointer("/step/action_id")
                .and_then(Value::as_str),
            Some(action_id.as_str())
        );
        assert_eq!(
            ctx.steps
                .first()
                .and_then(|step| step.get("action_id"))
                .and_then(Value::as_str),
            Some(action_id.as_str())
        );
    }

    #[test]
    fn success_finish_cleans_run_dir_but_keeps_outside_zip() {
        let temp = TempDir::new().expect("temp");
        let mut ctx = LabRunContext::create(temp.path(), Path::new("input.zip")).expect("ctx");
        let out = temp.path().join("out.zip");

        ctx.finish(&out, true, None, None).expect("finish");

        assert!(out.is_file());
        assert!(!ctx.run_dir.exists());
        assert!(ctx.ledger_path.as_ref().expect("ledger path").is_file());
        let ledger_text =
            fs::read_to_string(ctx.ledger_path.as_ref().unwrap()).expect("ledger text");
        assert_ordered(
            &ledger_text,
            &[
                "\"record_type\":\"finalizing\"",
                "\"event\":\"output_zip_written\"",
                "\"record_type\":\"finish_ok\"",
            ],
        );
    }

    #[test]
    fn completed_projection_reports_terminal_output_zip() {
        let temp = TempDir::new().expect("temp");
        let mut ctx = LabRunContext::create(temp.path(), Path::new("input.zip")).expect("ctx");
        let out = temp.path().join("out.zip");

        let archive = ctx.finish(&out, true, None, None).expect("finish");
        let completed = ctx
            .project_completed_run_from_ledger()
            .expect("completed projection");
        let output_zip = completed.require_output_zip().expect("output zip");

        assert!(completed.ok);
        assert_eq!(completed.status, "ok");
        assert_eq!(completed.record_type, "finish_ok");
        assert_eq!(completed.run_id, ctx.run_id);
        assert_eq!(output_zip.path, out.display().to_string());
        assert_eq!(output_zip.sha256, archive.sha256);
        assert_eq!(
            completed.ledger_path,
            ctx.ledger_path.as_ref().expect("ledger path").to_path_buf()
        );
    }

    #[test]
    fn completed_projection_rejects_finish_ok_with_missing_output_zip_file() {
        let temp = TempDir::new().expect("temp");
        let mut ctx = LabRunContext::create(temp.path(), Path::new("input.zip")).expect("ctx");
        ctx.ensure_ledger().expect("ledger");
        let summary = ctx.summary_json(true, None, None).expect("summary");
        let diagnostics = ctx.diagnostics_json(None, None);
        let environment = ctx.environment_json(None).expect("environment");
        ctx.append_finalizing_record(true, None, summary, diagnostics, environment)
            .expect("finalizing");
        let archive = commit_then_record(|| -> Result<_, LabLogError> {
            Ok(ArchiveResult {
                path: temp.path().join("missing.zip"),
                sha256: "missing".to_string(),
            })
        })
        .expect("commit proof");
        ctx.append_terminal_receipt(true, None, None, Some(&archive))
            .expect("terminal receipt");

        let err = ctx
            .project_completed_run_from_ledger()
            .expect_err("missing output zip file must fail");

        assert!(err.message.contains("output_zip file is missing"));
    }

    #[test]
    fn completed_projection_rejects_finish_ok_with_output_zip_sha256_mismatch() {
        let temp = TempDir::new().expect("temp");
        let mut ctx = LabRunContext::create(temp.path(), Path::new("input.zip")).expect("ctx");
        ctx.ensure_ledger().expect("ledger");
        let summary = ctx.summary_json(true, None, None).expect("summary");
        let diagnostics = ctx.diagnostics_json(None, None);
        let environment = ctx.environment_json(None).expect("environment");
        ctx.append_finalizing_record(true, None, summary, diagnostics, environment)
            .expect("finalizing");
        let out = temp.path().join("out.zip");
        fs::write(&out, b"changed after ledger receipt").expect("output file");
        let archive = commit_then_record(|| -> Result<_, LabLogError> {
            Ok(ArchiveResult {
                path: out,
                sha256: "not-the-real-sha256".to_string(),
            })
        })
        .expect("commit proof");
        ctx.append_terminal_receipt(true, None, None, Some(&archive))
            .expect("terminal receipt");

        let err = ctx
            .project_completed_run_from_ledger()
            .expect_err("sha mismatch must fail");

        assert!(err.message.contains("output_zip sha256 mismatch"));
    }

    #[test]
    fn completed_projection_requires_finalizing_record() {
        let temp = TempDir::new().expect("temp");
        let mut ctx = LabRunContext::create(temp.path(), Path::new("input.zip")).expect("ctx");
        ctx.ensure_ledger().expect("ledger");
        ctx.append_terminal_receipt(false, Some("terminal failure"), None, None)
            .expect("terminal receipt");

        let err = ctx
            .project_completed_run_from_ledger()
            .expect_err("missing finalizing must fail");

        assert!(err.message.contains("missing finalizing record"));
    }

    #[test]
    fn completed_projection_requires_terminal_receipt() {
        let temp = TempDir::new().expect("temp");
        let mut ctx = LabRunContext::create(temp.path(), Path::new("input.zip")).expect("ctx");
        ctx.ensure_ledger().expect("ledger");
        let summary = ctx.summary_json(true, None, None).expect("summary");
        let diagnostics = ctx.diagnostics_json(None, None);
        let environment = ctx.environment_json(None).expect("environment");
        ctx.append_finalizing_record(true, None, summary, diagnostics, environment)
            .expect("finalizing");

        let err = ctx
            .project_completed_run_from_ledger()
            .expect_err("missing terminal receipt must fail");

        assert!(err.message.contains("missing terminal receipt"));
    }

    #[test]
    fn zip_failure_after_success_does_not_record_finish_ok() {
        let temp = TempDir::new().expect("temp");
        let mut ctx = LabRunContext::create(temp.path(), Path::new("input.zip")).expect("ctx");
        let blocked_parent = temp.path().join("blocked-parent");
        fs::write(&blocked_parent, b"not a directory").expect("blocker");
        let out = blocked_parent.join("out.zip");

        let err = ctx
            .finish(&out, true, None, None)
            .expect_err("zip write failure");

        assert!(err.message.contains("terminal output failed"));
        assert!(temp.path().join("last-error.json").is_file());
        let ledger = LabLedger::read(ctx.ledger_path.as_ref().unwrap()).expect("ledger read");
        assert!(has_record_type(&ledger, "finalizing"));
        assert!(has_record_type(&ledger, "finish_error"));
        assert!(!has_record_type(&ledger, "finish_ok"));
        assert!(!has_event(&ledger, "output_zip_written"));
    }

    #[test]
    fn write_logs_failure_does_not_record_finish_ok() {
        let temp = TempDir::new().expect("temp");
        let mut ctx = LabRunContext::create(temp.path(), Path::new("input.zip")).expect("ctx");
        fs::remove_dir_all(&ctx.logs_dir).expect("remove logs dir");
        fs::write(&ctx.logs_dir, b"not a directory").expect("logs blocker");
        let out = temp.path().join("out.zip");

        let err = ctx
            .finish(&out, true, None, None)
            .expect_err("write_logs failure");

        assert!(err.message.contains("terminal output failed"));
        assert!(temp.path().join("last-error.json").is_file());
        let ledger = LabLedger::read(ctx.ledger_path.as_ref().unwrap()).expect("ledger read");
        assert!(has_record_type(&ledger, "finalizing"));
        assert!(has_record_type(&ledger, "finish_error"));
        assert!(!has_record_type(&ledger, "finish_ok"));
        assert!(!has_event(&ledger, "output_zip_written"));
    }

    #[test]
    fn path_inside_detects_run_dir_output() {
        let temp = TempDir::new().expect("temp");
        let ctx = LabRunContext::create(temp.path(), Path::new("input.zip")).expect("ctx");
        let inside = ctx.run_dir.join("result.zip");
        let outside = temp.path().join("result.zip");

        assert!(path_is_inside(&inside, &ctx.run_dir));
        assert!(!path_is_inside(&outside, &ctx.run_dir));
    }

    #[test]
    fn tier3_pause_checkpoint_includes_step_context() {
        let temp = TempDir::new().expect("temp");
        let mut ctx = LabRunContext::create(temp.path(), Path::new("input.zip")).expect("ctx");
        ctx.set_phase("page_guard_started");
        let operation = test_operation(Some("terminal"), None);
        ctx.set_step_context(7, &operation);
        let mut checkpoint = Tier3PauseCheckpoint {
            last_frame_index: 12,
            resident_bytes: 34,
            tier1_bytes: 10,
            tier2_bytes: 20,
            tier3_bytes: 30,
            active_segment_id: None,
            in_flight_flush_state: "idle".to_string(),
            current_step_index: None,
            current_step_id: None,
            current_operation_id: None,
            current_phase: None,
            expected_page: None,
            last_matched_page: None,
        };

        ctx.fill_pause_checkpoint(&mut checkpoint, Some("arknights/home"));
        let json = checkpoint.to_json();

        assert_eq!(json["current_step_index"], 7);
        assert_eq!(json["current_step_id"], "open_terminal");
        assert_eq!(json["current_operation_id"], "open_terminal");
        assert_eq!(json["current_phase"], "page_guard_started");
        assert_eq!(json["expected_page"], "terminal");
        assert_eq!(json["last_matched_page"], "arknights/home");
    }

    #[test]
    fn rejects_dangerous_zip_entry_without_writing_it() {
        let temp = TempDir::new().expect("temp");
        let zip = temp.path().join("input.zip");
        write_test_zip(
            &zip,
            &[
                ("control.json", br#"{}"#),
                ("resources/manifest.json", br#"{}"#),
                ("resources/tool.exe", b"danger"),
            ],
        );

        let err = match validate_lab_package_zip(&zip) {
            Ok(_) => panic!("dangerous entry accepted"),
            Err(err) => err,
        };

        assert_eq!(err.code, "package_invalid");
        assert!(
            !temp
                .path()
                .join("input")
                .join("resources")
                .join("tool.exe")
            .exists()
        );
    }
