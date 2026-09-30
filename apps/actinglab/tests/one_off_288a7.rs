// SPDX-License-Identifier: AGPL-3.0-only

// one-off (to be reverted): Workflow #288 A7 evidence on CI. Every task pack of the public
// umbrella BA bundle (HS7097/ActingCommand 536f048a, bundles/bluearchive-bundle-3ff697b.zip) is
// unsealed as A2b's E3 did and derived by the loader; the same run on the merge-base and on the
// head must print identical `package digest` data and identical sha256 lists of every derived
// document. The `resource convert` path over a copy of each pack's resources is listed too.

use actingcommand_contract::{ContentDirectory, ContentDirectoryVersion, PackageRef};
use actingcommand_pack_containment::{Containment, InstanceId, Sha256Hash};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};
use tempfile::TempDir;

const ASSET_URL: &str = "https://raw.githubusercontent.com/HS7097/ActingCommand/536f048a3cac26ddbb0391895391f98967704cb0/bundles/bluearchive-bundle-3ff697b.zip";
const ASSET_SHA256: &str = "df524eac6290c4c0b435f3872a89e12dbe4f49e37555fa102bf96dd71b696af8";

fn report(line: impl AsRef<str>) {
    // Direct handle writes are not captured by the test harness, so they reach the CI log.
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "ONE-OFF-A7 {}", line.as_ref());
}

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn run_actinglab(config: &Path, args: &[&str]) -> (bool, Value) {
    let output = Command::new(env!("CARGO_BIN_EXE_actinglab"))
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

/// Per-run temporary paths are replaced so that the merge-base and head runs compare equal.
fn normalize(text: &str, temp: &Path) -> String {
    let raw = arg(temp);
    let escaped = raw.replace('\\', "\\\\");
    let forward = raw.replace('\\', "/");
    text.replace(&escaped, "<temp>")
        .replace(raw, "<temp>")
        .replace(&forward, "<temp>")
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

fn reference(digest: &str) -> PackageRef {
    PackageRef::ContentDirectory(ContentDirectory {
        schema_version: ContentDirectoryVersion::V1,
        sha256: digest.to_owned(),
    })
}

#[test]
fn one_off_a7_derived_documents_of_every_umbrella_task_pack() {
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
    let asset_sha = sha(&asset_bytes);
    report(format!(
        "asset {} bytes sha256 {asset_sha} (expected {ASSET_SHA256})",
        asset_bytes.len()
    ));
    assert_eq!(asset_sha, ASSET_SHA256, "umbrella asset hash");
    let outer = read_zip(&asset_bytes);
    let index: Value = serde_json::from_slice(&outer["bundle.json"]).expect("bundle v1 index");
    let packs = index["packs"].as_array().expect("packs");
    report(format!(
        "index {} source_sha {} packs {}",
        index["schema_version"],
        index["source_sha"],
        packs.len()
    ));

    let unsealed_root = temp.path().join("unsealed");
    let convert_root = temp.path().join("convert");
    let mut admitted = 0_usize;
    let mut refused = 0_usize;
    let mut derived_documents = 0_usize;
    let mut converted = 0_usize;
    for pack in packs {
        let package_id = pack["package_id"].as_str().expect("package id").to_owned();
        let zip_bytes = outer[pack["path"].as_str().expect("path")].clone();
        assert_eq!(
            sha(&zip_bytes),
            pack["sha256"].as_str().expect("sha256"),
            "{package_id} sealed hash"
        );
        let entries = read_zip(&zip_bytes);
        let control: Value = serde_json::from_slice(&entries["control.json"]).expect("control");
        let game = control["game"].as_str().expect("game").to_owned();
        let server = control["server"].as_str().expect("server").to_owned();
        let entry_task = control["entry_task_id"].as_str().expect("entry").to_owned();
        let stem = format!("{game}.{server}");
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
            "{package_id}: sealed {} entries, removed {removed} derived, unsealed {} files",
            entries.len(),
            unsealed.len()
        ));

        // `package digest` on the unsealed directory: its full data (or refusal) is compared.
        let (ok, envelope) = run_actinglab(
            &config,
            &["package", "digest", "--package", arg(&directory)],
        );
        if !ok {
            refused += 1;
            let error = normalize(&envelope["error"].to_string(), temp.path());
            report(format!(
                "{package_id}: package digest REFUSED error sha256 {} {error}",
                sha(error.as_bytes())
            ));
            let zip_admission = Containment::for_metadata_validation()
                .load(
                    &InstanceId::new("one-off-a7-zip").expect("instance"),
                    &zip_bytes,
                    &Sha256Hash::digest(&zip_bytes),
                )
                .map(|_| "admitted".to_owned())
                .unwrap_or_else(|error| format!("refused: {error}"));
            report(format!(
                "{package_id}: sealed ZIP admission by the current loader: {}",
                normalize(&zip_admission, temp.path())
            ));
        } else {
            admitted += 1;
            let data = normalize(&envelope["data"].to_string(), temp.path());
            report(format!(
                "{package_id}: package digest data sha256 {} {data}",
                sha(data.as_bytes())
            ));
            let digest = envelope["data"]["reference"]["sha256"]
                .as_str()
                .expect("digest")
                .to_owned();

            // The loader's in-memory derivation of the same directory.
            let mut containment = Containment::for_metadata_validation();
            let bundle = containment
                .load_path(
                    &InstanceId::new("one-off-a7").expect("instance"),
                    &directory,
                    &reference(&digest),
                    false,
                    Instant::now() + Duration::from_secs(60),
                )
                .expect("admitted by package digest, admitted again");
            for path in &derived {
                match bundle.entry(path) {
                    Some(bytes) => {
                        derived_documents += 1;
                        report(format!(
                            "{package_id}: derived {path} {} bytes sha256 {}",
                            bytes.len(),
                            sha(bytes)
                        ));
                    }
                    None => report(format!("{package_id}: derived {path} ABSENT")),
                }
            }
            let mut listing = String::new();
            let mut count = 0_usize;
            for path in bundle.entry_paths() {
                count += 1;
                listing.push_str(&format!(
                    "{path} {}\n",
                    sha(bundle.entry(path).expect("listed entry"))
                ));
            }
            report(format!(
                "{package_id}: all {count} in-memory entries (path sha256 list) sha256 {}",
                sha(listing.as_bytes())
            ));
            report(format!(
                "{package_id}: execution operation {} sha256 {}",
                bundle.operation_path(),
                sha(&serde_json::to_vec(bundle.operation()).expect("operation json"))
            ));
            report(format!(
                "{package_id}: manifest value sha256 {}",
                sha(&serde_json::to_vec(bundle.manifest()).expect("manifest json"))
            ));
        }

        // `resource convert` over a copy of the unsealed resources (written outputs listed).
        let repo = convert_root.join(&package_id);
        let resources = unsealed
            .iter()
            .filter_map(|(path, bytes)| {
                path.strip_prefix("resources/")
                    .map(|relative| (relative.to_owned(), bytes.clone()))
            })
            .collect::<BTreeMap<_, _>>();
        write_tree(&repo, &resources);
        let entry_json = format!("operations/{entry_task}/task.json");
        let locale = resources
            .get(&entry_json)
            .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok())
            .and_then(|task| task["locale"].as_str().map(str::to_owned));
        let mut args = vec![
            "resource",
            "convert",
            "--repo",
            arg(&repo),
            "--game",
            game.as_str(),
            "--server",
            server.as_str(),
        ];
        if let Some(locale) = locale.as_deref() {
            args.extend(["--locale", locale]);
        }
        let (ok, envelope) = run_actinglab(&config, &args);
        if ok {
            converted += 1;
            let data = normalize(&envelope["data"].to_string(), temp.path());
            report(format!("{package_id}: resource convert ok {data}"));
            for path in &derived[1..] {
                let written = repo.join(path.strip_prefix("resources/").expect("prefix"));
                match fs::read(&written) {
                    Ok(bytes) => report(format!(
                        "{package_id}: convert wrote {path} {} bytes sha256 {}",
                        bytes.len(),
                        sha(&bytes)
                    )),
                    Err(error) => report(format!(
                        "{package_id}: convert {path} unreadable {:?}",
                        error.kind()
                    )),
                }
            }
        } else {
            let error = normalize(&envelope["error"].to_string(), temp.path());
            report(format!("{package_id}: resource convert REFUSED {error}"));
        }
    }
    report(format!(
        "summary: {} packs, package digest admitted {admitted} refused {refused}, loader derived documents listed {derived_documents}, resource convert ok {converted}",
        packs.len()
    ));
    assert_eq!(admitted + refused, packs.len());
}
