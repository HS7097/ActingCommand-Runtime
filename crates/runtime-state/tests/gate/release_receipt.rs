// SPDX-License-Identifier: AGPL-3.0-only

//! Test plan v2.2, L-1 runtime-state G6 (inventory-ledger key 29): a release transition whose
//! commit receipt was lost is committed with its pointer and history, and resubmitting it is
//! idempotent (`contracts/runtime-database.md`, "Release transactions and legacy provenance";
//! `contracts/ledger-store.md`: never claim NotPerformed, never resend the successful operation).

use actingcommand_contract::{ReleaseResourceVersion, ReleaseTransitionKind, RuntimeReleaseSet};
use actingcommand_runtime_state::{ReleaseArtifactSources, RuntimeStateStore};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

const STATE_KEY: &[u8] = b"0123456789abcdef";

fn sha256(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

/// Three artifact files and the release set naming them, with a neutral project id.
fn release(root: &Path, version: &str) -> (RuntimeReleaseSet, ReleaseArtifactSources) {
    let source_root = root.join(format!("release-source-{version}"));
    fs::create_dir(&source_root).expect("release source root");
    let runtime = source_root.join("runtime.bin");
    let ui = source_root.join("ui.bin");
    let resource = source_root.join("resource.bin");
    let runtime_bytes = format!("runtime:{version}");
    let ui_bytes = format!("ui:{version}");
    let resource_bytes = format!("resource:{version}");
    fs::write(&runtime, runtime_bytes.as_bytes()).expect("runtime artifact");
    fs::write(&ui, ui_bytes.as_bytes()).expect("UI artifact");
    fs::write(&resource, resource_bytes.as_bytes()).expect("resource artifact");
    let manifest = RuntimeReleaseSet::new(
        version,
        sha256(runtime_bytes.as_bytes()),
        version,
        sha256(ui_bytes.as_bytes()),
        vec![
            ReleaseResourceVersion::new("project-a", version, sha256(resource_bytes.as_bytes()))
                .expect("resource"),
        ],
    )
    .expect("release");
    let sources = ReleaseArtifactSources::new(
        runtime,
        ui,
        BTreeMap::from([("project-a".to_owned(), resource)]),
    );
    (manifest, sources)
}

#[test]
fn gate_lost_release_commit_receipt_is_recovered_and_resubmission_is_idempotent() {
    let root = TempDir::new().expect("root");
    let (manifest, sources) = release(root.path(), "1.0.0");
    let store = RuntimeStateStore::open(root.path(), STATE_KEY).expect("store");
    store
        .stage_release(manifest.clone(), &sources)
        .expect("stage release");
    let preview = store
        .preview_release_transition(ReleaseTransitionKind::Activate, manifest.release_id())
        .expect("preview");
    // The receipt is lost: nothing reads the reply, and the owner goes away.
    drop(store.commit_release_transition(&preview));
    drop(store);

    let store = RuntimeStateStore::open(root.path(), STATE_KEY).expect("reopened store");
    let active = store
        .active_release()
        .expect("active release")
        .expect("the transition is committed");
    assert_eq!(active.manifest(), &manifest);
    assert_eq!(active.revision(), preview.data().pointer_revision());
    assert_eq!(
        store.release_transitions().expect("transitions"),
        vec![preview.data().clone()]
    );

    let resubmitted = store
        .commit_release_transition(&preview)
        .expect("resubmission");
    assert_eq!(
        resubmitted, active,
        "resubmission returns the committed pointer"
    );
    assert_eq!(
        store.active_release().expect("active release"),
        Some(active.clone())
    );
    assert_eq!(
        store.release_transitions().expect("transitions").len(),
        1,
        "no second transition"
    );
    assert_eq!(resubmitted.revision(), preview.data().pointer_revision());
}
