# Scheduling Catalog Contract

The versioned [V2 contract](v2/README.md) adds explicit timeline availability
through the same compiler and evaluator. This document specifies V1.

The scheduling catalog is a data-only contract of four required documents plus one optional selection document. It cannot contain executable code, scripts, network requests, device actions, or implicit defaults.

## Offline inspection

`actinglab scheduling compile --tasks <tasks.json> --pools <pools.json>
--activity <activity.json> --timeline <timeline.json> [--selection <selection.json>]`
reads four explicit local files, plus the optional selection document when
`--selection` names one, through Lab and calls the existing policy
`compile_catalog` entrypoint.
With `--json`, one ordinary CLI envelope contains the compiler's `dry_run_json`
report, including summary and catalog hash. Rejection returns exit 2 and the
original structured compiler diagnostics under `error.details`. File read errors
also fail explicitly. V1 compilation and catalog identities retain their contract.

`actinglab scheduling timeline` takes those same file flags (including the
optional `--selection`) plus repeated
`--event-id <id>`, `--unix-ms <u64>`, `--monotonic-ms <u64>`,
`--instance-id <id>`, `--server-id <id>` and `--game-id <id>`. All are explicit;
instance context must correspond to the configuration being inspected. The
command reads no system clock, Runtime configuration, session, facts, capabilities
or task state. Routing/backend/execution global flags are rejected. Both commands
are `offline` and `read_only` and create no state directory or persistent facts.

The timeline response contains the compilation report and the pure policy query:
catalog ID/version/hash, supplied time and context, requested events in request
order, each event's declared scope and `scope_applies`, `availability.state`
(`true`, `false` or `unknown`), `active_interval` as `[start, end]` Unix milliseconds
representing the half-open interval `[start,end)`, and `next_wake_unix_ms`.
An inactive event has a null active interval. The top-level query wake is the
minimum wake across the selected events; it is not the full scheduler's wake.
Unknown or duplicate event IDs and empty selections are input errors.

Policy `inspect_timeline` shares the availability and scope owner used by
`TimelineActive`, including its existing clock, duration and validity semantics.
Positive availability requires explicit V2 validity. Zero-duration reset events
remain unknown for availability, including V1 reset events; their production
invalidation purpose is unchanged. Applicable closed/expired events are false,
and inapplicable scopes are unknown. Arithmetic failures retain policy errors.

Each source is limited to 1 MiB, and the compiler enforces the 4 MiB catalog bound.
The reader uses at most one additional sentinel byte per document to preserve the
compiler's size diagnostic. Paths use the existing 1024-byte source-text bound;
server/game context and event IDs use 128 bytes; the instance context uses the
registered 256-byte alias rule. Selection is limited to 4096 events. CLI query
arguments are bounded to 8212 tokens and 1 MiB, enough for all events and ten
single-value flags. Output payloads are limited to 16 MiB (four catalog budgets)
to allow structured source locations and diagnostics. Exceeding a bound fails
visibly without silently truncating a decision.

These commands verify compilation and event availability at a supplied time.
Production catalog activation, PolicyHost binding, admission and execution remain
the Runtime's normal chain; an offline query establishes no production run fact.

## Frozen V1 Documents

- `tasks.schema.json`: task entrypoints, bounded triggers, feedback stop conditions, effects, failure policy, load profile, loop budget, and instance overrides.
- `pools.schema.json`: scoped resource pools, regeneration or nonregenerating projections, observations, bounded group delay, and an optional resource valuation ("Pool Valuation" below).
- `activity.schema.json`: scoped activity windows, per-instance importance, bounded sessions, sampling policy, and goals.
- `timeline.schema.json`: scoped reset, maintenance, activity, and deadline events.
- `diagnostic.schema.json`: stable compiler diagnostic envelope.
- `selection` (optional fifth document): an `actingcommand.selection-policy.v1` scoring
  policy owned by [the selection-policy contract](../selection-policy.md). It carries its
  own schema version and no `catalog` descriptor.

All four catalog documents must carry the exact schema version `actingcommand.scheduling.v1` and an identical `catalog` descriptor. A mismatch rejects the whole catalog.

## Pool Projection

`projection` is required in V1 and V2 pools and accepts exactly one of these closed
objects:

- `{"amount": 1, "per_ms": 1000}`: both integers are positive. Each complete period
  adds `amount`, up to `capacity`, using the existing regeneration and wake rules.
- `{"kind": "none"}`: an available snapshot keeps its amount as time passes. It
  produces no regeneration wake and contributes zero fullness urgency to tasks
  consuming it. Deadline urgency and urgency from other regenerating pools remain.

Missing or null projections, unknown kinds or fields, mixed shapes, and zero
regeneration amounts or periods reject the catalog. Existing regenerating objects
retain their serialization and catalog hash. A `none` declaration changes the hash
and follows the ordinary version, approval and activation path.

Capacity keeps its positive safe-integer bound. Zero is a known empty amount;
missing observations remain Unknown. For inventory, a declared numeric ceiling
does not assert an account's physical storage limit. Neither projection mode
clamps an invalid observed amount into a valid balance. A V2 pool may instead
read its capacity from a fact of its scope (#386 CF-5, "Pool fact references"
below): an account's cap is read, never written as a constant.

Projection and value source are independent. `static_snapshot` keeps the caller's
fixed snapshot; it has no additional TTL or automatic update from resource readings.
`ledger_fact` keeps the original scope/key, confidence, expiry, native-instance
binding and input invalidation requirements. FactsChanged, expiry, timeline and
task-state wakes remain; time passing never refreshes a fact. Workflow #390
POOL-1: a package resource reading (or adjustment write) of the key is the live
pool's base, published without input invalidations, which the pool needs only
from a record that is not such a reading; the next reading or adjustment resets
the base, and the projection above regenerates from its observation time
(`contracts/resource-readings.md`, "Live pools"). A live pool with no usable
record (none, expired, below the pool's minimum confidence, or not an integer
within `0..=capacity`; #386 CF-5: against a capacity read from a fact, any
non-negative integer is usable) leaves its `resource_projection` predicate Unknown with
that fact reason (`fact_missing`, `fact_expired`, `fact_low_confidence`,
`type_mismatch`) instead of `resource_observation_missing`, so the task raises
the fact refresh's demand for the key ("Fact Refresh"). A resource target on a
live pool reads the same projected inventory ([resource-targets.md](../resource-targets.md)).

The pure forward projection advances a nonregenerating pool's simulated time
without increasing its amount or accumulating regeneration waste. Declared task
effects still affect the simulation, including insufficient-resource rejection,
production clipping and waste at capacity, overflow and insufficient-evidence
handling. It does not update real inventory or renew the observation's TTL.
`consumes`, `produces`, valuation, eligibility and resource-target scoring keep
their existing meanings; they do not reserve or settle real resource quantities.

A Runtime without `none` rejects the new shape. Historical catalog generations
are compiled when loaded, including for dispatch recovery, so switching the active
configuration to an older catalog does not establish binary downgrade safety for
a lineage containing `none`. Catalog rollback uses the existing approved-catalog
route on a Runtime that understands its recorded generations. No fact or event
wire field is added.

## Pool Valuation

Workflow #335 S2a. A pool may end with an optional `valuation` block that states
what one step of its resource is worth. V1 and V2 catalogs accept it alike, and a
catalog without it keeps its identity byte for byte ("Canonical Serialization And
Hash" below).

```json
"valuation": {
  "name": "Primary energy",
  "unit": "point",
  "scale": 1000,
  "base_weight_milli": 100,
  "gap": { "rule": "shortfall_linear", "weight_milli": 100 }
}
```

| Field | Bound | Meaning | Diagnostic |
| --- | --- | --- | --- |
| `name` | 1..=128 UTF-8 bytes, nonblank after trimming whitespace, no control characters | display name only | longer: `limit_exceeded`; blank or control character: `type_mismatch` |
| `unit` | 1..=32 UTF-8 bytes, the same text rule | display unit only | as `name` |
| `scale` (Q) | 1..=9,007,199,254,740,991 | measuring step: the resource amount one weight applies to | `limit_exceeded` |
| `base_weight_milli` (B) | 0..=1,000,000 | standing weight in milli for each produced step | `limit_exceeded` |
| `gap` | optional | shortfall conversion; when absent, a resource target must carry its own importance | — |
| `gap.rule` | `shortfall_linear` only | the target is to hold at least an amount | `type_mismatch` |
| `gap.weight_milli` (G) | 1..=1,000,000 | extra milli for each produced step per missing step of shortfall | `limit_exceeded` |

The resource kind is the pool `id`; there is no separate kind field. Task
quantities stay in `tasks[].produces` (`amount` or v2 `expected_amount_milli`, plus `confidence_milli`). Both
objects reject unknown fields (`unknown_field`), and a missing required field is
`missing_required_field`. Every violation rejects the complete catalog. A bound
violation or an unknown `rule` value carries the field's own JSON Pointer and
its source line and column. For a missing or unknown field the reason names the
field, while `json_path` and the line and column point at the nearest preceding
value (the existing parser's behaviour).

SS-2b (since 0.12.32; register §1.225; Alice's ruling 116, 「无害占位符或者空值」): in a V2 catalog a
pool may write `"valuation": "pending"`, the marker that its worth is not known yet. It does
nothing but make the net benefit of a judged task that touches the pool unknown ("Net benefit
within a tier"): every other reader sees no valuation, as without the member. Any other string
is `type_mismatch` at `/pools/<i>/valuation`, and so is the marker in a V1 document; `null`
stays what it was, no valuation. A catalog without the marker keeps its hash.

The block is a declaration. The compiler validates it and hashes it with the
catalog. Since SS-1 (0.12.30; Alice's ruling 100) the evaluator reads it for
every instance: its base weight weighs every producing task with or without a
resource target policy, and an active `actingcommand.resource-targets.v2`
policy only adds its targets' shortfall (`contracts/resource-targets.md`, "Base
valuation on every instance" and "Resource weights (v2)"). Until 0.12.29 it was
read only for an instance with an active v2 policy. A catalog without a
valuation ranks exactly as before, apart from the v1 `U` change of SS-1
(`contracts/resource-targets.md`, "One-way guard").

The bounds above are checked when the catalog is compiled (`compile_catalog`,
`actinglab scheduling compile`, Runtime activation and startup reload).
`validate_catalog_declaration` (`actinglab resource validate`) only parses a
document and checks its version, so an out-of-range value passes it and is
rejected at compilation. A Runtime build without this field rejects a catalog
that declares it (`unknown_field`) and cannot reopen a state root on which such
a catalog was activated or dispatched, even after its configuration returns to
an older catalog.

**A valuation written in the pack** (register §1.299, SCHED-C1). Today a `schedule.json` pool may
already write a declared `valuation` block: the composer copies a pool's V2 members as written,
and the compiler checks the bounds above and nothing more
([../standard-pack-schedule.md](../standard-pack-schedule.md), "The file"). So a constant
valuation the resource side writes by hand is accepted, and every reader reads it as it reads any
other: its weight is the `W` of the net benefit, its base weight weighs the pool's producers
(SS-1), and its pool's lines take part ("Pool lines"). Who wrote it, a person in the pack or the
model's export, is marked by the `schedule.json` pool member `valuation_origin` (SCHED-C1a, since
0.12.52), which the catalog never carries: a valuation decides the same whoever wrote it, and only the
reports tell the two apart ([../standard-pack-schedule.md](../standard-pack-schedule.md),
"Provenance").

### A goal's step and importance on a pool without a valuation

#455 B3-P (since 0.12.82; register §1.522; the coordinator's rulings of 11-23, not the owner's
words). In a V2 catalog a pool that writes **no `valuation`** (neither a block nor the pending
marker) may write `target_defaults`, the gap step and the importance a resource target on the pool
takes when it carries none of its own:

```json
"target_defaults": { "scale": 10, "importance_milli": 1000 }
```

| Field | Bound | Meaning | Diagnostic |
| --- | --- | --- | --- |
| `scale` (S) | required, 1..=9,007,199,254,740,991, a target's own `scale`'s bound | the gap at which the target weight equals the importance | `limit_exceeded` at it; missing: `missing_required_field` |
| `importance_milli` (I) | required, 1..=1,000,000, a target's own `importance_milli`'s bound | extra milli per produced step for each missing step `S` | as `scale` |

A resource target takes its own numbers, else the pool's valuation's (`scale`, `gap.weight_milli`),
else these ([../resource-targets.md](../resource-targets.md), "Who gives a goal its step and
importance"), read from the catalog in force at every check and evaluation and copied into no
stored goal. Refused, each at its pointer: beside a `valuation` block or the pending marker,
`type_mismatch` at `/pools/<i>/target_defaults` (one source per pool for a goal's step and
importance); in a V1 document, `unknown_field` there; another member, `unknown_field`. The member
makes nothing else of the pool valued: no base weight, no part of the net benefit of a judged task
that touches it (a judged task without a fixed value that touches it stays `benefit_unknown`,
`valuation_missing`), no part in its lines, and a fixed value beside its production is taken, as
beside any pool without a valuation ("Fixed values written in the pack"). A `schedule.json` whose
pool writes it names `pool_target_defaults` in `requires`
([../standard-pack-schedule.md](../standard-pack-schedule.md), "`requires`"). A pool without it
serializes, hashes and decides exactly as before.

### A pool whose spending a line governs

#455 SL1 (since 0.12.97; register §1.540; the coordinator's rulings of 11-28, after the owner's
words that such a currency has a line: above it the spending goes on as today, below it the rules
that spend are pressed, the harder the farther below, the pressing depends on time, and close to
the date nothing more may be spent). Some pools are what a player saves for a dated event: they
carry no valuation and cannot carry one (every rule that produces them keeps a fixed value, and a
fixed value beside a production of a valued pool is refused). In a V2 catalog a pool that writes
**neither a `valuation`** (a block or the pending marker) **nor `target_defaults`** may write
`spend_line`, which declares that a line a person may set governs its spending:

```json
"spend_line": { "price_milli": 33333, "scale": 6000, "income_floor_milli_per_day": 71429 }
```

| Field | Bound | What it will be read for | Diagnostic |
| --- | --- | --- | --- |
| `price_milli` | required, 1..=9,007,199,254,740,991, the bound of a task's fixed value | the milli of value of `scale` units of the pool, in the unit the instance's valuations weigh in (as a weight is the milli of value of `Q` steps): the full price of the pressing below a line | `limit_exceeded` at it; missing: `missing_required_field` |
| `scale` | required, 1..=9,007,199,254,740,991, the bound of a valuation's `scale` | the amount the price is of, and the shortfall below the line at which the pressing reaches the full price | as `price_milli` |
| `income_floor_milli_per_day` | required, 0..=9,007,199,254,740,991 | the thousandths of a unit the pool gains each day at the least, with nothing spent; 0 says nothing is sure; read for a line with a date | as `price_milli` |

In this build the three numbers are validated, serialized, hashed and shown in the compiled
catalog; **nothing computes with them**, so the resource side can write them now. Refused, each at
its pointer: beside a `valuation` block, the pending marker or `target_defaults`, `type_mismatch` at
`/pools/<i>/spend_line` (the diagnostic `target_defaults` gets beside a valuation: one reading per
pool of a goal on it; on such a pool a goal will be the line, never a weight on its producers); in a
V1 document, `unknown_field` there; another member, `unknown_field`.

What it changes is the net benefit of a judged task that **consumes** the pool ("Net benefit within
a tier", "A declared 0"): it is no longer unknown for lack of a valuation, and the consumption costs
nothing while no line is set. Nothing else reads the member. A task that **produces** the pool is
judged as with any pool without a valuation. Every other reader of a consumption reads the pool as
it reads any pool without a valuation: the reserve gate holds a consumer whose reading is short
("Task Reserves"), the forward projection applies the consumption, a run handed a `run_cap` leaves
the pools its task consumes unread until they are read after it ("Pool lines"), a reserve on the
pool gives its spend values; the pool's lines and the capped spending order act on valued pools
only, so the pool stays outside them (`pool_lines_unvalued` if it writes lines). The goal page and
the stored goals are untouched: the pool is not offered as one a goal can be set on by
`{at_least}`, a goal by its pool id is refused `pool_unvalued`, and the Lab's root check refuses a
`resource` entry on it (`goal_pool_unvalued`). The line itself, its date, the pressing, the hold
and the statement of a shortfall are later slices. A `schedule.json` whose pool writes the member
names `pool_spend_line` in `requires`
([../standard-pack-schedule.md](../standard-pack-schedule.md), "`requires`"). A pool without it
serializes, hashes and decides exactly as before.

### A goal's gap in two parts

#455 RR3b (since 0.12.98; register §1.542; the owner's ruling of 11-28 that what the characters in
training lack is pushed hard and what a standing stock lacks gently, the rulings the
coordinator's). In a V2 catalog a pool may write `goal_tiers`, which weighs the two parts of a
goal's gap on it with two strengths:

```json
"goal_tiers": { "need_importance_milli": 100000, "stock_cap_milli": 40 }
```

| Field | Bound | Meaning | Diagnostic |
| --- | --- | --- | --- |
| `need_importance_milli` (`I_n`) | required, 1..=1,000,000, a target's own `importance_milli`'s bound | the importance of the part of a gap that the recorded characters need | `limit_exceeded` at it; missing: `missing_required_field` |
| `stock_cap_milli` (`C`) | required, 0..=1,000,000, the bound of a gap weight | the most the part a stored goal asks beyond the characters' need may weigh; 0: it weighs nothing | as `need_importance_milli` |

For a target on such a pool with a known holding `c`, `N` the need the records stack on the pool
at the evaluation ("Needs computed from the records"; 0 without) and `T` what the stored target
itself asks (its own `at_least`; 0 for the goal the needs alone give the pool): `g_n = max(0,
N − c)`, `g_s = max(0, T − max(0, c − N))` (the holding goes to the characters first), and over the
target's one step `S` and its importance `I`, resolved as for any target
([../resource-targets.md](../resource-targets.md), "Who gives a goal its step and importance"),
`Γ = min(min(⌊I_n·g_n/S⌋, 1 000 000) + min(⌊I·g_s/S⌋, C), 1 000 000)`. `g_n + g_s` is the gap of
before, and satisfied, pending and unresolved are judged as before. Every reader of the weight reads
this one `Γ` over `S`: the production term, a fixed value's goal term, the capped spending order and
the score stage. A run a goal sends is told the need's part while it is open, else the stock's:
`gap_remaining` and the goal term of `run_cap` read `g_n` while `g_n > 0`, then `g_s`
([../remedies.md](../remedies.md), section 4). Below the trigger line of a regenerating pool a task
with a fixed value consumes, its goal terms on such pools count ("Fixed values written in the
pack", ruling 9 as A1 reads it). The member may stand beside a `valuation`, beside
`target_defaults` or alone (a target may carry its own step and importance). Refused, each at its
pointer: beside `spend_line`, `type_mismatch` at `/pools/<i>/goal_tiers` (on such a pool a goal is
a line, it weighs no producer); in a V1 document, `unknown_field` there; another member,
`unknown_field`. The numbers are read from the catalog in force at every evaluation, so a later
catalog's numbers weigh a stored goal at the next evaluation with nothing set again. A
`schedule.json` whose pool writes the member names `pool_goal_tiers` in `requires`
([../standard-pack-schedule.md](../standard-pack-schedule.md), "`requires`"). A pool without it
weighs the whole gap by one importance, `Γ = min(⌊I·g/S⌋, 1 000 000)`, and serializes, hashes and
decides exactly as before.

## Expected quantities (v2)

Each effect has exactly one quantity. `amount` is a positive integer in real inventory
units and keeps its existing semantics. A v2 effect may instead declare
`expected_amount_milli`, an integer in `0..=9007199254740991` measured in thousandths of
a real unit per completed task run: a `produces` entry, and (SS-2g, since 0.12.34, register
§1.232) a `consumes` entry. It has no default, cannot exceed `pool.capacity * 1000`
(a constant capacity; #386 CF-5), and is mutually exclusive with `amount`. Explicit null, missing quantities and unknown
fields are invalid. V1 effects require `amount` (`limit_exceeded` at the entry otherwise).

```json
{"pool_id":"material","direction":"produce","expected_amount_milli":400,"observation_source":"self_reported","confidence_milli":1000}
```

This declares an expected 0.4 units per completed run. Zero is a known zero yield;
an unknown yield must remain undeclared, not be replaced by zero. The producer computes
the expectation from separately established drop probabilities, quantities and batch counts.
The declaration is already that total: the Runtime does not multiply it by a probability,
batch count or `confidence_milli`. `observation_source` and `confidence_milli` remain required
evidence metadata; confidence is in `0..=1000` and is not a drop probability.

Task mapping, coverage and resource scoring retain these milli-units. Actual inventory
facts, pool capacities and inventory targets remain in real units. See
[`resource-targets.md`](../resource-targets.md) for final score rounding and checked bounds.
Forward projection returns `EvidenceInsufficient` with `effect_evidence_insufficient`
before applying a dispatched expected effect. The integer strategy planner returns an
unavailable production rate for a relevant expected producer, which follows its existing
needs-detection path; it rejects a reported definite rate inconsistent with that result.
Neither path records expected production as actual inventory.

SS-2b (since 0.12.32; register §1.225): in a V2 catalog an effect of either direction may write
`"amount": "pending"` as its one quantity, the marker that it is not known yet. It does nothing
but make a judged task's net benefit unknown ("Net benefit within a tier"): the resource terms,
the reserve gate, the tie-break and the spend values read it as no declared quantity of that
entry, forward projection as `effect_evidence_insufficient`, and the strategy planner as an
unavailable rate. With `expected_amount_milli` beside it the effect is `limit_exceeded`, as two
quantities are; any other string is `type_mismatch`, and so is the marker in a V1 document (at
the entry's `amount`). A catalog without the marker keeps its hash.

SS-2g (since 0.12.34; register §1.232; ACM-PRICE addendum 2.1 §3.5 and §7.8): a task the package
decides at run time (which project, which item) spends a different amount on each run, and the
model gives its expectation, so a V2 consumption may be declared the same way:

```json
{"pool_id":"material","direction":"consume","expected_amount_milli":2500,"observation_source":"self_reported","confidence_milli":1000}
```

A task's consumption of a pool per run is then `C = Σ amount × 1000 + Σ expected_amount_milli`
thousandths over its `consumes` entries for the pool (a pending `amount` adds nothing).
- **The net benefit** reads `C` exactly: its cost is `C·P/(1000·Q)` ("Net benefit within a
  tier"). A catalog without an expected consumption has the numerator `amount × 1000` of before.
- **Every reader of an integer consumption** reads `⌈C/1000⌉`, rounded up so that a reserve is
  never read as safer than the expectation says: the reserve views and the reserve gate ("Task
  Reserves"), the consumption a pool's reading does not show yet (the gate's step 3 and SS-6's
  allowance), SS-2's tie key ("Ties and chain heads") and the selection fields
  `task.consumes.<pool>`. The example above reads 3.
- **SS-6.** A task that reserves a pool it consumes by expectation gets no `run_cap`; its
  `spend_allowance` is unchanged and is what stops the spend, since one run may spend more than
  the expectation ([../remedies.md](../remedies.md), section 4, "Spend values"). Until #455 B1
  (0.12.82) a goal-driven production declared by expectation gave none either; since ruling 10 a
  goal never takes `run_cap` away: such a production bounds it by ⌈gap·1000 ÷ the expected yield
  per run in thousandths⌉, at least 1 ([../resource-targets.md](../resource-targets.md), "A goal
  on what a task with a fixed value produces"). #455 RR3b (since 0.12.98): on a pool that writes
  `goal_tiers` that gap is the need's part while it is open, else the stock's ("A goal's gap in
  two parts").
- **Forward projection** stops before applying it with `effect_evidence_insufficient`, as for an
  expected production.

No configuration key and no new code. A catalog without an expected consumption keeps its hash
and decides byte for byte as before.

The compiler enforces quantity exclusivity, version, direction and capacity together;
the declaration-only parser checks wire types. Existing integer declarations serialize
identically when the new field is absent. A binary that does not understand the field
rejects it, so a state lineage containing these declarations requires a compatible reader.

## A run's consumption read from a fact (SCHED-C1f1, since 0.12.54)

Register §1.303. The rules of this section are the coordinator's work order of 11-11 and the
coordinator's rulings of 11-11 14:5x on the nine questions of §1.303, not Alice's words: the member
names and the codes are this draft's, which the coordinator read, and the nine answers are the
coordinator's rulings. What one run of some tasks consumes of a pool is a state of the
account, not of the game: it depends on what the player fields and changes when they change it.
The pack reads the real amount on the device and writes it as an instance fact each time it runs,
and a line or threshold that belongs to the account is read from a fact, never written as a
constant (Alice 10-26, as "Fact references" and "Pool fact references" cite it). Until this form a
consumption could only be the constant `consumes[].amount`. A fact a task's own run writes cannot
gate that task's first run (the coordinator's rule of 10-28, "Self-read fact probe"), so the form
carries a fallback and the amount is never Unknown.

- **The form** (SCHED-C1f1, since 0.12.54). In an `actingcommand.scheduling.v2` catalog the `amount`
  of a `consumes` entry may be, instead of a positive integer, the object
  `{"type": "fact", "fact_key": B, "fallback": F}`:

  ```json
  {"pool_id": "pool.neutral", "direction": "consume", "amount": {"type": "fact", "fact_key": "resource.neutral.per_run", "fallback": 3}, "observation_source": "self_reported", "confidence_milli": 1000}
  ```

  All three members are required. One run's amount is the integer B holds when the instance holds
  a usable one ("Which reading is usable" below), else F, an integer in `1..=9007199254740991` that
  the pack writes. Why this shape: `type` and `fact_key` are the existing fact reference's ("Fact
  references"; schema `factReference`), so the format keeps one way of saying "read this fact" and
  B follows `fact_key`'s identifier rule; it names no `scope`, as no reference of the format does
  (each reads its own context's scope, here the candidate's instance); it has no `offset`, since
  the fact holds the amount itself, while a reference's offset is a margin on a line (a cap less a
  margin), and an amount plus a margin would be a number the catalog computes, which is the pack's
  to write; and `fallback` is what a reference of CF-4 or CF-5 does not need, because a fact they
  miss is asked of the fact refresh (a capacity) or makes a predicate false (a comparand), while
  this fact is written by the task's own run, which a missing amount must not stop. A `produces`
  entry takes no such form, and neither does `expected_amount_milli`.
- **Refused**, each at its pointer: in a V1 document, `unknown_field` at `amount/type` (as a
  reference of CF-4 and CF-5); a `type` other than `fact`, a `fact_key` that is no identifier and a
  `fallback` that is no integer (an explicit `null` included), `type_mismatch` at the member; a
  `fallback` outside `1..=9007199254740991`, `limit_exceeded` at `amount/fallback`; a `fallback`
  above a constant capacity, `effect_incompatible` at the entry, as a constant `amount` above it (a
  capacity read from a fact is not checked at compilation, "Pool fact references"); a missing
  member, `missing_required_field`; `offset`, `scope` or any other member, `unknown_field` at it;
  the form beside `expected_amount_milli`, `limit_exceeded` at the entry, as two quantities are;
  the form on a `produces` entry, `effect_incompatible` at its `amount`; and a second entry of one
  task on one pool that writes the form, `effect_incompatible` at its `amount`: one fact names a
  run's amount of a pool (constant entries beside it add to it as before).
- **The amount in effect.** At each evaluation, for each task whose `consumes` writes the form and
  each instance its scope covers, the evaluation resolves each such entry once, from the instance
  facts it already holds (the fact snapshot it pins): B in exactly that instance's scope, never a
  server or game scope (an amount is the account's). That value, the fact's or F, is the entry's
  amount for that instance in that evaluation, and the task's consumption of the pool is `C = Σ
  amount × 1000 + Σ expected_amount_milli` ("Expected quantities (v2)") with it in place of a
  constant. **One amount per evaluation of one instance:** every reader below reads that value, and
  none reads B again. The amount is never Unknown: B raises no fact refresh demand ("Fact
  Refresh"), gives no detection suggestion, and never makes a net benefit unknown ("Net benefit
  within a tier").
- **Which reading is usable** (the coordinator's ruling 1 of 11-11 14:5x, register §1.303 question
  1: the record is read by every fact's rule, not as this draft proposed). The record of B in the
  instance's scope that is current as every fact is: present at a confidence above 0, not past its
  own `expires_at_unix_ms`, and not invalidated by a timeline reset since it was observed ("Runtime
  Enforcement", with "Reset exclusions and the task's own last result"); its value an `integer` in
  `1..=9007199254740991` and, against a constant capacity of the pool, at most that capacity. The
  form has no `max_age_ms`: the pack states how long its reading holds by the validity it writes,
  and keeps the key out of its resets (`excludes_fact_prefixes`) when the amount outlives a day.
  - *Freshness and wakes, by the general rule.* A usable record bounds the
    `facts_fresh_until_unix_ms` of every candidate whose evaluation read the amount by its
    `expires_at_unix_ms` (an intent admitted past it is refused `policy_facts_stale`), and the
    evaluation wakes one millisecond after that expiry, as after a pool's reading or a capacity read
    from its fact ("Pool fact references"); a timeline reset that will invalidate the key caps a V2
    candidate's freshness before its occurrence and wakes the evaluation at it, as for every
    observation ("Runtime Enforcement"; [v2/README.md](v2/README.md)). The evaluation at that wake
    finds the record expired or invalidated and reads F. F bounds no freshness and wakes nothing. A
    new record wakes the scheduler as every fact publication does, and the next evaluation reads
    it.
  - *What was weighed.* The draft proposed the latest record whatever its age, its expiry or a
    reset since. For it: the pack rewrites the value at every run, the setup it measures does not
    end with a day or a reset, a wrong amount misjudges only the runs until the next one rewrites
    it, and a reader that reads through a reset exists already, a task's own last result. Against
    it, and ruled: one rule for facts, with no reader before the time-validity projection and no
    second meaning of a record's expiry; with a fallback an expired reading costs nothing but one
    run judged by F; and a value from long ago (the account changed by hand, the task not run
    since) is not read as current.
  - *No usable record because there is none, it expired, or a reset invalidated it*
    (`fact_missing`, `fact_expired`): F, with no warning, exactly alike: it is the state the
    fallback exists for (the first run, or a reading the pack let lapse), and the source below says
    `fallback`.
  - *A record that is present and current but not usable*: F, and the decision says so with the
    new warning `consumption_fact_unusable` (warning; `task_id`, `pool_id`, `fact_key` B,
    `unknown_reason` `fact_low_confidence` for a confidence of 0 or `type_mismatch` for a value of
    another kind, a record list or an integer out of range, as CF-5 reads a capacity below 1
    (ruling 5); `consumption` the `⌈C/1000⌉` the decision reads with F). It is the decision's first reason, before
    `benefit_fixed_value`, so it never settles the decision; it is given once per decision and pool,
    on every decision of that task on that instance in the evaluation. `fact_reference_unavailable`
    is not reused: it is info and says that a predicate is false.
- **Who reads it.** Each of these reads the amount in effect, and nothing else changes in it:
  1. *The net benefit's cost term* ("Net benefit within a tier"): `consume(k,p)` is `C/1000`
     exactly; "Fixed values written in the pack" reads the same `C` (`V − Σ_p C_p·W_p/(1000·Q_p)`);
     the bound `ε` of "Within tolerance" reads the same `C`, and an amount read from a fact is an
     integer, so it adds to no `m`.
  2. *The pool's lines* ("Pool lines"): the reading `r`, which takes off the consumption its
     reading does not show yet, each task dispatched since (the candidate's own included) by its
     own amount in effect; and
     the hold at the hard line below the trigger line, `r − ⌈C/1000⌉ < H` (`would_cross_hard_line`,
     `amount_after`).
  3. *A pool its lines cannot spend* ("Pool lines"): the decision reason `pool_lines_unspendable`
     reads each consumer's `⌈C/1000⌉` in effect on the instance (`least_consumption`). The compile
     warning of the same code has no instance: it reads each consumer's declared consumption, F in
     place of the fact's value (the amount an evaluation reads while B gives no usable reading;
     ruling 8), so a fallback no consumer's run fits under is named in the file. A fact below F can make that
     warning false for one instance; the decisions there read the fact and do not hold.
  4. *The capped spending order* ("Capped spending order"): a round's listing (`V = S +
     C·W/(1000·Q)` against `ε` without the pool's term), its order by `V ÷ C` frozen at its start,
     the plan (a member skipped at `R − ⌈C/1000⌉ < H`, `R` falling by each planned or dispatched
     member's consumption), the head's check before its dispatch, the head's `B = V − W·(c − a)`,
     the block's fit above `H` and its fallbacks. What an episode under way and a frozen order do
     with a changed amount is the next bullet.
  5. *Who is a consumer* (the capped order's, the consumers key's and the warning's): a task whose
     entry writes the form declares a consumption of the pool whatever the value read (F is at
     least 1).
  6. *The reserves* ("Task Reserves"): `after = projected − consumes`, the gate's steps 2 to 4 and
     `amount_after`, and the selection fields `task.consumes.<pool>`, `task.after.<pool>` and
     `task.reserve_gap.<pool>` ("Score-Assisted Priority").
  7. *The spend values* (SS-6, [../remedies.md](../remedies.md), section 4): `spend_allowance` (the
     gate's available amount less the consumption its step 3 counts), the gate's hold of an
     allowance below the task's consumption, and `run_cap`, ⌊allowance ÷ one run's amount⌋, which
     the run reads as `session.dispatch.schedule.run_cap`. #477 LF-1 (since 0.12.80): one run's
     amount of a pool whose lines take part in the spend values ("Task Reserves", "Spend
     values") is the one in effect as well, and such a pool's allowance is the lines' reading `r`
     (item 2) less its floor.
  8. *The tie-break* ("Ties and chain heads"): a candidate's runs left, by `c`.
  9. *Forward projection* ("Forward Planning And Maintenance", "Pool Projection"): it reads the
     amount once, from the facts it starts from, and holds it across its horizon, as it holds a
     capacity read from a fact ("Pool fact references"); a simulated run does not rewrite B, and an
     amount read so is applied as a constant one is, with no evidence gap of its own.
  10. *The compiler*: F is checked as a constant `amount` is (above), and SS-2d's requirements
     ("Within tolerance", "No loop") count an entry with the form as a `consumes` entry.

  Read by none of them, and unchanged: a regenerating pool's fullness urgency (the pool's fill),
  the resource layer's `R` and every resource target (production only), the strategy report's rate
  (production only), the no-progress guard (a pool the task only consumes is no basis, so a new
  amount is no progress) and the fact refresh (above).
- **An episode under way and a round's frozen order** (SCHED-C1d's "Episodes survive"; register
  §1.303, question 2). An amount read from a fact changes between two evaluations without a new
  catalog generation (a new reading, or a reading that lapses to F). The rule (the coordinator's
  ruling 2 of 11-11 14:5x, as this draft proposed):
  - *The consumers key holds what the catalog writes, not what the instance reads.* Each
    consumer's `⌈C/1000⌉` in the key is its declared consumption, every fact-driven amount taken at
    its F. A changed reading keeps the episode; a changed F ends it, as a changed constant does (the
    consumers changed); a changed `fact_key` alone, or a constant replaced by the form with that
    constant as its F, keeps it, and the next evaluation reads the new amount. The rows of
    `host.capped_episodes` keep their shape: a consumer row's `consumed` is that declared
    consumption.
  - *A round keeps its order, and reads the amounts of now.* The order frozen at the round's start
    (which consumers, in which place) stays until the round ends; every amount the round's plan and
    checks read at an evaluation is the one in effect at that evaluation; the next round lists and
    orders by the amounts then, as by the values then. A member row's `consumed` keeps the amount
    the order was frozen with, which no rule reads once this is built.
  - For: the pack rewrites B at each run, so a key that held the value read would end the episode
    at every run that wrote another value, and the next episode's first round would list the
    `benefit_limited` consumers again; a pack that replaces a constant by the form with the same F
    ends no episode; B is meant to be written by the consumer's own run (the amount its pack read
    at that run), so once a member ran in the round the amount in effect is the one its run read,
    which is what the plan should count it by; and the
    head's check and the block's fit read the amount the next run is judged by, so no run planned
    with a smaller amount ends below `H`.
  - Against: the plan is no longer fixed at the round's start (only its starting reading and its
    order are): a changed amount can drop a planned member that no longer fits above `H`, or plan
    one that was skipped; the key no longer tells what a consumer costs on the instance, so an
    episode goes on while every amount changed (when then no consumer fits, `pool_lines_unspendable`
    makes it wait, and a round that plans nothing ends it); and until the next round the frozen
    order need not be the `V ÷ C` order of the amounts in effect, as it need not be when a value
    changes.
  - The coordinator's example, a round that keeps the amounts it froze while the next round reads
    again, was weighed and not ruled: it counts a member that ran in the round by the amount from
    before its run, and a frozen amount at the head's check could let a run end below `H`.
- **What a person sees.**
  - *Where the amount came from*, in the manner of `lines_source`: the values `consumption`
    (`integer`, the `⌈C/1000⌉` of the reason's task for its pool in that evaluation) and
    `consumption_source` (`fact`, the value read, or `fallback`) on `pool_line_held`, `capped_spend`,
    `resource_reserve_insufficient` and `pool_lines_unspendable` (for this one, of the consumer that
    gives `least_consumption`, the first by task id among equal ones), whenever that task's
    consumption of that pool writes the form. A reason whose task's consumption is constant carries
    neither, as before. The net benefit's reasons (`benefit_fixed_value`,
    `policy_benefit_nonpositive`, `benefit_within_tolerance`, `rank_breakdown`) name no pool and
    carry no source (ruling 6). So a dispatched candidate with no pool reason shows its amount's
    source only through the tools' list and the fact itself (`ac_facts`), which the coordinator
    accepts for the first build and may revisit after the trial.
  - *The status read.* `ScheduleExplanation` shows them (`actingctl schedule`, `ac_schedule`,
    `ac_overview`'s `not_running`, `ac_diagnose`'s `not_run`): a row's `why` is the full outcome
    of its last reason, values included, and its `reasons` list the codes, the warning among them.
    A dispatched candidate's reasons travel in its dispatch events' reason chain as every reason
    does. No answer shape changes.
  - *The tools.* `actinglab standard-pack check` and `package bundle --schedule` list every rule
    whose amount is fact-driven, with its fact key and fallback
    ([../standard-pack-schedule.md](../standard-pack-schedule.md), "A run's consumption read from a
    fact").
- **Capability.** A Runtime that reads the form reports the always-on subsystem
  `consumption_fact` in `config.subsystems`
  ([`actingd-check-config.md`](../actingd-check-config.md), its row with the code). A
  `schedule.json` whose rule writes the form lists it in `requires`; the composer refuses the file
  without it, so a Runtime that does not name the subsystem refuses the file
  (`schedule_requires_missing`) instead of misreading it. An older Runtime given a catalog with the
  form refuses it at its reader (`type_mismatch` at the entry's `amount`).
- **Identity.** A catalog that does not write the form serializes, hashes and decides byte for
  byte as before, and a composition without it keeps its rules digest. With it, the object is
  hashed as written (a changed `fact_key` or `fallback` is a new catalog identity); the value read
  is part of no catalog hash, rules digest or decision identity input: it comes from the fact
  snapshot the evaluation already pins.

## Compatibility

V1 rejects unknown fields and any schema version other than the exact supported value. A future schema revision is a new immutable contract and requires an explicit, separately tested translator before V1 input can be promoted. Readers must never reinterpret a V1 field under newer semantics, partially load a catalog, or invent defaults for missing data.

The `env.*` namespace remains an ordinary fact-key family. Existing execution substitutions that use `{env:...}` remain an execution-boundary concern; scheduling predicates reference the same stored values through typed `fact` predicates without changing that substitution syntax.

## Catalog Source

ROUT-L (frozen model v1.0 §3.1–§3.2; slice RL-1a; [standard-pack-schedule.md](../standard-pack-schedule.md)).
A catalog comes from one of two sources:

- **The configuration** (today's path): `policy.catalog` names the four documents, and
  `policy.catalog_approval_ids` and `policy.procedure_manifest` bind them. Everything below
  ("Catalog Lineage", "Scope And Overrides") describes that path. RL-C removes it.
- **The standard packs**: each game root may hold `schedule.json`, the game's rules without
  scope, instance or approval. The policy crate's `compose_standard_pack_rules` composes the
  games of the bound instances into one V2 catalog `{catalog_id: "standard-packs",
  catalog_version, approval_refs: []}`: one copy of each rule and item per bound instance, the
  file's ids with one instance per (game, server) and `<id>@<alias>` ids, with references
  rewritten and `continues_from` the file's id, with two or more. The composed documents are
  bounded by the 4 MiB catalog bound, not 1 MiB each (`compile_composed_catalog`); every other
  compiler rule applies. A composition without rules is a valid empty catalog. The scheduler
  workbench that loads, composes and activates it is RL-1b's
  ([standard-pack-schedule.md](../standard-pack-schedule.md), "The scheduler workbench
  (RL-1b)"): it runs when the `policy` section names no `catalog`, and a configuration that
  names one keeps the configured path for this release. Under a composed generation a
  `policy.resources.pools` reading for a pool no composed game declares is left out of the
  evaluation (`policy_pool_reading_unused`), and `continues_from` is read per instance.

Both sources compile into the same `CompiledCatalog` and are evaluated alike: the evaluator
orders candidates by score, tie-breaker, task id and instance, never by document order.

## Catalog Lineage

Workflow #361 A. A state root is not bound to one catalog id. The generations that were active
in a ledger are the targets of its successful `catalog.activated` and `catalog.rolled_back`
records and of the legacy active-pointer migration; they are rebuilt from the ledger on every
start and never deleted. `actingd` decides the transition its configured catalog asks for
before it records anything, in this order:

| Active | Configured catalog | `policy.catalog_transition` | Plan | Recorded |
|---|---|---|---|---|
| none | any | absent | `first` | `catalog.activated` |
| none | any | present | refused `catalog_transition_expectation_mismatch` | nothing |
| A | the hash of A | any | `unchanged` | nothing |
| A | the id of A at a higher version | absent, or `replace` expecting A | `forward` | `catalog.activated` |
| A | anything else | absent | refused `catalog_activation_not_newer` | nothing |
| A | any | `replace` expecting another hash | refused `catalog_transition_expectation_mismatch` | nothing |
| A | a generation that was active | `replace` expecting A | `rollback` | `catalog.rolled_back` |
| A | another id, never active, above every version of that id that was active | `replace` expecting A | `switch` | `catalog.activated` |
| A | otherwise | `replace` expecting A | refused `catalog_replace_version_not_newer` | nothing |

`replace` is the only transition kind; `expected_active_catalog_hash` is a compare-and-swap
value, and the transition is recorded under the same compare-and-swap
(`catalog_active_generation_changed` when another transition won). The field may stay in the
configuration after it was applied: the next start finds the same hash and plans `unchanged`.
A `catalog_activation_not_newer` refusal names the active and the configured generation and the
`replace` transition, expecting the active hash, that would switch or roll back instead; `actingd`
prints it on its `FATAL` line. Proposal promotion stays forward-only. Replay accepts exactly these records: a successful
`catalog.activated` keeps the id at a higher version or changes the id, a successful
`catalog.rolled_back` keeps the id at a lower version or returns to a generation of another id
that was active before; every other sequence stays `catalog_generation_source_conflict`. Every
ledger that satisfied the earlier rule satisfies this one and projects to the same generation.
A Runtime older than this rule refuses a ledger with a cross-id transition at start with
`catalog_generation_source_conflict`; switching an installation back to such a Runtime after a
`replace` therefore fails loudly at its first start.

There is no deactivation record. Stopping the scheduler is a configuration without `policy`
(recorded as the disabled `policy_driver` subsystem) or the global scheduling pause; the last
active generation stays the active generation of the lineage, and a later configuration with
any catalog continues from it with `replace`.

Task ids are global: dispatch history, last dispatch time, budgets and failure streaks are kept
per (task id, instance) across a switch, so a catalog that keeps its task ids keeps its clock
state.

Workflow #372 K5: a V2 task may declare `continues_from`, the task ids of an earlier catalog it
continues (`v2/README.md`). Under the active catalog a task's accounting group is its own id and
those ids, and every per-(task, instance) state the Runtime derives from its ledger is read over
the group on that instance: the task budgets are summed (the budget receipt's
`task_daily_used`, `task_window_used` and `task_runtime_reserved_ms` count the group, each
entry keeping only its own id's runs); the last dispatch, terminal state and completed window
come from the group's latest admitted dispatch, and the last duration and failure streak from
its settlements folded in ledger order; an outcome of a continued id's run is read as the
task's; the failure streak, a pause, a backoff and a retry round are those of the group's id
whose failure state a settlement set or cleared last, and a new settlement of the task counts
on from it under the task's own id; a clock-slot record of a continued id counts for the task.
The group is read at read time only from the active catalog: nothing in the ledger or the
control state is re-keyed, replay stays per dispatch against its own generation, and the
declaration is not followed across generations (a task that continues a task which itself
continued another lists both ids). The Runtime never infers a rename: an undeclared rename is a
new task, and a task that keeps its id keeps its state as above.

## Scope And Overrides

Runtime evaluation replaces task runtime snapshots with its replayed admitted and
completed dispatches at the input ledger position. The latest admitted dispatch
supplies `last_dispatched_unix_ms`; a completed dispatch supplies its recorded
execution success/failure. This is scheduling execution state, not a business
effect or a catalog outcome key. No observed eligibility-start time is invented.

Before assigning an instance winner, Runtime consults the existing control owner's
read-only admission predicate for task/activity daily, window and runtime budgets,
activity windows and sampled intervals. Unavailable candidates leave the instance
and host capacity available to the next ranked task. The same predicate supplies
the next possible admission time at bounded weekly window/cadence boundaries.
Final admission still checks and commits the actual counters under the original lock.
An operator scheduling pause (`contracts/scheduling-pause.md`) presents the same way, as do the
other control gates of the one order (`contracts/player-hold.md`, "The one order of the control
gates", R-7a §2.6; Workflow #429 R-7a2, since 0.12.35): a candidate on a held instance is
deferred with the first gate's code and no next wake time, `dispatch_player_held` (R-7b),
`dispatch_paused_instance`, `dispatch_paused_global` or `dispatch_resource_fenced`, until the
hold ends, `ResumeScheduling` lifts the pause or a passing self-check lifts the fence.

`policy.dispatch_rejected` retains its immutable ranking reasons and optionally
carries `rejection`: the original Runtime error code, operation and fatal flag,
the decisive budget dimension with used/requested/limit values when applicable,
and the next eligible Unix time when one is available. These are bounded typed
control facts; native text and secrets are not copied into ranking or rejection
details. Existing events without rejection details remain unchanged.

Every task, pool, activity profile, and timeline event declares `instance`, `server`, or `game` scope. `instance_overrides` is the only V1 task override layer. A null override field means inherit the catalog value; it does not mean zero, false, or an inferred default. Activity profiles scoped to an instance carry its importance and goals.

The `instance_id` in instance scopes and overrides stores the exact registered
instance alias: 1–256 UTF-8 bytes, with Unicode control characters prohibited.
Case, permitted whitespace (including all-space aliases), and Unicode bytes are
preserved through compilation, facts, evaluation, admission and registry lookup.
The Runtime's typed `InstanceId` continues to identify links and leases. Catalog,
task, pool, server and game identifiers and references retain their own rules.
JSON Schema bounds alias character length; the shared contract/compiler also
enforces the UTF-8 byte limit.

In the Runtime the policy sees an instance's availability only as the
`session.instance.available` instance fact (`contracts/instance-fact-store.md`):
the configuration seeds it, and for an instance on a physical device it holds the
seed only while the instance's latest backend self-check passed: a physical instance
starts unavailable at every Runtime start and after every emulator stop/start/restart
until its connection preparation phase passes (Workflow #317 sc3, replacing sc2's bounded reading).
The policy never reads `backend.selfcheck.*` itself; an instance projected unavailable
receives no dispatch.

Activity sampling uses a ledger-derived seed and `same_round_stable`: the host records the seed once and must reuse the sampled value throughout the same scheduling round. Resampling within a round is invalid.

## Score-Assisted Priority

The predicate gate (trigger, feedback stop, timeline validity, cooldown, placement)
decides eligibility on its own; a score never overrides it. Between that gate and
ranking, the evaluator runs an optional score stage:

- With a `selection` document, every eligible (task, instance) pair becomes one
  selection candidate `<task_id>@<instance_id>` whose fields are `task.priority`,
  `task.strategic_weight_milli`, `task.urgency_milli`, `task.aging_ms`,
  `task.load_cost_milli` (integers), `instance.affinity` (boolean), for a task
  with `reserves` the integers `task.consumes.<pool>`, `task.after.<pool>` and
  `task.reserve_gap.<pool>` of each reserved pool ("Task Reserves" below), and every
  scalar fact the instance can see (its own, its server's and its game's scope,
  the most specific scope per key; timestamps and durations become integer
  milliseconds; record lists are unusable). A fact field carries only a record
  that has not expired at the evaluation instant, by the fact contract's own
  predicate (`now > expires_at` is expired); the `fact` source still reads an
  expired record as `fact_expired`. The document is evaluated once per instance
  over that instance's candidates and its own fact projection, pinned to the
  evaluation's `fact_snapshot_id` and instant, with the selection-policy crate's
  collect option (`evaluate_collect`): the `on_unknown` clauses the document
  declares do not apply at this layer. Only a candidate's score is consumed; a
  candidate a gate rejects carries no score. A document the crate cannot
  evaluate fails with `selection_policy_invalid`.
- A candidate whose gates or terms meet an unknown input is deferred on its own
  (`Deferred`, with no wake: it is evaluated again when the instance facts, the
  catalog or the configuration change) and takes no part in ranking. The other
  candidates of the instance and every other instance are scored as usual, and
  the evaluation succeeds. Its reasons follow the first rule that matches
  (Workflow #384, D2):
  - an input that is not a realistic key gives one `selection_input_unknown`
    (`instance_id=<id> rule_id=<gate or term id> unknown_reason=<reason>`) per
    gate or term that met one. A realistic key is an instance fact key of the
    `resource.` or `inventory.` family, read without a lookup miss; instruction
    keys (`session.`), pack data and the fields `task.*` and
    `instance.affinity` are not, and a reading cannot supply them;
  - otherwise every unknown key gives one `fact_refresh_unbound`
    (`instance_id=<id> fact_key=<key> unknown_reason=<reason>`). A fact field
    whose record has expired reports `fact_expired`; one with no record reports
    `field_missing`. In the Runtime, which hands the evaluator its readers, these
    keys go to the fact refresh instead ("Fact Refresh" below), which gives each
    key its reason.

  `<reason>` is one of `fact_missing`, `fact_expired`, `fact_stale`,
  `fact_low_confidence`, `fact_not_scalar`, `type_mismatch`, `field_missing` or
  `lookup_miss`.
- Every document fact a scored or gate-rejected candidate's evaluation read bounds
  its `facts_fresh_until_unix_ms`: a `fact` source by the earlier of its record's
  `expires_at` and its publication plus the document's `max_age_ms`, a fact field
  by its record's `expires_at`. The evaluation wakes at that bound + 1, as for an
  applied resource target, and admission refuses an intent past it as
  `policy_facts_stale`.
- `EvaluationFacts.priority_offsets` carries manual offsets
  `{task_id, instance_id?, offset_milli, origin: user|agent, observed_at_unix_ms}`;
  an instance-level entry overrides the task-level entry, `|offset_milli|` is
  bounded to 1,000,000, and duplicate `(task_id, instance_id)` pairs are input
  errors. Offsets apply with or without a selection document.
- `base_milli = score_milli (0 when absent) + utility_milli + offset_milli`
  (checked; the slice 5a utility term is 0 without a declared value), and
  `effective_milli = base_milli`, plus an applied resource target's task target
  score `s` (below): `base_milli + s` in `adjust` mode, `utility_milli + s` in
  `override` mode. It is added to the candidate's `total_score` as
  `effective_milli * 1000`, the same unit as the urgency and strategic terms. The
  decision's `rank.effective_milli` carries it (0 for a candidate the stage did
  not score). An overflow fails the evaluation with
  `policy_evaluation_numeric_overflow`.
- Resource targets (Workflow #308 RT-S1b, `contracts/resource-targets.md`): the
  evaluator reads each instance's stored `session.resource_targets` policy from
  the facts before the time-validity projection and resolves every target once
  per instance against the active catalog and the projected inventory. A named
  candidate whose target has a gap `g > 0` is *applied*: it enters the stage even
  without a selection document, value or offset, and `s = min(⌊g·I·u/(S·U)⌋,
  1_000_000)`. `adjust` adds `s`; `override` supersedes the selection score and
  the manual offset with `s`, keeps the utility term and every other ranking
  term, and is never deferred or promoted (its disposition is `none`). Every
  other candidate, including a named one that is satisfied (`g = 0`, which
  releases an override), pending, unmapped, expired or unreadable, keeps its rank
  and disposition byte for byte and only gains reasons. Every admission
  predicate stays in force.
- The cycle thresholds (absolute pair or percentiles) are computed from the
  staged candidates' `base_milli`, never from a target score, so a policy on one
  instance moves no threshold of another. A scored candidate is compared with its
  `effective_milli`.
- The tasks document may declare a catalog-level `priority_selection` block
  `{defer_below_milli, defer_for_ms (1..=86,400,000), promote_above_milli}` with
  `defer_below_milli < promote_above_milli`; it is only allowed next to a
  selection document (`priority_selection_without_document` otherwise). A scored
  candidate with `effective_milli < defer_below_milli` is deferred with reason
  `score_deferred` and wakes at `now + defer_for_ms`, taking no slot of the shared run limits;
  one with `effective_milli > promote_above_milli` is promoted and ranks ahead of
  every other candidate (`score_promoted`). An unscored candidate is never
  deferred or promoted: its effective value is `utility_milli + offset_milli`,
  plus an applied resource target's `s` in `adjust` mode; in `override` mode `s`
  supersedes the offset (the utility term is kept). The reason `score_unknown`
  (value `rule_id`, the gate's id) records why it is unscored, and
  `score_cost_unknown` that its utility term is dropped.
- Every candidate that passed through the stage carries the reason
  `score_computed` with detail `score=<milli|none> offset=<milli> effective=<milli>
  disposition=<deferred|promoted|none>`, followed by ` target=<id>:<mode>:<s>`
  for an applied target. Each reason is a registered code with its values (an
  outcome, Workflow #378 A2-P); its detail is developer text and is never
  persisted. The decision identity is unchanged.
- Resource target reasons follow `score_computed` and `score_unknown` and precede the
  disposition reason, at most three per candidate: one policy-level reason
  (the first that applies of `resource_target_policy_ignored`,
  `resource_target_policy_unreadable`, `resource_target_policy_expired`,
  `resource_target_tasks_unevaluable`), one target-level reason
  (`resource_target_applied`, `resource_target_satisfied`,
  `resource_target_pending` or `resource_target_unmapped`) and, for an applied
  override, `resource_target_override`, each with the target's id as the value
  `resource_target_id`. A candidate that bypasses the stage carries them too.
- An applied target bounds the candidate's `facts_fresh_until_unix_ms` by the
  inventory observation's expiry and the policy's `valid_until_unix_ms`, so
  admission refuses the intent as `policy_facts_stale` once either lapses, and
  wakes the evaluation at `expiry + 1` and `valid_until + 1` when later than now.
  No other target state changes freshness or wakes, and none requests detection.

Without a selection document, without a declared value, without offsets and
without a stored resource target record (a withdrawal counts as none) the stage
is a no-op and every evaluation output is byte-identical to a catalog compiled
before this stage existed, except for the dispatch decision record at the end of
each selected chain ("Dispatch Decision Record" below).

In the Runtime (Workflow #308 slice 4a-2) manual offsets are instance facts
`session.task.<task_id>.priority_offset` (integer milli; instance scope for one
instance, server or game scope for a task-level offset) in the instance fact
store, published through `PublishFacts` by an agent (`agent`/`adapter`) or by a
person: the console (`user`/`ui`) or `actingctl task-offset` (`cli`/`cli`).
Every evaluation projects the active records at its input ledger position into
`priority_offsets` (origin `user` for a person, `agent` otherwise), drops those
whose TTL has elapsed at the evaluation instant, and reports an offset naming a
task the catalog does not declare once per cycle as
`priority_offset_unknown_task` (warning; value `task_id`) without failing; the offsets are part of
the `fact_snapshot_id`. `contracts/instance-fact-store.md` ("Priority offset
facts") owns the key, its validation, the origin exception and the projection.
The `score_computed` reason travels in the reason chain the `policy.dispatch_intent`
payload already carries, and so does the dispatch decision record, in the same
`reasons`; each persisted reason is its outcome (`code`, `category`, `values`), and
no detail is persisted.

The Runtime catalog store accepts the selection document with the other four:
activation stages it as `selection.json` in the immutable generation, records it
as a fifth source in the generation manifest only when present, and reloads it
from there; the stored `catalog_hash` is verified against a recompilation as for
the four documents. An agent proposal compiled against a catalog with a
selection document carries that document unchanged into the proposed catalog;
proposal patches address only the four catalog documents.

## Fact Refresh

Workflow #384 D2-S4 (`rd-1009/d2/model-d2-fact-refresh.md` v1.4.2 §3.3, §3.4,
§3.7, §3.9). A realistic instance fact (a `resource.` or `inventory.` key,
which recognition writes) that a candidate reads and finds unknown never fails
the round and is never scored with an old value: the instance's recognition
maintenance package R_I (`instances[].recognition_package`) is dispatched to
read it, and the candidate waits.

- **Demand.** A candidate raises demand for the unknown realistic keys its
  selection document read ("Score-Assisted Priority"), and a task for those its
  undecided trigger or feedback stop read in exactly its own instance's scope
  (`trigger_unknown` / `feedback_stop_unknown` stay; an instruction key such as
  `session.*`, a server or game scope, a dependency or an outcome raises none).
  A `resource_projection` predicate on a `ledger_fact` pool of exactly the
  instance's scope reads the pool's key: without a usable record it raises
  demand for that key (Workflow #390 POOL-1, "Pool Projection"); a static
  snapshot pool raises none. #386 CF-5: a pool of exactly the instance's scope
  whose capacity fact gives no capacity raises demand for that fact's key
  ("Pool fact references"). SS-5 (since 0.12.32): a `fact` predicate with
  `on_unknown: probe` raises none for its key, which the task's own package
  reads ("Self-read fact probe"); every other unknown key of the same trigger
  raises demand as before.
  Workflow #411 6438574534 follow-up 2 (since 0.12.26): a task's own placement
  never holds back the demand its undecided trigger or feedback stop raises.
  Whether the reading can be placed is R_I's own gates' to decide (below): an
  unavailable instance holds R_I back, and the task takes the row that gate
  gives it (`fact_refresh_pending` with `gate_code` `instance_unavailable`).
  The task's own load (a heavy-scene slot it would take, a load its host
  cannot take) binds only its own dispatch, never R_I's light one. A task its
  instance cannot run whatever is read (the instance does not provide its
  operation, or its instance override disables it) raises none.
  Only candidates past the closure's positions 1-2 raise demand: on an instance
  a control gate (an operator pause or, since 0.12.35, Workflow #429 R-7a2, a
  #403 fence), a holder, a waiting claim or a takeover cooldown holds as
  the cycle starts, a candidate the selection document deferred carries that
  hold's reason alone (`dispatch_paused_*`, `dispatch_resource_fenced`,
  `dispatch_instance_held`, `dispatch_instance_cooldown`) and no fact refresh
  reason, and nothing is dispatched there. Workflow #411 ruling 77 A (Alice 10-28; since 0.12.25): a
  task whose undecided trigger or feedback stop read such a key there carries
  the same hold's reason after its `trigger_unknown` or
  `feedback_stop_unknown`, as its last reason and with the hold's wake, and no
  fact refresh reason; its decision's detection suggestions name the keys it
  waits for. Nothing is read while the hold lasts; the first round after it
  ends raises the demand as above. A reader the Runtime's admission holds is
  R_I's own limit below: the task names the key and the package under
  `fact_refresh_exhausted`. Workflow #490 FR-1 (since 0.12.81): the admission
  never holds R_I's pair by a pause ([policy-suspension.md](../policy-suspension.md),
  "Lifting (R19)"); its rest after failures is its own gate below.
- **No reader.** A key no reader can serve gives `fact_refresh_unbound` with
  `reader_problem`: `disabled` (`scheduling.fact_refresh.enabled` is off),
  `package_unbound` (the instance has no kept recognition maintenance package),
  `package_unreadable` (the Runtime start could not read its package's marks)
  or `key_not_marked` (its package has no mark for the key).
- **The reader's candidate.** R_I is requested for the keys its marks write:
  its decision carries one `fact_refresh_requested` (`instance_id`, `fact_key`,
  `unknown_reason`, and `rule_id` when a selection document rule read it) per
  key, and so does its dispatch's reason chain, which is how the run learns its
  requested key set (it reads only those keys' marks; a `linear_steps` package
  runs only the segments that reach them). Its decision and dispatch use the
  reserved task id `_fact_refresh`, operation and procedure `_fact_refresh`
  (a catalog id never starts with `_`), the instance's activity profile, a light
  load and a reserved run time of 60 s; its loop budget never binds. It takes
  the highest rank (catalog rank plus manual offset) of the candidates it
  unlocks and competes with the instance's other candidates; one winner per
  instance and the shared run limits ("Shared Run Limits") apply as usual.
- **Its own gates**, before the allocation: an instance without an activity
  profile (a limit, `activity_profile_missing`); the rest after failed runs in
  a row (a limit, `failure_pause`; Workflow #490 FR-1, since 0.12.81): when its
  latest run failed, its failed runs in a row since its last success reach
  `max_consecutive_failures` and its latest dispatch lies in the current
  activity day, R_I rests `failure_backoff_ms × 2^(k+1)`, `k` the failures in
  the row beyond the threshold (0 at it), at most 7 200 000 ms (two hours),
  counted from its latest failed dispatch (the admission instant of the pair's
  latest dispatch, `last_dispatched_unix_ms` of its task snapshot) and never
  past the end of the current activity window, waking at the rest's end (the
  earliest instant it may run again: after a long run, with a minimum interval
  longer than the rest, or with a backoff above two hours, the cooldown gate
  that follows names the later, real one): with
  the shipped settings 20 minutes after the third failure in a row, 40 after
  the fourth, 80 after the fifth and two hours from the sixth on. The rest ends
  by itself at its end, and at once when a scheduled run of another task of
  the catalogue on the same instance, dispatched after that latest failed
  dispatch, succeeded (the instance's task snapshots: each pair's latest
  admitted dispatch of the scheduler, no direct run, recovery ladder rung or
  return-home run). A failure of an earlier activity day rests nothing. The
  arithmetic saturates, and a `failure_backoff_ms` of 0 gives no rest: the
  minimum interval and the daily limit then bound R_I, as they do below the
  threshold. Then its
  dispatches in the current activity day reaching `daily_limit` (a limit,
  waking when the activity day's window ends); an unavailable instance, a host
  that cannot take its load (for now); and `task_cooldown_active` until both
  `min_interval_ms` after its latest dispatch and, after a failed run,
  `failure_backoff_ms` after that run's end (for now), which is what holds R_I
  after its rest ends early.
  Past these, #405 IDLE-1P: on an instance the Runtime's idle action closed as
  the cycle starts, R_I waits, with no wake, for the instance's next natural
  wake unless a candidate it reads for is due (its trigger true): its decision
  carries `instance_idle_action` and the candidates `fact_refresh_pending` with
  that `gate_code` (for now), so a reading alone never brings a closed game back
  ([runtime-config.md](../runtime-config.md), "The `idle` section"). When one
  of the gates above holds R_I there too, its decision keeps that gate's
  reasons and wake and adds `instance_idle_action`, and the candidates keep
  the row that gate gives them (its `gate_code`, or `fact_refresh_exhausted`)
  but take no wake of R_I's, neither as their wake nor as
  `next_eligible_unix_ms`.
  In the allocation the Runtime's admission defers it like any candidate; a
  closed activity window or a spent activity budget, a spent task budget and a
  paused pair are limits, everything else is for now (since 0.12.81 the
  admission never defers R_I as a paused pair, Workflow #490 FR-1).
- **Each affected candidate's reason**, per key R_I reads, by the first row that
  matches; the candidate is deferred, never failed, and the round always
  succeeds:
  1. R_I is selected this round: `fact_refresh_pending` (`instance_id`,
     `fact_key`, `unknown_reason`, `package_id`), no wake;
  2. a limit holds R_I back: `fact_refresh_exhausted` (`instance_id`,
     `fact_key`, `package_id`, `refresh_limit` = `daily_limit`,
     `failure_pause`, `activity_window` or `placement`; `gate_code` when the
     Runtime's admission named the gate; `next_eligible_unix_ms` when known);
  3. R_I's latest dispatch settled successfully at or after L, the instant this
     Runtime finished loading the instance facts, and the key is still unknown
     because of it (no record since that dispatch, or one recognition could not
     use: too little confidence, not a scalar, another type):
     `fact_refresh_unresolved` (`package_id`; `next_eligible_unix_ms`, R_I's
     next eligible time);
  4. anything else: `fact_refresh_pending` with `gate_code` (the code that
     deferred R_I), `failure_code` (the code R_I's latest run failed with, when
     it failed) and `next_eligible_unix_ms` when the gate names one.
- **Settings.** `scheduling.fact_refresh.enabled`, `min_interval_ms`,
  `daily_limit`, `failure_backoff_ms` and `max_consecutive_failures`
  ([runtime-config.md](../runtime-config.md)), read at every cycle; they are not
  part of any `detection_budget`.
- **Dispatch.** R_I is a policy dispatch of the instance's one queue (claim kind
  `policy_dispatch`): the Runtime admits, budgets and settles it like a catalog
  task's, under the Runtime-owned task spec, and runs the kept package by its
  location. Before any input the run checks that the package still qualifies as
  a reader; one that no longer does (a designated operation, no marks,
  `recognize_only`, a stability termination, or, Workflow #455 TK-1 since 0.12.89,
  a variable only a rule gives, `rule_given_variable`, which a refresh run can
  never be given) fails the run with
  `fact_reader_binding_invalid` and its `reader_problem`, which counts as one of
  R_I's failures. The readings are published as instance facts before the run's
  terminal, and the next round scores the candidates that read them. The fact
  refresh writes nothing to the ledger itself: R_I's dispatch, run and readings
  are recorded as every policy dispatch, run and reading is.
- **Transitions** (Workflow #384 D2-S5, D2 model §5.1 MP-3, §5.3 A-5, §6 S5).
  At each evaluated cycle's yield point a bounded recorder reads the reasons of
  the waiting candidates (the four codes above) and turns their changes into
  transitions, per (instance, key): the key starts to be waited for (its
  reason), the reason's code changes (the new reason), the key is no longer
  waited for (`fact_refresh_resolved`, info: `instance_id`, `fact_key`, and the
  `package_id` that served it), and one wait has lasted
  `scheduling.fact_refresh.overdue_ms` (`fact_reader_overdue`, warning:
  `instance_id`, `fact_key`, `elapsed_ms`, `package_id`; once per wait, only
  while a package serves the key, never while the setting is unset). The same
  input again gives no transition; an instance the closure's positions 1-2 hold
  raises no demand, so its keys keep their state and their wait goes on. Cycles
  and transitions are numbered from 1 per owner epoch, one by one, never reset.
  The recorder holds one entry per waited-for (instance, key), so at most the
  instances times the keys their candidates read. It only produces transitions:
  no decision reads it, and it writes nothing to the ledger; actingd writes each
  transition as one developer line (its code at its category's level, its
  values, `cycle_seq=<n> transition_seq=<n>`) and one `OUTCOME actingd:` line in
  the same write, and the ledger observer's G5 gate copies them from AO-4 on as
  `policy.fact_refresh_transition` ([v012-register.md](../v012-register.md) §1.2
  D1). `selection_input_unknown` names a rule, not a key, and is no transition.
- **Status read.** `FactRefreshStatus` answers what the recorder holds after the
  latest cycle: `{cycle_seq, transition_seq, instances: [{instance_alias,
  demands: [{fact_key, outcome, package_id?, since_unix_ms, overdue}]}]}`, from
  memory, appending nothing; `actingctl fact-refresh` prints it and `ac_overview`
  answers it as its block `fact_refresh`
  ([runtime-interface.md](../runtime-interface.md)).

## Remedies (Workflow #411 B15)

Written ahead of its code (B15 R0); slices R2 (the evaluator's plan) and R4 (the host) implement
it. [remedies.md](../remedies.md) is the contract: a policy run that succeeds on a mapped key whose
mapping declares a `remedy` opens a remedy round for its (task, instance) pair, and the evaluation
plans the round from the remedy table, the pair's recent runs, the recorded stops, the instance's
target record `session.remedy.choices` (an overlaid instance fact) and the configuration rows.

- **Trigger override and promotion.** Beside the immediate rerun (R22, `failure_retry_immediate`)
  and an owed rerun (`policy_rerun_owed`), a third Runtime-made trigger override exists: an allowed
  provider of the blocked pair's need is treated as triggered and past its cooldown and promoted
  ahead of the instance's other candidates, with one `policy_remedy_requested` reason per blocked
  pair it serves (at most 16); after the remedy is served, the blocked pair is treated so once,
  with `policy_remedy_retry`. Placement, the feedback stop, the deferrals, budgets, activity
  windows, one winner per instance and the shared run limits still apply. Between several requested
  providers the ranking of "Score-Assisted Priority" decides, unchanged.
- **Hold and stop.** While its round is open the blocked pair is deferred with
  `policy_remedy_waiting` at position 8a of the order of deferrals
  ([scheduling-blackout.md](../scheduling-blackout.md)); a stopped round defers it there with
  `policy_remedy_unchosen`, `policy_remedy_unavailable` or `policy_remedy_exhausted` until the
  stop is lifted. For the clock-slot records below, the three stop codes are persistent gates
  (`clock_slot_refused`) and `policy_remedy_waiting` is a waiting gate (`clock_slot_deferred` only
  once the window passed).
- **Run parameters.** A dispatch of a task whose package declares parameters carries
  `policy_dispatch_parameters` with the values the run reads as `session.dispatch.parameter.*`.
  Workflow #411 B8: a dispatch of a V2 task with `dispatch_parameters: true` carries, on the same
  reason and whether or not remedies are on, its schedule values (`next_occurrence_unix_ms`,
  `window_end_unix_ms`, `last_in_window`, `last_daily_slot`, `daily_remaining`), which the run
  reads as `session.dispatch.schedule.*` ([remedies.md](../remedies.md), section 4).
- **Records.** The reasons travel in the existing dispatch reason chains; the stop, lift and expiry
  are `goal_missed` planning signals with the `signal_id` prefix `signal:remedy:`; an intent whose
  round closed between evaluation and admission is refused with `policy_remedy_round_closed`. The
  evaluation's output keeps its shape, and with no remedy declared every evaluation is
  byte-identical to one without remedies.

## Chain Handover (Workflow #411 B41)

A chain step that hands over on its page goes first (coordinator triage Q15; register §1.157).

- **Handover.** On an instance, a round has a handover when a candidate of the round, the
  *successor*, follows a predecessor on the page the instance rests on: its trigger names
  `dependency_completed` (outside a `not`, accepting `succeeded`) of the predecessor; the
  predecessor's latest run on the instance succeeded; the instance's `task.page` runtime fact (the
  page its latest recognition matched) was recorded between that run's admission and its end, so
  nothing recognized another page since; and the entry page set of the successor's package holds
  that page (its declared `entry_page` or entry page set, or the entry its entry gate checks, each
  without the game prefix). The Runtime reads the package its procedure binding locates, only for
  such a candidate; a package it cannot read holds no page, and its run's own admission refuses
  it. A takeover, an unreachable ADB or an emulator restart drops `task.page`, so no instance has a
  handover until a run recognizes a page again. The first such candidate in rank order is the
  instance's successor.
- **The successor goes first.** It is allocated before every candidate of its instance that ranked
  ahead of it. While it is dispatched, each of those waits with `policy_chain_handover_waiting`
  (info; `instance_id`, `task_id` the successor, `page_id` the page), its decision deferred and
  with no wake of its own: a fact refresh reading too (its decision ends with that reason, and
  each candidate it reads for gets `fact_refresh_pending` with `gate_code`
  `policy_chain_handover_waiting`). The Runtime does not predict where another package's run ends,
  so every candidate that ranked ahead waits, not only those whose entry or prerequisite layer
  goes home. The candidates ranked behind the successor lose to it with
  `instance_already_selected` as before.
- **The waiting work keeps its own due time.** It runs after the chain step. The successor's
  random delay (`dispatch_random_delay_waiting`, position 9 of the order of deferrals) is part of
  its dispatch: while the successor waits for its planned start the candidates it went before keep
  waiting with `policy_chain_handover_waiting` (coordinator ruling 10-21). When any other gate
  defers the successor in the round (an admission gate at positions 1 to 8a, or a shared run limit),
  the candidates it went before take their turn in the same round, in rank order. When the
  successor is no longer a candidate (withdrawn, refused, no longer due) there is no handover.
- **Exception.** A successor whose own trigger or selection document reads a key the fact refresh
  reads is deferred for that key and is no candidate: the refresh goes first, as before. Its run
  then starts wherever the refresh left the instance, and its entry recovery decides as before.
- **Records.** No record is new: the waiting candidates' decisions carry the reason, the
  successor's dispatch decision record lists them with that `why`, and the not-run explanation
  (`ac_schedule`) shows it as their `why`. Without a resting page every evaluation is the one
  before B41.

## Ties and chain heads (SS-2)

Since 0.12.32 (Alice's ruling 104 points 2, 4 and 5, approved 2026-10-30, Workflow #386
comment 6523113979; register §1.222). Point 1, two priority tiers, is the resource side's
(RS-8; [standard-pack-schedule.md](../standard-pack-schedule.md), "Two priority tiers" warns of a
third). Point 3, net benefit within a tier, is SS-2b's ("Net benefit within a tier" below); for a
catalog that marks no task the rank value (`total`, "Dispatch Decision Record" item 1) is the
one of before.

- **Chain link.** A task's trigger *requires* a task when it names `dependency_completed` of
  it, accepting `succeeded`, reached from the trigger's root only through `all`: the trigger
  cannot be true unless that task's latest run on the instance ended in an accepted state. A
  dependency under an `any` is one alternative among others, an order and not a continuation,
  and one under a `not` the opposite: neither is a link. A task that names itself is not its own
  link. ("Chain Handover" also counts the `any` branches: it only decides who goes first on a
  page the instance already rests on.)
- **The head's run window.** A candidate continues a task it requires while, on its instance,
  that task's latest run succeeded and the candidate has not served it: it has no dispatch since
  that run's dispatch, or its own latest run since did not succeed (the test of
  `since_own_run`, "Condition edges"). Of several such tasks, the one with the latest dispatch
  (then the smaller id). Its *chain head* is found by walking up: from the task it continues, to
  a task that one requires whose latest run succeeded and was dispatched at or before its own
  latest dispatch, and so on, never visiting a task twice. The other task is read as a gate
  reads another task (after the timeline resets), the candidate itself from its own last result
  (Workflow #411 P19).
- **Chain inheritance.** In that window the candidate ranks with its chain head's `priority`
  and `strategic_weight_milli` (the head's `instance_overrides` entry for the instance applied)
  instead of its own; every other term (aging, urgency, contention, the score stage, its resource
  term) stays its own. Outside every window, it ranks with its own. Only the task layer reads
  this: the pool weights and resource terms read no task state (one-way rule, SS-1). The fact
  refresh lends a candidate's rank as before, inheritance included.
- **Tie-break (point 4).** Of candidates equal in `promoted`, `total` and `affinity`, the one
  closer to losing its eligibility goes first, by a key computed at the evaluation instant from
  the inputs its gates read, in integer arithmetic:
  - *time left*: for each `timeline_active`, and each `clock` with `missed`, that the trigger
    requires (through `all`), the milliseconds until it stops being true (the event window's end,
    the occurrence's grace end); the least of them;
  - *runs left*: for each threshold its own runs lower, the runs, this one included, that still
    meet it, by the task's declared consumption `c` of the pool: a required `resource_projection`
    on a pool it consumes, or a required `fact` on the reading key of a pool it consumes (same
    scope), compared `greater_than_or_equal` T (`⌊(x − T)/c⌋ + 1`, 0 below T) or `greater_than`
    T (the same with T + 1), T a number or a fact reference plus its offset; and each of its
    `reserves` on a pool it consumes (`⌊(x − at_least)/c⌋`, 0 below it), x the pool's projection
    or the fact's integer as the gate reads them; the least of them;
  - *chances*: the fewer of the runs left and the runs that can still start before the time left
    ends (`⌈time left / E⌉`, E the candidate's expected duration).

  A key with a time left or runs left is `bounded`; one without either is `none`; one that meets
  an unknown input (no pool value, a fact read as unknown, an evaluation error) is `unknown` and
  never fails the evaluation. Bounded keys go first, by fewer chances, then less time left, then
  fewer runs left. Then (the fallback, point 4) the key with more *conditions* (the trigger's
  required nodes, an `any` or a `not` counting one, plus its `reserves`); then (point 5)
  *declared order*, the task's position in the catalog's task list (a composed catalog keeps the
  rules' order of `schedule.json` per instance), the earlier first. The Runtime's own fact refresh
  task has no key (`unknown`, no condition) and comes after every declared task in a tie. The
  deterministic `tie_breaker` hash comes only after, for one task's candidates on different
  instances.
- **Records.** No record is new: `rank_breakdown` and the ranked `decision_candidate_not_selected`
  details carry `chain_head` and the tie-break key ("Dispatch Decision Record"); the developer
  text is never persisted. Without a chain window and without a tie, every evaluation, its ranks,
  reasons and details included, is the one before SS-2. No catalog member, configuration key or
  outcome code of the evaluation is added; a catalog serializes and hashes as before.

## Net benefit within a tier (SS-2b)

Since 0.12.32 (Alice's ruling 104 point 3, approved with 104 on 2026-10-30 and checked by the
research channel, Workflow #386 comment 6523148251; ruling 116 of 2026-10-30: business dispatch
waits until its values exist, 「当业务有了数据之后直接写进任务包」; register §1.225). No configuration
key and no per-instance mode: a deployed Runtime judges every marked task.

- **Who is judged.** A V2 task may carry `benefit_judged: true` ([v2/README.md](v2/README.md));
  the standard-pack composer writes it on every rule whose package is not maintenance-role
  ([standard-pack-schedule.md](../standard-pack-schedule.md), "Classification") and that does not
  declare itself maintenance (SS-2c, since 0.12.32, register §1.227: the `schedule.json` rule
  member `"maintenance": true`, "The maintenance declaration" there; maintenance tasks are not
  judged by net benefit, Alice's ruling 104 point 1 and design §4.10). A raw V2 catalog sets the
  mark itself; `maintenance` is no V2 task member. Such a task is a
  *business* task. Its candidate is *judged* unless it continues a business chain or is a manual
  request: a task whose trigger requires an instruction, a `fact` or `fact_time_reached`
  predicate on a key of the `session.` family reached from the root only through `all` (design
  `rd-1029/design-sch-state.md` §4.9). SS-2e (since 0.12.33, register §1.229; design §4.10: a
  chain's value is recorded on its head, a business package): a candidate in its chain head's run
  window ("Ties and chain heads") continues a business chain when at least one of the links walked
  from it up to that head, the head included, is a marked task; it then rides with that chain and
  is not judged. A candidate whose walked links are all unmarked, such as a business task that
  follows a return home, is a business decision of its own: it is judged like a candidate in no
  chain window, while its rank still carries its chain head's `priority` and
  `strategic_weight_milli` as SS-2 gives them. So in `M → A → B`, `M` unmarked and `A`, `B`
  marked, `A` is judged and `B` rides with `A`. Every other candidate is unjudged.
- **The net benefit** of a judged candidate `k`, in milli, the exact sum floored once:
  `B(k) = Σ_p produce(k,p)·W(k,p)/Q_p − Σ_p consume(k,p)·P(k,p)/Q_p`, with checked 128-bit
  intermediates. `produce(k,p)` is the resource layer's own `r(k,p)` (`amount` or v2
  `expected_amount_milli`; [resource-targets.md](../resource-targets.md), "Resource weights (v2)"),
  `W(k,p)` its effective weight `B_p + Γ` (the base weight plus a covering goal's shortfall
  weight; Workflow #455 G-R2s gave `B_p` alone to a task that carried the irreversible mark from
  0.12.60 to 0.12.76, and G-R2x, 0.12.77, withdrew it: such a task reads `B_p + Γ` as every task
  does, [resource-targets.md](../resource-targets.md), "A task that can do something
  irreversible"), `consume(k,p)` the task's declared consumption of the pool (its `consumes` amounts;
  SS-2g, since 0.12.34: in thousandths, `C/1000` exactly, an expected consumption included,
  "Expected quantities (v2)"; the reserve gate reads `⌈C/1000⌉`) and `Q_p` the pool's
  `valuation.scale`. `P(k,p)`, the cost of consuming pool `p`, is `W(k,p)`, whatever the pool's
  fill and whether it regenerates or not: the cost of a consumption is `W·c` (SS-2h, since
  0.12.46, register §1.289; Alice's ruling 138 of 11-07 and her answer of 11-09 16:39: below the
  cap nothing is spent for free). Until SS-2h a regenerating pool's cost was discounted between
  the task's reserve line on it and its capacity, `P = W·clamp((cap − cur)/(cap − reserve), 0,
  1)`; that discount is retired. A pool that reached its cap is spent by the capped spending order
  ("Capped spending order"), which also gives the head of its order its own `B` (below). Nothing
  here writes a pool weight (the one-way rule, SS-1). #455 B1 (since 0.12.82; register §1.518):
  a task with a fixed value reads of what it produces only its goal terms, each at its goal's
  `scale` ("Fixed values written in the pack" below, "A goal on what it produces").
- **Unknown is never 0.** A judged candidate whose task declares no `produces` and no `consumes`
  entry (SCHED-C1a, since 0.12.52: and no fixed value, "Fixed values written in the pack" below,
  which reads of its `produces` only its goal terms, so a pool it produces never makes its `B`
  unknown), an entry whose `amount` is the pending marker, a pool it touches whose `valuation` is the
  pending marker or absent (for the instance; #455 SL1: but for a pool that writes `spend_line` and
  that it consumes, "A declared 0" below), or a regenerating pool it consumes whose reading is
  unknown or expired (still read, as before, though its cost no longer depends on it) has an
  unknown `B`: the known part is never added up. It is not dispatched:
  deferred with `benefit_unknown` (info; `benefit_gap` `no_effects`, `entry_pending`,
  `valuation_pending`, `valuation_missing` or `reading_unknown`; `pool_id`, and `fact_key` with
  `unknown_reason` for an unknown reading of a fact), no wake of its own. An unknown reading also
  asks the fact refresh for it ("Fact Refresh"), as a predicate on it would; that candidate's
  decision then ends with the refresh's reason.
- **A declared 0** (#455 SL1, since 0.12.97; register §1.540; "A pool whose spending a line
  governs"). A pool that writes `spend_line` (it writes no valuation) and that a judged task
  consumes makes no `B` unknown, with or without a fixed value on the task. While no line is set,
  its consumption adds **no term** to `B` and **no share** to `ε`, whatever its amount, and the
  judgement's items (`policy_benefit_nonpositive`, `benefit_within_tolerance`) carry
  `<pool>:-0` at its place in the consumption's declaration order, so a reader of the decision sees
  the consumption was read and cost nothing. This 0 is a declared one, not an unknown read as 0:
  the pool says its spending is governed by a line, and no line is set. Its reading is not read for
  the cost, and the pool takes no part in the pools' lines (below). An entry on it whose `amount`
  is the pending marker stays `entry_pending`; a task that produces it is judged as with any pool
  without a valuation (without a fixed value `valuation_missing`). A task without a fixed value
  whose only effect is such a consumption has `S = ε = 0`, a sum without a rounding bound, which is
  exact: its known `B = 0` is `policy_benefit_nonpositive`, not within tolerance ("Within
  tolerance", *Three ways*). No reason code is new. For example, a rule of `V` 500 that consumes
  120 of such a pool was `benefit_unknown` `valuation_missing` and never ran; it is now `B = 500`,
  as it was before the consumption was written; a rule of `V` 50 000 that consumes the same 120 and
  30 of a pool valued 1000 milli a unit is `B = 50 000 − 30 000 = 20 000`, the cost and the `ε` of
  the valued pool alone.
- **Not positive.** A judged candidate whose known `B ≤ 0` is not dispatched: deferred with
  `policy_benefit_nonpositive` (info; `benefit_milli`, `expected_duration_ms`); a regenerating
  pool it consumes wakes the evaluation at its next step, as before (SS-2h: its cost no longer
  falls as it fills, but at its cap the capped spending order takes it).
- **The pools' lines** (SS-2h, since 0.12.46, register §1.289; "Pool lines", "Capped spending
  order"). After the judgement and before the decision, the lines of every valued pool a judged
  candidate consumes decide. In a capped pool's round the candidates its order tries pass with
  their own `B` whatever its sign (the head's `B = V − W·(c − a)`, `a = min(c, max(0, r − S))`,
  `V` its value without the pool's cost, `c` its consumption, `r` the reading, `S` the stop
  line), and every other judged consumer of the pool is held (`pool_line_held`); below the cap a
  candidate that would pass (positive, or released in the zero tier) but whose run would leave a
  pool below its hard line is held (`pool_line_held`, `would_cross_hard_line`). A `B` that is not
  positive ranks after every positive one of its tier (the comparison `B·E' > B'·E` is signed).
- **Ranking, per instance** (catalogs that mark a task; "Dispatch Decision Record" item 1):
  `promoted` first, as before; then the tier, the maintenance tier (every unmarked task, and the
  Runtime's own reading) before the business tier (the marked tasks), as ruling 104 point 1 puts
  maintenance high and business normal; within a tier the unjudged candidates first, in the order
  of before (`total`, then `affinity`, then the tie-break), then the judged candidates by their
  net benefit per expected millisecond `B ÷ E`, the higher first, `E` the duration the utility
  term divides by (the pair's expected duration E, else its last settled run's, else the declared
  one; at least 1), compared exactly as `B·E' > B'·E`; then `affinity` and the tie-break of
  "Ties and chain heads" on integer equality. The handwritten `priority` no longer orders judged
  candidates of one tier. The reserve gate takes each instance's candidates in this order.
- **Turns across instances** (SS-2f, since 0.12.46; Alice's answer K2 to question 137, 11-02
  19:4x, design `rd-1031/design-acm-price-addendum2.1.md` §5.4; register §1.288). Each game
  values its candidates in its own unit, so `B` compares only within one instance. In the
  business tier two candidates of different instances therefore go by their instances' turns,
  before every other key of the tier: the instance whose most recent policy dispatch is the
  earliest first, an instance never dispatched before every dispatched one, and at the same
  instant (two never dispatched included) the smaller instance id. An instance's most recent
  policy dispatch is the latest `last_dispatched_unix_ms` of its task snapshots, the recognition
  maintenance package's (the fact refresh's) included: the snapshots every evaluation already
  receives, which the Runtime projects from the policy dispatches it admitted, so nothing new is
  read and no ledger query is added. Within an instance the order above stays (unjudged first,
  then judged by `B ÷ E`); the turns apply among the promoted candidates and among the released
  ones of the zero tier ("Within tolerance") too, the zero tier still last.
  The maintenance tier, "one dispatch per instance", the reserve gate and the shared run limits
  are unchanged; the round's order stays total. A catalog without `benefit_judged`, or a round
  with no business candidate, orders as before. No configuration key, no new code or record.
- **Where it sits.** The gate is part of the score stage: it follows the stage's own deferrals
  and precedes the reserve gate, so a candidate it holds takes no reserve room, and it comes before
  the closure's positions ([../scheduling-blackout.md](../scheduling-blackout.md), "Order of the
  deferrals"), as `score_deferred` does. The idle plan counts a decision it holds neither due nor
  as a wake of its own.
- **Records.** No ledger record kind is new. A judged winner's `rank_breakdown` carries
  `benefit_milli` and `expected_duration_ms` as values and ends its detail with ` benefit_milli=<B>
  expected_duration_ms=<E>` (a ranked judged candidate of the record does too); the decision
  record and the not-running row name the new reasons as `why`.
- **No mark, no change.** A catalog without `benefit_judged` keeps its hash and decides byte for
  byte as before: no candidate is judged and the order is the one of before.
- **Declared maintenance** (SS-2c). A rule that declares itself maintenance composes unmarked, so
  its task is in the maintenance tier and its candidate is never judged: it ranks there in the
  order of before and is dispatched as before SS-2b, its `priority` and every gate unchanged. A
  `schedule.json` that declares nothing composes byte for byte as with SS-2b alone. The
  evaluator reads no new member: it sees only the mark, as before.
- **Within tolerance** (SS-2d, since 0.12.35; Alice's ruling 127, 10-31 13:19, the review's
  version; register §1.235; design `rd-1031/design-acm-price-addendum2.1.md` §7). It narrows
  "Not positive" above; no configuration key.
  - *The bound.* The exact sum `S` (before the floor) differs from the model's value at the
    exported prices by at most `ε = Σ_produces [R/(2000·Q) + n·(2W+1)/(4000·Q)] +
    Σ_consumes [C/(2000·Q) + m·(2W+1)/(4000·Q)]`: `R` and `C` the pool's per-run production and
    consumption in thousandths (`r(k,p)` and SS-2g's `C`), `n` and `m` its `expected_amount_milli`
    entries in each direction, `W` the weight `B` reads and `Q` the scale. SS-2h (register
    §1.289): the consumption term reads the whole `C`, since no share of the cost is discounted;
    until then it was multiplied by `σ`, the regenerating share of the retired discount. The
    capped spending order lists a consumer by its value without the pool's cost against this `ε`
    without that pool's term ("Capped spending order"). The two roundings are the exported weight's (half to even, half a milli per
    step) and each expected quantity's (half a thousandth). `ε` is an exact rational summed as `S`
    is; an overflow fails the evaluation as the sum's does.
  - *Three ways.* With `|S| ≤ ε` the candidate is *within tolerance*: its sign is the rounding's,
    so it is neither positive nor `policy_benefit_nonpositive`, and its decision carries
    `benefit_within_tolerance` (info; `benefit_milli` ⌊S⌋, `tolerance_milli` ⌈ε⌉, `zero_tier`; the
    developer text gives `S` and `ε` as exact fractions). Outside the band the gate is as above:
    `B = ⌊S⌋ > 0` is positive and ranks by `B ÷ E`, any other known `B` is not positive (so a `B` of
    0 above `ε` is still not dispatched, as before). #455 SL1 (since 0.12.97): a sum whose `ε` is 0
    has no rounding and is not in the band; only a task without a fixed value whose only effect is
    a consumption of a pool that writes `spend_line` has one ("A declared 0" above), `S = 0`, not
    positive.
  - *The zero tier.* A candidate within tolerance whose task carries `benefit_realization: true`
    ([v2/README.md](v2/README.md); the model's verdict that the task is a realization or
    conversion with a real downstream use, one more run wasting nothing), whose `B` is known, that
    produces a pool weighed above 0 on its instance (a model price or a goal's shortfall, which
    since G-R2x, 0.12.77, weighs a task that carries the irreversible mark too; a pool
    verified worthless is weighed 0) and whose latest run there did not fail is *released*
    (`zero_tier: released`). It takes part in the allocation after every unjudged and every
    positive judged candidate of the business tier, of every instance, a promotion not lifting
    it; among such candidates `affinity`, the tie-break and the hash decide (SS-2f: those of
    different instances by their turns, "Turns across instances"). So it runs only when
    its instance has nothing else chosen, and it takes no shared run slot and no reserve room
    from a candidate before it. Every other gate still applies to it: the reserve gate, the shared
    run limits, every pause and hold, and the no-progress guard ("No-progress guard"), which holds
    it after a successful run that changed no reading of the pools it reserves. Any other
    candidate within tolerance is deferred: `unmarked`, `no_downstream_value` or `latest_failed`
    (its failure keeps its own path; the zero tier does not retry it). A deferred one wakes at a
    regenerating pool's next step, as a not-positive one does.
  - *No loop.* The compiler refuses a marked task that is not `benefit_judged`, that lacks a
    `consumes` or a `produces` entry, that does not reserve or does not read by a fact every pool
    it consumes, or whose marked tasks form a cycle through their pools (`realization_cycle`). So
    each run lowers a pool to its reserve line and stops there, and its progress is visible.
  - *The idle plan* counts no decision within tolerance, released or deferred, as due or as a wake
    of its own, unless it was selected: the zero tier only fills an open game's idle time.
  - *No candidate in the band, no change.* A catalog in which no judged candidate falls within
    tolerance decides, ranks and records byte for byte as before; `benefit_realization` absent, a
    catalog keeps its hash.

## Fixed values written in the pack (SCHED-C1a, since 0.12.52)

Register §1.299. Alice, 11-09 15:47 (Workflow #428 comment 6734155178), splits the scoring into two
kinds: 「这个已经有结论了，就直接用固定值」, and
「第一个就是在任务包里面已经写明了，这种就直接沿用就行。另一种是需要动态决策的，这种我们在启用打分器，在自己算。」;
at 15:50, of the model, 「我们的这个数据建模它本身也是一个打分器，只不过它是产出的是离线数据，这个用来解决当前已经固定的。」
Of SS-2b's benefit rule she said at 15:57 (comment 6734197818)
「要是阻碍运行或者是不符合预期的话，就直接按照新的方法重写一遍就行。能跑就暂时可以先不用动它」. It now
blocks running: every judged rule of the standard packs declares no effect, so each would wait
with `benefit_unknown` (`no_effects`) once business dispatch resumes. What follows is the
coordinator's ruling of 11-11, not Alice's words; the member names and codes are this draft's.

**Today.** A judged candidate's `B` comes only from its effects ("Net benefit within a tier"): a
task with no `produces` and no `consumes` is `benefit_unknown` `no_effects`, and every pool an
entry touches needs a declared valuation. The task member `value_milli` (`0..=1,000,000`, read in
V1 and V2 documents alike) is no benefit: it is the score stage's utility term, `value_milli ×
urgency_total_milli / cost_ms`, added to a candidate's `total` whether or not it is judged (and a
task that writes it enters the score stage); the benefit gate never reads it.

- **The member.** A V2 task may write `benefit_fixed_value_milli`, an integer in
  `1..=9007199254740991`: `V`, the value of one run of the task without the cost of the pools it
  consumes, in milli of the value unit its instance's valuations weigh in (the unit of `W`: a
  weight is the milli of value of `Q` steps of a pool). It requires `benefit_judged: true`.
  `value_milli` is not reused: its reader, the score stage, multiplies it by urgency and divides it
  by a duration, for unjudged candidates too, so one number read in two ways would move a rule
  twice and on two scales. It keeps its meaning and its reader. A standard pack writes the value on
  its rule as `fixed_value_milli` and the composer writes the task member
  ([../standard-pack-schedule.md](../standard-pack-schedule.md), "The fixed value"); a raw V2
  catalog writes the task member itself.
- **Without a consumption** `B = V`, known: such a rule is never `benefit_unknown` for lack of
  effects or of a priced pool. A fixed value is exact, so it adds nothing to `ε` ("Within
  tolerance"); since `V ≥ 1`, the candidate is positive and ranks by `B ÷ E`. #455 B1 (since
  0.12.82): plus its goal terms, "A goal on what it produces" below; without one, `B = V`.
- **With a consumption** `B = V − Σ_p C_p·W_p/(1000·Q_p)`, the cost term of "Net benefit within a
  tier" unchanged (#455 B1: plus its goal terms). So below the cap (below the trigger line, "Pool lines") its net benefit is
  `V − W·c` as today, and in a capped round its value without the pool's cost is `V` less the cost
  of the other pools it consumes, ranked by `V ÷ C` as today ("Capped spending order"). A consumed
  pool must be valued for the instance, as before: `W` comes from a valuation the model writes, or
  from a constant valuation the pack declares ("A valuation written in the pack" above); without
  one the candidate stays `benefit_unknown` `valuation_missing` (`valuation_pending` for the
  marker), and a regenerating pool's unknown reading stays `reading_unknown`; #455 SL1 (since
  0.12.97): but a pool that writes `spend_line` costs nothing while no line is set ("A declared 0"
  above), so `B = V` less the other pools' cost. `ε` holds the consumption's terms only. A pool the pack values takes part in the lines and the capped order as
  any valued pool.
- **Its `produces`** entries are not read by `B` but for its goal terms (#455 B1, the next item);
  they keep their other readers (forward projection, resource targets). A produced pool that is
  priced is refused (below).
- **A goal on what it produces** (#455 B1, since 0.12.82; register §1.518; the coordinator's
  rulings of 11-23, not the owner's words). Each pool the task produces that declares no valuation
  (a priced or pending one is refused beside `V`, below) and that a goal in force covers with an
  open gap adds its *goal term* to `B`: its per-run yield `r(k,p)` times the weight the resource
  layer gives the pool for the goal over the goal's own `scale`, exactly as a production term of
  "Net benefit within a tier" ([../resource-targets.md](../resource-targets.md), "Resource weights
  (v2)": `W = B_p + Γ` with `B_p` 0 for a pool without a valuation, `Γ = min(⌊I·g/S⌋,
  1 000 000)`, `S` the goal's `scale` and `I` its `importance_milli`, which the goal must carry for
  such a pool). So `B = V + Σ_g r(k,g)·Γ_g/(1000·S_g) − Σ_p C_p·W_p/(1000·Q_p)`. An expected yield
  brings its rounding bound to `ε` as a production term does ("Within tolerance"), and the
  judgement's items (`policy_benefit_nonpositive`, `benefit_within_tolerance`) name each term as
  `<pool>:+<term>`, before the consumption's (but not where they are not counted, the next
  item). Nothing else of `produces` is read: a produced pool
  no goal covers is not looked up, one whose reading is unknown gives a pending goal (`Γ = 0`) and
  adds nothing, a met goal adds nothing, and an entry of a goal-covered pool whose amount is the
  pending marker counts as a yield of 0 (the coordinator's ruling of 11-23): it adds nothing and
  never makes `B` unknown; a goal never lowers a `B`. Every reader of `B` takes the
  terms: the order within the business tier (`B ÷ E`) and the reserve gate that takes the
  candidates in it, the gate's sign and its band (but below the trigger line of a pool it
  consumes, the next item), a capped round's listing and frozen order (its
  value without the pool's cost, `S` plus that cost, over the cost) and the head's own `B`
  ("Capped spending order"). For example, two stages of `V` 40 000 each consume 30 of a pool
  valued 1000 a unit (`B` 10 000 each, the tie-break deciding between them); one also produces 2
  a run of a pool without a valuation on which a goal asks for at least 40, `scale` 10,
  `importance_milli` 1000, read at 10: `g` 30, `Γ = ⌊1000·30/10⌋ = 3000`, its term
  `2000·3000/(10·1000) = 600`, so it is 10 600 and goes first. The goal's spend values
  (`gap_remaining` and its part in `run_cap`, [../remedies.md](../remedies.md), section 4) are as
  before, but that a goal never takes `run_cap` away (ruling 10): an expected yield bounds it by
  ⌈gap·1000 ÷ the yield in thousandths⌉ runs, at least 1, where before it removed it ("Expected
  quantities (v2)", SS-6); #455 RR3b (since 0.12.98): on a pool that writes `goal_tiers` the gap a
  run is bounded by and handed is the need's part while it is open, else the stock's, and the goal
  term is that pool's `Γ` of two parts ("A goal's gap in two parts"). Without such a goal every value, record, decision and identity is as before; what a
  catalog accepts or refuses, its hash and the `requires` names are unchanged, and an older
  Runtime ignores these productions as before.
- **A goal chooses which task spends a pool, the pool's lines choose when** (#455 B1 ruling 9,
  since 0.12.82; the coordinator's ruling of 11-23 as corrected after the review of #1884, not the
  owner's words). Where such a task consumes a pool that regenerates (its `projection` is not
  `none`) and the pool is below its trigger line ("Pool lines": the `trigger_line`, or the
  capacity without one) with no capped episode under way, or its lines give no value then
  (`capped_cap_unknown`, `lines_invalid`: counted as below), whether it may spend is judged by its
  benefit **without** its goal terms (`S` and `ε` without them, which the judgement keeps beside
  the sums with them), exactly as before them: a task positive without them spends as before (its
  hard line still holds it), and its order then reads its `B` with them; a task positive only by a
  goal term waits, with the reason it carries without the goal (`policy_benefit_nonpositive` with
  that `benefit_milli`, or `benefit_within_tolerance` with that `S` and `ε`). Ruling 11: that
  reason's items leave the goal terms out, its detail ends once with
  ` goal_terms=not_counted_below_trigger_line`, and its `benefit_fixed_value` gives the `B` judged
  (`fixed_value_milli` `V`); the goal itself shows in the candidate's `resource_targets` and
  `resource_weights`. At or above the trigger line, and for an episode under way, the capped
  spending order reads `S` with the terms (its listing, its frozen order, the head's `B`): when the
  pool reaches its line the task that produces the lacking pool heads the round and sweeps down to
  the stop line, its `run_cap` the capped rule's (LF-1, "What the head's run is handed"). A
  consumed pool that does not regenerate (`projection` `none`: a ticket, a shop currency) has no
  "when": the goal terms count there everywhere, so a rule that spends such a currency for a
  lacking material runs when the goal makes it worth it, its own cooldown pacing it. A follower
  of a chain is not judged and is due only after its head's run, so a head waiting below the line
  holds its chain. A worked timeline: a pool of capacity 180, `trigger_line` 160, `stop_line` 140,
  `hard_line` 104, regenerating one unit in six minutes, valued 1000 milli a unit; a task of `V`
  9 500 consumes 10 a run (`B` −500 without the goal, `S − W·c`) and produces 2 a run of a pool a
  goal lacks 30 of (term 600, `B` 100 with it). From 110 the pool regenerates: from 114 on one run
  would fit above the hard line, but below 160 the task waits `policy_benefit_nonpositive` −500,
  as without the goal (its detail ending ` goal_terms=not_counted_below_trigger_line`). At 160 (five hours after 110) the episode begins, the task heads it and is
  handed `run_cap` min(⌈(160 − 140) ÷ 10⌉, ⌊(160 − 104) ÷ 10⌋, ⌈30 ÷ 2⌉) = 2: one dispatch, two
  runs, to 140; the pool is read again after that run (LF-1), and the next sweep comes when it is
  back at 160, two hours later. Without ruling 9 it would have run once each hour from 114 on.
  #455 RR3b ruling A1 (since 0.12.98; register §1.542; the coordinator's): ruling 9 was made when a
  goal term had no bound, so any large gap made any rule positive and drained the pool at once;
  `goal_tiers` is that bound ("A goal's gap in two parts"). A goal term of a pool that writes
  `goal_tiers` is counted wherever goal terms are counted, **below the trigger line too**: ruling 9
  leaves out only the goal terms of pools that do not write the member. The judgement keeps,
  beside the sums with every goal term, the sums the gate reads below the line, those without the
  goal terms of pools without the member. So a character's need makes its producer positive there,
  and a stock's capped part lifts a rule that lacks less than the cap gives; such a task then
  spends as any task positive there does (its hard line, the reserve gate and every other gate
  still hold it), and the order reads its `B` with all its terms. When every goal term of the task
  is of a pool with the member, the gate's reason is the ordinary one, without the note; when some
  are left out, its items name the counted goal terms and its detail ends with the note as above.
  For example, a rule of `V` 998 consuming 1 a run of a regenerating pool valued 1000 milli a unit
  is −2 of its own; with a stock's capped term of 4 (`C` 40 at the step 10, one unit a run) it is
  `B = 2` below the line and spends there; a rule of `V` 900 (−100 of its own) is −96 with the
  stock's term and waits, and with a need of 2 units (`I_n` 100 000: a term of 2000) it spends. A
  pool without the member is ruled as above, byte for byte.
- **One scale** (the coordinator's point 3). Static and dynamic candidates of one instance are
  ordered by the keys the business tier already has, with no second stage: `promoted`, the tier,
  SS-2f's turns across instances, then `B ÷ E` compared exactly (`B·E' > B'·E`), then `affinity`
  and the tie-break of "Ties and chain heads"; the zero tier last. A static candidate's `B` is `V`,
  or `V` less its consumption's cost (#455 B1: plus its goal terms); a dynamic one's is its
  modelled sum. In a capped round both
  kinds are listed and frozen by `V ÷ C` alike. They compare because there is one unit per game,
  the unit of its valuations, in which the pack writes its fixed values and its constant valuations
  (`B` of two games is never compared: "Turns across instances").
- **Refused at compile time** (point 4), each at its pointer: in a V1 document the member is
  `unknown_field`; a value that is no integer `type_mismatch`, one outside `1..=9007199254740991`
  `limit_exceeded`; without `benefit_judged: true`, `missing_required_field` at `benefit_judged`.
  With the new diagnostic `fixed_value_conflict` (error) at `benefit_fixed_value_milli`: (a) a
  `produces` entry on a pool whose `valuation` is a declared block or the pending marker, since the
  rule would then have two values, the fixed one and its priced effects. Refusal, not precedence:
  neither may silently win, and a model export that prices a rule's effects removes its fixed value
  in the same change. (b) `benefit_realization: true`, since the zero tier reads a modelled value's
  rounding and a product weighed above 0, which a fixed value does not have. A value of 0 is no
  value: a rule that must not run is held or not written.
- **What a person sees** (point 5). The decision of a judged candidate whose `B` was computed from
  a fixed value carries `benefit_fixed_value` (info; `benefit_milli` `B`, `fixed_value_milli` `V`,
  `expected_duration_ms`) before its other reasons, whether it was dispatched, deferred
  (`policy_benefit_nonpositive`, `benefit_within_tolerance`), held by a pool's lines or spent by a
  capped order. A judged winner's `rank_breakdown` detail adds ` fixed_value_milli=<V>` after
  ` benefit_milli=<B> expected_duration_ms=<E>`. A candidate without a fixed value records as
  before.
- **Provenance** (point 6) is the composer's and the workbench's: which rules run on a fixed value
  and which pools on a valuation, each by who wrote it
  ([../standard-pack-schedule.md](../standard-pack-schedule.md), "Provenance"). The catalog carries
  no origin.
- **Capability.** A Runtime that reads the member reports the always-on subsystem
  `benefit_fixed_value` in `config.subsystems` ([`actingd-check-config.md`](../actingd-check-config.md),
  its row with the code). A `schedule.json` whose rule writes `fixed_value_milli`, or whose pool
  writes `valuation_origin`, lists it in `requires`; the composer refuses the file without it, so a
  Runtime that does not name the subsystem refuses the file (`schedule_requires_missing`) instead of
  reading the rule without its value. An older Runtime given a catalog with the member refuses it
  at its reader (`unknown_field`).
- **Identity.** A task without the member serializes, hashes and decides byte for byte as before.

## Task Reserves (Workflow #411 B16 part 2)

A V2 task may declare `reserves: [{pool_id, at_least}]`: the least amount, in the pool's real
units, that each named pool must still hold after a run of the task ([v2/README.md](v2/README.md)
gives the member's rules). The reserve is a hard gate, never a score penalty (register §1.167).

- **What a run leaves.** In the score stage ("Score-Assisted Priority" above) each candidate of
  such a task gets, per reserved pool, `after = projected − consumes`: the pool's projected
  amount at the evaluation (as the `resource_projection` predicate reads it) less the task's
  declared consumption of the pool (the sum of its `consumes` amounts for it, an expected
  consumption rounded up, SS-2g, "Expected quantities (v2)"; 0 when it declares none). Every
  "declared consumption" below reads the same integer. The selection document reads them as the
  integer fields `task.consumes.<pool>`, `task.after.<pool>` and `task.reserve_gap.<pool>`
  (`after − at_least`); the last two only
  while the pool's amount is known. A catalog with reserves runs the score stage even without a
  selection document, value, offset or resource target; every candidate without reserves keeps
  its rank, its reasons and its fields.
- **The gate.** After the stage, on each instance, the ranked candidates are taken by
  `effective_milli`, the higher first (a tie in allocation order). A candidate with reserves is
  not admitted this round when, for one of its reserved pools:
  1. the pool's amount is unknown (no usable reading or snapshot, or, #386 CF-5, a capacity fact
     that gives no capacity; #477 LF-1, since 0.12.80: or a reading older than the end of a run
     that was told its turns, "Pool lines", "At each evaluation"), so the reserve cannot be shown
     to hold;
  2. `after < at_least`;
  3. `after`, less the declared consumption of every other task whose latest dispatch on the
     instance is at or after the pool's latest observation (a reading that does not show that
     run yet), is below `at_least`;
  4. that amount, less the declared consumption of a candidate of the instance taken before it
     and not held, is below `at_least`: of two tasks that each fit alone but not together, the
     higher `effective_milli` runs this round, whatever their priorities. SS-2b (since 0.12.32):
     in a catalog that marks a task `benefit_judged` the candidates are taken in the allocation
     order instead (the tier, then `B ÷ E` for the judged ones; "Net benefit within a tier"),
     and a candidate the benefit gate holds is not among them.

  Its decision is deferred with `resource_reserve_insufficient` (info, `ladder_class` none;
  `pool_id`, `reserve_at_least`; `amount_after`, what the pool would hold after it, absent in
  case 1; `task_id` in cases 3 and 4, the task whose consumption goes first), and it takes no
  part in the allocation. A regenerating pool wakes the evaluation at its next projection step;
  otherwise the next reading (FactsChanged) brings the next evaluation. A candidate without
  reserves is never held, and its consumption still counts against the others.
- **Scored again after the refresh.** The other task of such a pair waits while the pool's
  latest reading predates the winner's dispatch (case 3), and is scored again on the first
  reading or adjustment taken since. A pool that is never read again after the winner keeps it
  waiting; a package that consumes a reserved pool should read or adjust it at its end
  ([resource-readings.md](../resource-readings.md)).
- **Several instances on one pool.** The gate counts the candidates and dispatches of the
  candidate's own instance only. Instances that share one pool are Workflow #367's
  multi-dispatch and are not handled here.
- **Records.** No record is new: the reason travels in the decisions and, when a dispatch
  decision record lists a held candidate, as its `why`. Without `reserves` every evaluation is
  the one before B16 part 2, and the catalog keeps its bytes and its hash.
- **Spend values (SS-6, since 0.12.31; register §1.218).** A selected dispatch of a task with
  reserves on pools it consumes carries, on `policy_dispatch_parameters`, each such pool's
  allowance (the gate's own available amount, its projection less the consumption step 3
  counts as not shown yet, less `at_least`, at least 0), and a run cap from them (at most 1000);
  a task that produces a pool an active goal covers also carries the goal's gap
  ([../remedies.md](../remedies.md), section 4, "Spend values"); from Workflow #455 G-R2s
  (0.12.60) until G-R2x (0.12.77) a task that carried the irreversible mark carried the values of
  an instance without a record instead; since G-R2x it carries the goal's gap as every task does.
  An allowance below the task's
  declared consumption of the pool, 0 included, is never dispatched: the gate holds it.
  #477 LF-1 (since 0.12.80; register §1.509): a pool's own lines take part too, so a rule needs
  no reserve to bound its turns. For a judged candidate (the candidates the lines constrain,
  "Pool lines", "Who is held"), each pool it consumes that has a declared valuation and whose
  lines in effect on the candidate's instance are written (the instance's own set, else the
  catalog's; not the defaults) has the floor of its hard line, whether or not the task reserves
  it; where it does, the floor is the larger of the hard line and `at_least`. Its allowance is
  the lines' gate's own reading `r` ("Pool lines", "At each evaluation") less the floor, at
  least 0, at the same entry, and `run_cap` counts it as it counts a reserved pool; on the head of
  the pool's capped episode its turns stop where the episode is spent ("Capped spending order",
  "What the head's run is handed"). The lines' gate holds a candidate whose run would leave
  `r − c` below the hard line, so a dispatched one's allowance pays for at least one run here
  too. A pool without lines, a pool without a valuation and a candidate the lines do not
  constrain hand exactly what they handed before.
  #477 LF-1, after the two reviews of #1872 (since 0.12.80; register §1.509; the coordinator's
  ruling, not the owner's words): the lines' allowance is the lines' gate's own reading, so the
  cap and that gate read one number; a reserved pool without lines is read as SS-6 read it. A run
  told its turns leaves every pool its task consumes unknown until it is read after that run's
  end ("Pool lines", "At each evaluation"), so no allowance and no `run_cap` is computed from a
  reading such a run made stale: step 1 holds a task that reserves the pool, and the lines' gate
  a judged candidate that consumes a pool with lines (`capped_cap_unknown`).

## Shared Run Limits (Workflow #427-B)

Since 0.12.22 (Workflow #427 plan r2 point 5, approved whole by Alice on 2026-10-28) the instances
of a host share only what is really shared, so that five instances run like five players once
their runs go on in parallel (#427-A):

- **No virtual deduction in the round.** The evaluator no longer charges a chosen dispatch's CPU,
  GPU and IO load against one host budget for the rest of the round, and there is no budget pool
  per instance either. `host_budget_deferred`, the reason that deduction gave, is retired.
- **Placement stays.** A task whose own load (its `load_profile`, or its instance override's)
  exceeds its host's `cpu_available_milli`, `gpu_available_milli` or `io_available_milli` as the
  round starts is blocked at placement with `host_capacity_insufficient`, as before.
- **Heavy-scene slots.** A heavy candidate (`performance.load.heavy_threshold_milli`) takes one of
  its host's heavy-scene slots, `performance.pacing.heavy_dispatch_limit.normal`, or `.paused`
  while the performance controller is above Normal. The slots in use are the heavy policy
  dispatches admitted and not yet settled (running, or admitted and not started; any instance)
  plus the heavy ones chosen earlier in the round. Each dispatch counts once: the controller's
  count, capped at the limit, and the policy host's own count of its admitted dispatches name the
  same dispatches, and the larger is used, so under pressure the real number is reported. A
  refused admission was never admitted and a settled (completed, failed or cancelled) dispatch is
  no longer, so either frees its slot; a candidate an admission gate defers in the round takes
  none. When the admitted ones alone take every slot, a heavy candidate is blocked at placement
  with `heavy_scene_budget_exhausted`; when the round's earlier choices fill them, it is deferred
  with `heavy_scene_budget_deferred`.
- **At most so many runs** (optional). `performance.pacing.max_concurrent_runs` (an integer from 1,
  no default; [runtime-config.md](../runtime-config.md)) limits the policy dispatches admitted or
  running across every instance plus those chosen earlier in the round. A candidate past it is
  deferred with `policy_concurrent_runs_deferred`. Without the key there is no such limit; `1`
  runs at most one policy dispatch at a time across every instance, the fallback should parallel
  runs misbehave. It is checked before the heavy-scene slots and holds every candidate, the
  recognition maintenance package's too.
- **The numbers.** The three reasons carry `used` (the slots taken), `limit_count` (the limit),
  `admitted_count` (taken by dispatches admitted or running) and `round_selected_count` (taken
  by dispatches chosen earlier in the round), so the explanation reads, for example, heavy scene
  2/2 or concurrent runs 3/3. A `heavy_scene_budget_exhausted` has `round_selected_count` 0; its
  `used` may be above `limit_count` when the limit fell under pressure.
- **Pacing modes.** The limits do not depend on `performance.pacing.mode`: `observe` (which never
  slows anything down; the release default before 0.12.31, `pace` since, Workflow #429 R-2), `pace`
  and `full` bind them alike, so under `observe` they are the protection.
- **Online (Workflow #429 R-2, since 0.12.31).** `heavy_dispatch_limit.normal` / `.paused` and
  `max_concurrent_runs` are settable and read from the configuration in effect at every
  evaluation: a change applies from the next round, and no dispatch admitted or running is cut.
- **Where.** The Runtime reads the admitted dispatches from its dispatch journal at each evaluation
  (`RunSlots`, `evaluate_with_run_slots`); the library's other entry points have none beyond the
  snapshot's `active_heavy_dispatches` and no maximum. Nothing is persisted: the reasons travel in
  the decisions, the schedule explanation (`why` keeps its values) and, when a dispatch decision
  record lists such a candidate, as its `why`.

## Application Update Hold (UPD-1)

Since 0.12.30 (design `rd-1029/design-upd1.md` §4, §6; Alice's rulings 95 and 96 of
2026-10-29; register §1.215). When any run, the readings refresh (fact refresh) and the startup
package included (register §1.215 UPD-16), meets the game's "client out of date" dialog (an
`interrupts.json` page with `result: "application_update_required"`,
[package-reference.md](../package-reference.md)), the Runtime records the instance fact
`health.application.update_required` true ([instance-fact-store.md](../instance-fact-store.md),
"Application update facts") and the instance waits for its update:

- **The hold.** Position 4a of the eligibility closure
  ([scheduling-blackout.md](../scheduling-blackout.md), "Order of the deferrals") defers every
  candidate of that instance with `application_update_pending` (`instance_id`, `fact_key`) and no
  wake, except a store update task: a task whose procedure's bound package declares
  `store_update`. The fact refresh is held too (`fact_refresh_pending`, `gate_code`
  `application_update_pending`). The hold does not depend on the ranking (fix round 3): a
  candidate behind the store update task that took the instance, behind the shared run limits or
  held by a chain handover is deferred `application_update_pending` too, never left eligible
  with `instance_already_selected`. Other instances are not held (#419). Nothing in a catalog
  names the hold: it is the Runtime's, so every rule of the game is held without a change.
- **The store update rule.** The game root's `schedule.json` writes one rule per server for its
  store update package whose trigger is the typed `fact` predicate
  `health.application.update_required eq true` (`max_age_ms` null), with its own `cooldown_ms`,
  the time before it looks again when the store has no new version yet
  ([standard-pack-schedule.md](../standard-pack-schedule.md)). The rule passes position 4a and
  every other gate as any task does.
- **After the update.** The store update package's successful run (with or without an update)
  records the fact false before its terminal; the evaluation it wakes lets the instance's
  candidates through again, the runs the dialog interrupted first, as owed reruns
  (`policy_rerun_owed`, "Condition termination and owed reruns" of
  [policy-suspension.md](../policy-suspension.md)); the rest follow their own triggers. The path
  makes no startup claim. A failed store update run leaves the fact true and the instance held;
  its task fails as any task does.
- **Explanation.** `actingctl schedule` and `ac_schedule` show `application_update_pending` as a
  held candidate's `why`; `ac_facts --instance` shows the fact and the run that wrote it. The idle
  plan counts a held decision neither due nor as a wake, so it does not wake the instance for it.
  The gate only sees candidates, so (fix round 1) the idle plan reads the waiting instances and
  the store update tasks of the same evaluation and counts no decision of a waiting instance but
  the store update task's, whatever its reason: a task in its cooldown or before its clock
  trigger, and the fact refresh, wake no waiting instance; the store update task's own cooldown
  still does.
- **No configuration key.** The store update rule's cadence is its `cooldown_ms`; the hold and
  its order are code.

## Order within one device (Workflow #437 ORD-1b, since 0.12.60; the host's inputs: ORD-1c, since 0.12.60)

Register §1.352 and §1.373; the owner's rulings on board #437 as the coordinator recorded them;
[device-group.md](../device-group.md), "One pick per device per round" and "The member in front".
It applies only to a device whose key the device map names for two or more members, and only when
the Runtime passes the order's inputs beside the map (`DeviceGroups::with_order`); without them
the device's one pick is its first candidate in the allocation order, as MAPP-1 built it. **From
ORD-1c on the Runtime passes them at every policy cycle whose device map names a device of
several**, so the selection below is in force for every device of several (which the load still
refuses until the release that first accepts a group of several, [device-group.md](../device-group.md),
"Release."); a cycle whose map names none is handed the map alone, exactly as before ORD-1c. An
instance alone on its device never reads any of it, and a catalog that declares
`max_queue_wait_ms` on its tasks decides there exactly as one that does not.

- **The inputs.** The member in front of the device, or none. For each waiting contender, when its
  wait began; a contender the Runtime gives no start waits from the evaluation's time. The longest
  wait of a task that declares none (1 800 000 ms when no configured one is passed). Two members of
  one device named in front refuse the evaluation (`policy_evaluation_input_invalid`). **What the
  Runtime passes** (ORD-1c, register §1.373; the coordinator's rulings 1, 2 and 4 on board #437):
  - *The member in front* is the member the host holds in front of the device: the member whose
    switch's own steps last ended with its application in front or started, or (since fix round 2)
    whose run's own step last launched or restarted its application, and none once a switch of the
    device was refused or failed (a Runtime-level failure included), once the member's own
    application was stopped or failed to start (by a request or by a step of its run), once a
    ladder rung (a Runtime-level failure of the application restart rung included) or the idle
    action changed what is in front, and none after a start ([device-group.md](../device-group.md),
    "The member in front", "Who forgets it").
    *What it does not see* (register §1.373, fix round 2): an emulator or an application that
    crashes, a person's work by hand on the device, a Lab raw input and a run's own inputs that
    bring another package to the front leave a member named in front that is not. That costs at
    most one switch more, which reads the real foreground, and lets a waiting task start at most
    one run later; no task waits without end, since its wait still counts against its limit. A stop
    command that failed and the ladder's emulator restart rung when it returns before its stop
    forget a member whose application may still be in front; that costs at most one switch made
    early.
  - *The default limit* is `scheduling.device_group.max_queue_wait_ms` in effect as the cycle
    starts (milliseconds `60000..=86400000`, default `1800000`; [runtime-config.md](../runtime-config.md)),
    so an instruction applies from the next round.
  - *The wait's clock.* A pair (task, member) has a wait only while its member belongs to a device
    of several, is not the member in front, and the pair is queued: it is due and has no dispatch
    in flight, and its decision is selected and not yet admitted, or its last reason is a wait
    behind the device's order or behind a run (`instance_already_selected`,
    `device_group_waiting` `held`, `device_group_waiting` `selected` with the order's `limit_ms`,
    `dispatch_instance_held`, `policy_chain_handover_waiting`, `policy_concurrent_runs_deferred`,
    `heavy_scene_budget_deferred`, `heavy_scene_budget_exhausted`). The wait begins at the instant
    of the first evaluation in which the pair is queued and keeps that start while it stays
    queued. Any other decision ends it: not due, a person's hold, a pause, a fence, a cooldown (the
    member's switch cooldown included), the task's own gates; and so does a row
    `device_group_waiting` `selected` without the order's values, which a member held as the cycle
    starts gets behind another member's pick (it is no contender). The admission of the pair's
    dispatch ends it. When a member takes the front (its switch succeeded), every wait of its pairs
    is forgotten; a member that loses the front (another member's switch succeeded) starts its
    queued pairs' waits anew at the first evaluated round after, from that round's instant; with
    none in front every member's queued pairs wait. Past its limit a wait writes nothing: no record,
    no held-set rule, no code.
  - *Its staging* is the eligibility age's: the verdicts of an evaluated cycle are applied when the
    next evaluation starts, and that evaluation passes the starts applied then, of the members not
    in front. An evaluation stages no wait of a member that took the front after it read the
    members in front (register §1.373, fix round 1). Memory only: none of it enters the ledger, a fact, a snapshot or a decision identity,
    and a start of the Runtime begins with no wait and none in front.
- **The contenders** of a device are the candidates of its members, but one the Runtime holds
  whole (`InstanceHolds`: UPD-1's hold, a switch cooldown), one whose member is held as the cycle
  starts (a pause, a fence, a person's hold, a holder of the device, a takeover cooldown, a
  blackout) and one the device's chain handover holds ("Chain Handover"). Those take their turns
  as before; a held member has no waiting task.
- **Each contender** has its expected duration `E` (the duration its dispatch reserves), its limit
  `L` (its task's `max_queue_wait_ms`, [v2/README.md](v2/README.md), else the default) and its
  deadline, its start plus `L`.
- **The selection** runs at the place of the device's first contender in the allocation order, over
  that order; nothing is sorted anew:
  1. No contender left: the device is not selected.
  2. **The chain successor**: when the device's chain successor ("Chain Handover") is a contender,
     it is the pick, whether or not it fits, whichever member is in front and when none is
     ([device-group.md](../device-group.md), "Never interrupted").
  3. **The binding waiter**: of the contenders of every member but the one in front (of all of them
     when none is in front), the one whose deadline is the earliest; at equal deadlines the earlier
     in the allocation order.
  4. **Stay with the front**: the member in front's first contender, in that member's own order,
     that ends by the binding waiter's deadline (`now + E ≤ deadline`: a run that ends exactly at
     the limit has not passed it); its first contender when no other member has one. A remedy
     request, an owed rerun or an immediate retry is first in its member's own order as before, but
     goes first on the device only when it fits, and of a waiting member it gains no precedence.
  5. **Switch**: when steps 2 and 4 pick none, the binding waiter; its run switches the application
     ([device-group.md](../device-group.md), "The switch before a run").
  6. The pick is tried as any candidate (the shared run limits, the admission, the remedy plan).
     Selected, it wins the device. Deferred, it keeps its own deferral and the selection runs again
     from step 1 without it.

  A running task is never interrupted: a waiting task past its limit goes first at the device's
  next free round. The time a switch takes counts in no order.

  **The shared run limits across devices.** The device takes its turn at the shared run limits
  ("Shared Run Limits") at the place of its first contender in the allocation order, and when its
  pick is deferred the selection runs again at once from that same place, before any candidate
  later in the allocation order is tried. So on a host that has a device of several, which
  candidate of an instance alone on another device gets a shared run limit's slot can differ from
  a host without it: the one way a device of several reaches an instance alone on its device. This
  is the design.
- **What a contender that does not run shows**, each with the state, the eligibility and the wake
  (none of its own) of a candidate behind its own instance's winner:
  - of the pick's own member, `instance_already_selected`; one of the member in front passed over
    before a pick that ends in time carries `required_ms` (its `E`) and `remaining_ms` (the binding
    waiter's time left before its limit);
  - of the member in front, when another member's pick switches the device,
    `device_group_waiting` `selected` with `required_ms` and `remaining_ms`, as above;
  - of every other member, `device_group_waiting` `selected` with `limit_ms` (its `L`) and
    `remaining_ms` (its own time left before its limit, 0 once past it).

  A contender tried and deferred keeps its own reason; a non-contender keeps the reason it has
  without the order (the hold, the handover's wait, a capped block's wait). No code is added: the
  values are optional values of the two codes, and travel where reasons travel.
- **Memory only.** None of the inputs enters the facts, a snapshot or a decision identity.

## Dispatch Decision Record

Workflow #308 RT-S1c. Every selected candidate's reason chain ends with the
record of why it was dispatched, appended after `placement_selected` and
`rank_assigned`; the selected decision's `reasons` stay equal to its chain. No other
decision gains a reason. The details below are developer text, kept only in the
in-memory decision; the dispatch events persist each reason's outcome, its code with its
values. Why a listed candidate did not run is persisted as values (Workflow #411 A26,
since 0.12.5): `decision_candidate_not_selected` carries `why`, `decision_state` and
`eligibility`, and `decision_record` carries `omitted_why`. The ranking terms of the
details (`total`, `behind_by`, `priority`, … `tie_breaker`, and `rank_breakdown`'s) have
no value keys and stay developer text.

1. `rank_breakdown`: `total=<t> base=<b> priority=<p> aging_ms=<a>
   strategic_milli=<w> urgency_milli=<u> contention=<c> effective=<e>
   score=<s|none> utility=<ut> offset=<o> target=<s|none> target_id=<id|->
   mode=<adjust|override|-> superseded=<0|1> promoted=<0|1> affinity=<0|1>
   tie_breaker=<u64> saturated=<0|1> lead=<l|none>`.
   - `base` is `total_score` before the score stage,
     `p*1_000_000 + a + w*1000 + u*1000 - c` (`c` is the contention penalty,
     load cost x contention basis points x 100), and `total = base + e*1000`
     unless `saturated=1`.
   - `score`, `utility`, `offset` and `target` are the score-stage terms
     (`none`/0 for a candidate the stage left untouched); `target_id` and `mode`
     name an applied resource target, `superseded=1` an applied override.
   - The ranking order is `promoted=1` first (ahead of every other candidate),
     then `total` descending, then `affinity=1`, then (SS-2, since 0.12.32) the
     tie-break of "Ties and chain heads" above: the candidate closer to losing
     its eligibility, then the one with more conditions, then the one declared
     first; then `tie_breaker` ascending, the lower winning, which now only
     orders one task's candidates on different instances (they never compete for
     one instance). `saturated=1` when a saturating sum clipped the total (the
     unsaturated 128-bit sum differs). SS-2b (since 0.12.32): in a catalog that
     marks a task `benefit_judged`, the tier comes after `promoted` and judged
     candidates rank by `B ÷ E` within it ("Net benefit within a tier"; SS-2f,
     since 0.12.46: business candidates of different instances go by their
     instances' turns first, "Turns across instances"); a
     judged winner's breakdown then ends with ` benefit_milli=<B>
     expected_duration_ms=<E>`, and its outcome carries both as values.
   - SS-2 (since 0.12.32): ` chain_head=<task id>` follows when the candidate
     ranks with its chain head's priority and strategic weight; and when
     `lead=0` (another ranked candidate of the instance has the same total, so
     the tie-break decided) the winner's key follows: ` tie=<bounded|none|unknown>
     chances=<n|none> time_left_ms=<n|none> runs_left=<n|none> conditions=<n>
     declared=<n|none>`. A candidate in no chain window and without a tie keeps
     the detail of before byte for byte.
   - `lead` is `total` minus the highest total among the instance's other
     ranked candidates, `score_deferred` ones included (`none` without one). It
     is negative when the winner was promoted above a higher total, or when a
     higher-total candidate was deferred by score, a shared run limit or the
     admission state; it can be positive while a better-ranked (promoted)
     candidate was deferred.
2. `decision_record`: `targets=<active:<sha>@<applied_at>|expired:<sha>@<applied_at>|unreadable:<code>|none>[ ignored=<server|game>@<ms>]
   related=<n> shown=<k> omitted=<m> omitted_why=<code>:<count>,...`.
   - `targets` is the instance's stored resource target policy, its version
     being `policy_sha256@applied_at` (`contracts/resource-targets.md`). The
     instance, catalog, input ledger position and fact snapshot are fields of
     the dispatch payload already.
   - The related candidates are every other decision of the same instance in
     the same evaluation. `omitted_why` counts the unlisted ones by their last
     reason code in ascending code order, `none` when none is omitted; the codes
     that would take the detail past 1010 bytes fold into `+<n>more` (`n` codes).
   - Value `omitted_why` (optional): the same counts, one record `{why, count}` per
     code in ascending code order, every code (no fold; a records value keeps 64 and
     adds `omitted_why_total`); left out when none is omitted.
3. `decision_candidate_not_selected` (values `task_id`, `why`, `decision_state`,
   `eligibility`), one per listed related candidate, at most eight: those that were
   ranked first, by total descending, then the others, each by task id.
   - `why` (a code) is the `why` of the detail, `decision_state` (vocabulary
     `decision_state`) its `state`, and `eligibility` (vocabulary `eligibility`:
     `true`, `false`, `unknown`) the candidate's eligibility, which the detail of a
     candidate that was not ranked prints.
   - Ranked: `why=<code> state=<state> total=<t> behind_by=<winner total - t>
     priority=… aging_ms=… strategic_milli=… urgency_milli=… contention=…
     effective=… score=… utility=… offset=… target=<s|none> promoted=<0|1>
     tie_breaker=<u64>` (the lower `tie_breaker` wins a tie the SS-2 tie-break
     leaves), then, SS-2, ` chain_head=<task id>` as in `rank_breakdown` and,
     when `behind_by=0`, the candidate's own tie-break key, so the two keys
     side by side show why the winner went first.
   - Others: `why=<code> state=<state> eligibility=<true|false|unknown>`.
   - `why` is the candidate's last reason, the one that settled it
     (`instance_already_selected`, `heavy_scene_budget_deferred`,
     `policy_concurrent_runs_deferred` ("Shared Run Limits" above; before 0.12.22 also
     `host_budget_deferred`), an admission eligibility code,
     `score_deferred`, `task_cooldown_active`, `trigger_false`, …); it is
     never `none` ("Why A Candidate Does Not Run" below).

A stored `actingcommand.resource-targets.v2` policy is named
`targets=active.v2:<sha>@<applied_at>` or `expired.v2:<sha>@<applied_at>`; while
it is active the target fields carry the candidate's resource term instead
(`target=<R>`, `target_id=<override id|*>`, `mode=<override|adjust>`) and
`rank_breakdown` ends with ` offset_kept=<0|1>`, as
`contracts/resource-targets.md`, "Resource weights (v2)", states. Since SS-1
(0.12.30) a candidate with a base term on any other instance carries the same
fields with its base term `R` (`target_id=*`, `mode=adjust`, ` offset_kept=1`),
unless a v1 target applies to it, which keeps `target=<s>` ("Base valuation on
every instance").

Bounds, every truncation marked: a chain of `L` reasons gains `rank_breakdown`,
`decision_record` and at most `min(8, 128 - L - 2)` candidates when
`128 - L >= 2`, otherwise nothing, and `rank_assigned`'s detail then ends with
`; decision_record omitted: reason chain at <L>/128`. A detail over 1024 bytes
is cut at a character boundary to at most 1010 bytes and ends with
`…[truncated]`. Codes carry no whitespace.

The record is written only where the chain already is: the `reasons` of the
policy dispatch events of an admission attempt (`policy.dispatch_intent`, then
`policy.dispatch_admitted` or `policy.dispatch_rejected`, and
`policy.dispatch_completed`). An evaluation round is never recorded on its own,
so a poll that dispatches nothing records nothing. Ranks, states, intents
(decision identity and prerequisites), wake times and selections are unchanged
by it.

## Why A Candidate Does Not Run

Workflow #386 (scheduling model v1.4.2 §8; slice SR-6a). Every decision that is not selected
carries at least one reason, and its last reason is a registered outcome code
(the outcome catalog `contracts/outcome-codes/`, the policy fragment for the evaluator's own codes): that is the
code `why` names in a dispatch decision record, and it is never `none`. An evaluator path that
would end a decision without a reason is a programming error: the candidate is still deferred,
with the reason `decision_reason_missing` (error; `task_id`, `instance_id`), and the round goes
on, so the gap is visible instead of silent. The order in which the deferrals of the scheduling
rules are decided, and so which code a deferred candidate carries, is in
[`scheduling-blackout.md`](../scheduling-blackout.md), "Order of the deferrals". Its last
position (Workflow #405 D1) is the random delay: on an instance whose
`scheduling.random_delay.enabled` is true in effect, a candidate that passed every other gate is
deferred with `dispatch_random_delay_waiting` (info; `instance_id` the alias, `task_id`,
`due_at_unix_ms`, `random_delay_ms`, `next_eligible_unix_ms` the planned start) until its drawn
delay has passed, and the intent it is then dispatched with records the delay
(`policy.dispatch_intent` `random_delay`). Evaluations
stay out of the ledger; a person sees the reason where the next paragraph says.

**Where a person sees it** (Workflow #386 SR-6b, MCP-19a; [`v012-register.md`](../v012-register.md)
§1.143). The policy host keeps the latest evaluated cycle's not-run explanation in memory, per
instance that has a decision in that evaluation, by alias:
- the instance's earliest decision wake;
- the decision selected for it: its task, its decision id and the state of its dispatch when the
  explanation is read (`pending` before its intent is recorded, then `intent`, `admitted`,
  `rejected` or `completed`, as the dispatch projection holds it; a rejected one did not run
  either);
- every other decision as a candidate row `{task_id, state, eligibility, why, reasons,
  reasons_omitted?, next_wake_unix_ms?, since_unix_ms, expected_runtime_ms,
  expected_runtime_source}` (the last two SCH-M ED-2: the duration the pair's dispatch would
  reserve in that evaluation, its expected duration E or its declared duration, and `runs:<n>` or
  `declared`; "Settlement Feedback And Eligibility Age"): `state` is `blocked` for a blocked
  decision and `deferred` for every other (an eligible one that another task outranked waited
  this round); `why` is the full outcome of its last reason, the code `why=` names in a dispatch
  decision record; `reasons` are the codes of its reasons in order, the last 32 when there are
  more (`reasons_omitted` counts the others), so it ends with `why`'s code; the wake is the
  decision's own. Due rows (eligibility `true`) come first, ranked ones by total score, highest
  first, then unranked ones, each tie by task id; then the rest by their wake, earliest first and
  none last, then by task id;
- one not-running row (scheduling model §8.2 item 4): the first due row's task, `why`, wake and
  since; with no due row, no task and `why` `due_task_missing` (info; `instance_id` the alias,
  `next_eligible_unix_ms` the instance's earliest wake when one is known).

A row's `since_unix_ms` is the first evaluation of an unbroken run of evaluations in which the
same (instance, task) had the same `why` code; the not-running row's, of the same (instance, task
or none, `why` code). Every evaluated cycle replaces the table, before the host adds its notes to
the decisions (so `eligible_since_unknown` and `policy_instance_fact_missing` below are not part
of it); a deferred cycle or a failed evaluation leaves it as it was, and a start begins with none,
so a run counts from the first evaluation after the start. Nothing of it is written to the ledger
and no decision reads it. The general read `ScheduleExplanation`
([`runtime-interface.md`](../runtime-interface.md), "New request kinds") answers it; its outputs
are MCP-19b's: `actingctl schedule`, MCP `ac_schedule`, the `not_running` row of `ac_overview` and
the `not_run` rows of `ac_diagnose`.

(Workflow #438 HELD-1, since 0.12.56) A row that keeps one `why` code of the held set past a
configured time leaves one warning record, and its end one info record, while the table itself
stays out of the ledger ("Held records" below). From HELD-1, `fact_refresh_pending`,
`fact_refresh_unresolved` and `fact_refresh_exhausted` that name the same `fact_key` count as one
`why` code for `since_unix_ms`: a row that moves between them keeps its start ("Held records",
"The clock").

What the ledger keeps (Workflow #411 A26, since 0.12.5): only a round that dispatches records
anything, in the reasons of its dispatch events ("Dispatch Decision Record" above). There each
candidate the record lists carries `why` (that code), `decision_state` and `eligibility` as values
of `decision_candidate_not_selected`, and `decision_record` counts the candidates it does not
list by `why` in its value `omitted_why`. Before A26, `why` was only in the developer detail,
which is not persisted.

**The change-only history** (Workflow 114-A, since 0.12.36; [`v012-register.md`](../v012-register.md)
§1.236; ledger revision 5; Alice's 114, method 2). The record `policy.not_run_changed`
([`ledger-observer.md`](../ledger-observer.md) §11.8) will keep, per instance, only what changed
between evaluated rounds of this table: a change set of the rows whose change key changed, the
instance's control block (its player hold, its own pause, the global pause and its resource
fence, with `effective`, the first of them in the one order of the control gates, by R-7a §2.6
([`player-hold.md`](../player-hold.md)), never restated here), the not-running row, and how a
player hold ended; a baseline of the whole table at a start, a catalog or binding change and on
its budget's terms; a marker when the instance's hourly budget is spent; a retirement when the
alias goes. A row carries the same round's `why`, `state`, `eligibility`, `since_unix_ms` and
wake as this table's, with the layer and class the writer's golden table gave the code, read as
written. 114-A freezes the shapes, the index keys and every vocabulary of the record and of the
read `ScheduleHistory`, and writes nothing: until 114-B, this table is still not written to the
ledger.

**A member that waits for another member of its device** (Workflow #437 MAPP; MAPP-1 builds the
evaluator's one pick per device and `selected`, register §1.328; MAPP-1b builds `held`,
`player_hold` and `fenced`, which the host's gates give, register §1.330;
[device-group.md](../device-group.md), "One pick per device per round"). In a device group of
several, a candidate of a member that does not get the device because another member has it
waits with `device_group_waiting` (registered; info; `instance_id` the alias, `alias` the member
that has the device, `device_group_wait` `selected`, `held`, `player_hold` or `fenced`, optional
`task_id` and `holder_kind`) and no wake of its own. This takes the place of
`instance_already_selected`, `dispatch_instance_held` or a control gate's code, which a candidate
of a lone instance gets; with `selected` the candidate has exactly the state, the eligibility and
the wake of one behind its own instance's winner (`instance_already_selected`), and only the code
and its values differ. A member's own holder or gate keeps its own code: its own control gates
come first, then its own holder; then another member's player hold (`player_hold`), another
member's fence (`fenced`) and another member's holder (`held`), before the device's takeover
cooldown. The policy admission refuses a dispatch of the member with the same code and values, at
info. It is the `why` of the member's not-running row while it waits. In a group of one it never
occurs.
After a failed switch (`application_switch_failed`, registered by MAPP-2a; no run is settled with
it before MAPP-2b) the member's own candidates wait out
its switch cooldown, `scheduling.device_group.switch_failure_cooldown_ms` (proposed, default
`300000`): each is deferred as during a takeover cooldown, with `dispatch_instance_cooldown`, the
cooldown's end as `next_eligible_unix_ms` and its wake, and the proposed optional value
`failure_code` `application_switch_failed`, while the device's other members go on
([device-group.md](../device-group.md), "The switch before a run").

Besides the deferrals, one reason ends a decision after admission: a dispatch that was admitted
but has not started is withdrawn when its task package is unloaded, and its decision record and
reason chain end with `pack_registry_dispatch_withdrawn` (info; `package_id`, optional
`request_id`, `instance_id`; Workflow #389 U-3, [`runtime-interface.md`](../runtime-interface.md),
"Unloading a package in use"). It is not a deferral, so it has no place in the order above.

**Status.** Written ahead of its code (documentation slice SR-0) and in force since SR-6a
(v0.12.0). A reason counts as registered when its spelling is an active code of the outcome
catalog; the evaluator's existing reason spellings are registered by the outcome-code typing of
the policy crate (Workflow #378 A2d).

The evaluator gives `decision_reason_missing` as it closes the round, before it writes any
dispatch decision record. A decision left eligible becomes deferred. So `why`, and the codes
`omitted_why` counts, never name `none`; `omitted_why=none` still means that no candidate was
omitted. `why` is always the last reason the evaluator wrote. The host can add reasons to a
cycle's decisions after the evaluation: `eligible_since_unknown` ("Settlement Feedback And
Eligibility Age" below) and `policy_instance_fact_missing` (value `fact_key`)
([`instance-fact-store.md`](../instance-fact-store.md), "Explanation"). These follow the
evaluator's reasons and do not replace the reason that settled the decision.

A `fact_time_reached` predicate that is false adds its own reason before `trigger_false` when it
decides the trigger (Workflow #405, "Fact-time wake" below): `fact_time_pending` (info;
`fact_key`, `next_eligible_unix_ms`) while the time its fact holds plus its offset has not come,
and `fact_time_unavailable` (info; `fact_key`, `fact_time_problem`) when its fact gives no time.

An edge member that makes its predicate false adds its own reason the same way (SCH-M CF-1,
"Condition edges" below): `dependency_already_served` (info; `task_id`, `predecessor_task_id`)
for `dependency_completed` with `since_own_run`, and `fact_time_served` (info; `fact_key`,
`served_by_dispatch_unix_ms`) for `fact_time_reached` with `once_per_reading`.

A `fact` predicate whose value is a fact reference adds `fact_reference_unavailable` (info;
`fact_key` the referenced fact, `unknown_reason`) the same way when the referenced fact gives no
integer (#386 CF-4, "Fact references" below), and so does a `resource_projection` whose value is
one (#386 CF-5, "Pool fact references" below).

A `fact` predicate with `on_unknown: probe` whose fact is unknown adds `fact_probe` (info;
`fact_key`, `unknown_reason`; SS-5, "Self-read fact probe" below): without
`served_by_dispatch_unix_ms` the predicate is true and the task is admitted to read the fact; with
it the predicate is false and, when it decides the trigger, `trigger_false` follows.

## Settlement Feedback And Eligibility Age

In the Runtime (Workflow #308 slice 5b) every `TaskRuntimeSnapshot` the host
projects carries what earlier runs of that task on that instance settled:

- `last_duration_ms` is the duration of the latest policy run completed at or
  before the input ledger position (`policy.dispatch_completed`): its
  `runtime_ms` when it succeeded, `observed_at_unix_ms - admitted_at_unix_ms`
  of its execution record when it failed (0 when a reconciled settlement
  precedes its admission). The score stage uses it as the utility cost:
  `cost_ms = last_duration_ms` when above zero, else the declared
  `expected_duration_ms`, and the `score_computed` detail prints that `cost_ms`.
  When the snapshot carries the expected duration E (below), the cost is E.
- `expected_runtime_ms` and `expected_runtime_runs` (SCH-M slice ED-1, frozen model v1.0 §3.4
  D-12, D-15) carry the expected duration E of the pair and how many samples it is the mean of:
  E = min(⌈mean of the latest `runs` successful scheduled runs of the task's package on the
  instance⌉ + `margin_ms`, the task's `loop_budget.max_runtime_ms`; SS-3, since 0.12.30: no
  ceiling when the V2 task does not write it), with the instance's
  `scheduling.expected_runtime.*` in effect (`actingcommand_policy::expected_runtime`). With no
  sample, or with `enabled` false, E is the declared value and both members are absent, so the
  evaluation input and every evaluation are byte for byte those without samples. When present, E
  (at most `max_runtime_ms`) replaces the declared `expected_duration_ms` as the candidate's cost,
  as the dispatch intent's `expected_duration_ms` (hence the budget reservations, forward
  planning and the random-delay cut) and in the strategy's production estimate. Hard deadlines,
  lease TTLs and `loop_budget.max_runtime_ms` never use E. SCH-M slice ED-2: the host fills the
  members in every authoritative projection from the instance's `health.run_duration` record
  ([instance-fact-store.md](../instance-fact-store.md), "Run duration facts"): the samples are the
  durations of the instance's successful scheduled runs of the task's package, written at their
  settlement; tasks that share a package share them. A fact refresh read's declared duration is
  `scheduling.fact_refresh.expected_duration_ms`, and its E reads the samples of the instance's
  recognition maintenance package.
  - **Admission** compares the intent's `expected_duration_ms` with the duration the admission's
    own projection gives the pair: with E there, a different value is `policy_facts_stale` (a
    sample landed after the evaluation, and the next cycle evaluates again); without, the declared
    duration must match as before (`policy_intent_catalog_mismatch`).
  - **The dispatch intent record** (`policy.dispatch_intent`) carries `expected_duration_ms` when
    the duration it reserved is not its task spec's declared one (E, or a configured fact refresh
    duration); only an intent carries it (`invalid_policy_payload`, field
    `expected_duration_ms`), never 0. Recovery and replay reserve that value (the coordinator's
    ruling of 10-24 14:1x), so a receipt reserved with E replays without
    `policy_budget_receipt_mismatch`; an intent without it reserved its spec's declared duration.
    An immediate retry round keeps the declared duration it opened under, which a later catalog's
    task must still declare.
  - **Not-run explanation**: every candidate row of `ScheduleExplanation` (`ac_schedule`,
    `actingctl schedule`) carries `expected_runtime_ms`, the duration the pair's dispatch would
    reserve in that evaluation, and `expected_runtime_source`, `declared` or `runs:<n>`.
  - With `scheduling.expected_runtime.enabled` false for an instance nothing is written for it and
    nothing is projected: its evaluations, intents and records are those without samples. This is
    the rollback without a restart (`ConfigureSettings`).
- `failure_streak` is the number of consecutive failed runs ending at that
  latest completed run, 0 after a success. The evaluator validates and carries
  it; the score stage does not consume it yet.
- Both are absent while no run of the pair has completed. They are
  position-exact, read from the same policy dispatch records as the four
  settlement facts of `contracts/runtime-fact-store.md` (`task.<task_id>.<name>`
  for `last_duration_ms`, `last_outcome`, `failure_streak` and
  `completed_at_unix_ms`), and saturate at the evaluator's input bounds
  (86,400,000 ms and 10,000), while the facts keep the exact values.
- **A stopped run** (Workflow #429 R-6b, since 0.12.55; [../policy-suspension.md](../policy-suspension.md),
  "A stopped run") settles as neither a success nor a failure: its pair's snapshot shows it as
  `terminal_state` `cancelled` with its `completed_window`, and `last_duration_ms` and
  `failure_streak` stay those of the pair's latest success or failure, so a `dependency_completed`
  is true after it only where its `terminal_states` names `cancelled`.
- **A clock occurrence left owed** (#477 CK-2, since 0.12.81; [v012-register.md](../v012-register.md)
  §1.516; "Clock Sources"): when the pair's latest dispatch settled through the input position as
  a failure of a task that is not sensitive, interrupted (`policy_settlement_interrupted`) or with
  a disposition that paused the pair, whatever its class (not a run over its runtime budget that
  reported its success), and the evaluation instant is still in the activity day that dispatch's
  admission was counted in (a projection without an instant counts as in it), the snapshot carries
  `clock_owed: true`, and a clock then reads the pair as never dispatched: every occurrence is
  unserved, the latest one due. Nothing else reads it. Every other snapshot leaves `clock_owed`
  out and is byte for byte as before, so a ledger without such a dispatch projects the same
  snapshots, `fact_snapshot_id` and decision identities; where it is present, `fact_snapshot_id`
  hashes it.

`eligible_since_unix_ms` is the eligibility age the evaluator turns into
`aging_ms = now - eligible_since_unix_ms`. The host keeps it in memory only
(iron rule 13), one instant per (task, instance):

- After every evaluated cycle, a pair whose decision is `eligible`, `selected`
  or deferred with `score_deferred`, and that has no dispatch in flight, keeps
  its instant or starts one at the cycle's evaluation instant. A pair deferred
  for another reason (cooldown, Runtime availability) or with a dispatch in
  flight keeps an existing instant but starts none. Every other pair (blocked,
  not eligible, or not evaluated) loses its instant. Admitting a pair's intent
  ends its instant at once.
- A cycle's verdicts take effect when the next evaluation starts, before its
  inputs are projected, so an evaluation and the admission of its intents
  project the same ages. An instant is reported only to an evaluation instant
  strictly after it (a zero age reads as absent), and admission projects as of
  its intent's evaluation instant. Projections bound to a ledger position alone
  (`ProjectPolicyInputIdentity`, strategic reports) report no age.
- `fact_snapshot_id` hashes the projected task snapshots, so each of the three
  fields is part of the snapshot identity and of the decision identity derived
  from it. Admission compares it per instance (`contracts/instance-fact-store.md`,
  "Admission staleness per instance"): a run settled on another instance earlier
  in the same cycle leaves an intent fresh.
- Nothing about ages reaches the ledger: evaluations are not ledger events, so
  a restarted host cannot rebuild them. It starts with no ages; the first cycle
  evaluated after a start adds `eligible_since_unknown` to every decision whose
  pair starts aging there, and those ages count from that cycle.

## Settlement Outcome Records

In the Runtime (Workflow #313 goal 5) every settled policy run that projects its
mapped outcome record projects three `ObservedOutcome` records for its task and
instance:

- `<outcome_key>`: `Boolean(true)`, as before; predicate semantics are unchanged.
- `<outcome_key>.duration_ms`: `Integer` milliseconds, the same value slice 5b
  records as the `last_duration_ms` settlement fact, i.e. the run's `runtime_ms`.
  A failed run projects no outcome record at all (an outcome residual for it
  remains the fatal `policy_outcome_failed_run_residual`), so the failure form
  of that duration never reaches these records.
- `<outcome_key>.completed_at_unix_ms`: `Integer`, the run's terminal timestamp.

The three share `observed_at_unix_ms` (the terminal timestamp),
`activity_window_id` and `expires_at_unix_ms` (none). A value beyond the `i64`
range fails with the fatal `policy_settlement_outcome_overflow`. Because the
three share observation, window and expiry, the window-result and feedback-stop
checks reach the same result as with the mapped record alone. Remedies (Workflow #411
B15, [remedies.md](../remedies.md)) read a settled successful run's terminal
`scheduling_disposition.outcome_key` whatever the catalog references, and leave these records
unchanged. The derived keys
obey the evaluator's input bounds: 128 bytes per key and 16,384 outcome
records per evaluation, now three per settled pair. An outcome key may
therefore be at most 107 bytes (`<outcome_key>.completed_at_unix_ms` within
128): catalog activation, rollback and promotion refuse a catalog that
references a longer outcome key for any of its tasks with
`outcome_key_too_long_for_settlement` (request class) before anything is
appended, next to the task id check `task_id_too_long_for_facts`. `fact_snapshot_id` hashes the projected
outcomes, so it differs from before for any input with a settled run, and the
decision identities derived from it follow; this is not a wire change. The
policy input structures, the ledger event set and the decision identity rules
are unchanged.

**Outcomes a successful run did not produce** (Workflow #390 OUT-1, forced
compromise O-04). Admission refuses a catalog that references an outcome key
the task's package does not declare (Runtime Boundary), so the package declared
every key the run's own catalog referenced for the task. For the pair's latest
run, when it succeeded, each of those keys that the run did not produce and the
active catalog still references projects one more `ObservedOutcome`:
`Boolean(false)`, with the run's observation instant, activity window and
expiry. An outcome predicate on such a key evaluates `false`, not Unknown. A
failed or cancelled run, a pair without a completed run, and a key the run's
own catalog did not reference stay Unknown (`outcome_missing`). When the active
catalog does not reference the produced key, the produced key's three records
are left out; the pair still projects its `false` records, and projects nothing
when it has none (Workflow #400). These records count against the same bound of
16,384 outcome records per evaluation.

A strategic goal's `MetricRef::Outcome` reads the record with its exact key for
the assessment's instance. As for `MetricRef::Fact`, an `Integer` value is the
metric, any other value is none, and a record whose `expires_at_unix_ms` is
before the report's `as_of_unix_ms` is none. Naming
`<outcome_key>.duration_ms` or `<outcome_key>.completed_at_unix_ms` reads the
settled duration or completion time.

Before it projects a completed run, the host confirms from the ledger that the
run's admission request is the run's only `policy.dispatch_admitted` up to its
terminal (Workflow #317 item F). The confirmation is cached in memory only,
keyed by the run's `(decision_id, completed_sequence)` and held once per
(task, instance) pair: a repeated cycle that projects the same run through the
same terminal does not read the ledger again, while a different key (a newer
run of the pair), a different run identity or terminal, or a restarted host
confirms again. A failed confirmation is not cached and keeps its error. The
failed-run residual check, the run and outcome identity match and the outcome
re-projection still run on every cycle.

## Runtime Enforcement

Time validity is evaluated over the pinned input snapshot. Timeline invalidation
matches an observation's key prefix and scope (the same scope or a containing
server/game scope). Outcome keys are `outcome.<task_id>.<outcome_key>`; settled
execution state without mapped outcomes uses `task.<task_id>.terminal_state`.
Only schedule occurrences inside the event's V2 half-open validity interval
invalidate observations. V1 occurrences are unbounded. Duration controls
availability, including zero-duration resets, and does not undo invalidation.
An observation at or before the last matching reset is Unknown until replaced
by a newer observation. Equal timestamps are conservative because the input
contains no ordering evidence within that millisecond. A V2 event may name key
prefixes it does not invalidate, and a reset never clears a task's own last
result for the task's own edges ("Reset exclusions and the task's own last
result" below).

A fact or outcome whose observed value kind differs from the kind its predicate
compares (`fact`, `outcome`), a `record_deadline` fact that is not a
`record_list`, or a record whose timestamp field is not `timestamp_ms` resolves
that predicate Unknown with reason `type_mismatch`; a record missing its
timestamp field resolves it Unknown with `field_missing` (the selection-policy
words; Workflow #355 D12). Neither fails the evaluation: the other tasks and
instances evaluate and dispatch normally, and the reason surfaces as the
planning signal whose outcome is `planning_fact_unknown` with the values
`fact_key` and `unknown_reason` (`type_mismatch` or `field_missing`). Each
new fact snapshot reserves one more detection for that instance's activity
window when that instance has no other dispatch in the cycle and its activity
window is open (as for `fact_missing`), so an unrepaired pack type error ends as
one detection-quota-exhausted signal per window. Errors of the catalog's own
comparison (an operator on kinds it does not accept, such as `contains` on two
integers) and arithmetic overflow still fail the evaluation; Runtime reports
them as `policy_evaluation_rejected` and keeps the policy code and message as
the record's native detail. Since 0.12.28 (ISO-PE, #419) such a failure no
longer stops the Runtime: the evaluation is skipped and evaluated again later
("Evaluation failures" below).

`ObservedOutcome.expires_at_unix_ms` is optional and uses the fact TTL convention:
the expiry millisecond is valid and expiry + 1 wakes reevaluation. Matching future
resets cap admission freshness at occurrence - 1 and wake at the occurrence.
Ledger-derived pools consume the same effective fact projection.

The first dispatch depends on `trigger`. `feedback_stop` is evaluated only after
a valid result for the same task and instance in the current activity window.
`ObservedOutcome.activity_window_id` comes from the settled run's admission;
`TaskRuntimeSnapshot.completed_window` carries the settled window ID and completion
timestamp for tasks with no mapped outcome consumer. Window identity follows the
selected activity profile and its first active declared window, including its
local start day for windows crossing midnight. A result must both name that
window and have completed inside it. An enabled stop predicate retains Unknown
when its required observation is unavailable. Missing historical window fields
default to `None`, which supplies no evidence for feedback stop; a missing outcome
expiry supplies no additional TTL. An enabled feedback stop's activity-window
closure also bounds freshness and wakes reevaluation. These input projections add no ledger wire or stored
transaction state.

The evaluator pins the selected activity profile in every dispatch intent. Runtime owns activity sampling, budget counters, retry state, and failure escalation; callers cannot supply remaining-budget values. Admission and execution ledger events record the selected profile, sample seed, activity window, cadence, cumulative task and activity budget receipts, and classified outcome. The admission record's activity sample names its window by `window_id` and `local_day` and, since 0.12.86 (#503 CYC-3; [v012-register.md](../v012-register.md) §1.529, `ledger` revision 10), by its end, `window_until_unix_ms`, taken from the one resolution of the window the admission makes; a record written before carries none.

Recoverable failures receive a positive, bounded backoff. Only repeated failures with the same failure code, failure identity and failure class share a consecutive-failure streak, and sensitive or severe failures are never automatically restarted. A failure records its `outcome` (whose category is the record's severity) and, for a scheduled `linear_steps` or (Workflow #390 EXEC-3) page-graph task, its `failure_identity`, which is rerun at once and lifted by a package update (`../policy-suspension.md`). Goal-missed, feasibility-red, and drift-predicted signals are informational planning facts: they do not consume failure tax, advance a failure streak, or pause execution. A run a `StopInstanceRun` stopped (Workflow #429 R-6b, since 0.12.55; [../policy-suspension.md](../policy-suspension.md), "A stopped run") receives none of the failure handling above: no backoff, streak, escalation, pause or ladder, no condition and no owed rerun, and the Runtime's own reruns its dispatch served end with it (since 0.12.55). A scheduled run that failed at its entry (Workflow #490 ENTRY-A, since 0.12.77; register §1.494; [../policy-suspension.md](../policy-suspension.md), "A failure at the entry": its main package executed no step, its failure came from the gate, a prerequisite or return-home layer or the home entry's check, does not accumulate, its code's class is `pack_logic_mismatch` whatever the ladder's `exclude_codes` say of it, and its instance's block has a recovery for it; a code of any other class, the capture backend's, the device link's or a fence's, is settled as before) is a condition of its instance, not a failure of its task: no backoff, streak, escalation, pause or immediate rerun, the counts its admission took given back (so a task's runs of the day are spent only by runs whose entry was reached), and no owed rerun; its instance is blocked until a recovery or a run of it succeeds, a resume lifts a pause of it, a blackout covers it at the lease sweep's tick, a fence takes it or the Runtime restarts ([../scheduling-blackout.md](../scheduling-blackout.md), "Order of the deferrals", position 4b, `instance_entry_blocked`; a store update package's task passes; a recovery the block held when it ends is dropped with its trigger's `recovery_ladder_suppressed` `cooldown` once more). The block's recovery is admitted, held through a cool-down or a transient refusal and tried again (a claim the queue refused on a growing interval, one second to one minute); it climbs from the first rung that restarts the application when no run was let in on the instance since its previous block ended; the first climb of a stretch of blocks with nothing let in between them starts as before, and every later one waits `scheduling.unavailable_probe`'s interval for the stretch's climbs from the previous climb's end, never before the cool-down window ends, whatever ended the previous climb (but a person's pause, under which nothing climbs and whose lifting ends the block); one that ends `exhausted` tells a person once per block and is followed by such a later climb; one the game's update dialog ended tells a person once and nothing climbs again in the stretch (the store update task passes); and a stretch that lasted `scheduling.unavailable_probe.attention_after_ms` is brought to a person once. Fix round 4 ("A pair that keeps failing at its entry"): no failure at the entry counts against its rule; a pair (instance, catalog task) that fails at its entry n times in a row in an activity window is not dispatched on that instance before `scheduling.unavailable_probe`'s interval for n has passed since the end of the block the latest failure set and of any later block of the instance (since the latest settlement until a block ends; `instance_entry_task_waiting`), and once its failures that came although another scheduled run was let in reach its rule's `escalation_threshold` it is set aside until the window's end (`instance_entry_task_set_aside`) and a person is told once (`instance_entry_task_set_aside_reported`); the instance's other tasks run meanwhile, first when a block ends, a store update task's success starts every pair of its instance from nothing, and a catalogue that becomes active starts every pair from nothing. The emulator is restarted at most once in a stretch of blocks. No rule is paused by failures at its entry, however long a condition lasts. A failure in a step of the main package is settled, rerun, counted and paused as before.

### Optional run-count bounds

SS-3 (since 0.12.30; Alice's ruling 100 on Workflow #386: dispatch follows live account state, and there is no "run task X N times"; register §1.212). In an `actingcommand.scheduling.v2` catalog a task's `loop_budget` may be left out, and so may each of its three items, `daily_limit`, `window_iteration_limit` and `max_runtime_ms`, on its own; so may each of an activity profile's `daily_budget`, `max_window_iterations` and `session_max_ms`. V1 keeps all six required (`missing_required_field` at the member). In V1 and V2 a bound that is written is a value: an explicit `null` is refused (`type_mismatch`), as the V2 schemas declare and as for an effect's `amount`; it is never read as "not written". A bound that is written applies exactly as before, and must still be above 0 (`loop_budget_missing`); a catalog that writes all of them serializes, hashes, admits, settles and records byte for byte as before. A bound that is not written is no dimension:

- **Admission** reserves nothing for it and never refuses on it (`policy_budget_exhausted` names only a written dimension). The dispatch intent carries only the written items in `prerequisites` (`daily_limit`, `window_iteration_limit`, `max_runtime_ms`), and the intent check requires each one it carries to be above 0 and the intent's expected duration to be within a carried `max_runtime_ms`.
- **The budget receipt** of `policy.dispatch_admitted` and `policy.dispatch_completed` carries neither member of an unwritten dimension (`task_daily_used`/`task_daily_limit`, `task_window_used`/`task_window_limit`, `task_runtime_reserved_ms`/`task_runtime_limit_ms` and the three activity pairs); a carried dimension holds both members with the count between 1 and the limit, as before, and a receipt with one member of a pair is invalid. The receipt is part of the admission record those two events carry (and the retry round of `policy.control_state_recorded` keeps), and the global ledger query's public projection hands it on unchanged as `PublicPayload.policy_admission`, which reaches clients through runtime-client (since 0.12.86 without its activity sample's `window_until_unix_ms`, #503 CYC-3, so that projection keeps its bytes; the full record carries it). The Runtime keeps no count for an unwritten dimension, and a condition rerun's refund gives back only the counts the admission took. Workflow #490 ENTRY-A (since 0.12.77): a run that failed at its entry gives back its four counts the same way, whatever `scheduling.rerun.refund_counts` says; budgets are still taken at admission, and an immediate rerun of a step failure still costs one.
- **The retry round** does not judge an unwritten dimension; retries stay bounded by `on_failure` and the round's window.
- **Settlement** never rewrites a run to `policy_runtime_budget_exceeded` on an unwritten runtime dimension: without `max_runtime_ms` (and `session_max_ms`) a window's cumulative runtime has no bound. The per-run deadline (the contained-task response bound and the package's own timeout) is unchanged, and so are failure retries and escalation, the per-occurrence rerun limit, the recovery ladder, in-pack loop caps and concurrency.
- **Expected duration E** has no ceiling without `max_runtime_ms` ("Settlement Feedback And Eligibility Age").
- **Workflow #411 B8** hands a task that opts in to `dispatch_parameters` `daily_remaining` and `last_daily_slot` only from the daily dimensions written (the task's `daily_limit`, the profile's `daily_budget`, the fewer of those written); with neither written both are absent (`../remedies.md`, section 4).
- **The strategy report's production estimate** takes the fewest runs an hour of the bounds written (`daily_limit`, `window_iteration_limit`, `max_runtime_ms` / E) and the period bound (one hour over the task's cycle), which is always present.

**Readers before 0.12.30.** A receipt that leaves members out is written only after the resource side first leaves a bound out; until then every receipt is written as before, and every build reads it. From then on a reader older than 0.12.30, which requires all twelve members, cannot decode it:

- **A Runtime before 0.12.30** refuses the ledger with `corrupt_ledger_record`: at open, when such a record lies among the newest `ledger_observer.open_verify_tail_records` rows it verifies (`../ledger-store.md`), and otherwise later, when the background recheck reaches the record, latches a Global failure and stops the Runtime with the same code (`../ledger-observer.md`). It also refuses a V2 catalog that leaves `loop_budget` out.
- **Tools and the UI before 0.12.30** fail to decode a ledger event query page (`RuntimeEventQueryPage`) that carries such a receipt in `PublicPayload.policy_admission`.

No ledger revision is bumped for this (`ledger` stays [3, 4]); the bump comes with SS-9, which deletes the members. After the resource side first leaves a bound out, no rollback below 0.12.30 is possible, so the resource side leaves bounds out only once every deployment runs 0.12.30 or later (the 0.12.30 release notes and Workflow #428 say both).

Deleting the members (SS-9), the no-progress guard (SS-4) and batch allowances (SS-6) are later slices.

## Task Execution Fields

`entrypoint.operation_id` names the Runtime mechanism capability that may execute the task. `procedure_ref` is the immutable external package or procedure-definition identity pinned into the dispatch intent and the immutable catalog generation referenced by its reason chain. It is not a file path, script, executable capability, or approval authority; Runtime must resolve it through the approved adapter/package boundary and reject any mismatch at admission.

Before an intent becomes trusted, a Runtime-owned manifest content-addressably binds `procedure_ref`, the package SHA-256 digest, `operation_id`, and the ordered `yield_points`. The binding digest participates in decision identity and is persisted with dispatch events. Admission resolves the alias again and requires the exact package and binding digests before requesting a lease.

`expected_duration_ms` is the declared reservation and planning duration D; a pair whose snapshot carries the expected duration E (SCH-M ED-1, "Settlement Feedback And Eligibility Age") uses E instead. `cooldown_ms` is the minimum interval after the last dispatch before the task can become eligible again. `next_run_clamp_ms` caps recoverable retry delay. `yield_points` names the only package-declared safe cooperation points that may be exposed to a mechanism adapter; it never grants a new operation. `sensitive` disables automatic restart after failure and does not weaken fatal-error propagation.

## Clock Sources

Every clock schedule declares exactly one source:

- `local` uses the host-provided monotonic coordinate and is valid only for interval schedules. The evaluator projects its next occurrence back to Unix time for transport.
- `server` uses a pinned timezone identity, base UTC offset, explicit DST offset, and bounded maintenance drift.
- `reveal` has the same calendar fields plus `reveal_source`, the immutable evidence identity from which the catalog author derived the pinned schedule.

The calendar kinds reach their local time from the source alone: the evaluation's Unix time less `maintenance_drift_ms`, plus `utc_offset_minutes` and `dst_offset_minutes`; an occurrence goes back to Unix time by the same offsets and the drift. `daily` occurs at each of its `minutes_of_day` of every local day. `weekly` occurs at minute `minute_of_day` (0 to 1439) of its `weekday` (1 Monday to 7 Sunday) of every local week.

`monthly` (#477 CK-1, since 0.12.83; [v012-register.md](../v012-register.md) §1.510), `{"kind": "monthly", "clock_source", "day_of_month", "minute_of_day"}`, occurs at minute `minute_of_day` (0 to 1439) of day `day_of_month` of every month of the Gregorian calendar (a leap year every fourth year, but not in a century year not divisible by 400), on the local day the weekly kind reaches by the same offsets; the policy crate computes the calendar itself, with no timezone database and no date library. `day_of_month` is 1 to 28, a day every month has, so no month lacks its occurrence: 0, 29, 30 and 31 are refused as an out-of-range `weekday` or `minute_of_day` of the weekly kind is, `predicate_uncomputable` at the clock ("clock schedule is outside the frozen V1 bounds"), and so is minute 1440. In every other respect a monthly clock is a weekly one: in V1 and V2 documents, a task's `clock` (its due and served occurrences, CK-2's owed occurrence below, `missed`, the clock-slot records, the next wake) and a timeline event's `schedule` (its window, its resets, a maintenance blackout) alike, and `local` is refused for it. A standard pack's `schedule.json` that writes one names `clock_monthly` in `requires` ([../standard-pack-schedule.md](../standard-pack-schedule.md), "`requires`"). A catalog document has no `requires`: a build before 0.12.83 refuses one that writes the kind as its reader refuses any unknown kind (`type_mismatch`, the unknown variant `monthly`), the whole catalog with it, and never reads it as another kind. A catalog without the kind serializes, and hashes, exactly as before.

A clock occurrence of a clock without `missed` is owed from the moment it occurs until the task is next dispatched; it has no expiry (Workflow #361). The latest dispatch admitted at or after it serves it, running or settled, except in two cases of a task that is not `sensitive`, in which it stays owed (#477 CK-2, since 0.12.81; [v012-register.md](../v012-register.md) §1.516): a dispatch that settled as interrupted (`policy_settlement_interrupted`), whether its run had started or not (the work order's "never ran" was the coordinator's wrong word: the Runtime's end may cut a run short half way), and one whose failure paused the pair, whatever the failure's class (every `pause_cause` but `sensitive_task`, [../policy-suspension.md](../policy-suspension.md), "Why a pair is paused": a severe failure such as a screen the package never recorded, a streak at `escalation_threshold`, retries used up under `on_failure` `pause`; a pause that a package update lifts is the author's repair, after which the task should run the same day), but for a run over its runtime budget that reported its success (`reported_success`: its work was done). A `sensitive` task never comes back by this rule. The occurrence stays owed only while the evaluation is in the activity day that dispatch's admission was counted in (its activity profile's window open then belongs to that local day, or none is open); in a later activity day the occurrence it answered is served, and only what the ordinary rule owes (an occurrence after that dispatch) is due, so a task limited to one run a day never runs a day's occurrence on the next day's budget. After either, the occurrence stays owed and the pair is dispatched on it as soon as what holds it lets it through: the first interruption in a row holds nothing, so the pair runs again at the next evaluation; a second interruption in a row and a pause hold it until a package update lifts the pause ([../policy-suspension.md](../policy-suspension.md), "Lifting (R19)"), and the pair then runs on the same occurrence, not at the next one. A success, a stop, a run a condition ended, a failure scheduled for a retry or continued (`on_failure` `continue` once its retries are used up) and a sensitive task's failure serve the occurrence as before, and the Runtime's own reruns (the immediate rerun, an owed rerun) keep their rules. A task whose failure passes by itself is therefore not tried again later the same day on its occurrence: that needs a pacing of its own and is not built, and a second occurrence the same day written as a retry is still how a catalog asks for it. The task snapshot says when its latest dispatch left its occurrence owed ("Settlement Feedback And Eligibility Age", `clock_owed`), and a clock under a `not` reads it as well. Several occurrences missed in a row collapse into the latest one. An occurrence that fell while the task's activity window was closed, or while no Runtime was running, is dispatched once at the first admissible evaluation of the next window, and that dispatch counts against the loop budget of that window's day. A timeline reset invalidates the task's settled state but does not settle a missed occurrence.

A catalog that must skip a missed occurrence instead of replaying it gives the clock `"missed": {"kind": "skip_after", "grace_ms": N}` (Workflow #361 R7; `actingcommand.scheduling.v2` only, `grace_ms` from 60 000 to 604 800 000). Such a clock's latest occurrence O, not yet served (as above: since 0.12.81 also O a dispatch that settled as interrupted or whose failure paused the pair left owed), is due only while the evaluation is before `O + grace_ms`, on the schedule's own clock; from that instant the clock is false until its next occurrence, so a skipped occurrence never takes the budget of a later day. A pair that was never dispatched follows the same rule: its latest occurrence is due only inside its grace. While the occurrence is due, the clock's next wake and its freshness end at `O + grace_ms`; once it expired, the clock adds the info reason `clock_occurrence_expired` {`occurrence_unix_ms`, `grace_ms`} to the decision's reason chain (the decision stays `trigger_false` when the clock decides the trigger). The skip is recorded once as `clock_slot_expired` (Workflow #372 K4, below). `missed` is refused under a `not` (`predicate_uncomputable`), in a V1 document as an unknown member, and with a grace outside its bounds (`limit_exceeded` at `…/missed/grace_ms`); an absent `missed` leaves the catalog's serialization and hash unchanged.

A due occurrence that a gate stops, or that a dispatch serves after its activity window opened, leaves one record; so does an occurrence lost to its `missed` grace or to a later occurrence with no other record of it (Workflow #372; "Clock-slot records" below).

The catalog contains the effective DST offset instead of consulting a hidden timezone database. The base UTC offset is bounded to `[-840, 840]` minutes and the explicit DST offset is independently bounded to `[-120, 120]` minutes; their effective sum is therefore bounded to `[-960, 960]` minutes. A DST transition, server-clock correction, reveal change, or maintenance delay creates a new immutable catalog generation and triggers full recomputation. `maintenance_drift_ms` shifts nominal occurrences and is bounded to seven days. Calendar and absolute schedules cannot use `local`, because monotonic coordinates do not identify wall-clock instants and are not portable across host boot epochs.

`next_wake_unix_ms` remains the earliest lower bound across timeline and predicate wakes. When a task wake is known, `preload_hint` pairs that lower bound with the task ID, its `procedure_ref` as `package_ref`, and an explicit confidence. A preload hint is advisory only and never constitutes admission or execution permission.

### Clock-slot records

A due clock occurrence that is not dispatched leaves one trace (Workflow #372). The trace is a `policy.planning_signal_observed` record of kind `goal_missed` whose severity is its code's category (`warning` for a refused or superseded slot, `info` for a deferred, owed or expired one): like every planning signal it consumes no failure tax, advances no failure streak, pauses nothing and wakes no agent. `actingd suspended` lists the records with their settled fate (`../policy-suspension.md`), and MCP `ac_diagnose` shows up to 10 of an instance's.

- **Came due, served, owed.** An occurrence O came due for a (task, instance) pair when an evaluation ranks the pair as a candidate (its trigger is true) and O is its due occurrence. A pair whose trigger is false did not come due and leaves nothing: a follower after a failed head or after its group's reset, an occurrence outside its own `timeline_active` window, a true `feedback_stop`. O is served by the pair's first admitted dispatch after O (#477 CK-2, since 0.12.81: and owed again while the latest of them settled as interrupted or its failure paused the pair of a task that is not sensitive, in the activity day of its admission, "Clock Sources"). A selected dispatch is owed when its O lies before the start of the activity window it is selected in; it is a first run when the pair had no dispatch. A dispatch whose run a `StopInstanceRun` stopped (Workflow #429 R-6b, since 0.12.55) served its O as every admitted dispatch does but those two kinds, so the stop does not make O due again. #477 CK-2: an O a dispatch left owed is due again, and the records follow it as any due O: the pause that holds its pair is a persistent gate (`clock_slot_refused` with `policy_task_paused` at first sight), and a record of a kind already written for O is not written again, before the dispatch or after it.
- **Codes.** `clock_slot_refused` (warning): a persistent gate refused a due O. `clock_slot_deferred` (info): a transient gate held it back. `clock_slot_owed` (info): the first evaluation that selects the pair for an owed O. `clock_slot_expired` (info) and `clock_slot_superseded` (warning): an unserved O lost to its clock's `missed` grace or to a later occurrence (Workflow #372 K4, "Lost occurrences" below). A pair with a dispatch in flight gets no record.

| gate | codes | recorded as | when |
|---|---|---|---|
| persistent | `policy_budget_exhausted` (with its `budget_dimension`), `policy_activity_window_closed`, `policy_task_paused`; at placement `instance_override_disabled`, `capability_missing`, `activity_profile_missing` | `clock_slot_refused` | first sight |
| control | the control gates of R-7a §2.6 (`contracts/player-hold.md`): `dispatch_player_held` (R-7b), `dispatch_paused_instance`, `dispatch_paused_global`, `dispatch_resource_fenced` (Workflow #429 R-7a2, since 0.12.35; before it this row was "operator pause", the two pause codes, and a fenced pair was selected and refused at admission) | `clock_slot_deferred` | first sight |
| queue wait | `dispatch_instance_held`, `policy_retry_backoff_active`, `policy_activity_interval_active` and any other eligibility deferral; at placement `instance_unavailable` | `clock_slot_deferred` | only at an evaluation where the pair's activity window is closed: the wait outlasted the window |
| waiting in the round | the evaluator's own deferrals (another candidate selected, host or scene budgets, capacity, score, cooldown) | nothing | once the window closes, such a candidate meets `policy_activity_window_closed` |

- **A pair with no dispatch** gets refused and deferred records only at an evaluation where its activity window is open, or when it has no activity profile. An occurrence before that, for example from an activation at night, is covered by the owed record of the pair's first run, which carries `first_run`.
- **Once per occurrence.** The record's `signal_id` is `signal:clock_slot:<refused|deferred|owed>:<hex>`, where `<hex>` is the lowercase SHA-256 of the UTF-8 bytes `task \0 instance \0 O`, with O in decimal ASCII. It is checked against the planning-signal projection before the record is written, so each kind is written at most once per (pair, O), across restarts too. A refused record stops later deferred ones. A deferred record stops a later refused one only for `policy_activity_window_closed`, the consequence of the pause or wait, not a new cause; a later budget, task-paused or configuration refusal is still written. Within a class the first gate seen wins.
- **Outcome.** The record's `instance_id` is the instance alias and its `task_id` the task. Its `outcome` is the code with its values (Workflow #378 A2-P): `task_id` and `alias` always, then each present value; an absent value is left out, never written as `none` or `null`. Refused and deferred records carry `gate_code` and `occurrence_unix_ms`, and when known `budget_dimension`, `window_end_unix_ms` (E, the end of the activity window open at the record), `next_eligible_unix_ms` (N) and `next_occurrence_unix_ms` (X). Owed records carry `occurrence_unix_ms` and `window_start_unix_ms`, `next_occurrence_unix_ms` (X) when known, and `first_run: true` for a first run. `gate_code` is a code, `budget_dimension` a vocabulary token, the times are Unix milliseconds and `first_run` is a boolean.
- **Fate.** A refused or deferred record predicts `same_window` when E is present and N < min(E, X): served late in the same window; `next_window` when N < X and E is absent or N ≥ E: served in the next window as an owed run, which for a daily-limited task displaces that day's own slot; `collapsed` when N ≥ X: absorbed by the next occurrence; and `pending` without N (a pause, a wait, an unavailable instance, a paused task, the configuration). The pair's first admitted dispatch D after the record settles it, whatever D's end (#477 CK-2: a D that left O owed settles the record as one that served it does): `collapsed` when D ≥ X, `same_window` when D < E, `next_window` otherwise; without D, `collapsed` once X has passed. An owed record settles as `served` when D < X, and as `absorbed` when D ≥ X or, without D, once X has passed. An expired record is settled `expired` and a superseded one `collapsed`; a refused or deferred record of an occurrence that has an expired record settles as `expired` (Workflow #372 K4).
- **Fact invalidation.** Planning signals are appended without an instance link, so a clock-slot record invalidates every active fact whose `invalidate_on` lists `policy.planning_signal_observed`, on any instance.
- **Lost occurrences (Workflow #372 K4).** After the three kinds above, every evaluation reads the pairs of its task snapshots that have a dispatch history. O is unserved when the pair has no admitted dispatch at or after O and none in flight. Over the counted clocks (those reached through `all` and `any`, not under `not`; none when one is an interval under one hour), the evaluation takes the occurrences after the pair's last dispatch (#477 CK-2, since 0.12.81: from the occurrence that dispatch answered, the latest of the counted clocks at or before it, while the snapshot says it left that one owed, `clock_owed`; so an owed occurrence whose `missed` grace runs out is recorded `clock_slot_expired` once, as any unserved one, and one a later occurrence absorbs with no other record of it `clock_slot_superseded`), at or before now and within 48 h before now, at most 32, newest first; occurrences of several clocks at one instant are one. With O′ the next occurrence after O over every counted clock:
  - `clock_slot_expired` {`occurrence_unix_ms`, `grace_ms`, `next_occurrence_unix_ms` (O′) when known}: O's clock has `missed`, `O + grace_ms` ≤ now, and O′ is absent or `O + grace_ms` ≤ O′. It is written even after a refused or deferred record of O, whose fate then settles as `expired`.
  - `clock_slot_superseded` {`occurrence_unix_ms`, `next_occurrence_unix_ms` (O′)}: O′ ≤ now, O did not expire, and no refused, deferred or expired record of O exists. In a trigger that mixes clocks with and without `missed`, the successor is taken over every counted clock.
  - A record is written only while the pair is live: its task is placed on the instance, and its trigger, evaluated with the evaluation's facts once every counted clock is counted due, is true. A follower whose head failed or a task outside its timeline window gets none. A pair that was never dispatched gets none either: R7's grace bounds its first run, and K2's owed record carries `first_run`. While the Runtime runs, the first evaluation after `O + grace_ms` or O′ comes within one cycle; after a Runtime gap, liveness reads the facts at the restart.
  - Each is written once per (pair, O), its `signal_id` `signal:clock_slot:<expired|superseded>:<hex>` with K2's digest, checked against the projection before it is written.
- **Not recorded.** An occurrence a catalog skips by making its trigger false, and the occurrences of a pair that was never dispatched.

## Held records (Workflow #438 HELD-1, since 0.12.56)

Written ahead of its code (contract slice HELD-C, [v012-register.md](../v012-register.md) §1.322;
the coordinator's rulings of 11-12 and its answers of the same day to the draft's nine questions,
settled in the register, not the owner's words). Built by the first code slice,
HELD-1 (Runtime 0.12.56, [v012-register.md](../v012-register.md) §1.327): the codes, keys and
vocabularies it names are registered. The wake kind is HELD-2's ([agent-wake.md](../agent-wake.md)),
not built yet.

A due task that a reason holds for hours and one held for minutes look the same in the not-run
explanation, and an info reason never becomes more than info with time. A held record says once
that a (task, instance) pair has been held past a configured time, and one more record says when
that hold ended.

- **Nothing but a record.** A held record and its end change no decision:
  they start no ladder, pause nothing, retry nothing, count toward no failure streak or tax, and
  the pair is scheduled exactly as without them, as for every planning signal ("Runtime
  Enforcement"). They are written after the evaluation and its explanation and read by no
  decision; with the record off, every evaluation and intent is the same, and so is every other
  record of the cycle.
- **Held.** At an evaluated cycle, a candidate row of the not-run
  explanation ("Why A Candidate Does Not Run") is held when all of these hold:
  1. its task is a catalog task: the fact refresh read (`FACT_REFRESH_TASK_ID`) is never held, its
     waits are its own reasons in the candidates that wait for it;
  2. the pair has no dispatch in flight (intent or admitted), as for the clock-slot records;
  3. its `why` code is in the held set below (for a code whose class a value decides, with that
     value), so its eligibility is `true` or `unknown`: no held code comes with `false`;
  4. the instance's record is on and the evaluation time is at least its `since_unix_ms` plus the
     threshold in effect for the instance ("The threshold" below).
- **The held set.** Closed: a code outside it is never held, and a slice that
  adds a deferral code classes it here. The register's census (§1.322) gives every code a row can
  carry with its reason.
  - *Held, due* (eligibility `true`): `policy_no_progress`, `instance_unavailable`,
    `heavy_scene_budget_exhausted`, `heavy_scene_budget_deferred`,
    `policy_concurrent_runs_deferred`, `instance_already_selected`,
    `policy_chain_handover_waiting`, `dispatch_instance_held` (whatever its `holder_kind`),
    `application_update_pending`, `selection_input_unknown`, `decision_reason_missing`,
    `device_group_waiting` (whatever its `device_group_wait`; classed by Workflow #437 MAPP-1a,
    [v012-register.md](../v012-register.md) §1.328).
  - *Held, undecidable* (eligibility `unknown`): `trigger_unknown`, `feedback_stop_unknown`.
  - *Held, either* (by the row's eligibility): `fact_refresh_pending`, `fact_refresh_unresolved`,
    `fact_refresh_exhausted`, `fact_refresh_unbound`.
  - *Held by a value*: `benefit_unknown` with `benefit_gap` `reading_unknown`; `pool_line_held`
    with `pool_line_hold` `capped_not_head` or `capped_cap_unknown`, and with
    `capped_not_dispatchable` when its `gate_code` is itself held.
  - Never held: what a person, the configuration or the values said (the control gates, a
    suspension, a blackout and its quiet, the day boundary, a closed package, a remedy stopped for
    a person, a spent budget, a closed activity window, a paused pair, a placement the
    configuration refuses, a reserve, a benefit that is not positive or within its tolerance, lines
    that cannot spend); what waits for a time the decision names (a cooldown, a score deferral, a
    takeover cooldown, an interval, a backoff, a random delay, an owed rerun, a remedy round); and a
    row that is not due (`trigger_false`, `feedback_stop_true`).
  - A wait behind a run in flight (`dispatch_instance_held`, the heavy-scene and shared-limit
    waits, `instance_already_selected`, and `device_group_waiting`, the wait of a member of a
    device group for the device another member has) stays held (settled by the coordinator on
    11-12): only so is a registration left open with no time and no other record seen, and only so
    does a member that another member of its device starves for hours leave a record. A queue of short runs
    changes the code at each run and never reaches the threshold; an instance whose runs are long
    gets its own `after_ms`, and a wake rule may leave such codes out (its `codes`,
    [agent-wake.md](../agent-wake.md), "Rules").
- **The clock.** The hold's start is the row's `since_unix_ms`, the table's
  own: another `why` code starts another run, as it does there, and no second clock is kept. One
  exception, a rule of HELD-1 (settled by the coordinator on 11-12): the three waits for one
  fact's refresh, `fact_refresh_pending` (the reader is asked), `fact_refresh_unresolved` (it read
  again and the key is still missing) and `fact_refresh_exhausted` (its attempts are used up),
  share one clock per `fact_key`. HELD-1 carries the table's `since_unix_ms` over when the row
  moves between them with the same `fact_key` ("Why A Candidate Does Not Run"), so the move
  restarts nothing and `ac_schedule` shows the same start; another `fact_key`, or any other code,
  starts another run. A start of the Runtime begins with no table, so a hold does not span a
  restart. The record is
  written at the first evaluated cycle at or after the start plus the threshold; the policy
  driver's reconciliation evaluates at least every `policy_cadence.reconciliation_interval_ms`
  (60 000 ms by default), so it comes within one interval. A deferred or skipped cycle leaves the
  table as it was and writes nothing (a skipped evaluation has its own records, "Evaluation
  failures").
- **The threshold and the switch.** `scheduling.task_held.enabled` (default
  `true`) and `scheduling.task_held.after_ms` (default 1 800 000, from 60 000 to 86 400 000), each
  also per instance and settable, read at every evaluated cycle
  ([runtime-config.md](../runtime-config.md), "Held records"). The threshold is one lower-bound
  duration, the floor alone, per instance too and changeable while the Runtime runs (settled by
  the coordinator on 11-12): no one rhythm exists for every pair (register §1.322). The
  coordinator's proposal of 11-12, a default "by the task's own rhythm", was dropped for that
  reason: a cooldown is a lower bound between two dispatches, not a rhythm; a clock's period
  exists only for a trigger with a clock; and the expected duration says how long a run takes,
  not how often it should run. A lower threshold reports, at the next
  evaluated cycle, every run already past it; a higher one withdraws nothing written. Off, an
  instance writes no new held record, and the end of a hold already reported is still written,
  so no reported hold is left without its end while the Runtime runs.
- **The record.** A `policy.planning_signal_observed` of kind `goal_missed`
  (the clock-slot records' carrier), `instance_id` the alias, `task_id` the task, observed at the
  evaluation time. Its `signal_id` is `signal:task_held:held:<hex>`, `<hex>` being the lowercase
  SHA-256 of the UTF-8 bytes `task \0 alias \0 since`, the start in decimal ASCII; it is checked
  against the planning-signal projection before it is written, so one hold is reported once,
  across restarts too. Its outcome is `task_held` (warning, ladder class `none`) with:
  - `task_id`, `alias`; `gate_code` the row's `why` code (the clock-slot records' key);
    `held_state` (vocabulary `held_state`: `due` for eligibility `true`, `undecidable` for
    `unknown`); `held_since_unix_ms` the start; `held_for_ms` the evaluation time less the start;
    `held_after_ms` the threshold in effect;
  - one `caused_by` link: the row's `why` outcome whole, with every value the reason carries, as
    the evaluation gave it. What would release the hold is there as far as the reason says it (for
    the no-progress guard `basis_readings` and `served_by_dispatch_unix_ms`; for an instance held
    by a claim `holder_kind`; for a fact refresh wait `fact_key`, `package_id`, its own
    `gate_code`); nothing is added that needs another observation. A reason without values
    (`trigger_unknown`) gives a link without values.

  For example, in full detail:

  ```json
  {"code":"task_held","category":"warning",
   "values":{"task_id":"task.neutral.head","alias":"neutral-a","gate_code":"policy_no_progress",
             "held_state":"due","held_since_unix_ms":1700000000000,"held_for_ms":1800000,
             "held_after_ms":1800000},
   "causes":[{"relation":"caused_by","code":"policy_no_progress","category":"info",
              "values":{"basis_readings":[{"fact_key":"resource.neutral.amount",
                                           "reading_value":"12","observed_at_unix_ms":1699999000000}],
                        "served_by_dispatch_unix_ms":1699998000000}}]}
  ```

  A stated limit of HELD-1 (settled by the coordinator on 11-12): an undecidable hold whose reason
  carries no values (`trigger_unknown`, `feedback_stop_unknown`) names no fact key, neither in its
  values nor in its link. What the trigger or the stop waits for is the task's own predicate in
  the catalog generation; the later detector of mutual waits reads it there.
- **The end.** A hold that was reported and has ended leaves one
  `signal:task_held:ended:<hex>` record (the held record's `<hex>`), checked against the projection
  as well, of outcome `task_held_ended` (info, ladder class `none`): `task_id`, `alias`, `gate_code`
  the hold's code, `held_since_unix_ms`, `held_end` (vocabulary `held_end`) and `held_for_ms` the
  time from the start to the evaluation that found it ended, absent for `restarted`. A hold that
  was never reported leaves nothing. `held_end`:
  - `selected`: the evaluation selected the pair's decision (whether its intent is then admitted
    is the dispatch records');
  - `why_changed`: the pair's row carries another `why` code, held or not (a move between the
    three waits for one fact's refresh with the same `fact_key` excepted, "The clock"): a trigger
    that turned false, a package its pack schedule closed or a pause ends a hold this way, since
    the row stays in the table with that code;
  - `row_gone`: the instance has rows and none of the pair (the task left the catalog generation,
    or its package left the composed catalog);
  - `instance_gone`: the evaluation decided nothing for the instance (it left the snapshot);
  - `restarted`: the hold was reported by an earlier run of the Runtime. Before a start records
    its settlement anchor (`host.contained_settlement`, #405 N-2), it reads by the ledger's type
    index the `policy.planning_signal_observed` records between the start record that anchor names
    (the last start that completed its settlement; the ledger's first record when there is none)
    and its own start record, the range the settlement of direct runs reads, and ends each held
    record among them that has no end record. The projection answers only one `signal_id` at a
    time, so it cannot list the open holds itself; the read covers the runs since the last settled
    start and is exact, since a start that stopped before recording its anchor leaves the anchor
    where it was. Counting begins again at the first evaluated cycle. A held record the start
    cannot read back (its `gate_code` is no code this build's catalog registers, a value is
    missing, or its id is not the digest of its own task, alias and start, as another build may
    have written it) stops no start: it is left without an end, named by its ledger sequence on
    the developer line `actingd task_held unreadable sequences=<s1,s2,…> more=<n>`, and stays
    listed open in `actingd suspended` (settled by the coordinator on 11-12, register §1.327). A
    `gate_code` this build registers and no longer holds reads back, and its hold is ended.
- **How many.** Two reports of one pair are at least one threshold apart (a
  new run starts after the last one ended and must last the threshold again), so one pair writes
  at most ⌈86 400 000 / `after_ms`⌉ held records a day and as many ends, 48 and 48 at the default,
  plus one restart end per open hold at a start; an instance writes at most that times the pairs
  placed on it. There is no cap: the bound holds by construction, and a wake rule coalesces an
  instance's records into one message ([agent-wake.md](../agent-wake.md)). A pair that moves between
  two held codes ends a reported hold (`why_changed`) and starts a new count at each change, so a
  pair that changes codes faster than the threshold is never reported. The fact refresh waits are
  the exception ("The clock"): a wait that moves between `fact_refresh_unresolved` and
  `fact_refresh_pending` each time the instance's reader is selected keeps one clock for its
  `fact_key`, and is reported once it has lasted the threshold (register §1.322, settled 2).
- **What else it does.** As every planning signal, a held record and its end
  invalidate each active fact whose `invalidate_on` lists `policy.planning_signal_observed`, on any
  instance (they have no instance link; "Clock-slot records", "Fact invalidation"). No fact the Runtime or a run
  of a package writes lists that type; only a client's `PublishFacts` can. Having no such effect
  would take a `ledger` revision, so the effect is kept (register §1.322, settled 4).
- **Where a person reads it.** Without HELD-1's readers: the record is in the
  errors view (warning and above) of `ac_events` and of an `ac_diagnose` without an instance; a
  read filtered to an instance does not show it, since a planning signal has no instance link;
  `actingd suspended` lists it nowhere; the `not_run` rows of `ac_diagnose` and `ac_schedule` show
  the row's `since_unix_ms`. HELD-1 adds `held` to `actingd suspended` (the held records still open
  or ended within 48 h of the ledger's latest event, newest first, at most 200, each with its end
  when recorded), the instance's `held` rows to `ac_diagnose`, and `held_reported` on a `not_run`
  row whose task and `since_unix_ms` an open held record names (actingctl joins them; no wire shape
  changes). A mark on `ScheduleExplanation`'s own row is a later slice's (`runtime-client`).
- **Two tasks that wait for each other.** Not detected here; a later detector
  reads a held head's basis keys from its link and a follower's `held_since_unix_ms` and
  `gate_code`, and pairs them through the catalog generation the evaluation holds (the follower's
  `dependency_completed` on the head, or a fact the head's package marks).
- **The change-only history.** The record reads the same table that 114-B
  will copy (`policy.not_run_changed`, "Why A Candidate Does Not Run") and changes no shape of the
  table; its `since` rule changes only for the three waits for one fact's refresh ("The clock"),
  whose start a history row then copies as the table holds it, so 114-A's frozen shapes stay and
  114-B is no harder to build; once
  114-B writes, a held record's (alias, task, `held_since_unix_ms`) names the row of the history
  whose `since_unix_ms` it equals.

## Fact-time wake (Workflow #405)

A task that is due when earlier work ends, at an instant a package read and published as a fact, names the predicate `{"kind": "fact_time_reached", "scope": …, "fact_key": …, "offset_ms": N, "max_age_ms": M}` (`actingcommand.scheduling.v2` only; [v2/README.md](v2/README.md)). The scheduler wakes at that instant instead of polling.

- **True** when the fact at (`scope`, `fact_key`) exists, its confidence is above 0, it is fresh exactly as for `fact` (observed no longer ago than `max_age_ms` when M is not null, and not past its own `expires_at_unix_ms`), its value is a `timestamp_ms` T, and the evaluation is at or after `T + N`. The sum is signed and never overflows; a sum below 0 counts as 0. While the predicate is true, its freshness is the fact's and it wakes the evaluation one millisecond after that freshness ends, as `fact` does.
- **False, not reached**, before `T + N`: the predicate adds the info reason `fact_time_pending` {`fact_key`, `next_eligible_unix_ms` = `T + N`} and wakes the evaluation at `T + N`, or one millisecond after the fact's freshness ends when that comes first. A newer reading of the fact wakes the scheduler as every fact publication does, and the next evaluation reads the new time.
- **False, no time**: the fact is missing, its confidence is 0, it is older than M, it is past its own expiry, or its value is not a `timestamp_ms`. The predicate adds the info reason `fact_time_unavailable` {`fact_key`, `fact_time_problem`: `missing`, `low_confidence`, `stale`, `expired` or `not_a_time`} and no wake. Unlike `fact` it is false, not Unknown: the time is the output of the package that ran the work, which the instance's recognition maintenance package cannot read, so the predicate never asks for a fact refresh ("Fact Refresh") and gives no detection suggestion.
- **Minimum interval.** Once T + N has passed, the predicate stays true until a newer reading moves the time, unless it carries `once_per_reading` ("Condition edges" below). The least time between two dispatches of the task is its `cooldown_ms` ("Task Execution Fields"); the predicate has no interval of its own.
- **Fallback.** A catalog may write the predicate and a clock in one `any`: whichever is true first makes the task due, so the clock still runs it while the fact gives no time. Under `all` and `not` the predicate combines as any other, and every other predicate evaluates as before. When the predicate decides the trigger, the decision ends with `trigger_false` after its reason.
- It is no counted clock for the clock-slot records (above) and references no outcome key.

## Condition edges (SCH-M CF-1)

A chain step runs once per new result of its predecessor, and a completion time is served once, without the pass, round and slot reset events that cleared `task.` and `outcome.` facts so that a level could fire again. Two optional boolean members ([v2/README.md](v2/README.md); `actingcommand.scheduling.v2` only) make a predicate an edge. Both are absent by default: a catalog without them evaluates, serializes and hashes exactly as before. Their inputs are the `TaskRuntimeSnapshot` fields of this task and of the predecessor on the decision's instance ("Settlement Feedback And Eligibility Age"); nothing else is read. This task's own fields are its own last result, which no timeline reset clears (since 0.12.25, "Reset exclusions and the task's own last result").

- **`dependency_completed` with `since_own_run: true`.** True when the predecessor's latest terminal state is one of `terminal_states`, and this task has not served it yet: it has no admitted dispatch on the instance, or its latest dispatch is before the predecessor's latest dispatch, or its latest run did not succeed. Otherwise false, with the info reason `dependency_already_served` {`task_id` this task, `predecessor_task_id`} and no wake; the predecessor's next accepted terminal makes it true again.
  - A failed successor stays due. Its cooldown, its retry and backoff (the Runtime's `on_failure` handling) and its daily limits bound how often it runs again.
  - A predecessor with no settled run is Unknown (`dependency_state_missing`) as before, and a terminal state the predicate does not accept is false as before.
  - With `false` or without the member the predicate is the level it always was: true whenever the predecessor's latest terminal state is accepted.
  - Chain handover ("Chain Handover") names the predecessor the same way, with or without the member.
- **`fact_time_reached` with `once_per_reading: true`.** A time T the fact holds counts as **served** when this task's latest run on the instance succeeded and was dispatched at or after max(`T + N`, the fact's `observed_at_unix_ms`). A served time makes the predicate false, with the info reason `fact_time_served` {`fact_key`, `served_by_dispatch_unix_ms` that dispatch} and no wake. It stays false until a newer reading, observed after that dispatch, gives a time; the publication of that reading wakes the scheduler, as every fact publication does. A run that failed, or one dispatched before the time or before the reading was observed, served nothing. Every other case is "Fact-time wake" above.
  - This closes the loop of an emptied slot: a slot left empty shows no countdown, so its last (past) time would otherwise stay true forever. A catalog names a free-slot count fact in an `any` beside it.
- When a member decides the trigger, the decision ends with `trigger_false` after its reason.
- **Version.** A V1 document that writes `since_own_run`, with either value, is refused as `unknown_field` at the member; `fact_time_reached` is V2 only as a whole.
- **Capability.** A Runtime that reads both members reports the always-on subsystem `condition_edges` in `config.subsystems` ([`actingd-check-config.md`](../actingd-check-config.md)). The catalog generator emits neither member for a Runtime whose check-config does not name it; an older Runtime refuses a catalog that writes them (`unknown_field`).

## Fact references (#386 CF-4)

A red line is the current cap less a margin, never a fixed number (Alice 10-26): a package publishes the cap as a fact beside the amount, and a rule compares the two. A `fact` predicate's `value` may be a **fact reference** instead of a typed value ([v2/README.md](v2/README.md); `actingcommand.scheduling.v2` only):

```text
{"kind": "fact", "scope": S, "fact_key": A, "comparison": C,
 "value": {"type": "fact", "fact_key": B, "offset": N}, "max_age_ms": M}
```

It reads: the integer the fact at (S, A) holds compares by C with the integer the fact at (S, B) holds plus N. All three members of the reference are required; N is a signed integer. A predicate with a typed value evaluates, serializes and hashes exactly as before.

- **Same scope.** The reference names no scope: B is read in the predicate's own scope S. In a `schedule.json` the composer fills S for the predicate as for every `fact` node, and the reference needs none.
- **One freshness.** Both facts obey the predicate's `max_age_ms` and each its own `expires_at_unix_ms`, and each needs a confidence above 0; the reference has no `max_age_ms` of its own. While the predicate is true or false, its freshness ends at the earlier end of the two facts' freshness, and it wakes the evaluation one millisecond after that, as `fact` does.
- **The fact itself** (S, A) is read as for every `fact` predicate: missing, at confidence 0, older than M or past its expiry, it is Unknown with its detection suggestion ("Fact Refresh"); a value that is not an `integer` is Unknown with `type_mismatch` ("Runtime Enforcement").
- **The referenced fact** (S, B) that gives no integer makes the predicate **false**, not Unknown: the info reason `fact_reference_unavailable` {`fact_key` B, `unknown_reason`: `fact_missing`, `fact_low_confidence`, `fact_stale`, `fact_expired` or `type_mismatch`}, no wake and no detection suggestion. The next reading of B wakes the scheduler, as every fact publication does. When the predicate decides the trigger, the decision ends with `trigger_false` after its reason. Under `not` a false predicate becomes true, as any other.
- **Comparison.** `eq`, `not_eq`, `less_than`, `less_than_or_equal`, `greater_than` and `greater_than_or_equal`, over the integers A and B + N; the sum never overflows. `contains` is refused.
- **Refusals**, each at its pointer: in a V1 document, `unknown_field` at `value/type`; `contains`, `predicate_uncomputable` at `comparison`; a reference `fact_key` that is no identifier, `type_mismatch` at `value/fact_key`; N outside -9007199254740991..=9007199254740991, `limit_exceeded` at `value/offset`; a reference that names a `scope` (a cross-scope reference), `unknown_field` at `value/scope`. A `value` whose `type` is neither a value kind nor `fact`, or an N that is no integer, is refused by the reader (`type_mismatch`).
- **Capability.** A Runtime that reads fact references reports the always-on subsystem `fact_reference` in `config.subsystems` ([`actingd-check-config.md`](../actingd-check-config.md)). A `schedule.json` whose rule writes one must list `fact_reference` in `requires` ([`standard-pack-schedule.md`](../standard-pack-schedule.md)); the composer refuses the file without it, so a Runtime that does not name the subsystem refuses the file (`schedule_requires_missing`) instead of misreading it. An older Runtime given a catalog with a reference refuses it at its reader.
- For example, a task due while the amount is at least the capacity less 1000: `"fact_key": "resource.neutral.amount", "comparison": "greater_than_or_equal", "value": {"type": "fact", "fact_key": "resource.neutral.capacity", "offset": -1000}`.

## Pool fact references (#386 CF-5)

An account's cap is read, never written as a constant (Alice 10-26). Two more V2 members accept the fact reference of "Fact references" above, `{"type": "fact", "fact_key": B, "offset": N}` ([v2/README.md](v2/README.md); `actingcommand.scheduling.v2` only): a pool's `capacity`, and a `resource_projection` predicate's `value`. Both read B **in the pool's own scope**; all three members are required and the reference names no scope. A pool or a predicate that writes a number evaluates, serializes and hashes exactly as before.

- **A capacity read from its fact.** `"capacity": {"type": "fact", "fact_key": B, "offset": N}`: at each evaluation the pool's capacity is the integer B holds plus N. B is read as the pool reads its own value: in the pool's scope, with at least the pool's `minimum_confidence_milli` (above 0 for a `static_snapshot` pool), not past its own `expires_at_unix_ms` (a timeline reset that invalidates its prefix removes it, as every fact), and the capacity must be at least 1. It has no `max_age_ms`: a reading's capacity expires with the reading (a `current_capacity` mark writes both parts with one lifetime, [resource-readings.md](../resource-readings.md)).
- **No capacity, no value: the projection defers.** While B gives no capacity the pool has no value, and no constant stands in for it. Its `resource_projection` is Unknown with the detection suggestion of B in the pool's scope and B's unknown reason: `fact_missing`, `fact_low_confidence`, `fact_expired`, or `type_mismatch` (no integer, or no positive capacity). So the task is deferred with `trigger_unknown` (or `feedback_stop_unknown`), and the fact refresh is asked for B ("Fact Refresh"), as for an unknown `fact` predicate. The pool's own reading is checked first: a pool whose reading is unusable asks for its own key, as before. A reserve on the pool is case 1 of "Task Reserves", a task that consumes it gets no fullness urgency from it, and a resource target on a live pool awaits observation with the matching reason ([resource-targets.md](../resource-targets.md)).
- **A reading above the capacity stays as read.** Against a capacity read from its fact, a pool's reading or snapshot above it is the pool's value (an amount can exceed its cap, for example through items): it is neither refused nor clamped, and it does not regenerate. Below the capacity each complete period adds `amount` up to the capacity, as before: regeneration stops at the capacity. Against a constant capacity a reading above it still leaves the pool without a value ("Pool Projection").
- **Freshness and wake.** The projection's freshness ends at the earlier end of the pool reading's and B's, and the evaluation wakes one millisecond after B's expiry, as after the reading's.
- **Bounds against a constant only.** The compiler checks a reserve's `at_least` ("Task Reserves") and an effect's quantity ("Expected quantities (v2)") against a constant capacity only. Against a capacity read from a fact nothing is checked at compilation: a reserve above the read capacity holds its task until the pool holds it.
- **A comparand read from a fact.** `"value": {"type": "fact", "fact_key": B, "offset": N}` on a `resource_projection` compares the pool's projected amount with the integer B holds in the pool's scope plus N, by its `comparison` (`contains` is refused as before); the sum never overflows. B obeys its own expiry and needs a confidence above 0, as a reference of a `fact` predicate without `max_age_ms` (a projection has none). A B that gives no integer makes the predicate **false**, with the info reason `fact_reference_unavailable` {`fact_key` B, `unknown_reason`}, no wake and no fact refresh, then `trigger_false` when it decides the trigger, as for a `fact` predicate ("Fact references"). The pool is read first: a pool without a value leaves the predicate Unknown, as above.
- **Forward projection** ("Forward Planning And Maintenance") reads B once, at its start, and holds that capacity across its horizon. A pool whose B gives no capacity there is left out, as a pool without a snapshot is, and reported as the evidence gap {`code` B's unknown reason, `fact_key` B}.
- **Refusals**, each at its pointer, as for a reference of a `fact` predicate: in a V1 document, `unknown_field` at `capacity/type` or `value/type`; a key that is no identifier, `type_mismatch` at `…/fact_key`; N outside -9007199254740991..=9007199254740991, `limit_exceeded` at `…/offset`; a reference that names a `scope`, `unknown_field` at `…/scope`. A `type` other than `fact`, or an N that is no integer, is refused by the reader (`type_mismatch`).
- **Capability.** A Runtime that reads both forms reports the always-on subsystem `pool_fact_reference` in `config.subsystems` ([`actingd-check-config.md`](../actingd-check-config.md)). A `schedule.json` that writes either must list `pool_fact_reference` in `requires` ([`standard-pack-schedule.md`](../standard-pack-schedule.md)); the composer refuses the file without it, so a Runtime that does not name the subsystem refuses the file (`schedule_requires_missing`) instead of misreading it. An older Runtime given a catalog with either refuses it at its reader.
- For example, a task due while the projected amount is at least the read capacity less 20: the pool `"capacity": {"type": "fact", "fact_key": "resource.neutral.capacity", "offset": 0}` and the trigger `{"kind": "resource_projection", "pool_id": "pool.neutral", "comparison": "greater_than_or_equal", "value": {"type": "fact", "fact_key": "resource.neutral.capacity", "offset": -20}}`.
- **A pool's own value read from a list** (#455 P1, since 0.12.78): a pool whose inventory is a sum over rows of a list reading writes no fact reference; its `observation` is of the kind `list_rows` ("A pool read from rows of a list" below).

## A pool read from rows of a list (#455 P1, since 0.12.78)

Register §1.504; the coordinator's rulings of 11-21 (board #455), not the owner's words. A game
with dozens of materials cannot give each its own fact (an instance holds at most 224), and no
reading by a fixed place on the screen is allowed; what a package reads is a whole list, one fact
under `inventory.list.` ("Record lists and the projection" below). So a V2 pool may declare that
its inventory is read from rows of that list ([v2/README.md](v2/README.md);
`actingcommand.scheduling.v2` only):

```text
"observation": {"kind": "list_rows", "fact_key": L, "key_column": K, "value_column": V,
                "rows": [{"key": k1, "factor": f1}, {"key": k2, "factor": f2}, …]},
"value_source": {"kind": "ledger_fact", "minimum_confidence_milli": C}
```

- **What it means.** The pool's inventory is `Σ f_j × v_j`, `v_j` the integer the column V holds
  in the row of the list reading L (in the pool's own scope, an instance) whose column K holds
  `k_j`. L is read with at least the confidence C, as a `fact` observation reads its key. The list
  itself enters neither the projection nor the evaluation's `facts`.
- **Who computes it, and when** (#455 P2, since 0.12.79, register §1.505; the coordinator's
  rulings of 11-22, not the owner's words). The host, before each evaluation, under the fact write
  gate it projects the fact pools under: it reads the instance's list fact L at the evaluation's
  ledger position and computes the sum with one pure function. The number goes where a fact pool's
  projected reading goes, the pool's snapshot in the evaluation's resources (its value the sum, its
  observation the list's), so it enters the snapshot's identity and each instance's view identity
  by the path a fact pool's reading takes; one moment at one ledger position gives one identity,
  and a catalog without such a pool gives the bytes and identities it gave before. Beside the
  left-out facts the evaluator is given, in memory only, each such pool's reading: the list's key,
  its observation time and its expiry, or why there is none. The evaluator reads no list. A
  forward projection starts from the same reading and carries it across its horizon as it carries
  a fact reading.
- **A key the list does not hold counts 0.** A list is written whole or not at all, so a key a
  list read whole does not hold is not on the screen it was read from. (The cost: a name the
  reading took for another valid key reads a held item as 0; a producer then runs once too often,
  which its target caps and the next reading corrects, and a consumer is held by its reserve.)
  Workflow #503 LIST-A (since 0.12.90): a list that wants only its declared keys
  ([resource-readings.md](../resource-readings.md), "A list of the wanted keys only") holds no row
  of a wanted item held at zero, whose screen shows no card of it, so this rule counts that item 0;
  its own cost, a wanted name read wrong far from every alias with confidence every time, which
  reads 0 every time and which the next reading does not correct, is written there, with its
  remedy. Nothing is paused, held or lowered on a reading of 0.
- **Unknown, never 0**, each a fact reading's reason under the list's key L: no list,
  `fact_missing`; a list past its expiry, `fact_expired` (the evaluation judges it by the expiry
  given with the reading); the list's confidence below C, `fact_low_confidence`; a named row's value
  that is no non-negative integer, or a sum beyond 2^53 − 1, `type_mismatch` (and, as for a fact
  reading, a sum a constant capacity cannot hold). A list that is not keyed by the pool's columns is
  `type_mismatch` too (fix round 1): one of its rows holds no text under K, or none of them holds a
  value under V, so a column name the package that reads the list does not write reads no key as
  absent. An empty list holds no key and reads 0. A timeline reset that invalidates L in the pool's
  scope at or after the list's observation leaves the pool without a reading (`fact_missing`), and
  one still to come bounds its expiry, as for a fact. Freshness is the list's: the consumption a
  reading does not show yet is that of every task dispatched since the list's observation.
- **Every reader takes it**: a `resource_projection` on the pool; the net benefit's cost term of a
  regenerating pool (`benefit_unknown` `reading_unknown` with L and the reason when unknown); the
  pool's lines and the capped spending order (`pool_line_held` `capped_cap_unknown` with L and the
  reason when unknown; `capped_spend` shows the sum); the reserve gate (case 1 when unknown); the
  urgency and the tie-break; the forward projection; a resource target's check, gap, receipt and
  views (the pool is observable under L; `awaiting_observation` with the reason when unknown); a
  goal entry's `current`; the suggestion of an unknown reason (L with its reason). The no-progress
  guard (fix round 1) takes such a pool into a task's basis as it takes a fact pool, a pool the
  trigger projects or the task reserves: the basis key is L in the pool's instance and its value
  the sum (when several pools read the one list, each one's sum), captured, observed and held
  with the readings the evaluation was given; a list read again with the same sum is no progress,
  one with another sum releases the guard ("No-progress guard").
- **No refresh is asked for it.** The fact refresh asks the instance's recognition maintenance
  package for the keys its marks write, and no mark may name a key under `inventory.list.`; so L is
  left out for the pool's instance with the reading, and an unknown reading raises no refresh demand
  (no `fact_refresh_*` reason). Nor (fix round 1) is a probe planned for it: a suggestion of a key
  the left-out set covers for its instance writes no `planning_fact_unknown` and takes no
  detection allowance. The package that reads the list is scheduled by its own rule; refresh on
  demand is a later slice.
- **Wakes.** The reading's expiry wakes the evaluation a millisecond after it, as a fact reading's
  does; a list's publication wakes it as every fact publication does.
- **In 0.12.78** (P1) no host computed the reading: every reader met the pool as a `ledger_fact`
  pool whose fact is missing, under L with `fact_missing`, a resource target on it was refused
  `resource_not_observable`, and a configured catalog holding one made each evaluation's input
  `policy_pool_binding_invalid`. From 0.12.79 a configured catalog holding one is accepted.
- **Refusals**, each at its pointer, with the compile's codes ([v2/README.md](v2/README.md)): in
  a V1 document `unknown_field` at `observation/kind`; beside `static_snapshot`, which would take
  the caller's fixed value and never the rows, `limit_exceeded` at the pool, as a `ledger_fact`
  pool without a fact observation is; a pool whose scope is not an instance (a list reading is a
  fact of one instance) `type_mismatch` at `observation/kind`; an L not under `inventory.list.`
  `type_mismatch` at `observation/fact_key`; a K or V that breaks the rule of a list's column names
  (`^[a-z][a-z0-9_]{0,31}$`) `type_mismatch` at the column, and V the same as K `type_mismatch` at
  `observation/value_column`; no row or more than 64 `limit_exceeded` at `observation/rows`; a key
  empty or with a control character `type_mismatch`, longer than 1024 bytes (a fact text's bound)
  `limit_exceeded`, a key twice `duplicate_id` (at the second), at `observation/rows/<j>/key`; a
  factor outside `1..=1000000` `limit_exceeded` at `observation/rows/<j>/factor`.
- **The one member that names the prefix.** An observation of this kind is the only catalog member
  that may name a key under `inventory.list.`, and it may name no other; every other member that
  names one stays refused ("Record lists and the projection").
- **Capability.** The subsystem is `pool_list_rows`. A `schedule.json` whose pool writes the kind
  must list it in `requires` ([`standard-pack-schedule.md`](../standard-pack-schedule.md)); the
  composer refuses the file without it (`schedule_invalid` at the first such pool's
  `observation`). A Runtime names it, always on, from 0.12.79 (#455 P2, `config.subsystems`), and
  accepts such a file. No Runtime of 0.12.78 names it, so a Runtime of that version refuses such a
  file (`schedule_requires_missing` at that `requires` entry) and keeps the game's last good rules.
  A Runtime of 0.12.77 refuses it there too; given a catalog with the kind, its reader refuses it
  (`type_mismatch`: an unknown `kind`).
- **Identity.** A pool without the kind serializes, hashes and decides exactly as before; a
  catalog without it compiles byte for byte as before.

## Needs computed from the records (Workflow #455 RR3, since 0.12.95)

Register §1.538; the coordinator's rulings of 11-28. A person says, on an instance, whom to raise
and to which grade (a `records` entry's records, the instance fact `session.goal_records`), and the
game root carries the game's rules for what that takes (`goal-tables.json`). Before every
evaluation the host computes, for each instance that holds records, how much of each pool the
recorded characters still need and do not have
([../standard-pack-goal-tables.md](../standard-pack-goal-tables.md), "What is computed"); the
evaluator stacks it on the instance's stored goals and raises the producers of what is lacking
([../resource-targets.md](../resource-targets.md), "Needs computed from the records").

- **Who computes it, and when.** The host, with one pure function, under the fact write gate the
  evaluation's inputs are projected under, right after the projection: from the instance's stored
  records, the root's tables as the scheduler workbench keeps them for the generation in force
  (read from memory before the gates), the instance's progress lists as the fact store holds them
  (they enter neither the projection nor the evaluation's `facts`, as every list reading), and the
  holdings, read as the evaluator reads a target's inventory (the time-validity projection of the
  same facts and readings, the same observation). The admission of an intent computes it again for
  the intent's instance, the same way, at the intent's evaluation time. #455 RR3c (since 0.12.98;
  register §1.543): with the handed runs at the inputs' ledger position, the list the evaluation of
  those inputs is given ("Pool lines", "At each evaluation"), so a holding a run left unread is
  unread to both.
- **What the evaluator is given.** Beside the left-out facts, the readings of the pools read from
  rows of a list and the handed runs, in memory only: per instance, each pool's need (by the id
  the root's files write it), every part that added nothing with its cause, the tables' sha256,
  when the records were set and when the set's freshness ends. It lives one evaluation; nothing of
  it is stored and nothing enters a snapshot.
- **Goals raise producers.** A pool's stacked goal asks `stock + need`; a pool on which a need
  lands without a stored goal is judged as if it held one of `need` with its own step and
  importance. Each covers the pool's producing candidates as a target without `tasks` does, so the
  pool's shortfall weight `Γ` raises them ("Resource weights (v2)" there); when the gap closes the
  push is gone. What a goal changes is which task is worth running: when one runs stays with its
  trigger and the pool's lines.
- **Identities.** The set's identity is hashed into the decision identity of every candidate of
  the instance (a change of the tables appends nothing to the ledger, and one identity must never
  name two decisions) and into the instance's view (`dispatch_view_identity`) after the rest; the
  admission compares it with the set computed again, so an intent taken on other progress, other
  records or other tables is `policy_facts_stale` whatever the snapshot's identity. An instance
  without records has no set: its identities and the bytes they hash are those of before.
- **Freshness and wakes.** The set's freshness ends with the earliest expiry among the lists and
  holdings it read; a candidate its goals move is no fresher, and the evaluation wakes a
  millisecond after, as for a fact that expires. A list's publication wakes an evaluation as every
  fact publication does.
- **Known limits.** The forward projection reads no computed need (as it reads no void, "A game
  whose root cannot be composed" in [../resource-targets.md](../resource-targets.md)); a reading
  is not made old by a run that has produced into it since (the gap reads the holding as it was
  read), unless (#455 RR3c, since 0.12.98) that run was told enough turns to close a goal's gap on
  the pool (#455 RR3d, since 0.12.101: or the runs told under the gap since the reading were,
  together): then the holding is unread until a reading observed at or after their end ("Pool
  lines"), the goal on the pool pending and, for an item with a recipe,
  `goal_records_holding_unknown` with `unknown_reason` `fact_stale`, nothing taken apart below it;
  a rule that produces an item with a recipe counts for what it produces, not for what it is made
  from.

## Pool lines (SS-2h)

Since 0.12.46 (register §1.289). Alice's ruling 138 (11-07, Workflow #428 comment 6706493161)
puts a planned line below a resource's cap, given by the model, a person or an agent, never
computed by the program; her answer of 11-09 17:37 makes it two water lines, a stop line above a
hard line, and 17:42 says where they come from (the model, or an estimate from data: for example
the hard line is the stop line less the costliest run's consumption) and that they constrain the
scheduler only and are moved from outside over time; 17:48 approved the whole (#428 comment
6735960127; design `rd-1031/design-c424-redline-1109.md` §1.6).

A V2 pool may write `stop_line` and `hard_line` ([v2/README.md](v2/README.md);
`actingcommand.scheduling.v2` only), both or neither, each in one of two forms:

- **A fact reference**, the shape of a capacity read from its fact ("Pool fact references"):
  `{"type": "fact", "fact_key": B, "offset": N}`, the integer the fact B of the pool's own scope
  holds plus N, read as a capacity fact is (at least the pool's minimum confidence, at least 1,
  and not past its expiry). Meant as the pool's capacity fact with a negative offset: the read cap
  less a margin. The Runtime adds the given offset to the fact it reads and computes no line.
- **A constant**, an integer in `0..=9007199254740991`, the line itself.

SCHED-C1c (register §1.299, since 0.12.53) adds a third member in the same two forms,
`trigger_line`, written only beside the other two ("The trigger line" below); SCHED-C1e (register
§1.299, since 0.12.54) adds lines given for one instance ("Lines for one instance" below).

- **Without lines** a pool's stop line `S` is its capacity and its hard line `H` is 0 (the
  research channel's proposal the coordinator approved, design §1.6 item 1): it is spent only at
  its cap, one run at a time, and has no protection of its own. SCHED-C1c (since 0.12.53): its
  trigger line `T` is its capacity too, and so is that of a pool that writes the two lines and no
  `trigger_line`.
- **At each evaluation** the lines of a valued pool are read on each instance its scope covers and
  must hold `0 ≤ H < S ≤ cap` (SCHED-C1c, since 0.12.53: `0 ≤ H < S ≤ T ≤ cap`). The reading `r` they are checked against is the pool's projected
  amount less the consumption its reading does not show yet: every task dispatched on the
  instance at or after the reading's observation, by its declared consumption (the reserve gate's
  step 3, "Task Reserves"), the candidate's own task included; each dispatch counts one run.
  #477 LF-1 (since 0.12.80; register §1.509; the coordinator's ruling after the two reviews of
  #1872, not the owner's words): **a run told its turns leaves its pools unread.** A pool read
  from a fact or from rows of a list (not a static value) is unknown to the whole evaluation (`unknown_reason`
  `fact_stale`, under the pool's own key) while a (task, instance) pair whose task consumes it
  (any entry of its `consumes`, whether or not the pool took part in the cap, on any instance)
  has a latest scheduled dispatch that was handed a `run_cap` (`policy_dispatch_parameters`,
  [../remedies.md](../remedies.md), section 4) and that has not ended, or ended after the pool's
  reading was observed. Only a reading observed at or after that end makes the pool known again;
  one the run wrote while it ran does not. The evaluation's pool values say so, so every reader
  of them sees the pool unknown: the triggers (`resource_projection` Unknown, the pool's key
  suggested with `fact_stale`), the net benefit, the lines (`capped_cap_unknown`; an episode under
  way waits), the reserve gate (its step 1) and the spend values (no `spend_allowance`, no
  `run_cap`). So nothing is computed from a reading that a run's unknown number of turns made
  stale, and the pool is read again before anything that needs its value is dispatched. A
  dispatch handed no `run_cap` leaves the pool known and counts one run, as before. The host gives
  the evaluation, beside the left-out facts, the pairs whose latest admitted dispatch at the
  inputs' ledger position carried a `run_cap` on its recorded reasons, each with when its run
  ended (the observation of its settlement) or that it has not; no record, event or wire member
  is new, and a start reads the same from the ledger. #455 RR3d (since 0.12.101; register §1.548;
  the coordinator's rulings of 11-28): **a run that failed at its entry leaves nothing unread.** A
  dispatch whose execution record, at the inputs' ledger position, is a failure its settlement
  settled as one at its run's entry (a condition whose failure identity carries the entry mark,
  [../policy-suspension.md](../policy-suspension.md), "A failure at the entry") is passed over
  before a pair's latest is chosen: its main package executed no step, so it consumed and produced
  nothing, and the pair's previous admitted dispatch stands, judged by the same rule. Every other
  end leaves the pool unknown as before: a success, any other failure (one at the entry of another
  class and one a start's reconciliation settled included), a stop and an interrupted settlement
  may each have done some turns, and nothing the host holds says how many. Limits: a package that runs one turn
  whatever its `run_cap` still leaves the pool unknown until the next reading; a pool whose scope
  covers several instances is unknown on all of them after such a run on any one; a run whose
  settlement is not recorded keeps it unknown until it is (a start settles an earlier epoch's
  runs); a later dispatch of the pair handed no `run_cap` takes the place of the one that was,
  and a pair whose latest admitted dispatch the host's journal no longer holds is not noted; the
  resource targets' conditions and a goal's gap read the pool's observation themselves, as
  before.
  #455 RR3c (since 0.12.98; register §1.543) and RR3d (since 0.12.101; register §1.548; the
  coordinator's rulings of 11-28): **the production told under a goal's gap leaves the pool unread
  once it reaches the gap.** For a pool whose current reading was observed at `t`, take the
  admitted dispatches the host's journal holds at the inputs' ledger position (of any pair, but one
  whose run failed at its entry, above) whose run has not ended or ended after `t` and whose
  recorded `policy_dispatch_parameters` carry `gaps_remaining` with a record
  `{effect_index: j, gap_remaining: g}`, `g ≥ 1`, whose entry `j` of the task's `produces` (as the
  catalogue in force at the evaluation declares it; an entry the task no longer has is passed over)
  is on the pool, in the order of their admission. The pool is unread when for one of them the sum,
  over it and those before it, of `n·y` reaches `g·1000`, `g` the gap that dispatch was itself told
  (the least where it was told several on the pool): `n` the `run_cap` each one's reason carries (1
  without one), `y` its task's production of the pool per run in thousandths as the goal term of
  `run_cap` reads it (its integer amounts times 1000, else its expected yield; a pending amount
  yields 0 and the dispatch is passed over), checked arithmetic, an overflow counting as reached.
  With one such dispatch this is 0.12.98's rule, `n·y ≥ g·1000`, byte for byte. So dispatches that
  each fall short of the gap but together reach it leave the pool unread (a gap of 6 told 3 twice;
  a stock gap of 150 at three turns of one unit at its fiftieth dispatch since the reading, read
  before), and a later dispatch of the pair that carries no gap for the pool (its goal pending, or it
  ran for another pool) no longer takes the place of one that closed it. Unread means what it means
  above, for the same span: a pool read from a fact or from rows of a list is unknown to the whole
  evaluation, `fact_stale` under its own key, for every reader of the pool values (the triggers,
  which ask the fact refresh for it, the net benefit, the lines, the reserve gate, the spend
  values), until a reading observed at or after the end of every run of the sum that reached the
  gap (a reading observed at a run's end or after counts only the dispatches after it); and, for
  this pool alone, also for the two readers that do not read the pool values: a resource target's
  observation of it (pending, `Γ` 0, no goal term of `run_cap` and no `gap_remaining` from it;
  [../resource-targets.md](../resource-targets.md), "Resource weights (v2)") and the holdings of the
  needs computed from the records ("Needs computed from the records" above). One judgement decides
  all three, and the host hands the needs and the evaluation the one list, read once under the gates
  the inputs are projected under. Dispatches whose sum does not reach the gap, and a dispatch that
  carried no `gaps_remaining`, leave the pool read, as before; the rule above for the pools a run
  consumes reads each pair's latest dispatch as before, and a pool both name is unknown once. The
  host notes every admitted dispatch from its recorded reasons, in the order of their admission,
  with its end, its gaps and its `run_cap`; nothing recorded is new, memory only, and a start reads
  the same from the ledger. Limits: a pool whose scope covers several instances is unread on all of
  them after such runs on one; a run that did fewer turns than it was told still counts what it was
  told until the pool is read; the yield is the catalogue's expectation, so a run with poor drops is
  read once and sent again; a run that still ran when the reading was observed is counted whole
  though the reading may show part of what it produced (the sum errs toward unread, at the cost of a
  reading); a pair's dispatches older than what the host's journal keeps of it (its latest 64
  admitted dispatches) are not counted, as a pair the journal no longer holds is not noted.
- **Below the cap** (SCHED-C1c, since 0.12.53: below the trigger line, while no episode is under way) a judged candidate that would pass the net-benefit gate (a positive `B`, or
  released in the zero tier) and whose run would leave `r − ⌈C/1000⌉ < H` is not dispatched:
  deferred with `pool_line_held` (info; `pool_line_hold` `would_cross_hard_line`, `pool_id`,
  `pool_reading`, `pool_capacity`, `stop_line`, `hard_line`, `amount_after`), waking at a
  regenerating pool's next step as a not-positive candidate does. #455 B1 ruling 9 (since
  0.12.82): below the trigger line of a pool that regenerates (the capacity without a written
  `trigger_line`), and while its lines give no value with no episode under way, whether a
  candidate with a fixed value would pass is judged without its goal terms ("Fixed values written
  in the pack", "A goal chooses which task spends a pool, the pool's lines choose when").
- **Unknown or invalid.** A pool that writes its lines and whose capacity, reading or a line's fact
  gives no value enters no capped state, and its judged candidates that would pass are deferred
  with `pool_line_held` `capped_cap_unknown` (`fact_key`, `unknown_reason` when a fact gives
  none). Lines read that break `0 ≤ H < S ≤ cap` hold them with `lines_invalid` (`stop_line`,
  `hard_line`, `pool_capacity`). A pool without lines whose reading is unknown has no hard line to
  keep: nothing changes for it (as before; a regenerating one is `benefit_unknown` anyway).
  SCHED-C1d (register §1.299, since 0.12.53): an unknown value no longer ends a capped episode
  under way; the episode waits ("Capped spending order", "Episodes survive"). Lines read that break
  their rules still end it, as SS-2h built.
- **Who is held.** The lines constrain the scheduler only (Alice, 17:42): a judged candidate. A
  manual request, a chain successor that rides with its chain, an unmarked task and the Runtime's
  own reading are never held by them.
- **Refusals**, each at its pointer: in a V1 document, `unknown_field` at the member (either form);
  a reference by a capacity reference's rules at `stop_line/…` or `hard_line/…`; a constant above
  9007199254740991, `limit_exceeded` at the member; and `pool_lines_invalid` (error) at the member
  when one is written without the other (at the one written), when a line is above the capacity
  where the compiler can tell (a constant above a constant capacity, or a reference to the
  capacity's own fact with a larger offset), and when the stop line is not above the hard line
  where it can tell (two constants, or two references to one fact by their offsets; at
  `hard_line`). Lines it cannot compare are checked at each evaluation (above). SCHED-C1c (since
  0.12.53): `trigger_line` by the same rules, at `trigger_line`: `pool_lines_invalid` when it is
  written without both other lines, below the stop line, or above the capacity, where the compiler
  can tell.
- **Without a valuation** (SS-2h fix 1): only a valued pool's lines take part. A V2 pool that writes
  both lines but no declared valuation (none, or the pending marker) compiles with the warning
  `pool_lines_unvalued` at its `stop_line` (the first compile warning; "Diagnostics"): the lines are
  carried as written and change no decision. Where it is seen (#428 CW-1, since 0.12.47): offline,
  in the report of `actinglab scheduling compile`; for a composed catalog, the Runtime's workbench
  records it when it activates the generation with that file changed, on the
  `standard_pack_registered` record of the game whose `schedule.json` gave the pool
  (`compile_warnings`, its code and its pointer in that file; no record for a file that did not
  change), and holds the active generation's warnings, every game's, in the program fact
  `config.catalog_warnings`, which
  `actingctl overview` and `ac_overview` show in their `catalog` block (`compile_warnings`)
  ([standard-pack-schedule.md](../standard-pack-schedule.md), "The scheduler workbench").
- **Capability.** A Runtime that reads the members reports the always-on subsystem `pool_lines` in
  `config.subsystems` ([`actingd-check-config.md`](../actingd-check-config.md)). A
  `schedule.json` whose pool writes either must list `pool_lines` in `requires`
  ([`standard-pack-schedule.md`](../standard-pack-schedule.md)); the composer refuses the file
  without it (`schedule_invalid` at the first such member), so a Runtime that does not name the
  subsystem refuses the file (`schedule_requires_missing`) instead of misreading it. An older
  Runtime given a catalog with either member refuses it at its reader (`unknown_field`).
  SCHED-C1c (since 0.12.53): `trigger_line` likewise with its own always-on subsystem
  `pool_trigger_line`, which a `schedule.json` whose pool writes it lists in `requires` (a Runtime
  that names `pool_lines` alone does not read it). #455 P1 (since 0.12.78): a pool whose
  `observation` is `list_rows` likewise lists `pool_list_rows`, which a Runtime names from 0.12.79
  (#455 P2) and no Runtime of 0.12.78 names ("A pool read from rows of a list").
- **Identity.** A pool without the members serializes, and hashes, exactly as before.
- For example, a pool whose cap is read, its stop line 40 below the cap and its hard line 76 below
  it: `"capacity": {"type": "fact", "fact_key": "resource.neutral.capacity", "offset": 0}`,
  `"stop_line": {"type": "fact", "fact_key": "resource.neutral.capacity", "offset": -40}` and
  `"hard_line": {"type": "fact", "fact_key": "resource.neutral.capacity", "offset": -76}`.
- A line given for one instance takes precedence over the catalog's, and the catalog's over the
  defaults above (SCHED-C1e, since 0.12.54, in place of SS-2h-c; "Lines for one instance" below).

The three parts below are SCHED-C1's (register §1.299), the coordinator's rulings of 11-11, not
Alice's words. "The trigger line" and "A pool its lines cannot spend" are built (SCHED-C1c, since
0.12.53); "Lines for one instance" is built by SCHED-C1e (since 0.12.54).

- **The trigger line** (the coordinator's point 10). A pool that writes both lines may also write
  `trigger_line`, in either form: the reading at which a capped episode begins ("Capped spending
  order"). Absent, it is the capacity, and nothing changes from what Alice released: the episode
  begins at the cap (point 2 of the ten points the coordinator wrote down from her approval of
  11-09 17:48, #428 comment 6735960127). Present, the lines read must hold `0 ≤ H < S ≤ T ≤ cap`,
  or the pool's judged consumers are held with `lines_invalid` (which then also carries
  `trigger_line`); its refusals and its subsystem are above. Below the trigger line, while no
  episode is under way, the pool is "below the cap" for the rules above: full price, and the hard
  line held. A trigger line one costliest run above the stop line makes the pool spend whenever it
  is that far above its stop line, without ever reaching its capacity; that choice is data the pack
  or a person sets, not the Runtime's.
- **Lines for one instance** (point 8; SCHED-C1e, since 0.12.54; Alice, 11-09 17:42:
  「这个红线会随着时间由外部修改」). A person
  or an agent sets one instance's lines of a pool while the Runtime runs through the Lab's
  `config set`, and clears them with `config clear` (the rows of
  [../lab-operations.json](../lab-operations.json); the request `ConfigureSettings`,
  [../runtime-config.md](../runtime-config.md)), on the per-instance key
  `instance.<instance id>.scheduling.pool_lines` (in the configuration file the `instances[]`
  entry's `scheduling.pool_lines`; no default; settable). An existing input, so nothing waits for
  the resource-targets reader (#429 R-13a). Its value is an object of at most 32 members, each named
  by a pool id as the instance's `schedule.json` writes it (a composed copy's id without
  `@<alias>`; in catalog mode the catalog's id), whose value is `{"stop_line", "hard_line",
  "trigger_line"?}`, each line in the two forms above (a reference reads its fact in the pool's
  scope).
  - *Precedence.* For a pool the key names, on that instance, its set replaces the catalog's lines
    whole (a `trigger_line` it does not write is the capacity, not the catalog's); otherwise the
    catalog's lines, otherwise the defaults. The catalog's lines stay the default, and no other
    instance is touched.
  - *Checks.* The configuration's section check refuses, as `config_value_invalid`
    (`section_check`, `wrong_type`, `out_of_range`), a value of another shape, a member that is no
    pool identifier, a set without both lines, and `H < S ≤ T` broken where it can tell (two
    constants, or two references to one fact by their offsets). The capacity is read, so
    `S ≤ cap` and `T ≤ cap` are checked at each evaluation (`lines_invalid`). An instruction that
    names a pool the active generation does not give the instance, or gives without a declared
    valuation, is refused with the existing `config_value_invalid` and its problem `section_check`
    (`config_key` the instance's key; the developer text names the pool: SCHED-C1e, the refusal's
    JSON pointer into the value, `/<pool id>`, as a remedy row's, no outcome value): the host fills the
    section check's context with the instance's valued pools of the active generation, as it fills
    the backends, so no new code or token is added. A restore at start keeps such a value (its
    context holds no pools), and a value whose pool a later generation no longer gives the instance
    changes nothing while the pool is absent (the coordinator's answer 5 of 11-11, register §1.299).
  - *Tier.* MCP's `ac_config_set` sets the key at the tier of every key but the backends (the
    coordinator's answer 8).
  - *Lasting.* An instruction value is restored at every start ("Restore at start" there) and is no
    part of the catalog: it holds across restarts and catalog generations until it is cleared.
  - *Episodes.* A change ends no episode: the lines are read at the next evaluation, and the episode
    ends only by its own rule against them ("Capped spending order").
  - *Who changed it.* Each change is recorded as every instruction is
    ([../runtime-config.md](../runtime-config.md), "`ConfigureSettings`"): the command's records
    (`cli.command` or its counter's own, `command.received`, then `command.validated`) and the new
    `host.config_overrides` row with its `set_unix_ms`, `request_id` and `channel`, the entry it
    came through (the Lab's command line or MCP, or the console).
  - *Seen.* `pool_line_held` and `capped_spend` carry the lines in effect, as today, and the new
    value `lines_source` (`instance`, `catalog` or `default`). SCHED-C1e: every reason that
    carries the lines (`stop_line`, `hard_line`) carries it, `pool_lines_unspendable` too; the
    holds that carry no lines (`capped_cap_unknown`, `capped_round_empty`, and the
    `capped_not_head` of a block candidate behind its dispatched head) carry none. The
    instance's set names the pool by its id in the instance's `schedule.json`: the evaluation
    takes it for the catalog pool of that id, or for the instance's copy `<id>@<alias>`. A
    resource target names its pool by the same rule (#503 GOAL-1, since 0.12.88;
    [../resource-targets.md](../resource-targets.md), "A target's pool").
  - It replaces the design's SS-2h-c (an optional member `pool_lines` of the resource-targets v3
    document through `ApplyResourceTargets`, waiting for R-13a; design
    `rd-1031/design-c424-redline-1109.md` §4.3).
- **A pool its lines cannot spend** (point 11). The released rule `H = S − (the costliest run)`
  makes every consumer fit from the cap, but nothing checks it today, and lines that break it can
  leave no consumer that fits. When `⌈C/1000⌉ > cap − H` for every judged consumer of a valued pool
  on an instance (each by its declared consumption, whether or not it is a candidate), no run can
  start from the capacity without leaving the pool below its hard line, and the pool stays full.
  Before SCHED-C1c each evaluation at the cap then began and ended an episode with the info reason
  `pool_line_held` `capped_round_empty`. With SCHED-C1c (since 0.12.53) it is a warning of its own:
  at an evaluation where the pool is at or above its trigger line, no episode begins (one under way
  waits), and its consumers are deferred with `pool_lines_unspendable` (warning; `pool_id`,
  `pool_capacity`, `stop_line`, `hard_line`, `least_consumption` the least `⌈C/1000⌉` among them;
  SCHED-C1e, since 0.12.54: `lines_source`).
  The status read shows it, as the not-run explanation every scheduling read answers
  (`ScheduleExplanation`: `actingctl schedule`, `ac_schedule`, `ac_overview`'s `not_running`,
  `ac_diagnose`'s `not_run`), and the idle plan counts it neither due nor as a wake of its own, as
  it counts `pool_line_held`. Where the compiler can tell (constant lines against a constant
  capacity, or a hard line that references the capacity's own fact, whose `cap − H` is minus its
  offset, against every consumer's integer consumption), the catalog also compiles with
  `pool_lines_unspendable` as a warning at the pool's `hard_line`, which reaches
  `config.catalog_warnings` and the overview ("Without a valuation" above).

## Capped spending order (SS-2h)

Since 0.12.46 (register §1.289). Alice's ruling 138 spends a resource at its cap by benefit,
highest first, the first round with the limited tasks too and later rounds with the repeatable
ones only; her answer 172 A ranks by the value of a run per unit of the resource; 173 (11-09
16:39) starts at the cap, at the start of the scheduler too, and freezes the order; 17:37 lists
only the tasks that can be dispatched now and never lets the cost keep one out before the order;
17:48 approved the layout inside the scheduler (乙, design §1.6 item 4). Only valued pools (a
declared `valuation` for the instance, the model's or a constant one the pack writes, "Pool
Valuation") and judged consumers (a `benefit_judged` task of the
instance that is no manual request and declares a consumption of the pool) take part: a catalog
without a valuation is untouched.

- **The episode.** When a pool's reading `r` (as in "Pool lines") reaches or exceeds its capacity
  on an instance, a capped episode begins there; one already above it at a start begins at the
  first evaluation. It ends once `r ≤ S` and `r < cap`, when a round lists or plans nothing
  though a consumer is a candidate (`capped_round_empty`, on every consumer's decision), when the
  catalog generation changes, or when the lines or the reading give no value. While no consumer
  is a candidate at all (each in its cooldown, running, or held by its own conditions) nothing
  could be dispatched in any case: the episode waits for its next round. A new episode begins at
  the cap again; each counts from 1 since the Runtime started (`capped_episode`).
  SCHED-C1c (register §1.299, since 0.12.53): the episode begins at `r ≥ T`, the pool's trigger
  line ("Pool lines"; the capacity without one, as before), and ends once `r ≤ S` and `r < T`; a
  pool its lines cannot spend begins none ("Pool lines"). SCHED-C1d (register §1.299, since
  0.12.53): it no longer ends on a catalog generation that leaves the pool's consumers unchanged,
  or on a capacity, a reading or a line's fact that gives no value, and it outlives a start, so its
  number counts on from the one the start kept ("Episodes survive" below).
- **A round** lists the pool's judged consumers that are candidates of the evaluation now (every
  condition the policy holds passed: the trigger with its counts and preconditions, the feedback
  stop, the cooldown, the no-progress guard, the placement and the score stage) whose value
  without the pool's cost `V = S + C·W/(1000·Q)` is above that value's rounding bound (SS-2d's `ε`
  without the pool's term); never by their cost. #455 B1 (since 0.12.82): a task with a fixed value
  has its goal terms in `S` and their bounds in `ε` ("Fixed values written in the pack", "A goal on
  what it produces"), so of two consumers of one value and one cost the one that produces a pool
  a goal lacks is listed with them and frozen first. From the second round on, a task marked
  `benefit_limited` ([v2/README.md](v2/README.md)) is not listed. The list is frozen for the round
  by `V ÷ C`, the higher first (compared exactly), then by task id: a value that changes during the
  round changes the order at the next round only.
- **The plan.** From the reading at the round's start, the order is taken in turn: while the
  accumulated reading `R` is above `S` (or at the cap), a consumer whose run would leave
  `R − ⌈C/1000⌉ < H` is skipped (`capped_would_cross_hard_line`, `amount_after`) and the next tried;
  one that fits is planned and `R` falls by its consumption. So the last one may take the reading
  past the stop line but never below the hard line; with `H = S − (the costliest run)` nothing is
  skipped. The plan is taken again at each evaluation of the round, from the same starting
  reading: one dispatched in the round counts with its consumption, one passed (below) not at all.
- **Dispatch, one at a time** (layout 乙). At each evaluation the head is the first planned
  consumer the round has neither dispatched (a task snapshot whose `last_dispatched_unix_ms` is at
  or after the round's start counts as dispatched in it) nor passed (below) that is a candidate
  now. Before it is dispatched the reading is read again: when the head would leave
  `r − ⌈C/1000⌉ < H` the round ends; when every planned one ran or was passed it ends too; while
  `r` is above `S` the next round starts at once. The head and the planned candidates after it
  that fit above `H` form the pool's block, followed by its fallbacks (below): in the allocation
  the block stays together at its head's place (its tier, `B ÷ E` and every key after, the head's
  `B` being `V − W·(c − a)`, `a = min(c, max(0, r − S))`: only the part of the consumption that
  takes the reading below the stop line costs), and the first of it that every later gate admits
  (the reserve gate, the shared run limits, every pause, hold, blackout and budget of the Runtime)
  is dispatched with `capped_spend` (info; `pool_id`, `pool_capacity`, `stop_line`, `hard_line`,
  `capped_episode`, `capped_round`, `capped_rank` its place in the round's order,
  `pool_reading`; SCHED-C1c, since 0.12.53: `trigger_line` when the pool has one; SCHED-C1e, since 0.12.54: `lines_source`). A block candidate a later gate defers keeps that gate's reason and the next one
  is tried; one behind the dispatched one, or behind its head when a more urgent candidate took
  the instance, waits with `pool_line_held` `capped_not_head` (`task_id` the head), a fallback
  with the hold the round gives it otherwise (`capped_not_head`, or
  `capped_would_cross_hard_line` when the plan skips it). A more urgent tier, and a business
  candidate of a higher `B ÷ E`, go first as always.
- **What the head's run is handed** (#477 LF-1, since 0.12.80; register §1.509; the
  coordinator's rulings of 11-22, not the owner's words). The plan counts one run of each planned
  consumer. A package whose run repeats its turn reads how many from `run_cap`
  (`session.dispatch.schedule.run_cap`, [../remedies.md](../remedies.md), section 4), and for
  the pool of the block it is handed the turns the plan would take with that consumer alone, run
  by run: from `r`, the lines' gate's own reading ("Pool lines", "At each evaluation": the
  projected amount less every task's consumption the reading does not show yet, the consumer's
  own included), as many runs of `c` as bring the reading to where the episode is spent
  (`r ≤ S` and `r < T`, the episode's rule above), and never one that would end below the floor
  (the hard line, or the task's reserve of the pool where that is larger):
  `min(⌈(r − s) ÷ c⌉, ⌊(r − floor) ÷ c⌋)`, `s` the largest reading the rule reads as spent: `S`,
  or `T − 1` when the stop line is the trigger line (one run that leaves exactly `S = T` does not
  end the episode). Every candidate of the block is handed it, since the one the order dispatches
  is the head of what it dispatches. Below the trigger line with no episode under way the turns
  are `⌊(r − floor) ÷ c⌋` ("Task Reserves", "Spend values"). For example, `S` 140, `H` 104, `T`
  the capacity 180, read at 180, `c` 15: ⌈40 ÷ 15⌉ = 3 runs (to 135; ⌊40 ÷ 15⌋ = 2 would stop at
  150 and leave the episode under way), the floor allows ⌊76 ÷ 15⌋ = 5: 3. With `S` = `T` = 160
  read at 175: ⌈(175 − 159) ÷ 15⌉ = 2. The order, the plan, the block, every hold and the
  episode's rule are unchanged: only the values on `policy_dispatch_parameters` are. After the
  head's run the pool is unknown until it is read after that run's end ("Pool lines", "At each
  evaluation"): the episode waits ("Episodes survive" below), and the first evaluation that reads
  the pool again judges the episode and its next head from that reading. A candidate in the
  blocks of two pools is the head of the first pool that takes it (pools in id order, the one
  whose `capped_spend` it carries); for the other pool its turns are `⌊(r − floor) ÷ c⌋`, which
  may take that pool past its stop line but never below its floor.
- **A stopped consumer** (Workflow #429 R-6b, since 0.12.55): a consumer whose run a
  `StopInstanceRun` stopped was dispatched in its round (its `last_dispatched_unix_ms`), so the
  plan counts its whole consumption and the round does not list it again; what it spent before
  the stop shows only from the pool's next reading, since a stopped run, as a failed one,
  publishes none ([../resource-readings.md](../resource-readings.md)), and until then the head
  after it is checked against the last reading.
- **Only what can be dispatched now** (SS-2h fix 1; Alice, 17:37: 「只加入现在空闲且能被调度/执行的
  任务进入排序」). A round lists only the candidates of the evaluation, so every condition the policy
  judges before the order holds for each when it is listed. The gates judged after it, the reserve
  gate and the Runtime's admission (the task's and its profile's daily, window and runtime budgets,
  the package's suspension and schedule, the quiet before a blackout and the day-boundary fit, a
  remedy round), are known only at the allocation, so:
  - **Passed.** A block candidate whose work entry such a gate defers is passed for the round,
    with the registered code of that gate's reason: the round's plan, taken again from the
    round's starting reading, leaves it out (it consumes nothing), so the next one of the order is
    planned in its place. While it is a candidate in a later evaluation of the round it is held
    with `capped_not_dispatchable`, `gate_code` that code. The next round lists it again (from the
    second round on only when it is not `benefit_limited`). Not passed, and so waited for: a
    candidate on an instance a whole-instance hold holds as the cycle starts (a control gate, a
    held instance or its takeover cooldown, an active blackout, the Runtime's positions 1, 2 and 4
    that `FactRefreshInputs::instance_holds` carries, or UPD-1's hold): nothing could be dispatched
    there, and the round waits; one whose dispatch only waits (the Runtime's random delay, the
    chain handover's `successor_wait_codes`); one a chain step goes before; one the pools' lines
    hold.
  - **Fallbacks.** So that the next one is tried at once, the block goes on, after its planned
    candidates, with what the round would plan once every candidate of the block before is passed,
    taken the same way (candidates now, until one would end below `H`), again while one is added.
    A fallback is reached only when every block candidate before it is refused.
  - **The round ends** when no planned consumer it has neither dispatched nor passed is a
    candidate now while a consumer of the pool is (one whose trigger turned false, for example):
    those left say why (`capped_not_dispatchable`, `gate_code` their own reason), and the next
    round starts at once while `r` is above `S`. So a consumer that cannot be dispatched never holds
    the pool at its cap.
- **The others are held** with `pool_line_held` and the episode's `capped_episode` and
  `capped_round`: a consumer of the order not in the block (`capped_not_head`, or
  `capped_would_cross_hard_line` when the plan skipped it), a limited one from the second round on
  (`capped_limited_spent`), one listed after the order was frozen or of no value without the
  pool's cost (`capped_not_head` when it would pass, else its own reason), one the round passed
  (`capped_not_dispatchable`, `gate_code` the code it was refused with), and a consumer that is
  no candidate of the evaluation (`capped_not_dispatchable`, `gate_code` its own last reason when
  registered: no run left, a precondition, a cooldown); it is listed again at the next round.
- **The memory.** The episodes, their round numbers, frozen orders and passed members are the
  Runtime's (`CappedMemory`, passed to `evaluate_with_capped_spending`), as the no-progress
  guard's are ("No-progress guard"): no decision identity holds them. Every other entry point
  evaluates with an empty memory. Before SCHED-C1d a start began with none, and a pool still at or
  above its cap then began a new episode; since it, a start keeps them (below).
- **Episodes survive** (SCHED-C1d, register §1.299, since 0.12.53; the coordinator's point 9 and
  its correction of 11-11, not Alice's words). An episode, with its number, its round number, its
  frozen order and the members it passed, outlives:
  - *a start.* Iron rule 13 (Alice: no separate on-disk file for program state; her rulings of
    10-09 20:2x and 20:5x: tagged ledger records, loaded into memory at a start, replace state
    files) keeps the memory as the tagged program fact `host.capped_episodes`
    ([../runtime-fact-store.md](../runtime-fact-store.md)), as `host.idle_state` is kept; the first
    text of SCHED-C1 named a state file of its own, which contradicted the rule, and the
    coordinator corrected it on 11-11 (register §1.299, answer 2). The host records the fact
    through the Runtime's fact store, so the ledger records it as every program fact, after each
    evaluated cycle that changed an episode, and once by a start that found it, under its own
    anchor; a Runtime that never has an episode records none. A start reads it before the first
    evaluation and keeps it by the rule of `host.idle_state` (#405 N-2): only when the start that
    recorded it, which the fact's writer row names by its start anchor, is the last start that
    completed its settlement (the anchor `host.contained_settlement` named before this start). A
    fact another start recorded (a start that kept none ran in between, such as a rollback's older
    build) is stale; an owner that settled no start in between, such as an offline ledger
    maintenance, leaves it current. What the start cannot keep is dropped and reported,
    `capped_state_failed` (warning; `operation` `load_capped_episodes`, `fact_key`
    `host.capped_episodes`): a value that does not read back as the fact's shape or breaks its
    rules (the whole value), a stale one (the whole value), and an episode whose instance is not
    registered or whose pool the active catalog does not give a declared valuation (that episode
    alone, with `instance_id` and `pool_id`); that pool begins without an episode, as before. A
    record the fact store refuses for this fact alone is reported the same way (`operation`
    `store_capped_episodes`, the refusal as the `caused_by` link): the memory stays, and the next
    change records the fact again. Neither stops the Runtime or an instance; a fatal fault of the
    store or the ledger is fatal as for every fact. Which tasks a round ran is still read from the
    task snapshots, which a start restores.
  - *a new catalog generation that leaves the pool's consumers unchanged*: the same pool id on the
    instance, and the same consumers, each by its accounting identity (its id, or one its
    `continues_from` names), its `⌈C/1000⌉` and its `benefit_limited` (the consumers key, which an
    episode keeps in place of the catalog hash). A changed value, valuation, line or other task
    keeps the episode: the round's frozen order stays, and the next round orders by the values
    then; a consumer that now runs under another id that continues its kept one takes that id in
    the order. Any other change of the consumers ends it, as every generation did before; a pool
    whose id changes (a second instance of its server bound) is another pool.
  - *an evaluation where the reading, the capacity or a line's fact gives no value* (an expired
    reading, a reset): the episode waits; while it waits its judged candidates are held with
    `pool_line_held` `capped_cap_unknown`, whether or not the pool writes lines, and when the values
    come back it goes on, or ends by its rule. Lines read that break their rules end it, as before.
  - *a change of the instance's lines* ("Pool lines", "Lines for one instance"; SCHED-C1e, since
    0.12.54): the next evaluation reads the new lines, and the episode ends only by its own rule
    against them; clearing them returns the pool to the catalog's lines the same way.

  It is dropped when it ends by its rule (`r ≤ S` and `r < T`, or a round that lists or plans
  nothing though a consumer is a candidate), when a generation changes the pool's consumers, when
  the instance or the pool leaves the catalog or the pool loses its declared valuation, and when a
  start does not keep it.
- **What a person sees.** A resumed episode's decisions carry its kept `capped_episode` and
  `capped_round` (`capped_spend`, `pool_line_held`), and the next episode of its pair counts on
  from its number; a waiting one's consumers carry `pool_line_held` `capped_cap_unknown`, which
  every scheduling read shows as the not-run explanation (`ScheduleExplanation`: `actingctl
  schedule`, `ac_schedule`, `ac_overview`'s `not_running`, `ac_diagnose`'s `not_run`), as any
  `pool_line_held`. The rows of `host.capped_episodes` are read with `actingctl facts --program`
  and `ac_facts`; each `capped_state_failed` is a Warning `runtime.failed` in the ledger
  (`actingctl events`, `ac_events`) and a `WARN actingd:` line.
- **Old triggers on a capped pool** (SCHED-C1c, since 0.12.53, in the composer; the coordinator's
  point 12, after point 9 of the ten points of #428 comment 6735960127). A consumer whose trigger
  requires, through `all`, a reading of the pool of at least its capacity less a margin (a
  `resource_projection` on the pool, or a `fact` on its reading key, compared
  `greater_than_or_equal` or `greater_than` with a fact reference to the pool's capacity fact)
  repeats what the capped order does once the pool is valued and writes its lines: the episode
  begins at the trigger line and spends down to the stop line by itself. Such a threshold at the
  stop line repeats it; above it, it ends the rounds early (the consumer stops being a candidate
  and the episode waits) and keeps the pool between the threshold and its cap; below it, it only
  limits full-price runs, which the hard line already guards. The evaluator reads the trigger as
  written; the composer reports each such rule as redundant
  (`standard_pack_line_trigger_redundant`, [../standard-pack-schedule.md](../standard-pack-schedule.md),
  "Tools").
- **Unchanged.** Every other gate and its order, the triggers, the reserves, the shared run
  limits, pauses, blackouts (a time window is only a blacklist), SS-4, the maintenance tier,
  SS-2f's turns across instances and the one dispatch per instance. A catalog without a valuation,
  lines and `benefit_limited` decides, records and hashes byte for byte as before; ruling 116's
  stop holds (no production pool declares a valuation). SCHED-C1: a catalog without its members
  (a fixed value, `trigger_line`) and an instance without `scheduling.pool_lines` decide as before;
  only the episodes' survival changes what an existing valued pool does. SCHED-C1e (since
  0.12.54): the reasons that carry a valued pool's lines carry `lines_source` too, the one value
  they gain.

## Reset exclusions and the task's own last result (Workflow #411 P19)

A daily reset that clears a family of readings must not clear the ready times in that family, which are absolute times that expire by themselves, and a reset must not make a chain step forget its own last run (#411 6455013768 items 1 and 2; since 0.12.25).

- **Excluded prefixes.** A V2 timeline event may carry `"excludes_fact_prefixes": [P, …]` ([v2/README.md](v2/README.md)): key prefixes it does **not** invalidate. An occurrence of the event invalidates the observation of a key when the key starts with one of its `invalidates_fact_prefixes` and with none of its `excludes_fact_prefixes` ("Runtime Enforcement"); this holds for facts, for `outcome.<task_id>.<outcome_key>` and for `task.<task_id>.terminal_state` alike. An excluded key keeps its observation through the event and expires only by its own `expires_at_unix_ms` (and a predicate's `max_age_ms`), and its expiry is not capped at the event's next occurrence. For example `"invalidates_fact_prefixes": ["resource."], "excludes_fact_prefixes": ["resource.neutral.ready."]` clears every `resource.` reading at the reset except the ready times below `resource.neutral.ready.`, and a `resource.` key added later is cleared without a change to the event.
  - The member is optional; absent, the event and its catalog serialize and hash exactly as before. It holds at most 128 entries, each an identifier (`type_mismatch` at the entry otherwise) that starts with one of the event's `invalidates_fact_prefixes`: an entry below none of them would exclude nothing and is refused as `dangling_reference` at the entry. A V1 document that writes it is refused as `unknown_field` at the member.
  - The event still wakes the evaluation at its occurrences, as before; a wake that clears nothing is harmless.
- **The task's own last result.** A reset that invalidates `task.<task_id>.terminal_state` clears the task's settled state for everything that reads it as a result of the current period: the window result before `feedback_stop` ("Runtime Enforcement") and a `dependency_completed` of **another** task that names it, which is Unknown (`dependency_state_missing`) until the task settles again. It never clears the task's own last result, which the task's own edges read:
  - a `dependency_completed` whose `task_id` is the task itself reads the task's latest terminal state on the instance, as settled, whatever reset came since;
  - `since_own_run` ("Condition edges") reads this task's own latest run that way, so a reset of the successor alone does not make an edge it already served due again;
  - `once_per_reading`'s served check ("Condition edges") reads this task's own latest run that way, so a reading the task served stays served through a reset, until a newer reading.
  - The latest dispatch, which `clock` and the cooldown read, was never cleared by a reset and is unchanged.
  - **The class of rule this changes.** A rule that uses a `dependency_completed` on its own task as the result of the current period, for example a retry branch on its own `failed`, now reads the task's last result across a reset, so after the reset it sees a result from before it. A retry branch that must fire only within the current period needs, as its period gate, a `dependency_completed` on an earlier task of the same period, which the reset clears.
- **Capability.** A Runtime that reads `excludes_fact_prefixes` and keeps the task's own last result reports the always-on subsystem `reset_exclusions` in `config.subsystems` ([`actingd-check-config.md`](../actingd-check-config.md)). A `schedule.json` whose event writes the member must list `reset_exclusions` in `requires` ([`standard-pack-schedule.md`](../standard-pack-schedule.md)); the composer refuses the file without it, so a Runtime that does not name the subsystem refuses the file (`schedule_requires_missing`) instead of misreading it. A `schedule.json` that relies on a reset keeping the task's own last result names it as well: nothing in the file shows that reliance, so the composer cannot require it. An older Runtime given a catalog whose event writes the member refuses it at its reader (`unknown_field`).

## Self-read fact probe (SS-5)

A fact that only the task's own package reads (a clear count, an activity level, a slot count: a reading mark of the task's package) must never gate that package forever (the coordinator's rule of 10-28; Alice's ruling 100, dispatch follows live account state; since 0.12.32, register §1.223). Missing or expired, such a fact makes a `fact` predicate Unknown, the recognition maintenance package cannot read it ("Fact Refresh"), and only a run of the task would: a first-run deadlock. A `fact` predicate may therefore carry `"on_unknown": "probe"` ([v2/README.md](v2/README.md); `actingcommand.scheduling.v2` only). Absent, the predicate evaluates, serializes and hashes exactly as before.

- **Known fact.** A fact that is present, above confidence 0, fresh (`max_age_ms`, its own expiry) and of the compared kind is compared as for every `fact` predicate; the member changes nothing.
- **Unknown fact, the period.** The fact is unknown for the reasons of a `fact` predicate: `fact_missing`, `fact_low_confidence`, `fact_stale`, `fact_expired` or `type_mismatch`. Its **unknown period** began at the later of (a) the latest occurrence, at or before the evaluation, of a timeline event whose `invalidates_fact_prefixes` clears the key in the predicate's scope ("Runtime Enforcement", with "Reset exclusions" above), and (b) the end of the freshness of a reading past it (the earlier of its `expires_at_unix_ms` and `observed_at_unix_ms` + `max_age_ms`). Without either the key was never cleared, and the period is open-ended.
- **Unknown fact, not served: true.** When this task's own latest run on the instance (its own last result, which no reset clears, "Reset exclusions" above) did not succeed, or was dispatched at or before the period began, the predicate is **true**, with the info reason `fact_probe` {`fact_key`, `unknown_reason`}. The task is admitted as for any true trigger (placement, gates, budgets and admission unchanged), and its run reads the fact. No detection suggestion is given and no fact refresh is asked for the key.
- **Unknown fact, served: false.** When that run succeeded and was dispatched after the period began (or at all, when the period is open-ended), the probe was spent: the predicate is **false**, with `fact_probe` {`fact_key`, `unknown_reason`, `served_by_dispatch_unix_ms`} and no wake. A reading of the key (whose publication wakes the scheduler), the next reset that clears it (an occurrence is a wake already) or the expiry of a later reading begins a new period. When the predicate decides the trigger, the decision ends with `trigger_false` after its reason.
  - A run that failed or was cancelled served nothing: the task stays due, and its cooldown, its retry and backoff and its daily limits bound how often it runs again, as for every failure.
  - The probe's run publishes the reading; a known value then decides as usual. A `remaining` of 0 makes a `≥ 1` gate false, and the task yields to the instance's other work.
- **Other unknowns ask as before.** A non-probe `fact` that is unknown in the same trigger keeps it Unknown (`trigger_unknown`), and the fact refresh asks the maintenance package for that key alone ("Fact Refresh"); the probed key is never requested, even when the maintenance package marks it too.
- **Placement, compile time.** A probe names a fact only the task's own package reads, so the compiler refuses, each `predicate_uncomputable` at `on_unknown`: a scope other than an instance (a reading writes the instance's scope only); a `fact_key` that does not start with `resource.` or `inventory.` (the keys a reading mark writes, [resource-readings.md](../resource-readings.md)); and a probe where its truth would not admit the task, below a `not` or in the `feedback_stop`. A V1 document that writes the member is refused as `unknown_field` at the member.
- **Placement, against the package.** Where a rule meets its package (`actinglab standard-pack check`, `package bundle --schedule`; [standard-pack-schedule.md](../standard-pack-schedule.md)) the key must be one of the reading-mark keys of the rule's own package; a key only another package reads, or none reads, is refused there (`standard_pack_root_invalid`, `schedule_invalid` at `on_unknown`).
- **Capability.** A Runtime that reads the member reports the always-on subsystem `fact_unknown_probe` in `config.subsystems` ([`actingd-check-config.md`](../actingd-check-config.md)). A `schedule.json` whose rule writes it must list `fact_unknown_probe` in `requires` ([`standard-pack-schedule.md`](../standard-pack-schedule.md)); the composer refuses the file without it, so a Runtime that does not name the subsystem refuses the file (`schedule_requires_missing`) instead of misreading it. An older Runtime given a catalog with the member refuses it at its reader (`unknown_field`).
- For example, a task due while its own count of remaining clears is at least 1, and admitted once per daily reset while the count is unknown: `{"kind": "fact", "scope": S, "fact_key": "resource.neutral.remaining", "comparison": "greater_than_or_equal", "value": {"type": "integer", "value": 1}, "max_age_ms": null, "on_unknown": "probe"}`, under a reset whose `invalidates_fact_prefixes` covers the key.

## No-progress guard (SS-4)

Once run counts are optional ("Optional run-count bounds") they no longer stop a task that keeps succeeding without changing anything, so a state guard does (Alice's ruling 105 A: no count-based last line; the no-progress guard and the failure retry cap replace counts; design `rd-1029/design-sch-state.md` §4.7; since 0.12.34, register §1.231). It is an edge, the generalization of `once_per_reading` ("Condition edges"), never a count: when a package spends nothing, a reading is broken or the game does not respond, the task runs at most once more and then waits for a changed reading.

- **The basis.** The basis of a dispatch is the pools, stocks, remaining counts and completion times its admission was decided on, as the evaluation that chose the dispatch held them (after the resets, "Runtime Enforcement"). The trigger is read again part by part as that evaluation read it, and the parts that made it true count: every child of a true `all`, the first true child of an `any` in its order, and below a `not` every fact of the negated predicate. Of those parts: each `resource.` or `inventory.` fact (the families recognition writes, [resource-readings.md](../resource-readings.md)) a `fact` or `record_deadline` names, and a fact reference's ("Fact references"); each fact a `fact_time_reached` names, whatever its family; and the reading, capacity and comparand facts of each pool a `resource_projection` projects ("Pool fact references"). Beside them, the reading and capacity facts of each pool the task reserves, which the reserve gate reads at every admission ("Task Reserves"). Such a pool read from rows of a list (#455 P2 fix round 1, since 0.12.79, register §1.505; "A pool read from rows of a list") is read by its list's key in its instance, and its reading is the sum the host computed for it, the reading the evaluation was given (when several pools read the one list, each one's sum, as rows `{pool_id, value}` in the order of their ids); a key no fact holds is read from those readings, at capture, at release and at the hold. A fact of another family (a lock, a health flag, a session value), the feedback stop and a pool the task only consumes or produces are no basis. Each reading is the value present above confidence 0, or none. A dispatch decided by a `clock`, a `timeline_active`, an `outcome` or a `dependency_completed` alone (a clock beside a lock; an `any` whose due clock comes first), or one whose trigger is not true as captured (an immediate rerun, a remedy request), has no basis and no guard: those are edges of their own. A basis of more than 32 keys arms none.
- **Armed.** The Runtime captures the basis of each new dispatch intent as the evaluation that chose it read its facts, and arms the pair's guard when that intent is admitted, replacing the pair's earlier one. Memory only: no ledger record, state file or snapshot carries it.
- **Released.** At every evaluated cycle, before the evaluation, a guard one of whose basis keys has a reading whose value differs from the admission's reading (a reading where the admission had none counts) is released, and stays released: a later reading of the old value does not arm it again. Only values are compared: a newer reading of the same value, such as a fact refresh that reads the same stock again, is no progress, and a key a reset cleared, which reads nothing, releases nothing. **Released by another cause** (SS-4b, since 0.12.55, register §1.310): at the evaluation that would hold the pair (below), its trigger is read again part by part as at capture, as it is true now, which gives the basis keys of the trigger as it is true now (with the reserve gate's keys, as at capture). When that basis is empty (the trigger is true now by a `clock`, a `timeline_active`, an `outcome` or a `dependency_completed` alone, a clock slot for example), or holds a key the armed guard's basis does not hold, the pair is due for another cause: it is **not held**, and its guard is released and stays released, as a changed reading releases it; the pair's next admitted dispatch arms a new guard on the basis of that dispatch.
- **Held.** A (task, instance) pair whose guard is armed, whose own latest run (its own last result, which no reset clears, "Reset exclusions and the task's own last result") is the guarded dispatch and **succeeded**, whose catalog generation is the one that dispatch pinned, and which is due **for the cause the guard served** (every key of the basis of its trigger as it is true now, read as under "Released by another cause", is a key of the guard's basis, and, as above, none of them reads another value) is deferred with `policy_no_progress` (info) {`basis_readings` one record per basis key: `fact_key`, and the admission's reading as `reading_value` (its canonical JSON, at most 128 bytes) or `reading_digest` (its SHA-256 when longer) with `observed_at_unix_ms`, nothing more when the key had no reading; `served_by_dispatch_unix_ms` the run's dispatch}, with **no wake**: a reading wakes the scheduler. The bound is 32 records, about 13 KiB at most; real rules name a handful of keys. An operator therefore sees `policy_no_progress` on a pair only while the pair is due for the cause its guarded run was admitted on; once it is due for another (a new completion time, a count, a clock slot), it is a candidate again and its decision reads as any other.
- **Why a new cause is not held** (SS-4b). The guard stops a run that changed nothing from being repeated for the same reason; a cause it did not serve is no repetition. Nor does it reopen the loop the guard exists for: a package that spends nothing, a broken reading or a game that does not respond leaves the same basis keys with the same readings, so the pair is still held after one more run, a clock beside its reading in an `all` included; a new cause gets one run, and that run arms its own guard.
- **Where it sits.** After the trigger, the feedback stop and the task cooldown, before placement, so a held pair is not a candidate: the net-benefit gate ("Net benefit within a tier"; judged and unjudged pairs alike) and the Runtime's deferrals ([`scheduling-blackout.md`](../scheduling-blackout.md), "Order of the deferrals") never see it, and its decision ends with `policy_no_progress`, which `ac_schedule`, the not-run explanation and the idle plan read ("Why A Candidate Does Not Run"; the idle plan counts it neither due nor as a wake of its own). A trigger that is Unknown still raises its fact refresh demand ("Fact Refresh"), so the reading that releases the guard can come.
- **Failures are unchanged.** A run that failed or was cancelled is never held: its retry, backoff and escalation (`on_failure`) apply as before. An immediate rerun, a remedy request or retry and an owed rerun are never held either. The guard only covers "succeeded but no progress". A run a `StopInstanceRun` stopped (Workflow #429 R-6b, since 0.12.55) is not held either: its admission armed the pair's guard as every admission does, and the guard holds only a guarded dispatch that succeeded.
- **Self-read probes** ("Self-read fact probe"). A probe run is admitted on no reading; the reading its run publishes (for example `remaining = 0`) is a reading where there was none and releases the guard, and the known value then decides as usual. A probe run that published nothing is spent for its period by the probe itself; after the next reset the probe would admit the task again, but no reading came since its admission, so the guard holds it with `policy_no_progress`, its key named with no admission reading, until a reading comes.
- **Chains.** A head held for the cause its guard served is not dispatched, so a successor whose trigger waits for the head's next terminal is not triggered either; the chain waits with its head. A head due for another cause (a clock slot, a completion time its guarded run was not admitted on) is not held: it runs, and its chain follows it. Before SS-4b a head held for one cause waited through every later cause, a clock slot included, until a reading of its basis changed, which only its own chain might have read.
- **Pauses.** The guard queues nothing and counts nothing: a paused instance dispatches nothing (its candidates wait under the pause as before), and once resumed each pair is held or not by its own guard.
- **Configuration.** `scheduling.no_progress.enabled` (default `true`), also per instance ([`runtime-config.md`](../runtime-config.md), "No-progress guard"); read at every evaluated cycle. Off on an instance, the evaluation of its pairs is the one before SS-4, the Runtime captures nothing for it and forgets its guards, so turning it off and on releases them. A Runtime start begins with no guard (each pair may run once more before its guard is armed again), and so does a new catalog generation for the pairs it decides.
- **Entry point.** The policy crate's `evaluate_with_no_progress` takes the armed guards (`NoProgressGuards`) beside the instance holds; `NoProgressMemory` is the Runtime's memory (`capture`, `admit`, `observe`, `guards`, since #455 P2 fix round 1 `capture_with_readings` and `observe_with_readings`, which the Runtime calls with the readings it gave the evaluation, and since SS-4b `forget_released`, which the Runtime calls after every evaluation with the guards it passed, so that a guard the evaluation released for another cause is gone). With no guard the evaluation is `evaluate_with_instance_holds`'s, byte for byte, and a pair held for its served cause decides as before SS-4b.

## An unavailable instance is probed again (Workflow #489 AVAIL-1, since 0.12.76)

Register §1.493. A physical policy instance whose device the Runtime self-checks is available to
the scheduler only while its stored self-check passes (Workflow #317 sc3): the backend self-check
gate publishes `session.instance.available` `false` (`backend_selfcheck:<entry>:<generation>`) when
a recorded open fails its check, and republishes the configured seed when a later one passes. The
evaluation blocks every candidate of an unavailable instance at placement (`instance_unavailable`,
`availability_source` `self_check`), so nothing is dispatched to it, and before this slice nothing
else ran its self-check again: a self-check runs only in a run's, a rung's or a preparation's opens
(a daemon start, an emulator control, a person's self-check or resume, a ladder, a fence's recheck),
and a ladder its failure matched could be suppressed by an open cool-down window, which drops it and
schedules nothing for the window's end. Two instances stayed unavailable for 15 and 16 hours on
11-20 although a self-check passed at once when a person ran one.

- **Which instances.** The unavailability probed is the device self-check's: one the backend
  self-check gate holds unavailable (its withdrawal `backend_selfcheck:…` in effect) while its
  configured seed is available and no fence, device link, device state or device system holds it,
  after (a) a device self-check that failed (its `device.self_check` record with
  `failure_code`; `track_unavailability`, called by every decision of the gate), or (b), fix round
  1 (the coordinator's rulings R1 and R5 of 11-20), any other failing step of a connection
  preparation, which withdraws it as `backend_selfcheck:unchecked`: a close of the kept session, or
  the close after the opens, that failed with its resources confirmed released; an open that failed
  without a report (an instance with an explicit host and port and no discovery binding included,
  which no emulator watch reads); the preparation's own lease refused
  (`withhold_after_failed_preparation`, `runtime_facts.rs`, called by `prepare_connection_attempt`
  and `record_connection_preparation_failure`, `resource_close.rs`). The step's code is the
  instance's cause, the code that withdrawal already has; no word is added. From there on nothing
  differs. No `passed` self-check stands for such an instance (R2): the same call drops its
  `backend.selfcheck.*` facts first unless an entry says `failed`, so the next recorded self-check
  alone makes it available, and a probe that passes publishes its own facts, its `device.self_check`
  records and the gate's republication of the configured seed. Fix round 2 (item 3, the
  coordinator's ruling 甲 of 11-21): also (c) a recorded open whose report leaves an entry
  `unknown` (its `status`, `connection` or its own check neither `failed` nor all `passed`), after
  which the gate withdraws the instance under `backend_selfcheck:<entry>:<generation>` with no
  failure code (`device_self_check`, `runtime_facts.rs`): it enters with **no cause**, and no word
  is added or borrowed. Its probe records and its warning then carry no `failure_code`; what tells
  a reader why is the `device.self_check` record of that open (a `runtime.lifecycle_observed`
  event under the instance) with `status` `unknown` and no `failure_code`, which precedes them
  (its entry and generation are the withdrawal's snapshot id), and the overview row's `backend_selfcheck`,
  where that entry shows `unknown`. A later failed step's code becomes its cause as for any other.
  It stops being one when the gate no longer holds it so (a fenced instance is its fence's, a
  handset's link its link's). Fix round 2 (item 1): while a probe of the instance is queued or
  runs, a decision of the gate does not take it out of the set; the probe's settlement decides,
  reading the gate's hold under the gate's own lock (`settle_unavailable_probe`). A probe whose
  open passes (the gate republishes the seed) and whose later step fails (the close after the
  opens, with two Nemu instances; the gate withdraws it again) is one failed probe of the same
  unavailability, with that step's code as its cause, and its count, interval, `since`, attention
  and held recovery kept; the seed it republished stood only inside the probe's preparation,
  which holds the instance's key from the claim's grant until after the preparation's
  `instance_preparation_finished` (the key is released just before the settlement), so no
  candidate can be dispatched on it (a dispatch needs a free key). A
  probe whose preparation ended well leaves the set with `instance_unavailable_probe_passed`. A
  queued probe that does not run (a hold took the instance meanwhile) settles the gate's decision
  as the gate would have. A decision of the gate while no probe is queued or runs behaves as
  before.
- **Which do not enter, and who owns them** (R3; their paths are unchanged). The preparation's
  emulator status read (#411 A48, `read_emulator_status_first`) ends the check before any open,
  and such an instance is the emulator watch's and the idle wake's (`emulator_watch.rs`; register
  §1.493 for what bounds them):
  - *the emulator stopped* (`instance_not_running` at `read_instance_state`; also an emulator
    control's or an idle action's stop, which drops the facts, `invalidate_backend_selfcheck_facts`
    in `emulator_instance.rs`): the watch restarts a configured instance (`start_emulator` on,
    `watch_emulator` → `restart_watched_emulator`), the idle wake starts the one its action stopped
    (#486 ASLEEP-1, since 0.12.77: such an instance is asleep, not unavailable, while nothing else
    withdrew or holds it, "An asleep instance" below; the evaluation judges it as if it were
    available, and the wake comes for a task that would be dispatched),
    and once the emulator runs with Android and an ADB port the watch recognizes it (`needs_recognition`
    for a pending binding, `unprepared_on_reported_port` for one bound here without self-check
    facts), each request at most `emulator_control.restart_attempts` within `restart_window_ms`
    (`EmulatorWatch::restart_budget`);
  - *the state that could not be read while the binding waits* (`instance_not_running` at
    `require_bound_adb_endpoint`): the watch's own poll reads it again every `watch_interval_ms`
    and recognizes or restarts it as above once the read answers;
  - *a running emulator whose device is not recognized* (`emulator_device_unrecognized`): the watch
    recognizes it once Android reports its ADB port, as above.

  The daemon start's withdrawal before its preparation is decided again by that preparation (an
  instance whose pause stands `released` at the start, by the resume's reconnect); a backend
  switch whose restore the registration refused keeps the instance its failed candidate's
  preparation took into the probes.

  Fix round 2 (item 5): the watch's code is unchanged, but what it meets is not. Once R2 drops the
  self-check facts of an instance rule (b) took in, a discovery-bound one (the watch polls only
  those) that runs with Android on its bound port qualifies for the watch's own recognition
  (`unprepared_on_reported_port`: bound to the reported port, no `backend.selfcheck.*` fact, no
  idle hold, no pause in stage `released`, no fence), which an instance taken in by a failed open
  (its `failed` facts kept) or by an `unknown` one does not. When its key is idle (no claim holds
  or waits for it, `instance_key_idle`) and no pause holds it (WATCH-P), the watch requests one
  autostart claim marked `recognize`: on its key it reads the emulator status again and ends
  without preparing anything when the recognition is no longer needed (`recognition_unneeded`:
  facts recorded meanwhile, a probe's included); otherwise it starts and stops no emulator, binds
  the reported port, self-checks the instance (a preparation, as a probe) and queues the
  instance's startup package as after a start (a package run, when one is configured), which a
  probe never does. Its bound is the
  watch's own: `emulator_control.restart_attempts` recognitions within `restart_window_ms`,
  counted apart from restarts, then `emulator_watch_restart_exhausted` (`watch_kind` `recognize`,
  once per streak; no fence, no wake) until the oldest leaves the window. It and a probe never run
  on the instance at once: each is a claim on the
  instance's key and runs on its worker while it holds the key; the watch asks only while the key
  is idle and the sweep queues a probe only while it is free, and two claims queued at the same
  tick run one after the other, each checking again on its key.
- **The probe** is the device self-check `actingctl selfcheck` makes (a preparation on the
  instance's key: it closes a retained session, reads the emulator status first, opens the backends
  and checks capture and input), as the Runtime's own recovery preparation: no game input, no
  application launch, no emulator start, no ladder of its own, and direction ① counts none of its
  records. It runs on a self-check claim, which the lease sweep queues only while nothing holds or
  waits for the instance's key and no takeover cool-down runs, so it never waits in front of a
  client's, the Lab's or the scheduler's own work; it holds the key for the preparation alone.
  Like every preparation, a probe closes a session that a keyless Lab observation kept open on the
  instance (`prepare_connection_attempt`'s retained close, #191 h3); the instance's admission lock,
  which the preparation holds throughout and an observation takes, serialises the two, so neither
  meets the other halfway. A probe whose worker panics ends at the worker's root
  (`instance_worker_loop`, `end_panicked_unavailable_probe`; fix round 2, item 2, the
  coordinator's ruling of 11-21, which takes back fix round 1's "the schedule goes on"): the
  panic is recorded once, the worker's `panic_caught` (failure category `run_unavailable_probe`,
  under the instance); the instance's entry stays `Panicked`, from which the sweep queues no
  probe, so **nothing probes the instance again in that unavailability**, whether or not the panic
  poisoned anything; a person is told at once (below); the climbing mark comes off whatever state
  the entry is in; and a failure of this clean-up is returned only after the panic's own record
  and the fatal mark a second panic of the worker owes. The instance leaves that state as it
  leaves the set: a decision of the gate that no longer holds it (a person's self-check or resume
  whose self-check passes, any other preparation that passes, a fence, a device link or state, the
  configuration), or a restart (the set is memory). A person's self-check that fails keeps it
  there. The panicked payload's key lapses at its TTL, as every panicked payload's does.
- **A fact this slice does not change** (verified in fix round 2; board issue #496). A panic
  inside the preparation unwinds through the guard of the device's admission lock
  (`HostShared::instance_guard`, an `Arc<Mutex<()>>` per device key, held by the probe's
  `run_unavailable_probe` across `prepare_recovery_connection`) and poisons it. The next lock of it
  answers `runtime_state_poisoned` (the host's `lock` maps a poisoned lock to that fatal code,
  `runtime_halted`, a global shared-lock failure) and stops the Runtime: the emulator watch's poll
  of a discovery-bound instance (its `try_lock` maps `Poisoned` to the same code), or a later
  preparation, run or observation of that device. Two panics of one worker without a finished
  payload between them are `runtime_restart_required`. So a panic inside the preparation still
  stops the whole Runtime at the next touch of that device; the probes no longer add a touch.
- **When** (`scheduling.unavailable_probe.*`, [runtime-config.md](../runtime-config.md)):
  `first_interval_ms` after the instance became one, then after each failed probe the interval
  times `factor`, at most `max_interval_ms`; an interval counts from the latest of the probe before,
  the instance's latest recorded self-check or dropped self-check facts, and the latest sweep tick at
  which a claim held or waited for its key. A probe that passes makes the instance available (the
  gate republishes the seed) and ends the probes; a probe that fails is recorded with the next one's
  time.
- **A cool-down's end.** A ladder its cool-down suppresses (`recovery_ladder_suppressed`
  `cooldown`) while the instance is one is held with it, with the `until_unix_ms` its suppression
  names. From then on (no later than the sweep's next tick) a probe is due whatever its interval:
  passing, the instance is available and the held recovery is dropped; failing, the instance's
  cool-down window in force is read again (fix round 1: a later climb, or a `triggered` admission,
  may have opened one since): while it runs, the recovery stays held until its end, the next
  probe's time; once none does, the held recovery is admitted after the probe's record as it would
  have been without the cool-down (every other rule of its admission as for every ladder), as a
  new ladder of the same trigger with ids and a claim of its own, and its claim queued. The
  cool-down still holds a recovery back while it runs; a drill is refused to its person as before;
  `stuck_recovery_cooldown_secs` keeps its meaning.
- **The held recovery, as a contract** (fix round 2, item 4). At most one recovery is held per
  instance in the set (`HeldRecovery`: the suppressed ladder and the `until_unix_ms` it is held
  to). A later suppression of a ladder of the same instance by its cool-down replaces the held
  one. Only the settlement of a probe that failed releases it, and only once its held end has
  passed and no cool-down window of the instance is in force (otherwise its held end becomes the
  window's end, which is also the next probe's time). Released, it is admitted with
  `respect_cooldown` off and every other admission rule; a refused admission writes that refusal's
  own `recovery_ladder_suppressed` (`admission_denied`, `blackout` or `already_running`), and an
  admitted one `recovery_ladder_started`. It is dropped, never taken up, when a later suppression
  replaces it, when the instance leaves the set (a probe that passed, or any other decision of the
  gate that no longer holds it: a passing self-check of a person or a preparation, a fence, a
  device link or state, the configuration), when a scheduling pause (the instance's or the
  global one), a fence or an idle stop holds the instance at a sweep tick, and at a restart (memory
  only, nothing recorded). Each drop but a restart's writes one record: the dropped ladder's own
  `recovery_ladder_suppressed` `cooldown` once more (info, under that ladder's links, its
  `until_unix_ms` the end it was held to, its `rule` the trigger item that matched): the cool-down
  suppressed that trigger and nothing starts from it. No code, word or member is added. A trigger
  therefore ends in exactly one of: `recovery_ladder_started`, a suppression with another reason,
  or a second `cooldown` suppression. While the instance's key is never free no probe runs and the
  held recovery waits; that is not a drop. A probe that panicked releases nothing: the recovery
  stays held until one of the drops above.
- **What holds the probes.** A scheduling pause of the instance or the global one, in any stage and
  mode; a fence (its own recheck owns it, and the fenced instance is not one); an emulator the idle
  action holds stopped (Workflow #486 owns its wake); a blackout that covers the instance (complete
  silence; probed after it); the Runtime's stop. Each probe is checked again on the key. A pause, a
  fence or an idle stop also drops a held recovery.
- **A person is told** once per unavailability that lasted `attention_after_ms`:
  `instance_unavailable_awaiting_person` (a warning `runtime.failed` record under the instance,
  handed to the daemon's log too). It ends when the instance is available again; the probes go on
  meanwhile. Fix round 2 (item 2): after a probe's panic the same warning is written at once,
  unless this unavailability already told a person, with `failure_code` `panic_caught` (the code
  the worker's own record carries), `elapsed_ms` how long so far, the probes counted and no
  `next_occurrence_unix_ms`: no probe follows.
- **Records.** `instance_unavailable_probe_failed` (warning: the probe's number, the failure of
  its self-check or of a later step of its preparation when one named a code, the next probe's
  time), `instance_unavailable_probe_passed` (info: the number, how long it was unavailable, its
  cause when it had one) and `instance_unavailable_awaiting_person`, each a `runtime.failed`
  record; the probe's own preparation records what every preparation records; a dropped held
  recovery writes its trigger's `recovery_ladder_suppressed` `cooldown` once more (above).
- **Status.** The status answers nothing of the probes in 0.12.76 (fix round 1, the coordinator's
  ruling): `RuntimeInstanceStatus.unavailable` ([runtime-interface.md](../runtime-interface.md)) is
  declared (`runtime-client` 14) and filled for no instance, since a reader of 0.12.75 refuses a
  status that carries it; the release that answers it raises the floor to 14. Until then a person
  or a tool tells such an instance by what 0.12.75 already reads, in `actingctl overview`,
  `ac_overview`, `actinglab status` and `ac_status`: `not_running` (`why` `instance_unavailable`,
  `availability_source` `self_check`) for every instance in the set, and the overview row's
  `backend_selfcheck` (`{<entry>: {status, checked_at_unix_ms}}`, an entry only while its status
  fact exists), which differs by how the instance entered (fix round 2, item 5): after a failed
  open, that entry `failed`; after an open that left an entry `unknown`, that entry `unknown`;
  after any other failing step of a preparation (rule (b)), whose facts were dropped (R2), an empty
  object `{}`, and so between its probes, except right after a probe whose own open failed, which
  shows that `failed` entry until the next probe drops it. And the records above. No console shows
  the probes yet. Memory only: a restart starts the set again at the gate's next decision.

## An asleep instance (#486 ASLEEP-1, since 0.12.77)

Register §1.497 (the coordinator's rulings 1 to 8 of 11-21 on board #486; the owner's words behind
them, 11-02: 「没有任务时直接关闭模拟器」). An instance whose emulator its idle action stopped
(`stop_emulator`, or `auto`'s highest tier) loses its availability with the stop: the stop drops
its self-check facts and the backend self-check gate withdraws it (`backend_selfcheck:unchecked`).
Before this slice the evaluation blocked every candidate of it at placement (`instance_unavailable`),
before any later gate, so the idle plan counted such a candidate due whatever its budget, backoff,
benefit or pool lines said, and the idle wake started the emulator for a task the next evaluation
then held back; the idle check stopped it again, with no end.

- **Asleep.** An instance is asleep while its idle action stops or holds its emulator stopped (the
  idle planner's `stop_emulator` phases: asked for and not ended, its claim holding the key while
  the stop runs, and done, its wake not come; `IdlePlanner::emulators_held_stopped`) and nothing but
  that stop withdrew its availability: the availability in effect is the backend self-check gate's
  withdrawal (the projection's word `self_check`, so its configured seed is available and no device
  link, device state or device system withdrew it), no fence holds it, and it is no instance the
  probes above take up (a self-check or a preparation step of its own that failed). An instance the
  projection holds available is never asleep. The host gives the evaluation the asleep instances as
  the cycle starts (`HostShared::asleep_for_policy`, `InstanceHolds::mark_asleep`). No fact changes:
  `session.instance.available` stays `false`, and nothing is dispatched to it.
- **Judged as available, nothing selected.** The evaluation judges each candidate of an asleep
  instance as it judges an available instance's: the rest of placement, the order of the deferrals
  ([scheduling-blackout.md](../scheduling-blackout.md)), the benefit gate, the pool lines and the
  capped order, the no-progress guard, the ranking, and its recognition maintenance package's own
  gates. Each candidate carries the decision, reason and wake of the gate that holds it. The one
  candidate the round would select there is not selected: its decision is deferred with
  `instance_asleep_wake_due` (`instance_id` its alias) and no wake of its own (#486 ASLEEP-5,
  since 0.12.83; until then the evaluation's time, so the evaluation's own wake was now, "When the
  driver looks again" below). It takes its instance and its device for the round, so the
  candidates behind it wait as behind a selected one (`instance_already_selected`,
  `device_group_waiting` `selected`, `capped_not_head`); a recognition maintenance package picked so
  makes its waiting candidates name `instance_asleep_wake_due` as their gate, with no wake. What
  judging does short of selecting still happens (the admission's preview, a clock slot's random
  delay drawn and noted, a suspension's lift check); nothing a dispatch alone causes does: no
  dispatch intent, reason chain or dispatch decision record, no shared run slot taken, no chain step
  settled as dispatched, no `capped_spend` and no pass of its capped block for its round (its
  episode does not advance past it), no no-progress basis captured, no budget counted.
- **The idle plan** follows: the decision above is due and is the task the wake names
  (`idle_wake_cause` `task_due`); every other decision counts as on an available instance (a spent
  budget: not due, its wake the release). Fix round 1 (the coordinator's ruling 2 of the round): on
  an instance whose idle action in effect can stop its emulator (`stop_emulator`, or `auto` with its
  emulator start), a decision a gate without a time holds (the benefit gate, the no-progress guard,
  a pool's lines) is not due and names no wake, whatever next time its trigger names; on any other
  instance it names its own wake as before. The wake is the emulator's start, then the startup
  package ([runtime-config.md](../runtime-config.md), "What wakes an idle instance").
- **After a Runtime start** (fix round 1, ruling 5) the instance is asleep as before it: the start
  restores it stopped with its wake, withdraws its availability (`backend_selfcheck:unchecked`) and
  does not prepare it (its emulator is stopped, so a preparation could only fail and take it into
  the unavailable instances above), so its withdrawal is the gate's `self_check` and the probes do
  not hold it.
- **An application closed** (#486 ASLEEP-2, since 0.12.78; register §1.506, the coordinator's
  rulings of 11-22 on board #486). An instance whose idle action closes or holds its application
  closed (`close_app`, or `auto`'s `close_app` tier: asked for and not ended, its claim holding the
  key while the close runs, and done, its wake not come; `IdlePlanner::applications_held_closed`),
  whose startup package can bring it back and whose availability nothing withdrew (the projection
  holds it available; no fence; no instance the probes take up) is asleep in the same way. Its
  snapshot is not changed: the evaluation judges it as it is, selects nothing on it, and defers the
  one candidate it would select there with `instance_asleep_wake_due`, with no wake of its own
  since 0.12.83 (#486 ASLEEP-5); the idle plan wakes it for that candidate alone, by its startup package
  (`InstanceHolds::judge_asleep_as_available` names such an instance asleep as well). The hold in
  front of its candidates while the close runs is the idle action's own claim on its key (position
  2, `dispatch_instance_held`); #484's look ahead behind a hold reads no benefit gate, no pool line
  and no no-progress guard, so on such an instance a candidate it lets through, or that it cannot
  answer, counts for the idle plan as held back by a later position that names no time (not due,
  no wake of its own), behind any hold of positions 1 or 2. Its idle plan takes no wake from a
  decision a gate without a time holds, as for an instance whose idle action can stop its emulator
  (fix round 1 above; `close_app`, and `auto` at any tier). Ruling 7: on every such instance a
  decision whose wake is only a regenerating pool's next projection step (the evaluation's next
  reading of the pool, `pool_projection_steps`; a `trigger_false` on a `resource_projection`
  among them) names no wake in the idle plan either; the evaluation in which the candidate would
  be selected wakes the instance. After a Runtime start it is asleep once
  the start's preparation made it available ([runtime-config.md](../runtime-config.md), "What wakes
  an idle instance").
- **A later position that names no time** (#486 ASLEEP-3, since 0.12.79; register §1.512, the
  coordinator's ruling 3 of 11-22 and its fix round 1). On an instance whose idle action can put
  it to sleep, asleep or awake, that no hold of positions 1 or 2 holds, a candidate a later
  position of the order of the deferrals defers with no time of its own (a suspended package, a
  closed pack schedule's row with no end, since 0.12.81 a value set on the application that
  excludes the task's rule, Workflow #500 SV-2's position 5a, an owed rerun's pacing, a remedy
  round, an availability denial with no release) counts for the idle plan as held back with
  no time, as behind a hold: not due, no wake of its own; "eligible with no wake is due" stays for
  a hold of the instance itself. Its decision, its reason and the not-run explanation are as
  before. Before 0.12.79 the plan counted such an eligible decision with no wake due: it woke an
  asleep instance for a task the next evaluation held back again (production v0.12.78 on 11-22, a
  closed pack schedule, after a start) and kept an awake one from its action. When the gate opens
  the evaluation selects the task, or on an asleep instance names it the task the instance is
  woken for. An entry block's candidates (position 4b) still count due.
  - **The candidate before its time** (#486 ASLEEP-4, since 0.12.82; register §1.520, the
    coordinator's rulings 1 to 3 of 11-23). On the same instances, a decision whose own wake is a
    time that has not come (its trigger's next time, a cooldown's end, the release of a time-bound
    position such as a blackout) names no wake in the idle plan when one of the positions above
    would hold the candidate back once that time has come: the idle plan asks them for the
    candidate as if its earlier position had let it through, each through the reader the
    evaluation itself asks for an eligible candidate, read only, and judges each answer as above
    (`PolicyHost::evaluate`, `names_no_time` and `hold_back_with_no_time`; no second list). It asks
    every such position, not only the first, since one that names no time holds the candidate
    whatever a time-bound one does before it: the package's suspension (without its lift check, as
    #484's look ahead), the pack schedule's row, the value set on the application or the rule's
    value the package refuses, an owed rerun's pacing and a remedy round. It reads nothing the time
    that has not come decides: no availability (the budgets, the activity window and interval, a
    backoff and a task's pause are judged on the dispatch the evaluation builds for an eligible
    candidate, and there is none), no benefit gate, no pool line, no no-progress guard. Where it
    cannot tell (the task is not in the catalog, a reader fails) the candidate keeps its wake: one
    wake for nothing is the lesser harm than sleeping through a task; a failure is said in the
    daemon's log as the look ahead behind a hold says its own. A decision already counted (behind
    a hold, or held back at an untimed position while eligible) is left as it is. The decision,
    its reason, its own wake (so the evaluation's next wake and the driver's timer) and the
    not-run explanation are as before. Production 0.12.81 on 11-23: an instance under
    `stop_emulator` asleep since 00:07 was woken at 03:34:40 for a reading task whose trigger
    turned true at 03:39:40; its pack schedule, closed with no end, then held the task back,
    nothing was admitted, and the emulator was stopped again at 03:49:50. When the holding position
    opens nothing here changes: a pack schedule's edit and a value set or cleared are facts, which
    wake the policy driver (`FactsChanged`); that evaluation's look ahead no longer holds the
    candidate, so its time is the wake again (before it), or the evaluation names it the task the
    instance is woken for (after it). A package repaired raises no wake; its suspension is lifted
    by the lift check at position 3 when the candidate reaches it, in the evaluation at the
    candidate's own time, which wakes the instance then, its lead lost.
- **At a start** (#486 ASLEEP-3, ruling 3): a wake the start owes waits for the start's first
  plan and comes as a sleeping instance's, never for a reading alone
  ([runtime-config.md](../runtime-config.md), "At a start", with its known limit), and the wake a
  scheduled task's run does not answer withholds the idle action for its hold alone, at least 10
  minutes and at most two hours, which neither a catalog's activation nor a reading's run ends
  (the same file, "The withheld action expires by itself").
- **When the driver looks again** (#486 ASLEEP-5, since 0.12.83; register §1.524, the
  coordinator's rulings 1 to 3 of 11-23). The task an asleep instance is woken for names no wake in
  the evaluation: the evaluator does not know when the instance will be awake. Until 0.12.83 it
  named the evaluation's time, so the evaluation's wake was now, and a round whose idle plan did not
  wake the instance (a task within its rounding error, which never wakes a closed game; a wake held
  by a pause; the action's own claim still running) was followed by the next at once, with nothing
  changed, without end. Now the host says when the policy driver looks again, from what the cycle's
  idle check did (`PolicyCycle::idle_wake_unix_ms`, beside the evaluation's own wake; the driver
  wakes no later than the earlier of the two): for an instance the check left closed or stopped,
  owing its wake, whose plan counts no task due, the plan's own wake when it has not come (what lets
  a held task through: a blackout's end, an activity window's opening, a later position's release
  behind a hold, a task's own time); none for every other instance. A wake under way (queued by
  this check or an earlier one) names no time: each of its claims ends by handing the instance's key
  back (`lease.released` or `lease.transferred`, whatever its end: the emulator up and the startup
  package run, a start that failed, a start a pause or `start_emulator` off ended on its key, a
  start an emulator control superseded or a stop or a drain interrupted), which raises the host's
  business notification and so the next cycle; a claim cancelled while queued is ended by the
  control that cancelled it, whose own claim's end does the same. An action under way names none
  either, for the same reason. A wake a pause holds (PAUSE-AUTO) names none: the lease sweep's
  idle check, every second, wakes the instance on the same plan once the pause no longer holds the
  start. A candidate a position with no time holds is no candidate this rule applies to (the
  closure defers it, with no wake, before the round could pick it); it was never woken by the
  evaluation's time, and what lifts such a position reaches the driver as before (register §1.524,
  its table). An asleep instance with a task due and nothing holding it is woken as fast as before:
  the evaluation that finds the task due is the one at the task's own time (its wake while it was
  not due yet), and its idle check starts the wake. What a cycle computes is unchanged: the
  selection, the order of the deferrals, the idle plan's decision to wake, the cool-down. No
  periodic evaluation is added.
- **Not asleep.** An instance unavailable for any other cause, an application closed among them
  (its candidates are blocked at placement as before, and #484's fix round 2 holds for it); an
  instance whose emulator a person stopped. An asleep instance under a person's pause waits for the
  pause's end as every idle instance does.

## Runtime Boundary

V1 persistent scheduling state is single-host, local-filesystem state. Catalog generations, active pointers, ledgers, leases, budget journals, fact projections, approval projections, and release-set pointers must not be shared concurrently by independent hosts. A future multi-host revision must add host identity and fencing to every one of those owners instead of partially extending the V1 files.

Cross-run decisions are reevaluated by the scheduler after each outcome. A bounded rule table may make mechanical choices inside one run, but it cannot call back into the scheduler for mutable external state. External state required by a run must be pinned into dispatch parameters and its reason chain before admission.

Scheduled contained admission carries the validated, package-bound task request
into the existing lease acquisition. Its TTL uses the request's response budget
plus the same heartbeat/closure reserve as direct task execution, with the
Scheduler's existing maximum and checked arithmetic. The declared task timeout,
request deadline, lease boundary and execution permissions still constrain the
run. Lease expiry and the effective task deadline retain their typed ledger
records. Planning duration remains a reservation estimate; actual runtime usage
is recorded by the existing completion/failure owner.

For a scheduled contained task, an empty catalog outcome-reference set means
there is no scheduling-result consumer. The package may declare and produce its
own valid outcomes; its actual final page, designated effect and terminal
`scheduling_disposition` remain in GlobalLedger. Completion and recovery create
no policy outcome projection or completed-run consumption identity in this case.
When the reference set is nonempty, it must be a subset of the package's declared
outcome keys (Workflow #390 OUT-1, forced compromise O-03): a package may declare
outcomes the catalog does not read, and a run may produce one of them. A
referenced key the package does not declare refuses the run before any effect
(`policy_run_outcome_declaration_mismatch`). Package validity and execution
evidence requirements apply in both cases.

## Per-Instance Lanes

Workflow #427-A (plan r2, approved by Alice on 2026-10-28), behind
`scheduling.parallel.enabled` (default `false`;
[runtime-config.md](../runtime-config.md), "Per-instance lanes"). actingd reads the switch at
the start of every policy round, the initial round included; a run already handed to a worker
finishes and is settled there whatever the switch says afterwards.

- **One evaluator.** Evaluation and admission stay on actingd's policy driver thread. With the
  switch on, a granted scheduled dispatch is handed, with its request, to its instance's
  worker (`actingd-instance-<alias>`), which runs it and settles it exactly as the driver does
  with the switch off (the contained run, then the settlement of its scheduling result), and
  then wakes the driver: with the run's recompute wake when its settlement projected one,
  otherwise with `FactsChanged`, as a failed run does. One run per instance at a time; the
  runs of different instances overlap. The round never waits for a run, so its time budget
  (`policy_cycle_yielded`) keeps its evaluation meaning only, and the initial round no longer
  delays `runtime_ready`.
- **One owner from hand-off to settlement.** The hand-off registers the run as in flight on
  its instance, with its decision id, while its lease is still held; the registration ends
  only after the run's settlement (or its refusal's) is recorded. It is the one source of:
  the instance's occupancy (the evaluation defers, and the admission refuses
  `dispatch_instance_held` with `holder_kind` `policy_dispatch`, a new dispatch on a registered
  instance, also once the run's terminal is written and its lease released and before it is
  settled); the reconciliation (a run a worker still holds is skipped, never settled early or
  twice); and the waits of an instance's scheduling pause and of an install drain.
- **Lifecycle.** The worker takes a lifecycle (continuation) admission when it takes the run;
  its work covers the run and the settlement, so a drain or a stop waits for it. A run handed
  off before an install drain or a scheduling pause began goes ahead, as the round's own run
  admitted before them does with the switch off: the drain waits for it, and the pause drains
  it like any contained run admitted before its gate closed; neither records a failure for it
  (fix 1, review L1). Only a run the worker takes after the Runtime's stop was requested is
  refused (`runtime_stopping`): it never starts, its lease is released, and it gets a live
  interrupted settlement (`policy_settlement_interrupted`, with the elapsed `runtime_ms`, not
  the start's recovered shape), lifted after the restart like the start's, the first one of its
  pair (fix 2, re-review L-2; wording, re-review 2 L-A); its registration is
  ended. Nothing is replayed after a lost receipt.
- **Failures (#419).** The run's failures go through the instance's failure containment: an
  instance-level failure fences that instance and the other lanes go on; only a global fault
  stops the Runtime. A worker panic drops the run (not retried, its lease lapses at its TTL);
  its registration stays, marked abandoned, so the instance stays occupied. A run whose
  settlement failed without stopping the Runtime is left the same way. Once such a run's lease
  ended, the reconciliation settles it by the start's rule (fix 1, review M1): as at start, a
  lease that already ended without a run-linked release (its expiry, a release without run
  links, a transfer, or an expiry and a transfer as one end) gets no release, so the lease
  keeps its one end (fix 2, re-review M-1; #670 rulings 2 and 3); the run is settled once,
  from its terminal or, without one, as interrupted from that end
  (`policy_settlement_interrupted`, which pauses the task on that instance as a run cut by a
  restart does); its registration is forgotten and the instance is free again. A lease that
  ended in more than one way leaves the run held, and the next start records it
  (`policy_settlement_dispatch_left_open`). A run whose settlement a start refuses (the host's
  recovery or the ledger) is recorded `policy_settlement_dispatch_unsettled` by the first start
  that meets it, which holds its instance once, and `policy_settlement_dispatch_left_open` with
  the refusal as `failure_code` by every later start (UNSET-1); its dispatch stays open, out of
  the active workloads, until a version that can settle it starts. Nothing closes a dispatch by
  time. UNSET-2 (0.12.51; [v012-register.md](../v012-register.md) §1.296): a run with no
  terminal whose one run-linked release is its own (the failure path's, effect `performed`) is
  settled as interrupted from that release whether or not it has effect or input records, as a
  run whose release the start recovered is, so a run cut by a stop after it released its own
  lease is settled by the next start, once. When an earlier start refused such a run, the start
  that completes its settlement records `policy_settlement_dispatch_closed` (Info) under the
  run's links, with its operation and the earlier refusal as `failure_code`: the settling start,
  or the next one when a stop cut the settling start between its execution record and its
  completion. A stop between the completion and that record leaves it unwritten; the settlement
  and the completion are the lasting records. That late settlement completes after
  the later runs of its (task, instance) pair, when there are any, so wherever the Runtime takes
  a pair's latest completed run (its value record and the journal a later start loads, its
  task snapshot, its outcome, `actingd suspended`) it takes the latest in admission order, and
  the late settlement is not a failure of its pair: the pair's failure streak, pause, backoff and
  retry round are what they were before the settlement (a retry round the run's own admission
  opened stays, and offers an immediate rerun only to a retryable failure of the pair, as before
  it), and its count of failures in a row and its latest failure are what they would be without
  that run, at the settlement and at every replay and load. When the pair has run since,
  its later run stays its latest; when it has not, the late run is its latest run and reads as
  interrupted (its task snapshot shows it, failed, and the pair's outcome is missing, as after
  any interrupted latest run).
- **Re-evaluation (C1, not behind the switch).** An evaluation whose validation finds its
  sampled outcome keys changed (`policy_outcome_snapshot_changed`: another run settled between
  the sample and the validation) is sampled again, at most three times per round, with one
  daemon line each (`actingd policy_cycle_resampled`); past that the round is given up
  (`actingd policy_cycle_given_up`) and deferred to its next trigger. It never stops the
  driver.
- **Shutdown.** The driver stops taking triggers and is joined; the workers hold no reference
  to actingd's host handle, so the host is released and closed, and its close joins the
  workers: a run a worker took finishes and is settled, one still queued is refused
  (`runtime_stopping`) and settled as interrupted, as above.
- **Device groups (Workflow #437 MAPP; [device-group.md](../device-group.md)).** MAPP-1 (register
  §1.328) builds the derived map, the scheduler state keyed by the device key and the round's one
  pick per device; MAPP-1b (register §1.330) builds the one worker (`actingd-device-<device_group>`)
  and the in-flight registration read across the group; MAPP-2a (register §1.342) builds the
  switch's parts and their one entry, which nothing calls before MAPP-2b, and the load
  refuses every group of several until the slice "order within one device". Several instances on one device are one device group. The group is derived from the
  configuration at assembly and never stored.
  - A group has one lane: one scheduler state (one lease, one queue, one cooldown), keyed by the
    group's device key, and one worker. The in-flight registration of any member's run holds the
    device for every member.
  - A round selects at most one dispatch per device, among the due candidates of all its members,
    in the one ranking order the round already has.
  - Before a member's run, the switch brings that member's application to the front ("The switch
    before a run" there).
  - A group of one is today's lane, unchanged.

## The policy driver's clock and the line of a cycle

#503 CYC-1 (0.12.84; [v012-register.md](../v012-register.md) §1.526, the coordinator's rulings 1
to 3). #503 CYC-2 (0.12.85; §1.528, the coordinator's rulings 1 to 3): the calls of the
evaluation's setup, the remedy journal's window ends and what of the performance thread wakes the
driver. #503 CYC-3 (0.12.86; §1.529, the coordinator's rulings 1 to 5): the window end comes from
the admission record. Not behind the switch.

- **A clock jump.** actingd's policy driver looks at the wall clock at least every half
  `policy_cadence.clock_jump_threshold_ms` (5 000 ms in the defaults, so every 2.5 s while it
  waits) and keeps, with each look, its own monotonic clock's reading. A look is a clock jump when
  the wall clock moved since the last look by more than the threshold more or less than the
  monotonic clock did, or went backwards. The driver then evaluates at once (the trigger
  `clock_observed`, the line's wake `clock_jump`), and the cadence takes the cycle as a full one
  (`ClockJump`), past its cool-down and debounce; it reads the driver's word and judges no jump
  of its own. However long a cycle or a wait took, both clocks moved by it: a long cycle or a late
  look is no jump. A time the driver waited for (the evaluation's wake or the host's) is a look
  that is no jump; its cycle goes through the cool-down and debounce as any trigger. Until
  0.12.84 the wall clock alone was compared with the last look, so a cycle longer than the
  threshold (5.2 s on production on 11-26) made the next look a jump, and its cycle followed at
  once, round after round.
- **The line of a cycle** (the daemon's stdout, one line a second at most; nothing in the ledger,
  nothing stored; register §1.524 for its first members, which are unchanged). After `total_ms` it
  names every other stretch of the host's cycle up to the end of its capped record, in the cycle's
  order, each from the end of the stretch before it:

  | Member | The stretch |
  |---|---|
  | `before_gates_ms` | from the cycle's start to its first gate: a reconciliation's pending settlements, the idle action's instances, the scheduler workbench's check and its tables |
  | `gates_wait_ms` | the waits to take the detection write gate, the procedure manifest, the outcome gate, the policy lock the outcome keys are read under and the fact write gate |
  | `gates_held_ms` | the work under those gates, `projection_ms` apart: the manifest's copy, the entry blocks, the verdicts, the outcome keys, the clock sample |
  | `cycle_inputs_ms` | from the gates' release to the evaluation's policy lock: the left-out lists, the run limits, the pauses and fences, the asleep instances, the queues and device groups, the lift view, the configuration in effect |
  | `policy_lock_wait_ms` | the wait to take the policy lock the evaluation runs under |
  | `evaluation_setup_ms` | under it, up to `evaluation_ms`: the outcome keys' validation, the blackouts, the handed runs, and the evaluation's own inputs (the cadence, the remedy table and plan, the evaluation's cost, the random delays, the immediate reruns, the fact refresh's readers, the variable plan, the run slots, the owed reruns, the instances' holds, the pack schedules, the no-progress observation); `evaluation_setup_slowest` names its slowest calls |
  | `no_progress_ms` | from the evaluation's end through the no-progress guards' capture of the new intents |
  | `detection_preview_ms`, `detection_preview_store_reads` | the detection planning signals' preview, and its reads of the planning-signal store |
  | `explanation_ms` | from the clock slots' preview to `held_ms`: the eligibility ages, the queue waits, the not-run explanation and the idle plan's inputs |
  | `after_held_ms` | from the held observation through the evaluation's last bookkeeping and the reads under the policy lock after it |
  | `capped_record_ms` | the capped record |
  | `unattributed_ms` | the time up to the end of the capped record that none of the named parts covers |
  | `evaluation_setup_slowest` | the three calls of `evaluation_setup_ms` that took longest, `name:milliseconds` each and comma-separated as `wakes` is, the slowest first and of two that took as long the earlier in the cycle's order; `none` when no call took any time; for a line of several cycles, the three slowest of the calls' sums |

  To read one: the named parts (with `projection_ms`, `evaluation_ms`, `clock_slots_ms` and
  `held_ms`) and `unattributed_ms` add up to `total_ms` less `after_capped_ms`. The waits timed
  apart are those for the five gates of `gates_wait_ms` and for the policy lock of
  `policy_lock_wait_ms`; a wait for any other lock (the entry blocks' and the registered
  instances' locks, the store's shared connection) and a slow read of the store show in the member
  of the stretch they fall in. `unattributed_ms` of more than a few milliseconds names a stretch no
  member times yet. Members are only added; none is renamed or dropped.
- **The detection preview asks nothing it knows.** For a signal id the last preview asked about,
  the preview takes that answer instead of asking the store; the store's planning signals are
  written by the Runtime alone and never removed, so an answer that names a signal stays true, and
  one that names none holds until the Runtime records a signal of that id, which updates it. A
  quota window the store holds no usage for is kept as one with none, as every reader of the
  window takes it, until the Runtime records usage for it. The preview asks each id once, however
  many suggestions name it. What it reserves, records and returns, and in what order, is what
  asking the store each time gives.
- **The calls of the evaluation's setup** (CYC-2, ruling 1). Each call of `evaluation_setup_ms` is
  timed, each from the end of the one before, on the cycle's lap clock, under a fixed word:
  `outcome_keys`, `blackouts` and `handed_runs` (under the evaluation's policy lock, before the
  evaluation is entered; #455 RR3c, since 0.12.98: `handed_runs` gives the evaluation the list
  read once with the needs under the gates, "Pool lines"), then `cadence`, `remedy_table` (the instances waiting for a client update
  and the remedy table when it is due), `evaluation_cost`, `random_delays`, `immediate_retries`,
  `fact_refresh_inputs`, `remedy_plan` (the remedy journal and plan), `variable_plan`, `run_slots`,
  `rerun_plan`, `instance_holds`, `pack_schedule` and `no_progress`. Two waits a call can make are
  timed apart from the call that made them and named on their own: `catalog_memo_wait`, the waits
  for the lock of the catalog store's memo of compiled generations, and `catalog_reads`, the reads
  of a generation from the catalog store's files (its documents read, their digests checked and
  the generation compiled) when the memo does not hold it. The memo keeps the active generation
  and `standard_packs.cached_generations` older ones (one by default) in composer mode, eight in
  configured mode. `evaluation_setup_ms` still covers the whole stretch; the calls' times lie
  within it.
- **A remedy run's window end is read once** (CYC-2, ruling 2). The remedy journal gives the
  remedy plan, for each run it holds, the end of the activity window the run was admitted in, read
  from the run's own catalog generation by the activity profile its admission names. Until 0.12.85
  every evaluation read it again for every run the journal holds (up to 64 per pair, back to the
  pair's last root blocked run), so a run admitted under a generation the memo no longer holds had
  that generation read from its files and compiled at every cycle, and two such generations in a
  pair's runs pushed each other out of a memo of one. Now the end is read the first time the
  journal holds the run and kept, by the run's decision, while the journal holds it: a run's
  generation and admission never change, so neither does the end. The journal, the plan, and every
  intent, signal and record are those the reading at every cycle gives.
- **A remedy run's window end is the admission's** (CYC-3, rulings 1 and 2). Since 0.12.86 every
  admission records the end of the activity window it was admitted in (`window_until_unix_ms` of
  its activity sample, `ledger` revision 10), from the one resolution of the window it makes for
  `window_id` and `local_day`: the same value the run's generation gives, since an admission
  resolves its window under that generation (the active one, which its intent must name), at its
  own time, by the profile it records. The remedy journal gives a run that carries it that end,
  reads no generation for it and keeps nothing; after a start the end comes back with the
  admission record, so the first cycle reads no generation for such a run. Only a run admitted
  before 0.12.86, whose record carries no end, is still read once from its generation and kept, as
  CYC-2 reads it. The end of the window a dispatch's failure at its entry counts in
  (`dispatch_window_end`) is the recorded one too, when the record carries it.
- **What of the performance thread wakes the driver** (CYC-2, ruling 3). A record of the
  performance thread wakes the driver only when it changes an input of the evaluation or of the
  admission that follows it. The evaluation reads, of the performance state, the pacing stage
  (the owed reruns' gate), the balance controller's level (the heavy dispatch limit) and its latest
  observation (the responsiveness and third-party pressure behind the ranking's contention
  penalty); the admission reads the committed capacity sample (the disk gate). A
  `perf.pacing_changed`, `perf.dispatch_share_changed`, `perf.run_hold`, `perf.balance_changed`,
  `perf.pressure_started`, `perf.pressure_ended`, `perf.monitor_degraded` and
  `perf.monitor_recovered` each wake it at once, as before. A `perf.stutter_detected` (an
  instance's frames stalling) wakes it never: the frame gap joins the pipeline samples, from which
  the next control observation reads the responsiveness, and a level that changes from it is a
  `perf.balance_changed`. A `perf.summary` wakes it only when its capacity sample changes what the
  disk gate decides on against the last summary recorded (a volume came or went, or a volume's state
  of sufficient, soft or hard pressure or unknown changed), the admission's own re-sample of a
  stale fact included; a summary of the interval, or of free bytes moved within a state, wakes it
  never. The latest observation's responsiveness and third-party pressure change with every sample
  and no record names them: an evaluation reads them as they are when it runs, as it reads the
  clock. They enter the ranking only (the contention penalty) and gate nothing, so no task waits
  for them; a ranking can be that of the last evaluation for at most the reconciliation interval
  (`policy_cadence.reconciliation_interval_ms`, 60 s by default), after which the driver evaluates
  whatever happened. The line's `wakes` counts `perf.summary` only for a summary that woke the
  driver.

## Evaluation failures

ISO-PE (#419, 0.12.28; production v0.12.26 on 10-29 11:19, where one
`policy_evaluation_input_invalid` ended the policy driver and the Runtime with it). Not behind
the switch.

- **What stops the Runtime.** Only a failure whose scope is `Global`
  ([runtime-outcomes.md](../runtime-outcomes.md), the failure scope) is returned from an
  evaluation and ends actingd's driver, and with it the Runtime. On the evaluation path these are:
  a ledger read that failed (`ledger_failure`: the one ledger is the evaluation's source of
  truth); a poisoned lock (`runtime_state_poisoned`: a thread died while it changed the host's
  shared state); the Runtime clock (`monotonic_clock_regressed`, `system_clock_invalid`); the
  configuration in effect that cannot be read; the shared policy projection disagreeing with the
  ledger (`policy_outcome_projection_mismatch`, `policy_outcome_failed_run_residual`, and the
  Global list's `policy_admission_event_invalid`, `policy_state_ledger_incomplete`); and any
  condition the host latched (its monitor stops the Runtime whatever the driver does). Each of
  them leaves no instance's work trustworthy, so none is one instance's.
- **What is skipped.** Every other evaluation failure (an input the evaluator refuses, a
  configured input the Runtime does not trust, an authority conflict, one instance's fault) is
  recorded and skipped: `policy_evaluation_skipped` (warning) with the failure as its
  `caused_by` link, and the cycle is a deferred one with no evaluation, due
  `scheduling.evaluation.retry_ms` later ([runtime-config.md](../runtime-config.md), "Skipped
  evaluations"). The driver keeps its trigger and evaluates again then; nothing is dispatched
  from the skipped evaluation, runs already handed to workers go on, and the Runtime keeps
  running. `scheduling.evaluation.escalate_after` skips in a row record
  `policy_evaluation_awaiting_person` (error) once; an evaluation that evaluates ends the run of
  skips. Two failures that are not global keep their own bounded handling:
  `policy_outcome_snapshot_changed` is sampled again (above), and `outcome_projection_not_ready`
  waits one debounce, with or without the trigger's recompute wakes, while fewer than
  `escalate_after` evaluations in a row have not evaluated. Each such wait counts as one of them
  (the `count` of the run) and records nothing; once they reach `escalate_after`, the next
  `outcome_projection_not_ready` is skipped as above: recorded, evaluated again `retry_ms` later
  and escalated as every run of skips. (Until 0.12.28 a projection that stayed not ready was
  evaluated again every debounce, with no record and no bound, when the trigger had recompute
  wakes, and ended the driver when it had none.)
- **The detail.** `policy_evaluation_rejected` carries the policy's code as its `caused_by`
  link and the policy's code and developer text as its native detail. An input check that names
  what it refused gives that link `policy_input`, `policy_input_problem` and the `fact_key`,
  `instance_id`, `task_id` or `pool_id` it names; a time check (`observed_after_evaluation`)
  also gives the input's `observed_at_unix_ms` and the evaluation's `evaluated_at_unix_ms`.
- **The evaluation's time.** The Runtime clock is sampled for an evaluation once it holds
  `policy_outcome_gate` and `fact_write_gate`, the gates under which a settlement and every fact
  are written and under which it reads its inputs. Every input it reads was committed before
  that sample, so none was observed after the evaluation's time. (Until 0.12.28 the time was
  sampled as the evaluation started, before it waited for the gates; a settlement on an instance
  worker committed in between made the evaluation refuse its own inputs as "later than
  evaluation time".) An input can still lie ahead of the Runtime clock when the wall clock is set
  back: a settlement's time is its admission's wall time plus the run's monotonic duration. Such
  an evaluation is skipped until the clock passes it.

## Record lists and the projection

Workflow #428 S168-LB1a, since 0.12.55 ([v012-register.md](../v012-register.md) §1.309, §1.311;
the coordinator's ruling 2 of 11-11, not Alice's words; #419). Before it, the store's facts
entered the evaluation whatever their rows, and the evaluator refuses the whole input when one
record list holds more than 256 rows: the evaluation is skipped for every instance (`policy_evaluation_skipped`
above), and after `escalate_after` skips in a row it records `policy_evaluation_awaiting_person`.
The store's row bound was 256 too, so no such record could exist; since S168-LB1a it is 1024
([instance-fact-store.md](../instance-fact-store.md), "List reading facts", "Row bound"), and one
long list must not stop the scheduler for every instance.

- **List readings are not the scheduler's.** The projection leaves out every record under
  `inventory.list.`, as it leaves out the policy instance keys, the priority offsets and the
  run-duration samples ([instance-fact-store.md](../instance-fact-store.md), "Projection into the
  evaluation", "Left out of the projection"). The catalog compile refuses, with `type_mismatch` at
  the member's `fact_key`, a catalog member that names a key under that prefix to read: a `fact`
  or `record_deadline` predicate, a fact reference ("Fact references"), a pool's ledger key and a
  pool fact reference ("Pool fact references"), a consumption read from a fact (SCHED-C1f1,
  "A run's consumption read from a fact") and a fact the selection document declares
  ([selection-policy.md](../selection-policy.md),
  "Scheduling consumer"). So such a predicate never compiles and never reads Unknown at
  evaluation. Before S168-LB1a such a catalog compiled and its predicate read the list record as
  any record list; S168-LB1a read the four resource repositories' `main` before it built the
  refusal and found no catalog that names one (register §1.311). A timeline event's prefixes name keys to invalidate, not to read, and are not
  refused. **The one exception** (#455 P1, since 0.12.78, register §1.504): a pool's observation
  of the kind `list_rows` names a key under the prefix, and only under it ("A pool read from rows
  of a list"). The list itself still does not enter the projection: the host hands the evaluation
  the number it computes from the list's rows (#455 P2, since 0.12.79; in 0.12.78 no host computed
  it, and such a pool had no reading).
- **Another long record list stays one instance's.** A record list of more than 256 rows, the
  policy's own bound, is left out for the instances its scope applies to. The host records the
  warning `policy_fact_rows_exceeded` (since 0.12.55) once for each such record after a start,
  not on every decision; a predicate that reads it is Unknown for those instances and the key
  raises no fact refresh demand, since a reading cannot shorten a list: the predicate reads
  `fact_missing` and its task keeps `trigger_unknown` (or `feedback_stop_unknown`) with no
  `fact_refresh_*` reason ([instance-fact-store.md](../instance-fact-store.md), "Left out of the
  projection"); every
  other instance and key evaluate as before. The evaluator's
  bound and the schema's `maxItems` (`v2/common.schema.json`, a predicate's `record_list` value)
  stay 256.
- **Identity.** A ledger with no record under `inventory.list.` and no record list over 256 rows
  gives the evaluation the inputs of before, byte for byte. With a list record, the decisions are
  the same and the evaluation's `facts` and its `fact_snapshot_id` no longer hold the record. The
  id is compared between an evaluation and its admission inside one running Runtime
  (`policy_facts_stale`), and a deploy restarts the Runtime, so no id is compared across builds;
  S168-LB1a confirmed that no reader recomputes a persisted one: the id is computed only by the
  projection, at an evaluation and at an admission, and the ledger compares two persisted
  dispatch records' ids only with each other (register §1.311).

## Forward Planning And Maintenance

Forward planning is a bounded dry-run of the same pure evaluator used for live policy decisions. It projects at most 24 hours, performs no ledger write, lease operation, execution, or device action, and reports incomplete evidence instead of inventing resource effects. This is a projection facility, not another scheduler.

Predictive maintenance compares ledger-pinned execution duration and fact-confidence trends within an explicit lookback window. Both evidence series must meet their declared sample minimum before a recheck can be suggested. Missing evidence produces an `evidence_insufficient` assessment and no planning signal.

## Bounds

The compiler enforces both schema limits and UTF-8 byte limits:

| Item | Limit |
| --- | ---: |
| One document | 1,048,576 bytes |
| Selection document | 524,288 bytes |
| Catalog (all documents together) | 4,194,304 bytes |
| Priority offsets per evaluation | 16,384, each within ±1,000,000 milli |
| Identifier/reference | 128 bytes |
| Registered instance alias | 256 UTF-8 bytes |
| Diagnostic text, fact string, or source URI | 1,024 bytes |
| Approval references | 64 |
| Tasks | 4,096 |
| Pools | 1,024 |
| Activity profiles | 1,024 |
| Timeline events | 4,096 |
| Predicate depth | 16 |
| Predicate nodes per root | 512 |
| Effects, references, or instance overrides per task | 128 each |
| Windows or goals per activity profile | 128 each |
| Pool valuation name / unit | 128 / 32 UTF-8 bytes |
| Pool valuation scale | 1..=9,007,199,254,740,991 |
| Pool valuation base weight / gap weight | 0..=1,000,000 / 1..=1,000,000 milli |
| A task's fixed value (SCHED-C1a, since 0.12.52) | 1..=9,007,199,254,740,991 milli |
| A consumption's fallback, and a usable amount read from a fact (SCHED-C1f1, since 0.12.54) | 1..=9,007,199,254,740,991, at most a constant capacity |
| Rows of a record list in the evaluation's input, and of a predicate's `record_list` value | 256 (since S168-LB1a, 0.12.55, the projection leaves a longer record out, "Record lists and the projection") |

Loop budgets are mandatory in V1; in V2 each run-count bound is optional (SS-3, "Optional run-count bounds"). Arrays and strings that exceed their limit reject the entire catalog. Duplicate identifiers, duplicate object keys, and unbounded recursive input are invalid.

## Canonical Serialization And Hash

The catalog hash is computed as follows:

1. Parse all four documents while rejecting duplicate object keys and invalid UTF-8. Parse the optional selection document with the selection-policy crate's own reader and validation.
2. Validate the exact V1 schemas and cross-document invariants.
3. Construct the JSON object `{"activity": A, "pools": P, "tasks": T, "timeline": L}` from the validated documents, adding `"selection": S` only when a selection document is present. No field is removed and no default is inserted, so a catalog without a selection document keeps its existing hash. In the same way a pool's `valuation`, and the `gap` inside one, appear only when declared: a catalog without them keeps its existing hash, and a declared block is hashed field by field, `name` and `unit` included, so changing any of its values is a new catalog identity that needs its own approval.
4. Serialize that object with RFC 8785 JSON Canonicalization Scheme. Object keys are sorted by JCS rules, array order is preserved, and no insignificant whitespace is emitted.
5. Compute SHA-256 over the canonical UTF-8 bytes.
6. Encode the result as `sha256:` followed by 64 lowercase hexadecimal characters.

Catalog producers must emit semantically unordered arrays in deterministic order. Because array order is preserved, reordering any array changes the hash. Approval references authorize the exact catalog version and hash; they are not executable permissions by themselves.

## Diagnostics

Compiler failures use `CatalogDiagnostic`. `code` is stable within V1; `reason` is human-readable and must not be parsed for control flow. `json_path` uses RFC 6901 JSON Pointer. `source` identifies the document, source URI, and one-based line and column. Version and catalog fields may be null only when malformed input prevents their extraction.

Any error-severity diagnostic rejects the complete four-document catalog. Warnings may accompany a successful dry-run but cannot conceal an error. The compiler must sort diagnostics deterministically by document, source position, code, and JSON path.

## Compiler Boundary

`actingcommand_policy::compile_catalog` accepts four in-memory `CatalogDocumentSource` values plus an optional fifth for the selection document. It performs no file access, script execution, network request, clock read, sleep, ledger write, lease operation, or device action. Success returns one complete `CompiledCatalog`; any error returns `CatalogCompileFailure` and no partial IR. Both outcomes expose canonical, byte-stable dry-run JSON.

## Neutral Example

`examples/catalog-a` is a synthetic, product-neutral catalog. It exercises all four documents without embedding external project, game, account, device, or private workflow data.

The example is the V1 upgrade map for the earlier `task-catalog.v0-draft` shape:

| Draft concept | V1 location |
| --- | --- |
| task identifier and operation | `tasks[].id`, `tasks[].entrypoint` |
| start condition | `tasks[].trigger` |
| feedback termination | `tasks[].feedback_stop` |
| resource assumptions | typed `tasks[].consumes` and `tasks[].produces` plus `pools[]` |
| retry behavior | `tasks[].on_failure` |
| loop and session bounds | `tasks[].loop_budget` and `activity.profiles[]` |
| schedule/reset/deadline data | `activity.profiles[].windows` and `timeline.events[]` |
| instance-specific priority | `tasks[].instance_overrides` |
| instance importance and targets | instance-scoped `activity.profiles[].importance_milli` and `goals` |

Draft fields that relied on runtime defaults or implementation-specific behavior have no implicit V1 mapping and must be supplied explicitly.
