// SPDX-License-Identifier: AGPL-3.0-only

// #411 A9 (#411 comment 6252521723, reproduced in comment 6253170525; register §1.101; exec gate
// key 86): `actinglab --json observe --scene <png> --package <package>` without `--verbose`, on a
// frame no page of the package matches, answers `unknown` with exit 0 and its near misses cut to
// fit Min's hard limit, not `observe_identity_limit_exceeded` (page-projection.md: only an
// identity that alone exceeds the hard limit fails). Each page of the package passes a shared
// header and fails an any_of group of five targets, so each near miss names the five.

use super::*;

const SCENE_COLOR: [u8; 3] = [12, 34, 56];
const OTHER_COLOR: [u8; 3] = [200, 200, 200];
const PAGES: [&str; 4] = ["page_a", "page_b", "page_c", "page_d"];
const GROUP: [&str; 5] = [
    "list_header_title",
    "list_header_subtitle",
    "confirm_button_enabled",
    "confirm_button_disabled",
    "close_button_visible",
];

fn color_target(id: &str, expected: [u8; 3]) -> Value {
    json!({
        "type": "color",
        "id": id,
        "region": {"x": 0, "y": 0, "width": 1, "height": 1},
        "expected": expected
    })
}

#[test]
fn a9_offline_observe_of_an_unknown_scene_exits_zero_in_min() {
    let _guard = env_lock();
    let _app_env = set_isolated_app_env();
    set_missing_config_env();
    unsafe {
        env::remove_var(SESSION_STATE_ENV);
    }
    let temp = TempDir::new().unwrap();
    let pack = temp.path().join("pack.json");
    let pages = temp.path().join("pages.json");
    let navigation = temp.path().join("navigation.json");
    let mut targets = vec![
        color_target("shared_header", SCENE_COLOR),
        color_target("entry_page_title", OTHER_COLOR),
    ];
    let mut definitions = vec![json!({"id": "entry_page", "required": ["entry_page_title"]})];
    for page in PAGES {
        let group = GROUP.map(|target| format!("{page}_{target}"));
        targets.extend(group.iter().map(|id| color_target(id, OTHER_COLOR)));
        definitions.push(json!({"id": page, "required": ["shared_header"], "any_of": [group]}));
    }
    fs::write(
        &pack,
        serde_json::to_vec(&json!({
            "schema_version": "0.3",
            "coordinate_space": {"width": 1, "height": 1},
            "targets": targets
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        &pages,
        serde_json::to_vec(&json!({"schema_version": "0.3", "pages": definitions})).unwrap(),
    )
    .unwrap();
    fs::write(&navigation, br#"{"navigation":[]}"#).unwrap();
    let fixture = seal_semantic_fixture(temp, "fixture", "test", &pack, &pages, Some(&navigation));
    let scene = fixture.path().join("scene.png");
    fs::write(&scene, encode_png(1, 1, SCENE_COLOR)).unwrap();
    let observe = |verbose: bool| {
        let mut args = vec![
            "--json".to_string(),
            "observe".to_string(),
            "--scene".to_string(),
            scene.display().to_string(),
            "--package".to_string(),
            fixture.zip.display().to_string(),
        ];
        if verbose {
            args.push("--verbose".to_string());
        }
        run_cli(args, true)
    };

    // The scene: in the Normal view the near misses alone are over 1 KiB, so beside the identity
    // they exceed Min's hard limit.
    let normal = observe(true);
    assert_eq!(normal.exit_code(), 0, "{}", normal.envelope_json());
    let listed = &normal.envelope.data.as_ref().unwrap()["candidates"];
    assert!(serde_json::to_vec(listed).unwrap().len() > 1024);

    let result = observe(false);
    assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
    let data = result.envelope.data.as_ref().unwrap();
    assert_eq!(data["observation"]["state"], "unknown");
    assert!(
        data["candidates"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );
}
