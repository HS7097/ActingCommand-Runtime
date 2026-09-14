# Selection policy

`actingcommand-selection-policy` owns the decision half of choosing one or more
candidates out of a bounded candidate set observed on a single screen. It is a
pure crate: it declares a document schema, a canonical identity for that
document, and one evaluator function. It performs no observation, holds no
lease, reaches no device, ledger, or network, and reads no clock. Every input is
an argument, so the same inputs always produce the same decision.

Observation (producing the candidate set) and execution (acting on a chosen
identifier) are separate contracts. This one covers only what happens between
them.

## Document

Schema version `actingcommand.selection-policy.v1`. One document holds:

- `policy_id` — the document's own identifier.
- `applies_to.candidate_layout_id` — the resource-declared candidate layout this
  document is written against. The evaluator does not check the candidate set
  against it; it copies it into the decision so the caller can.
- `applies_to.outcome_keys` — one resource-declared string per outcome
  (`selected`, `empty`, `insufficient`, `ambiguous`, `unknown`). Runtime never
  invents an outcome key; the decision reports the string the document supplied.
- `fields[]` — every candidate field the document may read, with its type.
- `facts[]` — every instance fact the document may read, with its type, the
  freshness bound `max_age_ms`, and the floor `minimum_confidence_milli`.
- `gates[]` — hard gates; a candidate whose gate predicate does not hold is
  rejected.
- `scoring[]` — weighted terms.
- `selection` — the selection mode and the required count.
- `tie_break[]` — the ordered tie-break keys applied after the score.

Types are closed: `integer`, `boolean`, and `enum_string` with a declared member
list. A term or predicate reads either a declared field (`{"source":"field"}`)
or a declared fact (`{"source":"fact"}`); an undeclared reference is a
validation error, not an unknown.

Every number in a document and in every input is an integer. Scores, weights,
lookup values, and threshold results are scaled by one thousand (milli).

### Transforms

A term turns one resolved value into an integer milli input through exactly one
transform:

- `identity` — passes an integer through unchanged.
- `threshold` — `at_least`, `then_milli`, `otherwise_milli` over an integer.
- `lookup` — an integer-valued table over integer, boolean, or enumerated string
  keys, with an optional `default_milli`.

`identity` and `threshold` require an integer value type; boolean and enumerated
string values must go through a lookup. A lookup key outside the declared value
type is a validation error.

The term's contribution is `transform_output * weight_milli / 1000`, truncated
toward zero. The candidate's score is the sum of its contributions. Any step
that leaves the 64-bit range is an `arithmetic_overflow` error, never a wrapped
or saturated value.

### Selection modes

- `top_k` — requires exactly `required_count` survivors. Fewer is
  `insufficient` and nothing is chosen.
- `exactly_one` — `required_count` must be one; otherwise identical to `top_k`.
- `none_allowed` — takes up to `required_count` survivors and accepts an empty
  answer as a normal outcome.

### Tie-break

Tie-break keys are applied in document order after the score. A key reads either
a declared value or the candidate identifier, each with a direction. If the two
candidates on either side of the cut compare equal under the score and under
every declared key, the outcome is `ambiguous` and nothing is chosen. The
candidate listing itself is still fully ordered, with the candidate identifier
as the last resort, so the report is stable.

## Canonical identity

The canonical form sorts object keys by their UTF-16 code units, emits no
insignificant whitespace, and leaves array order alone. It refuses anything that
would make a hash ambiguous:

- floating point numbers, including a whole number written with a decimal point
  or an exponent;
- integers outside the ECMAScript safe range;
- duplicate object keys;
- non-string object keys.

The document identity is `sha256:<hex>` over the canonical bytes. The decision
carries two of them: `policy_sha256` over the document, and `input_sha256` over
the candidate set, the fact snapshot, and the evaluation instant together. A
caller that records both can reproduce the decision exactly.

## Facts and unknown

The fact snapshot is built from published fact records, keeping the most
specific scope per key. A fact resolves to a known value only when all of the
following hold: a record exists under that key; the record is inline and scalar;
its own expiry has not passed; it is no older than the document's declared
`max_age_ms`; its confidence is non-zero and at or above the document's declared
floor; and its value has the declared type.

Otherwise it resolves to a typed reason: `fact_missing`, `fact_expired`,
`fact_stale`, `fact_low_confidence`, `fact_not_scalar`, or `type_mismatch`. A
candidate field behaves the same way, with `field_missing` and `type_mismatch`.
A lookup that covers neither the value nor a default yields `lookup_miss`.

Unknown is never read as `false`, as `0`, or as an empty string, and it is never
defaulted away. Each gate and each term states what happens when its input is
unknown, and the document cannot omit that statement:

- `drop_candidate` — the candidate leaves the ranking, its status becomes
  `unknown_dropped`, and the reason is recorded.
- `substitute_verdict` (gates) or `substitute_milli` (terms) — a value the
  document states in the open is used, and the decision records both the
  substitution and the reason it was needed.
- `abort_evaluation` — the whole evaluation ends with an `unknown` outcome
  carrying the reason and the rule that met it. Nothing is chosen.

Predicates are three-valued. `all` is false if any member is false, unknown if
any member is unknown and none is false, true otherwise. `any` is true if any
member is true, unknown if any member is unknown and none is true, false
otherwise. `not` negates true and false and leaves unknown alone.

## Decision

The decision reports, for every candidate in input order: its status (`ranked`,
`gate_rejected`, `unknown_dropped`), each gate's verdict, each term's outcome,
weight, and contribution in milli, its score, its rank among survivors, and its
own reasons. On top of that it carries the outcome, the resource-declared
outcome key, the chosen identifiers, and the decision-level reason chain, which
opens with both hashes and the declared requirement.

Gate and term evaluation short-circuits for a candidate the moment it is
rejected or dropped, so its breakdown ends at the rule that settled it.

An `Err` from the evaluator means the inputs could not be evaluated at all: an
invalid document, a candidate set over the limit or with a repeated or empty
identifier, or an overflow. An unknown input is not an error.

## Limits

Candidates 4096; fields 128; facts 128; gates 128; scoring terms 512; tie-break
keys 16; lookup entries 512 per transform; enumerated members 128; predicate
depth 16 and predicate nodes 512 per gate, matching the scheduling predicate
limits; identifiers, fact keys, and outcome keys 128 bytes; one document 512
KiB.

## Offline tool

`selection-eval --policy <file> --candidates <file> --facts <file>
[--now-unix-ms <n>]` runs one evaluation and prints one JSON envelope: the
decision, or a typed error. Exit code 0 carries a decision, exit code 2 an
error. The evaluation instant defaults to the snapshot's own instant, so the
tool reads no clock. It is a debugging aid and not a Runtime entry point.
