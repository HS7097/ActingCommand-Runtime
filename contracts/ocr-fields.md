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

For an explicit required Home entry using `any_of`, the zero-input task confirms that
entry within the interpreter's first capture, before collecting fields. The Host records
the existing entry-recognition, no-recovery and target-disposition facts from that decision.
An unmatched required entry fails on that frame without fields or recovery. The report
and observation share its FrameId; the run's terminal capture summary pins that same frame.
Tasks with actions retain the Host's entry preflight and recovery behavior.

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
