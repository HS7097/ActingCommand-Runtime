// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #444 NOTICES-3 (register §1.336; contracts/package-reference.md, "Notices"): the Tools
// read a bundle's notices directory by the contract crate's rules.
// - `package bundle --notices <directory>` lays every file of the directory into
//   `<out>/notices/` byte for byte (a byte-order mark and CRLF line ends kept, a file in a
//   subdirectory, the case of a name kept), lists nothing in `bundle.json` and writes no
//   `interfaces.json`; without the flag its output holds the entries it held before, no
//   `notices`, every file the same.
// - It refuses each rule family it can meet on disk (a name, a size, the count, the total, a file
//   that is not text, a pack named like the directory) with `standard_pack_root_invalid`, the
//   rule's word, the file in the source directory (the directory for the count and the total, the
//   pack's source directory for a pack), an empty pointer and the bound, and a path that is no
//   directory, or a directory that holds no file, with `validation_failed`; each time neither
//   `--out` nor its `.part` exists.
// - `standard-pack check` answers `notices {files, bytes}` for a game root that holds the
//   directory, no such member for one that does not, refuses a broken rule naming the file below
//   the game root, checks a root whose `bundle.json` lists a pack named like the directory (in any
//   ASCII case) as before, with no member, and looks at nothing `place` leaves beside it.
// Not here: the case-folding collisions (two paths equal under ASCII case folding, a file and a
// directory of one name, one directory's name in two cases) cannot be laid on the file system the
// tests run on, which folds case and so holds one entry per folded name; the module's own tests
// (contract gate row "#444 NOTICES-1") hold those rules. No link is made: no test of the pack
// loader's link refusal shows how one is made on the CI's Windows.
// A synthetic `neutral` game, server `test`, 16x9; a notice file is `NOTICE.md`.

use crate::{
    PackageBundleRequest, StandardPackCheckRequest, StandardPackCheckResponse, StandardPackNotices,
    StandardPackRootFiles, check_standard_pack_root, package_bundle,
};
use actingcommand_contract::{LabError, LabResult};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const GAME: &str = "neutral";
const SERVER: &str = "test";
const PACK: &str = "neutral.test.a";
const ROOT_INVALID: &str = "standard_pack_root_invalid";
const VALIDATION_FAILED: &str = "validation_failed";
/// A notice with a byte-order mark and CRLF line ends.
const NOTICE: &[u8] = b"\xef\xbb\xbfPlaceholder notice.\r\nSecond line.\r\n";
/// A text with a TAB and LF line ends.
const OTHER: &[u8] = b"Placeholder text.\nkey\tvalue\n";
/// The largest file the rules keep (2 MiB).
const FILE_BOUND: usize = 2_097_152;

// Writes the pack `package_id` as a package directory: its control, its resource table and its
// one task, from page `home` (a blue first pixel) to page `page_b` (a red one).
fn write_pack(directory: &Path, package_id: &str) {
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": package_id,
        "execution_mode": "navigable_route",
        "game": GAME,
        "server": SERVER,
        "resolution": {"width": 16, "height": 9},
        "entry_task_id": "task",
        "capture_interval_ms": 10,
        "step_timeout_ms": 600,
        "timeout_ms": 8000,
        "max_steps": 2
    });
    let probe = |page: &str, expected: [u8; 3]| {
        json!({
            "id": format!("{page}_colour"),
            "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
            "expected": expected
        })
    };
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": GAME,
        "server_scope": [SERVER],
        "locale": "en-US",
        "goal": "NOTICES-3 fixture",
        "coordinate_space": {"width": 16, "height": 9},
        "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
        "color_probes": [probe("home", [0, 0, 255]), probe("page_b", [255, 0, 0])],
        "page_rules": {
            "home": {"required": ["home_colour"]},
            "page_b": {"required": ["page_b_colour"]}
        },
        "entry_page": "home",
        "target_page": "page_b",
        "operations": [{
            "id": "go", "from": "home", "to": "page_b",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true, "retryable": false
        }]
    });
    for (path, value) in [
        ("control.json", control),
        (
            "resources/operations/resources.json",
            json!({"schema_version": "1.0", "resources": [], "resource_count": 0}),
        ),
        ("resources/operations/task/task.json", task),
    ] {
        let path = directory.join(path);
        fs::create_dir_all(path.parent().expect("pack file parent")).expect("pack directory");
        fs::write(path, serde_json::to_vec_pretty(&value).expect("pack JSON")).expect("pack file");
    }
}

// Writes each `(path, bytes)` below `directory`, a path `/`-separated.
fn write_files(directory: &Path, files: &[(String, Vec<u8>)]) {
    fs::create_dir_all(directory).expect("notices directory");
    for (path, bytes) in files {
        let file = path
            .split('/')
            .fold(directory.to_path_buf(), |parent, segment| {
                parent.join(segment)
            });
        fs::create_dir_all(file.parent().expect("file parent")).expect("file directory");
        fs::write(file, bytes).expect("file");
    }
}

// `(path, bytes)` pairs, owned.
fn owned(files: &[(&str, &[u8])]) -> Vec<(String, Vec<u8>)> {
    files
        .iter()
        .map(|(path, bytes)| ((*path).to_owned(), bytes.to_vec()))
        .collect()
}

// Every file below `directory` by its `/`-separated path, as the file system spells it, with its
// bytes.
fn files_below(directory: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut pending = vec![(directory.to_path_buf(), String::new())];
    while let Some((current, prefix)) = pending.pop() {
        for entry in fs::read_dir(&current).expect("directory listing") {
            let entry = entry.expect("directory entry");
            let name = entry.file_name().to_string_lossy().into_owned();
            let relative = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            if entry.file_type().expect("entry type").is_dir() {
                pending.push((entry.path(), relative));
            } else {
                files.insert(relative, fs::read(entry.path()).expect("file bytes"));
            }
        }
    }
    files
}

// The names of the entries of `directory`.
fn names(directory: &Path) -> BTreeSet<String> {
    fs::read_dir(directory)
        .expect("directory listing")
        .map(|entry| {
            entry
                .expect("directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

// The inputs of one `package bundle`: the applications table, one source directory per
// `(directory name, package_id)` below `<temporary>/sources`, and `--out` `<temporary>/bundle`;
// no root file and no notices directory.
fn bundle_inputs(packs: &[(&str, &str)]) -> (TempDir, PackageBundleRequest) {
    let temp = TempDir::new().expect("temporary directory");
    let applications = temp.path().join("applications.json");
    fs::write(
        &applications,
        serde_json::to_vec_pretty(&json!({
            "schema_version": "actingcommand.applications.v1",
            "game": GAME,
            "servers": {SERVER: {"application_id": "com.example.neutral", "label": "Test"}}
        }))
        .expect("applications JSON"),
    )
    .expect("applications table");
    let packs_root = temp.path().join("sources");
    for (directory, package_id) in packs {
        write_pack(&packs_root.join(directory), package_id);
    }
    let request = PackageBundleRequest {
        applications,
        packs_root,
        out: temp.path().join("bundle"),
        source: None,
        maintenance: None,
        root_files: StandardPackRootFiles::default(),
    };
    (temp, request)
}

// `<out>.part`, where `package bundle` writes before it renames.
fn part(request: &PackageBundleRequest) -> PathBuf {
    request.out.with_file_name("bundle.part")
}

// A refusal's code and the values of the outcome it carries.
fn refusal(error: &LabError) -> (String, Value) {
    let Some(details) = error.details.as_ref() else {
        panic!("{}: {} carries no outcome", error.code, error.message);
    };
    let outcome = &details["outcome"];
    assert_eq!(outcome["code"], error.code.as_str(), "{}", error.message);
    (error.code.clone(), outcome["values"].clone())
}

// An installed game root `<temporary>/packages/neutral/` whose only root file is `bundle.json`,
// listing each pack of `packs`, each in its directory `<game root>/<package_id>`.
fn installed(packs: &[&str]) -> (TempDir, PathBuf) {
    let temp = TempDir::new().expect("temporary directory");
    let game_root = temp.path().join("packages").join(GAME);
    fs::create_dir_all(&game_root).expect("game root");
    for package_id in packs {
        write_pack(&game_root.join(package_id), package_id);
    }
    let listed = packs
        .iter()
        .map(|package_id| {
            json!({
                "package_id": package_id, "server": SERVER, "entry_task_id": "task",
                "path": format!("packs/{package_id}"), "file_count": 3, "byte_count": 64
            })
        })
        .collect::<Vec<_>>();
    fs::write(
        game_root.join("bundle.json"),
        serde_json::to_vec_pretty(&json!({
            "schema_version": "actingcommand.bundle.v4", "game": GAME, "packs": listed
        }))
        .expect("bundle JSON"),
    )
    .expect("bundle index");
    (temp, game_root)
}

fn check(game_root: &Path) -> LabResult<StandardPackCheckResponse> {
    check_standard_pack_root(StandardPackCheckRequest {
        root: game_root.to_owned(),
    })
}

// `package bundle --notices` lays every file of the directory byte for byte into `notices/`, its
// name as the source spells it; the flag adds that directory and nothing else, and without it the
// output's root holds what it held before.
#[test]
fn notices3_package_bundle_lays_the_directory_byte_for_byte() {
    let (temp, request) = bundle_inputs(&[("pack", PACK)]);
    let source = temp.path().join("notices-source");
    let files = owned(&[
        ("NOTICE.md", NOTICE),
        ("texts/Other-Text.TXT", OTHER),
        ("texts/NOTICE", b"x\n"),
    ]);
    write_files(&source, &files);

    let plain_out = temp.path().join("plain");
    let plain = package_bundle(PackageBundleRequest {
        out: plain_out.clone(),
        ..request.clone()
    })
    .unwrap_or_else(|error| panic!("without --notices: {}: {}", error.code, error.message));
    let mut with_notices = request.clone();
    with_notices.root_files.notices = Some(source.clone());
    let laid = package_bundle(with_notices)
        .unwrap_or_else(|error| panic!("with --notices: {}: {}", error.code, error.message));
    assert_eq!(laid.status, "written");

    // Every file, byte for byte, under the name it has in the source, and nothing else.
    assert_eq!(
        files_below(&request.out.join("notices")),
        files.into_iter().collect::<BTreeMap<_, _>>()
    );
    // Without the flag the root holds what a bundle without root files held before; with it,
    // `notices` too, and every other file is the same, `bundle.json` among them.
    assert_eq!(
        names(&plain_out),
        BTreeSet::from(["applications.json", "bundle.json", "packs"].map(str::to_owned))
    );
    assert_eq!(
        names(&request.out),
        BTreeSet::from(["applications.json", "bundle.json", "notices", "packs"].map(str::to_owned))
    );
    let others = files_below(&request.out)
        .into_iter()
        .filter(|(path, _)| !path.starts_with("notices/"))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(others, files_below(&plain_out));
    assert_eq!(
        serde_json::to_value(&laid.index).expect("index JSON"),
        serde_json::to_value(&plain.index).expect("index JSON")
    );
}

// One refused directory: the case, its files, the file the refusal names below it (`None`: the
// directory itself), and the refusal's values beside `game`, `path` and `json_pointer`.
struct Refused {
    case: &'static str,
    files: Vec<(String, Vec<u8>)>,
    file: Option<&'static str>,
    values: Value,
}

// Each rule family a directory on disk can break refuses the bundle with the rule's word, the file
// in the source directory (the directory for the count and the total, the pack's source directory
// for a pack named like the directory), an empty pointer and the bound; nothing is created.
#[test]
fn notices3_package_bundle_refuses_each_broken_rule_and_creates_nothing() {
    let cases = [
        Refused {
            case: "a refused suffix",
            files: owned(&[("NOTICE.html", b"x")]),
            file: Some("NOTICE.html"),
            values: json!({"standard_pack_problem": "notice_name"}),
        },
        Refused {
            case: "a file one byte over its bound",
            files: vec![("NOTICE.md".to_owned(), vec![b'a'; FILE_BOUND + 1])],
            file: Some("NOTICE.md"),
            values: json!({
                "standard_pack_problem": "notice_bytes",
                "limit_bytes": 2_097_152,
                "actual_bytes": 2_097_153
            }),
        },
        Refused {
            case: "65 files",
            files: (0..=64)
                .map(|index| (format!("NOTICE-{index:02}.md"), b"x".to_vec()))
                .collect(),
            file: None,
            values: json!({
                "standard_pack_problem": "notice_count",
                "limit_count": 64,
                "count": 65
            }),
        },
        Refused {
            case: "8 MiB and one byte",
            files: ["a.md", "b.md", "c.md", "d.md"]
                .iter()
                .map(|path| ((*path).to_owned(), vec![b'a'; FILE_BOUND]))
                .chain([("e.md".to_owned(), b"x".to_vec())])
                .collect(),
            file: None,
            values: json!({
                "standard_pack_problem": "notice_total_bytes",
                "limit_bytes": 8_388_608,
                "actual_bytes": 8_388_609
            }),
        },
        Refused {
            case: "a control character",
            files: owned(&[("texts/NOTICE.md", b"one\x00two")]),
            file: Some("texts/NOTICE.md"),
            values: json!({"standard_pack_problem": "notice_not_text"}),
        },
    ];
    for Refused {
        case,
        files,
        file,
        values: mut expected,
    } in cases
    {
        let (temp, mut request) = bundle_inputs(&[("pack", PACK)]);
        let source = temp.path().join("notices-source");
        write_files(&source, &files);
        request.root_files.notices = Some(source.clone());
        let error = package_bundle(request.clone()).expect_err(case);
        let (code, values) = refusal(&error);
        assert_eq!(code, ROOT_INVALID, "{case}: {}", error.message);
        let path = file.map_or_else(
            || source.clone(),
            |file| {
                file.split('/')
                    .fold(source.clone(), |parent, segment| parent.join(segment))
            },
        );
        expected["game"] = json!(GAME);
        expected["path"] = json!(path.to_string_lossy());
        expected["json_pointer"] = json!("");
        assert_eq!(values, expected, "{case}");
        assert!(!request.out.exists(), "{case}: --out was written");
        assert!(!part(&request).exists(), "{case}: its .part was written");
    }

    // A pack named like the directory, in another case: refused at the pack's source directory,
    // with its `package_id`.
    let (temp, mut request) = bundle_inputs(&[("pack", PACK), ("notices-pack", "Notices")]);
    let source = temp.path().join("notices-source");
    write_files(&source, &owned(&[("NOTICE.md", NOTICE)]));
    request.root_files.notices = Some(source);
    let error = package_bundle(request.clone()).expect_err("a pack named like the directory");
    let (code, values) = refusal(&error);
    assert_eq!(code, ROOT_INVALID, "{}", error.message);
    let pack_source = std::path::absolute(&request.packs_root)
        .expect("absolute packs root")
        .join("notices-pack");
    assert_eq!(
        values,
        json!({
            "game": GAME,
            "path": pack_source.to_string_lossy(),
            "standard_pack_problem": "notice_name",
            "json_pointer": "",
            "package_id": "Notices"
        })
    );
    assert!(!request.out.exists(), "a pack: --out was written");
    assert!(!part(&request).exists(), "a pack: its .part was written");
}

// What a case gives as `--notices`, made in the temporary directory it is handed.
type Given = fn(&Path) -> PathBuf;

// A path that names no directory (none at all, or a file) and a directory that holds no file are
// refused as a `--loading` that names no file is, `validation_failed`; nothing is created.
#[test]
fn notices3_package_bundle_refuses_no_directory_and_an_empty_one() {
    let cases: [(&str, Given); 3] = [
        ("no such path", |temp| temp.join("no-such-directory")),
        ("a file", |temp| {
            let file = temp.join("NOTICE.md");
            fs::write(&file, NOTICE).expect("a file");
            file
        }),
        ("a directory without a file", |temp| {
            let directory = temp.join("notices-source");
            fs::create_dir_all(directory.join("texts")).expect("empty directories");
            directory
        }),
    ];
    for (case, given) in cases {
        let (temp, mut request) = bundle_inputs(&[("pack", PACK)]);
        request.root_files.notices = Some(given(temp.path()));
        let error = package_bundle(request.clone()).expect_err(case);
        assert_eq!(error.code, VALIDATION_FAILED, "{case}: {}", error.message);
        assert!(!request.out.exists(), "{case}: --out was written");
        assert!(!part(&request).exists(), "{case}: its .part was written");
    }
}

// `standard-pack check` of a game root without `notices/` has no `notices` member; with it, the
// count of its files and the sum of their lengths, never in `root_files`; what `place` leaves
// beside it is not looked at.
#[test]
fn notices3_check_counts_the_directory_and_has_no_member_without_it() {
    let (_temp, game_root) = installed(&[PACK]);
    let checked =
        check(&game_root).unwrap_or_else(|error| panic!("{}: {}", error.code, error.message));
    assert_eq!(checked.status, "checked");
    assert_eq!(checked.root_files, ["bundle.json"]);
    assert_eq!(checked.notices, None);
    let answer = serde_json::to_value(&checked).expect("the answer");
    assert!(answer.get("notices").is_none(), "{answer}");

    write_files(
        &game_root.join("notices"),
        &owned(&[("NOTICE.md", NOTICE), ("texts/OTHER.txt", OTHER)]),
    );
    for beside in [
        "notices.old-1760000000000",
        "notices.rolledback-1760000000001",
        "notices.new-1760000000002.part",
    ] {
        write_files(&game_root.join(beside), &owned(&[("NOTICE.html", b"\x00")]));
    }
    let checked =
        check(&game_root).unwrap_or_else(|error| panic!("{}: {}", error.code, error.message));
    assert_eq!(checked.status, "checked");
    assert_eq!(checked.pack_count, 1);
    assert_eq!(checked.root_files, ["bundle.json"]);
    let bytes = (NOTICE.len() + OTHER.len()) as u64;
    assert_eq!(
        checked.notices,
        Some(StandardPackNotices { files: 2, bytes })
    );
    let answer = serde_json::to_value(&checked).expect("the answer");
    assert_eq!(answer["notices"], json!({"files": 2, "bytes": bytes}));
}

// A broken rule on disk fails the check as a root file's problem does, naming the file below the
// game root, with the bound where the rule has one.
#[test]
fn notices3_check_refuses_a_broken_rule_naming_the_file() {
    let cases: [(&str, &str, &[u8], Value); 2] = [
        (
            "a control character",
            "texts/NOTICE.md",
            b"one\x00two",
            json!({"standard_pack_problem": "notice_not_text"}),
        ),
        (
            "an empty file",
            "NOTICE.md",
            b"",
            json!({"standard_pack_problem": "notice_bytes", "limit_bytes": 1, "actual_bytes": 0}),
        ),
    ];
    for (case, file, bytes, expected) in cases {
        let (_temp, game_root) = installed(&[PACK]);
        write_files(&game_root.join("notices"), &owned(&[(file, bytes)]));
        let error = check(&game_root).expect_err(case);
        let (code, values) = refusal(&error);
        assert_eq!(code, ROOT_INVALID, "{case}: {}", error.message);
        let path = file.split('/').fold(
            std::path::absolute(&game_root)
                .expect("absolute game root")
                .join("notices"),
            |parent, segment| parent.join(segment),
        );
        let mut expected = expected;
        expected["game"] = json!(GAME);
        expected["path"] = json!(path.to_string_lossy());
        expected["json_pointer"] = json!("");
        assert_eq!(values, expected, "{case}");
    }
}

// A game root whose `bundle.json` lists a pack named like the directory, in any ASCII case, is
// checked as before: the folder is that pack's, admitted as a pack, and the answer has no
// `notices` member.
#[test]
fn notices3_check_leaves_the_folder_of_a_pack_named_like_it() {
    for package_id in ["notices", "NOTICES"] {
        let (_temp, game_root) = installed(&[PACK, package_id]);
        let checked = check(&game_root)
            .unwrap_or_else(|error| panic!("{package_id}: {}: {}", error.code, error.message));
        assert_eq!(checked.status, "checked", "{package_id}");
        assert_eq!(checked.pack_count, 2, "{package_id}");
        assert_eq!(checked.root_files, ["bundle.json"], "{package_id}");
        assert!(
            checked.warnings.is_empty(),
            "{package_id}: {:?}",
            checked.warnings
        );
        assert_eq!(checked.notices, None, "{package_id}");
        let answer = serde_json::to_value(&checked).expect("the answer");
        assert!(answer.get("notices").is_none(), "{package_id}: {answer}");
    }
}
