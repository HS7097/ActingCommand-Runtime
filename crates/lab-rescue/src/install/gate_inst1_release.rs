// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #421 INST-1, test rows IT-1 to IT-4 of the frozen model INST-M v1.0 (§12, "Release" and
// "Adopt"). Every expected value comes from the model (§4.2, §4.3, §7) and its contract
// (contracts/install-cli.md, "Release verify and stage", "Adopt", "Documents"):
// - IT-1: `release verify` refuses each listed failure with it listed, two failures at once
//   list both, and the release folder is left as it was;
// - IT-2: without facts the result is a warning, the caveat `release_unconfirmed`; `stage`
//   refuses without facts or `--local-build`;
// - IT-3: a second stage of a kept build is `unchanged`; a stage interrupted after its `.part`
//   folder is finished by `resume` to the same bytes as an uninterrupted stage;
// - IT-4: `adopt` keeps exactly the manifest files (`.old-*` ignored), refuses a changed live
//   file (`live_build_modified`), and a second adopt is `unchanged`.
// The codes are the family's folded codes (ruling 54, `contracts/install-cli.md`, "Outcome
// codes"): `install_command_done`, `install_caveat_noted` and `install_command_refused`, each
// with the command as `operation`. The machine is the fake host of `fixture.rs`; every root and
// release folder is a temporary folder with neutral files.

use super::fixture::{
    BuildSpec, C1, C2, FakeHost, TAG, code, live_root, run, sha256_hex, tree, word, write_json,
};
use actingcommand_contract::outcome::Category;
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

/// The answer's outcome value `key`, when it is a string.
fn text<'a>(answer: &'a super::Answer, key: &str) -> Option<&'a str> {
    answer.outcome.value(key).and_then(Value::as_str)
}

/// The checks a refusal lists, from its `release_checks` value.
fn checks(answer: &super::Answer) -> Vec<String> {
    answer
        .outcome
        .value("release_checks")
        .and_then(Value::as_array)
        .map(|checks| {
            checks
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// A release folder of `build` with its facts file, after `tamper` changed the folder (the
/// facts are computed from the folder as `tamper` left it, unless `tamper_facts` changes them).
fn release(
    base: &Path,
    name: &str,
    build: &BuildSpec,
    tamper: impl FnOnce(&Path),
    tamper_facts: impl FnOnce(&mut Value),
) -> (std::path::PathBuf, std::path::PathBuf) {
    let dir = base.join(name);
    build.write_release(&dir);
    tamper(&dir);
    let mut facts = build.facts(&dir);
    tamper_facts(&mut facts);
    let facts_path = base.join(format!("{name}.facts.json"));
    write_json(&facts_path, &facts);
    (dir, facts_path)
}

// IT-1 (table): each failure refuses `release verify` with that failure listed, and the release
// folder is unchanged. Two failures at once list both.
#[test]
fn inst1_it_1_release_verify_lists_every_failure_and_writes_nothing() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    type BuildChange = Box<dyn Fn(&mut BuildSpec)>;
    type FolderChange = Box<dyn Fn(&Path, &BuildSpec)>;
    type FactsChange = Box<dyn Fn(&mut Value)>;
    type Row = (
        &'static str,
        BuildChange,
        FolderChange,
        FactsChange,
        Option<&'static str>,
        Vec<&'static str>,
    );
    let none_build = || -> BuildChange { Box::new(|_: &mut BuildSpec| {}) };
    let none_folder = || -> FolderChange { Box::new(|_: &Path, _: &BuildSpec| {}) };
    let none_facts = || -> FactsChange { Box::new(|_: &mut Value| {}) };
    let rows: Vec<Row> = vec![
        (
            "zip_sha256_differs_from_sums",
            none_build(),
            Box::new(|dir: &Path, build: &BuildSpec| {
                let other = sha256_hex(b"another zip");
                fs::write(
                    dir.join("SHA256SUMS"),
                    format!(
                        "{other}  {}\n{}  {}\n",
                        build.runtime_zip_name(),
                        sha256_hex(&fs::read(dir.join(build.tools_zip_name())).expect("zip")),
                        build.tools_zip_name()
                    ),
                )
                .expect("rewrite SHA256SUMS");
            }),
            none_facts(),
            None,
            vec!["sums_mismatch"],
        ),
        (
            "zip_sha256_differs_from_the_facts_digest",
            none_build(),
            none_folder(),
            Box::new(|facts: &mut Value| {
                facts["assets"][0]["digest"] = json!(format!("sha256:{}", sha256_hex(b"other")));
            }),
            None,
            vec!["digest_mismatch"],
        ),
        (
            "facts_commit_differs_from_the_zip_commit",
            none_build(),
            none_folder(),
            Box::new(|facts: &mut Value| facts["commit"] = json!(C2)),
            None,
            vec!["facts_commit_mismatch"],
        ),
        (
            "tag_differs_from_expect_tag",
            none_build(),
            none_folder(),
            none_facts(),
            Some("v9.9.9-other"),
            vec!["tag_mismatch"],
        ),
        (
            "manifest_layout_wrong",
            Box::new(|build: &mut BuildSpec| build.runtime.layout = "distribution-v0".to_owned()),
            none_folder(),
            none_facts(),
            None,
            vec!["manifest_layout"],
        ),
        (
            "manifest_repository_wrong",
            Box::new(|build: &mut BuildSpec| build.tools.repository = "neutral/other".to_owned()),
            none_folder(),
            none_facts(),
            None,
            vec!["manifest_repository"],
        ),
        (
            "manifest_commit_wrong",
            Box::new(|build: &mut BuildSpec| build.runtime.commit = C2.to_owned()),
            none_folder(),
            none_facts(),
            None,
            vec!["manifest_commit"],
        ),
        (
            "file_set_differs",
            Box::new(|build: &mut BuildSpec| {
                build
                    .tools
                    .unlisted
                    .push(("unlisted.dll".to_owned(), b"unlisted".to_vec()));
            }),
            none_folder(),
            none_facts(),
            None,
            vec!["file_set"],
        ),
        (
            "file_sha256_differs",
            Box::new(|build: &mut BuildSpec| {
                build.runtime.altered.push("actingctl.exe".to_owned())
            }),
            none_folder(),
            none_facts(),
            None,
            vec!["file_mismatch"],
        ),
        (
            "tools_speaks_differs_from_runtime_speaks",
            Box::new(|build: &mut BuildSpec| build.tools.speaks["ledger"] = json!([3, 3])),
            none_folder(),
            none_facts(),
            None,
            vec!["interfaces_differ"],
        ),
        (
            "two_failures_at_once",
            Box::new(|build: &mut BuildSpec| build.runtime.layout = "distribution-v0".to_owned()),
            none_folder(),
            none_facts(),
            Some("v9.9.9-other"),
            vec!["tag_mismatch", "manifest_layout"],
        ),
    ];
    for (name, change_build, change_folder, change_facts, expect_tag, expected) in rows {
        let mut build = BuildSpec::new(C1);
        change_build(&mut build);
        let (dir, facts) = release(
            temp.path(),
            name,
            &build,
            |dir| change_folder(dir, &build),
            change_facts,
        );
        let before = tree(&dir);
        let mut words = vec![
            "release",
            "verify",
            "--dir",
            word(&dir),
            "--facts",
            word(&facts),
        ];
        if let Some(tag) = expect_tag {
            words.extend(["--expect-tag", tag]);
        }
        let answer = run(&host, &words);
        assert_eq!(
            code(&answer),
            "install_command_refused",
            "{name}: {answer:?}"
        );
        assert_eq!(answer.outcome.category(), Category::Error, "{name}");
        assert_eq!(
            text(&answer, "install_refusal"),
            Some("release_check_failed"),
            "{name}"
        );
        assert_eq!(
            text(&answer, "operation"),
            Some("install_release_verify"),
            "{name}"
        );
        let mut listed = checks(&answer);
        listed.sort();
        let mut expected = expected
            .iter()
            .map(|check| (*check).to_owned())
            .collect::<Vec<_>>();
        expected.sort();
        assert_eq!(listed, expected, "{name}: {answer:?}");
        let rows = answer.members["failures"].as_array().expect("failures");
        for check in &expected {
            assert!(
                rows.iter().any(|row| row["check"] == json!(check)),
                "{name}: {check} is in failures: {rows:?}"
            );
        }
        assert_eq!(
            tree(&dir),
            before,
            "{name}: the release folder is unchanged"
        );
    }
}

// IT-2: without facts `release verify` answers a warning, the caveat `release_unconfirmed`;
// `release stage` refuses without facts or `--local-build`, and writes nothing under the root.
#[test]
fn inst1_it_2_without_facts_verify_warns_and_stage_needs_facts_or_local_build() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let build = BuildSpec::new(C1);
    let dir = temp.path().join("release");
    build.write_release(&dir);

    let verified = run(&host, &["release", "verify", "--dir", word(&dir)]);
    assert_eq!(code(&verified), "install_caveat_noted", "{verified:?}");
    assert_eq!(verified.outcome.category(), Category::Warning);
    assert_eq!(
        text(&verified, "install_caveat"),
        Some("release_unconfirmed")
    );
    assert_eq!(text(&verified, "operation"), Some("install_release_verify"));
    assert_eq!(verified.outcome.value("commit_sha"), Some(&json!(C1)));
    assert_eq!(verified.members["commit"], json!(C1));

    let root = live_root(temp.path(), "root", &BuildSpec::new(C2));
    let before = tree(&root);
    let refused = run(
        &host,
        &[
            "release",
            "stage",
            "--root",
            word(&root),
            "--dir",
            word(&dir),
        ],
    );
    assert_eq!(code(&refused), "install_command_refused", "{refused:?}");
    assert_eq!(
        text(&refused, "install_refusal"),
        Some("release_check_failed")
    );
    assert_eq!(text(&refused, "operation"), Some("install_release_stage"));
    assert_eq!(checks(&refused), ["facts_missing"]);
    assert_eq!(tree(&root), before, "nothing is written under the root");

    let staged = run(
        &host,
        &[
            "release",
            "stage",
            "--root",
            word(&root),
            "--dir",
            word(&dir),
            "--local-build",
        ],
    );
    assert_eq!(code(&staged), "install_command_done", "{staged:?}");
    assert_eq!(text(&staged, "operation"), Some("install_release_stage"));
}

// IT-3: a second stage of a kept, verified build is `unchanged`; a stage stopped after its first
// step (the `.part` folder extracted) is finished by `resume`, and the kept build holds the same
// bytes as an uninterrupted stage of the same release.
#[test]
fn inst1_it_3_a_second_stage_is_unchanged_and_resume_finishes_an_interrupted_stage() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let build = BuildSpec::new(C1);
    let dir = temp.path().join("release");
    build.write_release(&dir);
    let facts = temp.path().join("release.facts.json");
    write_json(&facts, &build.facts(&dir));
    let stage = |root: &Path| {
        run(
            &host,
            &[
                "release",
                "stage",
                "--root",
                word(root),
                "--dir",
                word(&dir),
                "--facts",
                word(&facts),
                "--expect-tag",
                TAG,
            ],
        )
    };

    // Uninterrupted, then again.
    let whole = live_root(temp.path(), "whole", &BuildSpec::new(C2));
    let first = stage(&whole);
    assert_eq!(code(&first), "install_command_done", "{first:?}");
    assert_eq!(text(&first, "operation"), Some("install_release_stage"));
    assert_eq!(first.members["unchanged"], json!(false));
    assert_eq!(
        first.outcome.value("installation_changed"),
        Some(&json!(true))
    );
    let second = stage(&whole);
    assert_eq!(code(&second), "install_command_done", "{second:?}");
    assert_eq!(second.members["unchanged"], json!(true));
    assert_eq!(
        second.outcome.value("installation_changed"),
        Some(&json!(false))
    );
    assert!(second.outcome.value("install_operation_id").is_none());

    // Interrupted after step 1 (extract), then resumed.
    let stopped = live_root(temp.path(), "stopped", &BuildSpec::new(C2));
    host.crash_after.set(Some(1));
    let interrupted = stage(&stopped);
    assert_eq!(
        code(&interrupted),
        "install_operation_interrupted",
        "{interrupted:?}"
    );
    assert_eq!(interrupted.outcome.category(), Category::Fatal);
    assert_eq!(
        text(&interrupted, "operation"),
        Some("install_release_stage")
    );
    let operation_id = interrupted
        .outcome
        .value("install_operation_id")
        .and_then(Value::as_str)
        .expect("the operation id")
        .to_owned();
    assert!(stopped.join("install").join("current.json").is_file());
    let parts = fs::read_dir(stopped.join("install").join("builds"))
        .expect("the builds folder")
        .map(|entry| {
            entry
                .expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    assert!(
        parts
            .iter()
            .any(|name| name.starts_with(C1) && name.ends_with(".part")),
        "the part folder stays: {parts:?}"
    );

    let resumed = run(
        &host,
        &[
            "resume",
            "--root",
            word(&stopped),
            "--operation",
            operation_id.as_str(),
        ],
    );
    assert_eq!(code(&resumed), "install_command_done", "{resumed:?}");
    assert_eq!(text(&resumed, "operation"), Some("install_resume"));
    assert!(!stopped.join("install").join("current.json").exists());
    assert_eq!(
        tree(&stopped.join("install").join("builds").join(C1)),
        tree(&whole.join("install").join("builds").join(C1)),
        "the resumed stage kept the same bytes"
    );
    let state: Value = serde_json::from_slice(
        &fs::read(stopped.join("install").join("state.json")).expect("state.json"),
    )
    .expect("state JSON");
    assert_eq!(state["builds"][0]["commit"], json!(C1), "{state}");
    assert_eq!(state["builds"][0]["origin"], json!("release"), "{state}");
}

// IT-4: `adopt` keeps exactly the listed files and both manifests (an `.old-*` beside them is
// ignored); a listed live file that differs refuses `live_build_modified`; a second adopt of the
// same live build is `unchanged`.
#[test]
fn inst1_it_4_adopt_keeps_the_listed_files_refuses_a_changed_one_and_is_idempotent() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let build = BuildSpec::new(C1);
    let root = live_root(temp.path(), "root", &build);
    fs::write(
        root.join("runtime").join("actingctl.exe.old-1700000000000"),
        b"older",
    )
    .expect("an older program file");

    let adopted = run(&host, &["adopt", "--root", word(&root)]);
    assert_eq!(code(&adopted), "install_command_done", "{adopted:?}");
    assert_eq!(text(&adopted, "operation"), Some("install_adopt"));
    assert_eq!(adopted.members["unchanged"], json!(false));
    assert_eq!(
        adopted.outcome.value("installation_changed"),
        Some(&json!(true))
    );
    let kept = tree(&root.join("install").join("builds").join(C1));
    let mut expected = std::collections::BTreeMap::new();
    for (half, spec) in [("runtime", &build.runtime), ("tools", &build.tools)] {
        for (path, bytes) in spec.entries() {
            expected.insert(format!("{half}/{path}"), bytes);
        }
    }
    assert_eq!(kept, expected, "exactly the listed files and the manifests");
    let state: Value = serde_json::from_slice(
        &fs::read(root.join("install").join("state.json")).expect("state.json"),
    )
    .expect("state JSON");
    assert_eq!(state["live"]["commit"], json!(C1), "{state}");
    assert_eq!(state["builds"][0]["origin"], json!("adopted"), "{state}");
    let config = fs::read(root.join("actingd.config.json")).expect("the configuration");
    assert_eq!(
        state["config_sha256"],
        json!(format!("sha256:{}", sha256_hex(&config))),
        "{state}"
    );

    let again = run(&host, &["adopt", "--root", word(&root)]);
    assert_eq!(code(&again), "install_command_done", "{again:?}");
    assert_eq!(again.members["unchanged"], json!(true));
    assert_eq!(
        again.outcome.value("installation_changed"),
        Some(&json!(false))
    );

    let changed = live_root(temp.path(), "changed", &build);
    fs::write(
        changed.join("tools").join("actingledger.exe"),
        b"changed by hand",
    )
    .expect("change a live file");
    let refused = run(&host, &["adopt", "--root", word(&changed)]);
    assert_eq!(code(&refused), "install_command_refused", "{refused:?}");
    assert_eq!(
        text(&refused, "install_refusal"),
        Some("live_build_modified")
    );
    assert_eq!(text(&refused, "operation"), Some("install_adopt"));
    assert_eq!(refused.outcome.value("count"), Some(&json!(1)));
    assert!(!changed.join("install").exists(), "nothing is written");
}
