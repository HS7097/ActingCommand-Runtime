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
    fn one_off_355_s5_actinglab_connects_only_to_a_local_runtime() {
        let _guard = env_lock();
        for endpoint in [
            "http://127.0.0.1:4317",
            "127.0.0.2:4317",
            "http://[::1]:4317",
            "https://127.0.0.1:4317",
        ] {
            let policy = runtime_endpoint_policy(endpoint).expect("loopback IP literal accepted");
            assert_eq!(policy.channel, RuntimeEndpointChannel::LocalDirect);
            println!(
                "one-off #355 S5: {endpoint} -> local_direct host={}",
                policy.host
            );
        }
        for endpoint in [
            "http://127.evil.com:4317",
            "http://localhost:4317",
            "localhost:4317",
            "https://localhost.attacker.com:4317",
            "http://0x7f.0.0.1:4317",
            "https://example.invalid:4317",
            "http://192.0.2.1:4317",
            "http://[::ffff:127.0.0.1]:4317",
        ] {
            let err = runtime_endpoint_policy(endpoint).expect_err("non-local endpoint refused");
            assert_eq!(err.code, "runtime_endpoint_not_local", "{endpoint}");
            assert_eq!(err.exit_code(), 3, "{endpoint}");
            println!(
                "one-off #355 S5: {endpoint} -> {} exit {}: {}",
                err.code,
                err.exit_code(),
                err.message
            );
        }
        for args in [
            vec!["--json", "session", "transport", "plan"],
            vec![
                "--json",
                "session",
                "transport",
                "plan",
                "--endpoint",
                "https://example.invalid:4317",
            ],
        ] {
            let result = run_cli(args.clone(), true);
            assert_eq!(result.exit_code(), 6, "{}", result.envelope_json());
            assert_eq!(
                result.envelope.error.as_ref().unwrap().code,
                "trusted_remote_transport_retired"
            );
            println!(
                "one-off #355 S5: {:?} -> exit {} {}",
                args,
                result.exit_code(),
                result.envelope.error.as_ref().unwrap().code
            );
        }
        let check = run_cli(
            [
                "--json",
                "session",
                "transport",
                "check",
                "--endpoint",
                "http://127.evil.com:4317",
            ],
            true,
        );
        assert_eq!(check.exit_code(), 0, "{}", check.envelope_json());
        let data = check.envelope.data.as_ref().unwrap();
        assert_eq!(
            data.pointer("/check/error_code").and_then(Value::as_str),
            Some("runtime_endpoint_not_local")
        );
        assert_eq!(
            data.get("safe_to_connect").and_then(Value::as_bool),
            Some(false)
        );
        println!(
            "one-off #355 S5: transport check 127.evil.com -> exit {} {}",
            check.exit_code(),
            check.envelope_json()
        );
        let commands = command_capabilities();
        let plan = commands
            .iter()
            .find(|command| command["command"] == "session transport plan")
            .expect("session transport plan capability");
        assert_eq!(plan["status"], "retired");
        assert_eq!(plan["reason_code"], "trusted_remote_transport_retired");
        println!("one-off #355 S5: capability {plan}");
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

    #[test]
    fn status_blocks_untrusted_remote_runtime_endpoint() {
        let _guard = env_lock();
        set_missing_config_env();
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
            "runtime_endpoint_not_local"
        );
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
