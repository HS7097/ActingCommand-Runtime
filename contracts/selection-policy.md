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

A gate's `gate_id` and a term's `term_id` are tokens: lowercase ASCII letters,
digits, `_`, `.`, `:` and `-`, because the decision's reasons carry them as the
outcome value `rule_id` (Workflow #378 A2-P). The document's own validation
bounds them only by length; pack admission refuses another id (see
[selection-graph.md](selection-graph.md), section Select step).
- `selection` — the selection mode and the required count.
- `tie_break[]` — the ordered tie-break keys applied after the score.

Types are closed: `integer`, `boolean`, and `enum_string` with a declared member
list; a fact may also be a `record_list` (section "Record lists and
`keyed_fact`"). A term or predicate reads a declared field (`{"source":"field"}`),
a declared fact (`{"source":"fact"}`), or one row of a declared record list
(`{"source":"keyed_fact"}`); an undeclared reference is a validation error, not
an unknown. A `string_in` predicate may list only members
the value's declared type declares; a member outside that list is a validation
error too, not a gate that quietly never holds.

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
a declared value or the candidate identifier, each with a direction. An
enumerated string declares a member list but no order, so a tie-break key cannot
read one; score it through a lookup instead. That is a validation error.

A key whose value is unknown for one candidate and known for the other orders
the known candidate first, whichever direction the key declares: an unknown
never stands in for a value, so it can never outrank one. The candidate's
verdict records a `tie_break_unknown` reason (its developer detail, which is not
persisted, names the key and why it could not be read). Two unknowns under the same key are indistinguishable and
fall through to the next key.

If the two candidates on either side of the cut compare equal under the score
and under every declared key, the outcome is `ambiguous` and nothing is chosen.
A pair the keys separate only because one side is unknown is therefore decided,
and a pair whose keys are unknown on both sides is ambiguous. The candidate
listing itself is still fully ordered, with the candidate identifier as the last
resort, so the report is stable whichever order the candidates arrive in.

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
its own expiry has not passed, reading the expiry instant itself as still usable
the way the fact contract's own predicate does; it is no older than the
document's declared `max_age_ms`; its confidence is non-zero and at or above the
document's declared floor; and its value has the declared type.

Otherwise it resolves to a typed reason: `fact_missing`, `fact_expired`,
`fact_stale`, `fact_low_confidence`, `fact_not_scalar`, or `type_mismatch`. A
candidate field behaves the same way, with `field_missing` and `type_mismatch`.
A lookup that covers neither the value nor a default yields `lookup_miss`.

Unknown is never read as `false`, as `0`, or as an empty string, and it is never
defaulted away. Each gate and each term states what happens when its input is
unknown, and the document cannot omit that statement. The statement governs the
default call, `evaluate`, which an in-task select step makes:

- `drop_candidate` — the candidate leaves the ranking, its status becomes
  `unknown_dropped`, and the reason is recorded.
- `substitute_verdict` (gates) or `substitute_milli` (terms) — a value the
  document states in the open is used, and the decision records both the
  substitution and the reason it was needed.
- `abort_evaluation` — the whole evaluation ends with an `unknown` outcome
  carrying the reason and the rule that met it. Nothing is chosen.

A tie-break key carries no such clause; the one rule above covers it, and the
verdict records every key it could not read.

Under the collect option, `evaluate_collect`, none of the three applies: an
unknown input neither ends the evaluation, nor drops the candidate, nor is
replaced. The result is one entry per candidate, in input order: its score when
every gate holds and every term reads a known value; the gate that rejected it
when a gate is decided against it; otherwise its unknown set, every input that
left a gate undecided or that a term could not use, each once, with the gate or
term that met it, the fact key or field name it read, and the reason. A member of `all` or `any` that the other members outweigh is not in
the set. Each entry also lists the inputs read as known values, a `fact` source
with the last instant it stays known, the earlier of the record's expiry and its
publication plus `max_age_ms`. Tie-break keys are not read, nothing is selected,
and no hash is computed. A document that declares a record list (see below) is
refused with `type_mismatch`: only a task's select step reads one.

Predicates are three-valued. `all` is false if any member is false, unknown if
any member is unknown and none is false, true otherwise. `any` is true if any
member is true, unknown if any member is unknown and none is true, false
otherwise. `not` negates true and false and leaves unknown alone.

### Two values compared

Workflow #411 B2. A gate predicate may compare two integer values:

```json
{"kind": "integer_compare",
 "left": {"source": "field", "field": "cost"}, "op": "le",
 "right": {"source": "field", "field": "funds"}, "mul": 1, "add": -2000}
```

- `left` and `right` each read a declared value (`field`, `fact` or `keyed_fact`) of type
  `integer`; another type is a `type_mismatch` validation error.
- `op` is `eq`, `ne`, `lt`, `le`, `gt` or `ge`. The predicate holds when
  `left op right × mul + add`, computed and compared in 128 bits, exactly as a value test
  compares its sides ([resource-declarations.md](resource-declarations.md), value tests).
- `mul` is optional, 1 to 1,000,000, default 1; `add` is optional, within ±(2^53 − 1), default
  0. A value outside is an `integer_out_of_range` validation error. Neither is written into the
  canonical form when the document leaves it out.
- A side that is unknown leaves the predicate unknown, so the gate's `on_unknown` decides; when
  both are unknown, the left side's reason is the one reported. Under the collect option each
  unknown side is listed. A side that reads `keyed_fact` makes the gate fail-closed, as below.
- A document that does not use `integer_compare` keeps its canonical bytes, its
  `policy_sha256` and its `input_sha256`. A reader before B2 refuses a document that uses it
  (an unknown predicate kind). In a package it belongs to `package` revision 6.

## Record lists and `keyed_fact`

An in-task document may read a record-list fact one row at a time. The list is
declared in `facts[]`, and only there:

```json
{"fact_key": "session.example.list.targets", "max_age_ms": 31536000000,
 "minimum_confidence_milli": 1000,
 "value_type": {"type": "record_list", "key_column": "id",
                "columns": {"rank": {"type": "integer"}}}}
```

Every row holds the string `key_column` and each declared column with its
declared scalar type (`integer`, `boolean` or `enum_string`); columns the
document does not declare are not read. A document declares at most 63 columns
per list, the fact contract's 64 fields per row less the key column.

A term, a gate predicate or a tie-break key reads one column:

```json
{"source": "keyed_fact", "fact_key": "session.example.list.targets",
 "key_field": "member", "value_column": "rank",
 "where": {"column": "kind", "equals": "kind-a"},
 "absent": {"type": "integer", "value": 0}}
```

- `key_field` is a declared `enum_string` field; in a select step it is the
  layout's identity field. The row whose key column equals the candidate's
  `key_field` value supplies `value_column`.
- `where` is optional. It keeps only the rows whose `enum_string` column equals
  `equals`, so one list can hold rows of several kinds.
- `absent` is required and has the column's type. When no row has the key, the
  value is `absent`, and it is known: a gate decides on it, and a rejection is a
  `gate_rejected` verdict, never an unknown.
- A record list is read only this way. A `record_list` field, or a
  `{"source":"fact"}` reference to a record list, is a `type_mismatch`
  validation error.

**Fail-closed declaration.** Every gate and every term that reads `keyed_fact`
declares `on_unknown: abort_evaluation`; any other handling is a
`type_mismatch` validation error. A tie-break key may read `keyed_fact` without
such a clause, because the up-front check below already guarantees the list.

**Up-front check.** Before it assesses any candidate, the evaluator reads every
declared list once, in document order. A list is unusable when it is missing,
past its own expiry, older than `max_age_ms`, below its confidence floor, not a
record list, or has a malformed row (the key or a declared column missing or of
another type), or when two rows that a reference's `where` keeps share one key.
An unusable list ends the evaluation with an `unknown` outcome carrying
`fact_missing`, `fact_expired`, `fact_stale`, `fact_low_confidence` or
`type_mismatch`, with no candidate verdict. This holds with zero candidates as
well, so `empty` is reachable only after every list was read. No unknown reason
is added for lists.

**Snapshot.** `SelectionFactSnapshot::from_instance_snapshot_with_lists` (and
`from_fact_records_with_lists`) carries the rows of exactly the record lists a
document declares (`SelectionPolicy::record_list_keys`), with timestamps and
durations as integer milliseconds. Every other record list stays
`fact_not_scalar`, so a document that declares none hashes the same
`input_sha256` as before.

**Staleness.** List facts are state facts: they carry no TTL, and a value-hash
snapshot identity keeps an unchanged list from being republished, so its
`published_at` can be old. The document therefore states `max_age_ms`
explicitly. A target list may use up to `MAX_FACT_TTL_MS` (31,536,000,000). A
list whose snapshot identity includes a reset window is republished at every
window switch, and its `max_age_ms` is one window plus a margin, so a missed
reset reads `fact_stale` and the evaluation aborts.

**Obligations of the list publisher** (the decision layer, which publishes these
facts; the evaluator only reads them):

- Each list uses `source_snapshot_id = decision-state:<sha256 of (alias,
  business, fact key, value, window ID)>` and is published only when the active
  record's snapshot ID differs. Republishing an unchanged list with a fresh
  `observed_at` under the same ID would be the fact store's fatal
  `fact_source_snapshot_identity_conflict`; an identical record reuses its
  event, and a new ID needs a strictly newer `observed_at`.
- Fact identities are `(scope, key)`; at most 256 are active per ledger
  lineage and none is reclaimed. A list costs one identity however many rows it
  holds and however often its rows change, because rows are replaced in place.
  The publisher refuses to start when the active identities plus the
  identities it plans would exceed 256.

**List readings** and **field lists** (Workflow #428 S168-4 and Workflow #450;
[resource-readings.md](resource-readings.md), "List readings" and "Lists from fields") are the
second publisher: a package's `read_list` step reads a list a page shows, or a field list groups
OCR fields read one position per frame, and the host writes it under `inventory.list.<name>` at the
run's successful end. Their rules differ from the decision layer's:

- **Whole or not at all.** A list is written only when every position was read; an unread
  position, a repeated key or more than 1024 positions (since S168-LB1a, 0.12.55; 256 before it;
  [resource-readings.md](resource-readings.md), "Rows") writes nothing (no partial list, no
  truncation, no empty list), and the previous record stays until its expiry. That is what makes
  `absent` sound: a key with no row in a usable list reading was not on the list. A list never
  read, or read too long ago, is unusable under the up-front check above: "not read" never reads
  as "not present".
- **An empty list** (the list was read and showed no position) is known and empty: every
  `keyed_fact` read of it is `absent`, and it adds no key to a list maximum (section "Table
  maximum over a list").
- **Fresh at every read.** Every complete read is a new observation
  (`run:<run id>/frame:<frame id>/<operation id>`) with a strictly newer observation time, also
  when its rows did not change, so the record's age is the age of the last read, and it expires
  `valid_for_ms` after it. A document still states `max_age_ms`; one at most the reading's
  `valid_for_ms` makes an old read `fact_stale` before it expires.
- **Complete rows.** Every row holds its key and every column the reading declares, so a
  document may declare any of them. To ask whether a key is listed, a document reads a column
  every row holds, such as a `passed` column, with `absent` the other value.
- **Several lists.** A package may split one list into several (one per kind of member), each
  its own key and fact identity; a document declares each list it reads in `facts[]`. A
  `keyed_fact` reads one list; a list maximum reads 1 to 16 of them at once, over the union of
  their keys (section "Table maximum over a list"). The identities count as above.

## Tables and `table`

Workflow #357 DB-2 (Runtime 0.12.37; register §1.65 TB5–TB10, §1.244; `since` 0.12.37). A document
may read a standard pack's shared table
([package-reference.md](package-reference.md), "Tables"): final values under a row key, with no
expression, float or text column. The Runtime knows the table's paths and column names, never what
they mean.

**Declaration.** The optional member `tables`, at most 8, each
`{table_id, path, row_key, columns}`. `table_id` is the file's, `[a-z][a-z0-9_]{0,63}`; `path` is
exactly `tables/<table_id>.json`, relative to the game root of the package that declares the step.
`row_key` copies the file's. `columns` maps every column the document reads, and every row-key column
but the key, to its type (`integer`, `boolean`, `enum_string`; never a record list). Left empty it is
not written, so a document without tables keeps its canonical bytes, `policy_sha256` and
`input_sha256`; `schema_version` does not change.

**Reference.** `{"source": "table", table_id, key_field, pins, value_column, fallback_column?,
absent?}`.

| Member | Rule |
|---|---|
| `key_field` | A declared `enum_string` field. In a select step, the layout's identity field |
| `pins` | 0 to 7, exactly the row key's columns but the key, each once, so a key names at most one row. `{column, equals}` with a typed literal, or `{column, equals_fact, otherwise?}` with a declared scalar fact of the column's type whose `allowed` lies inside the column's. `otherwise` is one level, `{equals}` or `{equals_fact}`, used when the fact is unavailable |
| `value_column` | A declared column |
| `fallback_column` | Scoring terms only: a different declared column of the same type, read in the same row when `value_column` is null |
| `absent` | Gates only: the value read for a key that has rows, none of this class |

A gate or a term that reads a table declares `drop_candidate` or `abort_evaluation`: missing data
never stands in for a value. A read with pins is itself a gate's read with `absent`, or follows a gate
that reads the same table, key and pins with `absent`: the class gate. A tie-break reads a table only
where a gate or a term reads the same table, key and pins, and with neither `absent` nor
`fallback_column`. All of these are `validate`'s, with no table at hand.

**Binding.** `SelectionTables::bind(policy, tables)` is the one constructor: the file's `row_key`
equals the declared one, each declared column is in the file with its type, and a declared `allowed`
is a subset of the file's. A difference is one word of the vocabulary `table_mismatch`: `row_key`,
`column_absent`, `column_type`, `allowed_not_subset`. A declared table that nothing bound is
`dangling_reference` in `evaluate_with_tables` and `upper_bound_with_tables`, and `evaluate` and
`upper_bound` are those two with no table at all. The crate still reads no file:
`LoadedTable::load(table, now)` takes a table its caller read, turns every cell of a column past its
`valid_until_unix_s` to null, and indexes the rows by key.

**One read**, in this order:

1. The key is the candidate's `key_field`.
2. Each pin's value: the literal, the fact, or its `otherwise` (reason `term_fallback_used` with
   `table_column`). Still unavailable, the read is Unknown with the first fact's own reason.
3. The row. No row has the key: Unknown, `lookup_miss`. The key has rows and none holds the pins: the
   read's `absent`, a known value, and the class gate decides against the candidate.
4. The cell. Null, also by expiry: the `fallback_column` in the same row when it is not null (reason
   `term_fallback_used`), else Unknown, `lookup_miss`.
5. An `enum_string` value outside the document's `allowed`: Unknown, `type_mismatch`, so no lookup's
   default can swallow a value the table added.

`lookup_miss` means here that nothing by table or by lookup transform holds a value.

**Static table drop.** A candidate dropped under `drop_candidate` only by steps 3 to 5 is dropped by
the table's own data, which reading again cannot change. A fact behind a pin is instance data and
never counts, nor does a value the term's own transform cannot use. The verdict's
`static_table_drop` says so, and is never serialized: the record holds `unknown_dropped` with its
reason, as for any drop. The select step does not count such a candidate as an unknown instance, so it
blocks nothing ([selection-graph.md](selection-graph.md), "Tables").

**Input identity.** A document that declares a table adds the member `tables` to the hashed input: an
object by `table_id`, each value the rows the references read for the candidates, each row an object
of its key column and its declared columns, in the order of the row key's projection, each once. An
expired column is the null the run read. A row read for no candidate is not in it, so the hash moves
when a row read moves and for no other. Tables are outside every package's content hash and the
suspension identity (M-357a v1.1 §2.7); no record names their release.

**Scheduling.** `evaluate_collect` refuses a document that declares a table, and the catalog compile
refuses it with `type_mismatch` (`tables=declared consumer=scheduling`), as for a record list. Dispatch
value goes through the resource line (Alice's 121 A); only a task's select step reads a table.

## Table maximum over a list

Workflow #428 S168-5a, written ahead of its code by S168-0 and corrected by its fix 1, built by
S168-5a (since 0.12.50; [v012-register.md](v012-register.md) §1.291, "Slice S168-5a"; the code
in `crates/selection-policy/src/list_max.rs`). A document may read the largest value one integer
column of a declared table holds over the keys one or more declared record lists list: how much
the best listed member is worth in one class of the table. The Runtime knows neither what the
table nor what the lists mean. The names are neutral examples:

```json
{"source": "table_max_over_list", "table_id": "member_values", "value_column": "value",
 "pins": [{"column": "domain", "equals": "domain.x"}],
 "lists": [{"fact_key": "inventory.list.members_kind_a",
            "where": {"column": "state", "equals": "state.a"}},
           {"fact_key": "inventory.list.members_kind_b"}],
 "empty": {"type": "integer", "value": -1}}
```

| Member | Rule |
|---|---|
| `table_id` | A table the document declares (section "Tables and `table`") |
| `value_column` | A declared column of type `integer`. Integers only: a `boolean` or `enum_string` column is a `type_mismatch` validation error |
| `pins` | As a `table` read's: 0 to 7, exactly the row key's columns but the key, each once, `{column, equals}` or `{column, equals_fact, otherwise?}` |
| `lists` | 1 to 16 entries `{fact_key, where?}` with distinct `fact_key`s; none, a 17th or a repeated `fact_key` is a validation error |
| `lists[].fact_key` | A record list the document declares in `facts[]`; the values of its `key_column` are the keys looked up in the table's key column |
| `lists[].where` | Optional, as a `keyed_fact` read's: keeps only the rows of its list whose `enum_string` column equals `equals` |
| `empty` | Required, `{"type": "integer", "value": n}`, n within ±(2^53 − 1): the known value read when every list is usable and no key of the union is of the class. The document states it in the open; the Runtime never supplies one, and it is 0 only when the document writes 0 |

**The union.** The keys a reference reads are the union of the keys its lists hold, each list's
rows after its own entry's `where`. A key held by several lists counts once: it is one key of the
union, looked up once.

**One read.** The value does not depend on the candidate: the evaluator reads each distinct
reference once per evaluation. Two references are the same reference when their canonical forms
are equal; any difference, the order of `lists` included, makes two. It reads them after the
up-front check of the record lists passed and before it assesses any candidate (so also with
zero candidates), in the order in which they first appear in the document: the gates in order,
each predicate depth first and its left side before its right, then the scoring terms in order.
Each read runs these steps:

1. The lists. A listed list that is not usable as section "Record lists" defines it (missing,
   expired, stale, below its floor, of another type or malformed, two rows its entry's `where`
   keeps sharing one key included, as the up-front check reads it) makes the read Unknown with
   that list's reason, the first such list in the order of `lists`, and nothing more is read.
   Every listed list is declared in `facts[]`, so the up-front check of section "Record lists"
   has read it before any candidate: there an unusable list ends the evaluation `unknown`, with
   no verdict, as for any declared list, with the reason of the first unusable list in the
   declared order of `facts[]`, and no list maximum is read. Step 1's own order, that of
   `lists`, is therefore what a direct read of one reference shows (a reference read alone, as a
   test reads it); a whole evaluation shows the up-front check's.
2. Each pin's value: the literal, the fact, or its `otherwise` (reason `term_fallback_used`), as
   a `table` read's step 2. Still unavailable, the read is Unknown with the first fact's reason.
3. Each key of the union, in ascending byte order. No row of the table has the key: the read is
   Unknown, `lookup_miss`. The key has rows and none holds the pins: the key is not of the class
   and adds nothing. The row's cell is null, also by expiry: the read is Unknown, `lookup_miss`.
   A row a `where` leaves out gives no key: it is not looked up and is no miss.
4. The value: the largest cell of the keys of the class; with none, `empty`.

| Case | The read |
|---|---|
| A listed list is unusable | Unknown, that list's reason, the first in the order of `lists` (step 1), when the reference is read alone; in a whole evaluation the up-front check ends it first, `unknown` with the first unusable list in the order of `facts[]` |
| Every list is usable and the union is empty (no row, or none that a `where` keeps) | `empty`, known |
| Every list is usable and no key of the union is of the class | `empty`, known |
| One key of the union has no row in the table | Unknown, `lookup_miss`, whatever the others hold: the missing key may hold the largest value |
| Every key of the union has no row in the table | Unknown, `lookup_miss` |
| A key of the class has a null cell | Unknown, `lookup_miss` |
| A pin is unavailable | Unknown, the first fact's reason |

A missing value is never read as 0 and never skipped: only a key the table knows and places
outside the class adds nothing. A reference that opts in (section "Lower bound of a list
maximum", S168-LB1b, since 0.12.55) reads what is known as a bound instead of Unknown, and still
never reads a missing value as 0.

**What the table owes a list maximum.** So a missing row of a class is a statement, never a gap:
it says the identity is not of the class. A table a list maximum reads, for every class a
reference pins, gives

- an identity with a value in the class: a row of the class with the value;
- an identity that may be of the class, whose value is not known: a row of the class with a null
  cell, which reads Unknown (`lookup_miss`). The `value_column` is then `nullable`: the table
  format lets an `integer` column hold null cells ([package-reference.md](package-reference.md),
  "Tables": `nullable` lets a cell be `null`);
- an identity known not to be of the class: no row of the class;
- every identity a listed list can hold: at least one row, so that no key of the union misses;
  an identity of no class at all gets a row of a class no reference pins.

Giving an identity whose value is unknown only a row in some other class would say that the
identity is not of this class: it would drop out of the maximum silently, the maximum would come
out too low, and a comparison against it could pass when it must not. The Runtime cannot tell such a row from a
true one; the table's builder owes the rule.

**Where it is read.** As a side of an `integer_compare` gate predicate, and as the input of a
scoring term through `identity` or `threshold`. A gate or a term that reads it declares
`drop_candidate` or `abort_evaluation`; a substitution is a `type_mismatch` validation error, as
for a table read: missing data never stands in for a value. A list maximum that reads Unknown
(any row of the case table that reads Unknown) is an Unknown read like a table read's: under
`drop_candidate` it drops the candidate, `unknown_dropped` with the read's reason; under
`abort_evaluation` the evaluation is `unknown`, with that reason and the rule. Such a drop is no
static table drop (section "Tables and `table`": only a `table` read's steps 3 to 5 make one, and
the lists are instance data), so a step that counts unknown instances counts it. A tie-break key
cannot read it (it is the same for every candidate), nor can a `fallback_column` name it; both
are `type_mismatch` validation errors. A document holds at most 32 list maxima, each over at
most 16 lists; a 33rd is a validation error. The 32 count distinct references (the same
reference, as "One read" defines it, counts once however often it occurs), since each is read
once and adds one reason.

**Several lists.** One reference reads every list it names, so a value is compared with the
largest over a list split by kind of member (one list per kind) by one `integer_compare` against
one reference that names them all, over their union. A document may still write one reference per
list when it wants a comparison per list. It then minds that each reference stands alone: one
whose list holds no key of the class reads its own `empty`, which decides its comparison, so each
`empty` lies on the side that comparison intends (below every value of the class for "above the
best listed"); each is Unknown on its own, and an `all` or `any` of the comparisons combines
three-valued verdicts; a key two lists hold is read by both references; and each reference counts
against the 32.

**Binding.** `SelectionTables::bind` checks the declared table as for a `table` read: the column
is in the file and of type `integer`, else `table_mismatch` (`column_absent`, `column_type`).

**Audit.** Each reference read adds one reason to the decision's reason chain,
`selection_table_list_max` (info), where the read happens: after the reasons the chain opens
with (both hashes and the declared requirement, section "Decision") and before any reason of a
candidate's assessment (an abort's included) and the outcome's reasons, one per distinct
reference in the order of the reads ("One read"). A pin that used its `otherwise` adds its
`term_fallback_used` just before its read's reason, with `rule_id` the gate or term that reads
the reference first and `table_column` the pin's column, one per such pin in the order of
`pins`. An evaluation the up-front check ends adds none. The reason carries `table_id`, `table_column` (the `value_column`),
`fact_keys` (the lists it read, in the order of `lists`: a list value of the catalog's
`list<token>` type, at most 16 items, so never cut), `count` (the number of distinct keys of the
union) and either `maximum` with `maximum_identity` (the value and the smallest key, in byte
order, that holds it; neither for `empty`) or `unknown_reason`. The reasons count against the
decision chain's 128 reasons and the record's 64 KiB of compact JSON. A record over 64 KiB is
`selection_record_too_large`: the producer checks it before any input, so the run fails with that
code and nothing is truncated ([selection-graph.md](selection-graph.md), "Validation and
bounds"). With the record's `policy_sha256` and `input_sha256`, the
`standard_pack_table_read` digest of the table ([selection-graph.md](selection-graph.md),
"Tables") and the lists' rows in the snapshot, a reviewer reads the value again. The values say
what the instance holds: they are account data, and stay in the ledger.

**Input identity.** Every listed list contributes. The hashed input's `tables` member also holds,
under the table's id, every row the read took (the row of each key of the union that holds the
pins), each once, as a `table` read's rows; the rows of every listed list are already in the
snapshot. The hash moves when a contributing row or a row of any listed list moves, and for no
other row.

**Upper bound.** The read is exact, as a scalar fact is. Unknown, it drops the identity under
`drop_candidate` and gives `Unbounded` under `abort_evaluation`, as any exact unknown input. A
reference that opts in and reads "at least L" is the range [L, +∞) (section "Lower bound of a
list maximum").

**Scheduling.** As for tables and record lists: `evaluate_collect` and the catalog compile refuse
a document that uses it (`type_mismatch`).

**Compatibility.** A document that does not use it keeps its canonical bytes, `policy_sha256` and
`input_sha256`. A reader before S168-5a refuses a document that uses it (an unknown value
source). In a package it belongs to `package` revision 8, with S168-4's `read_list`
([component-interfaces.md](component-interfaces.md), "Planned: Workflow #428 168").

## Lower bound of a list maximum

Workflow #428 S168-LB1b, since 0.12.55 ([v012-register.md](v012-register.md) §1.309, built by
§1.312; the coordinator's rulings 5 to 10 of 11-11, not Alice's words; the code in
`crates/selection-policy/src/list_max.rs`, `evaluator.rs` and `bound.rs`; the judge's use, "In a
judge step" below, is S168-5b's and not built yet). Under the read above, one key of the
union without a value (no row in the table, or a null cell of the class) makes the whole read
Unknown, whatever the other keys hold. Yet the largest value of a group can only grow when a
missing value becomes known: the values already known bound it from below. A reference opts in
with one optional member; without it the read is the one above, byte for byte. The names are
neutral examples:

```json
{"source": "table_max_over_list", "table_id": "member_values", "value_column": "value",
 "lower_bound_column": "value_at_least",
 "pins": [{"column": "domain", "equals": "domain.x"}],
 "lists": [{"fact_key": "inventory.list.members_kind_a"}],
 "empty": {"type": "integer", "value": -1}}
```

| Member | Rule |
|---|---|
| `lower_bound_column` | Optional. A column the document declares for the table, of type `integer`, other than `value_column`: the lower bound the table's maker gives a key whose value is not known, read in the same row as the value cell. An undeclared column is a `dangling_reference` validation error, another type or the `value_column` itself a `type_mismatch`, each with the detail `table_max_over_list=lower_bound_column`, as for the other members. `SelectionTables::bind` checks it as it checks `value_column` (`table_mismatch`: `column_absent`, `column_type`). Left out, it is not written, so a document without it keeps its canonical bytes, `policy_sha256` and `input_sha256`; two references that differ only by it are two references ("One read") |

**The opted-in read.** Steps 1 and 2 of "One read" (the lists, the pins) and every Unknown they
give are as above. Step 3 reads each key of the union, in ascending byte order:

| The key | It adds |
|---|---|
| has rows, none of which holds the pins | nothing: it is not of the class, as above |
| has a row of the class whose value cell holds a value | a known value |
| has a row of the class whose value cell is null (also by expiry) and whose bound cell holds a value | that bound; the read is no longer exact |
| has a row of the class whose value cell and bound cell are both null | nothing; the read is no longer exact |
| has no row in the table | nothing; the read is no longer exact, since the key may be of the class |
| has a row of the class whose value or bound cell holds another type | the read is Unknown, `type_mismatch`, as above |

Step 4, the value:

| Case | The read |
|---|---|
| No key is uncertain: every key of the union is outside the class, or of the class with a value | **exact M**: the largest value, with the smallest key that holds it; `empty` when no key is of the class, the empty union included, as above |
| A key is uncertain, and at least one value or bound is known | **at least L**: L is the largest of the known values and of the bounds, with the smallest key, in byte order, that gives it |
| A key is uncertain, and no value and no bound is known | Unknown, `lookup_miss`, as above: nothing is known of the class |

An uncertain key never makes the read Unknown while anything is known of the class; it only makes
the read "at least". Only a group of which nothing is known is Unknown, as today. A usable union
with no key of the class reads `empty`, a known value, as today.

**Reading an at-least value.** The read is the range [L, +∞), and every reader takes it so:

- **`integer_compare`.** True or false only where every value of the range gives the same
  answer; Unknown otherwise, which the gate's `on_unknown` decides as for any Unknown read
  (`drop_candidate` or `abort_evaluation`), with the reason `lookup_miss`, so no reason word is
  added. With the list maximum on the left and a known right side, R′ = right × `mul` + `add`:
  `ge` is true once L ≥ R′ and `gt` once L > R′; `le` and `eq` are false once L > R′ and `lt`
  once L ≥ R′; `ne` is true once L > R′. On the right the range is [L × `mul` + `add`, +∞), and
  against a known left side Y: `le` is true once Y ≤ L × `mul` + `add` and `lt` once
  Y < L × `mul` + `add`; `ge` and `eq` are false once Y < L × `mul` + `add` and `gt` once
  Y ≤ L × `mul` + `add`; `ne` is true once Y < L × `mul` + `add`. Two at-least sides settle
  nothing. An exact side and an Unknown side are read as above.
- **`threshold`.** `then_milli` once L ≥ `at_least`; Unknown otherwise, which the term's
  `on_unknown` decides.
- **`identity`.** The term takes L as its value, and the decision says so (the reason below):
  its contribution is L × `weight_milli` / 1000. With a negative weight that contribution is the
  most the term can give, not the least.
- **The upper bound** ("Upper bound" below) takes the range [L, +∞): a gate compares it as the
  range of a ranged input; `identity` has no finite bound with a positive weight (`Unbounded`)
  and its bound at L with a negative one (or a zero one, whose contribution is 0 whatever the
  value); `threshold` gives `then_milli` once L ≥ `at_least` and
  otherwise the larger of its two contributions. An exact read stays exact and an Unknown one is
  as above.

**What a person sees.** For a reference that opts in, the read's `selection_table_list_max`
carries `bound` (a new outcome vocabulary, `list_max_bound`: `exact` or `at_least`) and
`unknown_count` (the keys of the union with no value: no row, or a null value cell of the class);
with `exact`, `maximum` and `maximum_identity` as above (neither for `empty`); with `at_least`,
`lower_bound` (L) and `lower_bound_identity` (the key that gives it) in their place. An Unknown
read carries `unknown_reason` as above, and neither `bound` nor `unknown_count` (S168-LB1b's
reading: `bound` names only the two known readings). A reference without the member carries the
keys of before, byte for byte. A decision made on a lower bound, that is one in which an at-least
read settled a gate verdict or a term of a candidate (a comparison the bound settled in a gate
whose verdict is known, a threshold the bound settled, an identity term taken at L), carries one
info reason more, `selection_decided_on_lower_bound` (`rule_id` the first such gate or term in
the order of "One read", `count` how many distinct gates and terms were, over every candidate),
once, in the decision's chain after the candidates' assessment and before the outcome's reasons;
an `unknown` evaluation carries none. The four keys, the vocabulary and the code are registered
since 0.12.55 (S168-LB1b, [v012-register.md](v012-register.md) §1.312); the reasons count
against the chain's 128 reasons and the record's 64 KiB as every reason.

**In a judge step** (S168-5b, planned: [selection-graph.md](selection-graph.md), "Judge step",
"Which key"). A list maximum read as a lower bound is no Unknown read: a comparison the bound
settles is a known verdict (`ranked` or `gate_rejected`), so the judgment is `judged`; one it
does not settle is Unknown, so its candidate is `unknown_dropped` (or the evaluation `unknown`)
and the judgment is `unknown`; a score taken at L is a `ranked` verdict, `judged`, and the record
carries the bound in the two reasons above. The judge's record gains no member.

**What the table owes a lower bound.** The table's maker writes the bounds, in a nullable integer
column of the same table (`nullable`, [package-reference.md](package-reference.md), "Tables"):
for a key of the class whose value is not known, a value it is known to reach; a null bound says
that nothing is known. The Runtime computes no bound and reads no meaning into the keys or the
cells. The rules of "What the table owes a list maximum" stay; with the member a key without a
row makes the read "at least" instead of Unknown, and a row of another class still says that the
key is not of this class.

**Input identity.** The bound column is a declared column of the table, so each row the read
took carries its bound cell in the hashed input, as every declared column does; a key without a
row adds no row, as before.

**Scheduling.** As for the source itself: `evaluate_collect` and the catalog compile refuse a
document that uses it.

**Compatibility.** A reference without the member reads, hashes and decides byte for byte as
before. A reader before S168-LB1b refuses a document that writes it at admission
(`contained_task_select_policy_invalid`: the reference denies unknown members), as it refuses
any member it does not know, and so does the parser of its build. The selection format has no `requires`-style gate of its own (its
`schema_version` does not change): the gate is the bundle's declared `package` range and the
reader's refusal. In a package the member belongs to `package` revision 9, in force since
0.12.55 ([component-interfaces.md](component-interfaces.md), the row 9 of the `package`
interface; "Planned: Workflow #428 S168-LB1").

## Upper bound

`upper_bound(policy, facts, now_unix_ms, identities, field_domains)` returns the
largest score a candidate that was not read could reach: `NoneEligible`,
`Max(score_milli)` or `Unbounded`. It is pure, like `evaluate`.

- `field_domains` names the domain of every declared field: an `enum_string`
  field is `Identity`, which carries the hypothetical identity exactly; a
  `boolean` field takes either value; an `integer` field takes every value of
  its `Integer { min, max }`. The caller derives them from the layout.
- For each of `identities`, identity fields, scalar facts and `keyed_fact`
  values are exact. A gate excludes the identity only when it is false for every
  value of the domains; a gate that is true or unknown for some value may pass.
- Each term contributes the largest of its possible contributions, with the
  sign of its weight: `identity` at the domain's extremes, `threshold` at
  whichever of its values the domain reaches, `lookup` at the entries the domain
  reaches and its default. A `substitute_milli` value joins the candidates
  whenever the input is ranged or unknown. An exact input that is unknown drops
  the identity under `drop_candidate` and gives `Unbounded` under
  `abort_evaluation`.
- An `integer_compare` gate compares the ranges of its two sides, the right one
  scaled by `mul` and `add`: `lt`, `le`, `gt` and `ge` exactly, `eq` and `ne` as
  if every integer between the scaled ends could occur. A side that is unknown
  leaves it unknown.
- `NoneEligible` means no identity can be ranked. Leaving the 64-bit range, or
  a declared list that cannot be read, gives `Unbounded`.

A table value is exact for a hypothetical identity, as a `keyed_fact` value is, and so is a pinned
fact in the snapshot. A gate that reads a table excludes an identity it cannot hold for under
`drop_candidate`, and has no finite bound where it may be unknown and cannot hold under
`abort_evaluation`; every other gate is as it was, so a document without a table keeps its bound. A
key field that is not the identity domain has no finite bound (Workflow #357 DB-2).

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
keys 16; lookup entries 512 per transform; enumerated members 1024, the largest
business identity domain a pack may declare; record-list columns 63 per list;
predicate depth 16 and predicate nodes 512 per gate, matching the scheduling
predicate limits; identifiers, fact keys, and outcome keys 128 bytes; one
document 512 KiB; list maxima 32 distinct references per document and lists 16 per list
maximum (Workflow #428 S168-5a).

## Scheduling consumer

The scheduling policy crate consumes a selection-policy document as the optional
fifth catalog document (see [the scheduling contract](scheduling/README.md),
section "Score-Assisted Priority"). It uses the document as a scorer, not as a
chooser:

- Candidates are the eligible (task, instance) pairs of one evaluation,
  identified as `<task_id>@<instance_id>`, with the fields `task.priority`,
  `task.strategic_weight_milli`, `task.urgency_milli`, `task.aging_ms`,
  `task.load_cost_milli`, `instance.affinity`, for a task with catalog
  `reserves` (Workflow #411 B16 part 2) `task.consumes.<pool>`,
  `task.after.<pool>` and `task.reserve_gap.<pool>` per reserved pool (the last
  two while the pool's amount is known; the scheduling contract's "Task
  Reserves"), plus the instance's scalar facts that have not expired at the
  evaluation instant, under their own keys. A document declares whichever of
  these it reads.
- The fact snapshot is the instance's own projection of the evaluation's facts,
  built the same way `from_fact_records` builds one: most specific scope per key,
  record lists unusable, timestamps and durations as integer milliseconds. The
  evaluator is called once per instance.
- The call is the collect option, `evaluate_collect`, so the document's
  `on_unknown` clauses do not apply at this layer. Only a candidate's score is
  read, and a candidate a gate rejects carries none. A candidate with an unknown
  input is deferred on its own, under the reasons the scheduling contract names,
  and the scheduling evaluation goes on; `selection` and `tie_break` are not
  used.
- The document is hashed into the catalog identity and validated at catalog
  compile time with this crate's `validate`, so an invalid document rejects the
  whole catalog instead of being skipped.
- A document that declares a record list is refused at catalog compile with
  `type_mismatch`: record lists stay unusable to the scheduler, and `keyed_fact`
  is for in-task documents only.
- Since S168-LB1a (0.12.55; [v012-register.md](v012-register.md) §1.309, §1.311) the instance's
  projection holds no record under `inventory.list.`
  ([instance-fact-store.md](instance-fact-store.md), "Projection into the evaluation"), and a
  document that declares a fact under that prefix, of any type, is refused at catalog compile
  with `type_mismatch`, as a catalog predicate that names one is
  ([scheduling/README.md](scheduling/README.md), "Record lists and the projection").

## In-task consumer

A select step of a contained task (see [the selection graph](selection-graph.md), section
"Select step") consumes a document of its own package as a chooser:

- The document is a task file at `policies/<name>.json`, sealed by the SHA-256 of its bytes,
  read with this crate's rules in this order: the 512 KiB limit, the typed decode, the
  canonical form and `validate`. The parser checks it while authoring and Runtime admission
  again before any run.
- Candidates are the actionable candidates of the step layout's candidate projection
  ([candidate-projection.md](candidate-projection.md)), identified by their candidate IDs
  `<layout_id>#NN`. Each feature is a field of its name: a `passed` feature is a `boolean`, a
  `measure_milli` or `ocr_integer` feature an `integer`, and an `identity` feature an
  `enum_string` of the declared business-ID domain. The document's `fields` are features of that layout
  with those types, and `applies_to.candidate_layout_id` is the step's layout. A candidate whose
  feature is absent has no such field, and the document's `on_unknown` decides. `confidence` is
  not handed to the evaluator in v1.
  Identity rejection, explicit attribute fallback and unresolved consensus obey
  [business-identity-consensus.md](business-identity-consensus.md) before an input is permitted.
- The fact snapshot is `from_instance_snapshot_with_lists` of the instance's own
  ledger-pinned fact snapshot, which the host takes for the step, with the record lists the
  document declares; the evaluation instant is the host's clock when the snapshot is taken.
  The evaluator still reads no clock and no ledger.
- The snapshot also holds the run's read-only parameters (Workflow #411 B15,
  [remedies.md](remedies.md), section 4; written ahead of slice R5): one record
  `session.dispatch.parameter.<name>` = `<value>` (a string, instance scope) per parameter the
  package declares, the value its policy dispatch carries in `policy_dispatch_parameters`, or the
  declared default for a run without a dispatch. The records are an overlay, never stored. A
  document reads one as an `enum_string` field with the declared options and should use
  `on_unknown: abort_evaluation`; `input_sha256` covers the overlay, so two runs that differ only
  in a value differ in that hash. Workflow #411 B8: a policy run of a catalog task that opts in
  (`dispatch_parameters: true`) also holds its dispatch's schedule values in the same overlay,
  `session.dispatch.schedule.<key>` (`next_occurrence_unix_ms` and `window_end_unix_ms` read as
  `integer`, `last_in_window` and `last_daily_slot` as `boolean`, `daily_remaining` as `integer`);
  any other run holds none, and the document's `on_unknown` decides. SS-6 (since 0.12.31): a
  policy run whose dispatch carries spend values also holds `spend_allowance.<i>`,
  `gap_remaining.<j>` and `run_cap` under the same prefix, each read as `integer`
  ([remedies.md](remedies.md), section 4, "Spend values").
- A document that declares a record list needs a layout whose `unknown_identity` is
  `reject`, so the evaluator never meets an unknown key; admission and the parser refuse it
  otherwise (`contained_task_select_policy_mismatch`, `resource_declaration_invalid`).
- `selection.required_count` is `1`: a step chooses exactly one candidate. Only a `selected`
  outcome leads to an input; every other outcome fails the step (`selection_not_selected`),
  except an `empty` decision of a step that declares `on_empty`, which ends the run normally
  ([selection-graph.md](selection-graph.md), "Ending empty"), in a page-graph task and (Workflow
  #463 K1) in a `linear_steps` task alike.
  The zero-padded candidate IDs make `candidate_id` a stable last tie-break key.
- The decision is recorded whole in `task.selection_evaluated`, with `policy_sha256`,
  `input_sha256` and the document's package SHA-256.
- A judge step (Workflow #428 S168-5b; [selection-graph.md](selection-graph.md), "Judge step")
  consumes a document of its package the same way, with three differences: `selection.mode` is
  `none_allowed` and `required_count` at least its layout's slot count, so every candidate that
  survives is chosen and neither `insufficient` nor `ambiguous` can occur; no outcome leads to an
  input; and every outcome, `unknown` included, ends the run normally, with the step's
  `outcome_key` only for a known evaluation whose unknown check does not block and in which no
  candidate is `unknown_dropped`, and its `unknown_outcome_key` otherwise
  ([selection-graph.md](selection-graph.md), "Judge step", "Which key").

This crate is unchanged by that use: the scheduling consumer's `catalog_hash`, which depends on
the document's serialization, keeps its value.

## Offline tool

`selection-eval --policy <file> --candidates <file> --facts <file>
[--now-unix-ms <n>]` runs one evaluation and prints one JSON envelope: the
decision, or a typed error. Exit code 0 carries a decision, exit code 2 an
error. The evaluation instant defaults to the snapshot's own instant, so the
tool reads no clock. It is a debugging aid and not a Runtime entry point.
