# ActingLab A7 Issue 26 Splice Points

Status: A7 move-only record

A7 preserves Issue #26 G2/G3 behavior. It declares the insertion points below but does not add an external trust source, change self-hash behavior, or route semantic verbs through containment.

## Externally Supplied `LoadedBundle`

The future validation entry belongs immediately below `Lab::lab_validate` at `crates/lab/src/lab_run/api.rs:8`. The current path-specific helper starts at `crates/lab/src/lab_run/api.rs:100`; split its post-load work beginning at line 106 into a helper that accepts a containment-created `LoadedBundle`. A future scheduler or trusted-channel caller can then invoke that helper without constructing or re-reading a path.

The future execution entry belongs immediately below `Lab::lab_run` at `crates/lab/src/lab_run/api.rs:4`. The current execution helper starts at `crates/lab/src/lab_run/api.rs:155`; split its post-load work after the `input_unpacked` record ending at line 171 and before `lab_control_from_bundle` at line 174 into a helper that accepts the externally supplied bundle plus verified hash provenance. Ledger order must remain `input_unpacked -> control_loaded -> resources_loaded`.

Neither splice may expose a public `LoadedBundle` constructor. The capability must still originate from `Containment::load`.

## Current Containment Path

The current path is exact and remains unchanged:

1. `crates/lab/src/lab_run/api.rs:531` enters `load_lab_package_through_containment`, which reads the selected zip at line 536.
2. `crates/lab/src/lab_run/api.rs:546` calls `Containment::load` before any control, resource, recognition, or execution parsing.
3. `crates/lab/src/lab_run/bundle.rs:3` parses and validates control from the returned `LoadedBundle`.
4. `crates/lab/src/lab_run/bundle.rs:12` consumes the same bundle for manifest, operation, recognition pack, pages, navigation, and operation assets.

Issue #26 G2 remains unchanged at `crates/lab/src/lab_run/api.rs:542`: when no external expected hash is supplied, the selected local zip bytes still digest themselves before `Containment::load`.

## Future G3 Dispatch

The G3 semantic-verb dispatch insertion boundary is immediately before `let app_config = ports.config().load()?;` at `crates/lab/src/lab_run/api.rs:228`. At that point `load_lab_resources_from_bundle` has produced the contained resources at line 212 and the `resources_loaded` ledger event has completed at line 226, while device selection, lease acquisition, capture, and input have not begun. A future dispatcher can select a typed read or write verb against that contained state without creating another resource loader.

Current scattered-resource compatibility bypasses remain intentionally active:

- `apps/actinglab/src/readonly_cli.rs:19-74` handles `recognize`, `detect-page`, `current-page`, and `is-visible`; `recognition_input_with_config` starts at line 85 and resolves filesystem inputs through `apps/actinglab/src/main.rs:25471`.
- `apps/actinglab/src/drive_cli.rs:13-76` handles `tap-target` and `navigate` through the same filesystem recognition input; navigation additionally resolves a filesystem path through `apps/actinglab/src/main.rs:7122`.
- `apps/actinglab/src/main.rs:6522` and `apps/actinglab/src/main.rs:9459` keep Session recovery and monitor compatibility on `load_semantic_detector_with_env` (`main.rs:6900`) and `load_navigation_graph` (`main.rs:7026`).

A future G3 change must close these bypasses together or preserve an explicitly documented compatibility boundary. A7 does not alter them.
