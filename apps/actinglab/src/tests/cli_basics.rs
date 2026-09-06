    // Specification: https://github.com/HS7097/ActingCommand-Workflow/issues/269#issuecomment-5556059085
    #[test]
    fn scheduling_inspection_compiles_and_queries_without_runtime_state() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let runtime_root = temp.path().join("runtime-must-not-exist");
        let _runtime_env = use_runtime_state_root(&runtime_root);
        let config = temp.path().join("unread-config.json");
        fs::write(&config, "not a Runtime configuration").unwrap();
        set_config_env(&config);
        let example = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../contracts/scheduling/examples/catalog-a");
        let mut documents = Vec::new();
        let mut compile_args = vec!["--json".to_owned(), "scheduling".into(), "compile".into()];
        for name in ["tasks", "pools", "activity", "timeline"] {
            let bytes = fs::read(example.join(format!("{name}.json"))).unwrap();
            let path = temp.path().join(format!("{name}.json"));
            fs::write(&path, &bytes).unwrap();
            documents.push((path.clone(), serde_json::from_slice::<Value>(&bytes).unwrap()));
            compile_args.extend([format!("--{name}"), path.to_str().unwrap().to_owned()]);
        }
        let compiled = run_cli(compile_args.clone(), true);
        assert_eq!(compiled.exit_code(), 0, "{}", compiled.envelope_json());
        let json: Value = serde_json::from_str(&compiled.envelope_json()).unwrap();
        assert_eq!(json["command"], "scheduling compile");
        assert_eq!(json["data"]["status"], "accepted");
        assert_eq!(json["data"]["summary"]["catalog_id"], "fixture.catalog-a");
        let hash = json["data"]["summary"]["catalog_hash"].as_str().unwrap();
        assert!(hash.starts_with("sha256:"));
        assert_eq!(hash.len(), 71);
        assert!(!runtime_root.exists());
        for command in ["scheduling compile", "scheduling timeline"] {
            let capability = command_capabilities().into_iter()
                .find(|entry| entry["command"] == command).unwrap();
            assert_eq!(capability["needs"], json!(["offline", "read_only"]));
        }

        // A mixed version preserves the compiler's original structured diagnostics.
        documents[3].1["schema_version"] = json!("actingcommand.scheduling.v2");
        fs::write(&documents[3].0, serde_json::to_vec(&documents[3].1).unwrap()).unwrap();
        let rejected = run_cli(compile_args.clone(), true);
        assert_eq!(rejected.exit_code(), 2);
        let rejected: Value = serde_json::from_str(&rejected.envelope_json()).unwrap();
        assert_eq!(rejected["error"]["code"], "scheduling_catalog_rejected");
        assert_eq!(rejected["error"]["details"]["status"], "rejected");
        assert!(!rejected["error"]["details"]["diagnostics"].as_array().unwrap().is_empty());

        for (_, document) in &mut documents {
            document["schema_version"] = json!("actingcommand.scheduling.v2");
        }
        documents[3].1["events"] = json!([{
            "id":"neutral.window", "scope":{"kind":"server","server_id":"fixture-server-a"},
            "event_kind":"activity", "schedule":{"kind":"at","at_ms":1000,
                "clock_source":{"kind":"server","timezone_id":"fixed/utc","utc_offset_minutes":0,"dst_offset_minutes":0,"maintenance_drift_ms":0}},
            "duration_ms":1000, "invalidates_fact_prefixes":[],
            "validity":{"from_unix_ms":1100,"until_unix_ms":1900}
        }]);
        for (path, document) in &documents {
            fs::write(path, serde_json::to_vec(document).unwrap()).unwrap();
        }
        let mut query_args = compile_args.clone();
        query_args[2] = "timeline".into();
        query_args.extend(["--event-id", "neutral.window", "--unix-ms", "1100", "--monotonic-ms", "50",
            "--instance-id", "fixture-instance-a", "--server-id", "fixture-server-a", "--game-id", "fixture-game-a"].map(str::to_owned));
        let queried = run_cli(query_args.clone(), true);
        assert_eq!(queried.exit_code(), 0, "{}", queried.envelope_json());
        let queried: Value = serde_json::from_str(&queried.envelope_json()).unwrap();
        assert_eq!(queried["command"], "scheduling timeline");
        let timeline = &queried["data"]["timeline"];
        assert_eq!(timeline["time"], json!({"unix_ms":1100,"monotonic_ms":50}));
        assert_eq!(timeline["context"]["instance_id"], "fixture-instance-a");
        assert_eq!(timeline["events"][0]["scope_applies"], true);
        assert_eq!(timeline["events"][0]["availability"]["state"], "true");
        assert_eq!(timeline["events"][0]["availability"]["active_interval"], json!([1100,1900]));
        assert_eq!(timeline["next_wake_unix_ms"], 1900);
        for (flag, value) in [("--event-id", "unknown"), ("--monotonic-ms", "invalid")] {
            let mut invalid = query_args.clone();
            let index = invalid.iter().position(|arg| arg == flag).unwrap();
            invalid[index+1] = value.into();
            let result = run_cli(invalid, true);
            assert_eq!(result.exit_code(), 2);
            assert_eq!(serde_json::from_str::<Value>(&result.envelope_json()).unwrap()["ok"], false);
        }
        let mut missing_clock = query_args.clone();
        let index = missing_clock.iter().position(|arg| arg == "--unix-ms").unwrap();
        missing_clock.drain(index..index+2);
        assert_eq!(run_cli(missing_clock, true).exit_code(), 2);
        for extra in [vec!["--runtime-endpoint", "unused"], vec!["--version"], vec!["--tasks", "duplicate"], vec!["--unexpected", "value"]] {
            let mut invalid = compile_args.clone();
            invalid.extend(extra.into_iter().map(str::to_owned));
            assert_eq!(run_cli(invalid, true).exit_code(), 2);
        }
        for bytes in [b"{".to_vec(), vec![b' '; 1_048_577]] {
            fs::write(&documents[3].0, bytes).unwrap();
            let rejected = run_cli(compile_args.clone(), true);
            assert_eq!(rejected.exit_code(), 2);
            assert_eq!(rejected.envelope.error.as_ref().unwrap().code, "scheduling_catalog_rejected");
        }
        fs::remove_file(&documents[3].0).unwrap();
        let missing = run_cli(compile_args, true);
        assert_eq!(missing.exit_code(), 2);
        assert_eq!(missing.envelope.error.unwrap().code, "scheduling_source_read_failed");
        assert!(!runtime_root.exists());
        assert_eq!(fs::read_to_string(&config).unwrap(), "not a Runtime configuration");
        set_missing_config_env();
    }

    #[test]
    fn doctor_reports_path_adb_baseline_warning() {
        let adb = resolved_adb_json_from(Ok(path_baseline_adb()));
        assert_eq!(
            adb.get("source").and_then(Value::as_str),
            Some("path_adb_baseline")
        );
        assert!(
            adb.get("warning")
                .and_then(Value::as_str)
                .is_some_and(|warning| warning.contains("non-MuMu baseline"))
        );
    }

    #[test]
    fn device_config_rejects_path_adb_for_nemu_ipc_without_opt_in() {
        let _guard = env_lock();
        unsafe {
            env::remove_var(ALLOW_PATH_ADB_FOR_MUMU_ENV);
        }
        let instance = InstanceConfig {
            capture_backend: Some("nemu_ipc".to_string()),
            ..Default::default()
        };

        let error = enforce_path_adb_target_boundary(
            &path_baseline_adb(),
            Some(&instance),
            CaptureBackendChoice::NemuIpc,
        )
        .expect_err("MuMu/Nemu IPC must not use PATH baseline by default");

        assert_eq!(error.code, "device_error");
        assert!(error.message.contains(ALLOW_PATH_ADB_FOR_MUMU_ENV));
    }

    #[test]
    fn device_config_allows_path_adb_for_nemu_ipc_with_explicit_opt_in() {
        let _guard = env_lock();
        unsafe {
            env::set_var(ALLOW_PATH_ADB_FOR_MUMU_ENV, "1");
        }
        let instance = InstanceConfig {
            capture_backend: Some("nemu_ipc".to_string()),
            ..Default::default()
        };
        let resolved = path_baseline_adb();

        enforce_path_adb_target_boundary(&resolved, Some(&instance), CaptureBackendChoice::NemuIpc)
            .expect("explicit opt-in allows PATH baseline");

        assert_eq!(resolved.source, AdbPathSource::PathBaseline);
        assert!(
            resolved
                .warning
                .as_deref()
                .is_some_and(|warning| warning.contains("non-MuMu baseline"))
        );
        unsafe {
            env::remove_var(ALLOW_PATH_ADB_FOR_MUMU_ENV);
        }
    }

    #[test]
    fn scheduler_stub_is_exit_six() {
        let result = run_cli(["--json", "scheduler", "status"], true);
        assert_eq!(result.exit_code(), 6);
        assert_eq!(
            result.envelope.error.as_ref().unwrap().code,
            "scheduler_not_available"
        );
    }

    #[test]
    fn run_summary_capability_is_read_only_and_available() {
        let command = command_capabilities()
            .into_iter()
            .find(|command| command.get("command").and_then(Value::as_str) == Some("run summary"))
            .expect("run summary capability");
        assert_eq!(
            command.get("status").and_then(Value::as_str),
            Some("available")
        );
        assert_eq!(
            command.get("needs").and_then(Value::as_array),
            Some(&vec![
                Value::String("running_runtime".to_string()),
                Value::String("read_only".to_string()),
            ])
        );
        assert!(
            command
                .get("needs")
                .and_then(Value::as_array)
                .is_some_and(|needs| needs.iter().all(Value::is_string))
        );
    }

    #[test]
    fn resource_compile_maa_capability_is_offline_and_available() {
        let command = command_capabilities()
            .into_iter()
            .find(|command| {
                command.get("command").and_then(Value::as_str) == Some("resource compile-maa")
            })
            .expect("resource compile-maa capability");
        assert_eq!(
            command.get("status").and_then(Value::as_str),
            Some("available")
        );
        assert_eq!(
            command.get("needs").and_then(Value::as_array),
            Some(&vec![Value::String("offline".to_string())])
        );
    }

    #[test]
    fn config_set_and_get_round_trip() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        set_config_env(&config);

        let set = run_cli(
            [
                "--json",
                "config",
                "set",
                "instance.ba.serial",
                "127.0.0.1:16448",
            ],
            true,
        );
        assert_eq!(set.exit_code(), 0);
        let get = run_cli(["--json", "config", "get", "instance.ba.serial"], true);
        set_missing_config_env();

        assert_eq!(get.exit_code(), 0);
        assert_eq!(
            get.envelope
                .data
                .as_ref()
                .unwrap()
                .get("value")
                .and_then(Value::as_str),
            Some("127.0.0.1:16448")
        );
    }

    #[test]
    fn config_set_and_get_instance_package() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        set_config_env(&config);

        let set = run_cli(
            [
                "--json",
                "config",
                "set",
                "instance.ak.package",
                "com.hypergryph.arknights.bilibili",
            ],
            true,
        );
        assert_eq!(set.exit_code(), 0);
        let get = run_cli(["--json", "config", "get", "instance.ak.package"], true);
        set_missing_config_env();

        assert_eq!(get.exit_code(), 0);
        assert_eq!(
            get.envelope
                .data
                .as_ref()
                .unwrap()
                .get("value")
                .and_then(Value::as_str),
            Some("com.hypergryph.arknights.bilibili")
        );
    }

    #[test]
    fn config_set_and_get_instance_adb_and_capture_backend() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        set_config_env(&config);

        let adb = run_cli(
            [
                "--json",
                "config",
                "set",
                "instance.ak-b.adb_path",
                "C:\\Tools\\adb.exe",
            ],
            true,
        );
        let backend = run_cli(
            [
                "--json",
                "config",
                "set",
                "instance.ak-b.capture_backend",
                "nemu_ipc",
            ],
            true,
        );
        let get_adb = run_cli(["--json", "config", "get", "instance.ak-b.adb_path"], true);
        let get_backend = run_cli(
            ["--json", "config", "get", "instance.ak-b.capture_backend"],
            true,
        );
        set_missing_config_env();

        assert_eq!(adb.exit_code(), 0);
        assert_eq!(backend.exit_code(), 0);
        assert_eq!(
            get_adb
                .envelope
                .data
                .as_ref()
                .unwrap()
                .get("value")
                .and_then(Value::as_str),
            Some("C:\\Tools\\adb.exe")
        );
        assert_eq!(
            get_backend
                .envelope
                .data
                .as_ref()
                .unwrap()
                .get("value")
                .and_then(Value::as_str),
            Some("nemu_ipc")
        );
    }

    #[test]
    fn config_set_rejects_invalid_instance_capture_backend() {
        let _guard = env_lock();
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config.json");
        set_config_env(&config);

        let result = run_cli(
            [
                "--json",
                "config",
                "set",
                "instance.ak-b.capture_backend",
                "not-a-backend",
            ],
            true,
        );
        set_missing_config_env();

        assert_eq!(result.exit_code(), 2);
        assert_eq!(
            result.envelope.error.as_ref().unwrap().code,
            "validation_failed"
        );
    }

    #[test]
    fn write_json_file_atomic_uses_unique_tmp_and_publishes_complete_json() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("state.json");
        let stale_tmp = path.with_extension(format!("tmp-{}-stale", std::process::id()));
        fs::write(&stale_tmp, "stale").unwrap();

        for value in [
            json!({"value": 1}),
            json!({"value": 2}),
            json!({"value": 3}),
        ] {
            write_json_file_atomic(&path, &value).unwrap();
        }

        let stored = fs::read_to_string(&path).unwrap();
        let parsed: Value = serde_json::from_str(&stored).unwrap();
        assert_eq!(parsed.get("value").and_then(Value::as_u64), Some(3));
        assert!(!stale_tmp.exists());
        let leftovers = fs::read_dir(temp.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().contains("tmp-"))
            .count();
        assert_eq!(leftovers, 0);
    }
