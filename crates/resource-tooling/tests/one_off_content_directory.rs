// SPDX-License-Identifier: AGPL-3.0-only

// one-off (to be reverted): #288 A1 evidence on CI. E1 (digest vs the coreutils one-liner and
// a ZIP round trip), E2 (old reference bytes decode and re-encode unchanged), a neutral-fixture
// E3 (sealed derived outputs vs in-memory assembly) and E4 (tampering and admission refusals).

use actingcommand_contract::{
    ContentDirectory, ContentDirectoryVersion, PackageRef, content_directory_digest,
};
use actingcommand_pack_containment::{Containment, ContainmentError, InstanceId, Sha256Hash};
use actingcommand_resource_tooling::{
    AuthoringEnvironmentSnapshot, DEFAULT_MAX_BUFFERED_PAYLOAD_BYTES, PackageBuildTaskRequest,
    PackageEnvOptions, PackageSource, ResourceConvertRequest, prepare_package_build_task,
    resource_convert,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};
use tempfile::TempDir;

const GAME: &str = "fourth-game";
const SERVER: &str = "test-shard";
const LOCALE: &str = "x-fixture";
const DERIVED: [&str; 6] = [
    "resources/manifest.json",
    "resources/recognition/fourth-game.test-shard.pack.json",
    "resources/recognition/fourth-game.test-shard.pages.json",
    "resources/navigation/fourth-game.test-shard.navigation.json",
    "resources/operations/operations.index.json",
    "resources/operations/operations.primitives.json",
];

fn report(line: impl AsRef<str>) {
    // Direct handle writes are not captured by the test harness, so they reach the CI log.
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "ONE-OFF-A1 {}", line.as_ref());
}

fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(60)
}

fn reference(digest: &str) -> PackageRef {
    PackageRef::ContentDirectory(ContentDirectory {
        schema_version: ContentDirectoryVersion::V1,
        sha256: digest.to_owned(),
    })
}

fn load(locator: &Path, expected: &PackageRef) -> Result<(usize, PackageRef), ContainmentError> {
    let mut containment = Containment::new();
    let instance = InstanceId::new("one-off-a1").expect("instance");
    containment
        .load_path(&instance, locator, expected, false, deadline())
        .map(|bundle| (bundle.entry_count(), bundle.package_ref().clone()))
}

fn write_tree(root: &Path, entries: &BTreeMap<String, Vec<u8>>) {
    for (path, bytes) in entries {
        let target = root.join(path);
        fs::create_dir_all(target.parent().expect("parent")).expect("create parent");
        fs::write(target, bytes).expect("write entry");
    }
}

fn read_tree(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut entries = BTreeMap::new();
    let mut pending = vec![(root.to_path_buf(), String::new())];
    while let Some((directory, prefix)) = pending.pop() {
        for entry in fs::read_dir(&directory).expect("read dir") {
            let entry = entry.expect("dir entry");
            let name = entry.file_name().to_str().expect("utf-8 name").to_owned();
            let relative = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            if entry.file_type().expect("file type").is_dir() {
                pending.push((entry.path(), relative));
            } else {
                entries.insert(relative, fs::read(entry.path()).expect("read file"));
            }
        }
    }
    entries
}

fn tree_digest(entries: &BTreeMap<String, Vec<u8>>) -> String {
    content_directory_digest(
        entries
            .iter()
            .map(|(path, bytes)| (path.as_str(), *Sha256Hash::digest(bytes).as_bytes())),
    )
}

fn coreutils_digest(directory: &Path) -> String {
    let script = r#"{ printf 'actingcommand.package.content-directory.v1\n'; /usr/bin/find . -type f -printf '%P\0' | LC_ALL=C /usr/bin/sort -z | /usr/bin/xargs -0 /usr/bin/sha256sum -b | /usr/bin/sed 's/ \*/  /'; } | /usr/bin/sha256sum -b | /usr/bin/cut -c1-64"#;
    let output = Command::new(r"C:\Program Files\Git\bin\bash.exe")
        .arg("-c")
        .arg(script)
        .current_dir(directory)
        .output()
        .expect("run Git Bash coreutils");
    assert!(
        output.status.success(),
        "coreutils failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("utf-8 digest")
        .trim()
        .to_owned()
}

fn sealed_package(temp: &Path) -> (BTreeMap<String, Vec<u8>>, Vec<u8>) {
    let resource_root = temp.join("external-resources");
    write_external_resource_fixture(&resource_root);
    resource_convert(ResourceConvertRequest {
        repo: resource_root.clone(),
        game: None,
        server: None,
        locale: None,
        maa_tasks_root: None,
        dry_run: false,
    })
    .expect("convert fixture");
    let out = temp.join("sealed.zip");
    prepare_package_build_task(PackageBuildTaskRequest {
        source: PackageSource::Local(resource_root),
        temporary_root: temp.join("remote-source"),
        task_id: "return_home".to_string(),
        game: None,
        server: None,
        locale: None,
        package_id: None,
        execution_mode: None,
        resolution: None,
        include_recovery: false,
        out: out.clone(),
        dry_run: false,
        max_buffered_payload_bytes: DEFAULT_MAX_BUFFERED_PAYLOAD_BYTES,
        env: PackageEnvOptions::default(),
    })
    .expect("prepare sealed package")
    .build(&AuthoringEnvironmentSnapshot::default())
    .expect("build sealed package");
    let bytes = fs::read(&out).expect("read sealed zip");
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes.clone())).expect("open sealed zip");
    let mut entries = BTreeMap::new();
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).expect("sealed entry");
        if file.is_dir() {
            continue;
        }
        let mut content = Vec::new();
        file.read_to_end(&mut content).expect("read sealed entry");
        entries.insert(file.name().to_owned(), content);
    }
    (entries, bytes)
}

#[test]
fn one_off_a1_e1_e3_e4_content_directory() {
    let temp = TempDir::new().expect("temp dir");
    let (sealed, sealed_zip) = sealed_package(temp.path());
    report(format!(
        "sealed entries: {:?}",
        sealed.keys().collect::<Vec<_>>()
    ));
    let mut source = sealed.clone();
    for path in DERIVED {
        assert!(source.remove(path).is_some(), "sealed package lacks {path}");
    }
    let digest = tree_digest(&source);
    let packs = temp.path().join("packs");
    let pack = packs.join(&digest);
    write_tree(&pack, &source);
    report(format!("digest {digest} over {} files", source.len()));

    // E1: the coreutils one-liner (Git Bash, -b | sed) agrees with the contract function.
    let coreutils = coreutils_digest(&pack);
    report(format!("E1 coreutils {coreutils} contract {digest}"));
    assert_eq!(coreutils, digest, "E1 coreutils digest");

    // E1: a ZIP round trip keeps the digest; the extracted copy is admitted as well.
    let zip_path = temp.path().join("roundtrip.zip");
    {
        let mut writer = zip::ZipWriter::new(fs::File::create(&zip_path).expect("create zip"));
        let options =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (path, bytes) in &read_tree(&pack) {
            writer.start_file(path, options).expect("zip entry");
            writer.write_all(bytes).expect("zip bytes");
        }
        writer.finish().expect("finish zip");
    }
    let extracted = temp.path().join("roundtrip").join(&digest);
    let mut archive =
        zip::ZipArchive::new(fs::File::open(&zip_path).expect("open zip")).expect("zip archive");
    archive.extract(&extracted).expect("extract zip");
    let roundtrip = tree_digest(&read_tree(&extracted));
    report(format!("E1 zip round trip {roundtrip}"));
    assert_eq!(roundtrip, digest, "E1 zip round trip digest");
    load(&extracted, &reference(&digest)).expect("E1 extracted copy admitted");

    // Loader: the hash-named directory is admitted and issued with the ContentDirectory reference.
    let object = format!(
        r#"{{"schema_version":"actingcommand.package.content-directory.v1","sha256":"{digest}"}}"#
    );
    let parsed = PackageRef::parse_argument(&object).expect("parse object reference");
    assert_eq!(parsed, reference(&digest));
    let mut containment = Containment::new();
    let instance = InstanceId::new("one-off-a1").expect("instance");
    let bundle = containment
        .load_path(&instance, &pack, &parsed, false, deadline())
        .expect("content directory admitted");
    assert_eq!(bundle.package_ref(), &parsed);
    let issued = serde_json::to_string(bundle.package_ref()).expect("issued reference");
    report(format!("admitted: issued reference {issued}"));
    assert_eq!(issued, object);
    report(format!(
        "admitted: {} in-memory entries, task {}",
        bundle.entry_count(),
        bundle.task_id()
    ));

    // E3 (neutral fixture): in-memory derived outputs vs the sealed package's files.
    for path in &DERIVED[1..] {
        let memory: Value =
            serde_json::from_slice(bundle.entry(path).expect("in-memory derived")).expect("json");
        let sealed_value: Value = serde_json::from_slice(&sealed[*path]).expect("sealed json");
        let equal = memory == sealed_value;
        report(format!(
            "E3 {path}: {}",
            if equal { "EQUAL" } else { "DIFFERENT" }
        ));
        if !equal {
            report(format!("E3 memory {path}: {memory}"));
            report(format!("E3 sealed {path}: {sealed_value}"));
        }
        assert!(equal, "E3 derived output {path} differs");
    }
    let sealed_manifest: Value =
        serde_json::from_slice(&sealed["resources/manifest.json"]).expect("sealed manifest");
    report(format!(
        "E3 info resources/manifest.json: {}",
        if bundle.manifest() == &sealed_manifest {
            "EQUAL".to_owned()
        } else {
            format!(
                "DIFFERENT memory={} sealed={}",
                bundle.manifest(),
                sealed_manifest
            )
        }
    ));
    let mut zip_containment = Containment::new();
    let zip_bundle = zip_containment
        .load(
            &InstanceId::new("one-off-a1-zip").expect("instance"),
            &sealed_zip,
            &Sha256Hash::digest(&sealed_zip),
        )
        .expect("sealed zip admitted");
    report(format!(
        "E3 info execution operation vs sealed zip admission: {}",
        if bundle.operation() == zip_bundle.operation() {
            "EQUAL"
        } else {
            "DIFFERENT"
        }
    ));

    // E4 (a): a changed JSON value, made unparsable, is refused by digest before any parse.
    let tampered = temp.path().join("tamper-json").join(&digest);
    let mut changed = source.clone();
    changed.insert("control.json".to_owned(), b"{\"not json".to_vec());
    write_tree(&tampered, &changed);
    let error = load(&tampered, &reference(&digest)).expect_err("tampered json refused");
    report(format!("E4 changed json: {error}"));
    assert!(matches!(
        error,
        ContainmentError::ContentDirectoryDigestMismatch { file_count, .. } if file_count == source.len()
    ));

    // E4 (b): a removed image (interrupted copy) is refused by digest.
    let truncated = temp.path().join("tamper-image").join(&digest);
    let mut missing = source.clone();
    let image = missing
        .keys()
        .find(|path| path.ends_with(".png"))
        .cloned()
        .expect("fixture image");
    missing.remove(&image);
    write_tree(&truncated, &missing);
    let error = load(&truncated, &reference(&digest)).expect_err("missing image refused");
    report(format!("E4 removed {image}: {error}"));
    assert!(matches!(
        error,
        ContainmentError::ContentDirectoryDigestMismatch { file_count, .. } if file_count == source.len() - 1
    ));

    // Digest-form name that differs from the reference: refused before reading.
    let renamed = temp.path().join("renamed").join("a".repeat(64));
    write_tree(&renamed, &source);
    let error = load(&renamed, &reference(&digest)).expect_err("name mismatch");
    report(format!("name mismatch: {error}"));
    assert_eq!(
        error,
        ContainmentError::SourceTree {
            code: "content_directory_name_mismatch"
        }
    );

    // An author directory with a non-digest name is verified against the explicit reference.
    let author = temp.path().join("author").join("work");
    write_tree(&author, &source);
    load(&author, &reference(&digest)).expect("author directory admitted");
    report("author directory with explicit reference: admitted");

    // Empty directories do not contribute to the digest.
    let empty = temp.path().join("empty-dir").join(&digest);
    write_tree(&empty, &source);
    fs::create_dir_all(empty.join("resources/unused/nested")).expect("empty directory");
    load(&empty, &reference(&digest)).expect("empty directories ignored");
    report("empty directories: admitted with the same digest");

    let refusal = |name: &str, extra: &[(&str, &[u8])]| {
        let root = temp.path().join(name).join("work");
        let mut entries = source.clone();
        for (path, bytes) in extra {
            entries.insert((*path).to_owned(), bytes.to_vec());
        }
        write_tree(&root, &entries);
        let expected = tree_digest(&entries);
        let error = load(&root, &reference(&expected)).expect_err(name);
        report(format!("{name}: {error}"));
        error
    };
    assert_eq!(
        refusal(
            "lfs",
            &[(
                "resources/operations/return_home/assets/POINTER.png",
                b"version https://git-lfs.github.com/spec/v1\noid sha256:0000\nsize 1\n"
            )]
        ),
        ContainmentError::SourceTree {
            code: "content_directory_lfs_pointer"
        }
    );
    assert_eq!(
        refusal("git-segment", &[(".git/config", b"[core]\n")]),
        ContainmentError::SourceTree {
            code: "content_directory_path_invalid"
        }
    );
    assert!(matches!(
        refusal("script", &[("resources/tool.py", b"print(1)\n")]),
        ContainmentError::ForbiddenEntry { .. }
    ));
    assert_eq!(
        refusal("derived", &[(DERIVED[0], sealed[DERIVED[0]].as_slice())]),
        ContainmentError::SourceTree {
            code: "source_contains_derived_output"
        }
    );

    // Links inside the directory are refused (symbolic link and junction).
    let linked = temp.path().join("link-file").join("work");
    write_tree(&linked, &source);
    match std::os::windows::fs::symlink_file(
        linked.join("control.json"),
        linked.join("resources/alias.json"),
    ) {
        Ok(()) => {
            let error = load(&linked, &reference(&digest)).expect_err("symlink refused");
            report(format!("symlink file: {error}"));
            assert_eq!(
                error,
                ContainmentError::SourceTree {
                    code: "content_directory_link_or_type"
                }
            );
        }
        Err(error) => report(format!("SKIPPED symlink file: cannot create ({error})")),
    }
    let junctioned = temp.path().join("link-dir").join("work");
    write_tree(&junctioned, &source);
    let status = Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(junctioned.join("resources/junction"))
        .arg(junctioned.join("resources/operations"))
        .output()
        .expect("run mklink");
    if status.status.success() {
        let error = load(&junctioned, &reference(&digest)).expect_err("junction refused");
        report(format!("junction: {error}"));
        assert_eq!(
            error,
            ContainmentError::SourceTree {
                code: "content_directory_link_or_type"
            }
        );
    } else {
        report(format!(
            "SKIPPED junction: mklink failed ({})",
            String::from_utf8_lossy(&status.stderr)
        ));
    }

    // A relative locator is refused.
    let error = load(Path::new("packs/relative"), &reference(&digest)).expect_err("relative");
    report(format!("relative locator: {error}"));
    assert_eq!(
        error,
        ContainmentError::SourceTree {
            code: "content_directory_locator_not_absolute"
        }
    );
}

#[derive(Serialize, Deserialize)]
struct PrefixedSlot {
    #[serde(with = "actingcommand_contract::prefixed_reference")]
    package_digest: PackageRef,
}

#[derive(Serialize, Deserialize)]
struct OptionalPrefixedSlot {
    #[serde(with = "actingcommand_contract::optional_prefixed_reference")]
    package_digest: Option<PackageRef>,
}

#[derive(Serialize, Deserialize)]
struct BareSlot {
    expected_sha256: PackageRef,
}

#[test]
fn one_off_a1_e2_reference_bytes_round_trip() {
    let hex = "0123456789abcdef".repeat(4);
    let git = r#"{"schema_version":"actingcommand.package.git-source-tree.v1","repository":"https://example.org/team/resources","commit":{"algorithm":"sha1","hex":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"bundle_path":"bundles/neutral","tree":{"algorithm":"sha1","hex":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}}"#;
    let content = format!(
        r#"{{"schema_version":"actingcommand.package.content-directory.v1","sha256":"{hex}"}}"#
    );
    let cases = [
        (format!(r#"{{"expected_sha256":"{hex}"}}"#), "bare legacy"),
        (r#"{"expected_sha256":""}"#.to_owned(), "bare empty"),
        (
            format!(r#"{{"expected_sha256":{git}}}"#),
            "bare git source tree",
        ),
        (
            format!(r#"{{"expected_sha256":{content}}}"#),
            "bare content directory",
        ),
    ];
    for (wire, name) in &cases {
        let decoded: BareSlot = serde_json::from_str(wire).expect(name);
        let encoded = serde_json::to_string(&decoded).expect(name);
        report(format!(
            "E2 {name}: {:?} identical={}",
            variant(&decoded.expected_sha256),
            &encoded == wire
        ));
        assert_eq!(&encoded, wire, "{name}");
    }
    let prefixed = [
        (
            format!(r#"{{"package_digest":"sha256:{hex}"}}"#),
            "prefixed legacy",
        ),
        (r#"{"package_digest":""}"#.to_owned(), "prefixed empty"),
        (
            format!(r#"{{"package_digest":{git}}}"#),
            "prefixed git source tree",
        ),
        (
            format!(r#"{{"package_digest":{content}}}"#),
            "prefixed content directory",
        ),
    ];
    for (wire, name) in &prefixed {
        let decoded: PrefixedSlot = serde_json::from_str(wire).expect(name);
        let encoded = serde_json::to_string(&decoded).expect(name);
        report(format!(
            "E2 {name}: {:?} identical={}",
            variant(&decoded.package_digest),
            &encoded == wire
        ));
        assert_eq!(&encoded, wire, "{name}");
        let optional: OptionalPrefixedSlot = serde_json::from_str(wire).expect(name);
        assert_eq!(&serde_json::to_string(&optional).expect(name), wire);
    }
    let none = r#"{"package_digest":null}"#;
    let optional: OptionalPrefixedSlot = serde_json::from_str(none).expect("optional none");
    assert_eq!(
        serde_json::to_string(&optional).expect("optional none"),
        none
    );
    assert!(
        serde_json::from_str::<PrefixedSlot>(&format!(r#"{{"package_digest":"{hex}"}}"#)).is_err()
    );

    let git_ref: PackageRef = serde_json::from_str(git).expect("git");
    assert!(matches!(git_ref, PackageRef::GitSourceTree(_)));
    let content_ref: PackageRef = serde_json::from_str(&content).expect("content");
    assert!(matches!(content_ref, PackageRef::ContentDirectory(_)));
    assert_eq!(PackageRef::parse_argument(git).expect("git arg"), git_ref);
    assert_eq!(
        PackageRef::parse_argument(&content).expect("content arg"),
        content_ref
    );
    for invalid in [
        format!(
            r#"{{"schema_version":"actingcommand.package.content-directory.v1","sha256":"{hex}","extra":1}}"#
        ),
        format!(r#"{{"schema_version":"actingcommand.package.git-source-tree.v1","sha256":"{hex}"}}"#),
        r#"{"schema_version":"actingcommand.package.content-directory.v1","repository":"https://example.org/team/resources","commit":{"algorithm":"sha1","hex":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"bundle_path":"bundles/neutral","tree":{"algorithm":"sha1","hex":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}}"#.to_owned(),
    ] {
        assert!(serde_json::from_str::<PackageRef>(&invalid).is_err(), "{invalid}");
        assert!(PackageRef::parse_argument(&invalid).is_err(), "{invalid}");
    }
    let upper = format!(
        r#"{{"schema_version":"actingcommand.package.content-directory.v1","sha256":"{}"}}"#,
        hex.to_uppercase()
    );
    assert!(PackageRef::parse_argument(&upper).is_err());
    assert!(content_ref.is_directory_source() && git_ref.is_directory_source());
    assert!(!PackageRef::from(hex.as_str()).is_directory_source());
    assert_eq!(content_ref.legacy_sha256(), None);
    report("E2 cross-decoding and validation refusals: as specified");
}

fn variant(reference: &PackageRef) -> &'static str {
    match reference {
        PackageRef::LegacyZipSha256(_) => "LegacyZipSha256",
        PackageRef::GitSourceTree(_) => "GitSourceTree",
        PackageRef::ContentDirectory(_) => "ContentDirectory",
    }
}

fn write_external_resource_fixture(root: &Path) {
    fs::create_dir_all(root.join("operations/return_home/assets")).expect("operation assets");
    fs::create_dir_all(root.join("navigation")).expect("navigation directory");
    fs::write(
        root.join("operations/resources.json"),
        serde_json::to_vec_pretty(&json!({
            "schema_version": "1.0",
            "resources": [],
            "resource_count": 0
        }))
        .expect("resources json"),
    )
    .expect("write resources");
    fs::write(
        root.join("operations/return_home/assets/HOME.png"),
        one_pixel_png(),
    )
    .expect("write template");
    fs::write(
        root.join("operations/return_home/task.json"),
        serde_json::to_vec_pretty(&json!({
            "schema_version": "0.3",
            "task_id": "return_home",
            "game": GAME,
            "server_scope": [SERVER],
            "locale": LOCALE,
            "goal": "external neutral fixture",
            "coordinate_space": {"width": 1280, "height": 720},
            "defaults": {"template_threshold": 0.9, "color_max_distance": 20.0},
            "anchors": [{
                "id": "home",
                "template": "assets/HOME.png",
                "region": {"mode": "rect", "rect": {"x": 20, "y": 20, "width": 30, "height": 30}},
                "threshold": 0.8,
                "color_check": null
            }],
            "entry_page": "home",
            "target_page": "home",
            "operations": [{
                "id": "home_noop",
                "purpose": "neutral fixture",
                "from": "home",
                "to": null,
                "click": {"kind": "point", "x": 1, "y": 1},
                "verify_template": null,
                "guard": {
                    "page_id": "home",
                    "target_id": "page/home",
                    "expected_rect": {"x": 1, "y": 1, "width": 1, "height": 1},
                    "verify_template": "assets/HOME.png"
                },
                "consumes": [],
                "produces": []
            }]
        }))
        .expect("task json"),
    )
    .expect("write task");
}

fn one_pixel_png() -> &'static [u8] {
    &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6,
        0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 156, 99, 248, 15, 4, 0, 9,
        251, 3, 253, 167, 89, 75, 221, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ]
}
