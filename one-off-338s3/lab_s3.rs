
// ---------------------------------------------------------------------------------------------
// One-off (to be reverted): Workflow #338 S3 evidence. The workflow appends this module to a copy
// of apps/actinglab/tests/runtime_input_proxy.rs (apps/actinglab/tests/oneoff_338s3.rs) to reuse
// its in-process fixture Runtime (FakeProvider) and resource writers. Every actinglab, actingctl
// and actingd it runs is the product commit's exact-SHA build (ONEOFF_ACTINGLAB, ONEOFF_ACTINGCTL,
// ONEOFF_ACTINGD); the same actinglab.exe is the CLI side and the install's tools\actinglab.exe.
mod oneoff_338s3 {
    use super::*;
    use serde_json::{Value, json};
    use std::collections::BTreeSet;
    use std::io::{BufRead, BufReader};
    use std::path::{Path, PathBuf};
    use std::process::{Child, ChildStdin, Command, Stdio};
    use std::sync::mpsc::{self, Receiver};
    use std::time::{Duration, Instant};

    const ALIAS: &str = "node.a";

    fn artifact(name: &str) -> PathBuf {
        PathBuf::from(std::env::var_os(name).unwrap_or_else(|| panic!("{name} is not set")))
    }

    fn clip(text: &str) -> String {
        const MAX: usize = 1500;
        if text.len() <= MAX {
            return text.to_owned();
        }
        let mut end = MAX;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        format!("{} ...(+{} bytes)", &text[..end], text.len() - end)
    }

    /// The person's environment and an install root with the exact-SHA tools.
    struct Env {
        config: PathBuf,
        runtime_root: PathBuf,
        local_app_data: PathBuf,
        install: PathBuf,
    }

    impl Env {
        fn new(root: &Path, runtime_root: &Path) -> Self {
            let config = root.join("actinglab.json");
            fs::write(&config, "{}").expect("config");
            let local_app_data = root.join("local-app-data");
            fs::create_dir_all(&local_app_data).expect("local app data");
            let install = root.join("install");
            fs::create_dir_all(install.join("tools")).expect("tools");
            fs::create_dir_all(install.join("runtime")).expect("runtime");
            fs::copy(artifact("ONEOFF_ACTINGLAB"), install.join("tools").join("actinglab.exe"))
                .expect("actinglab");
            fs::copy(
                artifact("ONEOFF_ACTINGD"),
                install.join("runtime").join("actingcommand-actingd.exe"),
            )
            .expect("actingd");
            fs::copy(
                artifact("ONEOFF_ACTINGD_CONFIG"),
                install.join("actingd.config.json"),
            )
            .expect("actingd config");
            Self {
                config,
                runtime_root: runtime_root.to_path_buf(),
                local_app_data,
                install,
            }
        }

        fn apply(&self, command: &mut Command) {
            command
                .env("ACTINGLAB_CONFIG_PATH", &self.config)
                .env("ACTINGCOMMAND_RUNTIME_STATE_ROOT", &self.runtime_root)
                .env("LOCALAPPDATA", &self.local_app_data)
                .env_remove("ACTINGLAB_REQUIRE_SESSION_DAEMON")
                .env_remove("ACTINGLAB_SESSION_STATE_DIR");
        }

        /// `actinglab --json <args>` as a person runs it: its exit code and envelope.
        fn cli(&self, args: &[&str]) -> (Option<i32>, Value) {
            let mut command = Command::new(self.install.join("tools").join("actinglab.exe"));
            command.arg("--json").args(args);
            self.apply(&mut command);
            let output = command.output().expect("run actinglab");
            let envelope =
                serde_json::from_slice(output.stdout.trim_ascii()).unwrap_or(Value::Null);
            println!(
                "S3|CLI|actinglab --json {} -> exit {:?}: {}",
                args.join(" "),
                output.status.code(),
                clip(&envelope.to_string())
            );
            (output.status.code(), envelope)
        }
    }

    struct Server {
        label: String,
        child: Child,
        stdin: Option<ChildStdin>,
        lines: Receiver<String>,
        next_id: u64,
    }

    impl Server {
        fn spawn(label: &str, env: &Env, state_root: &Path, tiers: &str) -> Self {
            Self::spawn_at(label, env, &env.install, state_root, tiers)
        }

        fn spawn_at(label: &str, env: &Env, install: &Path, state_root: &Path, tiers: &str) -> Self {
            let mut command = Command::new(artifact("ONEOFF_ACTINGCTL"));
            command
                .arg("mcp-serve")
                .arg("--root")
                .arg(install)
                .arg("--state-root")
                .arg(state_root)
                .args(["--tier", tiers])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            env.apply(&mut command);
            let mut child = command.spawn().expect("spawn mcp-serve");
            let stdout = child.stdout.take().expect("stdout");
            let (sender, lines) = mpsc::channel();
            thread::spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { return };
                    if sender.send(line).is_err() {
                        return;
                    }
                }
            });
            let stdin = child.stdin.take();
            let mut server = Self {
                label: label.to_owned(),
                child,
                stdin,
                lines,
                next_id: 1,
            };
            let answer = server.request(json!({
                "method": "initialize",
                "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "oneoff", "version": "0"}},
            }));
            assert_eq!(answer["result"]["protocolVersion"], "2025-11-25");
            server.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
            server
        }

        fn send(&mut self, message: &Value) {
            let stdin = self.stdin.as_mut().expect("stdin");
            writeln!(stdin, "{message}").expect("write");
            stdin.flush().expect("flush");
        }

        fn request(&mut self, mut message: Value) -> Value {
            let id = self.next_id;
            self.next_id += 1;
            message["jsonrpc"] = json!("2.0");
            message["id"] = json!(id);
            self.send(&message);
            let deadline = Instant::now() + Duration::from_secs(60);
            loop {
                let line = self
                    .lines
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .expect("an answer");
                let answer: Value = serde_json::from_str(&line).expect("JSON");
                if answer.get("id") == Some(&json!(id)) {
                    return answer;
                }
            }
        }

        fn tool(&mut self, name: &str, arguments: Value) -> (Value, Duration) {
            let started = Instant::now();
            let answer = self.request(json!({
                "method": "tools/call",
                "params": {"name": name, "arguments": arguments},
            }));
            let elapsed = started.elapsed();
            let structured = answer["result"]["structuredContent"].clone();
            println!(
                "S3|MCP|{}|{name} {} -> {} ms: {}",
                self.label,
                clip(&arguments.to_string()),
                elapsed.as_millis(),
                clip(&structured.to_string())
            );
            (structured, elapsed)
        }

        fn kill(mut self) {
            self.child.kill().expect("kill mcp-serve");
            let _ = self.child.wait();
            println!("S3|MCP|{}|mcp-serve process killed", self.label);
        }

        fn close(mut self) {
            drop(self.stdin.take());
            let status = self.child.wait().expect("mcp-serve exit");
            assert!(status.success());
        }
    }

    fn differences(left: &Value, right: &Value, path: &str, out: &mut Vec<String>) {
        match (left, right) {
            (Value::Object(a), Value::Object(b)) => {
                let keys = a.keys().chain(b.keys()).collect::<BTreeSet<_>>();
                for key in keys {
                    let next = format!("{path}/{key}");
                    match (a.get(key), b.get(key)) {
                        (Some(x), Some(y)) => differences(x, y, &next, out),
                        _ => out.push(format!("{next} (on one side only)")),
                    }
                }
            }
            (Value::Array(a), Value::Array(b)) if a.len() == b.len() => {
                for (index, (x, y)) in a.iter().zip(b).enumerate() {
                    differences(x, y, &format!("{path}/{index}"), out);
                }
            }
            _ if left == right => {}
            _ => out.push(path.to_owned()),
        }
    }

    /// Prints where the CLI's data and the MCP result differ; the paths, empty when identical.
    fn parity(what: &str, cli: &Value, mcp: &Value) -> Vec<String> {
        let mut out = Vec::new();
        differences(cli, mcp, "", &mut out);
        if out.is_empty() {
            println!(
                "S3|PARITY|{what}|identical, field by field ({} bytes)",
                cli.to_string().len()
            );
        } else {
            println!("S3|PARITY|{what}|differs at {out:?}");
        }
        out
    }

    /// The CLI's error envelope against an MCP error: class by exit code, lab_error verbatim.
    fn error_parity(what: &str, cli: &(Option<i32>, Value), mcp: &Value, class: &str) {
        assert_eq!(cli.1["ok"], false, "{what}: the CLI did not fail");
        assert_eq!(mcp["ok"], false, "{what}: MCP did not fail");
        let error = &mcp["error"];
        println!(
            "S3|EXIT|{what}|exit {:?} -> class {} code {} | lab_error == CLI error: {}",
            cli.0,
            error["class"],
            error["code"],
            error["details"]["lab_error"] == cli.1["error"]
        );
        assert_eq!(error["class"], class, "{what}: class");
        assert_eq!(error["details"]["lab_exit_code"], json!(cli.0), "{what}: exit code");
        assert_eq!(error["details"]["lab_error"], cli.1["error"], "{what}: lab_error");
    }

    fn ledger(runtime_root: &Path) -> Vec<Value> {
        RuntimeClient::connect(RuntimeClientConfig::new(
            runtime_root,
            EventActor::Cli,
            EventSource::Cli,
        ))
        .expect("ledger client")
        .query_events(EventQuery::default(), ProjectionProfile::Forensic)
        .expect("events")
        .iter()
        .map(|event| serde_json::to_value(event).expect("event JSON"))
        .collect()
    }

    fn client_actions(runtime_root: &Path, control: &str) -> Vec<Value> {
        let marker = format!("\"control_id\":\"{control}\"");
        ledger(runtime_root)
            .into_iter()
            .filter(|row| row["event_type"] == "client.action")
            .filter(|row| row["payload"].to_string().contains(&marker))
            .collect()
    }

    /// The client.action of `control` carries `req_id` and shares only that value with the
    /// Lab request's correlation.
    fn provenance(runtime_root: &Path, control: &str, req_id: &str, before: usize) {
        let actions = client_actions(runtime_root, control);
        assert_eq!(actions.len(), before + 1, "one new client.action of {control}");
        let action = actions.last().expect("client.action");
        let action_correlation = action["links"]["correlation_id"].clone();
        let payload = action["payload"].to_string();
        println!("S3|PROVENANCE|{control}|client.action links {} payload {}", action["links"], clip(&payload));
        assert!(
            payload.contains(&format!("\"value\":{{\"type\":\"path_safe_string\",\"value\":\"{req_id}\"}}")),
            "the client.action carries the req_id"
        );
        assert!(payload.contains("\"surface_id\":\"mcp\""));
        let rows = ledger(runtime_root);
        let in_action = rows
            .iter()
            .filter(|row| row["links"]["correlation_id"] == action_correlation)
            .map(|row| row["event_type"].as_str().unwrap_or_default().to_owned())
            .collect::<Vec<_>>();
        let lab_correlations = rows
            .iter()
            .filter(|row| {
                row["links"]["request_id"] == json!(req_id)
                    || row["links"]["correlation_id"] == json!(req_id)
            })
            .map(|row| row["links"]["correlation_id"].clone())
            .collect::<BTreeSet<_>>();
        let lab_types = rows
            .iter()
            .filter(|row| lab_correlations.contains(&row["links"]["correlation_id"]))
            .map(|row| row["event_type"].as_str().unwrap_or_default().to_owned())
            .collect::<Vec<_>>();
        println!(
            "S3|PROVENANCE|{control}|req_id {req_id}; client.action correlation {action_correlation} holds {in_action:?}; Lab correlation(s) {lab_correlations:?} hold {lab_types:?}"
        );
        assert!(!lab_correlations.is_empty(), "the Lab request is in the ledger");
        assert!(!lab_correlations.contains(&action_correlation), "no shared correlation");
        assert!(lab_types.iter().any(|kind| kind == "lab.request"), "lab.request written");
        assert!(!lab_types.iter().any(|kind| kind == "client.action"), "no client.action in the Lab correlation");
        assert!(in_action.iter().all(|kind| kind == "client.action" || kind == "governance.identity_declared"));
    }

    fn write_frame(path: &Path, color: [u8; 3]) {
        let frame = Frame::from_pixels(
            1,
            1,
            color.to_vec(),
            PixelFormat::Rgb8,
            CaptureBackendName::AdbScreencap,
        )
        .expect("frame");
        fs::write(path, frame.encode_png_fast().expect("png")).expect("write frame");
    }

    /// A neutral contained task as a content directory (as the S2 one-off package).
    fn write_task_directory(root: &Path) {
        let files: &[(&str, &str)] = &[
            ("control.json", r#"{"schema_version":"Lab-1y.control.v1","package_id":"neutral.semantic.task","execution_mode":"navigable_route","game":"neutral","server":"test","resolution":{"width":16,"height":9},"entry_task_id":"task","capture_interval_ms":1,"step_timeout_ms":50,"timeout_ms":30000,"max_steps":2}"#),
            ("resources/manifest.json", r#"{"schema_version":"0.3","entry_task_id":"task"}"#),
            ("resources/operations/task/task.json", r#"{"schema_version":"0.6","task_id":"task","game":"neutral","server_scope":["test"],"coordinate_space":{"width":16,"height":9},"entry_page":"home","target_page":"terminal","operations":[{"id":"open_terminal","from":"home","to":"terminal","click":{"kind":"point","x":1,"y":0},"unguarded_trusted_coordinate":true}]}"#),
            ("resources/recognition/neutral.test.pack.json", r#"{"schema_version":"0.3","game":"neutral","server":"test","coordinate_space":{"width":16,"height":9},"defaults":{"color_max_distance":0.0},"targets":[{"type":"color","id":"page/home","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]},{"type":"color","id":"page/terminal","region":{"x":0,"y":0,"width":1,"height":1},"expected":[0,0,255]}]}"#),
            ("resources/recognition/neutral.test.pages.json", r#"{"schema_version":"0.3","pages":[{"id":"neutral/home","required":["page/home"],"optional":[],"forbidden":[]},{"id":"neutral/terminal","required":["page/terminal"],"optional":[],"forbidden":[]}]}"#),
        ];
        for (path, contents) in files {
            let path = root.join(path);
            fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            fs::write(path, contents).expect("file");
        }
    }

    /// Every file under `root`: path, size and modification time.
    fn listing(root: &Path) -> Vec<String> {
        fn walk(dir: &Path, out: &mut Vec<String>) {
            let Ok(entries) = fs::read_dir(dir) else { return };
            for entry in entries.flatten() {
                let path = entry.path();
                let meta = entry.metadata().expect("metadata");
                if meta.is_dir() {
                    out.push(format!("{}/", path.display()));
                    walk(&path, out);
                } else {
                    out.push(format!(
                        "{} {} {:?}",
                        path.display(),
                        meta.len(),
                        meta.modified().ok()
                    ));
                }
            }
        }
        let mut out = Vec::new();
        walk(root, &mut out);
        out.sort();
        out
    }

    /// The fixture Runtime (FakeProvider "node.a") and its Lab paths: parity, provenance,
    /// recording, the binding draft, a long call and a dead handle.
    #[test]
    fn oneoff_338s3_lab_with_runtime() {
        let root = TempDir::new().expect("tempdir");
        let runtime_root = root.path().join("runtime");
        let resources = root.path().join("resources");
        let semantic_package = root.path().join("semantic.zip");
        write_navigation_resources(&resources);
        let pack = fs::read_to_string(resources.join("recognition/arknights.cn.pack.json"))
            .unwrap()
            .replace("[0,0,255]", "[0,255,0]");
        let pages = fs::read(resources.join("recognition/arknights.cn.pages.json")).unwrap();
        let navigation =
            fs::read(resources.join("navigation/arknights.cn.navigation.json")).unwrap();
        write_zip(&semantic_package, &[
            ("control.json", br#"{"game":"arknights","server":"cn","entry_task_id":"task"}"#),
            ("resources/manifest.json", br#"{"schema_version":"0.3","entry_task_id":"task"}"#),
            ("resources/operations/task/task.json", br#"{"task_id":"task","post_admission_ocr":{"mode":"fields_v1","fields":[{"id":"name","target_id":"home_anchor","privacy":"personal"}]}}"#),
            ("resources/recognition/arknights.cn.pack.json", pack.as_bytes()),
            ("resources/recognition/arknights.cn.pages.json", &pages),
            ("resources/navigation/arknights.cn.navigation.json", &navigation),
        ]);
        let zip = semantic_package.to_str().expect("utf-8").to_owned();
        let sha = format!(
            "{:x}",
            Sha256::digest(fs::read(&semantic_package).expect("semantic package"))
        );
        let state = Arc::new(FakeState::default());
        state.transition_after_tap.store(true, Ordering::Release);
        let instance_id = *IdentifierIssuer::new()
            .expect("identifier issuer")
            .mint_instance_id()
            .expect("instance id")
            .transport();
        let _host = RuntimeHost::start(
            RuntimeHostConfig::new(&runtime_root, b"actinglab-runtime-lab2-test"),
            Arc::new(FakeProvider {
                instance_alias: ALIAS,
                instance_id,
                state: Arc::clone(&state),
                frame_size: 1,
            }),
        )
        .expect("runtime host");
        let env = Env::new(root.path(), &runtime_root);

        // The tier gate, the table and the `--` refusal.
        let mut observer = Server::spawn("observer", &env, &runtime_root, "observer");
        let (refused, _) = observer.tool("ac_record_status", json!({"instance": ALIAS}));
        assert_eq!(refused["error"]["code"], "tier_not_enabled");
        observer.close();
        let mut server = Server::spawn("author", &env, &runtime_root, "operator,author");
        let listed = server.request(json!({"method": "tools/list", "params": {}}));
        let tools = listed["result"]["tools"].as_array().expect("tools").clone();
        let names = tools.iter().map(|tool| tool["name"].as_str().unwrap_or_default().to_owned()).collect::<Vec<_>>();
        println!("S3|TABLE|tools/list with operator,author: {} tools {names:?}", names.len());
        assert_eq!(names.len(), 22);
        let mut patterned = 0;
        for tool in &tools {
            let name = tool["name"].as_str().unwrap_or_default();
            if !(name.starts_with("ac_lab_") || name.starts_with("ac_record_") || name == "ac_pack_check" || name == "ac_catalog_check") {
                continue;
            }
            assert_eq!(tool["inputSchema"]["additionalProperties"], false, "{name}");
            for (property, schema) in tool["inputSchema"]["properties"].as_object().expect("properties") {
                if schema["type"] == "string" {
                    assert_eq!(schema["pattern"], "^([^-]|-([^-]|$))", "{name}.{property}");
                    patterned += 1;
                }
            }
        }
        println!("S3|DASHES|schema: every string property of the Lab tools ({patterned}) has pattern ^([^-]|-([^-]|$)) and additionalProperties false");
        for (tool, arguments) in [
            ("ac_record_start", json!({"instance": ALIAS, "task_id": "--force"})),
            ("ac_lab_observe", json!({"scene": "--capture", "zip": zip, "expected_sha256": sha})),
            ("ac_catalog_check", json!({"repo": "--x", "catalog": "c.json", "server": "test"})),
            ("ac_pack_check", json!({"package": "--package-ref"})),
        ] {
            let (answer, _) = server.tool(tool, arguments);
            assert_eq!(answer["error"]["code"], "arguments_invalid", "{tool}");
            println!("S3|DASHES|server: {tool} -> {} {}", answer["error"]["code"], answer["error"]["message"]);
        }

        // Runtime-backed observe: CLI and MCP, then the provenance of the MCP call.
        let frame = root.path().join("frame.png");
        let frame_text = frame.to_str().expect("utf-8").to_owned();
        let (code, _) = env.cli(&["observe", "--instance", ALIAS, "--zip", &zip, "--expected-sha256", &sha, "--with-frame", &frame_text, "--capture"]);
        assert_eq!(code, Some(0));
        let observe_args = ["observe", "--instance", ALIAS, "--zip", &zip, "--expected-sha256", &sha, "--capture", "--verbose"];
        let (code, cli_observe) = env.cli(&observe_args);
        assert_eq!(code, Some(0));
        let before = client_actions(&runtime_root, "ac_lab_observe").len();
        let (observed, _) = server.tool(
            "ac_lab_observe",
            json!({"instance": ALIAS, "capture": true, "zip": zip, "expected_sha256": sha, "verbose": true}),
        );
        assert_eq!(observed["ok"], true);
        parity("observe --capture (Runtime-backed, two requests)", &cli_observe["data"], &observed["result"]);
        let req_id = observed["result"]["req_id"].as_str().expect("req_id").to_owned();
        println!("S3|PROVENANCE|warnings of the call: {}", observed["warnings"]);
        provenance(&runtime_root, "ac_lab_observe", &req_id, before);

        // Runtime-backed do on the element the MCP observation offered.
        let element = observed["result"]["observation"]["elements"][0]["id"].as_str().expect("element").to_owned();
        let sequence = observed["result"]["projection_source"]["projection_sequence"].as_u64().expect("sequence");
        let hash = observed["result"]["projection_source"]["content_sha256"].as_str().expect("hash").to_owned();
        let before = client_actions(&runtime_root, "ac_lab_do").len();
        let (acted, _) = server.tool(
            "ac_lab_do",
            json!({"instance": ALIAS, "target": element, "capture": true, "zip": zip, "expected_sha256": sha, "projection_sequence": sequence, "projection_hash": hash, "verbose": true}),
        );
        if acted["ok"] == true {
            let req_id = acted["result"]["req_id"].as_str().expect("req_id").to_owned();
            println!("S3|DO|executed {} effect {}", acted["result"]["executed"], acted["result"]["effect"]);
            provenance(&runtime_root, "ac_lab_do", &req_id, before);
        } else {
            println!("S3|DO|refused: {}", clip(&acted["error"].to_string()));
            println!("S3|DO|client.action count {} -> {}", before, client_actions(&runtime_root, "ac_lab_do").len());
        }

        // Offline observe on the captured frame: parity, and no client.action.
        let all_actions = |root: &Path| ledger(root).iter().filter(|row| row["event_type"] == "client.action").count();
        let actions_before = all_actions(&runtime_root);
        let offline_args = ["observe", "--instance", ALIAS, "--zip", &zip, "--expected-sha256", &sha, "--scene", &frame_text];
        let (code, cli_offline) = env.cli(&offline_args);
        assert_eq!(code, Some(0));
        let (offline, _) = server.tool(
            "ac_lab_observe",
            json!({"instance": ALIAS, "scene": frame_text, "zip": zip, "expected_sha256": sha}),
        );
        assert_eq!(offline["ok"], true);
        parity("observe --scene (offline)", &cli_offline["data"], &offline["result"]);
        assert_eq!(all_actions(&runtime_root), actions_before, "offline observe records no client.action");
        println!("S3|PROVENANCE|offline observe: client.action events {actions_before} -> {}", all_actions(&runtime_root));

        // Recording: start, status, refusals, mark, stop, binding draft; none records a client.action.
        let state_dir = root.path().join("record-state");
        let state_text = state_dir.to_str().expect("utf-8").to_owned();
        let record_frame = root.path().join("record-frame.png");
        write_frame(&record_frame, [255, 0, 0]);
        let (started, _) = server.tool(
            "ac_record_start",
            json!({"instance": ALIAS, "task_id": "neutral_task", "locale": "en", "record_id": "oneoff338", "state_dir": state_text}),
        );
        assert_eq!(started["ok"], true);
        let (code, cli_status) = env.cli(&["record", "status", "--instance", ALIAS, "--state-dir", &state_text]);
        assert_eq!(code, Some(0));
        let (status, _) = server.tool("ac_record_status", json!({"instance": ALIAS, "state_dir": state_text}));
        assert!(parity("record status", &cli_status["data"], &status["result"]).is_empty());
        let cli_again = env.cli(&["record", "start", "--instance", ALIAS, "--task-id", "neutral_task", "--state-dir", &state_text]);
        let (again, _) = server.tool("ac_record_start", json!({"instance": ALIAS, "task_id": "neutral_task", "state_dir": state_text}));
        error_parity("record start on an active recording (3)", &cli_again, &again, "safety");
        let bad = json!({"schema_version": "actingcommand.lab-record-mark.v1", "colour": 1});
        let cli_bad = env.cli(&["record", "mark", "--instance", ALIAS, "--request-json", &bad.to_string(), "--state-dir", &state_text]);
        let (bad_mark, _) = server.tool("ac_record_mark", json!({"instance": ALIAS, "request": bad, "state_dir": state_text}));
        error_parity("record mark with an unknown field (2)", &cli_bad, &bad_mark, "usage");
        let mark = json!({
            "schema_version": "actingcommand.lab-record-mark.v1",
            "frame": record_frame.to_str().expect("utf-8"),
            "page": "home",
            "add": [{"id": "state/home", "family": "color", "region": {"x": 0, "y": 0, "width": 1, "height": 1}}],
        });
        let (_, cli_mark) = env.cli(&["record", "mark", "--instance", ALIAS, "--request-json", &mark.to_string(), "--state-dir", &state_text, "--dry-run"]);
        let (dry_mark, _) = server.tool("ac_record_mark", json!({"instance": ALIAS, "request": mark, "state_dir": state_text, "dry_run": true}));
        if cli_mark["ok"] == true {
            parity("record mark --dry-run", &cli_mark["data"], &dry_mark["result"]);
        } else {
            println!("S3|PARITY|record mark --dry-run|both refused: lab_error == CLI error: {}", dry_mark["error"]["details"]["lab_error"] == cli_mark["error"]);
        }
        let (marked, _) = server.tool("ac_record_mark", json!({"instance": ALIAS, "request": mark, "state_dir": state_text}));
        println!("S3|RECORD|mark ok {}", marked["ok"]);
        let lab_dir = root.path().join("lab-out");
        let lab_dir_text = lab_dir.to_str().expect("utf-8").to_owned();
        let (_, cli_stop) = env.cli(&["record", "stop", "--instance", ALIAS, "--game", "neutral", "--server", "test", "--lab-dir", &lab_dir_text, "--state-dir", &state_text, "--dry-run"]);
        let (dry_stop, _) = server.tool("ac_record_stop", json!({"instance": ALIAS, "game": "neutral", "server": "test", "lab_dir": lab_dir_text, "state_dir": state_text, "dry_run": true}));
        if cli_stop["ok"] == true {
            parity("record stop --dry-run", &cli_stop["data"], &dry_stop["result"]);
        } else {
            println!("S3|PARITY|record stop --dry-run|both refused: lab_error == CLI error: {}", dry_stop["error"]["details"]["lab_error"] == cli_stop["error"]);
        }
        let (stopped, _) = server.tool("ac_record_stop", json!({"instance": ALIAS, "game": "neutral", "server": "test", "lab_dir": lab_dir_text, "state_dir": state_text}));
        println!("S3|RECORD|stop ok {} lab.status {} lab.path {}", stopped["ok"], stopped["result"]["lab"]["status"], stopped["result"]["lab"]["path"]);
        let (code, cli_status) = env.cli(&["record", "status", "--instance", ALIAS, "--state-dir", &state_text]);
        assert_eq!(code, Some(0));
        let (status, _) = server.tool("ac_record_status", json!({"instance": ALIAS, "state_dir": state_text}));
        assert!(parity("record status after stop", &cli_status["data"], &status["result"]).is_empty());
        if stopped["ok"] == true && stopped["result"]["lab"].is_object() {
            let (draft, _) = server.tool("ac_binding_draft", json!({"record_stop": stopped["result"]}));
            let result = &draft["result"];
            println!("S3|DRAFT|binding_example verbatim: {}", result["binding_example"] == stopped["result"]["lab"]["binding_example"]);
            println!("S3|DRAFT|binding_requires verbatim: {}", result["binding_requires"] == stopped["result"]["lab"]["binding_requires"]);
            println!("S3|DRAFT|prerequisite_entry_example verbatim: {}", result["prerequisite_entry_example"] == stopped["result"]["lab"]["prerequisite_entry_example"]);
            println!("S3|DRAFT|catalog_on_failure_example verbatim: {}", result["catalog_on_failure_example"] == stopped["result"]["lab"]["catalog_on_failure_example"]);
            println!("S3|DRAFT|admission: {}", clip(&result["admission"].to_string()));
            println!("S3|DRAFT|check_config: {}", clip(&result["check_config"].to_string()));
            println!("S3|DRAFT|manual_steps: {}", clip(&result["manual_steps"].to_string()));
            println!("S3|DRAFT|warnings: {}", draft["warnings"]);
            let package = stopped["result"]["lab"]["binding_example"]["scheduled_execution"]["package_path"].as_str().unwrap_or_default().to_owned();
            let reference = stopped["result"]["lab"]["package_ref"].to_string();
            let (_, cli_preflight) = env.cli(&["package", "preflight", "--package", &package, "--package-ref", &reference]);
            let side = if cli_preflight["ok"] == true { &result["admission"]["preflight"] } else { &result["admission"]["preflight_error"]["details"]["lab_error"] };
            let other = if cli_preflight["ok"] == true { &cli_preflight["data"] } else { &cli_preflight["error"] };
            parity("binding draft admission = package preflight", other, side);
        }
        let record_actions = ["ac_record_start", "ac_record_mark", "ac_record_stop", "ac_record_status", "ac_binding_draft"]
            .iter()
            .map(|control| (control.to_string(), client_actions(&runtime_root, control).len()))
            .collect::<Vec<_>>();
        println!("S3|PROVENANCE|client.action events of the recording tools: {record_actions:?}");
        assert!(record_actions.iter().all(|(_, count)| *count == 0));

        // A long call: the offline observe sleeps 30 s in actinglab (test delay) and becomes a job.
        let (long, elapsed) = server.tool(
            "ac_lab_observe",
            json!({"instance": ALIAS, "scene": frame_text, "zip": zip, "expected_sha256": sha, "test_capture_delay_ms": 30000}),
        );
        let handle = long["result"]["handle"].as_str().expect("a handle").to_owned();
        println!("S3|LONG|answered in {} ms with handle {handle} job_phase {}", elapsed.as_millis(), long["result"]["job_phase"]);
        assert!(elapsed < Duration::from_secs(25));
        assert!(handle.starts_with("lab_job_"));
        let deadline = Instant::now() + Duration::from_secs(90);
        let finished = loop {
            let (job, _) = server.tool("ac_get_run", json!({"handle": handle, "wait_s": 20}));
            if job["result"]["job"]["phase"] != "running" {
                break job;
            }
            assert!(Instant::now() < deadline, "the long job did not end");
        };
        println!("S3|LONG|job ended: phase {} outcome.state {} backend {}", finished["result"]["job"]["phase"], finished["result"]["job"]["outcome"]["state"], finished["result"]["job"]["outcome"]["backend"]);
        assert_eq!(finished["result"]["job"]["phase"], "done");
        parity("observe --scene as a long job", &cli_offline["data"], &finished["result"]["job"]["outcome"]);
        server.kill();
        let mut fresh = Server::spawn("fresh", &env, &runtime_root, "author");
        let (unknown, _) = fresh.tool("ac_get_run", json!({"handle": handle}));
        println!("S3|LONG|handle from the ended process -> {} {}", unknown["error"]["code"], unknown["error"]["message"]);
        assert_eq!(unknown["error"]["code"], "handle_unknown");
        fresh.close();
    }

    /// No Runtime: the offline checks, exit codes 4 and 5, a broken install, and the catalog
    /// check writing nothing.
    #[test]
    fn oneoff_338s3_offline_checks() {
        let root = TempDir::new().expect("tempdir");
        let runtime_root = root.path().join("no-runtime");
        fs::create_dir_all(&runtime_root).expect("empty state root");
        let env = Env::new(root.path(), &runtime_root);
        let mut server = Server::spawn("offline", &env, &runtime_root, "author");

        // ac_catalog_check: compile result verbatim, and nothing written.
        let repo = root.path().join("repo");
        fs::create_dir_all(&repo).expect("repo");
        let catalog = json!({
            "schema_version":"actingcommand.business-catalog.v1","catalog_id":"synthetic","pools":{"material":10},
            "recognition":{"kind":"ocr_aliases","max_distance":1,"minimum_margin":1,"minimum_confidence_milli":800},
            "entries":[{"id":"first","names":{"test":["Synthetic text"]},"duration_seconds":60,
                "source":{"uri":"https://example.invalid/source","date":"2026-10-03"},
                "rewards":[{"pool_id":"material","quantity_milli":1000,"probability_milli":400,"batches":1,"confidence_milli":700,"observation_source":"self_reported","observed_amount":2}]}]
        });
        fs::write(repo.join("catalog.json"), serde_json::to_vec(&catalog).unwrap()).expect("catalog");
        let repo_text = repo.to_str().expect("utf-8").to_owned();
        let (code, cli_catalog) = env.cli(&["resource", "catalog", "--repo", &repo_text, "--catalog", "catalog.json", "--catalog-server", "test"]);
        assert_eq!(code, Some(0));
        let before = listing(root.path());
        let (compiled, _) = server.tool("ac_catalog_check", json!({"repo": repo_text, "catalog": "catalog.json", "server": "test"}));
        let (refused, _) = server.tool("ac_catalog_check", json!({"repo": repo_text, "catalog": "missing.json", "server": "test"}));
        let after = listing(root.path());
        assert!(parity("resource catalog", &cli_catalog["data"], &compiled["result"]).is_empty());
        let cli_refused = env.cli(&["resource", "catalog", "--repo", &repo_text, "--catalog", "missing.json", "--catalog-server", "test"]);
        error_parity("resource catalog of a missing file", &cli_refused, &refused, "usage");
        println!("S3|CATALOG|files under the test root before {} after {} identical {}", before.len(), after.len(), before == after);
        for line in before.iter().take(40) {
            println!("S3|CATALOG|listing {line}");
        }
        assert_eq!(before, after, "ac_catalog_check wrote nothing");

        // ac_pack_check: digest and preflight verbatim; a given reference checked.
        let task = root.path().join("neutral-task");
        write_task_directory(&task);
        let task_text = task.to_str().expect("utf-8").to_owned();
        let (code, cli_digest) = env.cli(&["package", "digest", "--package", &task_text]);
        assert_eq!(code, Some(0));
        let reference = cli_digest["data"]["reference"].to_string();
        let cli_preflight = env.cli(&["package", "preflight", "--package", &task_text, "--package-ref", &reference]);
        let (checked, _) = server.tool("ac_pack_check", json!({"package": task_text}));
        if checked["ok"] == true {
            assert!(parity("package digest", &cli_digest["data"], &checked["result"]["digest"]).is_empty());
            assert!(parity("package preflight", &cli_preflight.1["data"], &checked["result"]["preflight"]).is_empty());
        } else {
            error_parity("package preflight", &cli_preflight, &checked, checked["error"]["class"].as_str().unwrap_or_default());
            assert!(parity("package digest (kept in details)", &cli_digest["data"], &checked["error"]["details"]["digest"]).is_empty());
        }
        let (given, _) = server.tool("ac_pack_check", json!({"package": task_text, "package_ref": reference}));
        println!("S3|PACK|given digest reference -> package_ref_check {}", given["result"]["package_ref_check"]);
        let wrong = "0".repeat(64);
        let cli_wrong = env.cli(&["package", "preflight", "--package", &task_text, "--package-ref", &wrong]);
        let (mismatch, _) = server.tool("ac_pack_check", json!({"package": task_text, "package_ref": wrong}));
        println!("S3|PACK|wrong reference -> ok {} package_ref_check {}", mismatch["ok"], if mismatch["ok"] == true { mismatch["result"]["package_ref_check"].clone() } else { mismatch["error"]["details"]["package_ref_check"].clone() });
        if mismatch["ok"] == false {
            error_parity("package preflight with a reference that is not the package's", &cli_wrong, &mismatch, mismatch["error"]["class"].as_str().unwrap_or_default());
        }

        // Exit 4: the Runtime is not running for a Runtime-backed observe.
        let zip = root.path().join("semantic.zip");
        let resources = root.path().join("resources");
        write_navigation_resources(&resources);
        write_semantic_package(&zip, &resources);
        let zip_text = zip.to_str().expect("utf-8").to_owned();
        let sha = format!("{:x}", Sha256::digest(fs::read(&zip).expect("zip")));
        let cli_down = env.cli(&["observe", "--instance", ALIAS, "--zip", &zip_text, "--expected-sha256", &sha, "--capture"]);
        let (down, _) = server.tool("ac_lab_observe", json!({"instance": ALIAS, "capture": true, "zip": zip_text, "expected_sha256": sha}));
        error_parity("observe --capture without a running Runtime (4)", &cli_down, &down, "device");
        println!("S3|EXIT|its client.action attempt: {}", clip(&down["error"]["details"]["warnings"].to_string()));

        // Exit 5: the recording state directory cannot be created (below a file).
        let file = root.path().join("a-file");
        fs::write(&file, b"x").expect("file");
        let blocked = file.join("state");
        let blocked_text = blocked.to_str().expect("utf-8").to_owned();
        let cli_blocked = env.cli(&["record", "status", "--instance", ALIAS, "--state-dir", &blocked_text]);
        let (unwritable, _) = server.tool("ac_record_status", json!({"instance": ALIAS, "state_dir": blocked_text}));
        error_parity("record status with a state directory below a file (5)", &cli_blocked, &unwritable, "runtime");

        // Exit 2 on a Lab-2 verb: observe without --scene or --capture.
        let cli_usage = env.cli(&["observe", "--instance", ALIAS, "--zip", &zip_text, "--expected-sha256", &sha]);
        let (usage, _) = server.tool("ac_lab_observe", json!({"instance": ALIAS, "zip": zip_text, "expected_sha256": sha}));
        error_parity("observe without scene or capture (2)", &cli_usage, &usage, "usage");
        server.close();

        // A broken install: tools\actinglab.exe is another program, so no envelope comes back.
        let broken = root.path().join("broken");
        fs::create_dir_all(broken.join("tools")).expect("broken tools");
        fs::copy(artifact("ONEOFF_ACTINGCTL"), broken.join("tools").join("actinglab.exe")).expect("substitute");
        let mut broken_server = Server::spawn_at("broken", &env, &broken, &runtime_root, "author");
        let (failed, _) = broken_server.tool("ac_record_status", json!({"instance": ALIAS}));
        println!("S3|EXIT|no envelope -> class {} code {} lab_exit_code {} stderr_tail {}", failed["error"]["class"], failed["error"]["code"], failed["error"]["details"]["lab_exit_code"], clip(&failed["error"]["details"]["stderr_tail"].to_string()));
        assert_eq!(failed["error"]["code"], "lab_process_failed");
        assert_eq!(failed["error"]["class"], "runtime");
        broken_server.close();
    }
}
