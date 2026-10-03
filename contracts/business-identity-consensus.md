# Business identity, consensus and catalog authoring

These optional declarations extend recognition pack `0.7` targets, candidate layouts and task
source declarations (task `0.6` through `0.9`). The package carrier, physical `{layout_id}#{slot}` IDs, feature limit
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
in 0..=1000, with a required floor in 1..=1000; missing confidence remains unknown.

Icon recognition instead declares `{"kind":"icon_templates","minimum_score_milli":900,
"minimum_margin_milli":50}` and each slot supplies `identity_templates[feature_name]`, with
1..=16 `{id, variant?, target_id}` entries naming existing template targets. Thresholds are
1..=1000, margins 1..=1000. The template score is normalized integer milli. Each pool closes
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

The task and recognition pack can declare `target_consensus`, keyed by an existing target ID:

```json
{"target_consensus":{"ready":{"samples":[{"frame":0},{"frame":1},{"frame":2}],"k":2,"sample_interval_ms":50}}}
```

OCR, template, color and color-digest predicates use `k`-of-N through
`SceneEvaluation::evaluate_target`; page rules, composite checks and input guards consume
that same result. A target's frame indices are contiguous from zero, its interval is bounded
to 1..=1000 ms when multiple frames are used, and multiple targets share the pack's interval.
At least one sample uses the current frame with zero jitter. The current sample supplies
the result's geometry. Single-frame target parameter variants require no additional captures.
NN and composite targets do not declare sampling; composite members may declare it.
Candidate feature mapping declares its own sampling on raw targets. One raw target has one
sampling declaration owner: targets throughout a candidate feature's reference closure,
including composite members, and OCR per-frame template anchors use raw recognition.
Raw OCR observation explicitly refuses
predicate consensus; authors use a separate raw reading target for inventory fields.

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
multiplied into separate hidden lists. Samples must differ in actual frame, region or effective
recognition parameter. Duplicate declarations, including an explicit default metric paired
with the same implicit default, are refused before execution. Reusing a frame reference does
not create another frame. Shared references to the same target/sample share its recorded evaluation. Unaggregated
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
For one select, admission sums `2*C + (2*F-1)*P + G*(1 + (F>1))`: `C` is one candidate
projection's declared provider timeouts and waits, `F` its frame count, `P` one complete page
capture transaction's provider timeouts and target-sample waits, and `G` one guard's provider
timeouts. H1 and H2 each add `F-1` page capture transactions, H2 has one fresh initial capture,
and an H2 projection that adds frames also checks its guard on the final frame. The initial
page transaction before StepStarted has its own entry-observation budget. The sum must fit
the original step/task limits at preparation and the remaining budget before selection starts.
Page references include all predicate roles and composite members; sampled target cache reuse
is counted once per context, while ordinary provider references retain their actual call count.
Capture, local computation and ledger costs use the same absolute deadline without a separate
duration allowance. Execution also checks remaining time; bounded waits
are recorded as page-recognition waits and actual backend time as recognition evaluation.
No sample delay occurs after an input Intent. The final tap still uses the normal fenced input
path. A consumer without the sampling checkpoint explicitly refuses sampled task execution.

The kernel captures one bounded frame transaction for page detection, required-home checks
and input guards; it checks geometry and active permission between captures and records each
backend call before continuing. Target raw outcomes carry typed sample frame hashes, effective
parameters, actual durations and optional verdicts (`null` on a failed call). Target aggregate
rows retain `k`, per-sample evidence and the verdict. Sample records and their OCR children do
not claim the latest capture's frame ID. The existing diagnostic artifact keeps raw inputs;
single-frame offline consumers retain them inside `sample_evaluations` instead.

The select deadline is fixed at StepStarted and capped by the existing task deadline. H1/H2,
each additional page capture, target and candidate evaluation, confirmation guard and final
pre-input check inherit it. Entry-recognition and postcondition observation loops likewise
retain the deadline established by their caller for that observation phase.

`SceneEvaluation::project_candidates` is the single-frame adapter;
`project_candidates_with_samples` consumes the exact scene set and uses the same mapping and
aggregation. Offline `observe` and online contained observation use this owner. They currently
supply one frame: same-frame jitter/parameter consensus works, and a declaration requiring
more frames reports `candidate_samples_unavailable` with coverage. Online observation records
that refusal as a partial observation with no successful recognition. Public candidate rows
use the strictest privacy of their referenced targets; full inputs stay in controlled evidence.
Target sampling likewise reports required/provided frames and `recognition_coverage`; an
unsupported multi-frame observation has no successful recognition. Required raw sample
evidence exceeding the existing observation budget fails instead of being omitted.

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
