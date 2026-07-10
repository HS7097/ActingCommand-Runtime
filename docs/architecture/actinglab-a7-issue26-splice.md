# ActingLab A7 Issue 26 Splice Points

Status: A7 move-only record

A7 preserves Issue #26 G2/G3 behavior. It declares the insertion points below but does not add an external trust source, change self-hash behavior, or route semantic verbs through containment.

## Externally Supplied `LoadedBundle`

The future validation entry belongs immediately below `Lab::lab_validate` at `crates/lab/src/lab_run/api.rs:8`. The current path-specific helper starts at `crates/lab/src/lab_run/api.rs:95`; split its post-load work at lines 102-104 into a helper that accepts a containment-created `LoadedBundle`. A future scheduler or trusted-channel caller can then invoke that helper without constructing or re-reading a path.

The future execution entry belongs immediately below `Lab::lab_run` at `crates/lab/src/lab_run/api.rs:4`. The current execution helper starts at `crates/lab/src/lab_run/api.rs:150`; split its post-load work after the `input_unpacked` record and before `lab_control_from_bundle` at line 169 into a helper that accepts the externally supplied bundle plus verified hash provenance. Ledger order must remain `input_unpacked -> control_loaded -> resources_loaded`.

Neither splice may expose a public `LoadedBundle` constructor. The capability must still originate from `Containment::load`.

## Current Containment Path

The current path is exact and remains unchanged:

1. `crates/lab/src/lab_run/api.rs:530` reads the selected zip and enters `load_lab_package_through_containment`.
2. `crates/lab/src/lab_run/api.rs:545` calls `Containment::load` before any control, resource, recognition, or execution parsing.
3. `crates/lab/src/lab_run/bundle.rs:3` parses and validates control from the returned `LoadedBundle`.
4. `crates/lab/src/lab_run/bundle.rs:13` consumes the same bundle for manifest, operation, recognition pack, pages, navigation, and operation assets.

Issue #26 G2 remains unchanged at `crates/lab/src/lab_run/api.rs:541`: when no external expected hash is supplied, the selected local zip bytes still digest themselves before `Containment::load`.

## Future G3 Dispatch

The G3 semantic-verb dispatch insertion point is `crates/lab/src/lab_run/api.rs:207-223`, after `load_lab_resources_from_bundle` has produced the contained control/resources and before device selection, lease acquisition, capture, or input. A future dispatcher can select a typed read or write verb against that contained state without creating another resource loader.

Current scattered-resource compatibility bypasses remain intentionally active:

- `apps/actinglab/src/readonly_cli.rs:19-85` handles `recognize`, `detect-page`, `current-page`, and `is-visible`; `recognition_input_with_config` resolves filesystem inputs through `apps/actinglab/src/main.rs:25471`.
- `apps/actinglab/src/drive_cli.rs:13-55` handles `tap-target` and `navigate` through the same filesystem recognition input; navigation additionally resolves a filesystem path through `apps/actinglab/src/main.rs:7122`.
- `apps/actinglab/src/main.rs:6522` and `apps/actinglab/src/main.rs:9459` keep Session recovery and monitor compatibility on `load_semantic_detector_with_env` (`main.rs:6900`) and `load_navigation_graph` (`main.rs:7026`).

A future G3 change must close these bypasses together or preserve an explicitly documented compatibility boundary. A7 does not alter them.
