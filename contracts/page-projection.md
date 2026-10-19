# Page projection and resource annotations

`actingcommand-contract::page_projection` owns the pure projection function,
metadata validation and `actingcommand.page-projection.v1` DTO. It accepts one
frame's already resolved facts. It performs no recognition, file access, provider
call, device operation or state mutation. The offline `actinglab observe --scene`
adapter calls this owner; Runtime consumption uses the same boundary.

## Annotation source and admission

The optional source is `navigation/<game>.<server>.projection.json` below the
resolved resource root. This is the only authored source for these annotations.
Parsing validates it against generated pack/pages/navigation and existing
operation field identities. Package construction copies the selected declaration
to `resources/navigation/<game>.<server>.projection.json`. Parsing does not
overwrite the source. Selected packages first validate the complete source, then
retain only references present in the selected package.

The JSON document has required `schema_version`, `actions`, `targets`, `fields`
and `pages` members. Its schema is `actingcommand.page-projection-metadata.v1`.
All record types reject unknown members. Unknown versions, invalid types, repeated
identities (including conflicting classifications), unresolved references, empty
sources/scopes, out-of-frame rectangles, more than 4096 annotations or a document
over 1 MiB fail admission.

```json
{
  "schema_version": "actingcommand.page-projection-metadata.v1",
  "actions": [{
    "action": {"role": "page_op", "task_id": "sample", "resource_id": "open", "page": "sample/home"},
    "safety": "dangerous",
    "source": "resource-authority/decision-1"
  }],
  "targets": [{"target_id": "sample/value", "privacy": "public", "source": "resource-authority/field-1"}],
  "fields": [],
  "pages": [{
    "page_id": "sample/home", "completeness": "windowed",
    "scope": "currently visible inventory rows", "source": "resource-authority/window-1",
    "visible_rect": {"x": 0, "y": 0, "width": 100, "height": 100}
  }]
}
```

Action keys bind role, task, resource ID and page. `navigate` binds a navigation
edge's `id` and `from_page` and uses an empty task ID. `page_op` binds its
`task_id`, `id` and `page`. `control_point` binds `name`, an empty task ID and a
null page. Targets bind recognition target IDs. Fields bind the tuple
`task_id`, `field_id`, `target_id` from an admitted operation's `fields_v1`
declaration; this module only catalogs identities and does not admit operation
versions or interpret OCR values. Existing operation validators remain owners of
those declarations. Page annotations bind existing page IDs.

Containment verifies the archive's external hash before extraction. For an
annotated package, the annotation file, pack, pages, navigation and referenced
field operation documents also require matching manifest SHA-256 entries.
The generated-package validator and Containment call the same annotation
admission function. A declaration at another navigation stem fails. A missing
annotation file leaves optional action/window metadata unknown; values follow
the verified resource identities and classifications described below.

## Safety, privacy and window meanings

`safety` is `safe`, `dangerous` or `unclassified`, with a nonempty classification
source. A newly authored record omitting `safety` defaults to `dangerous`.
A missing record projects `unclassified` and a null source. No name, purpose,
word list, consumption tier or rectangle is interpreted as a safety decision.
This field describes an existing decision and grants no production execution
permission. Existing effect/resource-policy, cost, Containment, current-page,
geometry, lease/fencing and ledger checks remain in their existing owners.

Target and field privacy annotations use `public` or `personal`. Values bound to
verified resource identities are public unless a valid source marks them personal.
A field tuple also requires its original operation classification in the verified
catalog. The operation's mandatory `public` declaration is sufficient; companion
records can supply classification/source information or tighten it to `personal`.
The operation contract is consumed through its current validator.

Any personal classification on the target, field, original operation declaration
or supplied result requires redaction. Public classifications cannot downgrade it.
A composite check (`selection-graph.md`) evaluates its members, so a personal
classification of any member classifies the composite as personal too.
An unknown target, unknown field tuple or mismatched target/field is rejected.
A field without its cataloged operation classification remains redacted with
`privacy: null`, even when the caller or companion labels it public. This unknown
source state is distinct from a known `personal` classification.

Redaction removes raw text, parsed value and error detail together, preserving
identity, parse status and a `redacted` flag. No image bytes or raw personal
artifacts are embedded. A frame identity identifies bytes; it does not grant
access to an artifact. The adapter supplies facts from the same verified frame;
the projection retains the existing frame identity and coordinate-space checks.

Window completeness is `complete`, `windowed` or `unknown`. An annotation names
the declared scope and its source. `windowed` requires an in-frame visible
rectangle; absence of a declaration yields `unknown`. The projection only
describes this frame and declared scope. It does not infer whole-inventory
coverage. `output_truncated` and `omitted_count` describe serialization omissions;
`truncated` is true for either a windowed declaration or output omissions.
Thus `truncated=false` with `page_window_completeness=unknown` proves no coverage.

## Shared input and output

The input binds a frame digest, digest kind and dimensions to its matched page,
resolved action inputs, target results and parsed values. Offline frames use
SHA-256 of decoded row-major RGB8 bytes (`kind=rgb8`); online adapters may supply
a verified artifact digest (`kind=artifact`). Dimensions must match the resource
coordinate space. Each fact is supplied from that same frame; the pure function
does not acquire or refresh facts. At most 4096 input facts are accepted.

No match yields `state=unknown`; multiple matching pages yield `state=conflict`.
Neither yields normal page elements, values or unscoped controls. The existing
offline page detector reports its conflict error before invoking projection.
On one confirmed page the output contains action identity, role, purpose/label,
recognition basis, availability, resolvability (`actionable`, not authorization),
blocking reason, declared safety and resolved geometry. Unmatched targets have
no guessed geometry. A declared/resolved action whose geometry is outside the
current frame remains readable with `blocked_reason=invalid_frame_geometry`,
`actionable=false` and null input. #411 Lab L2: an action whose click is a segmented drag
(`single_touch_drag_with_vertical_brake_v1`), a gesture the input's `tap` and `drag` cannot
describe, is listed from the confirmed page declaration (`recognition_basis` is
`matched_page_declaration`) with `blocked_reason=gesture_not_projectable`, `actionable=false`
and null input; a Lab operation on it is `lab_element_unavailable`, and a package run performs
it. Unscoped controls remain explicitly unrecognized/unknown.
Missing required/optional/any-of targets retain group information; a forbidden
target's absence is not a missing element. Static geometry comes from the
confirmed page declaration. Offline action target resolution evaluates the target
on the same scene with the Lab's provider for OCR and NN ([lab-recording.md](lab-recording.md),
"OCR in the Lab"; #411 Lab L6): an OCR target or a value test when OCR is
available, an NN target when the provider holds its model; otherwise the target
is not evaluated (`dynamic_target_not_evaluated`). Offline `observe` fills the
values (`fields`) of every target it evaluated, of every kind, as the online
observation fills them: from the targets of every page in page order, then each
target `--targets` names that no page evaluated (an optional target of no page),
one entry per target whose `value` is `{evaluations: [{evaluation_index, page,
group_index, target_index, role, evaluation}]}`, and one more per field an
operation declares on that target (`post_admission_ocr` `fields_v1`), with that
field's privacy.

The complete DTO is limited to 64 emitted entries across elements, unscoped
controls, missing targets and fields, and 32 KiB of compact JSON. Counts describe
the original input and actual emission. An oversized entry can be omitted; an
identity that cannot fit fails explicitly. The `Min` transport can request a
smaller byte budget from the same owner. It never edits DTO fields independently.
The default Min envelope targets 1 KiB and keeps at most one resolvable element
when that target requires its existing 2 KiB hard limit. Unresolvable entries may
all be omitted, with original counts and a freshly hashed truncation state. When
that one element does not fit the hard limit either, Min omits it too and answers
the identity alone in the same truncation state; only an identity that alone
exceeds the hard limit fails, with actinglab's `observe_identity_limit_exceeded`
(#391 MCP-391b).
When nothing matched, offline `observe`'s near-miss `candidates` are a transport
member: Min removes them from the end before the identity is judged, marked
`candidates_truncated` with `candidates_omitted_count` (#411 A9).
Empty missing/control/field arrays and an absent window are omitted in JSON;
the root `omitted_count` is the serialized omission count. The offline frame
digest supplies the default Min provenance; `frame_source` remains available by
explicit field selection.

`content_sha256` hashes compact `serde_json` serialization of the entire DTO as a
JSON value, with only `content_sha256` removed. DTO members retain declaration
order, nested JSON values and arrays retain input order, and strings use UTF-8.
The owner recomputes the hash after budgeting; frame identity and declared
metadata visible in the output participate. An unchanged input and budget produce
an unchanged hash. Transport fields outside `observation` are not covered.

The offline envelope retains its existing outer protocol. Its `observation`
member now uses this shared schema, including `frame`, `fields`, `window`,
`output_truncated` and `content_sha256`; no local projection implementation is
retained. The online successor must attach its existing RecognitionCompleted
and DiagnosticJson lifecycle, verified return and instance ledger sequence.
This module does not allocate a sequence or add a ledger/event authority.
