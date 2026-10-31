// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #429 R-15b, the release reader's UI side (A.4 b) and its source checks of test E01
// (frozen design `rd-1031/design-r15b-ab-v4.1.md` §1.9, §2.5 and §7 R-15b; contracts/install-cli.md,
// "Release verify and stage"):
// - a UI release (`acui-windows-<commit>.zip` and `SHA256SUMS`) verifies and stages into
//   `install\ui-builds\<commit>\{ui, release.json}`, writes no `state.json`, a second stage is
//   `unchanged` and an interrupted one is finished by `resume` to the same bytes;
// - each failure of a UI release is listed: the facts' repository, the manifest's commit, the
//   file set, a file's digest, a declaration that breaks the parse rules, an
//   `installation_selection_schema` outside the declared `install-selection` range, the facts'
//   digest and commit;
// - a zip the reader recognises as no release zip refuses `zip_set` with its name, and so does a
//   UI zip beside another release zip, each zip named;
// - the Runtime and Tools stage answers and keeps its build as before.
// The machine is the fake host of `fixture.rs`; every root and release folder is a temporary
// folder with neutral files.

use super::fixture::{
    BuildSpec, C1, C2, FakeHost, TAG, code, live_root, run, sha256_hex, tree, word, write_json,
};
use actingcommand_contract::outcome::Category;
use serde_json::{Value, json};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const UI_REPOSITORY: &str = "neutral/ui";
const U1: &str = "3333333333333333333333333333333333333333";

/// A neutral UI build: its manifest members and its files, as the UI build workflow writes them
/// (files at the zip's root, no payload layout, `installation_selection_schema`).
#[derive(Clone)]
struct UiSpec {
    commit: String,
    repository: String,
    interfaces_schema: String,
    speaks: Value,
    selection_schema: Option<String>,
    files: Vec<(String, Vec<u8>)>,
    unlisted: Vec<(String, Vec<u8>)>,
    altered: Vec<String>,
}

impl UiSpec {
    fn new(commit: &str) -> Self {
        Self {
            commit: commit.to_owned(),
            repository: UI_REPOSITORY.to_owned(),
            interfaces_schema: "actingcommand.component-interfaces.v1".to_owned(),
            speaks: json!({
                "actingd-config": [5, 23],
                "install-selection": [2, 2],
                "install-control": [0, 1],
                "ledger": [3, 4],
                "runtime-client": [2, 4],
                "package": [3, 5],
                "runtime-outcomes": [1, 1]
            }),
            selection_schema: Some("actingcommand.install-selection.v2".to_owned()),
            files: vec![
                (
                    "acui.exe".to_owned(),
                    format!("neutral console {commit}").into_bytes(),
                ),
                ("LICENSE".to_owned(), b"neutral licence".to_vec()),
                ("README.md".to_owned(), b"neutral readme".to_vec()),
            ],
            unlisted: Vec::new(),
            altered: Vec::new(),
        }
    }

    fn zip_name(&self) -> String {
        format!("acui-windows-{}.zip", self.commit)
    }

    fn manifest(&self) -> Value {
        let files = self
            .files
            .iter()
            .map(|(path, bytes)| {
                json!({"path": path, "size_bytes": bytes.len(), "sha256": sha256_hex(bytes)})
            })
            .collect::<Vec<_>>();
        let mut manifest = json!({
            "repository": self.repository,
            "commit_sha": self.commit,
            "tree_sha": self.commit,
            "files": files,
            "interfaces": {"schema_version": self.interfaces_schema, "speaks": self.speaks},
        });
        if let Some(schema) = &self.selection_schema {
            manifest["installation_selection_schema"] = json!(schema);
        }
        manifest
    }

    /// Every entry as the zip holds it: the manifest, the listed files (altered ones changed)
    /// and the unlisted ones.
    fn entries(&self) -> Vec<(String, Vec<u8>)> {
        let mut entries = vec![(
            "BUILD-MANIFEST.json".to_owned(),
            serde_json::to_vec_pretty(&self.manifest()).expect("a manifest"),
        )];
        for (path, bytes) in &self.files {
            let mut bytes = bytes.clone();
            if self.altered.contains(path) {
                bytes.extend_from_slice(b" altered");
            }
            entries.push((path.clone(), bytes));
        }
        entries.extend(self.unlisted.iter().cloned());
        entries
    }

    /// A UI release folder: the zip and `SHA256SUMS`.
    fn write_release(&self, dir: &Path) {
        self.write_release_named(dir, &self.zip_name());
    }

    /// A UI release folder whose zip is named `zip_name`, whatever commit the manifest names.
    fn write_release_named(&self, dir: &Path, zip_name: &str) {
        fs::create_dir_all(dir).expect("create the release folder");
        let zip = dir.join(zip_name);
        let mut writer = zip::ZipWriter::new(File::create(&zip).expect("create a zip"));
        for (name, bytes) in self.entries() {
            writer
                .start_file(name, zip::write::FileOptions::default())
                .expect("start a zip entry");
            writer.write_all(&bytes).expect("write a zip entry");
        }
        writer.finish().expect("finish a zip");
        let bytes = fs::read(&zip).expect("read the zip");
        fs::write(
            dir.join("SHA256SUMS"),
            format!("{}  {zip_name}\n", sha256_hex(&bytes)),
        )
        .expect("write SHA256SUMS");
    }

    /// The release-facts document of a UI release folder as it is now.
    fn facts(&self, dir: &Path) -> Value {
        let assets = [self.zip_name(), "SHA256SUMS".to_owned()]
            .iter()
            .map(|name| {
                let bytes = fs::read(dir.join(name)).expect("read a release file");
                json!({"name": name, "size": bytes.len(), "digest": format!("sha256:{}", sha256_hex(&bytes))})
            })
            .collect::<Vec<_>>();
        json!({
            "repository": UI_REPOSITORY,
            "tag": TAG,
            "commit": self.commit,
            "prerelease": false,
            "assets": assets,
        })
    }
}

fn text<'a>(answer: &'a super::Answer, key: &str) -> Option<&'a str> {
    answer.outcome.value(key).and_then(Value::as_str)
}

fn checks(answer: &super::Answer) -> Vec<String> {
    let mut checks = answer
        .outcome
        .value("release_checks")
        .and_then(Value::as_array)
        .map(|checks| {
            checks
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    checks.sort();
    checks
}

/// The `file` of every failure row of check `check`, sorted.
fn failure_files(answer: &super::Answer, check: &str) -> Vec<String> {
    let mut files = answer.members["failures"]
        .as_array()
        .expect("failures")
        .iter()
        .filter(|row| row["check"] == json!(check))
        .filter_map(|row| row["file"].as_str().map(str::to_owned))
        .collect::<Vec<_>>();
    files.sort();
    files
}

/// A UI release folder of `spec` and its facts file, the facts computed from the folder unless
/// `tamper_facts` changes them.
fn ui_release(
    base: &Path,
    name: &str,
    spec: &UiSpec,
    tamper_facts: impl FnOnce(&mut Value),
) -> (PathBuf, PathBuf) {
    let dir = base.join(name);
    spec.write_release(&dir);
    let mut facts = spec.facts(&dir);
    tamper_facts(&mut facts);
    let facts_path = base.join(format!("{name}.facts.json"));
    write_json(&facts_path, &facts);
    (dir, facts_path)
}

fn stage(host: &FakeHost, root: &Path, dir: &Path, facts: &Path) -> super::Answer {
    run(
        host,
        &[
            "release",
            "stage",
            "--root",
            word(root),
            "--dir",
            word(dir),
            "--facts",
            word(facts),
        ],
    )
}

// A UI release verifies; `stage` keeps it under install\ui-builds\<commit>\ with release.json
// naming the member and writes no state.json; a second stage is unchanged; a stage stopped after
// its extraction is finished by resume to the same bytes.
#[test]
fn r15b_a_ui_release_verifies_and_stages_into_ui_builds() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let spec = UiSpec::new(U1);
    let (dir, facts) = ui_release(temp.path(), "ui-release", &spec, |_| {});

    let verified = run(
        &host,
        &[
            "release",
            "verify",
            "--dir",
            word(&dir),
            "--facts",
            word(&facts),
        ],
    );
    assert_eq!(code(&verified), "install_command_done", "{verified:?}");
    assert_eq!(text(&verified, "operation"), Some("install_release_verify"));
    assert_eq!(verified.outcome.value("commit_sha"), Some(&json!(U1)));
    assert_eq!(verified.members["member"], json!("ui"));
    assert_eq!(verified.members["files"], json!(3));
    assert_eq!(
        verified.members["interfaces"]["install-selection"],
        json!([2, 2])
    );

    let root = live_root(temp.path(), "root", &BuildSpec::new(C1));
    let state_before = fs::read(root.join("install").join("state.json")).ok();
    let staged = stage(&host, &root, &dir, &facts);
    assert_eq!(code(&staged), "install_command_done", "{staged:?}");
    assert_eq!(text(&staged, "operation"), Some("install_release_stage"));
    assert_eq!(staged.members["unchanged"], json!(false));
    assert_eq!(staged.members["member"], json!("ui"));
    assert_eq!(
        staged.outcome.value("installation_changed"),
        Some(&json!(true))
    );
    let kept = root.join("install").join("ui-builds").join(U1);
    let mut expected = std::collections::BTreeMap::new();
    for (path, bytes) in spec.entries() {
        expected.insert(format!("ui/{path}"), bytes);
    }
    let mut placed = tree(&kept);
    let record: Value =
        serde_json::from_slice(&placed.remove("release.json").expect("release.json"))
            .expect("release.json is JSON");
    assert_eq!(placed, expected, "exactly the zip's files below ui\\");
    assert_eq!(record["member"], json!("ui"), "{record}");
    assert_eq!(record["commit"], json!(U1), "{record}");
    assert_eq!(
        record["facts"]["repository"],
        json!(UI_REPOSITORY),
        "{record}"
    );
    assert_eq!(
        fs::read(root.join("install").join("state.json")).ok(),
        state_before,
        "state.json v1 lists no UI build"
    );
    assert!(
        !root.join("install").join("builds").join(U1).exists(),
        "a UI build is not a Runtime build"
    );

    let again = stage(&host, &root, &dir, &facts);
    assert_eq!(code(&again), "install_command_done", "{again:?}");
    assert_eq!(again.members["unchanged"], json!(true));
    assert_eq!(
        again.outcome.value("installation_changed"),
        Some(&json!(false))
    );

    let stopped = live_root(temp.path(), "stopped", &BuildSpec::new(C1));
    host.crash_after.set(Some(1));
    let interrupted = stage(&host, &stopped, &dir, &facts);
    assert_eq!(
        code(&interrupted),
        "install_operation_interrupted",
        "{interrupted:?}"
    );
    let operation_id = text(&interrupted, "install_operation_id")
        .expect("the operation id")
        .to_owned();
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
    assert!(!stopped.join("install").join("current.json").exists());
    assert_eq!(
        tree(&stopped.join("install").join("ui-builds").join(U1)),
        tree(&kept),
        "the resumed stage kept the same bytes"
    );
}

// Each failure of a UI release refuses release verify with that failure listed, and the folder
// is unchanged.
#[test]
fn r15b_each_failure_of_a_ui_release_is_listed() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    type SpecChange = Box<dyn Fn(&mut UiSpec)>;
    type FactsChange = Box<dyn Fn(&mut Value)>;
    let no_spec = || -> SpecChange { Box::new(|_: &mut UiSpec| {}) };
    let no_facts = || -> FactsChange { Box::new(|_: &mut Value| {}) };
    let rows: Vec<(&str, SpecChange, FactsChange, Vec<&str>)> = vec![
        (
            "the_manifest_names_another_repository_than_the_facts",
            Box::new(|spec: &mut UiSpec| spec.repository = "neutral/other".to_owned()),
            no_facts(),
            vec!["manifest_repository"],
        ),
        (
            "the_manifest_names_another_commit",
            Box::new(|spec: &mut UiSpec| spec.commit = C2.to_owned()),
            no_facts(),
            vec!["manifest_commit"],
        ),
        (
            "the_zip_holds_an_unlisted_file",
            Box::new(|spec: &mut UiSpec| {
                spec.unlisted
                    .push(("unlisted.dll".to_owned(), b"unlisted".to_vec()))
            }),
            no_facts(),
            vec!["file_set"],
        ),
        (
            "a_file_differs_from_the_manifest",
            Box::new(|spec: &mut UiSpec| spec.altered.push("acui.exe".to_owned())),
            no_facts(),
            vec!["file_mismatch"],
        ),
        (
            "the_declaration_breaks_the_parse_rules",
            Box::new(|spec: &mut UiSpec| {
                spec.interfaces_schema = "actingcommand.component-interfaces.v0".to_owned()
            }),
            no_facts(),
            vec!["interfaces_invalid"],
        ),
        (
            "the_declaration_lacks_a_v1_interface",
            Box::new(|spec: &mut UiSpec| {
                spec.speaks
                    .as_object_mut()
                    .expect("speaks")
                    .remove("ledger");
            }),
            no_facts(),
            vec!["interfaces_invalid"],
        ),
        (
            "the_selection_schema_is_outside_the_declared_range",
            Box::new(|spec: &mut UiSpec| {
                spec.selection_schema = Some("actingcommand.install-selection.v1".to_owned())
            }),
            no_facts(),
            vec!["selection_schema_mismatch"],
        ),
        (
            "the_range_is_outside_the_selection_schema",
            Box::new(|spec: &mut UiSpec| spec.speaks["install-selection"] = json!([1, 1])),
            no_facts(),
            vec!["selection_schema_mismatch"],
        ),
        (
            "the_manifest_names_no_selection_schema",
            Box::new(|spec: &mut UiSpec| spec.selection_schema = None),
            no_facts(),
            vec!["selection_schema_mismatch"],
        ),
        (
            "the_zip_differs_from_its_facts_digest",
            no_spec(),
            Box::new(|facts: &mut Value| {
                facts["assets"][0]["digest"] = json!(format!("sha256:{}", sha256_hex(b"other")));
            }),
            vec!["digest_mismatch"],
        ),
        (
            "the_facts_name_another_commit",
            no_spec(),
            Box::new(|facts: &mut Value| facts["commit"] = json!(C2)),
            vec!["facts_commit_mismatch"],
        ),
    ];
    for (name, change_spec, change_facts, expected) in rows {
        let mut spec = UiSpec::new(U1);
        change_spec(&mut spec);
        // The zip and the facts keep the commit U1, so a manifest of another commit is the
        // manifest's own mismatch.
        let named = UiSpec::new(U1);
        let dir = temp.path().join(name);
        spec.write_release_named(&dir, &named.zip_name());
        let mut facts = named.facts(&dir);
        change_facts(&mut facts);
        let facts_path = temp.path().join(format!("{name}.facts.json"));
        write_json(&facts_path, &facts);
        let before = tree(&dir);
        let answer = run(
            &host,
            &[
                "release",
                "verify",
                "--dir",
                word(&dir),
                "--facts",
                word(&facts_path),
            ],
        );
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
        let mut expected = expected
            .iter()
            .map(|check| (*check).to_owned())
            .collect::<Vec<_>>();
        expected.sort();
        assert_eq!(checks(&answer), expected, "{name}: {answer:?}");
        assert_eq!(
            tree(&dir),
            before,
            "{name}: the release folder is unchanged"
        );
    }
}

// A zip the reader recognises as no release zip refuses zip_set with its name (beside a Runtime
// pair, beside a UI zip, and at stage, which writes nothing); a UI zip beside a Runtime pair, and
// two UI zips, refuse zip_set naming every zip.
#[test]
fn r15b_an_unrecognised_zip_refuses_zip_set_by_its_name() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let verify = |dir: &Path| run(&host, &["release", "verify", "--dir", word(dir)]);
    let assert_zip_set = |answer: &super::Answer, names: &[&str], case: &str| {
        assert_eq!(
            code(answer),
            "install_command_refused",
            "{case}: {answer:?}"
        );
        assert_eq!(
            text(answer, "install_refusal"),
            Some("release_check_failed"),
            "{case}"
        );
        assert_eq!(checks(answer), ["zip_set"], "{case}: {answer:?}");
        let mut names = names
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(failure_files(answer, "zip_set"), names, "{case}");
    };

    let build = BuildSpec::new(C1);
    let pair = temp.path().join("pair-and-other");
    build.write_release(&pair);
    fs::write(pair.join("notes.zip"), b"not a release zip").expect("an extra zip");
    assert_zip_set(&verify(&pair), &["notes.zip"], "beside a Runtime pair");

    let ui = UiSpec::new(U1);
    let beside_ui = temp.path().join("ui-and-other");
    ui.write_release(&beside_ui);
    fs::write(beside_ui.join("acui-windows-short.zip"), b"no commit").expect("an extra zip");
    fs::write(beside_ui.join("EXTRA.ZIP"), b"upper case").expect("an extra zip");
    assert_zip_set(
        &verify(&beside_ui),
        &["EXTRA.ZIP", "acui-windows-short.zip"],
        "beside a UI zip",
    );

    let mixed = temp.path().join("pair-and-ui");
    build.write_release(&mixed);
    let ui_dir = temp.path().join("ui-only");
    ui.write_release(&ui_dir);
    fs::copy(ui_dir.join(ui.zip_name()), mixed.join(ui.zip_name())).expect("copy the UI zip");
    assert_zip_set(
        &verify(&mixed),
        &[
            build.runtime_zip_name().as_str(),
            build.tools_zip_name().as_str(),
            ui.zip_name().as_str(),
        ],
        "a UI zip beside a Runtime pair",
    );

    let two = temp.path().join("two-ui");
    ui.write_release(&two);
    let other = UiSpec::new(C2);
    fs::copy(ui_dir.join(ui.zip_name()), two.join(other.zip_name())).expect("a second UI zip");
    assert_zip_set(
        &verify(&two),
        &[ui.zip_name().as_str(), other.zip_name().as_str()],
        "two UI zips",
    );

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
            word(&pair),
            "--local-build",
        ],
    );
    assert_zip_set(&refused, &["notes.zip"], "stage");
    assert_eq!(text(&refused, "operation"), Some("install_release_stage"));
    assert_eq!(tree(&root), before, "stage writes nothing");
}

// The Runtime and Tools stage answers as before: no member `member`, its release.json without
// one, and its build recorded in state.json.
#[test]
fn r15b_the_runtime_and_tools_stage_answers_and_keeps_its_build_as_before() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let build = BuildSpec::new(C1);
    let dir = temp.path().join("release");
    build.write_release(&dir);
    let facts = temp.path().join("release.facts.json");
    write_json(&facts, &build.facts(&dir));
    let root = live_root(temp.path(), "root", &BuildSpec::new(C2));
    let staged = stage(&host, &root, &dir, &facts);
    assert_eq!(code(&staged), "install_command_done", "{staged:?}");
    assert!(staged.members.get("member").is_none(), "{staged:?}");
    let record: Value = serde_json::from_slice(
        &fs::read(
            root.join("install")
                .join("builds")
                .join(C1)
                .join("release.json"),
        )
        .expect("release.json"),
    )
    .expect("release.json is JSON");
    assert!(record.get("member").is_none(), "{record}");
    let state: Value = serde_json::from_slice(
        &fs::read(root.join("install").join("state.json")).expect("state.json"),
    )
    .expect("state JSON");
    assert_eq!(state["builds"][0]["commit"], json!(C1), "{state}");
    assert!(!root.join("install").join("ui-builds").exists());
}
