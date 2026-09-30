// SPDX-License-Identifier: AGPL-3.0-only

// one-off (to be reverted): Workflow #288 A2b evidence on CI. E1 (`actinglab package digest`
// against the coreutils one-liner in Git Bash, and a ZIP round trip), E3 on the real sealed
// packs of the public umbrella release asset (unsealed directories through the new loader,
// derived outputs listed against the sealed files, not asserted), and `package bundle` over
// the same directories, including its refusals.

use actingcommand_contract::{
    BundleIndexV2, ContentDirectory, ContentDirectoryVersion, PackageRef,
};
use actingcommand_pack_containment::{Containment, InstanceId, Sha256Hash};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::Path;
use std::process::{Command, Output};
use std::time::{Duration, Instant};
use tempfile::TempDir;

const ASSET_URL: &str = "https://github.com/HS7097/ActingCommand/releases/download/build-r651fead-uf5cb9d9/bluearchive-bundle-3ff697b.zip";
const ASSET_SHA256: &str = "df524eac6290c4c0b435f3872a89e12dbe4f49e37555fa102bf96dd71b696af8";
const SOURCE_REPOSITORY: &str = "HS7097/ActingCommand-Resources-BlueArchive";

fn report(line: impl AsRef<str>) {
    // Direct handle writes are not captured by the test harness, so they reach the CI log.
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "ONE-OFF-A2B {}", line.as_ref());
}

fn short(value: Option<&Value>) -> String {
    let text = value.map_or_else(|| "<absent>".to_owned(), Value::to_string);
    if text.chars().count() > 240 {
        format!(
            "{}...({} bytes)",
            text.chars().take(240).collect::<String>(),
            text.len()
        )
    } else {
        text
    }
}

fn json_diff(pointer: &str, memory: &Value, sealed: &Value, out: &mut Vec<String>) {
    if out.len() >= 30 || memory == sealed {
        return;
    }
    match (memory, sealed) {
        (Value::Object(left), Value::Object(right)) => {
            let keys = left.keys().chain(right.keys()).collect::<BTreeSet<_>>();
            for key in keys {
                match (left.get(key), right.get(key)) {
                    (Some(a), Some(b)) => json_diff(&format!("{pointer}/{key}"), a, b, out),
                    (a, b) => out.push(format!(
                        "{pointer}/{key}: memory={} sealed={}",
                        short(a),
                        short(b)
                    )),
                }
            }
        }
        (Value::Array(left), Value::Array(right)) if left.len() == right.len() => {
            for (index, (a, b)) in left.iter().zip(right).enumerate() {
                json_diff(&format!("{pointer}/{index}"), a, b, out);
            }
        }
        _ => out.push(format!(
            "{pointer}: memory={} sealed={}",
            short(Some(memory)),
            short(Some(sealed))
        )),
    }
}

fn run_actinglab(config: &Path, args: &[&str]) -> (bool, Value) {
    let output: Output = Command::new(env!("CARGO_BIN_EXE_actinglab"))
        .arg("--json")
        .args(args)
        .env("ACTINGLAB_CONFIG_PATH", config)
        .output()
        .expect("run actinglab");
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "parse CLI JSON: {error}\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output.status.success(), envelope)
}

fn arg(path: &Path) -> &str {
    path.to_str().expect("utf-8 path")
}

fn read_zip(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).expect("open zip");
    let mut entries = BTreeMap::new();
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).expect("zip entry");
        if file.is_dir() {
            continue;
        }
        let mut content = Vec::new();
        file.read_to_end(&mut content).expect("read zip entry");
        entries.insert(file.name().to_owned(), content);
    }
    entries
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

fn reference(digest: &str) -> PackageRef {
    PackageRef::ContentDirectory(ContentDirectory {
        schema_version: ContentDirectoryVersion::V1,
        sha256: digest.to_owned(),
    })
}

#[test]
fn one_off_a2b_e1_e3_digest_and_bundle_on_the_sealed_packs() {
    let temp = TempDir::new().expect("temp dir");
    let config = temp.path().join("config.json");
    let asset = temp.path().join("asset.zip");
    let status = Command::new("curl")
        .args(["-L", "-f", "-sS", "-o"])
        .arg(&asset)
        .arg(ASSET_URL)
        .status()
        .expect("run curl");
    assert!(status.success(), "download {ASSET_URL}: {status}");
    let asset_bytes = fs::read(&asset).expect("read asset");
    let asset_sha = Sha256Hash::digest(&asset_bytes).to_string();
    report(format!(
        "asset {} bytes sha256 {asset_sha} (expected {ASSET_SHA256})",
        asset_bytes.len()
    ));
    assert_eq!(asset_sha, ASSET_SHA256, "umbrella asset hash");
    let outer = read_zip(&asset_bytes);
    let index: Value = serde_json::from_slice(&outer["bundle.json"]).expect("bundle v1 index");
    let applications = outer["applications.json"].clone();
    report(format!(
        "sealed index {} source_sha {} packs {}",
        index["schema_version"],
        index["source_sha"],
        index["packs"].as_array().expect("packs").len()
    ));

    let unsealed_root = temp.path().join("unsealed");
    let mut total = 0_usize;
    let mut admitted = BTreeMap::new();
    let mut e3_equal = 0;
    let mut e3_different = 0;
    for pack in index["packs"].as_array().expect("packs") {
        total += 1;
        let package_id = pack["package_id"].as_str().expect("package id").to_owned();
        let zip_bytes = outer[pack["path"].as_str().expect("path")].clone();
        assert_eq!(
            Sha256Hash::digest(&zip_bytes).to_string(),
            pack["sha256"].as_str().expect("sha256"),
            "{package_id} sealed hash"
        );
        let entries = read_zip(&zip_bytes);
        let control: Value = serde_json::from_slice(&entries["control.json"]).expect("control");
        let stem = format!(
            "{}.{}",
            control["game"].as_str().expect("game"),
            control["server"].as_str().expect("server")
        );
        let derived = [
            "resources/manifest.json".to_owned(),
            format!("resources/recognition/{stem}.pack.json"),
            format!("resources/recognition/{stem}.pages.json"),
            format!("resources/navigation/{stem}.navigation.json"),
            "resources/operations/operations.index.json".to_owned(),
            "resources/operations/operations.primitives.json".to_owned(),
        ];
        let mut unsealed = entries.clone();
        let removed = derived
            .iter()
            .filter(|path| unsealed.remove(*path).is_some())
            .count();
        let directory = unsealed_root.join(&package_id);
        write_tree(&directory, &unsealed);
        report(format!(
            "E3 {package_id}: sealed {} entries, removed {removed} derived, unsealed {} files",
            entries.len(),
            unsealed.len()
        ));

        // E3: the unsealed directory through the new loader, via `package digest`.
        let (ok, envelope) = run_actinglab(
            &config,
            &["package", "digest", "--package", arg(&directory)],
        );
        if !ok {
            report(format!(
                "E3 {package_id}: package digest REFUSED: {}",
                envelope["error"]
            ));
            continue;
        }
        let data = &envelope["data"];
        let digest = data["reference"]["sha256"]
            .as_str()
            .expect("digest")
            .to_owned();
        report(format!(
            "E3 {package_id}: admitted reference {} package_id {} server {} entry {} files {} bytes {}",
            data["reference"],
            data["package_id"],
            data["server"],
            data["entry_task_id"],
            data["file_count"],
            data["byte_count"]
        ));
        assert_eq!(data["package_id"], package_id.as_str());

        // E1: the coreutils one-liner (Git Bash, -b | sed) agrees with `package digest`.
        let coreutils = coreutils_digest(&directory);
        report(format!(
            "E1 {package_id}: coreutils {coreutils} actinglab {digest} equal={}",
            coreutils == digest
        ));
        assert_eq!(coreutils, digest, "E1 {package_id}");

        // E3: in-memory derived outputs of the same directory against the sealed files.
        let mut containment = Containment::for_metadata_validation();
        let instance = InstanceId::new("one-off-a2b").expect("instance");
        let bundle = containment
            .load_path(
                &instance,
                &directory,
                &reference(&digest),
                false,
                Instant::now() + Duration::from_secs(60),
            )
            .expect("admitted by package digest, admitted again");
        for path in &derived[1..] {
            let memory: Value =
                serde_json::from_slice(bundle.entry(path).expect("in-memory derived"))
                    .expect("memory json");
            let sealed_value: Value =
                serde_json::from_slice(&entries[path.as_str()]).expect("sealed json");
            let mut differences = Vec::new();
            json_diff("", &memory, &sealed_value, &mut differences);
            if differences.is_empty() {
                e3_equal += 1;
                report(format!("E3 {package_id} {path}: EQUAL"));
            } else {
                e3_different += 1;
                report(format!(
                    "E3 {package_id} {path}: DIFFERENT ({} differences shown, at most 30)",
                    differences.len()
                ));
                for difference in differences {
                    report(format!("E3 {package_id} {path}   {difference}"));
                }
            }
        }
        let memory_operation = bundle.operation().clone();
        let mut zip_containment = Containment::for_metadata_validation();
        match zip_containment.load(
            &InstanceId::new("one-off-a2b-zip").expect("instance"),
            &zip_bytes,
            &Sha256Hash::digest(&zip_bytes),
        ) {
            Ok(zip_bundle) => {
                let mut differences = Vec::new();
                json_diff(
                    "",
                    &memory_operation,
                    zip_bundle.operation(),
                    &mut differences,
                );
                report(format!(
                    "E3 info {package_id} execution operation vs sealed ZIP admission: {}",
                    if differences.is_empty() {
                        "EQUAL".to_owned()
                    } else {
                        format!("DIFFERENT {differences:?}")
                    }
                ));
            }
            Err(error) => report(format!(
                "E3 info {package_id} sealed ZIP admission refused: {error}"
            )),
        }
        admitted.insert(package_id, digest);
    }
    report(format!(
        "E3 summary: {} of {total} unsealed directories admitted; derived files EQUAL {e3_equal} DIFFERENT {e3_different}",
        admitted.len()
    ));

    // E1: a ZIP round trip keeps the digest, and the extracted copy is admitted under it.
    let (first_id, first_digest) = admitted.iter().next().expect("one admitted pack");
    let zip_path = temp.path().join("roundtrip.zip");
    {
        let mut writer = zip::ZipWriter::new(fs::File::create(&zip_path).expect("create zip"));
        let options =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (path, bytes) in read_tree(&unsealed_root.join(first_id)) {
            writer.start_file(path, options).expect("zip entry");
            writer.write_all(&bytes).expect("zip bytes");
        }
        writer.finish().expect("finish zip");
    }
    let extracted = temp.path().join("roundtrip").join(first_digest);
    zip::ZipArchive::new(fs::File::open(&zip_path).expect("open zip"))
        .expect("zip archive")
        .extract(&extracted)
        .expect("extract zip");
    let (ok, envelope) = run_actinglab(
        &config,
        &["package", "digest", "--package", arg(&extracted)],
    );
    report(format!(
        "E1 zip round trip {first_id}: ok={ok} digest {} (before {first_digest})",
        envelope["data"]["reference"]["sha256"]
    ));
    assert!(ok, "round trip admitted: {envelope}");
    assert_eq!(
        envelope["data"]["reference"]["sha256"],
        first_digest.as_str()
    );

    // `package bundle` over the same unsealed directories.
    let applications_path = temp.path().join("applications.json");
    fs::write(&applications_path, &applications).expect("write applications");
    let commit = index["source_sha"].as_str().expect("source sha");
    let out = temp.path().join("section");
    let (ok, envelope) = run_actinglab(
        &config,
        &[
            "package",
            "bundle",
            "--applications",
            arg(&applications_path),
            "--packs-root",
            arg(&unsealed_root),
            "--out",
            arg(&out),
            "--source-repository",
            SOURCE_REPOSITORY,
            "--source-commit",
            commit,
        ],
    );
    if !ok {
        report(format!("bundle REFUSED: {}", envelope["error"]));
        assert!(
            admitted.len() < total,
            "bundle refused although every directory was admitted"
        );
        return;
    }
    let top = fs::read_dir(&out)
        .expect("read section")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<BTreeSet<_>>();
    report(format!("bundle written: top level {top:?}"));
    let written = fs::read(out.join("bundle.json")).expect("bundle.json");
    report(format!(
        "bundle.json: {}",
        String::from_utf8_lossy(&written).replace('\n', " ")
    ));
    let parsed: BundleIndexV2 = serde_json::from_slice(&written).expect("decode BundleIndexV2");
    parsed.validate().expect("BundleIndexV2::validate");
    assert_eq!(
        serde_json::to_value(&parsed).expect("index value"),
        envelope["data"]["index"]
    );
    assert_eq!(
        fs::read(out.join("applications.json")).expect("applications copy"),
        applications
    );
    assert!(!temp.path().join("section.part").exists());
    let applications_value: Value = serde_json::from_slice(&applications).expect("applications");
    for (server, entry) in applications_value["servers"].as_object().expect("servers") {
        let default = entry["default_package_id"].as_str().expect("default id");
        let found = parsed
            .packs
            .iter()
            .find(|pack| pack.package_id == default && &pack.server == server)
            .expect("default pack indexed");
        report(format!(
            "bundle default {server}: {default} -> {}",
            found.path
        ));
    }
    for pack in &parsed.packs {
        let copy = out.join("packs").join(&pack.digest);
        let coreutils = coreutils_digest(&copy);
        let (ok, envelope) =
            run_actinglab(&config, &["package", "digest", "--package", arg(&copy)]);
        report(format!(
            "bundle pack {}: digest {} source-directory digest {:?} coreutils equal={} copy admitted={ok}",
            pack.package_id,
            pack.digest,
            admitted.get(&pack.package_id),
            coreutils == pack.digest
        ));
        assert!(ok, "{envelope}");
        assert_eq!(coreutils, pack.digest);
        assert_eq!(admitted.get(&pack.package_id), Some(&pack.digest));
    }

    // Refusals: an existing output, a missing default package (leaves `.part`, named), and a
    // digest-named copy whose content changed.
    let (ok, envelope) = run_actinglab(
        &config,
        &[
            "package",
            "bundle",
            "--applications",
            arg(&applications_path),
            "--packs-root",
            arg(&unsealed_root),
            "--out",
            arg(&out),
        ],
    );
    report(format!(
        "refusal existing out: ok={ok} {}",
        envelope["error"]
    ));
    assert_eq!(envelope["error"]["code"], "package_bundle_out_exists");
    let mut missing_default = applications_value.clone();
    for entry in missing_default["servers"]
        .as_object_mut()
        .expect("servers")
        .values_mut()
    {
        entry["default_package_id"] = Value::String("neutral.absent.task".to_owned());
    }
    let missing_path = temp.path().join("applications-missing.json");
    fs::write(
        &missing_path,
        serde_json::to_vec(&missing_default).expect("json"),
    )
    .expect("write applications");
    let second = temp.path().join("second");
    let (ok, envelope) = run_actinglab(
        &config,
        &[
            "package",
            "bundle",
            "--applications",
            arg(&missing_path),
            "--packs-root",
            arg(&unsealed_root),
            "--out",
            arg(&second),
        ],
    );
    report(format!(
        "refusal missing default: ok={ok} {} part_exists={} out_exists={}",
        envelope["error"],
        temp.path().join("second.part").exists(),
        second.exists()
    ));
    assert_eq!(
        envelope["error"]["code"],
        "package_bundle_default_package_missing"
    );
    let pack = &parsed.packs[0];
    let tampered = temp.path().join("tampered").join(&pack.digest);
    let mut files = read_tree(&out.join("packs").join(&pack.digest));
    files.insert("control.json".to_owned(), b"{\"changed\":true}".to_vec());
    write_tree(&tampered, &files);
    let (ok, envelope) =
        run_actinglab(&config, &["package", "digest", "--package", arg(&tampered)]);
    report(format!(
        "refusal changed digest-named copy: ok={ok} {}",
        envelope["error"]
    ));
    assert!(!ok);
    assert!(
        envelope["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("content_directory_name_mismatch"))
    );
}
