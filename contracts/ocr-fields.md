# Operation 0.8 OCR fields

Operation schema `0.8` requires `post_admission_ocr.mode = fields_v1`.
Operation `0.7` continues to use its existing truth-set collection declaration and output.
Fields and collection declarations cannot be mixed. Fields declarations, limits, field
definitions, value types and dictionary references reject unknown members and variants.
Recognition targets, page admission, Containment, provider routing and GlobalLedger remain
the existing owners. No game-specific values are part of this contract.

The following is the `post_admission_ocr` member of an operation, not a complete package:

```json
{
  "mode": "fields_v1",
  "page_ids": ["panel"],
  "fields": [
    {
      "id": "count",
      "group": "snapshot",
      "target_id": "ocr/count",
      "required": true,
      "privacy": "public",
      "trim": "whitespace_v1",
      "value": { "type": "unsigned_integer", "min": 0, "max": 1000000 }
    }
  ],
  "limits": {
    "max_frames": 2,
    "max_items": 8,
    "max_string_bytes": 64,
    "max_total_bytes": 4096,
    "max_truth_entries": 8
  },
  "outcome_key": "fields_recorded"
}
```

`fields_recorded` is the fixed scheduling outcome key for this mode. Exactly one existing
scheduling mapping must bind it. It also lets the existing terminal fact require the OCR
artifacts without adding a Global event family or changing the Global event schema.

A `0.8` fields task may declare `operations: []` in `navigable_route` mode with
`stop_on_confirmation` omitted or true. Its nonempty target-page set, fields-page set
and sole `fields_recorded` mapping's terminal-page set must coincide under the existing
game-prefix page normalization, with no duplicate aliases. The mapping has
`no_designated_effect`, no designated operation, and the task has no recovery or stability
declaration. Build-task and Runtime admission apply the same contract predicate; the
offline package consumer delegates fields-package deep validation to Runtime admission.
The first frame is page-admitted before fields are read from that same frame. Reaching
a declared terminal page produces the ordinary verified report and successful receipt
with zero executed steps and zero input calls. A different admitted page cannot advance
an empty task and fails without fields success or input.

An entry is required Home when `entry_page`, after removing the exact current game prefix,
is `home` and its resolved page definition has nonempty `any_of`. The same resolver governs
admission, scheduling outcome coverage and execution, across operation schema versions.
Ordinary `PreparedContainedTask::run` and `run_with_options`, including offline simulation,
require the interpreter's original first captured frame to uniquely identify that Home.
This check follows complete page recognition and precedes fields collection, target
completion and input planning. A different or missing first page fails with
`contained_task_home_entry_not_matched` and the existing typed entry-recognition fact.
Ambiguous recognition retains the existing `contained_task_recognition_conflict` failure.
Only the first evaluation has this constraint; subsequent legal pages execute normally.
Scheduling outcome coverage starts at this enforced Home. Without it, coverage still
starts at every observable page. Operation priority, designated-effect completion and the
unique scheduling mapping retain their meanings; `entry_page: "any"` keeps its existing path.

For zero-input fields the entry decision and observations use the original single frame.
The Host records the existing no-recovery and target-disposition facts from that decision.
Tasks with actions retain the Host's preflight, bound recovery and recheck captures, followed
by the ordinary interpreter's own first capture. Passing preflight does not authorize a
later non-Home first interpreter frame. Its failure occurs before target input or fields;
the previous recovery decision remains the original fact.

The Host's existing hash-bound recovery call uses `run_entry_recovery`, which checks
`is_entry_recovery_compatible` before using the same interpreter. Compatible recovery
packages have no scheduling outcome, stability or post-admission OCR/fields declaration.
This restricted call keeps the recovery package's existing entry applicability; it exposes
no general entry-skip option or CLI/RPC operation. The Host still owns the exact hash,
instance and lease binding, actual Home terminal result, subsequent recheck and combined
step cap. Recovery does not recursively invoke target preflight.

`package dry-run` admits both nonempty and zero-input `0.8` packages through the typed
Runtime validator, including field, dictionary, privacy and outcome declarations. It
uses the existing production interpreter with OCR disabled for offline simulation;
`post_admission_ocr` reports `declared: true`, `executed: false` and
`pending_real_execution: true`. The local Lab execution loader and its validation entry
retain their `0.3` through `0.7` version boundary. Fields execution uses Runtime task-run.

There are one or two unique admitted pages and 1–32 ordered fields. Each field has a unique
ID and OCR target; its group ID associates fields within one admitted frame only. IDs are
1–128 ASCII alphanumeric characters or `_-/.:`. Each target must be a bounded OCR ROI and
must not form part of its page's recognition gate. Fields are collected at the existing
post-admission capture points, in declaration order, with one provider observation per
target. Parsing and dictionary mapping never call OCR again.

Each field can optionally declare `required_on_pages`, containing one or two full detector
page IDs. When any field uses it, every `page_ids` entry must also be a full detector ID,
such as `"neutral/result"` or `"neutral/lucky"`. Each condition must be a byte-for-byte
member of that same `page_ids` array. Source conversion and Runtime admission resolve these
IDs using the exact current game prefix and require one matching page with the identical
full ID. Empty, duplicate, `any`, short, foreign, missing and ambiguous references fail
admission. Existing ID and collection bounds still apply; authored IDs are not rewritten.
Omitting `required_on_pages` preserves existing behavior and serialization without a null
member.

For example, a declaration collecting on `["neutral/result", "neutral/lucky"]` may set
`"required": false, "required_on_pages": ["neutral/lucky"]` on a quantity field. The one
shared contract predicate is `required || required_on_pages.contains(record.page_id)`.
Kernel collection and client verification both use it. `required: true` remains mandatory
on every collection page. Outside the condition, an unresolved optional value is saved as
unresolved with its original reason; no zero or resolved value is synthesized. Field
order, groups, counts, provider calls, budgets, original text, normalization, extraction,
privacy and failure-report persistence keep their existing meanings. In conditional mode
the client also checks collection membership by exact full ID, without inferring a game
or loading another manifest.

Selected packages retain collection pages and their required, alternative (`any_of`) and
forbidden recognition targets, direct relative-template anchors and declared error pages.
They preserve those page criteria through generated resources and the sealed package.
Every page still needs its existing positive Template/Color gate; OCR runs after admission.
An unknown extra-result page can follow the existing error-page path. Actual layouts and
complete item coverage remain resource calibration and execution-evidence responsibilities.

An OCR target may locate its ROI relative to a template matched in the current frame.
The source and sealed pack use this same region object:

```json
{
  "mode": "template_relative",
  "anchor_target_id": "item/icon",
  "offset": { "x": -4, "y": 18 },
  "width": 48,
  "height": 20
}
```

Only OCR accepts this region. The anchor must be a template in the same admitted pack,
with a static rectangle or `full_frame` search region. Selected builds retain that direct
template and its hashed assets, including a template declared by an otherwise unselected
source bundle; they do not select the donor task or its OCR. Missing and non-template
references fail admission. Width and height are positive `i32`; offsets are signed `i32`.
The recognition owner adds offsets to the actual best match's top-left coordinates with
checked arithmetic. The entire ROI must fit in the current frame. It never clips, scales,
or substitutes a declared search rectangle. Existing absolute/full-frame OCR is retained.

Page recognition and field collection share a Scene-bound template evaluation context.
A previously evaluated template is reused; an unevaluated anchor is evaluated once in
that Scene. OCR output is not cached, and callers cannot import an old template result.
The context is bounded by admitted target identities and is released with the frame.
Best-match score, threshold and the existing color check govern the anchor. This API
does not establish match uniqueness. Resource authors choose discriminating icons and
search areas and calibrate offsets for their actual layouts.

An unmatched anchor, checked-coordinate overflow, or out-of-frame ROI produces
`region_unresolved` with its typed reason and no OCR invocation/execution evidence.
Required fields then fail the existing fields criterion; optional fields retain their
unresolved result without a fabricated value. Real provider failures keep their original
classification. Field/page gates, capture return values and backend session ownership
retain their existing contracts.

OCR observations and field records carry `region` evidence: anchor identity, actual
matched rectangle/raw score/score/threshold/pass decision, offset, frame dimensions,
requested dimensions and resolved ROI or unresolved reason. The Host commits this through
the existing DiagnosticJson lifecycle with the exact Task/Run/FrameId and original frame
artifact reference/hash before client projection. The client checks the native frame and
artifact lifecycle and the report/observation ROI binding. Online page projections expose
these actual ROI facts. Anchor, OCR target and field annotations contribute to the existing
personal-data union; public output redacts personal text, values and detail. GlobalLedger
schema and provider/backend identities are unchanged. Actual icons/layouts and real-device
calibration remain resource-authoring and Final Audit work.

`whitespace_v1` trims Unicode whitespace using Rust `str::trim`. An `unsigned_integer`
then requires a nonempty, complete ASCII decimal string. Zero and leading zeros are legal;
signs, separators, decimal points, non-ASCII digits and suffixes are not. The value must fit
`u64` and the inclusive declared `min..=max`. Empty, invalid, overflow and out-of-range
results are distinct. No display abbreviation is expanded into an exact value.

The other value type is:

```json
{
  "type": "dictionary_entry",
  "dictionary": { "path": "words.json", "sha256": "<64 lowercase hex digits>" }
}
```

The path is relative to the task's operation directory, bounded to 256 bytes and cannot
escape it. Source tooling, generated-package closure and Runtime admission verify the
hash-bound bytes. The dictionary uses `actingcommand.ocr-truth-set.v1` (`items`) or `.v2`
(`items`, optional `aliases` with `observed`/`canonical`). Trim/lowercase matching selects
one canonical item, preserving the item's declared spelling. Alias canonical references
must exactly name a declared item. Duplicate normalized items, duplicate/conflicting
aliases and invalid canonical references fail admission as ambiguous. Unknown observations
remain unresolved; no tolerant substitution or retry is applied to fields.

A `dictionary_entry` field may explicitly declare `text_extraction`:

```json
{
  "mode": "strip_declared_suffix_v1",
  "suffix": [
    { "type": "ascii_digits", "count": 2 },
    { "type": "literal", "value": "/" },
    { "type": "ascii_digits", "count": 4 },
    { "type": "literal", "value": ":" },
    { "type": "ascii_digits", "count": 2 },
    { "type": "literal", "value": " expires" }
  ]
}
```

The field still queries the complete trimmed input first, including exact aliases.
Only an unknown dictionary entry triggers extraction. The shared contract rule matches
the entire declared suffix once from the right, trims the remaining prefix, and passes
that prefix to the same exact dictionary lookup. For example, `Sample09/2803:59 expires`
can resolve to the declared item `Sample`. Unknown suffixes, empty prefixes and unknown
items remain unresolved. The declaration does not establish other date layouts.

There is one suffix sequence of 1–16 segments. An ASCII digit segment has exactly 1–8
digits; a literal is nonempty and at most 64 UTF-8 bytes. The complete suffix is at most
256 bytes. Unknown modes, segment types, invalid limits and use on another field type
fail admission. Omitting `text_extraction` preserves the existing declaration output and
full-input behavior. Resource authors own the literals and the source text budget.

`raw_text` stays the original bounded observation, and `normalized_text` stays its
Unicode-whitespace-trimmed form. A successful suffix extraction adds `extraction` with
`rule_version: "strip_declared_suffix_v1"`, `matched_suffix: {"start": ..., "end": ...}`,
`extracted_text`, and `extracted_range` in the same range shape. Both ranges are half-open
UTF-8 byte offsets in `normalized_text`. The client recomputes the evidence with the
shared rule under the existing frame/artifact/declaration binding; inconsistent text,
ranges or rule versions fail projection. Personal fields also redact this evidence.
The default path omits it. Extraction makes no additional OCR call and retains no state.

Original text and block text are counted before extraction. An over-budget observation
retains the existing truncation failure; a shorter extracted value cannot avoid it.
This capability does not increase or reset a resource's declared limits.

Limits retain the existing maxima: 256 frames, 4096 observations, 4096 bytes per string,
4 MiB total observed text/error detail, and 4096 dictionary entries. Aliases are bounded to
1024 per dictionary. Total loaded dictionary bytes are also bounded by `max_total_bytes`.
At the declared frame cap, collection stops at that cap. A text or item budget exhaustion
retains the bounded prefix and prior observations, marks the incomplete field and remaining
fields explicitly, and fails the task. It never silently reports a complete inventory.

Each frame produces one record per group, in first-declared group order, with fields in
declaration order. Subsequent frames produce additional records; values are never merged
or overwritten. Records retain the admitted page, observation index, field/target IDs,
raw and trimmed text, typed value or explicit reason, and bounded provider error detail.
`required` is mandatory. A required unresolved field fails after the frame's raw observation
and parsed report are saved; parsing failures still allow the other fields in that frame
to be read once. Provider or budget failures stop further calls and mark missing fields.
Optional unresolved fields remain visible in a successful report. Observation counts are
not field values and provide no item identity or inventory coverage.

Once at least one frame has been collected, ordinary task failures such as a guard
refusal, task timeout or OCR provider failure save the accumulated bounded fields report
once before returning the original task error. Capture, action-seed and input callback
errors do the same only when their error owner explicitly classifies them as nonfatal.
The production adapter uses `RuntimeHostError::is_fatal()`; an adapter without a declared
classification defaults to unknown. The execution result retains the original error and
distinguishes nonfatal operation failures from record failures. Parsed facts from earlier
frames retain their declarations, groups and values. Fatal or unknown operation failures,
all record failures, and artifact/ledger persistence failures propagate without another
report attempt, including failure after a report was already saved.

The host saves raw observations and `actingcommand.runtime.post-admission-ocr-fields.v1`
reports inside the existing created/verified `DiagnosticJson` artifact chain. The existing
comparison envelope carries the versioned report. Task/run and minted FrameId links remain
host-owned; report frame indices resolve only to that run's verified observation artifacts.

`RuntimeClient::run_contained_task` and `actingctl task-run` expose
`official_ocr_fields_projection` with schema
`actingcommand.runtime.official-ocr-fields-projection.v1`. It contains the declaration,
ordered records, each record's FrameId and observation artifact, report artifact lifecycle
references, provider evidence and explicit failure. The client validates the ledger
created/verified lifecycle, hashes, run/task/frame identity, complete field grouping, raw
observation binding and budgets before projecting. The collection-mode output key and
shape remain unchanged.

A fields failure retains the failed Runtime receipt and terminal in the returned flow.
The report's `failure` describes field collection or parsing; it may be null when all
collected fields resolved and a later ordinary task operation failed. The task outcome
remains in the receipt. The CLI writes the structured JSON and uses the receipt's error
to exit nonzero. No field projection can turn a failed task into success.

For failed runs, the client establishes fields mode from verified observation markers,
the typed report schema or the existing fields-failure terminal before requiring a
fields report. Fields-mode missing, malformed, mismatched or unverified evidence fails
explicitly. A 0.7 collection run with partial observations and a later ordinary provider
failure retains its original nonfatal rejection and terminal; it does not require a
final comparison report. Other Runtime errors retain their existing error behavior.

`privacy` is mandatory and is either `public` or `personal`; omission is not public.
For personal fields, the program-facing projection removes raw text, trimmed text,
canonical/numeric values and provider detail, and sets `redacted: true`. Status, field
identity, grouping and artifact provenance remain available. Raw personal observations
and reports stay at the existing controlled artifact paths with `DebugFull` retention and
`Pending` redaction, never falsely marked as publicly redacted. Consumers of outward
profiles must use the redacted fields projection; raw references identify restricted
evidence and do not authorize exporting its contents. Frame coverage and acquisition
remain the responsibility of the resource task and its authorized execution window.
