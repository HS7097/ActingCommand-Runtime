// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_pack_containment::{LoadedBundle, PackageLayout, Sha256Hash, TaskId};

fn main() {
    let _bundle = LoadedBundle {
        task_id: TaskId::new("task").unwrap(),
        verified: Sha256Hash::digest(b"zip"),
        layout: PackageLayout::Lab,
    };
}
