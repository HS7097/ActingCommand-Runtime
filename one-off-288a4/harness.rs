// SPDX-License-Identifier: AGPL-3.0-only

//! One-off (to be reverted), Workflow #288 A4 evidence. Copied into pack-containment as an
//! example and run on the exact merge-base and on the PR head over the same prepared inputs:
//! (a) recorded GitSourceTree references decode and re-encode byte for byte; (b) loading
//! them; (c) ZIP and content-directory admission of the umbrella 536f048a BA packs with the
//! SHA-256 of every admitted and derived document.

use actingcommand_contract::{PackageRef, TaskSemanticFact};
use actingcommand_pack_containment::{Containment, ContainmentError, InstanceId, LoadedBundle};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(120)
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn instance(name: String) -> InstanceId {
    InstanceId::new(name).expect("instance id")
}

/// Prints the admission and the SHA-256 of every document of the loaded capability.
fn report(
    prefix: &str,
    result: Result<&LoadedBundle, ContainmentError>,
) -> Option<BTreeMap<String, String>> {
    match result {
        Ok(bundle) => {
            let mut documents = BTreeMap::new();
            for path in bundle.entry_paths() {
                documents.insert(path.to_owned(), sha(bundle.entry(path).expect("listed")));
            }
            documents.insert(
                "<execution document>".to_owned(),
                sha(&serde_json::to_vec(bundle.operation()).expect("operation")),
            );
            documents.insert(
                "<manifest value>".to_owned(),
                sha(&serde_json::to_vec(bundle.manifest()).expect("manifest")),
            );
            println!(
                "{prefix}|admitted task={} layout={:?} entries={} resident_bytes={} ref={}",
                bundle.task_id().as_str(),
                bundle.layout(),
                bundle.entry_count(),
                bundle.resident_bytes(),
                serde_json::to_string(bundle.package_ref()).expect("reference")
            );
            for (path, hash) in &documents {
                println!("{prefix}|doc {path} {hash}");
            }
            Some(documents)
        }
        Err(error) => {
            println!("{prefix}|refused {error:?} display=\"{error}\"");
            None
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let work = Path::new(&args[1]);
    let label = &args[2];
    println!("H|label {label}");
    let packs: Vec<Value> = serde_json::from_slice(&read(&work.join("packs.json"))).expect("packs");
    let mut containment = Containment::for_metadata_validation();
    for pack in &packs {
        let id = pack["package_id"].as_str().expect("package_id");

        // (a) The recorded reference and a PackageAdmitted fact holding it.
        let text = read(&work.join("refs").join("git").join(format!("{id}.json")));
        let decoded: PackageRef = serde_json::from_slice(&text).expect("decode reference");
        let encoded = serde_json::to_vec(&decoded).expect("encode reference");
        let variant = match &decoded {
            PackageRef::GitSourceTree(_) => "GitSourceTree",
            PackageRef::LegacyZipSha256(_) => "LegacyZipSha256",
            _ => "other",
        };
        let argument =
            PackageRef::parse_argument(std::str::from_utf8(&text).expect("utf-8 reference"));
        println!(
            "A|{id}|reference in_sha256={} out_sha256={} bytes_equal={} variant={variant} validate_ok={} parse_argument_equal={} prefixed_wire_sha256={}",
            sha(&text),
            sha(&encoded),
            encoded == text,
            decoded.validate().is_ok(),
            argument.as_ref().ok() == Some(&decoded),
            sha(&serde_json::to_vec(&decoded.prefixed_wire_value()).expect("prefixed"))
        );
        let fact_text = read(&work.join("refs").join("fact").join(format!("{id}.json")));
        let fact: TaskSemanticFact = serde_json::from_slice(&fact_text).expect("decode fact");
        let fact_encoded = serde_json::to_vec(&fact).expect("encode fact");
        println!(
            "A|{id}|package_admitted_fact in_sha256={} out_sha256={} bytes_equal={}",
            sha(&fact_text),
            sha(&fact_encoded),
            fact_encoded == fact_text
        );

        // (b) Loading the GitSourceTree reference from its Git worktree.
        let git_dir = work.join("git").join("packs").join(id);
        let git_documents = report(
            &format!("B|{id}|git"),
            containment.load_path(
                &instance(format!("a4_git_{id}")),
                &git_dir,
                &decoded,
                false,
                deadline(),
            ),
        );

        // (c) The published ZIP and the same pack as a content directory.
        let zip_reference =
            PackageRef::LegacyZipSha256(pack["sha256"].as_str().expect("sha256").to_owned());
        report(
            &format!("C|{id}|zip"),
            containment.load_path(
                &instance(format!("a4_zip_{id}")),
                &work.join("zips").join(format!("{id}.zip")),
                &zip_reference,
                false,
                deadline(),
            ),
        );
        let directory = work.join("dirs").join(id);
        let snapshot = containment
            .snapshot_content_directory(&directory, deadline())
            .expect("content-directory snapshot");
        let directory_reference = PackageRef::ContentDirectory(snapshot.reference.clone());
        println!(
            "C|{id}|dir reference={} files={}",
            serde_json::to_string(&directory_reference).expect("reference"),
            snapshot.entries.len()
        );
        let directory_documents = report(
            &format!("C|{id}|dir"),
            containment.load_path(
                &instance(format!("a4_dir_{id}")),
                &directory,
                &directory_reference,
                false,
                deadline(),
            ),
        );
        match (&git_documents, &directory_documents) {
            (Some(git), Some(directory)) => println!(
                "B|{id}|git_vs_content_directory documents={} equal={}",
                git.len(),
                git == directory
            ),
            _ => println!(
                "B|{id}|git_vs_content_directory not_comparable git_admitted={} directory_admitted={}",
                git_documents.is_some(),
                directory_documents.is_some()
            ),
        }
    }
    println!("H|done {label}");
}
