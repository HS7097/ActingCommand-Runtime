use actingcommand_pack_containment::LoadedBundle;

fn raw_bytes(bundle: &LoadedBundle) {
    let _ = bundle.entry("control.json");
    let _ = bundle.resource_entry("manifest.json");
}

fn main() {}
