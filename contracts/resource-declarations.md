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
| `operations/*/task.json`, `operations/resources.json` | `pack-containment::source` declaration APIs (an operation carries exactly one effect: `click`, or the `application { action }` effect of `contracts/application-lifecycle.md`) |
| Task truth-set and dictionary JSON | The task owner's `declaration_file_requests` and declaration validation |
| `recognition/*.pack.json` | `load_pack_from_json_str` |
| `recognition/*.pages.json` | `load_page_set_from_json_str` |
| `navigation/*.navigation.json` | `validate_navigation_declarations` |
| `navigation/*.projection.json` | `ProjectionMetadata::parse` |
| `env-detection/detections.json` | `parse_environment_catalog_value` |
| `scheduling/{tasks,pools,activity,timeline}.json` | The production scheduling document parser and schema-version check |
| `scheduling/procedure-manifest.*.json` | Shared `ProcedureBindingConfigFile` / `ScheduledExecutionConfigFile` serde declarations |
| `control.json` | The source control declaration validator and production task-control parser |
| `applications.json` (repository root only) | The actinglab applications table check (`actingcommand.applications.v1`) |
| Parser semantic-mapping and task-facts declarations | The parser's existing declaration-pair rules |

The applications table may carry an optional top-level `label`: the game's display name for people, free text (bilingual `中文 / English` recommended) of 1-128 bytes after trimming, without control characters; consumers fall back to `game` when it is absent.

Each `servers.<server>` entry of the applications table carries `application_id`,
`label` and an optional `default_package_id`. When present, `default_package_id` is a
non-empty string of at most 128 bytes matching `^[a-z0-9]+(\.[a-z0-9_]+)+$`; when
absent the server declares no default package. The validator does not check it against
sealed packs; the resource repository's bundle generator does and fails when the named
package is not sealed.

Navigation declarations share the source validator's required fields, coordinate
representations and click grammar. Drive navigation applies its static-region and
overlap checks when compiling and consuming its execution graph.

The named directories describe conventional resource layouts; explicitly selected
pack/pages/navigation/projection filenames also use their native parsers at the
root. Paths are relative to their actual repository or resource subroot. Nested self-owned
roots are supported without identifying a game in the Runtime. Task validation
reads the corresponding `operations/resources.json` and only the JSON
dependencies requested by the shared declaration parser. MAA pair validation
uses the first operation's declared game, matching the existing parser.

Repository provenance (`manifest.yaml`), `.github` configuration, upstream and
archived material, art catalogs (including historical equipment metadata), and
current authoring/reference files have an
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
file context. Structured declaration errors retain the shared JSON-pointer details.
Validation does not convert resources, build a package, load image/model bytes,
create an evaluator, detect an environment or acquire a Runtime/device holder.
Production keeps its subsequent asset, reference, admission and execution checks.

## Pages and recognition backends in `task.json`

`pack-containment::source` turns the pages a task names (`entry_page`, `target_page`,
`error_pages`, `scheduling_outcome` terminal pages, `post_admission_ocr` pages, and an
operation's `from`, `to` or `expect_after.page_id`) into generated page definitions. A page is
declared by a template anchor (an `anchors[]` entry whose `id` is the page or starts with
`<page>_`) or by a `page_rules.<page>` entry with a non-empty `required`, `optional` or
`any_of`. No recognition backend is mandatory. A page declared only by its rule gets no
implicit `page/<page>` target: its requirements are exactly the rule's targets, in any
combination of `anchors` and `verify_templates` (template), `color_probes` (color or color
digest), `ocr_targets` (OCR) and `checks` (named checks). A page with an anchor keeps its
`page/<page>` requirement (or the `any_of` group of its `<page>_*` variants, which a positive
rule replaces) and adds the rule's targets. Every generated page needs a `required` target or
an `any_of` group; the page detector refuses one without.

A check that combines backends is either one page rule requiring one target per backend,
usually over the same region, or a named check of the `checks` family (below) that a page
rule or an operation guard references as one target. The page matches only when every
`required` target passes its own threshold, every `any_of` group has a passing target and no
`forbidden` target passes:

```json
{
  "color_probes": [
    {"id": "state/claim_ready", "region": {"mode": "rect", "rect": {"x": 1100, "y": 620, "width": 120, "height": 40}},
     "expected": [250, 210, 60]}
  ],
  "ocr_targets": [
    {"id": "text/claim_ready", "region": {"mode": "rect", "rect": {"x": 1100, "y": 620, "width": 120, "height": 40}},
     "languages": ["en"], "timeout_ms": 1000, "match_mode": "contains", "expected": ["Claim"],
     "case_sensitive": false, "minimum_confidence": 0.8,
     "model_ref": "PP-OCRv6_medium", "model_sha256": "<64 lowercase hex digits of the model>"}
  ],
  "page_rules": {
    "claim_ready": {"required": ["state/claim_ready", "text/claim_ready"]}
  }
}
```

| Backend | Declared in | A target passes when |
| --- | --- | --- |
| Template | `anchors[]`, `verify_templates[]` | its match score reaches its `threshold` (default: the task's `defaults.template_threshold`) |
| Template with color | `anchors[].color_check` | one candidate meets the template threshold and the color condition together (`template-relative-color.md`); the color distance is the check's `max_distance`, or the package default when absent |
| Color | `color_probes[]` with `expected` | the mean RGB of its region is within the target's `max_distance` of `expected`; when `max_distance` is absent, the package's `defaults.color_max_distance` applies, taken from the entry task's `defaults` (20 when absent) |
| Color digest | `color_probes[]` with `digest` | the `color_digest.v1` digest of its region is within the declared `max_mean_milli` (and `max_cell`, when declared) of the declared cells (`color-digest.md`); there is no default |
| OCR | `ocr_targets[]` | the recognized text matches one `expected` value under `match_mode` and `case_sensitive`, with confidence at least `minimum_confidence` |
| Check | `checks[]` | `all_of`: every member passes; `any_of`: at least one member passes. Every member is evaluated with its own threshold (`selection-graph.md`, section Checks) |

### Pack schema `0.7` declarations

Task schemas `0.6` through `0.9` accept the following declarations without a schema change.
An older task schema refuses each of them with `UnconsumedField` at its pointer. A source that
uses none of them derives every document byte for byte as before.

| Declaration | Rule |
| --- | --- |
| `checks[]` | `{"id", "all_of": [...]}` or `{"id", "any_of": [...]}`: exactly one of the two, with 2 to 8 distinct member IDs. Each member is a template, color, color digest, OCR or NN target of the derived pack, never another check. |
| `color_probes[].max_distance`, `anchors[].color_check.max_distance` | Optional finite number `>= 0`, the target's own color threshold. |
| `color_probes[].digest` | A color digest entry (`color-digest.md`, section Package declaration). A color probe declares exactly one of `expected` and `digest`; a digest entry has no `max_distance`. |
| `operations[].guard.check` | A string naming the check that guards the input; the guard evaluates its `target_id`, which is that check. |

A check, like a color digest, is never clicked and never locates anything. `guard.check`
requires the guard target to be a `composite` target. `guard.color_probe` accepts a color or a
color digest target. A `target`, `target_center` or `offset` click still requires a template
guard (`guard.verify_template`).

Color digest and check IDs share one namespace with every other target ID. The same ID with
an identical definition, repeated by several tasks, is kept once; any other reuse of the ID
is refused at the digest or check entry that reuses it (`/color_probes/<i>/id`,
`/checks/<i>/id`), whichever family declared the ID first. The older families keep their
first-declaration rule among themselves.

Refusals carry the task's `task.json` and the JSON pointer of the offending field, with the
reason `InvalidValue`, `InvalidType`, `MissingField`, `UnknownField` or `UnconsumedField`. A
check member that is not a target of the derived pack, or is another check, is refused at
`/checks/<i>/all_of/<j>` (or `any_of`). Every check of the selected tasks needs its members in
the same build.

Only `pack.json` is written at schema `0.7`, and only when it holds a `composite` or
`color_digest` target or a per-target `max_distance`. The page set, navigation, operation
index and primitives stay at `0.6`.
