# Business identity, consensus and catalog authoring

These optional declarations extend recognition pack `0.7` candidate layouts and their task
source declarations. The package carrier, physical `{layout_id}#{slot}` IDs, feature limit
(8), slot limit (64), provider evaluation limit (16 OCR/NN calls per projection), and 32 KiB
core projection limit remain unchanged. An undeclared capability uses the single-frame path.

## Identity and readable attributes

A feature has `value: "identity"` and a required `identity` object:

```json
{
  "name": "business_id",
  "value": "identity",
  "identity": {
    "entries": [{"id": "item_a", "variant": "short", "aliases": ["Item A", "A item"]}],
    "recognition": {
      "kind": "ocr_aliases",
      "max_distance": 1,
      "minimum_margin": 1,
      "minimum_confidence_milli": 850,
      "confusions": {"0": "o"}
    }
  }
}
```

The existing slot `targets` map binds an OCR identity feature to its OCR target. Entries are
bounded to 128 unique `(id, variant)` pairs; identifiers/variants are nonempty, control-free,
at most 128 bytes. Each entry has 1..=8 aliases of at most 128 Unicode characters/512 bytes.
Normalization folds fullwidth ASCII and halfwidth katakana, composes voiced kana, lowercases,
removes whitespace, then applies up to 32 explicit simultaneous single-character confusion
substitutions. The same normalization applies to aliases and readings. The bounded edit
distance is at most 8; the required runner-up margin is 1..=9. Confidence is integer milli
in 0..=1000; missing confidence remains unknown even at a zero floor.

Icon recognition instead declares `{"kind":"icon_templates","minimum_score_milli":900,
"minimum_margin_milli":50}` and each slot supplies `identity_templates[feature_name]`, with
1..=16 `{id, variant?, target_id}` entries naming existing template targets. Thresholds are
0..=1000, margins 1..=1000. The template score is normalized integer milli. Each pool closes
against the feature's identity domain and contains no repeated target ID.

Best/runner-up comparison is between different business IDs. Multiple aliases or templates
of one ID are not competitors. An equally good unresolved variant of the winning ID is
ambiguous. Missing, out-of-domain, low-confidence and ambiguous readings are typed `unknown`
features. A known `identity` feature carries `value` (business ID), `variant`, `source`,
`distance` and `confidence`; selection-policy consumes `value` through its existing
`enum_string` field and inline lookup. Its declared enum must equal the identity domain.

For current numeric attributes, `value: "ocr_integer"` requires
`integer: {min, max, format?, minimum_confidence_milli}`. It reuses the existing
`OcrUnsignedIntegerFormat` parser after whitespace trim. `format` defaults to `ascii_decimal`;
`comma_grouped` and `current_capacity` keep their existing semantics. Bounds satisfy
`0 <= min <= max <= 2^53-1`; confidence is 1..=1000. The result is a real integer, not a milli
quantity. Missing/low confidence and failed parsing produce typed Unknown. `passed` and
`measure_milli` retain their existing meanings, including OCR confidence for `measure_milli`.

The layout's `unknown_identity` defaults to `reject`. Explicit `readable_attributes` allows
ranking using known current attributes. Hard gates cannot substitute a passing verdict for
Unknown. Scoring substitutions are allowed only for identity terms and only if their weighted
contribution is nonpositive; all other scoring inputs must be known. An unresolved declared
consensus rejects the candidate regardless of this fallback.

The candidate content hash includes identity ID, variant and source, all scalar feature
values, Unknown reasons, and physical geometry. Confidence, directory distance, raw sample
evidence and frame identity remain evidence outside the hash. H1/H2 independently construct
their projections. A changed identity, variant, attribute or slot arrangement invalidates
confirmation. Confirmation evidence retains the typed H2 projection when it has recognition
evidence; the click rectangle and input frame reference come from its current final frame.

## Finite consensus

Each feature may declare `consensus: {samples, aggregate}`. `samples` has 1..=5 entries:

```json
{
  "samples": [{"frame":0}, {"frame":1,"dx":1}, {"frame":2,"dy":-1}],
  "aggregate": {"kind":"majority","k":2}
}
```

`frame` defaults to 0 and lies in 0..=4. `dx`/`dy` default to 0 and lie in -8..=8 pixels.
The union of declared frames must be contiguous from 0. A multiple-frame layout requires
`sample_interval_ms` in 1..=1000; single-frame layouts require 0 (the default). Jitter shifts
fixed regions or existing template-relative offsets; keyword regions with nonzero jitter
are refused. Rectangles must remain in the coordinate space. The optional `template_metric`
is the existing `ccorr_normed` or `ccoeff_normed` parameter; non-template targets reject it.
OCR, template, color and color-digest targets support sampling. NN and composite targets
retain their single evaluation semantics and cannot declare consensus.

The sample list is the one total budget: frames, jitter and parameter variants are not
multiplied into separate hidden lists. Repeated identical entries are distinct actual samples;
shared references to the same target/sample can share that recorded evaluation. Unaggregated
attributes are read from the transaction's latest frame. Evaluations cannot be reused across
transactions, geometry changes, effects, or H1/H2.

| Feature | Aggregation |
| --- | --- |
| `passed` | `k_of_n {k}`: true at k true votes; false when even all unknown votes cannot reach k; otherwise Unknown |
| `measure_milli`, `ocr_integer` | `median`: lower middle observed value for even N, all samples required; or `k_of_n {k}`: one unique value with at least k identical readings |
| `identity` | `majority {k}`: meet k votes first, then higher vote count, then smaller observed directory distance; an unresolved tie is ambiguous |
| `identity` | `closest_single {maximum_distance}`: smallest observed directory distance under the explicit 0..=1000 cap; a tied different identity/variant is ambiguous |

`k` is 1..=N. Votes group the stable `(id, variant)` tuple after each sample's alias/template
mapping. Samples missing because of a provider, capture, recognition or ledger failure fail
the operation; they are never negative votes. The typed `recognition_evidence` in the core
projection includes each frame index/RGB8 SHA-256, input text/confidence, icon scores or scalar
reading, and its mapped value. Aggregation is a pure function of these recorded values.
The original artifact and projection byte limits apply; a refusal never truncates a decision.

The kernel owns capture and waiting under its existing Runtime lease/cancellation checks.
Admission counts worst-case provider calls and both H1/H2 declared provider timeouts plus waits
against the original task/step budgets. Execution also checks remaining time; bounded waits
are recorded as page-recognition waits and actual backend time as recognition evaluation.
No sample delay occurs after an input Intent. The final tap still uses the normal fenced input
path. A consumer without the sampling checkpoint explicitly refuses sampled task execution.

`SceneEvaluation::project_candidates` is the single-frame adapter;
`project_candidates_with_samples` consumes the exact scene set and uses the same mapping and
aggregation. Offline `observe` and online contained observation use this owner. They currently
supply one frame: same-frame jitter/parameter consensus works, and a declaration requiring
more frames reports `candidate_samples_unavailable` with coverage. Online observation records
that refusal as a partial observation with no successful recognition. Public candidate rows
use the strictest privacy of their referenced targets; full inputs stay in controlled evidence.

## One static catalog

`actinglab resource catalog --repo <root> --catalog <JSON> --catalog-server <server>
[--field business_id]` returns deterministic authoring JSON without network access. The input
schema is `actingcommand.business-catalog.v1`: `catalog_id`, `recognition` (the identity
recognition object above), `pools` (ID to positive capacity) and 1..=128 `entries`.

Each entry has a unique stable `id`, optional `variant`, `names` (server to 1..=8 names/aliases),
optional `duration_seconds`, `costs: [{pool_id, amount?}]`, `rewards`, optional
`preference_milli`, and `source: {uri, date}` with a calendar-valid `YYYY-MM-DD` date.
Variants that need different lookup values have their own stable row IDs. Input is at most
1 MiB, output at most 2 MiB; identifiers, pool counts and effect counts are bounded. Unknown
fields, duplicate row IDs and unclosed pool references are errors. The selected server must
have a name mapping for every row. Icon target pools remain part of the existing layout.

A reward is `{pool_id, quantity_milli?, probability_milli?, batches?, confidence_milli?,
observation_source, observed_amount?}`. Quantity is per successful batch in milli real units;
probability and confidence are separate 0..=1000 values; batches are 1..=1,000,000.
`observation_source` retains the scheduling values `self_reported`, `scan_verified`, `inferred`.
`observed_amount` is retained only as provenance, never published as an inventory fact.

The generated object carries the exact source SHA-256 and source rows, `identity_feature`,
the matching selection-policy `policy_field`, and existing typed `lookup_transforms` for
duration, costs, expected rewards, probabilities, evidence confidence and preferences.
Duration/cost values are converted to milli units for `value_milli`; already-milli dimensions
keep their units. Missing keys have no default and remain lookup misses, with explicit
`unknown_values`. Authors assign policy weights and all business hard gates themselves.

`scheduling_produces` targets `actingcommand.scheduling.v2` and emits only
`expected_amount_milli`, never `amount`. It computes
`quantity_milli * probability_milli * batches / 1000` with checked bounds and requires exact
milli representability, `expected <= pool.capacity * 1000`, and `expected <= 2^53-1`.
`1000 * 400 * 1 / 1000 = 400`
means 0.4 real units. Evidence confidence does not multiply this number. Missing expectation
dimensions or confidence omit that effect and report their unknown fields; a genuine known
zero remains zero. Static catalog JSON is never a Runtime input parser or inventory owner.

## Preparation coverage

`actinglab package preflight --package <directory-or-zip> --package-ref <external-reference>`
uses hash-first containment and `PreparedContainedTask::from_bundle`, the same task/control,
version, phase, reference and select-policy preparation owner as production. It does not
execute recognition, a task, a provider or a device. Its response/error distinguishes package
load and declaration coverage from task preparation and execution. `package validate` continues
to report package-format coverage and explicitly says task preparation was not run.
This authoring command is not a resource CI gate; resource CI remains declaration/identity only.
