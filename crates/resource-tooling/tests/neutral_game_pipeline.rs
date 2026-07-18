// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_policy::{CatalogDocumentSource, CatalogSources, compile_catalog};
use actingcommand_resource_tooling::{
    AuthoringEnvironmentSnapshot, DEFAULT_MAX_BUFFERED_PAYLOAD_BYTES, PackageBuildTaskRequest,
    PackageEnvOptions, PackageSource, ResourceConvertRequest, open_published_package,
    prepare_package_build_task, resource_convert,
};
use serde_json::json;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

const GAME: &str = "fourth-game";
const SERVER: &str = "test-shard";
const LOCALE: &str = "x-fixture";

#[test]
fn external_neutral_game_metadata_converts_schedules_and_packages() {
    let temp = TempDir::new().expect("temp dir");
    let resource_root = temp.path().join("external-resources");
    write_external_resource_fixture(&resource_root);

    let converted = resource_convert(ResourceConvertRequest {
        repo: resource_root.clone(),
        game: None,
        server: None,
        locale: None,
        maa_tasks_root: None,
        dry_run: false,
    })
    .expect("convert external neutral-game metadata");
    assert_eq!(converted.game, GAME);
    assert_eq!(converted.server, SERVER);
    assert_eq!(converted.locale, LOCALE);

    let catalog = compile_catalog(&neutral_catalog_sources())
        .expect("compile scheduling catalog for external neutral game");
    assert!(catalog.summary().counts.tasks > 0);

    let out = temp.path().join("neutral-game.zip");
    let prepared = prepare_package_build_task(PackageBuildTaskRequest {
        source: PackageSource::Local(resource_root),
        temporary_root: temp.path().join("remote-source"),
        task_id: "return_home".to_string(),
        game: None,
        server: None,
        locale: None,
        package_id: None,
        execution_mode: None,
        resolution: None,
        include_recovery: false,
        out: out.clone(),
        dry_run: false,
        max_buffered_payload_bytes: DEFAULT_MAX_BUFFERED_PAYLOAD_BYTES,
        env: PackageEnvOptions::default(),
    })
    .expect("prepare neutral-game package from external metadata");
    assert_eq!(prepared.game(), GAME);
    assert_eq!(prepared.server(), SERVER);
    let package = prepared
        .build(&AuthoringEnvironmentSnapshot::default())
        .expect("build neutral-game package");
    assert_eq!(package.game, GAME);
    assert_eq!(package.server, SERVER);
    let published = open_published_package(&out).expect("open published neutral-game package");
    assert!(published.path().is_file());
    published.close().expect("close published package");
}

fn write_external_resource_fixture(root: &Path) {
    fs::create_dir_all(root.join("operations/return_home/assets")).expect("operation assets");
    fs::create_dir_all(root.join("navigation")).expect("navigation directory");
    fs::write(
        root.join("operations/resources.json"),
        serde_json::to_vec_pretty(&json!({
            "schema_version": "1.0",
            "resources": [],
            "resource_count": 0
        }))
        .expect("resources json"),
    )
    .expect("write resources");
    fs::write(
        root.join("operations/return_home/assets/HOME.png"),
        one_pixel_png(),
    )
    .expect("write template");
    fs::write(
        root.join("operations/return_home/task.json"),
        serde_json::to_vec_pretty(&json!({
            "schema_version": "0.3",
            "task_id": "return_home",
            "game": GAME,
            "server_scope": [SERVER],
            "locale": LOCALE,
            "goal": "external neutral fixture",
            "coordinate_space": {"width": 1280, "height": 720},
            "defaults": {"template_threshold": 0.9, "color_max_distance": 20.0},
            "anchors": [{
                "id": "home",
                "template": "assets/HOME.png",
                "region": {"mode": "rect", "rect": {"x": 20, "y": 20, "width": 30, "height": 30}},
                "threshold": 0.8,
                "color_check": null
            }],
            "entry_page": "home",
            "target_page": "home",
            "operations": [{
                "id": "home_noop",
                "purpose": "neutral fixture",
                "from": "home",
                "to": null,
                "click": {"kind": "point", "x": 1, "y": 1},
                "verify_template": null,
                "guard": {
                    "page_id": "home",
                    "target_id": "page/home",
                    "expected_rect": {"x": 1, "y": 1, "width": 1, "height": 1},
                    "verify_template": "assets/HOME.png"
                },
                "consumes": [],
                "produces": []
            }]
        }))
        .expect("task json"),
    )
    .expect("write task");
}

fn neutral_catalog_sources() -> CatalogSources {
    CatalogSources {
        tasks: catalog_source(
            "tasks.json",
            include_bytes!("../../../contracts/scheduling/examples/h1-neutral-activity/tasks.json"),
        ),
        pools: catalog_source(
            "pools.json",
            include_bytes!("../../../contracts/scheduling/examples/h1-neutral-activity/pools.json"),
        ),
        activity: catalog_source(
            "activity.json",
            &replace_neutral_game(include_bytes!(
                "../../../contracts/scheduling/examples/h1-neutral-activity/activity.json"
            )),
        ),
        timeline: catalog_source(
            "timeline.json",
            &replace_neutral_game(include_bytes!(
                "../../../contracts/scheduling/examples/h1-neutral-activity/timeline.json"
            )),
        ),
    }
}

fn replace_neutral_game(source: &[u8]) -> Vec<u8> {
    String::from_utf8(source.to_vec())
        .expect("catalog utf-8")
        .replace("neutral-game", GAME)
        .into_bytes()
}

fn catalog_source(name: &str, bytes: &[u8]) -> CatalogDocumentSource {
    CatalogDocumentSource::new(format!("memory://neutral-game/{name}"), bytes.to_vec())
}

fn one_pixel_png() -> &'static [u8] {
    &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6,
        0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 156, 99, 248, 15, 4, 0, 9,
        251, 3, 253, 167, 89, 75, 221, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ]
}
