    #[test]
    fn runtime_endpoint_policy_allows_loopback_without_auth() {
        let _guard = env_lock();
        let policy = runtime_endpoint_policy("http://127.0.0.1:4317").unwrap();
        assert_eq!(policy.channel, RuntimeEndpointChannel::LocalDirect);
        assert_eq!(policy.scheme, "http");
        assert_eq!(policy.host, "127.0.0.1");
        assert_eq!(policy.port, 4317);
    }

    #[test]
    fn runtime_endpoint_policy_blocks_remote_http() {
        let _guard = env_lock();
        let err = runtime_endpoint_policy("http://example.invalid:4317").unwrap_err();
        assert_eq!(err.code, "runtime_endpoint_not_local");
        assert_eq!(err.exit_code(), 3);
    }

    #[test]
    fn runtime_endpoint_policy_blocks_remote_https_without_auth() {
        let _guard = env_lock();
        let err = runtime_endpoint_policy("https://example.invalid:4317").unwrap_err();
        assert_eq!(err.code, "runtime_endpoint_not_local");
        assert_eq!(err.exit_code(), 3);
    }

    #[test]
    fn session_transport_check_reports_loopback_policy() {
        let _guard = env_lock();
        let result = run_cli(
            [
                "--json",
                "session",
                "transport",
                "check",
                "--endpoint",
                "http://127.0.0.1:4317",
            ],
            true,
        );

        assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
        let data = result.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.get("schema_version").and_then(Value::as_str),
            Some("session.transport_check.v0.1")
        );
        assert_eq!(
            data.pointer("/check/policy/channel")
                .and_then(Value::as_str),
            Some("local_direct")
        );
        assert_eq!(
            data.get("does_not_start_listener").and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn session_transport_check_blocks_remote_http() {
        let _guard = env_lock();
        let result = run_cli(
            [
                "--json",
                "session",
                "transport",
                "check",
                "--endpoint",
                "http://192.0.2.1:4317",
            ],
            true,
        );

        assert_eq!(result.exit_code(), 0);
        let data = result.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.get("safe_to_connect").and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            data.pointer("/check/error_code").and_then(Value::as_str),
            Some("runtime_endpoint_not_local")
        );
        assert_eq!(
            data.pointer("/check/blocked_by/0").and_then(Value::as_str),
            Some("local_runtime_only")
        );
    }

    // Workflow #434 LAB-2b fix 2: actinglab's own `status`, which refused an untrusted remote
    // endpoint here, is the CLI module's core row now (its case is in
    // `gate_434_lab2b::status_and_run_show_take_their_core_rows`). `doctor` takes the same path
    // through the own dispatcher: the endpoint of `--runtime-endpoint`
    // (`effective_runtime_endpoint`) through `runtime_endpoint_policy`, before any TCP probe. The
    // endpoint is refused and never probed: only a probed endpoint's check carries `reachable`.
    #[test]
    fn doctor_refuses_untrusted_remote_runtime_endpoint_before_any_request() {
        let _guard = env_lock();
        set_missing_config_env();
        let result = run_cli(
            [
                "--json",
                "--runtime-endpoint",
                "http://example.invalid:4317",
                "doctor",
            ],
            true,
        );
        assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
        let checks = result
            .envelope
            .data
            .as_ref()
            .unwrap()
            .get("checks")
            .and_then(Value::as_array)
            .unwrap();
        let runtime = checks
            .iter()
            .find(|check| check.get("name").and_then(Value::as_str) == Some("runtime_endpoint"))
            .expect("runtime endpoint check");
        assert_eq!(runtime.get("ok").and_then(Value::as_bool), Some(false));
        assert_eq!(
            runtime.get("endpoint").and_then(Value::as_str),
            Some("http://example.invalid:4317")
        );
        assert_eq!(
            runtime
                .pointer("/policy/error_code")
                .and_then(Value::as_str),
            Some("runtime_endpoint_not_local")
        );
        assert_eq!(
            runtime
                .pointer("/policy/blocked_by/0")
                .and_then(Value::as_str),
            Some("local_runtime_only")
        );
        assert!(runtime.pointer("/policy/reachable").is_none(), "{runtime}");
    }

    #[test]
    fn doctor_reports_remote_endpoint_policy_without_blocking() {
        let _guard = env_lock();
        set_missing_config_env();
        let result = run_cli(
            [
                "--json",
                "--runtime-endpoint",
                "https://example.invalid:4317",
                "doctor",
            ],
            true,
        );
        assert_eq!(result.exit_code(), 0);
        let checks = result
            .envelope
            .data
            .as_ref()
            .unwrap()
            .get("checks")
            .and_then(Value::as_array)
            .unwrap();
        let runtime = checks
            .iter()
            .find(|check| check.get("name").and_then(Value::as_str) == Some("runtime_endpoint"))
            .expect("runtime endpoint check");
        assert_eq!(runtime.get("ok").and_then(Value::as_bool), Some(false));
        assert_eq!(
            runtime
                .pointer("/policy/error_code")
                .and_then(Value::as_str),
            Some("runtime_endpoint_not_local")
        );
    }
