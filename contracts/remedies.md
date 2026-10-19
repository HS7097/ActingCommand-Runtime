# Declared remedies: "needs further processing" (Workflow #411 B15)

A run whose business result is not the result the business expects, for a reason the package
recognises and can explain, ends on a **mapped named outcome key whose mapping declares a
remedy**. The run is still a successful run with that key. The remedy is static package data: the
blocked mapping names an opaque `need` and the opaque `capabilities` that can serve it; a provider
package declares what it `provides`, optionally with named parameters whose options and default it
declares. Which capability runs, and with which parameter values, comes from the instance's target
record (written by a person or an agent), else the configuration, else the package defaults; with
none, the task stops and waits for a person. The scheduler holds the blocked task, dispatches an
allowed provider with priority, retries the blocked task once, follows a chain of at most three
layers, and stops for a person when a round is used up.

**Status.** Written ahead of its code (B15 R0); slices R1–R7 implement it: R1 the package members
and the bundle check (section 2, section 10), R2 the evaluator's plan (sections 3.3, 4, 5, 6, 7.2),
R3 the configuration keys (section 3.2), R4 the host (sections 5, 7.1, 8), R5 the run parameters
(section 4), R6 the choice writers (section 3.1), R7 the reports and the check-config notes
(sections 5.5, 10). Until R1, a package that declares `remedy` or `provides` is refused as an
unknown member, as today; until R4, nothing reads the declarations, the record or the keys, and a
run that ends on any mapped key is handled as today.

Everything here is game-neutral. Needs, capabilities, parameter names and option names are tokens
the packages declare; the examples use neutral placeholders (`need_a`, `cap_x`, `param_p`,
`opt_1`; pages `home`, `page_b`, `result`). No contract, default, code or test of the Runtime
names a game, a need, a capability, a remedy or an order of remedies.

## 1 Semantics

1. **What it is.** A mapping's `remedy` says: when the run ends on this key, the business result
   was not the expected one, and serving `need` may make it reachable. The run is a success with
   that key; its effect condition is whatever happened (`no_designated_effect`, or
   `designated_effect_completed` when part of the work was done before the block). The key
   travels, as every mapped key does, in the terminal's `scheduling_disposition`.
2. **What it is not.** An unrecognised screen, an unconfirmed step, a refused guard, a device or
   backend error, a timeout: these stay failures (the failure identity of
   [policy-suspension.md](policy-suspension.md), the immediate rerun, the recovery ladder). The
   Runtime never turns a failure into a remedy key. Only the package, by mapping a recognised page
   to a key, says "this is a known, explainable block".
3. **Opaque names.** `need`, capability, parameter and option names are tokens the packages
   declare. The Runtime compares them for equality and never interprets them. It has no built-in
   pairing of needs with capabilities and no order between capabilities, and it never chooses on
   its own between remedies (sections 3.3 and 6).
4. **Scope.** Policy runs only. A direct run (`actingctl task-run`, MCP `ac_run_pack`) or a Lab run
   that ends on a remedy key ends as today: nothing is held or requested.
5. **Switch.** With `scheduling.remedy.enabled` `false` every remedy key is a plain named outcome
   again (no hold, no request); the package checks still run.

### 1.1 One general mechanism, never a fixed one

1. Nothing in the Runtime knows a need, a capability, a remedy or an order of remedies.
2. There is no built-in pairing and no fixed preference. The pairing need to capabilities is the
   blocked package's declaration; which capabilities may run comes only from the instance's target
   record, the configuration or the package default, in that order (section 3.3); with none, the
   task stops for a person (section 5.5). Between several allowed remedies the general ranking
   decides (section 6); no code, default or table orders them.
3. The same declarations serve every kind of block the packages can recognise: a resource at its
   cap that blocks a claim (one sink), a full storage that blocks an action (two alternatives, one
   irreversible, chosen by the target record), a currency that overflows unless spent, a stamina
   that is full. The worked examples belong to the resource repositories, never to the Runtime.
4. A remedy that nothing provides is a finding, never a silent no-op: a bundle and standard-pack
   check entry, a check-config note, and a visible stop at run time (section 10).

### 1.2 Parameters and package defaults

1. The package default is a declaration: with no target record and no configuration, it runs,
   irreversible remedies included, within the defaults the provider declares.
2. A provider declares named parameters with enumerated options and a default (section 2.2); the
   target record or the configuration selects values per instance (sections 3.1, 3.2); an unknown
   parameter or value is refused at write and stops the remedy at use (sections 3.1, 5.5), never
   ignored.
3. The chosen values reach the run as read-only facts (section 4); the package's own selection
   policy (filters, gates, ordering) applies them; the game's own protections stay the game's.
4. They are recorded in the dispatch's reason chain (`policy_dispatch_parameters`), so the ledger
   holds them without a new shape (section 8).

## 2 Package declarations (package interface revision 4)

### 2.1 The blocked side: `remedy` on a mapping (`task.json`)

```json
"scheduling_outcome": {
  "mappings": [
    {"outcome_key": "done", "effect": "designated_effect_completed", "terminal_pages": ["result"]},
    {"outcome_key": "blocked_k", "effect": "no_designated_effect", "terminal_pages": ["page_b"],
     "remedy": {"need": "need_a", "capabilities": ["cap_x", "cap_y"], "default": ["cap_x"]}}
  ]
}
```

| Member | Rule | Refusal |
|---|---|---|
| `remedy` | optional, at most one per mapping | `invalid_scheduling_outcome_declaration`, field `remedy` |
| `need` | a token `^[a-z][a-z0-9_]{0,63}$` | the same, field `remedy.need` |
| `capabilities` | 1 to 8 unique tokens of the same charset; never `none` | the same, field `remedy.capabilities` |
| `default` | optional; unique tokens, a subset of `capabilities`; absent or `[]` means no package default | the same, field `remedy.default` |

A package may declare remedies on any number of its mappings; two mappings may share a need. A
mapping without `remedy` serializes byte for byte as before. Unknown members stay refused.

### 2.2 The provider side: `provides` (`control.json`)

```json
"provides": [
  {"capability": "cap_x"},
  {"capability": "cap_y", "irreversible": true,
   "parameters": [
     {"name": "param_p", "options": ["opt_1", "opt_2"], "default": "opt_1"},
     {"name": "param_q", "options": ["opt_3", "opt_4", "opt_5"], "default": "opt_3"}]}
]
```

| Member | Rule | Refusal (`contained_task_control_invalid`) |
|---|---|---|
| `provides` | optional, 1 to 8 entries, capabilities unique | `reason=provides_invalid` |
| `capability` | the capability charset; never `none` | `reason=provides_invalid` |
| `irreversible` | optional boolean, default `false` | `reason=provides_invalid` |
| `parameters` | optional, 0 to 8 entries; **at least one when `irreversible`**: the declared consumption scope | `reason=provides_parameters_invalid` |
| `parameters[].name` | a token `^[a-z][a-z0-9_]{0,63}$`, unique across the package's whole `provides` | `reason=provides_parameters_invalid` |
| `parameters[].options` | 1 to 16 unique tokens | `reason=provides_parameters_invalid` |
| `parameters[].default` | one of `options` (required) | `reason=provides_parameters_invalid` |

Any execution mode that can be a policy task may provide capabilities. A package may both provide
capabilities and declare remedies; that is how chains form. A `control.json` without `provides`
serializes byte for byte as before.

### 2.3 Irreversibility and scope

- A capability is irreversible on an instance when any provider bound there declares it so.
- Its scope is its parameters: the package declares the options and the default (for example a
  default that consumes only a narrow class of items), and the package's own selection policy
  applies the values it is handed. The game's own protections stay the game's. The Runtime only
  guarantees that the run receives exactly the values chosen by the source order, each one an
  option the package declared.
- The package default counts as a declaration: with no target record and no configuration, an
  irreversible remedy runs with its declared defaults. The marker makes the package check require
  declared parameters, and it is recorded on every request (`irreversible` of
  `policy_remedy_requested`).
- There is no host-wide switch for irreversible remedies; `scheduling.remedy.enabled` and a
  provider package variant that stops at its confirmation page for a person's look (a resource-side
  choice) cover the first deployments.

### 2.4 Static rules (package check, kernel admission and the source parser alike)

1. The members parse with the rules above; unknown members stay refused. The source parser refuses
   the same violations as `resource_declaration_invalid` with the JSON pointer.
2. A package does not provide a capability one of its own remedies names (a one-package cycle):
   `contained_task_control_invalid` `reason=provides_own_remedy`.
3. Parameter names are unique across the package's `provides`, so a parameter belongs to one
   capability and a run's overlay (section 4) is unambiguous.
4. No other rule is local; the rules across packages are in section 10.

## 3 Choices: target record, configuration, package default

### 3.1 The target record `session.remedy.choices`

An instance fact of the `session.` family, one record per scope (`instance` or `game`; never
`server`, whose id is not game-qualified), written by a person or an agent
([instance-fact-store.md](instance-fact-store.md), "Remedy choice facts", owns the key, its write
rule, its writers and its capacity). Its value is a `record_list` of rows whose fields are
strings:

```text
{row: "allow", need, capability}                  this source allows capability for need
{row: "none",  need}                              this source decides: no remedy for need
{row: "parameter", capability, parameter, value}  this source sets a parameter's value
```

The most specific record that applies to an instance applies as a whole (instance over game), as
for the pack schedule. A write that breaks the shape (a server scope included), or names a need,
capability, parameter or option that the packages bound to an instance the scope covers do not
declare, is refused as `session_remedy_choices_invalid` (`fact_key`, `field`, and `instance_id` for a meaning
error). Unknown names are refused, never ignored. It is a separate record from
`session.resource_targets`: a remedy choice gates (no choice stops the task) and carries
permissions with parameter values, while resource targets only score.

### 3.2 The configuration default

| Key | Default | Meaning |
|---|---|---|
| `scheduling.remedy.enabled` | `true` | the switch of section 1 item 5 |
| `scheduling.remedy.max_per_round` | `2` (1 to 8) | remedy requests per blocked layer and round (section 5.4) |
| `scheduling.remedy.choices` | `[]` | rows of the section 3.1 shape (a JSON list of objects), host-wide |
| `instance.<instance id>.scheduling.remedy.choices` | the section's list | an instance's rows; when set, it replaces the section's list as a whole |

All four are settable (`ConfigureSettings`, `ac_config_set`); their defaults live only in
`actingd.defaults.json` ([runtime-config.md](runtime-config.md)). A row that breaks the shape
refuses the configuration as `config_value_invalid` (with its `config_key`) at load, in
`actingd check-config` and in `ConfigureSettings`. A row whose names the packages do not declare
(an unknown need, capability, parameter or option) is a check-config note
`policy_remedy_declaration_unusable` (`remedy_problem` `choice_invalid` or `parameter_invalid`)
and, at use, a stop (section 5.5): loud, never ignored. The chain depth is fixed at 3 and is not a
key.

### 3.3 Resolution (per instance; the Runtime never fills a gap itself)

For a blocked pair whose key K declares `need` N and `capabilities` C:

1. **Allowed set.** The first source that names N decides it entirely:
   1. the target record that applies to the instance (instance over game, as a
      whole): its `allow` rows of N, or its `none` row of N;
   2. the configuration list in effect for the instance: its rows of N;
   3. the mapping's `default`;
   4. nothing: the pair stops, `policy_remedy_unchosen` with `remedy_problem` `no_choice`.

   A `none` row gives the empty set: the pair stops, `choice_none`. The allowed set is the decided
   set intersected with C; a decided capability outside C, or an empty intersection, stops the pair
   with `choice_invalid`. The deciding source is recorded as `remedy_source` (`target`,
   `configuration`, `package`).
2. **Parameter values**, per parameter of the provider's capability, independently: the target
   record's `parameter` row, else the configuration's, else the provider's declared `default`. A
   value that is not one of the provider's options is `parameter_invalid`: that provider is not
   requestable; with no requestable provider left, the pair stops.
3. **Several allowed capabilities, or several providers of one:** all are requested; the existing
   ranking orders them (section 6). No fixed preference exists anywhere.

## 4 Run parameters (the channel shared with Workflow #411 B8)

- **Carrier.** Every policy dispatch of a task whose bound package declares parameters carries one
  info reason `policy_dispatch_parameters` with `run_parameters`: one record
  `{parameter_name, parameter_value, remedy_source}` per declared parameter of the package, its
  value resolved by section 3.3 item 2, and the source that gave it. A remedy request resolves the
  requested capability's parameters this way, and a normal policy run of a provider task resolves
  them the same way: a person's choice holds for every policy run of the package on that instance.
  The chain is persisted in the dispatch events, so the chosen values are in the ledger and survive
  a restart with no new member.
- **Overlay.** The host takes the values from the run's own reason chain, as the fact refresh's run
  reads its requested keys from its own chain ([scheduling/README.md](scheduling/README.md), "Fact
  Refresh"), and adds the record `session.dispatch.parameter.<parameter_name>` =
  `<parameter_value>` (string; instance scope; confidence 1000; observed at the dispatch's
  admission; no expiry) to every instance fact snapshot it takes for that run: the select steps'
  snapshots and any other in-run read of instance facts. The package's selection policy declares
  the key as an `enum_string` field with the declared options and should use
  `on_unknown: abort_evaluation` ([selection-policy.md](selection-policy.md), "In-task consumer").
- **Runs without a dispatch** (direct runs, Lab online and offline runs): the overlay holds every
  declared parameter with its declared default. Nothing is recorded for them; the defaults are
  package data, so the run is reproducible from the package.
- **Never stored.** The overlay appends no record, takes no fact identity, and is not visible to
  `InstanceFactSnapshot` or `ac_facts`.
- **Reserved prefix.** `PublishFact` / `PublishFacts` of any key under `session.dispatch.` is
  refused as `session_dispatch_key_reserved` (non-fatal request error, before the fact write gate,
  from every origin, nothing appended).
- **Selection record.** The select step's `task.selection_evaluated` keeps its `input_sha256` over
  the snapshot, overlay included, so two runs that differ only in a value differ in that hash.
- Workflow #411 B8 adds its own typed values to the same reason and the same overlay.

## 5 Scheduler flow

### 5.1 Words

- **Blocked run B:** a policy run of task T on instance I that succeeded with a key K whose mapping
  declares a remedy, while `scheduling.remedy.enabled` is on. The host reads K from B's terminal
  `scheduling_disposition`, whether or not the active catalog references K.
- **Round:** opened by a root blocked run (a blocked run that was not itself a remedy request),
  owned by the pair (T, I) and identified by B's `decision_id`. It holds a **chain** of layers 1 to
  3; layer 1 is (T, K). A round ends **resolved**, **stopped** or **expired**.
- **Provider:** a catalog task on I whose bound package `provides` a capability of the layer's
  allowed set; its pair is not the layer's blocked pair and is not already in the chain.

### 5.2 Hold

While a round is open, its root pair and every blocked pair of its chain are deferred with
`policy_remedy_waiting` (info; no wake of their own: the round's events wake the evaluation). The
deferral takes position **8a** of the order of deferrals
([scheduling-blackout.md](scheduling-blackout.md), "Order of the deferrals"): after positions 1 to
7, before availability. A stopped pair is deferred at the same position with its stop code
(section 5.5).

### 5.3 Request

For each open layer whose last blocked run has no settled remedy since:

- every requestable provider of the allowed set is treated as triggered and past its cooldown, as
  the immediate rerun (R22, `failure_retry_immediate`) and an owed rerun (`policy_rerun_owed`) are,
  and is **promoted** ahead of the instance's other candidates, with one reason
  `policy_remedy_requested` per blocked pair it would serve;
- everything else still applies to it: placement, the feedback stop, positions 1 to 8 of the
  deferrals, budgets, activity windows, one winner per instance and the host budget;
- one provider dispatch may serve several blocked pairs of the instance (several tasks blocked on
  the same need); a dispatch carries at most 16 `policy_remedy_requested` reasons;
- **not requestable**, a persistent condition: no provider task on I (`capability_unbound`); every
  provider is refused by the table's cycle check or is already in the chain (`cycle`); the
  providers' parameter declarations disagree (`parameters_mismatch`); every provider pair is
  paused, its package suspended, or it is disabled for I by an override or a placement refusal
  (`provider_unavailable`). When nothing else is requestable, the layer stops with
  `policy_remedy_unavailable`. The persistent gates are the clock-slot table's persistent codes
  (`policy_task_paused`, `pack_admission_suspended`, `instance_override_disabled`,
  `capability_missing`, `activity_profile_missing`);
- **only transiently held** (a budget, an activity window, an interval, a backoff, a blackout,
  pacing, a held instance, a dispatch in flight): the layer waits, and T keeps
  `policy_remedy_waiting` with `gate_code` and, when known, `next_eligible_unix_ms`.

### 5.4 Settle and retry

A layer's remedy is **served** by the first successful run, admitted after the layer's last
blocked run, of any provider of the allowed set on I whose key carries no remedy, requested or not
(a provider that ran on its own trigger serves too). Then:

- the layer's blocked pair is **retried once**: treated as triggered and past its cooldown,
  promoted, with the reason `policy_remedy_retry`;
- the retry ends on a key without a remedy, or on any other success: the layer is **resolved**; for
  a layer k > 1 the remedy of layer k − 1 is then served (its blocked pair is retried next);
  resolving layer 1 resolves the round;
- the retry ends blocked again (on any remedy key): a new cycle of the same layer, unless the layer
  used `scheduling.remedy.max_per_round` requests: then the round stops with
  `policy_remedy_exhausted` (`round_limit`);
- the retry **fails** (a real failure): the round closes; the pair's failure is handled as every
  failure is (the immediate rerun, the failure identity); no remedy code;
- a requested provider **fails**: that request is used; the layer may request again (the
  provider's own immediate rerun is the same dispatch and carries both reasons) until
  `max_per_round`, then `policy_remedy_exhausted` (`round_limit`);
- a requested provider ends **blocked** on a key with a remedy: it becomes layer k + 1 when k < 3;
  at k = 3 the round stops with `policy_remedy_exhausted` (`depth_limit`).

### 5.5 Stop for a person

A round stops with one of three codes, each with `remedy_problem`:

| Code | `remedy_problem` | Meaning |
|---|---|---|
| `policy_remedy_unchosen` (warning) | `no_choice`, `choice_none`, `choice_invalid`, `parameter_invalid` | no source allows a usable remedy |
| `policy_remedy_unavailable` (error) | `capability_unbound`, `cycle`, `parameters_mismatch`, `provider_unavailable` | allowed, but nothing can serve it |
| `policy_remedy_exhausted` (warning) | `round_limit`, `depth_limit` | remedies ran and the block stayed |

- The **root** pair stays deferred with the code (position 8a, no wake) across windows and
  restarts until the stop is lifted (section 5.6); the chain's other pairs return to their own
  triggers.
- The stop is written once as a `policy.planning_signal_observed` of kind `goal_missed` whose
  `signal_id` is `signal:remedy:stopped:<hex>`, `<hex>` being the lowercase SHA-256 of the UTF-8
  bytes `task \0 instance \0 decision_id` of the round; its `instance_id` is the instance alias, its
  `task_id` the root task, and its outcome the stop code with its values, among them
  `remedy_inputs_digest`. It is checked against the planning-signal projection before it is
  written (the clock-slot rule), so it is written once per round, across restarts too.
- It is visible in `actingd suspended` (the list `remedies`,
  [policy-suspension.md](policy-suspension.md)), in MCP `ac_diagnose` (the instance's `remedies`
  rows), in the not-running row once SR-6b has it, and in the clock-slot records: a stop code is a
  persistent gate (`clock_slot_refused`), `policy_remedy_waiting` a waiting gate
  ([scheduling-blackout.md](scheduling-blackout.md)). It never starts the recovery ladder.

### 5.6 Lift

A stopped root pair returns to its own trigger rules (an owed clock occurrence then runs at once)
when:

1. **its inputs change:** each evaluation recomputes `remedy_inputs_digest`, the SHA-256 over the
   versions of the target records that apply to I, the configuration rows in effect for I, the
   remedy table entries of (T, K) and of every provider of K's capabilities (their declarations
   and the provider task set), and the active catalog hash. A digest different from the stop's
   lifts it. Writing a choice, changing the configuration, loading a package whose declarations
   changed or activating another catalog therefore lifts it at the next evaluation, without a
   restart. The digest hashes declarations and versions, never a package's content;
2. **a person lifts it:** `actingctl remedy lift --instance <selector>` or `ac_remedy_set` with
   `lift: true` republishes the instance's record (the same rows, or an empty list when there is
   none, with a new observation), which changes the digest. A person who fixed the game by hand
   lifts and the task runs once.

No timer lifts a stop. A lift is written once as a `goal_missed` planning signal
`signal:remedy:lifted:<hex>` (the round's `<hex>`), outcome `policy_remedy_lifted`.

### 5.7 Expiry

A round that is neither resolved nor stopped when the activity window of its root blocked run ends
**expires**: `policy_remedy_expired` (warning), written once as a `goal_missed` planning signal
`signal:remedy:expired:<hex>`; the pairs return to their trigger rules. A transient wait therefore
lasts at most one window; the next window's run opens a new round if the block is still there.

### 5.8 State without new persistence

The host derives every round from records the ledger already holds, live and at start:

- the pair's latest settled policy run and, when it succeeded, its terminal's
  `scheduling_disposition.outcome_key` (one terminal read per settlement, the read the settlement
  already makes for referenced keys, now made whatever the catalog references);
- the dispatches after it whose chains carry `policy_remedy_requested` or `policy_remedy_retry`
  naming the round (`task_id`, `decision_id`), their admissions and their settlements;
- the round's stop, lift and expiry signals (`signal:remedy:`).

The open rounds are held in memory only, per pair at most 64 runs since the pair's latest root
blocked run, and rebuilt in ledger order at start (as the immediate rerun's retry rounds are).

### 5.9 Admission

An intent whose chain carries `policy_remedy_requested` or `policy_remedy_retry` is admitted only
while the round and the layer it names are still open and not yet served (the state may change
between evaluation and admission). Otherwise it is refused as the existing refused admission
(`policy.dispatch_rejected`) with `policy_remedy_round_closed` (info): no lease, no run. The
target record and the configuration are inputs of the evaluation, so a write between evaluation
and admission already refuses the intent as `policy_facts_stale` (the record is an overlaid
instance fact, like `session.resource_targets`).

## 6 Ranking between allowed remedies

The source order and the requestability rules of section 5.3 are the gate; every requestable
provider is a promoted candidate, and the **existing ranking** orders them: the total from
priority, aging, strategic weight, urgency, contention and the score stage's effective value
(selection document, manual offsets, resource targets) ([scheduling/README.md](scheduling/README.md),
"Score-Assisted Priority"). The winner's `rank_breakdown` and the others'
`decision_candidate_not_selected` record why the others were not chosen. A cost term for declared
`consumes` comes only with Workflow #411 B16 part 2; until then nothing else orders remedies. A
person who wants one remedy over another allows only that one in the target record, or sets a
priority offset on its task.

## 7 Host pieces

### 7.1 The remedy table

Built at start and when a catalog is activated; rebuilt at most once per
`scheduling.suspension.lift_check_min_interval_ms` while a round is open or a stop is recorded, so
a package fixed in place is seen without a restart; and, once Workflow #389 U-3 implements them,
at `LoadPack` and `UnloadPack`. For each catalog task, its bound package (the procedure manifest's
location) is read metadata-only, as the fact refresh reads its marks: its mappings' remedies and
its `provides`. The table maps (task, outcome key) to its remedy and each capability to its
provider tasks (with their scope, irreversibility and parameters). Then:

- **cycles:** the task graph "blocked task to the providers of its remedies' capabilities" is
  checked; every edge on a cycle is refused (`cycle`); the rest stays usable;
- **unbound capabilities:** a capability named by a bound remedy with no provider task in the
  catalog for an instance its task covers (`capability_unbound`);
- **parameter disagreement:** providers of one capability on one instance with different parameter
  declarations (`parameters_mismatch`).

A package that cannot be read contributes nothing and is named in the finding; building the table
never fails a start or an activation. Each finding is a check-config note
`policy_remedy_declaration_unusable` (warning) and, at use, the stop of section 5.5. The table is
memory only.

### 7.2 Inputs to the evaluator

The pure policy crate plans remedies from the table, the pairs' recent runs (section 5.8), the
recorded stops and the signal ids already written, the configuration rows per instance,
`scheduling.remedy.enabled` and `max_per_round`, and reads the target record from the
evaluation's facts. The plan gives, per pair, a trigger override (a request or a retry, with its
reasons and promotion) or a deferral (a hold or a stop, for position 8a), the run parameters per
pair, and the stop, lift and expiry signals. The evaluation's output keeps its shape; the host
appends the signals (deduplicated by `signal_id`) and places the deferrals in its eligibility
callback at position 8a. With the switch off or an empty table the plan is empty and every
evaluation is byte-identical to one without remedies.

## 8 Records (where every choice is)

| What | Record (existing) | Content |
|---|---|---|
| the blocked result | the `task.completed` terminal's `scheduling_disposition` | `outcome_key`, effect (unchanged) |
| the remedy request and its source | the provider's dispatch chain: `policy_remedy_requested` | `task_id` (the blocked task), `decision_id` (the round), `outcome_key`, `need`, `capability`, `capabilities_allowed`, `capabilities_declared`, `remedy_source`, `remedy_depth`, `count`, `limit_count`, `irreversible` |
| the chosen parameter values | the same chain: `policy_dispatch_parameters` | `run_parameters` records (name, value, source) |
| why the other providers lost | the same chain: `rank_breakdown`, `decision_record`, `decision_candidate_not_selected` | as today |
| the retry | the blocked task's chain: `policy_remedy_retry` | `task_id`, `decision_id`, `outcome_key`, `capability` (served), `count`, `limit_count`, `remedy_depth` |
| a hold | the blocked task's decision: `policy_remedy_waiting` | not persisted (a deferral), as every deferral |
| stop, lift, expiry | `policy.planning_signal_observed`, kind `goal_missed`, `signal:remedy:*` | the code with its values |
| the person's or agent's choice | `fact.published` of `session.remedy.choices` | rows, the writer's origin |
| the configuration | `config.parameters.scheduling` and `config.parameters.instance` rows | as every key |
| a refused admission | `policy.dispatch_rejected` | `policy_remedy_round_closed` |

**No persisted ledger shape changes**: every record above exists; the new codes, keys and
vocabulary tokens are catalog entries, and the new package members ride the shared `package`
interface revision 4 ([v012-register.md](v012-register.md) §1.110).

## 9 Failures and EXEC-3

- A run that ends on a remedy key is a success: no failure identity, no streak, no rerun, no pause;
  it clears the pair's failure streak and its immediate-rerun round
  ([policy-suspension.md](policy-suspension.md), "Which failures accumulate").
- So a task blocked again and again never pauses by repeating; the remedy round bounds it instead
  (`max_per_round`, the depth, the stop).
- A provider's or a retry's real failure is an ordinary failure with its own identity.

## 10 Validation (loud at every layer)

| Where | What |
|---|---|
| `actinglab package check`, kernel admission, the source parser | sections 2.1, 2.2, 2.4 |
| `package bundle`, `actinglab standard-pack check` (the game root) | across the packages of the game root: every capability a remedy names is provided by a package of the root; no cycle over packages; providers of one capability declare identical parameters; one warning `package_bundle_remedy_invalid` per finding (`remedy_problem` `capability_unprovided`, `cycle` (one entry per package on the cycle) or `parameters_mismatch`) ([package-reference.md](package-reference.md)) |
| `actingd check-config` (catalog and packages) | the table findings of section 7.1 and the meaning of the configuration rows, as notes `policy_remedy_declaration_unusable`; the outcome stays `check_config_passed`, nothing is written |
| writing choices | `session_remedy_choices_invalid` |
| run time | the stops of section 5.5 |

A remedy whose capability nothing provides is therefore a bundle-check entry, a check-config note
and a visible stop at run time: never a silent no-op.

## 11 Interplay

- **Budgets, activity windows, sampled intervals, backoff:** apply to providers and retries; they
  make a layer wait, never stop; a wait ends with the window (section 5.7).
- **Operator pause, held instance, package suspension, blackouts, the pack registry, the pack
  schedule, pacing** (positions 1 to 7): come first; a held round simply waits.
- **Performance pacing:** stretches or pauses a provider or a retry like any run.
- **Recovery ladder:** no remedy code starts it; a blocked result is a success.
- **Fact refresh:** unchanged; a provider candidate may raise demand like any candidate.
- **Feedback stop:** applies to a retry, as to the immediate rerun; a catalog must not let a remedy
  key satisfy the task's feedback stop (a resource-side rule).
- **Clock slots:** a stop is a persistent gate (`clock_slot_refused`), a wait a waiting gate; a
  remedy dispatch of a provider after one of its occurrences serves that occurrence.
- **Catalog routines** that already order "blocked, then a sink" with outcome predicates must be
  removed when the packages declare remedies, or both will run.
- **Workflow #411 B16 part 1** (`on_max_runs` to a named key) and **B18** (refusal markers): such a
  key may carry a remedy like any key.

The effect condition says what happened to the designated effect; whether the business result is
the expected one is the key's remedy declaration. No third effect value is needed.

## 12 Codes

| Code | Owner | Category | Values (optional in parentheses) | Emitted by |
|---|---|---|---|---|
| `policy_remedy_waiting` | policy | info | `task_id`, `instance_id`, `outcome_key`, `need`, `decision_id` (`gate_code`, `next_eligible_unix_ms`) | R2 |
| `policy_remedy_requested` | policy | info | `task_id`, `instance_id`, `decision_id`, `outcome_key`, `need`, `capability`, `capabilities_allowed`, `capabilities_declared`, `remedy_source`, `remedy_depth`, `count`, `limit_count`, `irreversible` | R2 |
| `policy_remedy_retry` | policy | info | `task_id`, `instance_id`, `decision_id`, `outcome_key`, `capability`, `remedy_depth`, `count`, `limit_count` | R2 |
| `policy_remedy_unchosen` | policy | warning | `task_id`, `instance_id`, `decision_id`, `outcome_key`, `need`, `remedy_problem`, `remedy_inputs_digest` (`remedy_source`, `capability`) | R2, R4 |
| `policy_remedy_unavailable` | policy | error | as `policy_remedy_unchosen` | R2, R4 |
| `policy_remedy_exhausted` | policy | warning | as `policy_remedy_unchosen`, and `count`, `limit_count`, `remedy_depth` | R2, R4 |
| `policy_remedy_lifted` | policy | info | `task_id`, `instance_id`, `decision_id`, `remedy_inputs_digest` | R2, R4 |
| `policy_remedy_expired` | policy | warning | `task_id`, `instance_id`, `decision_id`, `outcome_key`, `need`, `window_end_unix_ms` (`gate_code`) | R2, R4 |
| `policy_dispatch_parameters` | policy | info | `run_parameters` | R2 (B8 adds its values) |
| `policy_remedy_round_closed` | runtime-host | info | `task_id`, `instance_id`, `decision_id` | R4 |
| `policy_remedy_declaration_unusable` | runtime-host | warning | `remedy_problem` (`task_id`, `outcome_key`, `need`, `capability`, `instance_id`, `package_id`: a finding on a blocked mapping names all of `task_id`, `outcome_key` and `need`; a configuration row with an unknown need names `need` only of the three; a `parameter` row names none of them, and the developer detail names the row) | R7 |
| `session_remedy_choices_invalid` | runtime-host | error | `fact_key`, `field` (`instance_id`) | R6 |
| `session_dispatch_key_reserved` | runtime-host | error | `fact_key` | R5 |
| `package_bundle_remedy_invalid` | resource-tooling | warning | `package_id`, `remedy_problem` (`need`, `capability`, `outcome_key`) | R1 |

New keys: `need`, `capability`, `parameter_name`, `parameter_value` (tokens),
`capabilities_allowed`, `capabilities_declared` (lists of tokens), `remedy_source` (vocabulary
`remedy_source`: `target`, `configuration`, `package`), `remedy_problem` (vocabulary
`remedy_problem`: the tokens of sections 3.3, 5.3, 5.5, 7.1 and 10), `remedy_depth` (integer),
`decision_id` (id), `irreversible` (boolean), `remedy_inputs_digest` (hash), `run_parameters`
(records of `parameter_name`, `parameter_value`, `remedy_source`). Every entry is `review:
pending`, `ladder_class` `none`, since 0.12.3. None of them triggers the recovery ladder; the
package format's refusals reuse `invalid_scheduling_outcome_declaration` (with the field) and
`contained_task_control_invalid` (with `reason`), as their siblings do.
