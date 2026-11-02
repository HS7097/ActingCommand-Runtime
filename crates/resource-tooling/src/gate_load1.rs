// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #407 LOAD-1 (register §1.257; design rd-1031/check-speed/model-loading.md §2, §7
// LOAD-1): a game's `loading.json` (`actingcommand.loading.v1`) and the packs' loading copies.
//
// - The contract's check refuses each broken member of the root file at its JSON pointer.
// - `package loading --write` copies into each pack the pages of its server, only the targets
//   they read and their templates, and writes nothing into an excluded pack.
// - `package loading --check` and `standard-pack check` name every pack whose copy is stale.
// - A page without a template target, a template of one colour, a page id a pack declares itself
//   and a pack of another coordinate space are refused, and nothing is written.
// - `package bundle --loading` refuses a stale pack and otherwise lays `loading.json` and its
//   templates beside `bundle.json`; without `--loading` a pack that carries a copy is refused.
//
// A synthetic `neutral` game, 64x24, servers `test` and `other`. Each pack goes from `home` (a
// blue first pixel) to its own target page (a red one). The root file declares the template
// targets `loading/spinner` and `loading/caption` and the colour target `loading/backdrop`; the
// page `transition` (every server) reads the spinner and the backdrop, the page `download`
// (server `other`) reads the caption.

use crate::{
    PackLoadingState, PackageBundleRequest, PackageLoadingMode, PackageLoadingRequest,
    PackageLoadingResponse, StandardPackCheckRequest, StandardPackRootFiles,
    check_standard_pack_root, package_bundle, package_loading,
};
use actingcommand_contract::{
    LabError, LabResult, StandardPackProblem, parse_standard_pack_loading,
};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const GAME: &str = "neutral";
const ALPHA: &str = "neutral.test.alpha";
const BETA: &str = "neutral.other.beta";
const GAMMA: &str = "neutral.other.gamma";
const ROOT_INVALID: &str = "standard_pack_root_invalid";

// ---- Fixtures.

// One pack: its id, server and coordinate width (the height is 3/8 of it), and the page it goes
// to from `home`.
#[derive(Clone, Copy)]
struct Pack {
    package_id: &'static str,
    server: &'static str,
    width: u32,
    target: &'static str,
}

const fn pack(package_id: &'static str, server: &'static str) -> Pack {
    Pack {
        package_id,
        server,
        width: 64,
        target: "page_b",
    }
}

fn probe(page: &str, expected: [u8; 3]) -> Value {
    json!({
        "id": format!("{page}_colour"),
        "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
        "expected": expected
    })
}

// Writes `pack` as a source package directory.
fn write_pack(directory: &Path, pack: Pack) {
    let height = pack.width * 3 / 8;
    let target = pack.target;
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": pack.package_id,
        "execution_mode": "navigable_route",
        "game": GAME,
        "server": pack.server,
        "resolution": {"width": pack.width, "height": height},
        "entry_task_id": "task",
        "capture_interval_ms": 10,
        "step_timeout_ms": 600,
        "timeout_ms": 8000,
        "max_steps": 2
    });
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": GAME,
        "server_scope": [pack.server],
        "locale": "en-US",
        "goal": "LOAD-1 fixture",
        "coordinate_space": {"width": pack.width, "height": height},
        "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
        "color_probes": [probe("home", [0, 0, 255]), probe(target, [255, 0, 0])],
        "page_rules": {
            "home": {"required": ["home_colour"]},
            target: {"required": [format!("{target}_colour")]}
        },
        "entry_page": "home",
        "target_page": target,
        "operations": [{
            "id": "go", "from": "home", "to": target,
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
        write(&directory.join(path), &pretty(&value));
    }
}

fn region(x: u32) -> Value {
    json!({"mode": "rect", "rect": {"x": x, "y": 0, "width": 8, "height": 8}})
}

// The three targets, as declared.
fn spinner() -> Value {
    json!({"id": "loading/spinner", "template": "loading/spinner.png", "region": region(0),
           "threshold": 0.8})
}

fn caption() -> Value {
    json!({"id": "loading/caption", "template": "loading/caption.png", "region": region(8)})
}

fn backdrop() -> Value {
    json!({"id": "loading/backdrop", "region": region(16), "expected": [8, 8, 8]})
}

// The root file that keeps every rule; `exclude_packages` names GAMMA.
fn valid_root() -> Value {
    json!({
        "schema_version": "actingcommand.loading.v1",
        "game": GAME,
        "coordinate_space": {"width": 64, "height": 24},
        "defaults": {"template_threshold": 0.9, "match_metric": "ccoeff_normed"},
        "max_ms": 30000,
        "anchors": [spinner()],
        "verify_templates": [caption()],
        "color_probes": [backdrop()],
        "pages": [
            {"id": "transition", "required": ["loading/spinner", "loading/backdrop"]},
            {"id": "download", "servers": ["other"], "required": ["loading/caption"]}
        ],
        "exclude_packages": [GAMMA]
    })
}

// An uncompressed RGB PNG of one stored deflate block (as the kernel's gate tests write one).
fn encode_png(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
    let mut scanlines = Vec::new();
    for y in 0..height {
        scanlines.push(0);
        for x in 0..width {
            scanlines.extend_from_slice(&pixel(x, y));
        }
    }
    let length = u16::try_from(scanlines.len()).expect("one stored block");
    let mut zlib = vec![0x78, 0x01, 0x01];
    zlib.extend_from_slice(&length.to_le_bytes());
    zlib.extend_from_slice(&(!length).to_le_bytes());
    zlib.extend_from_slice(&scanlines);
    let (mut a, mut b) = (1_u32, 0_u32);
    for byte in &scanlines {
        a = (a + u32::from(*byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::new();
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &header);
    chunk(&mut png, b"IDAT", &zlib);
    chunk(&mut png, b"IEND", &[]);
    png
}

fn chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    png.extend_from_slice(&(data.len() as u32).to_be_bytes());
    png.extend_from_slice(kind);
    png.extend_from_slice(data);
    let mut crc = 0xffff_ffff_u32;
    for byte in kind.iter().chain(data) {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0_u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    png.extend_from_slice(&(!crc).to_be_bytes());
}

// A spinner of two colours in a checker, and a caption of a gradient.
fn spinner_png() -> Vec<u8> {
    encode_png(8, 8, |x, y| {
        if (x + y) % 2 == 0 {
            [250, 250, 250]
        } else {
            [20, 20, 20]
        }
    })
}

fn caption_png() -> Vec<u8> {
    encode_png(8, 8, |x, y| [(x * 30) as u8, (y * 30) as u8, 90])
}

fn pretty(value: &Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(value).expect("fixture JSON");
    bytes.push(b'\n');
    bytes
}

fn write(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().expect("fixture parent")).expect("fixture directory");
    fs::write(path, bytes).expect("fixture file");
}

// A resource repository: `loading.json` and its templates at its root, one directory per pack
// under `packs/`.
struct Repo {
    _temp: TempDir,
    root: PathBuf,
}

impl Repo {
    fn new(packs: &[Pack], loading: &Value, templates: &[(&str, Vec<u8>)]) -> Self {
        let temp = TempDir::new().expect("temporary directory");
        let root = temp.path().join("repo");
        write(&root.join("loading.json"), &pretty(loading));
        for (name, bytes) in templates {
            write(&root.join("loading").join(name), bytes);
        }
        for pack in packs {
            write_pack(&root.join("packs").join(pack.package_id), *pack);
        }
        Self { _temp: temp, root }
    }

    fn root_file(&self) -> PathBuf {
        self.root.join("loading.json")
    }

    fn pack(&self, package_id: &str) -> PathBuf {
        self.root.join("packs").join(package_id)
    }

    fn run(&self, mode: PackageLoadingMode) -> LabResult<PackageLoadingResponse> {
        package_loading(PackageLoadingRequest {
            root_file: self.root_file(),
            packs_root: self.root.join("packs"),
            mode,
        })
    }
}

fn templates() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("spinner.png", spinner_png()),
        ("caption.png", caption_png()),
    ]
}

// The three packs: ALPHA of `test`, BETA and GAMMA of `other`.
fn three_packs() -> [Pack; 3] {
    [
        pack(ALPHA, "test"),
        pack(BETA, "other"),
        pack(GAMMA, "other"),
    ]
}

// Each issue of a `standard_pack_root_invalid` refusal: its problem, pointer and package (empty
// when the issue is the root file's); `details.outcome` is the first of them.
fn issues(error: &LabError) -> Vec<(String, String, String)> {
    assert_eq!(error.code, ROOT_INVALID, "{}", error.message);
    let details = error.details.as_ref().expect("the refusal's details");
    let listed = details["issues"].as_array().expect("details.issues");
    assert!(!listed.is_empty(), "{}", error.message);
    assert_eq!(
        details["outcome"], listed[0],
        "details.outcome is the first issue"
    );
    listed
        .iter()
        .map(|issue| {
            assert_eq!(issue["code"], ROOT_INVALID);
            let value = |key: &str| issue["values"][key].as_str().unwrap_or("").to_owned();
            assert_eq!(value("game"), GAME);
            (
                value("standard_pack_problem"),
                value("json_pointer"),
                value("package_id"),
            )
        })
        .collect()
}

fn issue(
    problem: StandardPackProblem,
    pointer: &str,
    package_id: &str,
) -> (String, String, String) {
    (
        problem.as_str().to_owned(),
        pointer.to_owned(),
        package_id.to_owned(),
    )
}

// The files of a pack's loading copy, by path inside the pack.
fn copy_files(directory: &Path) -> BTreeSet<String> {
    let mut files = BTreeSet::new();
    if directory.join("resources/loading.json").is_file() {
        files.insert("resources/loading.json".to_owned());
    }
    if let Ok(entries) = fs::read_dir(directory.join("resources/loading")) {
        for entry in entries {
            let name = entry.expect("copy entry").file_name();
            files.insert(format!("resources/loading/{}", name.to_string_lossy()));
        }
    }
    files
}

fn copy_document(directory: &Path) -> Value {
    serde_json::from_slice(&fs::read(directory.join("resources/loading.json")).expect("copy"))
        .expect("copy JSON")
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("copy directory");
    for entry in fs::read_dir(from).expect("read directory") {
        let path = entry.expect("entry").path();
        let target = to.join(path.file_name().expect("entry name"));
        if path.is_dir() {
            copy_tree(&path, &target);
        } else {
            fs::copy(path, target).expect("copy file");
        }
    }
}

// ---- Tests.

// The contract's check of `loading.json`: each broken member is refused with its own problem at
// its pointer, and the valid file reads.
#[test]
fn loading_root_file_refuses_each_broken_member() {
    let valid = valid_root();
    let parsed = parse_standard_pack_loading(&pretty(&valid), Some(GAME)).expect("valid root");
    assert_eq!(parsed.pages.len(), 2);
    assert_eq!(parsed.targets.len(), 3);

    // The valid root with the member at `pointer` set to `value` (added when it is missing).
    let set = |pointer: &str, value: Value| {
        let mut root = valid_root();
        let mut place = &mut root;
        for segment in pointer.split('/').skip(1) {
            place = match segment.parse::<usize>() {
                Ok(index) if place.is_array() => &mut place[index],
                _ => &mut place[segment],
            };
        }
        *place = value;
        pretty(&root)
    };
    let without = |parent: &str, member: &str| {
        let mut root = valid_root();
        root.pointer_mut(parent)
            .and_then(Value::as_object_mut)
            .expect("fixture object")
            .remove(member);
        pretty(&root)
    };
    let many_pages = (0..17)
        .map(|index| json!({"id": format!("page_{index}"), "required": ["loading/spinner"]}))
        .collect::<Vec<_>>();
    let text = String::from_utf8(pretty(&valid)).expect("UTF-8 fixture");
    let repeated = format!("{{\"max_ms\": 1, {}", &text[1..]);
    let mut extra = valid_root();
    extra["extra"] = json!(1);
    use StandardPackProblem::*;
    let cases: Vec<(&str, Vec<u8>, StandardPackProblem, &str)> = vec![
        ("no JSON", b"not json".to_vec(), Malformed, ""),
        (
            "a repeated member",
            repeated.into_bytes(),
            Malformed,
            "/max_ms",
        ),
        ("an unknown member", pretty(&extra), Malformed, "/extra"),
        ("no pages", without("", "pages"), Malformed, "/pages"),
        (
            "another version",
            set("/schema_version", json!("actingcommand.loading.v2")),
            Malformed,
            "/schema_version",
        ),
        (
            "another game",
            set("/game", json!("elsewhere")),
            GameMismatch,
            "/game",
        ),
        (
            "a side of 0",
            set("/coordinate_space/width", json!(0)),
            Malformed,
            "/coordinate_space/width",
        ),
        (
            "a threshold above 1",
            set("/defaults/template_threshold", json!(1.5)),
            Malformed,
            "/defaults/template_threshold",
        ),
        (
            "max_ms 0",
            set("/max_ms", json!(0)),
            LoadingMaxInvalid,
            "/max_ms",
        ),
        (
            "max_ms above the bound",
            set("/max_ms", json!(1_800_001)),
            LoadingMaxInvalid,
            "/max_ms",
        ),
        (
            "an id without the prefix",
            set("/anchors/0/id", json!("spinner")),
            LoadingTargetInvalid,
            "/anchors/0/id",
        ),
        (
            "an id declared twice",
            set("/color_probes/0/id", json!("loading/spinner")),
            LoadingTargetInvalid,
            "/color_probes/0/id",
        ),
        (
            "a template outside loading/",
            set("/anchors/0/template", json!("spinner.png")),
            LoadingTargetInvalid,
            "/anchors/0/template",
        ),
        (
            "a target without a region",
            without("/anchors/0", "region"),
            Malformed,
            "/anchors/0/region",
        ),
        ("no page", set("/pages", json!([])), PagesCount, "/pages"),
        (
            "17 pages",
            set("/pages", json!(many_pages)),
            PagesCount,
            "/pages",
        ),
        (
            "a page id that is no page id",
            set("/pages/0/id", json!("Transition")),
            PageIdInvalid,
            "/pages/0/id",
        ),
        (
            "a page id twice",
            set("/pages/1/id", json!("transition")),
            PageDuplicate,
            "/pages/1/id",
        ),
        (
            "an undeclared target",
            set("/pages/0/forbidden", json!(["loading/missing"])),
            LoadingPageInvalid,
            "/pages/0/forbidden/0",
        ),
        (
            "no server",
            set("/pages/1/servers", json!([])),
            Malformed,
            "/pages/1/servers",
        ),
        (
            "an exclusion twice",
            set("/exclude_packages", json!([GAMMA, GAMMA])),
            Malformed,
            "/exclude_packages/1",
        ),
    ];
    for (case, bytes, problem, pointer) in cases {
        let found = parse_standard_pack_loading(&bytes, Some(GAME))
            .expect_err(case)
            .into_iter()
            .map(|issue| (issue.problem, issue.pointer, issue.file))
            .collect::<Vec<_>>();
        assert!(
            found.contains(&(problem, pointer.to_owned(), "loading.json")),
            "{case}: {found:?}"
        );
    }

    // `package loading` refuses the file with every issue, each `standard_pack_root_invalid`.
    let repo = Repo::new(
        &[pack(ALPHA, "test")],
        &json!({"schema_version": "actingcommand.loading.v1", "game": GAME, "max_ms": 0,
                "coordinate_space": {"width": 64, "height": 24}, "pages": []}),
        &templates(),
    );
    let error = repo
        .run(PackageLoadingMode::Check)
        .expect_err("a broken root file");
    assert_eq!(
        issues(&error),
        vec![
            issue(LoadingMaxInvalid, "/max_ms", ""),
            issue(PagesCount, "/pages", ""),
        ]
    );
}

// `--write` gives a pack of `test` the page of every server, only the targets it reads and their
// templates; `--check` then passes, and a second `--write` writes nothing.
#[test]
fn loading_write_copies_selected_pages_targets_and_templates() {
    let repo = Repo::new(&[pack(ALPHA, "test")], &valid_root(), &templates());
    let written = repo.run(PackageLoadingMode::Write).expect("--write");
    assert_eq!(written.status, "written");
    assert_eq!(written.game, GAME);
    assert_eq!(written.max_ms, 30000);
    assert_eq!(written.packs.len(), 1);
    assert_eq!(written.packs[0].package_id, ALPHA);
    assert_eq!(written.packs[0].pages, ["transition"]);
    assert_eq!(written.packs[0].state, PackLoadingState::Written);

    let alpha = repo.pack(ALPHA);
    assert_eq!(
        copy_document(&alpha),
        json!({
            "schema_version": "actingcommand.pack-loading.v1",
            "coordinate_space": {"width": 64, "height": 24},
            "defaults": {"template_threshold": 0.9, "match_metric": "ccoeff_normed"},
            "max_ms": 30000,
            "anchors": [spinner()],
            "color_probes": [backdrop()],
            "pages": [{"id": "transition", "required": ["loading/spinner", "loading/backdrop"]}]
        })
    );
    assert_eq!(
        copy_files(&alpha),
        BTreeSet::from([
            "resources/loading.json".to_owned(),
            "resources/loading/spinner.png".to_owned()
        ])
    );
    assert_eq!(
        fs::read(alpha.join("resources/loading/spinner.png")).expect("template"),
        spinner_png()
    );

    let checked = repo.run(PackageLoadingMode::Check).expect("--check");
    assert_eq!(checked.status, "checked");
    assert_eq!(checked.packs[0].state, PackLoadingState::Current);
    let again = repo
        .run(PackageLoadingMode::Write)
        .expect("a second --write");
    assert_eq!(again.packs[0].state, PackLoadingState::Current);
}

// A pack of `other` takes both pages and all three targets; an excluded pack takes nothing and
// is left as it is.
#[test]
fn loading_write_selects_pages_by_server_and_skips_excluded_packages() {
    let repo = Repo::new(&three_packs(), &valid_root(), &templates());
    let written = repo.run(PackageLoadingMode::Write).expect("--write");
    let reports = written
        .packs
        .iter()
        .map(|pack| {
            (
                pack.package_id.as_str(),
                pack.server.as_str(),
                pack.pages.clone(),
                pack.excluded,
                pack.state,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        reports,
        vec![
            (
                BETA,
                "other",
                vec!["transition".to_owned(), "download".to_owned()],
                false,
                PackLoadingState::Written
            ),
            (GAMMA, "other", Vec::new(), true, PackLoadingState::Current),
            (
                ALPHA,
                "test",
                vec!["transition".to_owned()],
                false,
                PackLoadingState::Written
            ),
        ]
    );
    let beta = repo.pack(BETA);
    assert_eq!(
        copy_document(&beta),
        json!({
            "schema_version": "actingcommand.pack-loading.v1",
            "coordinate_space": {"width": 64, "height": 24},
            "defaults": {"template_threshold": 0.9, "match_metric": "ccoeff_normed"},
            "max_ms": 30000,
            "anchors": [spinner()],
            "verify_templates": [caption()],
            "color_probes": [backdrop()],
            "pages": [
                {"id": "transition", "required": ["loading/spinner", "loading/backdrop"]},
                {"id": "download", "required": ["loading/caption"]}
            ]
        })
    );
    assert_eq!(
        copy_files(&beta),
        BTreeSet::from([
            "resources/loading.json".to_owned(),
            "resources/loading/caption.png".to_owned(),
            "resources/loading/spinner.png".to_owned()
        ])
    );
    assert_eq!(
        fs::read(beta.join("resources/loading/caption.png")).expect("template"),
        caption_png()
    );
    assert!(copy_files(&repo.pack(GAMMA)).is_empty());
    assert_eq!(
        copy_files(&repo.pack(ALPHA)),
        BTreeSet::from([
            "resources/loading.json".to_owned(),
            "resources/loading/spinner.png".to_owned()
        ])
    );
}

// Three stale packs (a changed copy, a changed template, a copy in an excluded pack) are each
// named once, by `package loading --check` and by `standard-pack check` of the installed root.
#[test]
fn loading_check_names_every_stale_pack() {
    let mut packs = three_packs().to_vec();
    packs.push(pack("neutral.test.delta", "test"));
    let repo = Repo::new(&packs, &valid_root(), &templates());
    repo.run(PackageLoadingMode::Write).expect("--write");
    let alpha = repo.pack(ALPHA);
    let mut changed = copy_document(&alpha);
    changed["max_ms"] = json!(60000);
    write(&alpha.join("resources/loading.json"), &pretty(&changed));
    write(
        &repo.pack(BETA).join("resources/loading/spinner.png"),
        &caption_png(),
    );
    write(
        &repo.pack(GAMMA).join("resources/loading.json"),
        &fs::read(alpha.join("resources/loading.json")).expect("copy"),
    );
    let expected = vec![
        (BETA, "resources/loading/spinner.png"),
        (GAMMA, "resources/loading.json"),
        (ALPHA, "resources/loading.json"),
    ];
    let named = |error: &LabError| {
        let details = error.details.as_ref().expect("details");
        let paths = details["issues"]
            .as_array()
            .expect("issues")
            .iter()
            .map(|issue| issue["values"]["path"].as_str().unwrap_or("").to_owned())
            .collect::<Vec<_>>();
        let found = issues(error);
        assert_eq!(found.len(), expected.len(), "{found:?}");
        for ((package_id, relative), (found, path)) in expected.iter().zip(found.iter().zip(&paths))
        {
            assert_eq!(
                found,
                &issue(StandardPackProblem::LoadingPackStale, "", package_id)
            );
            assert!(
                Path::new(path).ends_with(relative),
                "{path} is not {relative}"
            );
        }
    };
    named(
        &repo
            .run(PackageLoadingMode::Check)
            .expect_err("--check of stale packs"),
    );

    // The installed root of the same packs, with `loading.json` and its templates.
    let installed = TempDir::new().expect("temporary directory");
    let game_root = installed.path().join("packages").join(GAME);
    let mut listed = Vec::new();
    for pack in &packs {
        copy_tree(
            &repo.pack(pack.package_id),
            &game_root.join(pack.package_id),
        );
        listed.push(json!({
            "package_id": pack.package_id, "server": pack.server, "entry_task_id": "task",
            "path": format!("packs/{}", pack.package_id), "file_count": 3, "byte_count": 64
        }));
    }
    write(
        &game_root.join("bundle.json"),
        &pretty(
            &json!({"schema_version": "actingcommand.bundle.v4", "game": GAME,
                        "packs": listed}),
        ),
    );
    copy_tree(&repo.root.join("loading"), &game_root.join("loading"));
    fs::copy(repo.root_file(), game_root.join("loading.json")).expect("root file");
    let error = check_standard_pack_root(StandardPackCheckRequest { root: game_root })
        .expect_err("standard-pack check of stale packs");
    let found = issues(&error)
        .into_iter()
        .map(|(problem, _, package_id)| (problem, package_id))
        .collect::<BTreeSet<_>>();
    assert_eq!(
        found,
        expected
            .iter()
            .map(|(package_id, _)| (
                StandardPackProblem::LoadingPackStale.as_str().to_owned(),
                (*package_id).to_owned()
            ))
            .collect::<BTreeSet<_>>()
    );
}

// A page whose `required` holds only a colour target is refused, and nothing is written.
#[test]
fn loading_refuses_a_page_without_a_template_target() {
    let mut root = valid_root();
    root["pages"][0]["required"] = json!(["loading/backdrop"]);
    let repo = Repo::new(&three_packs(), &root, &templates());
    let error = repo
        .run(PackageLoadingMode::Write)
        .expect_err("a page without a template target");
    assert_eq!(
        issues(&error),
        vec![issue(
            StandardPackProblem::LoadingPageInvalid,
            "/pages/0/required",
            ""
        )]
    );
    for package_id in [ALPHA, BETA, GAMMA] {
        assert!(
            copy_files(&repo.pack(package_id)).is_empty(),
            "{package_id}"
        );
    }
}

// A template whose pixels are all of one colour is refused at its target's `template`, and
// nothing is written.
#[test]
fn loading_refuses_a_single_colour_template() {
    let black = encode_png(8, 8, |_, _| [0, 0, 0]);
    let repo = Repo::new(
        &three_packs(),
        &valid_root(),
        &[("spinner.png", black), ("caption.png", caption_png())],
    );
    let error = repo
        .run(PackageLoadingMode::Write)
        .expect_err("a template of one colour");
    assert_eq!(
        issues(&error),
        vec![issue(
            StandardPackProblem::LoadingTemplateInvalid,
            "/anchors/0/template",
            ""
        )]
    );
    for package_id in [ALPHA, BETA, GAMMA] {
        assert!(
            copy_files(&repo.pack(package_id)).is_empty(),
            "{package_id}"
        );
    }
}

// A pack that declares the page `transition` itself is refused at the page, naming the pack, and
// nothing is written into any pack.
#[test]
fn loading_refuses_a_page_id_a_pack_declares() {
    let own = Pack {
        target: "transition",
        ..pack(ALPHA, "test")
    };
    let repo = Repo::new(
        &[own, pack(BETA, "other"), pack(GAMMA, "other")],
        &valid_root(),
        &templates(),
    );
    let error = repo
        .run(PackageLoadingMode::Write)
        .expect_err("a page the pack declares");
    assert_eq!(
        issues(&error),
        vec![issue(
            StandardPackProblem::LoadingPageCollision,
            "/pages/0/id",
            ALPHA
        )]
    );
    for package_id in [ALPHA, BETA, GAMMA] {
        assert!(
            copy_files(&repo.pack(package_id)).is_empty(),
            "{package_id}"
        );
    }
}

// A pack declared in 32x12 cannot take pages of a 64x24 root file; nothing is written.
#[test]
fn loading_refuses_a_pack_of_another_coordinate_space() {
    let small = Pack {
        width: 32,
        ..pack(ALPHA, "test")
    };
    let repo = Repo::new(
        &[small, pack(BETA, "other"), pack(GAMMA, "other")],
        &valid_root(),
        &templates(),
    );
    let error = repo
        .run(PackageLoadingMode::Check)
        .expect_err("a pack of another coordinate space");
    assert_eq!(
        issues(&error),
        vec![issue(
            StandardPackProblem::LoadingCoordinateMismatch,
            "/coordinate_space",
            ALPHA
        )]
    );
    for package_id in [ALPHA, BETA, GAMMA] {
        assert!(
            copy_files(&repo.pack(package_id)).is_empty(),
            "{package_id}"
        );
    }
}

// `package bundle --loading` lays `loading.json` and its templates beside `bundle.json` byte for
// byte once every copy is current, refuses a stale pack and writes nothing; without `--loading`,
// each pack that carries a copy is refused.
#[test]
fn bundle_with_loading_refuses_a_stale_pack_and_lays_the_root_file() {
    let repo = Repo::new(&three_packs(), &valid_root(), &templates());
    repo.run(PackageLoadingMode::Write).expect("--write");
    let applications = repo.root.join("applications.json");
    write(
        &applications,
        &pretty(&json!({
            "schema_version": "actingcommand.applications.v1",
            "game": GAME,
            "servers": {
                "test": {"application_id": "com.example.neutral", "label": "Test"},
                "other": {"application_id": "com.example.neutral", "label": "Other"}
            }
        })),
    );
    let bundle = |out: &str, loading: bool| {
        package_bundle(PackageBundleRequest {
            applications: applications.clone(),
            packs_root: repo.root.join("packs"),
            out: repo.root.join(out),
            source: None,
            maintenance: None,
            root_files: StandardPackRootFiles {
                loading: loading.then(|| repo.root_file()),
                ..StandardPackRootFiles::default()
            },
        })
    };

    bundle("bundle", true).unwrap_or_else(|error| panic!("{}: {}", error.code, error.message));
    let out = repo.root.join("bundle");
    assert_eq!(
        fs::read(out.join("loading.json")).expect("laid root file"),
        fs::read(repo.root_file()).expect("root file")
    );
    for (name, bytes) in templates() {
        assert_eq!(
            fs::read(out.join("loading").join(name)).expect("laid template"),
            bytes,
            "{name}"
        );
    }
    assert_eq!(
        copy_document(&out.join("packs").join(BETA)),
        copy_document(&repo.pack(BETA))
    );

    let mut changed = copy_document(&repo.pack(BETA));
    changed["pages"] = json!([changed["pages"][0].clone()]);
    write(
        &repo.pack(BETA).join("resources/loading.json"),
        &pretty(&changed),
    );
    let error = bundle("stale", true).expect_err("a stale pack");
    assert_eq!(
        issues(&error),
        vec![issue(StandardPackProblem::LoadingPackStale, "", BETA)]
    );
    let error = bundle("unchecked", false).expect_err("copies without --loading");
    assert_eq!(
        issues(&error),
        vec![
            issue(StandardPackProblem::LoadingPackStale, "", BETA),
            issue(StandardPackProblem::LoadingPackStale, "", ALPHA),
        ]
    );
    for out in ["stale", "stale.part", "unchecked", "unchecked.part"] {
        assert!(!repo.root.join(out).exists(), "{out} was written");
    }
}
