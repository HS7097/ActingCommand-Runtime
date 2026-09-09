// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_pack_containment::{LoadedBundle, PackageLayout, TaskId};

fn main() {
    let _bundle = LoadedBundle {
        task_id: TaskId::new("task").unwrap(),
        verified: "a".repeat(64).into(),
        layout: PackageLayout::Lab,
    };
}
