// SPDX-License-Identifier: AGPL-3.0-only

//! Test plan v2.2 EXEC-I6: one fixture package set read by the offline bundle check
//! (resource-tooling, `package bundle --maintenance`) and by the Runtime's admission
//! (runtime-host), so both judge the same packages (E-4 G4b,
//! `contracts/package-reference.md`, "Maintenance declarations (bundle v3)").
//!
//! Every package is a `linear_steps` package of game `neutral`, server `test`, written as an
//! assembled package directory: one click from `home` (first pixel red) to `list` (first pixel
//! blue). A case is a set of such packages, the ones bound as prerequisite packages, and the
//! code with which the shared prerequisite chain refuses the run of [`ROOT`].

use serde_json::{Value, json};
use std::fs;
use std::path::Path;

pub const GAME: &str = "neutral";
pub const SERVER: &str = "test";
/// The package whose run (Runtime) or chain (bundle) each case judges.
pub const ROOT: &str = "neutral.i6.root";
/// The resolution of every package but the one a case makes incompatible.
pub const RESOLUTION: (u32, u32) = (16, 9);

/// One package of a case.
pub struct I6Pack {
    pub package_id: &'static str,
    pub prerequisite: Option<&'static str>,
    pub resolution: (u32, u32),
}

/// A package set, the package ids bound as prerequisite packages, and the code of the refusal.
pub struct I6Case {
    pub name: &'static str,
    pub packs: Vec<I6Pack>,
    pub bound: Vec<&'static str>,
    pub code: &'static str,
}

const fn pack(package_id: &'static str, prerequisite: Option<&'static str>) -> I6Pack {
    I6Pack {
        package_id,
        prerequisite,
        resolution: RESOLUTION,
    }
}

/// The refused chains of the set: an unbound prerequisite, a cycle back to the root, a
/// prerequisite of another resolution, and a fourth declared layer.
pub fn refused_cases() -> Vec<I6Case> {
    vec![
        I6Case {
            name: "unbound",
            packs: vec![
                pack(ROOT, Some("neutral.i6.lead")),
                pack("neutral.i6.lead", None),
            ],
            bound: Vec::new(),
            code: "contained_task_prerequisite_unbound",
        },
        I6Case {
            name: "cycle",
            packs: vec![
                pack(ROOT, Some("neutral.i6.lead")),
                pack("neutral.i6.lead", Some(ROOT)),
            ],
            bound: vec!["neutral.i6.lead", ROOT],
            code: "contained_task_prerequisite_cycle",
        },
        I6Case {
            name: "resolution",
            packs: vec![
                pack(ROOT, Some("neutral.i6.lead")),
                I6Pack {
                    resolution: (32, 18),
                    ..pack("neutral.i6.lead", None)
                },
            ],
            bound: vec!["neutral.i6.lead"],
            code: "contained_task_prerequisite_incompatible",
        },
        I6Case {
            name: "depth",
            packs: vec![
                pack(ROOT, Some("neutral.i6.first")),
                pack("neutral.i6.first", Some("neutral.i6.second")),
                pack("neutral.i6.second", Some("neutral.i6.third")),
                pack("neutral.i6.third", Some("neutral.i6.fourth")),
                pack("neutral.i6.fourth", None),
            ],
            bound: vec![
                "neutral.i6.first",
                "neutral.i6.second",
                "neutral.i6.third",
                "neutral.i6.fourth",
            ],
            code: "contained_task_prerequisite_depth_exceeded",
        },
    ]
}

/// A set whose chain passes: the root and its one bound prerequisite package.
pub fn admitted_case() -> I6Case {
    I6Case {
        name: "admitted",
        packs: vec![
            pack(ROOT, Some("neutral.i6.lead")),
            pack("neutral.i6.lead", None),
        ],
        bound: vec!["neutral.i6.lead"],
        code: "",
    }
}

/// Writes `pack` as an assembled package directory at `directory`.
pub fn write_pack(directory: &Path, pack: &I6Pack) {
    let (width, height) = pack.resolution;
    let mut control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": pack.package_id,
        "execution_mode": "linear_steps",
        "game": GAME,
        "server": SERVER,
        "resolution": {"width": width, "height": height},
        "entry_task_id": "task",
        "capture_interval_ms": 10,
        "step_timeout_ms": 600,
        "timeout_ms": 8000,
        "max_steps": 1
    });
    if let Some(prerequisite) = pack.prerequisite {
        control["prerequisite_package_id"] = json!(prerequisite);
    }
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": GAME,
        "server_scope": [SERVER],
        "coordinate_space": {"width": width, "height": height},
        "entry_page": "home",
        "target_page": "list",
        "max_steps": 1,
        "operations": [{
            "id": "open",
            "from": "home",
            "to": "list",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true
        }]
    });
    let probe = |id: &str, expected: [u8; 3]| {
        json!({"type": "color", "id": id,
               "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": expected})
    };
    let recognition = json!({
        "schema_version": "0.3",
        "game": GAME,
        "server": SERVER,
        "coordinate_space": {"width": width, "height": height},
        "defaults": {"color_max_distance": 0.0},
        "targets": [probe("pixel/first_red", [255, 0, 0]), probe("pixel/first_blue", [0, 0, 255])]
    });
    let page = |page: &str, target: &str| json!({"id": format!("{GAME}/{page}"), "required": [target], "optional": [], "forbidden": []});
    let pages = json!({
        "schema_version": "0.3",
        "pages": [page("home", "pixel/first_red"), page("list", "pixel/first_blue")]
    });
    let files: [(&[&str], Value); 5] = [
        (&["control.json"], control),
        (
            &["resources", "manifest.json"],
            json!({"schema_version": "0.3", "entry_task_id": "task"}),
        ),
        (&["resources", "operations", "task", "task.json"], task),
        (
            &["resources", "recognition", "neutral.test.pack.json"],
            recognition,
        ),
        (
            &["resources", "recognition", "neutral.test.pages.json"],
            pages,
        ),
    ];
    for (segments, value) in files {
        let path = segments
            .iter()
            .fold(directory.to_path_buf(), |parent, segment| {
                parent.join(segment)
            });
        fs::create_dir_all(path.parent().expect("fixture file parent")).expect("fixture directory");
        fs::write(
            path,
            serde_json::to_vec_pretty(&value).expect("fixture JSON"),
        )
        .expect("fixture file");
    }
}

/// Writes every package of `case` below `packs_root`, one directory per package id.
pub fn write_case(packs_root: &Path, case: &I6Case) {
    for pack in &case.packs {
        write_pack(&packs_root.join(pack.package_id), pack);
    }
}
