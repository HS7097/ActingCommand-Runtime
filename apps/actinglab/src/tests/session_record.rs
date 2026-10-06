    #[test]
    fn session_record_start_status_and_stop_write_context() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
                "--holder",
                "scheduler",
                "--lease-id",
                "lease-1",
            ],
            true,
        );
        let status = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "status",
                "--state-dir",
                state_dir.to_str().unwrap(),
            ],
            true,
        );
        let stop = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "stop",
                "--state-dir",
                state_dir.to_str().unwrap(),
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        let start_data = start.envelope.data.as_ref().unwrap();
        assert_eq!(
            start_data.get("auto_recording").and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            start_data.pointer("/record/status").and_then(Value::as_str),
            Some("active")
        );
        assert_eq!(
            start_data
                .pointer("/record/task_id")
                .and_then(Value::as_str),
            Some("daily-check")
        );
        assert_eq!(
            start_data
                .pointer("/record/instance")
                .and_then(Value::as_str),
            Some("ak")
        );
        assert_eq!(
            start_data.pointer("/record/holder").and_then(Value::as_str),
            Some("scheduler")
        );
        assert_eq!(
            start_data
                .pointer("/record/lease_id")
                .and_then(Value::as_str),
            Some("lease-1")
        );
        assert!(
            start_data
                .pointer("/record/steps")
                .and_then(Value::as_array)
                .is_some_and(Vec::is_empty)
        );

        assert_eq!(status.exit_code(), 0);
        assert_eq!(
            status
                .envelope
                .data
                .as_ref()
                .unwrap()
                .get("status")
                .and_then(Value::as_str),
            Some("available")
        );

        assert_eq!(stop.exit_code(), 0);
        assert_eq!(
            stop.envelope
                .data
                .as_ref()
                .unwrap()
                .pointer("/record/status")
                .and_then(Value::as_str),
            Some("stopped")
        );
    }

    #[test]
    fn top_level_record_alias_uses_session_record_context() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let status = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "status",
                "--state-dir",
                state_dir.to_str().unwrap(),
            ],
            true,
        );
        let stop = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "record",
                "stop",
                "--state-dir",
                state_dir.to_str().unwrap(),
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(
            start
                .envelope
                .data
                .as_ref()
                .unwrap()
                .pointer("/record/task_id")
                .and_then(Value::as_str),
            Some("daily-check")
        );
        assert_eq!(status.exit_code(), 0);
        assert_eq!(
            status
                .envelope
                .data
                .as_ref()
                .unwrap()
                .pointer("/record/status")
                .and_then(Value::as_str),
            Some("active")
        );
        assert_eq!(stop.exit_code(), 0);
        assert_eq!(stop.envelope.command.as_str(), "record");
        assert_eq!(
            stop.envelope
                .data
                .as_ref()
                .unwrap()
                .pointer("/record/status")
                .and_then(Value::as_str),
            Some("stopped")
        );
    }

    #[test]
    fn top_level_record_build_task_reports_retirement() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        set_config_env(&config);

        let build = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "record",
                "build-task",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--out",
                temp.path().join("draft").to_str().unwrap(),
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(build.exit_code(), 2);
        assert_eq!(
            build.envelope.error.as_ref().unwrap().code,
            "resource_production_retired"
        );
    }

    #[test]
    fn stream_command_reports_bounded_dry_run_contract() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        set_config_env(&config);

        let stream = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "stream",
                "--dry-run",
                "--max-frames",
                "2",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(stream.exit_code(), 0);
        let data = stream.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.get("mode").and_then(Value::as_str),
            Some("bounded_stream")
        );
        assert_eq!(
            data.pointer("/input_relay/status").and_then(Value::as_str),
            Some("disabled")
        );
        assert_eq!(
            data.pointer("/capture/dry_run").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/contract/schema_version")
                .and_then(Value::as_str),
            Some("session.stream.v0.1")
        );
        assert_eq!(
            data.pointer("/contract/status").and_then(Value::as_str),
            Some("available")
        );
        assert_eq!(
            data.pointer("/contract/event_schema_version")
                .and_then(Value::as_str),
            Some("session.stream.event.v0.1")
        );
        assert_eq!(
            data.pointer("/contract/safety/session_layer_only_throat")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/contract/input_relay/requested")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            data.pointer("/contract/input_relay/execution_model")
                .and_then(Value::as_str),
            Some("planned_only")
        );
        assert_eq!(
            data.pointer("/trusted_channel/long_lived_stream_implemented")
                .and_then(Value::as_bool),
            Some(false)
        );
        let stream_id = data.get("stream_id").and_then(Value::as_str).unwrap();
        assert!(stream_id.starts_with("stream-"));
        let events = data.get("events").and_then(Value::as_array).unwrap();
        assert_eq!(events.len(), 4);
        assert_eq!(
            events[0].get("schema_version").and_then(Value::as_str),
            Some("session.stream.event.v0.1")
        );
        assert_eq!(
            events[0].get("stream_id").and_then(Value::as_str),
            Some(stream_id)
        );
        assert_eq!(
            events[0].get("event_index").and_then(Value::as_u64),
            Some(0)
        );
        assert_eq!(
            events[0].get("type").and_then(Value::as_str),
            Some("stream.started")
        );
        assert_eq!(
            events[1].get("stream_id").and_then(Value::as_str),
            Some(stream_id)
        );
        assert_eq!(
            events[1].get("event_index").and_then(Value::as_u64),
            Some(1)
        );
        assert_eq!(
            events[1].get("type").and_then(Value::as_str),
            Some("stream.frame_sampled")
        );
        assert_eq!(
            events[3].get("stream_id").and_then(Value::as_str),
            Some(stream_id)
        );
        assert_eq!(
            events[3].get("event_index").and_then(Value::as_u64),
            Some(3)
        );
        assert_eq!(
            events[3].get("type").and_then(Value::as_str),
            Some("stream.completed")
        );
        assert_eq!(
            data.get("frames").and_then(Value::as_array).unwrap().len(),
            2
        );
    }

    #[test]
    fn session_record_active_start_requires_force() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        set_config_env(&config);

        let first = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let conflict = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check-2",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(first.exit_code(), 0);
        assert_eq!(conflict.exit_code(), 3);
        assert_eq!(
            conflict.envelope.error.as_ref().unwrap().code,
            "record_session_active"
        );
    }

    #[test]
    fn session_record_build_task_refuses_without_record() {
        let temp = TempDir::new().unwrap();
        let result = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "build-task",
                "--state-dir",
                temp.path().to_str().unwrap(),
            ],
            true,
        );

        assert_eq!(result.exit_code(), 2);
        assert_eq!(
            result.envelope.error.as_ref().unwrap().code,
            "resource_production_retired"
        );
    }

    #[test]
    fn session_record_step_anchor_records_region_schema() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "home-anchor",
                "--id",
                "page/home",
                "--region",
                "10,20,30,40",
                "--color-check",
                "--threshold",
                "0.96",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        let data = step.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.get("status").and_then(Value::as_str),
            Some("step_recorded")
        );
        assert_eq!(data.get("step_count").and_then(Value::as_u64), Some(1));
        assert_eq!(
            data.pointer("/step/step_id").and_then(Value::as_str),
            Some("home-anchor")
        );
        assert_eq!(
            data.pointer("/step/kind").and_then(Value::as_str),
            Some("anchor")
        );
        assert_eq!(
            data.pointer("/step/id").and_then(Value::as_str),
            Some("page/home")
        );
        assert_eq!(
            data.pointer("/step/region/mode").and_then(Value::as_str),
            Some("rect")
        );
        assert_eq!(
            data.pointer("/step/region/rect/x").and_then(Value::as_i64),
            Some(10)
        );
        assert_eq!(
            data.pointer("/step/color_check").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/step/threshold").and_then(Value::as_f64),
            Some(0.96)
        );
        assert_eq!(
            data.pointer("/step/evaluation/status")
                .and_then(Value::as_str),
            Some("deferred")
        );
        assert_eq!(
            data.pointer("/step/evaluation/reason")
                .and_then(Value::as_str),
            Some("frame_not_provided")
        );
        assert!(data.pointer("/step/evaluation/backtest").is_none());
        assert_eq!(
            data.pointer("/record/steps/0/step_id")
                .and_then(Value::as_str),
            Some("home-anchor")
        );
    }

    #[test]
    fn session_record_step_color_probe_records_deferred_schema() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "color-probe",
                "--step-id",
                "home-color",
                "--id",
                "color/home-status",
                "--region",
                "10,20,30,40",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        let data = step.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/step/kind").and_then(Value::as_str),
            Some("color_probe")
        );
        assert_eq!(
            data.pointer("/step/id").and_then(Value::as_str),
            Some("color/home-status")
        );
        assert_eq!(
            data.pointer("/step/region/mode").and_then(Value::as_str),
            Some("rect")
        );
        assert!(data.pointer("/step/expected").is_none());
        assert_eq!(
            data.pointer("/step/evaluation/status")
                .and_then(Value::as_str),
            Some("deferred")
        );
        assert_eq!(
            data.pointer("/step/evaluation/reason")
                .and_then(Value::as_str),
            Some("frame_not_provided")
        );
    }

    #[test]
    fn session_record_step_color_probe_samples_frame() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "color-probe",
                "--step-id",
                "home-color",
                "--id",
                "color/home-status",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(
            step.exit_code(),
            0,
            "{}",
            serde_json::to_string_pretty(&step.envelope).unwrap()
        );
        let data = step.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/step/kind").and_then(Value::as_str),
            Some("color_probe")
        );
        assert_eq!(
            data.pointer("/step/expected/0").and_then(Value::as_u64),
            Some(3)
        );
        assert_eq!(
            data.pointer("/step/expected/1").and_then(Value::as_u64),
            Some(5)
        );
        assert_eq!(
            data.pointer("/step/expected/2").and_then(Value::as_u64),
            Some(128)
        );
        assert_eq!(
            data.pointer("/step/frame_provenance/source")
                .and_then(Value::as_str),
            Some("local_png")
        );
        assert_eq!(
            data.pointer("/step/evaluation/status")
                .and_then(Value::as_str),
            Some("passed")
        );
        assert_eq!(
            data.pointer("/step/evaluation/reason")
                .and_then(Value::as_str),
            Some("color_probe_sampled")
        );
    }

    #[test]
    fn session_record_step_verify_template_records_deferred_schema() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "verify-template",
                "--step-id",
                "mail-ready",
                "--id",
                "template/mail-ready",
                "--region",
                "10,20,30,40",
                "--threshold",
                "0.97",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        let data = step.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/step/kind").and_then(Value::as_str),
            Some("verify_template")
        );
        assert_eq!(
            data.pointer("/step/id").and_then(Value::as_str),
            Some("template/mail-ready")
        );
        assert_eq!(
            data.pointer("/step/region/mode").and_then(Value::as_str),
            Some("rect")
        );
        assert_eq!(
            data.pointer("/step/threshold").and_then(Value::as_f64),
            Some(0.97)
        );
        assert!(data.pointer("/step/artifact").is_none());
        assert_eq!(
            data.pointer("/step/evaluation/status")
                .and_then(Value::as_str),
            Some("deferred")
        );
        assert_eq!(
            data.pointer("/step/evaluation/reason")
                .and_then(Value::as_str),
            Some("frame_not_provided")
        );
    }

    #[test]
    fn session_record_step_verify_template_materializes_frame_crop() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "verify-template",
                "--step-id",
                "mail-ready",
                "--id",
                "template/mail-ready",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(
            step.exit_code(),
            0,
            "{}",
            serde_json::to_string_pretty(&step.envelope).unwrap()
        );
        let data = step.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/step/kind").and_then(Value::as_str),
            Some("verify_template")
        );
        assert_eq!(
            data.pointer("/step/artifact/width").and_then(Value::as_u64),
            Some(4)
        );
        assert_eq!(
            data.pointer("/step/artifact/height")
                .and_then(Value::as_u64),
            Some(5)
        );
        assert_eq!(
            data.pointer("/step/evaluation/status")
                .and_then(Value::as_str),
            Some("passed")
        );
        assert_eq!(
            data.pointer("/step/evaluation/backtest/passed")
                .and_then(Value::as_bool),
            Some(true)
        );
        let artifact_path = data
            .pointer("/step/artifact/path")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .expect("artifact path");
        assert!(artifact_path.exists());
    }

    #[test]
    fn session_record_amend_recomputes_frame_backed_color_probe() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "color-probe",
                "--step-id",
                "home-color",
                "--id",
                "color/home-status",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
            ],
            true,
        );
        let amend = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "amend",
                "home-color",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--region",
                "4,1,2,3",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        assert_eq!(
            amend.exit_code(),
            0,
            "{}",
            serde_json::to_string_pretty(&amend.envelope).unwrap()
        );
        let data = amend.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/step/kind").and_then(Value::as_str),
            Some("color_probe")
        );
        assert_eq!(
            data.pointer("/step/region/rect/x").and_then(Value::as_i64),
            Some(4)
        );
        assert_eq!(
            data.pointer("/step/region/rect/y").and_then(Value::as_i64),
            Some(1)
        );
        assert_eq!(
            data.pointer("/step/expected/0").and_then(Value::as_u64),
            Some(4)
        );
        assert_eq!(
            data.pointer("/step/expected/1").and_then(Value::as_u64),
            Some(2)
        );
        assert_eq!(
            data.pointer("/step/expected/2").and_then(Value::as_u64),
            Some(128)
        );
        assert_eq!(
            data.pointer("/step/evaluation/status")
                .and_then(Value::as_str),
            Some("passed")
        );
        assert_eq!(
            data.pointer("/step/evaluation/reason")
                .and_then(Value::as_str),
            Some("color_probe_sampled")
        );
    }

    #[test]
    fn session_record_amend_deferred_color_probe_does_not_fake_expected_color() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "color-probe",
                "--step-id",
                "home-color",
                "--id",
                "color/home-status",
                "--region",
                "2,3,4,5",
            ],
            true,
        );
        let amend = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "amend",
                "home-color",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--region",
                "4,1,2,3",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        assert_eq!(amend.exit_code(), 0);
        let data = amend.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/step/kind").and_then(Value::as_str),
            Some("color_probe")
        );
        assert!(data.pointer("/step/expected").is_none());
        assert_eq!(
            data.pointer("/step/evaluation/status")
                .and_then(Value::as_str),
            Some("deferred")
        );
        assert_eq!(
            data.pointer("/step/evaluation/reason")
                .and_then(Value::as_str),
            Some("amended_without_frame_provenance")
        );
    }

    #[test]
    fn session_record_amend_rebacktests_frame_backed_verify_template() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "verify-template",
                "--step-id",
                "mail-ready",
                "--id",
                "template/mail-ready",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
            ],
            true,
        );
        let amend = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "amend",
                "mail-ready",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--region",
                "1,2,3,4",
                "--threshold",
                "0.90",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        assert_eq!(
            amend.exit_code(),
            0,
            "{}",
            serde_json::to_string_pretty(&amend.envelope).unwrap()
        );
        let data = amend.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/step/kind").and_then(Value::as_str),
            Some("verify_template")
        );
        assert_eq!(
            data.pointer("/step/threshold").and_then(Value::as_f64),
            Some(0.90)
        );
        assert_eq!(
            data.pointer("/step/artifact/width").and_then(Value::as_u64),
            Some(3)
        );
        assert_eq!(
            data.pointer("/step/artifact/height")
                .and_then(Value::as_u64),
            Some(4)
        );
        assert_eq!(
            data.pointer("/step/evaluation/status")
                .and_then(Value::as_str),
            Some("passed")
        );
        assert_eq!(
            data.pointer("/step/evaluation/backtest/x")
                .and_then(Value::as_i64),
            Some(1)
        );
        assert_eq!(
            data.pointer("/step/evaluation/backtest/y")
                .and_then(Value::as_i64),
            Some(2)
        );
        assert!(
            data.pointer("/step/evaluation/backtest/threshold")
                .and_then(Value::as_f64)
                .is_some_and(|threshold| (threshold - 0.90).abs() < 0.00001)
        );
        let artifact_path = data
            .pointer("/step/artifact/path")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .expect("artifact path");
        assert!(artifact_path.is_file());
    }

    #[test]
    fn session_record_step_anchor_materializes_frame_crop() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let artifact_dir = state_dir.join("artifacts");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "home-anchor",
                "--id",
                "page/home",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
                "--artifact-dir",
                artifact_dir.to_str().unwrap(),
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        let data = step.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/step/frame_provenance/source")
                .and_then(Value::as_str),
            Some("local_png")
        );
        assert_eq!(
            data.pointer("/step/frame_provenance/width")
                .and_then(Value::as_u64),
            Some(12)
        );
        assert_eq!(
            data.pointer("/step/artifact/kind").and_then(Value::as_str),
            Some("template_crop")
        );
        assert_eq!(
            data.pointer("/step/artifact/width").and_then(Value::as_u64),
            Some(4)
        );
        assert_eq!(
            data.pointer("/step/artifact/height")
                .and_then(Value::as_u64),
            Some(5)
        );
        assert_eq!(
            data.pointer("/step/evaluation/status")
                .and_then(Value::as_str),
            Some("passed")
        );
        assert_eq!(
            data.pointer("/step/evaluation/reason")
                .and_then(Value::as_str),
            Some("self_backtest_passed")
        );
        assert_eq!(
            data.pointer("/step/evaluation/backtest/source")
                .and_then(Value::as_str),
            Some("local_png_self_test")
        );
        assert_eq!(
            data.pointer("/step/evaluation/backtest/metric")
                .and_then(Value::as_str),
            Some("ccorr_normed")
        );
        assert_eq!(
            data.pointer("/step/evaluation/backtest/x")
                .and_then(Value::as_i64),
            Some(2)
        );
        assert_eq!(
            data.pointer("/step/evaluation/backtest/y")
                .and_then(Value::as_i64),
            Some(3)
        );
        assert_eq!(
            data.pointer("/step/evaluation/backtest/passed")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert!(
            data.pointer("/step/evaluation/backtest/score")
                .and_then(Value::as_f64)
                .is_some_and(|score| score >= 0.99)
        );
        assert!(
            data.pointer("/step/evaluation/backtest/threshold")
                .and_then(Value::as_f64)
                .is_some_and(|threshold| (threshold - 0.95).abs() < 0.00001)
        );
        assert!(data.pointer("/step/evaluation/contrast_backtest").is_none());
        let artifact_path = data
            .pointer("/step/artifact/path")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .expect("artifact path");
        assert!(artifact_path.exists());
        let artifact_png = fs::read(&artifact_path).unwrap();
        let artifact_frame = Frame::from_png(artifact_png, CaptureBackendName::AdbScreencap)
            .expect("artifact frame");
        assert_eq!(artifact_frame.width, 4);
        assert_eq!(artifact_frame.height, 5);
        assert_eq!(
            data.pointer("/record/steps/0/artifact/path")
                .and_then(Value::as_str),
            Some(artifact_path.to_str().unwrap())
        );
    }

    #[test]
    fn session_record_step_rejects_artifact_dir_outside_state_dir() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let escaped_artifact_dir = temp.path().join("outside-artifacts");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "home-anchor",
                "--id",
                "page/home",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
                "--artifact-dir",
                escaped_artifact_dir.to_str().unwrap(),
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 3);
        assert_eq!(step.envelope.error.as_ref().unwrap().code, "path_escape");
        assert!(!escaped_artifact_dir.exists());
    }

    #[test]
    fn ensure_path_within_rejects_directory_alias_escape() {
        let temp = TempDir::new().unwrap();
        let base = temp.path().join("state");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&base).unwrap();
        fs::create_dir_all(&outside).unwrap();

        let absolute_escape =
            ensure_path_within(&base, &outside.join("artifact.png"), "test", &["record"])
                .unwrap_err();
        assert_eq!(absolute_escape.code, "path_escape");

        let link = base.join("linked-outside");
        if create_test_dir_alias(&link, &outside) {
            let linked_escape = ensure_path_within(
                &base,
                Path::new("linked-outside/artifact.png"),
                "test",
                &["record"],
            )
            .unwrap_err();
            assert_eq!(linked_escape.code, "path_escape");
            let _ = fs::remove_dir(&link);
        }
    }

    #[test]
    fn session_record_artifact_source_must_stay_within_state_dir() {
        let temp = TempDir::new().unwrap();
        let state_dir = temp.path().join("session");
        fs::create_dir_all(&state_dir).unwrap();
        let escaped_artifact = temp.path().join("outside.png");
        fs::write(&escaped_artifact, test_record_frame_png(4, 5)).unwrap();
        let err = ensure_path_within(&state_dir, &escaped_artifact, "record artifact", &["record"])
            .expect_err("artifact outside the state directory must fail closed");
        assert_eq!(err.code, "path_escape");
        assert_eq!(err.exit_code(), 3);
    }
    #[test]
    fn session_record_anchor_materializes_current_capture_source_frame_metadata() {
        let temp = TempDir::new().unwrap();
        let png = test_record_frame_png(12, 10);
        let frame =
            Frame::from_png(png.clone(), CaptureBackendName::NemuIpc).expect("source frame");
        let source_path = temp.path().join("source-frame-home.png");
        fs::write(&source_path, &png).unwrap();
        let empty_args = Vec::<String>::new();
        let flags = FlagArgs::parse(&empty_args).unwrap();
        let source_frame = SessionRecordSourceFrame {
            frame,
            png,
            source: "current_capture".to_string(),
            path: source_path.clone(),
            recorded_at_unix_ms: current_unix_ms(),
            capture_backend: Some("nemu_ipc".to_string()),
            freshness: Some(json!({
                "required": true,
                "fresh": true,
                "backend": "nemu_ipc"
            })),
            capture_attempts: vec![json!({
                "backend": "nemu_ipc",
                "ok": true,
                "message": "primed"
            })],
        };
        let materialized = materialize_anchor_artifact_from_source(
            source_frame,
            SessionRecordAnchorRegionResolution {
                rect: SessionRecordRect {
                    x: 2,
                    y: 3,
                    width: 4,
                    height: 5,
                },
                auto_region: None,
            },
            &temp.path().join("artifacts"),
            "home-anchor",
            "page/home",
            Some(0.95),
            &flags,
        )
        .expect("materialized current capture source frame");

        assert_eq!(materialized.frame_provenance.source, "current_capture");
        assert_eq!(
            materialized.frame_provenance.path,
            source_path.display().to_string()
        );
        assert_eq!(
            materialized.frame_provenance.capture_backend.as_deref(),
            Some("nemu_ipc")
        );
        assert_eq!(
            materialized
                .frame_provenance
                .freshness
                .as_ref()
                .and_then(|value| value.get("fresh"))
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(materialized.frame_provenance.capture_attempts.len(), 1);
        assert_eq!(materialized.artifact.width, 4);
        assert_eq!(materialized.artifact.height, 5);
        assert_eq!(materialized.evaluation.status, "passed");
        assert!(PathBuf::from(&materialized.artifact.path).is_file());
    }

    #[test]
    fn session_record_step_anchor_rejects_frame_and_capture_together() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--id",
                "page/home",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
                "--capture",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 2);
        assert_eq!(
            step.envelope.error.as_ref().unwrap().code,
            "validation_failed"
        );
        assert!(
            step.envelope
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("not both")
        );
    }

    #[test]
    fn session_record_step_anchor_contrast_frame_passes_when_distinct() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let contrast_path = temp.path().join("contrast.png");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        fs::write(&contrast_path, test_contrast_record_frame_png(12, 10)).unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "home-anchor",
                "--id",
                "page/home",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
                "--contrast-frame",
                contrast_path.to_str().unwrap(),
                "--threshold",
                "0.999",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(
            step.exit_code(),
            0,
            "{}",
            serde_json::to_string_pretty(&step.envelope).unwrap()
        );
        let data = step.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/step/evaluation/status")
                .and_then(Value::as_str),
            Some("passed")
        );
        assert_eq!(
            data.pointer("/step/evaluation/reason")
                .and_then(Value::as_str),
            Some("self_and_contrast_backtest_passed")
        );
        assert_eq!(
            data.pointer("/step/evaluation/contrast_backtest/source")
                .and_then(Value::as_str),
            Some("local_png_contrast")
        );
        assert_eq!(
            data.pointer("/step/evaluation/contrast_backtest/passed")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert!(
            data.pointer("/step/evaluation/contrast_backtest/score")
                .and_then(Value::as_f64)
                .is_some_and(|score| score < 0.999)
        );
    }

    #[test]
    fn session_record_step_anchor_contrast_frame_fails_when_matching() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "home-anchor",
                "--id",
                "page/home",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
                "--negative-frame",
                frame_path.to_str().unwrap(),
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        let data = step.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/step/evaluation/status")
                .and_then(Value::as_str),
            Some("failed")
        );
        assert_eq!(
            data.pointer("/step/evaluation/reason")
                .and_then(Value::as_str),
            Some("contrast_backtest_matched")
        );
        assert_eq!(
            data.pointer("/step/evaluation/backtest/passed")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/step/evaluation/contrast_backtest/passed")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert!(
            data.pointer("/step/evaluation/contrast_backtest/score")
                .and_then(Value::as_f64)
                .is_some_and(|score| score >= 0.95)
        );
    }

    #[test]
    fn session_record_step_anchor_auto_region_materializes_frame_crop() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let artifact_dir = state_dir.join("artifacts");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--id",
                "page/home",
                "--region",
                "auto",
                "--frame",
                frame_path.to_str().unwrap(),
                "--artifact-dir",
                artifact_dir.to_str().unwrap(),
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        let data = step.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/step/region/mode").and_then(Value::as_str),
            Some("rect")
        );
        assert!(
            data.pointer("/step/region/rect/width")
                .and_then(Value::as_i64)
                .is_some_and(|width| width > 0 && width <= 12)
        );
        assert!(
            data.pointer("/step/region/rect/height")
                .and_then(Value::as_i64)
                .is_some_and(|height| height > 0 && height <= 10)
        );
        assert_eq!(
            data.pointer("/step/artifact/kind").and_then(Value::as_str),
            Some("template_crop")
        );
        assert_eq!(
            data.pointer("/step/evaluation/status")
                .and_then(Value::as_str),
            Some("passed")
        );
        let artifact_path = data
            .pointer("/step/artifact/path")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .expect("artifact path");
        assert!(artifact_path.exists());
    }

    #[test]
    fn session_record_step_anchor_auto_region_prefers_contrast_rejected_candidate() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let contrast_path = temp.path().join("contrast.png");
        fs::write(
            &frame_path,
            test_auto_region_discrimination_frame_png(false),
        )
        .unwrap();
        fs::write(
            &contrast_path,
            test_auto_region_discrimination_frame_png(true),
        )
        .unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--id",
                "page/home",
                "--region",
                "auto",
                "--frame",
                frame_path.to_str().unwrap(),
                "--contrast-frame",
                contrast_path.to_str().unwrap(),
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        let data = step.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/step/evaluation/auto_region/selected_reason")
                .and_then(Value::as_str),
            Some("contrast_rejected_highest_variance")
        );
        assert_eq!(
            data.pointer("/step/evaluation/auto_region/selected/x")
                .and_then(Value::as_i64),
            Some(4)
        );
        assert_eq!(
            data.pointer("/step/evaluation/auto_region/selected/y")
                .and_then(Value::as_i64),
            Some(3)
        );
        assert_eq!(
            data.pointer("/step/region/rect/x").and_then(Value::as_i64),
            Some(4)
        );
        assert_eq!(
            data.pointer("/step/region/rect/y").and_then(Value::as_i64),
            Some(3)
        );
        let candidates = data
            .pointer("/step/evaluation/auto_region/candidates")
            .and_then(Value::as_array)
            .expect("auto-region candidates");
        assert_eq!(candidates.len(), 9);
        assert_eq!(
            candidates
                .iter()
                .filter(
                    |candidate| candidate.get("selected").and_then(Value::as_bool) == Some(true)
                )
                .count(),
            1
        );
        assert!(candidates.iter().any(|candidate| {
            candidate.get("contrast_passed").and_then(Value::as_bool) == Some(true)
        }));
        assert!(candidates.iter().any(|candidate| {
            candidate.get("contrast_passed").and_then(Value::as_bool) == Some(false)
        }));
        assert_eq!(
            data.pointer("/step/evaluation/contrast_backtest/passed")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/step/evaluation/status")
                .and_then(Value::as_str),
            Some("passed")
        );
    }

    #[test]
    fn session_record_candidates_lists_auto_region_report() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let contrast_path = temp.path().join("contrast.png");
        fs::write(
            &frame_path,
            test_auto_region_discrimination_frame_png(false),
        )
        .unwrap();
        fs::write(
            &contrast_path,
            test_auto_region_discrimination_frame_png(true),
        )
        .unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "home-anchor",
                "--id",
                "page/home",
                "--region",
                "auto",
                "--frame",
                frame_path.to_str().unwrap(),
                "--contrast-frame",
                contrast_path.to_str().unwrap(),
            ],
            true,
        );
        let candidates = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "candidates",
                "home-anchor",
                "--state-dir",
                state_dir.to_str().unwrap(),
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        assert_eq!(candidates.exit_code(), 0);
        let data = candidates.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/status").and_then(Value::as_str),
            Some("candidates_listed")
        );
        assert_eq!(
            data.pointer("/step_id").and_then(Value::as_str),
            Some("home-anchor")
        );
        assert_eq!(
            data.pointer("/candidate_count").and_then(Value::as_u64),
            Some(9)
        );
        assert_eq!(
            data.pointer("/selected_index").and_then(Value::as_u64),
            Some(4)
        );
        assert_eq!(
            data.pointer("/auto_region/selected_reason")
                .and_then(Value::as_str),
            Some("contrast_rejected_highest_variance")
        );
        assert_eq!(
            data.pointer("/auto_region/selected/x")
                .and_then(Value::as_i64),
            Some(4)
        );
        assert_eq!(
            data.pointer("/auto_region/selected/y")
                .and_then(Value::as_i64),
            Some(3)
        );
    }

    #[test]
    fn session_record_candidates_lists_color_probe_auto_region_report() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        fs::write(
            &frame_path,
            test_auto_region_discrimination_frame_png(false),
        )
        .unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "color-probe",
                "--step-id",
                "home-color",
                "--id",
                "color/home-status",
                "--region",
                "auto",
                "--frame",
                frame_path.to_str().unwrap(),
            ],
            true,
        );
        let candidates = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "candidates",
                "home-color",
                "--state-dir",
                state_dir.to_str().unwrap(),
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        assert_eq!(
            candidates.exit_code(),
            0,
            "{}",
            serde_json::to_string_pretty(&candidates.envelope).unwrap()
        );
        let data = candidates.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/resource_kind").and_then(Value::as_str),
            Some("color_probe")
        );
        assert_eq!(
            data.pointer("/resource_id").and_then(Value::as_str),
            Some("color/home-status")
        );
        assert_eq!(
            data.pointer("/anchor_id").and_then(Value::as_str),
            Some("color/home-status")
        );
        assert!(
            data.pointer("/candidate_count")
                .and_then(Value::as_u64)
                .is_some_and(|count| count > 0)
        );
    }

    #[test]
    fn session_record_candidates_requires_auto_region_report() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "home-anchor",
                "--id",
                "page/home",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
            ],
            true,
        );
        let candidates = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "candidates",
                "home-anchor",
                "--state-dir",
                state_dir.to_str().unwrap(),
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        assert_eq!(candidates.exit_code(), 2);
        assert_eq!(
            candidates.envelope.error.as_ref().unwrap().code,
            "validation_failed"
        );
        assert!(
            candidates
                .envelope
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("auto-region candidate report")
        );
    }

    #[test]
    fn session_record_step_anchor_auto_without_frame_stays_deferred() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--id",
                "page/home",
                "--region",
                "auto",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        let data = step.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/step/region/mode").and_then(Value::as_str),
            Some("auto")
        );
        assert!(data.pointer("/step/artifact").is_none());
        assert_eq!(
            data.pointer("/step/evaluation/status")
                .and_then(Value::as_str),
            Some("deferred")
        );
        assert_eq!(
            data.pointer("/step/evaluation/reason")
                .and_then(Value::as_str),
            Some("frame_not_provided")
        );
    }

    #[test]
    fn session_record_step_anchor_rejects_out_of_bounds_frame_crop() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--id",
                "page/home",
                "--region",
                "10,8,4,4",
                "--frame",
                frame_path.to_str().unwrap(),
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 2);
        assert_eq!(
            step.envelope.error.as_ref().unwrap().code,
            "validation_failed"
        );
        assert!(
            step.envelope
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("exceeds frame")
        );
    }

    #[test]
    fn session_record_step_operation_records_coord_click() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "operation",
                "--from",
                "page/home",
                "--to",
                "page/mail",
                "--click",
                "100,200",
                "--destructive",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        let data = step.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/step/kind").and_then(Value::as_str),
            Some("operation")
        );
        assert_eq!(
            data.pointer("/step/from").and_then(Value::as_str),
            Some("page/home")
        );
        assert_eq!(
            data.pointer("/step/to").and_then(Value::as_str),
            Some("page/mail")
        );
        assert_eq!(
            data.pointer("/step/click/type").and_then(Value::as_str),
            Some("coord")
        );
        assert_eq!(
            data.pointer("/step/click/x").and_then(Value::as_i64),
            Some(100)
        );
        assert_eq!(
            data.pointer("/step/destructive").and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn session_record_build_task_rejects_deferred_color_probe() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let out = temp.path().join("draft");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let home_anchor = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "home-anchor",
                "--id",
                "page/home",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
            ],
            true,
        );
        let mail_anchor = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "mail-anchor",
                "--id",
                "page/mail",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
            ],
            true,
        );
        let color_probe = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "color-probe",
                "--step-id",
                "home-color",
                "--id",
                "color/home-status",
                "--region",
                "2,3,4,5",
            ],
            true,
        );
        let operation = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "operation",
                "--step-id",
                "home-to-mail",
                "--from",
                "page/home",
                "--to",
                "page/mail",
                "--click",
                "5,6",
            ],
            true,
        );
        let build = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "build-task",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--out",
                out.to_str().unwrap(),
                "--game",
                "arknights",
                "--server",
                "cn",
                "--locale",
                "zh-CN",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(home_anchor.exit_code(), 0);
        assert_eq!(mail_anchor.exit_code(), 0);
        assert_eq!(color_probe.exit_code(), 0);
        assert_eq!(operation.exit_code(), 0);
        assert_eq!(build.exit_code(), 2);
        assert_eq!(build.envelope.error.as_ref().unwrap().code, "resource_production_retired");
        assert!(!out.exists());
    }
    #[test]
    fn session_record_build_task_rejects_deferred_verify_template() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let out = temp.path().join("draft");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let home_anchor = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "home-anchor",
                "--id",
                "page/home",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
            ],
            true,
        );
        let mail_anchor = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "mail-anchor",
                "--id",
                "page/mail",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
            ],
            true,
        );
        let verify_template = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "verify-template",
                "--step-id",
                "mail-ready",
                "--id",
                "template/mail-ready",
                "--region",
                "2,3,4,5",
            ],
            true,
        );
        let operation = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "operation",
                "--step-id",
                "home-to-mail",
                "--from",
                "page/home",
                "--to",
                "page/mail",
                "--click",
                "5,6",
            ],
            true,
        );
        let build = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "build-task",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--out",
                out.to_str().unwrap(),
                "--game",
                "arknights",
                "--server",
                "cn",
                "--locale",
                "zh-CN",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(home_anchor.exit_code(), 0);
        assert_eq!(mail_anchor.exit_code(), 0);
        assert_eq!(verify_template.exit_code(), 0);
        assert_eq!(operation.exit_code(), 0);
        assert_eq!(build.exit_code(), 2);
        assert_eq!(build.envelope.error.as_ref().unwrap().code, "resource_production_retired");
        assert!(!out.exists());
    }
    #[test]
    fn session_record_promote_refuses_before_runtime_or_target_mutation() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let runtime_root = temp.path().join("missing-runtime");
        let _runtime_env = use_runtime_state_root(&runtime_root);
        let repo = temp.path().join("resource-repo");
        let ours = repo.join("ours");
        fs::create_dir_all(ours.join("operations")).unwrap();
        fs::create_dir_all(ours.join("recognition")).unwrap();
        prepare_recorded_steps(&config, &state_dir, &frame_path);

        let promote = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "promote",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--repo",
                repo.to_str().unwrap(),
                "--game",
                "arknights",
                "--server",
                "cn",
                "--locale",
                "zh-CN",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(promote.exit_code(), 2);
        assert_eq!(
            promote.envelope.error.as_ref().unwrap().code,
            "resource_production_retired"
        );
        assert!(!ours.join("operations/daily-check").exists());
        assert!(!ours.join("operations/resources.json").exists());
        assert!(!ours.join("recognition/arknights.cn.pack.json").exists());
    }

    #[test]
    fn session_record_promote_refusal_preserves_canonical_tree() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let runtime_root = temp.path().join("runtime");
        let _runtime_env = use_runtime_state_root(&runtime_root);
        let host = start_authoring_runtime(&runtime_root);
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let repo = temp.path().join("resource-repo");
        let ours = repo.join("ours");
        let existing_task = ours.join("operations/daily-check");
        let broken_task = ours.join("operations/broken");
        fs::create_dir_all(&existing_task).unwrap();
        fs::create_dir_all(&broken_task).unwrap();
        fs::create_dir_all(ours.join("recognition")).unwrap();
        fs::write(existing_task.join("sentinel.txt"), "canonical-before").unwrap();
        fs::write(broken_task.join("task.json"), "{not-json").unwrap();
        fs::write(
            ours.join("operations/resources.json"),
            r#"{"schema_version":"1.0","resources":[],"resource_count":0}"#,
        )
        .unwrap();
        prepare_recorded_steps(&config, &state_dir, &frame_path);

        let promote = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "promote",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--repo",
                repo.to_str().unwrap(),
                "--game",
                "arknights",
                "--server",
                "cn",
                "--locale",
                "zh-CN",
                "--force",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(promote.exit_code(), 2);
        assert_eq!(promote.envelope.error.as_ref().unwrap().code, "resource_production_retired");
        assert_eq!(
            fs::read_to_string(existing_task.join("sentinel.txt")).unwrap(),
            "canonical-before"
        );
        assert!(!existing_task.join("task.json").exists());
        assert_eq!(
            fs::read_to_string(broken_task.join("task.json")).unwrap(),
            "{not-json"
        );
        assert!(!ours.join("recognition/arknights.cn.pack.json").exists());
        host.close().expect("close Runtime host");
    }

    #[test]
    fn session_record_build_task_rejects_unresolved_target_click() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let out = temp.path().join("draft");
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let operation = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "operation",
                "--step-id",
                "open-mail",
                "--from",
                "page/home",
                "--to",
                "page/mail",
                "--click",
                "mail_button",
            ],
            true,
        );
        let build = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "build-task",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--out",
                out.to_str().unwrap(),
                "--game",
                "arknights",
                "--server",
                "cn",
                "--locale",
                "zh-CN",
                "--resolution",
                "1280x720",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(operation.exit_code(), 0);
        assert_eq!(build.exit_code(), 2);
        assert_eq!(build.envelope.error.as_ref().unwrap().code, "resource_production_retired");
        assert!(!out.exists());
    }
    #[test]
    fn session_record_build_task_rejects_missing_page_anchor() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let out = temp.path().join("draft");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let anchor = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "home-anchor",
                "--id",
                "page/home",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
            ],
            true,
        );
        let operation = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "operation",
                "--step-id",
                "missing-mail-anchor",
                "--from",
                "page/home",
                "--to",
                "page/mail",
                "--click",
                "5,6",
            ],
            true,
        );
        let build = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "build-task",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--out",
                out.to_str().unwrap(),
                "--game",
                "arknights",
                "--server",
                "cn",
                "--locale",
                "zh-CN",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(anchor.exit_code(), 0);
        assert_eq!(operation.exit_code(), 0);
        assert_eq!(build.exit_code(), 2);
        assert_eq!(build.envelope.error.as_ref().unwrap().code, "resource_production_retired");
        assert!(!out.exists());
    }
    #[test]
    fn session_record_build_task_rejects_out_of_bounds_click() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let out = temp.path().join("draft");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let anchor = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "home-anchor",
                "--id",
                "page/home",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
            ],
            true,
        );
        let operation = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "operation",
                "--step-id",
                "bad-click",
                "--from",
                "page/home",
                "--to",
                "page/home",
                "--click",
                "100,200",
            ],
            true,
        );
        let build = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "build-task",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--out",
                out.to_str().unwrap(),
                "--game",
                "arknights",
                "--server",
                "cn",
                "--locale",
                "zh-CN",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(anchor.exit_code(), 0);
        assert_eq!(operation.exit_code(), 0);
        assert_eq!(build.exit_code(), 2);
        assert_eq!(build.envelope.error.as_ref().unwrap().code, "resource_production_retired");
        assert!(!out.exists());
    }
    #[test]
    fn session_record_step_requires_active_record() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        set_config_env(&config);

        let result = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--id",
                "page/home",
                "--region",
                "auto",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(result.exit_code(), 3);
        assert_eq!(
            result.envelope.error.as_ref().unwrap().code,
            "record_session_not_active"
        );
    }

    #[test]
    fn session_record_step_rejects_duplicate_step_id() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let first = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "home-anchor",
                "--id",
                "page/home",
                "--region",
                "auto",
            ],
            true,
        );
        let duplicate = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "operation",
                "--step-id",
                "home-anchor",
                "--from",
                "page/home",
                "--to",
                "null",
                "--click",
                "mail_button",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(first.exit_code(), 0);
        assert_eq!(duplicate.exit_code(), 3);
        assert_eq!(
            duplicate.envelope.error.as_ref().unwrap().code,
            "record_step_id_conflict"
        );
    }

    #[test]
    fn session_record_amend_updates_anchor_metadata() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "home-anchor",
                "--id",
                "page/home",
                "--region",
                "10,20,30,40",
                "--color-check",
                "--threshold",
                "0.96",
            ],
            true,
        );
        let amend = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "amend",
                "home-anchor",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--region",
                "auto",
                "--no-color-check",
                "--clear-threshold",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        assert_eq!(amend.exit_code(), 0);
        let data = amend.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.get("status").and_then(Value::as_str),
            Some("step_amended")
        );
        assert_eq!(
            data.pointer("/step/region/mode").and_then(Value::as_str),
            Some("auto")
        );
        assert_eq!(
            data.pointer("/step/color_check").and_then(Value::as_bool),
            Some(false)
        );
        assert!(data.pointer("/step/threshold").is_some_and(Value::is_null));
        assert_eq!(
            data.pointer("/step/evaluation/reason")
                .and_then(Value::as_str),
            Some("amended_without_frame_provenance")
        );
    }

    #[test]
    fn session_record_amend_rebacktests_frame_backed_anchor() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let artifact_dir = state_dir.join("artifacts");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "home-anchor",
                "--id",
                "page/home",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
                "--artifact-dir",
                artifact_dir.to_str().unwrap(),
            ],
            true,
        );
        let amend = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "amend",
                "home-anchor",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--region",
                "1,2,3,4",
                "--threshold",
                "0.90",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        assert_eq!(
            amend.exit_code(),
            0,
            "{}",
            serde_json::to_string_pretty(&amend.envelope).unwrap()
        );
        let data = amend.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/step/evaluation/status")
                .and_then(Value::as_str),
            Some("passed")
        );
        assert_eq!(
            data.pointer("/step/evaluation/reason")
                .and_then(Value::as_str),
            Some("self_backtest_passed")
        );
        assert_eq!(
            data.pointer("/step/evaluation/backtest/x")
                .and_then(Value::as_i64),
            Some(1)
        );
        assert_eq!(
            data.pointer("/step/evaluation/backtest/y")
                .and_then(Value::as_i64),
            Some(2)
        );
        assert!(
            data.pointer("/step/evaluation/backtest/threshold")
                .and_then(Value::as_f64)
                .is_some_and(|threshold| (threshold - 0.90).abs() < 0.00001)
        );
        assert_eq!(
            data.pointer("/step/artifact/width").and_then(Value::as_u64),
            Some(3)
        );
        assert_eq!(
            data.pointer("/step/artifact/height")
                .and_then(Value::as_u64),
            Some(4)
        );
        assert_eq!(
            data.pointer("/step/frame_provenance/path")
                .and_then(Value::as_str),
            Some(frame_path.to_str().unwrap())
        );
        let artifact_path = data
            .pointer("/step/artifact/path")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .expect("artifact path");
        assert!(artifact_path.is_file());
    }

    #[test]
    fn session_record_amend_selects_auto_region_candidate_and_rebacktests() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let contrast_path = temp.path().join("contrast.png");
        fs::write(
            &frame_path,
            test_auto_region_discrimination_frame_png(false),
        )
        .unwrap();
        fs::write(
            &contrast_path,
            test_auto_region_discrimination_frame_png(true),
        )
        .unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "home-anchor",
                "--id",
                "page/home",
                "--region",
                "auto",
                "--frame",
                frame_path.to_str().unwrap(),
                "--contrast-frame",
                contrast_path.to_str().unwrap(),
            ],
            true,
        );
        let amend = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "amend",
                "home-anchor",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--candidate-index",
                "0",
                "--contrast-frame",
                contrast_path.to_str().unwrap(),
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        assert_eq!(amend.exit_code(), 0);
        let data = amend.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/step/region/rect/x").and_then(Value::as_i64),
            Some(0)
        );
        assert_eq!(
            data.pointer("/step/region/rect/y").and_then(Value::as_i64),
            Some(0)
        );
        assert_eq!(
            data.pointer("/step/evaluation/auto_region/selected_reason")
                .and_then(Value::as_str),
            Some("operator_selected_candidate")
        );
        assert_eq!(
            data.pointer("/step/evaluation/auto_region/selected/x")
                .and_then(Value::as_i64),
            Some(0)
        );
        assert_eq!(
            data.pointer("/step/evaluation/auto_region/selected/y")
                .and_then(Value::as_i64),
            Some(0)
        );
        let candidates = data
            .pointer("/step/evaluation/auto_region/candidates")
            .and_then(Value::as_array)
            .expect("auto-region candidates");
        assert_eq!(
            candidates
                .iter()
                .filter(
                    |candidate| candidate.get("selected").and_then(Value::as_bool) == Some(true)
                )
                .count(),
            1
        );
        assert_eq!(
            candidates
                .first()
                .and_then(|candidate| candidate.get("selected"))
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/step/evaluation/status")
                .and_then(Value::as_str),
            Some("failed")
        );
        assert_eq!(
            data.pointer("/step/evaluation/reason")
                .and_then(Value::as_str),
            Some("contrast_backtest_matched")
        );
        assert_eq!(
            data.pointer("/step/evaluation/contrast_backtest/passed")
                .and_then(Value::as_bool),
            Some(false)
        );
    }

    #[test]
    fn session_record_amend_candidate_index_requires_auto_region_report() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "home-anchor",
                "--id",
                "page/home",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
            ],
            true,
        );
        let amend = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "amend",
                "home-anchor",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--candidate-index",
                "0",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        assert_eq!(amend.exit_code(), 2);
        assert_eq!(
            amend.envelope.error.as_ref().unwrap().code,
            "validation_failed"
        );
        assert!(
            amend
                .envelope
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("auto-region candidate report")
        );
    }

    #[test]
    fn session_record_amend_updates_operation_metadata() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "operation",
                "--step-id",
                "open-mail",
                "--from",
                "page/home",
                "--to",
                "page/mail",
                "--click",
                "100,200",
                "--destructive",
            ],
            true,
        );
        let amend = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "amend",
                "--step-id",
                "open-mail",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--to",
                "null",
                "--click",
                "mail_button",
                "--non-destructive",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        assert_eq!(amend.exit_code(), 0);
        let data = amend.envelope.data.as_ref().unwrap();
        assert!(data.pointer("/step/to").is_some_and(Value::is_null));
        assert_eq!(
            data.pointer("/step/click/type").and_then(Value::as_str),
            Some("target")
        );
        assert_eq!(
            data.pointer("/step/click/target").and_then(Value::as_str),
            Some("mail_button")
        );
        assert_eq!(
            data.pointer("/step/destructive").and_then(Value::as_bool),
            Some(false)
        );
    }

    #[test]
    fn session_record_amend_requires_supported_field() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let step = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "home-anchor",
                "--id",
                "page/home",
                "--region",
                "auto",
            ],
            true,
        );
        let amend = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "amend",
                "home-anchor",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--from",
                "page/other",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(step.exit_code(), 0);
        assert_eq!(amend.exit_code(), 2);
        assert_eq!(
            amend.envelope.error.as_ref().unwrap().code,
            "validation_failed"
        );
    }

    #[test]
    fn drift_diagnostics_contract_rejects_fields_outside_amend_whitelist() {
        let diagnostics = json!({
            "trigger": "resource_drift",
            "target_id": "page/home",
            "measured": {
                "matched_rect": {"x": 1, "y": 2, "width": 3, "height": 4}
            },
            "proposed_changes": {
                "region": {"x": 1, "y": 2, "width": 3, "height": 4},
                "click": {"x": 10, "y": 20}
            }
        });

        let err = parse_session_record_drift_diagnostics(PathBuf::from("drift.json"), &diagnostics)
            .expect_err("unsupported proposed field");

        assert!(err.message.contains("outside the amend whitelist"));
    }

    #[test]
    fn recognize_target_output_uses_shared_evaluation_shape() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let pack_root = temp.path().to_path_buf();
        let template_dir = pack_root.join("operations/task/assets");
        let pack_path = temp.path().join("pack.json");
        let pages_path = temp.path().join("pages.json");
        let scene_path = temp.path().join("scene.png");
        fs::create_dir_all(&template_dir).unwrap();
        let png = test_record_frame_png(1, 1);
        fs::write(template_dir.join("HOME.png"), &png).unwrap();
        fs::write(&scene_path, &png).unwrap();
        write_json_file(
            &pack_path,
            &json!({
                "schema_version": "0.3",
                "game": "arknights",
                "server": "cn",
                "locale": "zh-CN",
                "coordinate_space": {"width": 1, "height": 1},
                "defaults": {"template_threshold": 0.9, "color_max_distance": 20.0},
                "targets": [{
                    "type": "template",
                    "id": "page/home",
                    "template_path": "operations/task/assets/HOME.png",
                    "region": {"x": 0, "y": 0, "width": 1, "height": 1},
                    "threshold": 0.9
                }]
            }),
        )
        .unwrap();
        write_json_file(&pages_path, &json!({"schema_version":"0.3","pages":[]})).unwrap();
        set_missing_config_env();
        let temp = seal_semantic_fixture(temp, "arknights", "cn", &pack_path, &pages_path, None);

        let result = run_semantic_cli(
            &temp,
            [
                "--json",
                "recognize",
                "--target",
                "page/home",
                "--scene",
                scene_path.to_str().unwrap(),
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(
            result.exit_code(),
            0,
            "{}",
            serde_json::to_string_pretty(&result.envelope).unwrap()
        );
        let data = result.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/matched_rect/width").and_then(Value::as_i64),
            Some(1)
        );
        assert_eq!(
            data.pointer("/template/height").and_then(Value::as_i64),
            Some(1)
        );
        assert_eq!(
            data.pointer("/evaluation/matched_rect/width")
                .and_then(Value::as_i64),
            Some(1)
        );
        assert_eq!(
            data.pointer("/evaluation/template/height")
                .and_then(Value::as_i64),
            Some(1)
        );
    }

    #[test]
    fn env_needs_detection_hint_is_machine_readable() {
        let values = vec![env_detection::ResolvedEnvValue {
            key: "ui_theme".to_string(),
            value: "Siege".to_string(),
            confidence: 0.72,
            source: "detect_ui_theme@Siege".to_string(),
            detector_id: "detect_ui_theme".to_string(),
            source_result: "detect_ui_theme@1783600000000".to_string(),
        }];

        let hint = env_needs_detection_json(
            "recognize",
            "target_below_threshold",
            "button/depot_enter",
            &values,
        )
        .expect("env hint");

        assert_eq!(
            hint.pointer("/status").and_then(Value::as_str),
            Some("needs_detection")
        );
        assert_eq!(
            hint.pointer("/detector_ids/0").and_then(Value::as_str),
            Some("detect_ui_theme")
        );
        assert_eq!(
            hint.pointer("/keys/0/key").and_then(Value::as_str),
            Some("ui_theme")
        );
        assert_eq!(
            hint.pointer("/keys/0/detector_id").and_then(Value::as_str),
            Some("detect_ui_theme")
        );
    }

    #[test]
    fn drift_diagnostics_uses_measured_matched_rect_without_proposed_region() {
        let diagnostics = json!({
            "trigger": "resource_drift",
            "target_id": "page/home",
            "measured": {
                "matched_rect": {"x": 4, "y": 5, "width": 6, "height": 7}
            }
        });

        let parsed =
            parse_session_record_drift_diagnostics(PathBuf::from("drift.json"), &diagnostics)
                .expect("measured matched_rect is a valid fallback");

        assert_eq!(parsed.region.x, 4);
        assert_eq!(parsed.region.y, 5);
        assert_eq!(parsed.region.width, 6);
        assert_eq!(parsed.region.height, 7);
        assert_eq!(parsed.changed_fields, vec!["region"]);
    }

    #[test]
    fn record_drift_target_not_found_is_safety_blocked() {
        let record = drift_test_record(json!([drift_test_anchor_step("home-anchor", "page/home")]));
        let diagnostics = parse_session_record_drift_diagnostics(
            PathBuf::from("drift.json"),
            &json!({
                "trigger": "resource_drift",
                "target_id": "page/missing",
                "measured": {
                    "matched_rect": {"x": 1, "y": 2, "width": 3, "height": 4}
                }
            }),
        )
        .unwrap();

        let err = find_drift_amend_step(&record, &diagnostics, None)
            .expect_err("missing drift target must fail");

        assert_eq!(err.code, "record_drift_target_not_found");
    }

    #[test]
    fn record_drift_target_ambiguous_is_safety_blocked() {
        let record = drift_test_record(json!([
            drift_test_anchor_step("home-anchor", "home"),
            drift_test_anchor_step("page-home-anchor", "page/home")
        ]));
        let diagnostics = parse_session_record_drift_diagnostics(
            PathBuf::from("drift.json"),
            &json!({
                "trigger": "resource_drift",
                "target_id": "page/home",
                "measured": {
                    "matched_rect": {"x": 1, "y": 2, "width": 3, "height": 4}
                }
            }),
        )
        .unwrap();

        let err = find_drift_amend_step(&record, &diagnostics, None)
            .expect_err("ambiguous drift target must fail");

        assert_eq!(err.code, "record_drift_target_ambiguous");
    }

    #[test]
    fn session_record_amend_from_drift_diagnostics_updates_anchor() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        let frame_path = temp.path().join("source.png");
        let diagnostics_path = temp.path().join("drift.json");

        fs::write(&frame_path, test_record_frame_png(12, 10)).unwrap();
        write_json_file(
            &diagnostics_path,
            &json!({
                "trigger": "resource_drift",
                "target_id": "page/home",
                "measured": {
                    "matched_rect": {"x": 1, "y": 2, "width": 3, "height": 4},
                    "template": {"score": 0.82, "threshold": 0.95}
                },
                "proposed_changes": {
                    "region": {"mode": "rect", "rect": {"x": 1, "y": 2, "width": 3, "height": 4}},
                    "threshold": 0.90
                }
            }),
        )
        .unwrap();
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let anchor = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "anchor",
                "--step-id",
                "home-anchor",
                "--id",
                "home",
                "--region",
                "2,3,4,5",
                "--frame",
                frame_path.to_str().unwrap(),
            ],
            true,
        );
        let operation = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "step",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--kind",
                "operation",
                "--step-id",
                "open-home",
                "--from",
                "home",
                "--to",
                "home",
                "--click",
                "4,4",
            ],
            true,
        );
        let amend = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "amend",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--from-drift-diagnostics",
                diagnostics_path.to_str().unwrap(),
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(anchor.exit_code(), 0);
        assert_eq!(operation.exit_code(), 0);
        assert_eq!(
            amend.exit_code(),
            0,
            "{}",
            serde_json::to_string_pretty(&amend.envelope).unwrap()
        );
        let amend_data = amend.envelope.data.as_ref().unwrap();
        assert_eq!(
            amend_data.get("status").and_then(Value::as_str),
            Some("drift_diagnostics_amended")
        );
        assert_eq!(
            amend_data.pointer("/amend/step_id").and_then(Value::as_str),
            Some("home-anchor")
        );
        assert_eq!(
            amend_data
                .pointer("/amend/changed_fields/0")
                .and_then(Value::as_str),
            Some("region")
        );
        assert_eq!(
            amend_data
                .pointer("/amend/changed_fields/1")
                .and_then(Value::as_str),
            Some("threshold")
        );
        assert_eq!(
            amend_data
                .pointer("/record/steps/0/region/rect/x")
                .and_then(Value::as_i64),
            Some(1)
        );
        assert_eq!(
            amend_data
                .pointer("/record/steps/0/threshold")
                .and_then(Value::as_f64),
            Some(0.90)
        );
        assert_eq!(
            amend_data
                .pointer("/record/steps/0/evaluation/status")
                .and_then(Value::as_str),
            Some("passed")
        );
        assert!(
            amend_data
                .pointer("/record/steps/0/evaluation/backtest")
                .is_some()
        );
    }

    #[test]
    fn session_record_amend_from_drift_diagnostics_requires_readable_json() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        let state_dir = temp.path().join("session");
        set_config_env(&config);

        let start = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "start",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--task-id",
                "daily-check",
            ],
            true,
        );
        let amend = run_cli(
            [
                "--json",
                "--instance",
                "ak",
                "session",
                "record",
                "amend",
                "--state-dir",
                state_dir.to_str().unwrap(),
                "--from-drift-diagnostics",
                temp.path().join("missing.json").to_str().unwrap(),
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(start.exit_code(), 0);
        assert_eq!(amend.exit_code(), 2);
        assert_eq!(
            amend.envelope.error.as_ref().unwrap().code,
            "validation_failed"
        );
        assert!(
            amend
                .envelope
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("drift diagnostics file is missing")
        );
    }
