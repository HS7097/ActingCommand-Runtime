    #[test]
    fn runtime_endpoint_policy_allows_loopback_without_auth() {
        let _guard = env_lock();
        let _trusted_remote_env = TrustedRemoteEnvGuard::clear();
        let policy = runtime_endpoint_policy("http://127.0.0.1:4317").unwrap();
        assert_eq!(policy.channel, RuntimeEndpointChannel::LocalDirect);
        assert_eq!(policy.scheme, "http");
        assert_eq!(policy.host, "127.0.0.1");
        assert_eq!(policy.port, 4317);
        assert_eq!(policy.auth_material, None);
    }

    #[test]
    fn runtime_endpoint_policy_blocks_remote_http() {
        let _guard = env_lock();
        let _trusted_remote_env = TrustedRemoteEnvGuard::clear();
        let err = runtime_endpoint_policy("http://example.invalid:4317").unwrap_err();
        assert_eq!(err.code, "trusted_remote_transport_blocked");
        assert_eq!(err.exit_code(), 3);
    }

    #[test]
    fn runtime_endpoint_policy_blocks_remote_https_without_auth() {
        let _guard = env_lock();
        let _trusted_remote_env = TrustedRemoteEnvGuard::clear();
        let err = runtime_endpoint_policy("https://example.invalid:4317").unwrap_err();
        assert_eq!(err.code, "trusted_remote_auth_required");
        assert_eq!(err.exit_code(), 3);
    }

    #[test]
    fn runtime_endpoint_policy_accepts_remote_https_with_token() {
        let _guard = env_lock();
        let _trusted_remote_env = TrustedRemoteEnvGuard::with_token("test-token");
        let policy = runtime_endpoint_policy("https://example.invalid:4317").unwrap();
        assert_eq!(policy.channel, RuntimeEndpointChannel::TrustedRemote);
        assert_eq!(policy.scheme, "https");
        assert_eq!(policy.auth_material, Some("token"));
    }

    #[test]
    fn session_transport_check_reports_loopback_policy() {
        let _guard = env_lock();
        let _trusted_remote_env = TrustedRemoteEnvGuard::clear();
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
            data.pointer("/check/policy/authentication_required")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            data.get("does_not_start_listener").and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn session_transport_plan_reports_reserved_trusted_channel_without_listener() {
        let _guard = env_lock();
        let _trusted_remote_env = TrustedRemoteEnvGuard::clear();
        let result = run_cli(["--json", "session", "transport", "plan"], true);

        assert_eq!(result.exit_code(), 0);
        let data = result.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.get("schema_version").and_then(Value::as_str),
            Some("session.transport_plan.v0.1")
        );
        assert_eq!(data.get("status").and_then(Value::as_str), Some("reserved"));
        assert_eq!(
            data.pointer("/trusted_remote/network_listener_implemented")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            data.pointer("/trusted_remote/ready_to_accept_remote_clients")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            data.pointer("/trusted_remote/token_configured")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            data.pointer("/trusted_remote/endpoint_policy/checked")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            data.pointer("/trusted_remote_gate/schema_version")
                .and_then(Value::as_str),
            Some("session.trusted_remote_gate.v0.1")
        );
        assert_eq!(
            data.pointer("/trusted_remote_gate/status")
                .and_then(Value::as_str),
            Some("reserved")
        );
        assert_eq!(
            data.pointer("/trusted_remote_gate/auth_material_configured")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            data.pointer("/trusted_remote_gate/safe_to_accept_remote_clients")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert!(
            data.pointer("/trusted_remote_gate/blocked_reasons")
                .and_then(Value::as_array)
                .expect("trusted remote gate must expose blocked reasons")
                .iter()
                .any(|reason| {
                    reason.get("code").and_then(Value::as_str)
                        == Some("trusted_remote_auth_required")
                })
        );
        assert_eq!(
            data.pointer("/next_actions/schema_version")
                .and_then(Value::as_str),
            Some("session.transport_next_actions.v0.1")
        );
        assert_eq!(
            data.pointer("/next_actions/ordered/0/action")
                .and_then(Value::as_str),
            Some("classify_endpoint_policy")
        );
        assert_eq!(
            data.pointer("/next_actions/trusted_remote/auth_material_configured")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            data.pointer("/guarantees/does_not_start_listener")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/guarantees/does_not_probe_tcp")
                .and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn session_transport_plan_blocks_remote_http_without_tcp_probe() {
        let _guard = env_lock();
        let _trusted_remote_env = TrustedRemoteEnvGuard::clear();
        let result = run_cli(
            [
                "--json",
                "session",
                "transport",
                "plan",
                "--endpoint",
                "http://192.0.2.1:4317",
            ],
            true,
        );

        assert_eq!(result.exit_code(), 0);
        let data = result.envelope.data.as_ref().unwrap();
        assert_eq!(data.get("status").and_then(Value::as_str), Some("blocked"));
        assert_eq!(
            data.pointer("/trusted_remote/endpoint_policy/safe_for_policy")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            data.pointer("/trusted_remote/endpoint_policy/error_code")
                .and_then(Value::as_str),
            Some("trusted_remote_transport_blocked")
        );
        assert_eq!(
            data.pointer("/trusted_remote_gate/status")
                .and_then(Value::as_str),
            Some("blocked")
        );
        assert_eq!(
            data.pointer("/trusted_remote_gate/endpoint_policy_safe")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            data.pointer("/trusted_remote_gate/guarantees/does_not_probe_tcp")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/trusted_remote/endpoint_policy/does_not_probe_tcp")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert!(
            data.get("blockers")
                .and_then(Value::as_array)
                .unwrap()
                .iter()
                .any(|blocker| blocker.get("kind").and_then(Value::as_str)
                    == Some("trusted_remote_endpoint_policy"))
        );
        assert_eq!(
            data.pointer("/next_actions/status").and_then(Value::as_str),
            Some("blocked")
        );
        assert_eq!(
            data.pointer("/next_actions/ordered/0/action")
                .and_then(Value::as_str),
            Some("review_endpoint_policy_blocker")
        );
        assert_eq!(
            data.pointer("/next_actions/guarantees/does_not_probe_tcp")
                .and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn session_transport_plan_accepts_remote_https_policy_but_keeps_listener_reserved() {
        let _guard = env_lock();
        let _trusted_remote_env = TrustedRemoteEnvGuard::with_token("test-token");
        let result = run_cli(
            [
                "--json",
                "session",
                "transport",
                "plan",
                "--endpoint",
                "https://example.invalid:4317",
            ],
            true,
        );
        assert_eq!(result.exit_code(), 0);
        let data = result.envelope.data.as_ref().unwrap();
        assert_eq!(data.get("status").and_then(Value::as_str), Some("reserved"));
        assert_eq!(
            data.pointer("/trusted_remote/endpoint_policy/safe_for_policy")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/trusted_remote/endpoint_policy/policy/channel")
                .and_then(Value::as_str),
            Some("trusted_remote")
        );
        assert_eq!(
            data.pointer("/trusted_remote/endpoint_policy/policy/auth_material")
                .and_then(Value::as_str),
            Some("token")
        );
        assert_eq!(
            data.pointer("/trusted_remote_gate/status")
                .and_then(Value::as_str),
            Some("reserved")
        );
        assert_eq!(
            data.pointer("/trusted_remote_gate/trusted_remote_requested")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/trusted_remote_gate/auth_material_configured")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/trusted_remote_gate/network_listener_implemented")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            data.pointer("/trusted_remote_gate/guarantees/does_not_start_tls")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/trusted_remote/ready_to_accept_remote_clients")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            data.pointer("/next_actions/status").and_then(Value::as_str),
            Some("reserved")
        );
        assert_eq!(
            data.pointer("/next_actions/trusted_remote/auth_material_configured")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            data.pointer("/next_actions/ordered/0/action")
                .and_then(Value::as_str),
            Some("review_listener_and_tls_design")
        );
    }

    #[test]
    fn session_transport_check_blocks_remote_http() {
        let _guard = env_lock();
        let _trusted_remote_env = TrustedRemoteEnvGuard::clear();
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
            Some("trusted_remote_transport_blocked")
        );
        assert_eq!(
            data.pointer("/check/blocked_by/1").and_then(Value::as_str),
            Some("encryption")
        );
    }

    #[test]
    fn status_blocks_untrusted_remote_runtime_endpoint() {
        let _guard = env_lock();
        set_missing_config_env();
        let _trusted_remote_env = TrustedRemoteEnvGuard::clear();
        let result = run_cli(
            [
                "--json",
                "--runtime-endpoint",
                "http://example.invalid:4317",
                "status",
            ],
            true,
        );
        assert_eq!(result.exit_code(), 3);
        assert_eq!(
            result.envelope.error.as_ref().unwrap().code,
            "trusted_remote_transport_blocked"
        );
    }

    #[test]
    fn doctor_reports_remote_endpoint_policy_without_blocking() {
        let _guard = env_lock();
        set_missing_config_env();
        let _trusted_remote_env = TrustedRemoteEnvGuard::clear();
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
            Some("trusted_remote_auth_required")
        );
    }
