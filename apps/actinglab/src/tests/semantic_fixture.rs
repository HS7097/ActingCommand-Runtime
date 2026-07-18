// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

pub(super) struct SemanticFixture {
    temp: TempDir,
    pub(super) zip: PathBuf,
    expected_sha256: String,
}

impl SemanticFixture {
    pub(super) fn path(&self) -> &Path {
        self.temp.path()
    }
}

pub(super) fn run_semantic_cli<I>(
    fixture: &SemanticFixture,
    args: I,
    json_default: bool,
) -> CliResult
where
    I: IntoIterator,
    I::Item: Into<String>,
{
    let mut args = args.into_iter().map(Into::into).collect::<Vec<_>>();
    let command = parse_invocation(args.clone(), json_default)
        .expect("semantic test invocation")
        .command_name;
    if matches!(
        command.as_str(),
        "recognize"
            | "detect-page"
            | "current-page"
            | "is-visible"
            | "tap-target"
            | "navigate"
            | "observe"
            | "do"
            | "ensure"
            | "wait"
    ) {
        args.extend([
            "--zip".to_string(),
            fixture.zip.display().to_string(),
            "--expected-sha256".to_string(),
            fixture.expected_sha256.clone(),
        ]);
    }
    run_cli(args, json_default)
}

pub(super) fn seal_semantic_fixture(
    temp: TempDir,
    game: &str,
    server: &str,
    pack_source: &Path,
    pages_source: &Path,
    navigation_source: Option<&Path>,
) -> SemanticFixture {
    let zip_path = temp.path().join("semantic.bundle.zip");
    let source_paths = [Some(pack_source), Some(pages_source), navigation_source]
        .into_iter()
        .flatten()
        .map(Path::to_path_buf)
        .collect::<BTreeSet<_>>();
    let file = File::create(&zip_path).unwrap();
    let mut zip = ZipWriter::new(file);
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let control = format!(r#"{{"game":"{game}","server":"{server}","entry_task_id":"task"}}"#);
    for (name, bytes) in [
        ("control.json", control.as_bytes()),
        (
            "resources/manifest.json",
            br#"{"schema_version":"0.3","entry_task_id":"task"}"#.as_slice(),
        ),
        ("resources/operations/task/task.json", br#"{}"#.as_slice()),
    ] {
        zip.start_file(name, options).unwrap();
        zip.write_all(bytes).unwrap();
    }
    for (source, destination) in [
        (
            pack_source,
            format!("resources/recognition/{game}.{server}.pack.json"),
        ),
        (
            pages_source,
            format!("resources/recognition/{game}.{server}.pages.json"),
        ),
    ] {
        zip.start_file(destination, options).unwrap();
        zip.write_all(&fs::read(source).unwrap()).unwrap();
    }
    if let Some(source) = navigation_source {
        zip.start_file(
            format!("resources/navigation/{game}.{server}.navigation.json"),
            options,
        )
        .unwrap();
        zip.write_all(&fs::read(source).unwrap()).unwrap();
    }
    for source in find_files(temp.path(), |_| true).unwrap() {
        if source_paths.contains(&source) || source == zip_path {
            continue;
        }
        let relative = source.strip_prefix(temp.path()).unwrap();
        let relative = relative.to_string_lossy().replace('\\', "/");
        zip.start_file(format!("resources/{relative}"), options)
            .unwrap();
        zip.write_all(&fs::read(source).unwrap()).unwrap();
    }
    zip.finish().unwrap();
    let expected_sha256 = format!("{:x}", Sha256::digest(fs::read(&zip_path).unwrap()));
    SemanticFixture {
        temp,
        zip: zip_path,
        expected_sha256,
    }
}

pub(super) fn semantic_resource_root(include_destructive_overlap: bool) -> SemanticFixture {
    let temp = TempDir::new().unwrap();
    let recognition = temp.path().join("recognition");
    let navigation = temp.path().join("navigation");
    fs::create_dir(&recognition).unwrap();
    fs::create_dir(&navigation).unwrap();
    fs::write(
        recognition.join("sample.local.pack.json"),
        r#"{
            "schema_version":"0.3",
            "coordinate_space":{"width":1,"height":1},
            "targets":[
                {"type":"color","id":"home_anchor","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]},
                {"type":"color","id":"target_anchor","region":{"x":0,"y":0,"width":1,"height":1},"expected":[0,0,255]},
                {"type":"color","id":"home_button","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0],"click":{"x":10,"y":20,"width":4,"height":6}}
            ]
        }"#,
    )
    .unwrap();
    fs::write(
        recognition.join("sample.local.pages.json"),
        r#"{
            "schema_version":"0.3",
            "pages":[
                {"id":"sample/home","required":["home_anchor"]},
                {"id":"sample/target","required":["target_anchor"]}
            ]
        }"#,
    )
    .unwrap();
    let destructive = if include_destructive_overlap {
        r#"[{"id":"delete","click":{"kind":"rect","x":10,"y":20,"width":4,"height":6}}]"#
    } else {
        "[]"
    };
    fs::write(
        navigation.join("sample.local.navigation.json"),
        format!(
            r#"{{
                "schema_version":"0.3",
                "game":"sample",
                "server":"local",
                "control_points":[{{"name":"wake","point":[3,4],"note":"test wake"}}],
                "navigation":[{{
                    "id":"home_to_target",
                    "from_page":"sample/home",
                    "to_page":"sample/target",
                    "effect":"navigation_only",
                    "click":{{"kind":"rect","x":10,"y":20,"width":4,"height":6}}
                }},
                {{
                    "id":"target_to_home",
                    "from_page":"sample/target",
                    "to_page":"sample/home",
                    "effect":"navigation_only",
                    "click":{{"kind":"point","point":"2,3"}}
                }}],
                "destructive_actions":{destructive}
            }}"#
        ),
    )
    .unwrap();
    let pack = recognition.join("sample.local.pack.json");
    let pages = recognition.join("sample.local.pages.json");
    let graph = navigation.join("sample.local.navigation.json");
    seal_semantic_fixture(temp, "sample", "local", &pack, &pages, Some(&graph))
}

pub(super) fn template_drift_resource_root() -> SemanticFixture {
    let temp = TempDir::new().unwrap();
    let recognition = temp.path().join("recognition");
    let navigation = temp.path().join("navigation");
    fs::create_dir(&recognition).unwrap();
    fs::create_dir(&navigation).unwrap();
    fs::write(
        recognition.join("home-button.png"),
        encode_png(1, 1, [255, 0, 0]),
    )
    .unwrap();
    fs::write(
        recognition.join("sample.local.pack.json"),
        r#"{
            "schema_version":"0.3",
            "coordinate_space":{"width":3,"height":1},
            "targets":[
                {
                    "type":"template",
                    "id":"home_button",
                    "template_path":"recognition/home-button.png",
                    "region":{"x":0,"y":0,"width":3,"height":1},
                    "threshold":0.9,
                    "click":{"x":0,"y":0,"width":1,"height":1}
                }
            ]
        }"#,
    )
    .unwrap();
    fs::write(
        recognition.join("sample.local.pages.json"),
        r#"{
            "schema_version":"0.3",
            "pages":[{"id":"sample/home","required":["home_button"]}]
        }"#,
    )
    .unwrap();
    fs::write(
        navigation.join("sample.local.navigation.json"),
        r#"{
            "schema_version":"0.3",
            "game":"sample",
            "server":"local",
            "navigation":[],
            "destructive_actions":[]
        }"#,
    )
    .unwrap();
    let pack = recognition.join("sample.local.pack.json");
    let pages = recognition.join("sample.local.pages.json");
    let graph = navigation.join("sample.local.navigation.json");
    seal_semantic_fixture(temp, "sample", "local", &pack, &pages, Some(&graph))
}

pub(super) fn synthetic_game_resource_root() -> SemanticFixture {
    let temp = TempDir::new().unwrap();
    fs::create_dir(temp.path().join("recognition")).unwrap();
    fs::create_dir(temp.path().join("navigation")).unwrap();
    fs::write(
        temp.path().join("synthetic.pack.json"),
        r#"{
            "schema_version":"0.3",
            "coordinate_space":{"width":1,"height":1},
            "targets":[
                {"type":"color","id":"synthetic_home_anchor","region":{"x":0,"y":0,"width":1,"height":1},"expected":[10,20,30]},
                {"type":"color","id":"synthetic_target_anchor","region":{"x":0,"y":0,"width":1,"height":1},"expected":[30,20,10]},
                {"type":"color","id":"synthetic_button","region":{"x":0,"y":0,"width":1,"height":1},"expected":[10,20,30],"click":{"x":1,"y":2,"width":3,"height":4}}
            ]
        }"#,
    )
    .unwrap();
    fs::write(
        temp.path().join("synthetic.pages.json"),
        r#"{
            "schema_version":"0.3",
            "pages":[
                {"id":"synthetic/home","required":["synthetic_home_anchor"]},
                {"id":"synthetic/target","required":["synthetic_target_anchor"]}
            ]
        }"#,
    )
    .unwrap();
    fs::write(
        temp.path().join("synthetic.navigation.json"),
        r#"{
            "schema_version":"0.3",
            "game":"synthetic",
            "server":"lab",
            "navigation":[
                {"id":"synthetic_home_to_target","from_page":"synthetic/home","to_page":"synthetic/target","effect":"navigation_only","click":{"kind":"rect","x":1,"y":2,"width":3,"height":4}}
            ],
            "destructive_actions":[]
        }"#,
    )
    .unwrap();
    let pack = temp.path().join("synthetic.pack.json");
    let pages = temp.path().join("synthetic.pages.json");
    let graph = temp.path().join("synthetic.navigation.json");
    seal_semantic_fixture(temp, "synthetic", "lab", &pack, &pages, Some(&graph))
}
