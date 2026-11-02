// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #432 AGT-1a (register §1.245): the client words stay in `crates/agent-forward`. The
// section's parser, the host's pure parts and (from AGT-1b) the worker name no client. Mentions
// elsewhere stay as they are.

use std::path::{Path, PathBuf};

const CLIENT_WORDS: [&str; 6] = [
    "claude",
    "codex",
    "antigravity",
    "dsh",
    "deepseek",
    "bridge",
];

fn sources(path: &Path, files: &mut Vec<PathBuf>) {
    if path.is_dir() {
        let entries = std::fs::read_dir(path).expect("the folder reads");
        for entry in entries {
            sources(&entry.expect("the entry reads").path(), files);
        }
    } else if path.extension().is_some_and(|extension| extension == "rs") {
        files.push(path.to_path_buf());
    }
}

#[test]
fn agent_wake_sources_name_no_client() {
    let host = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    sources(&host.join("src").join("agent_wake"), &mut files);
    assert!(files.len() >= 3, "the pure parts: {files:?}");
    files.push(
        host.join("..")
            .join("runtime-config")
            .join("src")
            .join("agent_wake.rs"),
    );
    // AGT-1b's worker: scanned once it exists.
    let worker = host.join("src").join("host").join("agent_wake.rs");
    if worker.exists() {
        files.push(worker);
    }
    for file in &files {
        let text = std::fs::read_to_string(file)
            .unwrap_or_else(|error| panic!("{}: {error}", file.display()))
            .to_lowercase();
        for word in CLIENT_WORDS {
            assert!(!text.contains(word), "{} names {word}", file.display());
        }
    }
}
