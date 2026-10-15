# Read-only resource declaration validation

Candidate identity, readable integer, consensus and catalog authoring fields use the same
declaration owner; see [business-identity-consensus.md](business-identity-consensus.md).

`actinglab --json resource validate --repo <root> --changed-path <relative-path>`
validates the selected program declarations. `--changed-path` is repeatable.
`--changed-paths-file <relative-path>` accepts the NUL-terminated status/path pairs from
native Git (git diff --name-status --no-renames -z), relative to the same repository root. An empty Git selection is
reported with an empty `entries` array. Each supplied path has a result; unknown
JSON/YAML declaration families fail rather than count as validated, except inside a directory
the repository declares in `declaration-exclusions.json` (below).

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
| `declaration-exclusions.json` (repository root only) | Resource tooling's `DeclarationExclusions::parse` (`actingcommand.declaration-exclusions.v1`, Workflow #396 TOOL-2); reported with family `declaration_exclusions` |
| `interrupts.json` (repository root only) | The shared interrupt pages (`actingcommand.interrupts.v1`, Workflow #373 NAV-6): `parse_standard_pack_interrupts` against the repository root's `pages.json` (its `game`, its page ids); reported with family `standard_pack_interrupts` |
| Parser semantic-mapping and task-facts declarations | The parser's existing declaration-pair rules |

A task of schema `0.8` or `0.9` may declare the top-level family `resource_readings`
(`contracts/resource-readings.md`); in `0.3`–`0.7` it is `UnconsumedField` @
`/resource_readings`. The declaration gate reports each entry's field pointer
(`/resource_readings/<i>/<field>`); the parser's source validation adds the page and target
cross-references, each naming the entry's field pointer. Workflow #384 D2-S3: the family
`resource_adjustments` follows the same gate (`UnconsumedField` @ `/resource_adjustments` before
`0.8`; field pointers `/resource_adjustments/<i>/<field>`, an amount's unknown member
`UnknownField` @ `/resource_adjustments/<i>/amount/<member>`), and the source validation adds the
amount's target cross-reference. No task schema, content-directory, `control.json`,
`resources.json` or derived pack/pages version changes.

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

Workflow #390 LAB-2 (forced-compromises mapping T-03): the generated navigation, the
projection of a task's click operations, also collects an operation whose `to` is a page set,
as an edge `{"id", "from_page", "to_pages": [...], "click", "source"}` with every page it may
reach (game-qualified, in name order) instead of one `to_page`. An edge names exactly one of
`to_page` (a string) and `to_pages` (at least one string): both are `UnknownField` at
`to_pages`, an empty set `InvalidValue`, neither `MissingField` at `to_page`. Such an edge is a
navigation element of the page projection like any other, and no route passes it (the drive
graph and actinglab's navigation tools leave it out), since its click does not decide which page
comes. A `long_press` navigation click is read as a drag that does not move (one point held for
its `duration_ms`).

Workflow #390 lpbox (coordinator ruling 6 of wave 16, from Alice's rule of 10-09 that pack taps
use click boxes, never fixed points): a `long_press` (alias `long_tap`) click, in a task
operation and in a navigation edge, is either a click box `{"kind", "x", "y", "width",
"height", "duration_ms"}` or, as before, a point `{"kind", "x", "y", "duration_ms"}`. A box
names both `width` and `height` (one without the other is `MissingField` at the absent one),
each at least 1, and lies inside the task's resolution. The kernel holds a point of the box for
`duration_ms`, drawn with the step's action seed as a `rect` click's tap point is, and records
the seed and the box as the effect intent's one sampling region (`xorshift64_uniform_rect_v1`);
without a seed it holds the box's center. A point long press holds its point and records no
sampling. Drive navigation and actinglab's navigation tools hold a box's center, as they tap a
`rect` click's; a guard synthesized from the click takes the box as its `expected_rect`. The
whole box must be harmless on every frame the press can fire on: an author sizes it to the
pressed control, and declares a point only for a target without a stable extent.

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

A repository names the directories of its own non-declaration material, such as its
regression case families, in the root file `declaration-exclusions.json` (Workflow #396
TOOL-2):

```json
{"schema_version": "actingcommand.declaration-exclusions.v1", "directories": ["regression"]}
```

No other member is accepted. `directories` holds 1-64 distinct repository-relative
directories, each written as `/`-separated normal segments (no empty, `.` or `..` segment, no
leading or trailing `/`, no `\`, `:` or control character) of at most 1024 bytes. A selected
path inside one of them, at any depth and compared segment by segment as Git spells it, to
which no declaration parser is mapped (a path that would otherwise fail with "no mapped
parser", or on removal with "no mapped current-reference check") has the `excluded` result
with reason `repository_declared_exclusion`. A mapped family inside a declared directory (for
example a `*.pack.json` or a `control.json`) is still validated, and an unmapped JSON/YAML
file outside every declared directory still fails. The declaration is read from the selected
tree once per run, when the first unmapped path needs it: a repository without it declares
nothing, and a declaration that does not parse fails that path with
`resource_declaration_invalid` and the declaration's own issue (`declaration_file`
`declaration-exclusions.json`, its JSON pointer and reason). Its removal needs no other
document.

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
`forbidden` target passes. Located text, value tests and checks holding either are evaluated
lazily (section Task schema `0.9` value tests); the match is the one a full evaluation gives:

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
| Template | `anchors[]`, `verify_templates[]` | its match score reaches its `threshold` (default: the task's `defaults.template_threshold`); with `unique`, no second place lies within its margin (section Task schema `0.9` unique matches); with a mask, only the template pixels it selects are scored (section Task schema `0.9` template masks); with `scales`, the template is searched resampled by each declared factor (section Task schema `0.9` multi-scale templates) |
| Template with color | `anchors[].color_check`, `verify_templates[].color_check` (task schema `0.9`) | one candidate meets the template threshold and the color condition together (`template-relative-color.md`); the color distance is the check's `max_distance`, or the package default when absent |
| Color | `color_probes[]` with `expected` | the mean RGB of its region is within the target's `max_distance` of `expected`; when `max_distance` is absent, the package's `defaults.color_max_distance` applies, taken from the entry task's `defaults` (20 when absent) |
| Color digest | `color_probes[]` with `digest` | the `color_digest.v1` digest of its region is within the declared `max_mean_milli` (and `max_cell`, when declared) of the declared cells (`color-digest.md`); there is no default |
| OCR | `ocr_targets[]` | the recognized text matches one `expected` value under `match_mode` and `case_sensitive`, with confidence at least `minimum_confidence` |
| Located text | `ocr_targets[]` with `locate` (task schema `0.9`) | one block read in the region matches one `expected` value under `match_mode` after the identity normalization, with confidence at least `minimum_confidence`, and lies in a declared cell when `cells` is declared (`ocr-fields.md`, section Located text); with `unique`, no other matching block lies within its margin |
| Check | `checks[]` | `all_of`: every member passes; `any_of`: at least one member passes. Every member is evaluated with its own threshold (`selection-graph.md`, section Checks) |
| Value test | `checks[]` with `value_test` (task schema `0.9`) | both sides read, and the left value compared with the right one holds; `parses` needs only the left side to read (section Task schema `0.9` value tests) |

### Pack schema `0.7` declarations

Task schemas `0.6` through `0.9` accept the following declarations without a schema change.
An older task schema refuses each of them with `UnconsumedField` at its pointer. A source that
uses none of them derives every document byte for byte as before.

| Declaration | Rule |
| --- | --- |
| `checks[]` | `{"id", "all_of": [...]}` or `{"id", "any_of": [...]}`: exactly one of the two, with 2 to 8 distinct member IDs. Each member is a template, color, color digest, OCR or NN target or a value test of the derived pack, never another `all_of` or `any_of` check. Task schema `0.9` also accepts `{"id", "value_test": {...}}` (section Task schema `0.9` value tests). |
| `color_probes[].max_distance`, `anchors[].color_check.max_distance` | Optional finite number `>= 0`, the target's own color threshold. |
| `color_probes[].digest` | A color digest entry (`color-digest.md`, section Package declaration). A color probe declares exactly one of `expected` and `digest`; a digest entry has no `max_distance`. |
| `operations[].guard.check` | A string naming the check, or (task schema `0.9`) the value test, that guards the input; the guard evaluates its `target_id`, which is that check. |
| `candidate_layouts[]` | `{"id", "page_id", "kind": "fixed_slots", "features": [...], "slots": [...]}`: the candidate layouts of a page the task declares (`selection-graph.md`, section Candidate layouts, Source declaration). Each slot's `rect` and `click` lie inside the task's `coordinate_space`; each slot target is a template, color, color digest, check, OCR or NN target of the derived pack, and a `measure_milli` feature never reads a check. |
| `operations[].select` | `{"layout_id", "policy": {"path": "policies/<name>.json"}}`, the operation's only effect, with a declared `guard` of any accepted kind, an `expect_after` and no trusted coordinate (`selection-graph.md`, section Select step). The layout is one the task declares on the step's `from` page; the policy document is read as a selection-policy document and applies to the layout. A `sha256` the policy may still declare is optional and ignored, whatever its value (Workflow #336 PKG-S2). |

A check, like a color digest, is never clicked and never locates anything. `guard.check`
requires the guard target to be a `composite` or (Workflow #396 REC-6) `value_test` target. `guard.color_probe` accepts a color or a
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

### Task schema `0.9` value tests

Workflow #396 REC-6. A task schema `0.9` `checks[]` entry may declare a value test instead of
members; an older task schema refuses `value_test` with `UnconsumedField`, and an entry that
declares it beside `all_of` or `any_of` is refused at that member. It derives a recognition pack
schema `0.7` target of type `value_test` with the same `left`, `op` and `right`; an older pack
schema refuses the type. A source that declares none derives every document byte for byte as
before.

```json
{"id": "chk/count_matches_remaining",
 "value_test": {
   "left":  {"target_id": "ocr/count", "trim": "whitespace_v1",
             "value": {"type": "unsigned_integer", "format": "ascii_decimal", "min": 0, "max": 999}},
   "op": "eq",
   "right": {"target_id": "ocr/remaining", "trim": "whitespace_all_v1",
             "value": {"type": "unsigned_integer", "format": "current_capacity", "min": 0, "max": 999},
             "part": "current", "mul": 1, "add": 0}}}
```

| Member | Rule |
| --- | --- |
| a side (`left`, an operand `right`) | `target_id` names an OCR target of the pack without `locate` and without `target_consensus`; `trim` is `whitespace_v1` (both ends, as `str::trim`) or `whitespace_all_v1` (every Unicode whitespace character: `1 / 3` reads `1/3`); `value` is `{"type": "unsigned_integer", "min", "max", "format"?}` with `min <= max` and a format of the shared parser (`ocr-fields.md`, default `ascii_decimal`); `part` (`current` or `capacity`) is required with `current_capacity` and refused with any other format |
| `op` | `eq`, `ne`, `lt`, `le`, `gt`, `ge` or `parses`; `parses` has no `right` (`UnconsumedField`), every other `op` requires one (`MissingField`) |
| `right` | `{"const": n}` with `0 <= n <= 2^53-1`, or another side with `mul` (1 to 1,000,000) and `add` (`-(2^53-1)` to `2^53-1`), both required; `mul` and `add` are refused on `left` |

Each side reads its OCR target once per request and frame, within the text read budget of its
consumer (`ocr-fields.md`, section Located text), trims the text and parses it with the shared
`OcrUnsignedIntegerFormat` parser; the declared part must lie in `min..=max`, and the read's
confidence must be present and at least the OCR target's `minimum_confidence`. The right value
is the read value times `mul` plus `add`; both sides compare as 128-bit integers, without
overflow. A side that does not read fails the test with `failure` `value_test_unresolved`
(`target_id`, `value_side` `left` or `right`, the left side first, and `value_problem`
`empty`, `invalid`, `overflow`, `out_of_range`, `region_unresolved` or `low_confidence`); a
comparison that does not hold fails it with `value_test_not_met` (`target_id`, `value_op`). A
provider failure is an evaluation error. The read text and values travel only in the value
test's evidence (`value_test`: each side's `target_id`, `raw_text`, `value` or `problem`, `op`,
the right side as `{"kind": "const", "const"}` or `{"kind": "operand", "operand", "mul", "add"}`,
and `passed`) on its evaluation and on the task diagnostic target row; a projection treats a
value test, and a check holding one, as personal when a side's OCR target is personal, and
redacts that evidence.

A value test may stand in a page rule's `required`, `any_of` and `optional`, as a member of a
check, and as an operation's `guard.check` (directly or through a check); a guard whose value
test does not pass refuses its input (`contained_task_guard_refused`, the code as its `cause=`
token). In `forbidden`, directly or through a check, it is refused at the page set's
`/pages/<i>/forbidden/<j>` (`InvalidValue`): a test that cannot read does not pass, which there
would let the page match unseen; declare the opposite comparison in `required` instead. A value
test is never clicked, locates nothing, and is refused as a candidate layout slot target or
feature and with `target_consensus`.

Workflow #390 R-10. An OCR target whose region is `template_relative` (`ocr-fields.md`) cannot
be evaluated while its anchor template does not pass: wherever its pass or fail is asked (a page
rule, a member of a check, `guard.target_id`, `guard.check`) it did not pass, with `failure`
`ocr_region_unresolved` and its region evidence (`unresolved` `anchor_not_matched`), and calls
no provider. In `required` its page does not match, in `any_of` it is one member that did not
pass, and a guard on it refuses its input (`contained_task_guard_refused`); every other page of
the frame is recognized as before. In `forbidden`, directly or as a member of a check, such a
target is admitted only when its anchor template stands directly in the same page's
`required`; otherwise it is refused at the page set's `/pages/<i>/forbidden/<j>`
(`InvalidValue`), for the reason a value test is: without its anchor it would let the page match
unseen. With the anchor required, a missing anchor rules the page out through `required`, and a
present one evaluates the text as usual (the text present rules the page out). An anchor inside
a required check does not count.

A page recognition evaluates each page's immediate targets first, in the order `required`,
`any_of`, `optional`, `forbidden`. Its lazy targets (located text, value tests and checks
holding either) follow in the order `required`, `any_of`, `forbidden`, `optional`, and only
while the page can still match: once an immediate `required` target failed, an `any_of` group
of immediate targets all failed, an immediate `forbidden` target passed, or a lazy `required`
target failed, every remaining lazy target is skipped. A skipped target did not pass, calls no
provider and carries `skipped` (`page_unmatched`) on its page result and task diagnostic row,
with no evidence. The page results keep the declared order, and the match, including a frame
that matches more than one page, is the one a full evaluation gives.

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

Only `pack.json` is written at schema `0.7`, and only when it holds a `composite`,
`color_digest` or (task schema `0.9`) `value_test` target, a per-target `max_distance` or a
candidate layout. The page set,
navigation, operation index and primitives stay at `0.6`.

A select step's refusals name the task's `task.json` (`/operations/<i>/select/…`, its `guard`,
`expect_after` or `unguarded_trusted_coordinate`) or the policy document itself (the document, `/applies_to/candidate_layout_id`,
`/fields/<j>/name`, `/fields/<j>/value_type`, `/selection/required_count`). A select step is no
navigation edge and no page operation; its primitive has `click: null`, its declared `guard`
and the `select` object, and the sealed task keeps `select` and the declared guard.

### Task schema `0.9` multi-scale templates

Workflow #396 REC-5 (REC-M v1.2.1 §7). Task schema `0.9` accepts
`"scales": {"factors_milli": [...]}` on `anchors[]` and `verify_templates[]` entries; an older
task schema refuses it with `UnconsumedField`, and operations do not take it. It passes to the
derived template target as declared, and a source that uses it derives pack schema `0.7`; a
source that does not derives every document byte for byte as before. In the recognition pack,
schema `0.7` template targets accept it; an older pack schema refuses it with `UnconsumedField`
at `/targets/<i>/scales`. `factors_milli` is required (`MissingField`), an array (`InvalidType`)
of 1 to 4 integers (`InvalidType` for another value), each from 500 to 2000 and none repeated
(`InvalidValue` at `/targets/<i>/scales/factors_milli/<k>`; at the array for another count).
There is no default and the factors are the package's: a factor describes the interface's own
zoom in the package's base coordinates, never a device resolution, and the frame is converted
to the base coordinates before any template is resampled.

```json
{"id": "icon/a", "template": "assets/a.png", "region": {"x": 0, "y": 80, "width": 1280, "height": 560},
 "threshold": 0.88, "scales": {"factors_milli": [1000, 750]}}
```

The template's gray image is resampled to `round(width * f / 1000)` by
`round(height * f / 1000)` pixels (rounded half up) for each factor `f` with the resampling
primitive of `actingcommand-recognition` (`resample`: an axis that shrinks averages areas, an
axis that grows or keeps its length interpolates linearly between pixel centres; 16.16
fixed-point weights; each value rounded half up; the same bytes on every host; Workflow #392
converts frames with the same primitive). Every resampled template is scored over the target's
region on the exact score map (section Task schema `0.9` unique matches), with the region's
transforms shared by every factor and one five-second search limit for all of them. The highest
score wins; an equal score keeps the earlier factor in the list, then the earlier `(y, x)`
position. Only `[1000]` scores like a target without `scales` wherever that target is already
scored exactly (`ccoeff_normed` on a region below the coarse-search size, a relative color, a
mask or `unique`); a `ccorr_normed` target is then scored exactly as the direct scorer scores
it, as a target that declares `unique` is.

The match's box is the resampled template's, and the factor it hit at scales the package
lengths that hang on the match, each `(|v| * f + 500) div 1000` with the sign of `v`, a width or
height at least 1: a relative color region (`template-relative-color.md`), a
`template_relative` OCR region whose anchor declares `scales` (`ocr-fields.md`) and a click
`offset` (a `target` click takes the resampled box itself). With `unique`, a position at
another factor whose box does not intersect the best one is a second place like any other. A
mask applies to each resampled template's gray values. The evaluation and the task diagnostic
target row carry the factor as the template's `scale_milli`; a target without `scales` carries
none.

A target that declares `scales` is refused when the evaluator is built as a `target_consensus`
target (each frame would take its own factor), as a `repeated_anchor` anchor or as a candidate
layout feature read at an offset (both search the template as declared), and template-region
batch evaluation refuses it too. Where a package is admitted with its bytes, every resampled
template keeps at least 4 pixels a side (`TEMPLATE_SCALED_MIN_SIDE`, a contract constant) and
fits its region (its rectangle, or the coordinate space for `full_frame`), with
`resource_declaration_invalid` (`InvalidValue`) at `/targets/<i>/scales/factors_milli/<k>`
otherwise; the mask minimum and the `ccorr_normed` threshold floor (section Task schema `0.9`
template masks) hold for each resampled template, whose floor refusal names its `scale_milli`.

### Task schema `0.9` observation windows and input rechecks

Workflow #390 EXEC-2 (forced-compromises mapping, rows E-05 and E-06). An operation of a
page-graph task may declare an observation window, and a click may declare an input recheck.
An older task schema refuses either with `UnconsumedField`; a `linear_steps` task refuses either
at admission (`contained_task_linear_invalid`, `reason=observation_window` or
`reason=input_recheck`, [Linear steps](linear-steps.md)). A task that declares neither runs as
before.

```json
"observation_window": {"until": "window_end", "window_ms": 60000}
"observation_window": {"until": "stable", "window_ms": 10000, "frames": 3}
"input_recheck": {"kind": "fresh_frame"}
"input_recheck": {"kind": "stable_frames", "frames": 3}
```

| Member | Declaration gate | Runtime admission (`contained_task_operation_invalid`) |
| --- | --- | --- |
| `observation_window` | `until` is `window_end` (members `until`, `window_ms`) or `stable` (members `until`, `window_ms`, `frames`); every member is required (`MissingField`), `window_ms` and `frames` are unsigned integers (`InvalidType`), another member is `UnknownField` and another `until` `InvalidValue` | `window_ms` from 1 to 60,000; `frames` from 2 to 32, and `frames - 1` confirmation intervals (`expect_after.interval_ms`, else the control capture interval) fit in `window_ms`; the operation declares a destination (`to` or `expect_after`) |
| `input_recheck` | `kind` is `fresh_frame` (member `kind`) or `stable_frames` (members `kind`, `frames`), with the same refusals | the operation's effect is a click (not an `application` effect, not a select step, which confirms its candidate on a fresh frame already); `frames` from 2 to 32, and `frames - 1` control capture intervals fit in the control step timeout |

The post-input delay keeps its bound of 5,000 ms.

**Observation window.** When a destination of the operation passed after its input, the step
does not finish at once: frames are taken one confirmation interval apart and evaluated as
confirmation frames are. An error page fails the step as during the confirmation. A frame on
which no destination passed (another page of the package, or none) is offered to the shared
interrupt pages ([Package references](package-reference.md), "Shared interrupt pages"), and one
that is handled extends the window by the time the handling took.

- `window_end` watches the whole window for a late popup. A frame on which another destination
  passes ends it at once: the step finishes on that page. Otherwise the window ends at its end on
  a frame that shows a destination; when the latest frame shows none, frames go on up to the
  operation's confirmation budget after the end, and the step is unconfirmed, as a confirmation
  that timed out, when none passes.
- `stable` ends on the frame that makes `frames` consecutive frames of one destination, the frame
  of the arrival included: the step finishes on that page. A window that ends first ends the run
  with `contained_task_observation_stability_unconfirmed` (`pack_operation_id`, and `timeout_ms`
  the window); the input is never repeated.

The step's `StepFinished` names the page it finished on, and the run goes on from that frame.

**Input recheck.** It comes after the guard passed on the step's frame, before the action is
drawn, the one sampling deadline check before the input and the effect intent (Workflow #386
SR-1).

- `fresh_frame`: one fresh frame is captured and recognized, and the guard is evaluated on it;
  the input is drawn from and bound to that frame. A guard that does not pass there refuses the
  input with `contained_task_guard_refused`, through the operation's failure decision as a
  refusal on the step's frame does; a fresh frame on which no page of the package passes is
  `contained_task_page_unknown`.
- `stable_frames`: fresh frames are taken one control capture interval apart until the guard
  passed on the step's page on `frames` consecutive frames, the step's frame included; a frame on
  which it does not pass, or on which no page of the package passes, starts the count again. The
  input is drawn from and bound to the last frame. When the control step timeout, counted from
  the start of the wait, ends first, the input is refused with
  `contained_task_input_stability_unconfirmed` (`pack_operation_id`, and `timeout_ms` the step
  timeout), through the operation's failure decision.

Each fresh frame is an observation before the input: it feeds no collector, and a sampling
timeout of it takes the step's observe-only retries ([Business identity, consensus and catalog
authoring](business-identity-consensus.md), "Observe-only retries"). The frames of a window and
of a recheck are recorded as every capture, recognition and guard evaluation is; no record is
added.

### Task schema `0.9` designated effect sets

Workflow #390 OUT-2 (forced-compromises mapping, rows O-01, O-02, R-03 and S-01). A page-graph
task's `scheduling_outcome` may name a designated effect set in `designated_effects` instead of
the one `designated_operation`. Each member is an operation of the task whose effect is a
designated effect, with an explicit cap on how often it may complete in one run. A loop over a
counted business step (collect one item, come back, collect the next) declares its step a member
and leaves the loop through the page's next operation once the cap is reached; the cap, not
`max_steps`, bounds the loop. An older task schema refuses the member with `UnconsumedField`.

```json
"scheduling_outcome": {
  "designated_effects": [
    {"operation_id": "collect", "max_repeats": 5},
    {"operation_id": "start", "max_repeats": {"fact_key": "session.example.start.remaining"}},
    {"operation_id": "confirm"}
  ],
  "mappings": [...]
}
```

| Rule | Refusal |
| --- | --- |
| 1 to 8 members; each `operation_id` names one operation of the task, at most once in the set | `contained_task_outcome_declaration_invalid` |
| `max_repeats` is absent (once), a constant from 1 to 1,000, or `{"fact_key": <instance fact key>}` | `contained_task_outcome_declaration_invalid` |
| `designated_effects` and `designated_operation` are not both declared | `contained_task_outcome_declaration_invalid` |
| the set is a task schema `0.9` declaration (declaration gate: `UnconsumedField`) | `contained_task_outcome_declaration_invalid` |
| in a phased route, no member is retryable | `contained_task_phases_invalid` |

**Selection.** A member that completed as often as its cap is not selected again: the page's
operations are chosen as before (the first whose `from` names the page, then the first from
`any`, within the stage of a phased route), without the capped members. The static reachability
check follows each path with the members that completed on it: wherever a member may have
reached its cap (a fact cap may be 0; a constant cap after the member completed), the page's next
operation is followed too, and a page left without one refuses the task
(`contained_task_outcome_declaration_incomplete`; in a phased route
`task_phases_unreachable`). A member may be reached again after its effect; the one
`designated_operation` still may not.

**Fact caps** (FCM S-01: the decision layer writes the fact, the package reads it). The caps are
read once, before the run's first step, from the instance's ledger-pinned fact snapshot (the one a
select step reads): the record of the key must be an unexpired integer from 0 to 1,000. A cap of 0
means the member does not run. Otherwise the run ends before any input with
`contained_task_designated_effect_cap_unresolved` (`pack_operation_id`, `fact_key`; with
`unknown_reason` `fact_missing`, `fact_expired`, `fact_not_scalar` or `type_mismatch`, or with
`count` and `limit_count` for an integer out of range; neither when the Runtime has no fact
snapshot for the run).

**Outcome.** The mapping condition `designated_effect_completed` holds when any member completed,
`no_designated_effect` when none did. The terminal's scheduling disposition records the set's run
as `{"kind": "designated_effects", "effects": [{"operation_label", "count", "max_repeats"}]}`: one
tally per member in declaration order, `count` the times its effect completed (0 when it did not)
and `max_repeats` the cap the run held it to, so a run that did a step n times and one that found
nothing to do are told apart in the ledger. A member that completed more often than its cap, or
twice in one step, fails the run's outcome with `contained_task_outcome_designated_effect_duplicate`.
The policy inputs are unchanged: a catalog reads the mapped outcome key; the counts stay in the
ledger.

Wherever another rule names a package's designated operation (a recognition package, a
prerequisite package, a `linear_steps` task, a zero-input fields task), a designated effect set is
refused the same way, with the same reason token (`designated_operation`).

### Task schema `0.9` candidate operations

Workflow #390 EXEC-1 (forced-compromises mapping, rows E-04 and E-13). One page of a page-graph
task may carry several ordered candidate operations. An operation may declare `when`, the
condition under which it is a candidate, and a click may declare `on_guard_refused`, so that a
refusal of its guard falls to the page's next candidate instead of ending the run. An older task
schema refuses either with `UnconsumedField`; a `linear_steps` task refuses either at admission
(`contained_task_linear_invalid`, `reason=when` or `reason=on_guard_refused`,
[Linear steps](linear-steps.md)). A task that declares neither runs as before.

```json
"when": {"present": "check/claim_ready"}
"when": {"absent": "check/claim_ready"}
"on_guard_refused": "next_candidate"
```

| Member | Declaration gate | Runtime admission (`contained_task_operation_invalid`) |
| --- | --- | --- |
| `when` | an object with exactly one member, `present` or `absent`, naming a target (a string, `InvalidType` otherwise); no member is `MissingField` at `when`, both are `InvalidValue` at `when`, another member `UnknownField` | the target is a target of the recognition package that is judged on one frame: not a `click_only` target and not one under target consensus; not declared on the entry application (the first operation, an `application` effect from `any`, which runs before any page is known); its text reads, one consumer, stay within the static budget of a guard's ([OCR fields](ocr-fields.md)), refused otherwise as a guard's are at `/operations/<i>/when` |
| `on_guard_refused` | the string `next_candidate` (`InvalidValue` otherwise) | the operation is a click with a declared guard (not an `unguarded_trusted_coordinate` click, not an `application` effect, not a select step) |

**Candidates.** The candidates of a page are the operations chosen as before, in order: those
whose `from` names the page in the task's operation order, then those from `any`; within the
stage of a phased route, and without the members of a designated effect set that reached their
cap. The first candidate is taken.

- **`when`.** Before its step starts, the candidate's target is evaluated on the frame the step
  would start on, the frame the run is on: `present` holds when the target passes, `absent` when
  it does not. A candidate whose `when` does not hold is withdrawn: no step starts and no step
  index is taken, and the next candidate is taken on the same frame. The evaluation is recorded
  as a guard evaluation is, with the diagnostic phase `when` ([Task diagnostic
  stream](task-diagnostic-stream.md)); one that does not conclude ends the run with
  `contained_task_recognition_failed`.
- **`on_guard_refused: next_candidate`.** When the click's guard refuses
  (`contained_task_guard_refused`) on its step's frame at the step's first attempt, before any
  input of the step, the step finishes on the frame's page without an input (a `StepFinished`
  with no `EffectIntent`), stays counted in the run's executed steps, and the next candidate is
  taken on the same frame as the next step. A guard that could not be evaluated
  (`contained_task_guard_evaluation_failed`), a sampling timeout (observe-only retries), a refusal
  on a later attempt and a refusal of an input recheck (Workflow #390 EXEC-2) keep the operation's
  failure decision.
- A candidate passed over in either way is not taken again on that frame; the next frame of the
  run starts the page's candidates again.
- **No candidate left.** When every candidate of the page was passed over on the frame, the run
  ends before any further input with `contained_task_operation_candidates_exhausted` (`page_id`
  the frame's page, `count` the candidates passed over). A frame that matches more than one page
  is still `contained_task_recognition_conflict`.

**Static checks.** The page-graph reachability check of a scheduling outcome and the phased route
check follow every candidate that may run: after a candidate that declares `when` or
`on_guard_refused`, the page's next candidate is followed too, so each terminal page it reaches
needs its mapping. A page left without a candidate after such candidates only ends that path (the
run's `contained_task_operation_candidates_exhausted`); a page left without one once a member of a
designated effect set may have reached its cap is refused as before.
