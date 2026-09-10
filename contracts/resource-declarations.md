# Read-only resource declaration validation

`actinglab --json resource validate --repo <root> --changed-path <relative-path>`
validates the selected program declarations. `--changed-path` is repeatable.
`--changed-paths-file <relative-path>` accepts the NUL-terminated status/path pairs from
native Git (git diff --name-status --no-renames -z), relative to the same repository root. An empty Git selection is
reported with an empty `entries` array. Each supplied path has a result; unknown
JSON/YAML declaration families fail rather than count as validated.

`actinglab --json operation validate --repo <root> --operation-dir <relative-dir>`
uses the same task and resource-table declaration parsers and retains the existing
`unresolved_coords` safety refusal and operation response envelope. `operation inspect`
and `operation explain` keep their existing behavior.

| Declaration | Existing rule owner |
| --- | --- |
| `operations/*/task.json`, `operations/resources.json` | `pack-containment::source` declaration APIs |
| Task truth-set and dictionary JSON | The task owner's `declaration_file_requests` and declaration validation |
| `recognition/*.pack.json` | `load_pack_from_json_str` |
| `recognition/*.pages.json` | `load_page_set_from_json_str` |
| `navigation/*.navigation.json` | `DriveNavigationGraph::parse_json` |
| `navigation/*.projection.json` | `ProjectionMetadata::parse` |
| `env-detection/detections.json` | `parse_environment_catalog_value` |
| `scheduling/{tasks,pools,activity,timeline}.json` | The production scheduling document parser and schema-version check |
| `scheduling/procedure-manifest.*.json` | Shared `ProcedureBindingConfigFile` / `ScheduledExecutionConfigFile` serde declarations |
| `control.json` | The production task-control parser and declaration validation |
| `tasks/maa-semantic-mapping.json`, `upstream-sync/maa.tasks.json` | The converter's existing declaration-pair rules |

The named directories describe conventional resource layouts; explicitly selected
pack/pages/navigation/projection filenames also use their native parsers at the
root. Paths are relative to their actual repository or resource subroot. Nested self-owned
roots are supported without identifying a game in the Runtime. Task validation
reads the corresponding `operations/resources.json` and only the JSON
dependencies requested by the shared declaration parser. MAA pair validation
uses the first operation's declared game, matching the existing converter.

Repository provenance (`manifest.yaml`), `.github` configuration, upstream and
archived material, art catalogs, and current authoring/reference files have an
explicit `excluded` result. This includes `task.src.json`, split clicks,
components, preparation files, generated operation index/primitives, resource
safety/migration notes, task annotations/catalog, Home-fact templates, recovery
notes and recognition overrides. The formal resource reader consumes the
materialized operation declarations and generated recognition/navigation files.
Adding another program consumer requires mapping its declaration parser.

Only explicit Git D entries are removal requests; missing added/modified files
fail. For removed operation declarations or JSON dependencies, the reader validates
the remaining tasks and their required JSON in the same operation scope.
Remaining scheduling catalogs must retain their four input documents;
remaining projection metadata retains its pack/pages/navigation inputs; a MAA
pair is present together or absent together. Removal results state this local
reference scope. Unmapped removals fail explicitly.

The adapter bounds each selection to 4096 paths, each document to 16 MiB, total
reads to 64 MiB, and the Git path list to 1 MiB. Relative normal paths are required;
links, reparse points, paths outside the root, unreadable files and exceeded
bounds fail. The task owner's smaller dependency bounds remain effective.

Success uses `actinglab.resource-declarations.v1`, with per-path results and the
actual JSON read paths/byte count. Native parser errors retain their reason and
file context. Task declaration errors retain the shared JSON-pointer details.
Validation does not convert resources, build a package, load image/model bytes,
create an evaluator, detect an environment or acquire a Runtime/device holder.
Production keeps its subsequent asset, reference, admission and execution checks.
