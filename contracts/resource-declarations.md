# Read-only resource declaration validation

Candidate identity, readable integer, consensus and catalog authoring fields use the same
declaration owner; see [business-identity-consensus.md](business-identity-consensus.md).

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
| `operations/*/task.json`, `operations/resources.json` | `pack-containment::source` declaration APIs (an operation carries exactly one effect: `click`, the `application { action }` effect of `contracts/application-lifecycle.md`, or a `select` step of `contracts/selection-graph.md`) |
| Task truth-set and dictionary JSON | The task owner's `declaration_file_requests` and declaration validation |
| Task selection-policy documents (`operations/*/policies/*.json`) | The task owner's `declaration_file_requests` (read category `SelectionPolicy`) and the selection-policy crate's document reading, with the select step's checks (`contracts/selection-graph.md`, section Select step); reported with the family `selection_policy` |
| `recognition/*.pack.json` | `load_pack_from_json_str` |
| `recognition/*.pages.json` | `load_page_set_from_json_str` |
| `navigation/*.navigation.json` | `validate_navigation_declarations` |
| `navigation/*.projection.json` | `ProjectionMetadata::parse` |
| `env-detection/detections.json` | `parse_environment_catalog_value` |
| `scheduling/{tasks,pools,activity,timeline}.json` | The production scheduling document parser and schema-version check |
| `scheduling/procedure-manifest.*.json` | Shared `ProcedureBindingConfigFile` / `ScheduledExecutionConfigFile` serde declarations |
| `control.json` | The source control declaration validator and production task-control parser |
| `applications.json` (repository root only) | The actinglab applications table check (`actingcommand.applications.v1`) |
| `maintenance.json` (repository root only) | Shared `Vec<BundleMaintenance>` decoding and `validate_bundle_maintenance_declarations`; reported with family `maintenance` |
| `pages.json` (repository root only) | The standard-pack page list (`actingcommand.pages.v1`, Workflow #373 NAV-6): the contract crate's `parse_standard_pack_pages`, the Runtime reader's own check; reported with family `standard_pack_pages` |
| `interrupts.json` (repository root only) | The shared interrupt pages (`actingcommand.interrupts.v1`, Workflow #373 NAV-6): `parse_standard_pack_interrupts` against the repository root's `pages.json` (its `game`, its page ids); reported with family `standard_pack_interrupts` |
| Parser semantic-mapping and task-facts declarations | The parser's existing declaration-pair rules |

A task of schema `0.8` or `0.9` may declare the top-level family `resource_readings`
(`contracts/resource-readings.md`); in `0.3`–`0.7` it is `UnconsumedField` @
`/resource_readings`. The declaration gate reports each entry's field pointer
(`/resource_readings/<i>/<field>`); the parser's source validation adds the page and target
cross-references, each naming the entry's field pointer. No task schema, content-directory,
`control.json`, `resources.json` or derived pack/pages version changes.

The applications table may carry an optional top-level `label`: the game's display name for people, free text (bilingual `中文 / English` recommended) of 1-128 bytes after trimming, without control characters; consumers fall back to `game` when it is absent.

Each `servers.<server>` entry of the applications table carries `application_id`,
`label` and an optional `default_package_id`. When present, `default_package_id` is a
non-empty string of at most 128 bytes matching `^[a-z0-9]+(\.[a-z0-9_]+)+$`; when
absent the server declares no default package. The validator does not check it against
sealed packs; the resource repository's bundle generator does and fails when the named
package is not sealed.

The source maintenance array uses exactly `package_id`, `server` and `uses`, with the
shared bundle field types and identifier bounds. An empty array is valid. The contract
owner checks nonempty, distinct supported uses, unique package/server entries and at
most one startup and return-home entry per server. Unknown fields, uses and malformed
values fail. This declaration read uses the existing path/document/aggregate budgets;
it reads no packs. Bundle references, content digests, actual use eligibility and full
prerequisite chains are checked by the formal bundle generator and execution kernel
before generation, as specified in [package-reference.md](package-reference.md).

The standard-pack root files are checked as the Runtime's root reader checks them
(`package-reference.md`, "Standard package root"): shape, version, `game`, page ids, counts
and, for `interrupts.json`, its pages against `pages.json`. The packages they name are
checked against the packed bundle by the bundle generator, as maintenance declarations are.

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
| Template | `anchors[]`, `verify_templates[]` | its match score reaches its `threshold` (default: the task's `defaults.template_threshold`); with `unique`, no second place lies within its margin (section Task schema `0.9` unique matches); with a mask, only the template pixels it selects are scored (section Task schema `0.9` template masks) |
| Template with color | `anchors[].color_check`, `verify_templates[].color_check` (task schema `0.9`) | one candidate meets the template threshold and the color condition together (`template-relative-color.md`); the color distance is the check's `max_distance`, or the package default when absent |
| Color | `color_probes[]` with `expected` | the mean RGB of its region is within the target's `max_distance` of `expected`; when `max_distance` is absent, the package's `defaults.color_max_distance` applies, taken from the entry task's `defaults` (20 when absent) |
| Color digest | `color_probes[]` with `digest` | the `color_digest.v1` digest of its region is within the declared `max_mean_milli` (and `max_cell`, when declared) of the declared cells (`color-digest.md`); there is no default |
| OCR | `ocr_targets[]` | the recognized text matches one `expected` value under `match_mode` and `case_sensitive`, with confidence at least `minimum_confidence` |
| Located text | `ocr_targets[]` with `locate` (task schema `0.9`) | one block read in the region matches one `expected` value under `match_mode` after the identity normalization, with confidence at least `minimum_confidence`, and lies in a declared cell when `cells` is declared (`ocr-fields.md`, section Located text); with `unique`, no other matching block lies within its margin |
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
| `candidate_layouts[]` | `{"id", "page_id", "kind": "fixed_slots", "features": [...], "slots": [...]}`: the candidate layouts of a page the task declares (`selection-graph.md`, section Candidate layouts, Source declaration). Each slot's `rect` and `click` lie inside the task's `coordinate_space`; each slot target is a template, color, color digest, check, OCR or NN target of the derived pack, and a `measure_milli` feature never reads a check. |
| `operations[].select` | `{"layout_id", "policy": {"path": "policies/<name>.json", "sha256"}}`, the operation's only effect, with a declared `guard` of any accepted kind, an `expect_after` and no trusted coordinate (`selection-graph.md`, section Select step). The layout is one the task declares on the step's `from` page; the policy document is read as a selection-policy document and applies to the layout. Its declared `sha256` is format-checked and compared with nothing (Workflow #336 PKG-S1). |

A check, like a color digest, is never clicked and never locates anything. `guard.check`
requires the guard target to be a `composite` target. `guard.color_probe` accepts a color or a
color digest target. A `target` or `offset` click requires a template guard
(`guard.verify_template`) or a located-text guard (`guard.verify_text`, below); a
`target_center` click requires a template guard.

### Task schema `0.9` located text

Workflow #396 REC-1. Task schema `0.9` accepts the following declarations; an older task
schema refuses each with `UnconsumedField` at its pointer. A source that uses neither
derives every document byte for byte as before; one that uses `locate` derives pack schema
`0.7`.

| Declaration | Rule |
| --- | --- |
| `ocr_targets[].locate` | `{"normalize": "identity_v1", "blocks": "detect" \| "region", "line_merge"?, "cells"?}` (`ocr-fields.md`, section Located text). The target is never an OCR field or reading target, an OCR relative anchor, a candidate layout slot target or feature, nor declares `target_consensus`. |
| `operations[].guard.verify_text` | A string naming the guard's own `target_id`, a locate OCR target. The guard evaluates that target; a `target` or `offset` click takes the found cell, or block, as its box; `target_center` is refused. |

### Task schema `0.9` unique matches

Workflow #396 REC-2. Task schema `0.9` accepts `"unique": {"margin_milli": n}` on
`anchors[]`, `verify_templates[]` and `ocr_targets[]` entries; an older task schema refuses it
with `UnconsumedField` at its pointer. It passes to the derived target as declared, and a
source that uses it derives pack schema `0.7`; a source that does not derives every document
byte for byte as before. In the recognition pack, `unique` is a member of a template target or
of an OCR target that declares `locate`; another target, or a pack schema before `0.7`,
refuses it with `UnconsumedField`. `margin_milli` is required (`MissingField`), an integer
(`InvalidType`) from 1 to 1000 (`InvalidValue`); there is no default.

The declaration holds wherever the target is evaluated: page rules, guards, clicks, OCR
anchors, checks and candidate layout slots. A candidate layout feature read at an offset and a
`repeated_anchor` anchor (`candidate-projection.md`) check no second place, so a template that
declares `unique` is refused in either role when the evaluator is built, at the feature's
`target` or the layout's `anchor`. A target that would pass fails when a second place
scores at least the best match's score minus `margin_milli`. Scores are compared in milli,
rounded down: a template's normalized match score under its own metric, a text block's
confidence (0 when absent).

- **Template.** The best match is found by the target's ordinary rules: the highest score,
  with the target's threshold and color check (`template-relative-color.md` for a relative
  color, which picks the highest joint candidate). A second place is a position whose box
  does not intersect the best match's box, that meets the target's own acceptance condition
  (a relative color is checked at each position; an absolute color holds for all positions
  alike) and scores within the margin; a score below the threshold counts when it is within
  the margin. Without a relative color, a margin of 1000 therefore fails whenever the search
  region holds a second box that does not overlap the best one.
- **Located text.** A second place is another block whose text matches an `expected` value
  (`ocr-fields.md`, section Located text), whether its confidence reaches
  `minimum_confidence` or not, and whatever its cell. A detection with more boxes than the
  model folder's `max_boxes` fails the target with `text_read_blocks_truncated`, since the
  dropped boxes could hold a second place.

A second place makes the target fail with `failure` `target_match_not_unique` (`target_id`,
`target_kind` `template` or `ocr`, `score_milli`, `second_score_milli`, `margin_milli`): a
guard refuses its input (`contained_task_guard_refused`, the code as its `cause=` token) and a
page rule does not hold. A target that declares `unique` and otherwise passes carries
`uniqueness` (`score_milli`, `margin_milli`, and `second_score_milli` and `second_rect` when a
second place exists) on its evaluation and on the task diagnostic target row.

A template that declares `unique` is scored at every position of its search region from exact
integer sums, `full_frame` included: the products `sum(T * I)` by an FFT cross-correlation
rounded to integers, or directly when that is cheaper, and the window sums from the exact
integral images. Its scores are bit for bit the direct scorer's; for `ccorr_normed` they can
differ from the default search in the last bit, and a large region can find the true best
that the default coarse search misses, so authors calibrate thresholds again with the same
engine. Admission sets no limit on the search's work. The search keeps the five-second limit;
past it, evaluation fails with `target_match_search_incomplete` (`target_id`, `timeout_ms`)
and the target never passes on a partial search: a guard's evaluation fails
(`contained_task_guard_evaluation_failed`, the code as its `cause=` token). The joint
relative-color search and the `ccoeff_normed` search of a bounded region use the same exact
scores for every target, with their results unchanged.

### Task schema `0.9` template masks, `verify_templates` color checks and the threshold floor

Workflow #396 REC-7. Task schema `0.9` accepts a gray-range mask on `anchors[]` and
`verify_templates[]` entries: the MAA form `"maskRange": [lower, upper]` or the object
`"mask": {"type": "range", "lower", "upper"}`, one of the two, with integer bounds from 0 to 255 and
`lower <= upper` (`InvalidType` or `InvalidValue` at the offending member; `InvalidValue` at
`maskRange` when both forms are declared). An older task schema refuses either with
`UnconsumedField`, and operations still refuse both. Both forms derive the template target's
`"mask": {"type": "range", "lower", "upper"}`, and a source that uses a mask derives pack schema
`0.7`; a source that does not derives every document byte for byte as before. In the
recognition pack, schema `0.7` accepts a template `mask` of `type` `range` only (another `type`
is `InvalidValue` at `/targets/<i>/mask/type`, bounds outside 0..=255 or `lower > upper`
`InvalidValue`); schema `0.6` still refuses every mask with `UnconsumedField`, and schemas `0.1` to
`0.5` keep loading one as a semantic this build cannot evaluate.

A template pixel whose gray value, as every search reads the template, lies in
`lower..=upper` is scored, and every other pixel is ignored. Over the `n` selected pixels,
`ccorr_normed = sum_m(T*I) / sqrt(sum_m(T^2) * sum_m(I^2))` and
`ccoeff_normed = (sum_m(T*I) - sum_m(T) * sum_m(I) / n) / sqrt((sum_m(T^2) - sum_m(T)^2 / n) * (sum_m(I^2) - sum_m(I)^2 / n))`,
0 when the denominator is 0. A masked template is scored on the exact score map (section Task
schema `0.9` unique matches): the three sums stay exact integers, summed directly or by FFT
cross-correlations rounded to integers. Its search keeps the five-second limit; past it,
evaluation fails with `target_match_search_incomplete` at the stage `target_match_exact`
(`target_match_unique` when the target also declares `unique`). The mask holds wherever the
target is evaluated (page rules, guards, clicks, OCR anchors, checks, `target_consensus` and
candidate layout slots) and combines with `color_check` and `unique`. A `repeated_anchor` anchor
and a candidate layout feature read at an offset are searched over every template pixel, so a
masked template is refused in either role when the evaluator is built; template-region batch
evaluation refuses a masked target too.

Task schema `0.9` `verify_templates[]` entries take `color_check` as `anchors[]` entries do:
absolute or `template_relative`, with an optional `max_distance` (`template-relative-color.md`).
An older task schema refuses it with `UnconsumedField`. The derived template target carries the
check, so a guard whose `verify_template` names that target checks the color with the template.

Where a package is admitted with its bytes (the Runtime, `actinglab package digest` and
`actinglab package bundle`), admission reads the template of every target this build can
evaluate; the declaration gate and metadata-only validation read no image.

- A mask selects at least 16 template pixels (a contract constant), and under `ccoeff_normed`
  they are not all of one value.
- Under `ccorr_normed` (the pack's `defaults.match_metric`, or the Runtime default), a threshold
  above 0 is at least the template's flat-field score in milli plus 50
  (`TEMPLATE_FLAT_FIELD_MARGIN_MILLI`, a contract constant). The flat-field score is
  `sum(T) / sqrt(n * sum(T^2))` over the scored pixels (the masked ones when the target declares a
  mask), the score of the template at any region of one non-zero brightness, in milli rounded
  down, and 0 for a template whose scored pixels are all black. A threshold of 0 declares a target
  that always passes and is not checked; `ccoeff_normed` scores a region of one brightness 0 and
  has no floor.

A refusal is `resource_declaration_invalid`, reason `InvalidValue`, at the recognition pack's
`/targets/<i>/mask`, or at the threshold in effect: `/targets/<i>/threshold`, or
`/defaults/template_threshold` when the target declares none. A pack derived from source writes
every template's threshold on its target, so the refusal names the derived pack's target; the
developer text names every template below its floor with its ID, `flat_score_milli` and floor.
This rule refuses packages that were admitted before it: run `actinglab package bundle` over a
resource repository before deploying a build that has it.

Color digest and check IDs share one namespace with every other target ID. The same ID with
an identical definition, repeated by several tasks, is kept once; any other reuse of the ID
is refused at the digest or check entry that reuses it (`/color_probes/<i>/id`,
`/checks/<i>/id`), whichever family declared the ID first. The older families keep their
first-declaration rule among themselves.

Candidate layout IDs have their own namespace. The same layout ID with an identical derived
definition, repeated by several tasks, is kept once; any other reuse is refused at
`/candidate_layouts/<i>/id`.

Refusals carry the task's `task.json` and the JSON pointer of the offending field, with the
reason `InvalidValue`, `InvalidType`, `MissingField`, `UnknownField` or `UnconsumedField`. A
check member that is not a target of the derived pack, or is another check, is refused at
`/checks/<i>/all_of/<j>` (or `any_of`). Every check of the selected tasks needs its members in
the same build. A candidate layout's structure and rectangles are checked by the declaration
gate; its page is refused at `/candidate_layouts/<i>/page_id` when the declaring task does not
declare that page, and a slot target that is not an evaluable target of the derived pack at
`/candidate_layouts/<i>/slots/<k>/targets/<name>`. Every layout of the selected tasks needs its
targets in the same build. The derived layout names its page by the full page-set ID
`<game>/<page>`.

Only `pack.json` is written at schema `0.7`, and only when it holds a `composite` or
`color_digest` target, a per-target `max_distance` or a candidate layout. The page set,
navigation, operation index and primitives stay at `0.6`.

A select step's refusals name the task's `task.json` (`/operations/<i>/select/…`, its `guard`,
`expect_after` or `unguarded_trusted_coordinate`) or the policy document itself (the document, `/applies_to/candidate_layout_id`,
`/fields/<j>/name`, `/fields/<j>/value_type`, `/selection/required_count`). A select step is no
navigation edge and no page operation; its primitive has `click: null`, its declared `guard`
and the `select` object, and the sealed task keeps `select` and the declared guard.
