# Device groups: several applications on one device (MAPP, not built yet)

Workflow #437 MAPP-C (contracts only; [v012-register.md](v012-register.md) §1.323). **Every rule of
this file is (MAPP, not built yet)** unless its section names MAPP-1 ([v012-register.md](v012-register.md)
§1.328, Runtime 0.12.56), MAPP-1b (§1.330, Runtime 0.12.57), MAPP-2a (§1.342, Runtime 0.12.59) or
MAPP-2b (§1.353, Runtime 0.12.60):
no code, key, code, value key or word named here exists until the code slices of §1.323 build it,
and every code, value key and vocabulary word below is proposed but those MAPP-1 registers:
`device_group`, `instance.<instance id>.device_group`, `device_group_config_invalid` with the
vocabulary `device_group_problem`, `device_group_assembled`, `device_group_waiting` with the
vocabulary `device_group_wait`, (MAPP-1b) `device_group_action_refused`, (MAPP-2a)
`application_switch_failed` with the vocabulary `application_switch_step`, and (MAPP-2b) the key
`scheduling.device_group.switch_failure_cooldown_ms` and the optional value `failure_code` of
`dispatch_instance_cooldown`. The coordinator cut
MAPP-1 in two: MAPP-1a (§1.328) builds the configuration form and its checks, the refusal of every
group of several, the member → device key map, the scheduler keyed by device and the evaluator's
one pick per device; MAPP-1b (§1.330) builds the host's worker, occupancy, admission guard and
pause per device, the gates of the group, the session rule, the refusals of device-level actions
and a chain handover's successor before the device's other members. The player hold's own
acquire, release and second-hold refusal are not built (the player hold itself is not built yet,
Workflow #429 R-7b). The coordinator cut MAPP-2 in two as well: MAPP-2a (§1.342, built in Runtime
0.12.59) builds the switch's parts (another member's stop under the device's key, the member's
startup run on the borrowed key, the stopped member's page forgotten), their one entry and its
code, with no caller in the Runtime; MAPP-2b (§1.353, built in Runtime 0.12.60) calls the switch
from the run paths, takes its time out of the run's clock, settles a run with its failure, adds
the member's switch cooldown and its key, and sizes a member's dispatch lease for the switch and
the run ("The switch before a run"). The load still refuses every group of several, so no root
reaches any of it. The other contracts point here instead of repeating the rules.

The owner settled the direction on 2026-11-11 (board #437): one device may hold several applications
that the scheduler serves in turn; one application is in front at any time; one device has one
queue, and a task of another application that comes due waits in it even where waiting loses some
yield; a task package that is running is never interrupted; the queue's order is by weight and by
time; and 「切换到另一个应用时，原先的应用直接杀掉就行」, starting and killing by package name through
ADB. Everything else in this file is the coordinator's rulings of 11-11 and its answers of 11-12 to
the fourteen questions of §1.323, written as the code allows them, not her words. Those questions
are settled; each answer stands in the section it belongs to, and §1.323 lists them.

MAPP-1c ([v012-register.md](v012-register.md) §1.350, built in Runtime 0.12.60) closes the two
cases in which a member's session stayed open after a keyless observation ("The session close of
a keyless observation"; "Device-level actions", rows 7 and 15) and keys the device's queue-order
lock by the device. MAPP-1d ([v012-register.md](v012-register.md) §1.359, built in Runtime
0.12.60) closes the three gaps MAPP-1c's review found and listed as open: the monitor's probe
("The monitor's probe in a device group"; row 15), the hand-over at a lapsed run lease's expiry
(row 7) and the queue's grant judged by one member's gate ("One queue, one worker, one lease per
device"). What a section says of MAPP-1c and MAPP-1d is built.

## Words (MAPP, not built yet)

- **Device**: one emulator of the emulator manager, or one handset: what one ADB endpoint reaches.
  An emulator is its port, a handset its serial ([device-binding.md](device-binding.md), "Identity,
  label and fingerprint").
- **Instance**: today's instance entry, unchanged: one application on one device, with its own
  alias, instance id, `application_id`, policy identity (its `policy.facts.instances[]` row, one
  game and one server), facts, counters, pause, hold and history.
- **Device group**: the instances that declare the same device. Every instance is in exactly one
  group; an instance that declares none is a group of one. **Member**: an instance of a group.
  **The member in front**: the member whose `application_id` the device reports in the foreground.
- Nothing a person reads calls an instance an account. (An account is the login of one
  application; the Runtime does not model it.) How the console and the command line show a device
  with its applications is a later slice ("Later slices").

## The configuration form (MAPP-1, built)

An `instances[]` entry takes one optional member, `device_group` (`actingd-config` revision 29,
in force from Runtime 0.12.56: [component-interfaces.md](component-interfaces.md), row 29 and
"Planned: Workflow #437 MAPP"):

```json
{"alias": "neutral.a", "instance_id": "instance_<32 hex>",
 "application_id": "com.example.fixture.a", "instance_index": 2,
 "touch_backend": "adb_shell_input", "capture_backend": "adb",
 "startup_package": {"package": "packages/neutral-a-startup.zip"},
 "idle": {"action": "none"}, "recovery_ladder_profile": "handset",
 "device_group": "device.one"}
```

- `device_group` is a name of 1 to 64 bytes under the alias rules (`validate_alias`,
  `crates/runtime-host/src/provider.rs`); any other value is refused with `name_invalid` (below).
  It names no device value: no serial, no port, no index.
  Entries that name the same value are one group. An entry that names it alone is a group of one
  and behaves as an entry without it.
- It is read once at assembly and is not settable. The manifest reports it as the row
  `instance.<instance id>.device_group` (section `instance`, not settable, read at assembly), as it
  reports `device_kind`. An entry without the member has no such row.
- **Why a member of the entry.** The entry already holds everything else of the instance, its
  device keys included. It denies unknown members (`InstanceConfig`,
  `apps/actingd/src/config.rs:1036-1169`, `#[serde(deny_unknown_fields)]` at 1037), so every build
  before revision 29 refuses the file with `config_decode_failed` (config.rs:1476) at check-config
  and at start. A top-level section would be refused just as well (the file denies unknown
  members), but it would be a second list of aliases that must be kept equal to the entries.

**A group of several** (two or more entries with one `device_group`) is assembled only when every
rule below holds. Each broken rule refuses the configuration at stage `assemble`, in check-config
and at start alike, with `device_group_config_invalid`. That code is registered (MAPP-1): error,
ladder class `none`, owner `actingd`. Its values are `aliases` (the entries concerned), `config_key`
(the member that breaks the rule) and `device_group_problem` (a registered vocabulary). MAPP-1
checks the name first, on every entry that names a group (`name_invalid`), then the kind rules
(`handset_member`, `fixture_member`) before `device_mismatch`, then the table's order
([v012-register.md](v012-register.md) §1.328, points 1 and 4).

| Rule | `config_key` | `device_group_problem` |
|---|---|---|
| Every entry that names `device_group` names it by a name of 1 to 64 bytes under the alias rules: not empty, at most 64 bytes, no control character. It applies to a group of one too, and `aliases` is the entry: its alias is right, its member is not | `device_group` | `name_invalid` |
| Every member names the same device by the same key with the same value: all the same `instance_index`, all the same `instance_name`, or all the same `host` and `port` (and the same `serial` when one is named). check-config never runs discovery ([actingd-check-config.md](actingd-check-config.md), "Discovery-bound instances"), so it cannot prove that an index and a name reach one emulator | the key that differs | `device_mismatch` |
| No member is a handset (`device.kind` `handset`). The private binding file binds one serial to one alias ([device-binding.md](device-binding.md), "The private binding file", `duplicate_serial`); a group-aware binding is a later slice | `device` | `handset_member` |
| No member is a fixture entry | `fixture_backend` | `fixture_member` |
| Every member has its own `application_id`. The kill and the foreground gate tell members apart by package name ("Identity on the device") | `application_id` | `application_shared` |
| Every member declares a `startup_package`. The switch reaches the member's home only through it ("The switch before a run") | `startup_package` | `startup_package_missing` |
| No member turns on a device-level action that slice 1 does not make group-aware ("Device-level actions"): `start_emulator` `true`; an `idle.action` in effect other than `none` (the defaults file's `close_app` included, so a member names `none`); a ladder profile in effect whose rungs hold `emulator_restart`. The profile in effect is the member's `recovery_ladder_profile`, else its kind's profile, else the section's `rungs` ([emulator-control.md](emulator-control.md), "Ladder profiles"). An emulator names no profile by default and climbs the section's `rungs`, which hold that rung, so a member names a profile without it: the neutrally named profile MAPP-1 adds to the defaults file's `recovery_ladder.profiles` (a value of that existing key, with no `emulator_restart`; its name is MAPP-1's), or the defaults file's `handset` (`return_home`, `application_restart`). Under the default `rules.start_beyond_profile` `exhaust`, a trigger that starts beyond it (`navigation_exhausted`, start rung 3) ends `exhausted`, as on a handset | `start_emulator`, `idle.action`, `recovery_ladder_profile` | `device_action_unsupported` |

A member's setting that conflicts with its group is refused, never overridden: the file is
corrected by a person (fail loud).

**Until the slice "order within one device"** ("Later slices", "Release"), a build refuses every
group of several at stage `assemble`, in check-config and at start alike, with
`device_group_config_invalid`, `config_key` `device_group` and `device_group_problem`
`several_members_unsupported` (MAPP-1). It is checked after the rules of the table, so a group
that breaks one of them is refused with that rule's problem first. A group of one is accepted. The
slice that builds the order within one device removes this refusal.

While the Runtime runs, an instruction that would break the table's last rule on a member of a
group of several is refused by the whole-instance check: `config_value_invalid` with
`config_value_problem` `section_check` on the key ([runtime-config.md](runtime-config.md),
"`ConfigureSettings`"). The keys are `instance.<id>.start_emulator`, `instance.<id>.idle.action`,
the section's `idle.action` where a member takes it, `instance.<id>.recovery_ladder_profile`, and
the `recovery_ladder` profile a member climbs by.

**Undeclared sharing.** Today nothing refuses two entries on one device. The registry refuses only
a duplicate alias and a duplicate instance id (`crates/runtime-host/src/provider.rs:1161-1186`).
The start's discovery resolves every deferred entry and keeps one alias per discovered index
(`apps/actingd/src/config/provider_startup.rs:105-120`, `bound.insert` overwrites). One worker is
created per registered instance (`crates/runtime-host/src/host/host_claims.rs:117-143`). So an
older build given two entries on one emulator registers both and drives the device from two
workers.

From MAPP on, two or more entries that are not one group and name the same device are refused with
`device_group_config_invalid`, `device_group_problem` `undeclared_shared_device`, and `config_key`
the shared key. The check runs in two places:

- **At assembly and in check-config**, when the entries name the same `instance_index`,
  `instance_name`, `serial`, or `host` and `port`. An explicit entry that names a `serial` is
  addressed by it, and its `host` and `port` are the daemon's defaults when it names none: two
  explicit entries are one device when both name the same serial, or when neither names a
  serial and their `host` and `port` are equal; a serial beside another serial or beside an
  endpoint proves no sharing ([v012-register.md](v012-register.md) §1.328, point 10).
- **At start, after the discovery**, when entries of different groups resolve to the same
  discovered index or ADB port: an explicit `HOST:PORT` beside a discovery-bound entry, or an index
  and a name of one emulator. An explicit entry that names a `serial` is left out of this
  comparison (its endpoint may be the daemon's default, the first emulator's port). It is
  recorded as every discovery refusal is, as a Provider startup failure
  ([provider-startup.md](provider-startup.md)).

Handsets are already covered: the private binding file refuses one serial bound to two aliases.
This refusal is the one difference a configuration without `device_group` can see, and only a
configuration that drives one device from two workers today meets it. Before MAPP-1 ships, the
production configuration files are read, read only, for two entries that name one device, so that
no production root meets the refusal at its start unannounced.

**What check-config and the start print.**

- check-config: the `instances[]` report row of a member that names `device_group` carries
  `device_group`, the name. A row of an entry without it is as today.
- The start: one daemon line per group of several, `device_group_assembled` (MAPP-1; info;
  values `aliases`, `device_label`, the latter the `HOST:PORT` the first member is bound to and
  absent while that endpoint is pending). It goes to the daemon log only, and nothing new is
  recorded in the ledger: each member records its own `runtime.instance_bound`, with the same
  endpoint.
- A group of one prints nothing new.

## A group of one is today (MAPP-1)

**The rule the code slices are tested against.** With no entry declaring a shared device, every
decision, every record, every key and every byte the Runtime persists is what it is today, and no
data is migrated. It holds because:

- The group is derived from the file at assembly ("Derived, never stored") and is the member
  alone. Its device key is the member's own instance id, so every map keyed by the device key has
  today's keys and values ("One queue, one worker, one lease per device").
- The new reason (`device_group_waiting`), the switch and its cooldown exist only when another
  member has the device, which a group of one never has. The group rules of the form apply only to
  a group of several, but the name's (`name_invalid`), which a file without `device_group` never
  meets.
- No record type, payload member, phase word, fact key, tag or closed vocabulary of the ledger is
  added ("Interfaces").
- A file without `device_group` keeps its revision (28 or lower). check-config's report and the
  manifest gain nothing for it but the switch cooldown's key (below). The worker thread keeps its
  name, `actingd-instance-<alias>`.
- The owner journal's active instances are the same ids (`protected_instance_ids`,
  `crates/scheduler/src/lib.rs:2585-2593`, written by `persist_active_instances`,
  `crates/runtime-host/src/host/lease.rs:3242-3247`).

There are three exceptions:

- the refusal of undeclared sharing above;
- the switch cooldown's key `scheduling.device_group.switch_failure_cooldown_ms` ("The switch
  before a run"). From MAPP-2b (Runtime 0.12.60) on, like every key of the `scheduling` section,
  it is a row of the manifest and of the program fact `config.parameters.scheduling` on every
  root, with its default ([runtime-config.md](runtime-config.md), "The `scheduling` section").
  Nothing a group of one does reads it;
- the defaults file's ladder profile `without_emulator_restart` (MAPP-1, "The configuration
  form"), a member of the default of `recovery_ladder.profiles`. On a root whose file and
  instructions leave that key at its default (a profile set the file names replaces the default
  whole), three things change, and only these: the value of the manifest's row
  `recovery_ladder.profiles`, which gains the profile; the program fact
  `config.parameters.recovery_ladder`, which carries that row; and the ladder's configuration
  digest, `config_digest` of `recovery_ladder_started`, the SHA-256 of the section's values in
  effect ([runtime-config.md](runtime-config.md), "The recovery ladder section"). A root whose
  instruction set the key keeps the instruction's value, and its start notes
  `config_override_base_changed` on the key, as for any default that changed under an
  instruction. Nothing climbs by the profile unless an entry names it
  (`instance.<id>.recovery_ladder_profile`): the defaults' `kind_profiles` do not name it, so
  every ladder climbs the rungs it climbs today.

## Derived, never stored (MAPP-1: the map, the scheduler and the evaluator; MAPP-1b: the workers and the guards)

At assembly the configuration gives one map, member → device key. A group's device key is the
smallest instance id of its members (`InstanceId` order); a group of one's is the member's own id.
The map is handed to the scheduler, the workers, the admission guards and the evaluator, and lives
in memory only. The group adds no persisted state: no state file, no fact, no tag. (The standing
rule: what survives a restart lives only in tagged ledger records. Alice's rule of 10-07: one
queue, no second state.)

A restart rebuilds the map from the configuration in effect at that start, and with it empty queues,
the workers and the guards, as every start does today: leases and queues are memory only already
(`ClaimKind`, `crates/scheduler/src/lib.rs:107-108`, "it is never persisted"). The owner journal
keeps the member ids it keeps today. The next start maps each listed id to its group by the
configuration in effect and gives the whole group the takeover cooldown (`SeedScheduler::new`,
lib.rs:923-939).

## One queue, one worker, one lease per device (MAPP-1: the scheduler; MAPP-1b: the worker, the in-flight registration, the admission guard and the pause's wait; MAPP-1d: each entry by its own member's gate)

Today "the queue" of an instance is three things, each keyed by its instance id:

- **The scheduler's state**: `InstanceState { lease, last_release, cooldown_until_monotonic_ms,
  queue }` in `SeedScheduler.instances: BTreeMap<InstanceId, InstanceState>` (lib.rs:892-908).
- **The instance's worker**: `InstanceWorkers.senders: BTreeMap<InstanceId, Sender>`
  (host_claims.rs:107-143), thread `actingd-instance-<alias>`.
- **The in-flight registration** of a handed-off policy run, which is the instance's occupancy
  until its settlement (`policy_run_registered`,
  `crates/runtime-host/src/host/policy_dispatch.rs:101-134`; [scheduling/README.md](scheduling/README.md),
  "Per-Instance Lanes").

A policy dispatch is never queued: it goes only to a free instance (`ClaimKind::PolicyDispatch`,
lib.rs:111). The group keys these as follows:

- **The scheduler** keys `InstanceState` by the device key: one lease, one queue, one cooldown and
  one last release per device. There is no lock beside the queues: the one `InstanceState` per
  device is the exclusion.
  - A queue entry keeps, in memory, the member it serves. `QueueEntry` (lib.rs:594-607) holds no
    instance today, because the map key gave it.
  - **Each entry is judged by the gate of the member it serves** (MAPP-1d, built): every grant
    from the device's queue (the queue's own grant, the transfers at a lease's end, the immediate
    grants and the queueing) reads the routine gate of each member of the device
    (`HostShared::device_gates`, `host/device_group.rs`: the pauses, a #403 fence and blackout
    silence, as `routine_gate` reads them) and hands the scheduler one gate per member
    (`MemberGates`). An entry its own member's gate holds (a paused or fenced member's routine
    claim) stays in its place in the queue and does not block the entries of the device's other
    members behind it: the first entry, in the queue's order (priority, then arrival), that its own
    member's gate lets through is granted. Nothing is reordered, so once the gate opens the held
    entry is first again, in its place. A dedicated close lease is not such a grant: it is
    refused, as for an instance alone, by every waiting entry that the routine gate of the member
    asking for it lets through (`grant_dedicated_instance_lease`;
    [read-session-resource-close.md](read-session-resource-close.md)), so a claim its own member's
    gate holds refuses it too and the refusal stays #411 A52's non-fatal one (judged per member,
    that claim refused nothing, the scheduler refused the close lease as a queue ahead,
    `lease_busy`, and a member's capture failure on a free device ended the whole Runtime: found by
    MAPP-1d's review, fixed before its merge). The queue's
    grant then names the member the entry serves: its performance directive and a ladder's
    performance release are read for that member, and its `lease.granted` names that member (the
    grant of a member's entry pumped by another member named the pumping member before). The
    hand-offs to a holder's own ladder or claim keep the holder's member's gate: they concern that
    member's claim only. A person's hold is not a routine gate today (Workflow #429 R-7b builds the
    hold's gate); the group's hold of the policy's dispatch is MAPP-1b's (`player_hold`). For an
    instance alone on its device every entry is its own, its one gate is read once, and nothing
    changes.
  - A lease's token keeps the member's instance id (`LeaseToken::new`, lib.rs:1167-1173), so every
    record's links, the run's identity check (`PolicyRunLeaseInstanceMismatch`,
    `crates/runtime-host/src/host/contained_task.rs:7868-7877`) and the fence of a write stay the
    member's. `lease_locations` maps a lease to its device key.
  - Every entry point that takes an instance id resolves it to its device key first. An answer
    about the device (is there a holder, a waiting claim, a cooldown) is the device's. An answer
    about one member (does this member hold the device) compares the token's instance.
- **One worker per device.** The device's worker takes every member's host claims and handed-off
  policy runs, one at a time. Its thread is `actingd-instance-<alias>` for a group of one and
  `actingd-device-<device_group>` for a group of several (thread names are not persisted).
- **The in-flight registration** stays per member (decision id, member), and a member's occupancy
  reads it across the group. While any member's run is registered, the device is held for every
  member (`InstanceOccupancy::Held`, `crates/runtime-host/src/policy_host.rs:328-342`, `holder_kind`
  `policy_dispatch`). The admission's own check reads the same (`dispatch_held_error`,
  policy_dispatch.rs:182-214).
- **The admission guard** (`instance_guard`, lease.rs:3230-3240) is keyed by the device key, so a
  device step of one member never meets a keyless observation of another.
- **A scheduling pause of one member** waits only for that member's own work. Today the pause's
  release reads `active_lease(instance_id)` (`release_paused_instance`, policy_dispatch.rs:3126-3170),
  which would be the device's lease. It waits for a lease only when its token names the member, and
  refuses for a queued claim only when the claim serves the member.

So a group of several has one queue (the device's claims and the one pick of a round), one worker
and one lease. Every other key stays the member's: decisions, facts, counters, pauses, holds and
history are keyed by the member's alias or id, as today.

## One pick per device per round (MAPP-1: the one pick and `selected`; MAPP-1b: the host's `held`, `player_hold` and `fenced`)

Today an evaluation selects at most one candidate per instance: `selected_instances`
(`crates/policy/src/evaluator.rs:2186`), checked at 2236 (reason `instance_already_selected`,
2282-2285) and filled at 2502. The set becomes keyed by the device key:

- **How the evaluator learns the groups.** It takes the member → device map as an argument of its
  own, in memory, beside the instance holds (`InstanceHolds`, evaluator.rs:343-380) and the run
  slots. The map is never a member of `EvaluationFacts` (`InstanceSnapshot` denies unknown members,
  evaluator.rs:181-191) and never part of a decision identity (`deterministic_decision_id`,
  evaluator.rs:2332-2340, keeps the member's alias).
- **Who wins.** The candidates are already one ordered list across every instance
  (`allocation_order`, evaluator.rs:2166). The device's first candidate in that order wins the
  device, and every later candidate of any member of the device waits.

**Ranking across members: the order within one device (ORD-1b, since 0.12.60).** With the
order's inputs beside the map, the members of one device are ordered as
[scheduling/README.md](scheduling/README.md), "Order within one device", writes (register
§1.352; the owner's rulings on board #437):

- **The limit.** Every task has a longest time it may wait in the queue: its `max_queue_wait_ms`
  ([scheduling/v2/README.md](scheduling/v2/README.md)), else the order's default.
- **Stay with the front.** The device stays with the application in front while one of its due
  tasks, in that member's own order, ends before any waiting task of another member passes its
  limit.
- **Switch.** Otherwise the waiting task whose limit ends first goes (at equal ends, the earlier
  in the allocation order), and its run switches the application ("The switch before a run"). A
  running task is never interrupted.
- **Within a member** its own order stays (promoted, then the maintenance tier, then the business
  tier by `B ÷ E`); across members it is replaced by the rule above. The values of two members
  (`B`, the resource terms) are never compared, and the time a switch takes counts in no order and
  not in a run's own duration.

Without the order's inputs the device's first candidate in the allocation order wins it, as MAPP-1
built it; no Runtime build passes them before the host's slice of the order. Until the release
that first accepts a group of several, the load refuses every such group ("The configuration
form", "Release").

**The reason a member waits: `device_group_waiting`** (registered by MAPP-1; info, ladder class
`none`, owner `policy`). A candidate of a member that does not get the device because another member has it
carries this reason. It takes the place of the code a lone instance would give
(`instance_already_selected`, `dispatch_instance_held`, the control gates' codes) and has no wake
of its own: the other member's settlement, release or lifting wakes the next evaluation, as today.
With `selected` the candidate waits with exactly the state, the eligibility and the wake of a
candidate behind its own instance's winner (`instance_already_selected`); only the code and its
values differ (settled by the coordinator, [v012-register.md](v012-register.md) §1.328). Its
values:

- `instance_id` (required): the candidate's alias, as the policy's reasons name it.
- `alias` (required): the member that has the device.
- `device_group_wait` (required; a vocabulary registered by MAPP-1):
  - `selected`: the other member's candidate won the device this round;
  - `held`: a claim or a run of the other member holds the device, or waits for it first;
  - `player_hold`: a person holds the device through the other member ("Pause and hold");
  - `fenced`: the other member is fenced ("A failure stays where it belongs").
- `task_id` (optional): the other member's task, with `selected`, and with `held` while a policy
  run of the other member holds the device.
- `holder_kind` (optional): with `held`, the claim kind (`vocab:claim_kind`).

It travels where reasons travel today: the decision; the not-run explanation (`ac_schedule`,
`actingctl schedule`); a dispatch decision record's `decision_candidate_not_selected` `why`; and
the not-run history. When a member's own holder or gate holds it, that holder or gate keeps its
own code.

## Never interrupted (MAPP-1: a client `QueueLease`'s preemption rule; MAPP-1b: a chain handover's successor; the rest, not built yet)

A running task package of one member is never stopped for another member's task. A policy dispatch
is never queued (lib.rs:111), only a client `QueueLease` may ask for a preemption (lib.rs:605-606),
and the device's lease is held until the run's end. In a group of several, a client `QueueLease`
naming one member waits in the device's queue and asks for no preemption of a holder that serves
another member. On its own member it keeps today's rule.

Whatever today's rules run next on one instance runs next on its device:

- A ladder handed the key at the run's failed end ([emulator-control.md](emulator-control.md),
  "Triggers", task stage: the lease end becomes a transfer).
- A ladder's continuation, which is ordered first and never gated.
- A chain handover's successor, which goes before the candidates that outranked it on its instance
  (evaluator.rs:2167-2182). The device's other members wait behind it as any candidate ranked after
  it does (MAPP-1b, register §1.330: it is also moved before the candidates of the device's other
  members that outranked it, which wait `device_group_waiting` `selected` once it is dispatched; a
  device has at most one handover in a round, the first in rank order, and the candidates that
  wait under `policy_chain_handover_waiting` stay those of its own instance).
- An owed rerun, an immediate retry and a remedy's request. These are candidates of the member with
  their own rank: they keep the device when they win it in the round, as on a lone instance they
  win the instance.

Another member's due task queues behind: it waits with `device_group_waiting` until the device is
free and it wins a round.

## The switch before a run (MAPP-2a: its parts and its entry, built in Runtime 0.12.59; MAPP-2b: its callers, the run's clock, the lease, the failure's settlement and the cooldown, built in Runtime 0.12.60)

In a group of several, before a run of a member, the Runtime brings that member's application to
the front, under the device's lease.

**What is built (MAPP-2a, Runtime 0.12.59).** The steps below, as the parts
`HostShared::stop_member_application` (step 2, in `crates/runtime-host/src/host/switch.rs` over
`control_member_application` in `host/input.rs`), `HostShared::run_startup_package_on_borrowed_key`
(step 3, `host/startup_package.rs` over `run_prepared_package_on_borrowed_key` in
`host/contained_task.rs`) and `HostShared::forget_stopped_application_facts` (step 2's forgetting,
`host/runtime_facts.rs`), and their one entry `HostShared::switch_application` (also public as
`RuntimeHost::switch_application`). **What MAPP-2b builds (Runtime 0.12.60, register §1.353):**
the calls from the run paths named below (`HostShared::switch_before_run` and
`HostShared::stop_others_before_start`, `host/switch.rs`), the switch's time taken out of the run's
clock ("The run's clock"), a member's dispatch lease sized for the startup run and the run ("The
key's time"), a run's failed settlement with `application_switch_failed` ("When the switch
fails"), and the member's switch cooldown and its key ("The member's switch cooldown"). Every part
and every caller begins from a member of a group of several (`HostShared::switch_applies`, over
`in_group_of_several`), which the load refuses (`several_members_unsupported`), so no root reaches
them. An instance alone on its device keeps its path unchanged because the run path's existing
gates (the ED-2 gate among them) pass unchanged and because the slice's only entry is that
group-of-several check; the MAPP-2b gate test pins that the entry is never taken (no device step
of the switch, the lease `D_run + R`, a switch time of 0 for the settlement, no cooldown entry)
and that the run's records are equal, payload by payload with ids and times normalised, to a host
on which the entry is switched off.

**Where the switch runs** (MAPP-2b).

- For a policy run: inside `run_scheduled_contained_task` (`host/contained_task.rs`), after the
  run's context, identity, package and outcome checks (the outcome declaration's check is the last)
  and before the run's request lifecycle is appended (`append_scheduled_request_lifecycle`). So a
  run that its own checks refuse stops nothing. It runs on the device's worker, or on the driver's
  thread with `scheduling.parallel.enabled` off.
- For a run a person starts (a direct task run from the Lab, the console or MCP `ac_run_pack`,
  `run_contained_task`): on its claim's key once granted and its deadline set, before the run
  executes. Its request lifecycle was appended at its receipt, before its claim.
- Before a start of a member's application outside a switched run, the switch's steps 1 and 2
  only (`stop_others_before_start`: the member in front stopped, or every other member with the
  front unknown, nothing with the member's own package in front), since the caller's own action
  then starts the member's application: an `ApplicationLifecycle` `launch` or `restart` a person
  asks for (`ac_app`), on the request's key after its claim and before its command; and the
  ladder's application restart rung ([emulator-control.md](emulator-control.md), "Rungs"), on the
  ladder's key before the rung stops the member's own application. Not built, because nothing
  reaches it on a member: a startup package after an emulator start (an operator's emulator
  control of a member is refused, "Device-level actions", row 5, and a member names no
  `start_emulator`) and the release of a player hold (the player hold itself is Workflow #429
  R-7b's).
- Never inside a run. A package's own `application` effect restarts the member's application,
  which is in front already.
- Never while another member's run holds the device: the switch runs only at the start of a run
  that already holds the device's key, so a running task is never interrupted ("Never
  interrupted").

**Its steps, on the device's key** (the key of the member's run: its token on its connection).

1. **Read.** The key must still validate as the device's (`validate_write`); a key that does not
   fails the step `foreground_read` with the scheduler's refusal, before any device step. Then one
   foreground query through the member's registration: the read-only query of the foreground gate
   ([application-lifecycle.md](application-lifecycle.md), "The foreground gate";
   `observe_foreground_application`, provider.rs:972-988). When the package in front is the
   member's own `application_id`, nothing more is done and the run starts: no stop, no startup run,
   and no record is written. Which member's package is in front is told by the `application_id`
   each member's registration assigns, read from memory (`assigned_application_id` of the
   execution provider): no other device query is made.
2. **Stop.** Before any stop of a run's switch, the key's remaining time must cover the startup
   run of step 3 and the run's own (step 3, "The key's time"); otherwise the step
   `startup_package` fails with `contained_task_deadline_unavailable` and nothing is stopped. (A
   start outside a run runs no startup package, so it asks no time.) When the package in front is
   another member's `application_id`, that member's application is force-stopped under the
   device's key; when the package in front is no member's, or no resumed activity is reported,
   every other member's application is stopped the same way, in id order, one stop each, because
   the Runtime cannot tell which one may still run. Each stop (`stop_member_application`):
   - **Under the device's key.** The key's holder must be a member of a group of several and the
     member to stop another member of the same group, and the key must validate as the device's;
     a key of another device, and a member outside the holder's group, are refused before any
     device step (`instance_mismatch`). The application command validates the key as the device's
     key, not as the stopped member's.
   - **The stopped member's records.** A host-minted `ApplicationLifecycle { stop }` request for
     that member, with the switch's causation id C, writes `command.received` and
     `command.validated`, then the existing application command acts on that member's
     registration and package: `scheduler.admitted`, `application.intent` and
     `application.completed`, all under the stopped member's links (its instance, the key's lease,
     the request and C). This is how the ladder's application restart rung stops its application,
     on another member. Nothing is recorded under the member to run.
   - **No session left.** The session the stop used is closed under the key, whatever the stop's
     result ("Device-level actions", row 7).
   - **The page forgotten.** Once the force-stop has succeeded, before the session's close and
     whatever that close then does, the stopped member's `application.foreground` and `task.page`
     are dropped (`runtime.fact_invalidated`, reason `device_closed`, as an emulator stop drops
     them), so no chain handover of that member fires on a page it no longer shows. Its other
     facts stay. Who reads `application.foreground` and why dropping it is right:
     the foreground gate (it records a new reading when the value changes, so a dropped value makes
     the next reading recorded), the overview's `foreground` block (`actingctl overview`,
     `ac_overview`, the Lab: it shows no package rather than one no longer running), and the
     generic fact listing and the ledger's fact-key filter, which show the drop.
   What `am force-stop` does to a package that is not running is not verified on the devices.
3. **Start and home.** The member's startup package runs on the key as a host run, as after an
   emulator start ([application-lifecycle.md](application-lifecycle.md), "The startup package
   hook"), on the borrowed key (`run_startup_package_on_borrowed_key`).
   - It writes `runtime.lifecycle_observed` phase `startup_package_scheduled` under the run's
     request links with the causation id C the stops of step 2 carry.
   - The startup run then runs under C: self-minted request and correlation ids, origin
     `(Agent, Adapter)`, its full `task.*` chain.
   - Its first step is its `application` effect, `restart`: a force-stop and then a `monkey` launch
     of the member's `application_id` (provider.rs:902-929). So the application is started by its
     package name through the one existing path, an application launch over ADB, and the run
     succeeds only on the home page its package declares. Nothing on the launcher is tapped.
   - **The borrowed key.** The key is never renewed (a renewal mints a new token, and the run
     checks the one it holds throughout) and never ended: no release, no queued continuation, no
     new token. When the startup run ends, on success or on failure, the key is the run's again,
     its token unchanged and valid, and a claim of another member queued on the device stays
     queued. A key the startup run lost (it no longer validates) fails the step.
   - **The key's time** (MAPP-2b). A member's run is granted a lease for both: its startup
     package's response deadline `D_s`, its own response deadline `D_run` and the heartbeat reserve
     `R` (the scheduler's `maximum_client_heartbeat_interval_ms`, 5 000 ms by default), so
     `D_s + D_run + R`, where an instance alone on its device is granted `D_run + R` as before
     (`HostShared::dispatch_lease_ttl`, `host/switch.rs`, at the policy admission and for a direct
     run's claim). The run then asks of the key, at each check, its own time: the time to the
     key's expiry less what its lease holds beyond its response deadline (for a member's lease,
     the startup run's lease time `D_s + R`), so the time the switch's own steps take before the
     startup run is not asked again (`RunNeed::OwnTime`; a policy dispatch keeps that instant
     with its dispatch clock from its admission). Before any stop, and again before the startup
     run, the key's remaining time must cover the startup run's lease time and that time: it does
     on a lease sized so, and it does not on a lease sized for the run alone, which is refused at
     the step `startup_package` with `contained_task_deadline_unavailable` before any stop. The
     startup run's deadline is the earlier of its response deadline and the key's expiry less
     that time, so it gets its `D_s` and leaves the run its own time.
   - **Where the read and the stops go.** The time of the front read (step 1) and of the stops
     (step 2) is not part of `D_s`: the lease is `D_s + D_run + R`, and the run's deadline is cut
     at the lease's expiry less the reserve, so the run has `D_run` less what the read and the
     stops took (and less its own checks before the switch, as every run's deadline is counted
     from its grant). In the worst case the task has less than `D_run` and, when it needs all of
     it, fails as an ordinary timeout of its deadline; when the time since the grant passes
     `D_run` before the startup run, the check before the startup run refuses the key and the
     switch fails at `startup_package` with `contained_task_deadline_unavailable`. Nothing of the
     run's clock bounds the read and the stops: each ADB command they run ends within the ADB
     command timeout of the registration that runs it (an entry's `command_timeout_ms`, 12 000 ms
     by default), and a switch makes one read and at most one stop per other member.
   - **What cannot fit.** The scheduler grants at most the longest response deadline plus the
     reserve (`ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS`, 1 800 000 ms, plus `R`), so a
     member whose `D_s + D_run` exceeds 1 800 000 ms cannot be served: its dispatch is refused at
     the admission, before any lease and any device step, and its direct run before its claim,
     both with `contained_task_deadline_unavailable` (an existing code), the same way every time.
     A lone instance's lease and refusals are as before.
   - **The member that cannot fit, as built (a known gap).** The evaluator does not know the
     bound. It selects such a member's task again in every round, as for any due task; when the
     task wins the device's one pick, the round's admission writes the dispatch's intent and its
     refusal (`policy.dispatch_rejected`, severity error, `contained_task_deadline_unavailable`),
     and nothing else happens: no lease, no device step, no switch, no cooldown, no failure of
     the pair. The device's other members meet `device_group_waiting` (`selected`) in every such
     round and wait while the refused task keeps winning the pick, so it can starve its device.
     **With today's request deadlines every run of a member of a group of several is refused by
     this bound**: every startup package request (`apps/actingd/src/config.rs`,
     `StartupPackageConfigFile::request`), every configured procedure's request (the same file,
     the procedure request) and every composed pack request
     (`crates/runtime-host/src/host/standard_pack_schedule.rs`) carries the longest response
     deadline, 1 800 000 ms, every member must declare a startup package ("The configuration
     form"), and a fact refresh's request carries 60 000 ms; so `D_s + D_run` exceeds the bound
     for every run of every member, the refresh included, and only fixtures with shorter
     deadlines pass it. The lease across a switch is reworked in a later slice before any group of
     several is accepted, and such a task must never become a candidate: it is held where tasks
     are composed, in the slice that orders a device or the one that lets the load accept a group
     ("Release"). The admission's refusal stays as it is until then; the MAPP-2b gate pins
     today's behaviour in a test named for that later slice to replace
     (`mapp2b_known_gap_a_member_that_cannot_fit_is_selected_and_refused_every_round`).
4. **Run.** The member's run starts on the same key.

**The run's clock** (MAPP-2b). The time a switch takes counts in no order and in no run's own
duration (board #437). So the switch's time (from before its read to its end, the startup run
included, on the Runtime clock) is taken out of the run's own time:

- **The deadline.** The run's deadline, set before the switch, moves on by the switch's time,
  within the key's lease (`ContainedRunControl::postpone_deadline`; a direct run's deadline,
  anchored at its receipt, the same).
- **The dispatch clock.** A policy run's switch time is kept with its dispatch clock until its
  settlement (`PolicyDispatchClock::switch_ms`, in memory), which subtracts it from the elapsed time:
  the execution record's `runtime_ms`, and from it the run-duration sample, the
  `task.<task id>.last_duration_ms` fact and the budget comparison (with a factor segment above
  1000, the budget time less that segment's reduction), are the run's own. The record's
  `observed_at_unix_ms` stays the instant the run ended. A settlement after a restart measures the
  run from its own request, after the switch, as before. A run with no switch (every instance alone
  on its device) subtracts 0, so its runtime, its sample and its verdict are what they were.
- **Nothing else.** No sample of the switch's time is kept, no default switch time and no key for
  it exist, and the quiet-hours horizon is not changed.

After a switch the earlier application is not running, so at most one member's application is
alive on a device. A person who starts another application by hand is outside this rule ("Pause
and hold").

**The foreground gate** is unchanged: each member's pointer input is compared with that member's
own `application_id`. The switch sends no pointer input, so the gate does not run during it; any
inputs of the startup run meet the gate as every startup run's do.

**When the switch fails.** Each failure fails the member's run only, before any effect of the run,
with `application_switch_failed` (registered by MAPP-2a: error, ladder class `none`, owner
`runtime-host`, since 0.12.59), whose cause (`caused_by`) is the step's own failure. Its values are
`alias` (the member to run), `application_switch_step` (a vocabulary registered by MAPP-2a:
`foreground_read`, `stop`, `startup_package`), `failure_code` (the step's own code) and
`application_id` (the package the step concerned: the member's own for `foreground_read` and
`startup_package`, the stopped member's for `stop`). The steps done before the failed one stay
done. Two refusals come
back as themselves, not as `application_switch_failed`, because that code's required `alias` or
`application_id` would name nothing: a member to run, a key's holder or a member to stop that the
registry does not hold (`instance_mismatch`), and a member to run or a member to stop whose
registration assigns no application (`application_effect_requires_assigned_application`). Each
comes before any device step of the part that meets it, and neither is reachable while the load
refuses every group of several.

**How the run ends** (MAPP-2b). A policy run whose switch failed ends there, before its request
lifecycle, and is settled as every run refused before its request lifecycle is: a failed
settlement of the pair, its failure the code's top node, its lease released
(`run_scheduled_policy_task`, `host/policy_lanes.rs`, then `record_scheduled_policy_failure` and
`complete_scheduled_policy_failure`, `host/policy_outcome.rs`). A direct run's switch fails after
its request lifecycle: its key is released (`cleanup_composite_failure`) and its receipt is failed
with the code. An `ApplicationLifecycle` request's stop fails the request the same way, its key
released, and its own command is not sent. The ladder's application restart rung fails with the
code as its reason, and the ladder keeps its key and goes on as after any failed rung.

**Never a ladder from a failed switch** (MAPP-2b; the survey's "not verified" settled by reading the
code). A ladder takes the device's key only at the failed end of a run that committed a
`task.failed` terminal: the policy run's ladder trigger is its control's committed failed terminal
(`ContainedRunControl::ladder_trigger`, which takes it with the ladder source noted before the
switch), read only by `cleanup_composite_failure_with_run_links` at a run-linked lease end, and a
direct run's queues one only through `ladder_after_direct_run` at the same function's unlinked end
(staged by `with_recovery_ladder_staging` at the end of the direct path, which a failed switch
returns before). A run whose switch failed has no `task.*` record of its own (the startup
run's are the startup run's, which never starts a ladder), and its key is released by
`ensure_scheduled_policy_lease_released`, which passes no trigger, or, for a direct run, by
`cleanup_composite_failure`, which releases the key and queues nothing.

| Step | What its failure is and does |
|---|---|
| `foreground_read` | A key that no longer validates as the device's (the scheduler's refusal), before any device step. The query's ADB failure is handled as the gate's: the member's `device.connected`, `application.foreground` and `task.page` are invalidated with `adb_unreachable`, and an unconfirmed ADB cleanup fences the member. `failure_code` is `application_foreground_unknown` or `instance_resources_fenced` |
| `stop` | A key of another device or a member outside the holder's group, before any device step (`instance_mismatch`), or a key that no longer validates. The stop's own failure records, as the rung's (`application.failed` and the lifecycle failure). A fatal device error with confirmed resources fences the member whose application was being stopped (#411 A46). The stopped member's page is not forgotten. A failed close of the session the stop used fails the step too: after a force-stop that succeeded, with the close's own code, the page forgotten all the same; after a failed one, as that failure's related cleanup failure |
| `startup_package` | A key whose remaining time does not cover the startup run and the run after it, before any stop (`contained_task_deadline_unavailable`); no startup package (`startup_package_missing`); the startup run's admission refusal (`startup_package_admission_failed`, `startup_package_adb_not_ready`) or its failed terminal; a key the startup run lost. A home that is not reached is the step's timeout. The stopped member's application stays stopped and the member's own is not known to run, and the key stays with the run until the run's end releases it |

What each failing step leaves on the device: at `foreground_read`, nothing stopped and nothing
started; at `stop`, the members stopped before the failed one stay stopped, the one whose stop
failed is as its device left it, and the member's own application is not started; at
`startup_package`, every member the switch meant to stop is stopped, and the member's own
application was not brought to its home (its startup run may have restarted it).

**What a person sees of a failed switch on the policy path** (MAPP-2b; no view changes). The run's
failure, `application_switch_failed` with its step and its cause, as the run's failure record
under the run's links, and the pair's failed execution record (`policy.execution_recorded`, its
failure the same code): both in the ledger and in its Errors view. The member's
`dispatch_instance_cooldown` with `failure_code` `application_switch_failed` and
`next_eligible_unix_ms` in the schedule explanation (`ac_schedule`, `actingctl schedule`) for as
long as the cooldown runs. **No row in the run list**: the run list is built from task records,
and a policy run whose switch failed has none (it fails before its request lifecycle writes
`task.requested`); the startup run's own `task.*` records, when step 3 ran, are the startup run's.

**A stop or a shutdown in mid-switch** (MAPP-2b; read in the code, the stop request pinned by the
MAPP-2b gate). Mid-switch is after the other member's stop and before this member's startup run
ends.

- **A stop request** (`StopInstanceRun` naming the run, `ac_stop_run`): the run's control is in
  the run registry from its preparation (a policy run) or its receipt (a direct run), so the stop
  reaches it and answers at once that the run is still stopping. Nothing of the switch reads it:
  the stops already made stay made and the startup run (a run of its own, which no stop may end)
  goes on to its end. The run then stops at its first check, as a run stopped during its
  preparation does: it ends `contained_task_cancelled`, the Runtime resets the device on the
  run's key before the key's release (`reset_stopped_run`), and a policy run settles as one
  `stopped` execution record, its runtime its own time as for any settlement, no failure of the
  pair, no owed rerun, an interruption in the routine summary. The key is released at the run's
  end as a stopped run's, and no ladder starts. Left on the device: the other member stopped,
  this member's application at its home (unless its startup run failed, which fails the switch
  as above), then the stop's reset. No switch cooldown starts: the switch did not fail.
- **A shutdown request** (`RequestShutdown`, `actingctl request-shutdown`): the device's key is
  held, so it is refused at once, `runtime_shutdown_denied` (`runtime_busy`), its
  `shutdown_request` record naming the decision `busy`, as during any run. The switch and the run
  go on as if it had not come: key, settlement, device and cooldown are those of the switch
  without it.
- **The Runtime's own stop** (its process ending, `RuntimeHost::close`), which is not refused: of
  the switch's steps only the startup run's ADB readiness probe reads it, and gives up once it is
  asked. Asked before that probe, it makes the startup run refused (`startup_package_adb_not_ready`),
  so the switch fails at `startup_package` and ends as any failed switch: the run settled failed,
  the key released, no ladder, the other member stopped and this member not started, and the
  cooldown it starts ends with the process. Asked after the probe, the startup run and the run go
  on to their end.

What a failed switch does to each party:

- **To the task:** one failed settlement of its (task, member) pair, with its streak and retry as
  for any failure.
- **To the member's ladder:** nothing. The run has no `task.failed` terminal at the task stage, and
  startup runs never start a ladder ([emulator-control.md](emulator-control.md), "Triggers"). So a
  member whose application does not start backs off through its pairs' failures.
- **To the other members:** their applications are as the step left them; a stopped member is
  started again by its own next switch. A fence holds the whole group ("A failure stays where it
  belongs").
- **To the member's other due work:** the member's switch cooldown (below).

**The member's switch cooldown** (MAPP-2b, built in Runtime 0.12.60). After a failed switch the
member does not compete for the device for `scheduling.device_group.switch_failure_cooldown_ms`
(milliseconds `1..=3600000`, default `300000`, five minutes). Without it, a member with many due
tasks would try each of them in turn, each try a kill and a restart of an application, and starve
the device's other members. It is the scheduling contract's takeover cooldown, kept per member:
position 2 of [scheduling-blackout.md](scheduling-blackout.md), "Order of the deferrals".

- **Start and end.** It starts when a run's switch fails with `application_switch_failed` (a
  policy run's or a direct run's; `HostShared::switch_before_run`), just before the run's failed
  settlement or failed receipt on the same thread, and ends at that instant plus the value in
  effect then. A change of the key applies to cooldowns that start after it. A start outside a run
  (an `ApplicationLifecycle` request, the ladder's rung) whose stop fails starts none.
- **Where it lives.** In the host's memory, one deadline per member on the Runtime's monotonic
  clock (`HostShared::switch_cooldowns`, `host.rs`), never stored: a restart drops it as it drops
  leases and queues ("Derived, never stored"). An ended cooldown is forgotten at the next policy
  cycle. The device's own cooldown (the takeover cooldown) stays one per device.
- **What it holds.** Every policy candidate of the member, an owed rerun, an immediate rerun, a
  remedy request or retry and a chain's successor included. Each is deferred at position 2, after
  the member's own control gates and before any wait for another member of its device, with
  `dispatch_instance_cooldown`: `instance_id` the member, `next_eligible_unix_ms` the cooldown's
  end, and the existing key `failure_code` `application_switch_failed`, an optional value of that
  code (registered by MAPP-2b) that tells it from a takeover cooldown. Its wake is the cooldown's
  end, and like any hold of positions 1 and 2 it raises no fact refresh demand
  ([scheduling/README.md](scheduling/README.md), "Fact Refresh"). The host passes the members in
  their cooldown to each evaluation (`policy_switch_cooldowns`, `PolicyEvaluationContext::
  switch_cooldowns`); the eligibility callback answers each asked candidate with the deferral
  (`fact_refresh::member_hold`), and the member is held whole in the evaluator's instance holds
  (`InstanceHolds`), so a candidate the round settles before asking (behind another member's
  winner of the device, the shared run limits or a chain handover) waits with the same reason.
  The evaluator's choice among members is not changed.
- **What it does not hold.** The device's other members compete for the device as before, so the
  device serves them meanwhile. A person's request for the member (a direct run, an
  `ApplicationLifecycle` request) is not held. A direct run goes through the switch, and its failed
  switch starts the cooldown again.
- **No ladder.** The failure still starts no recovery ladder (above), and the cooldown is no rung.
- **The key.** A member of the `scheduling` section (`device_group.switch_failure_cooldown_ms`,
  `actingd-config` revision 30, Runtime 0.12.60), settable, default from the defaults file, read
  when a switch fails. It names no application and has no per-instance or per-group form: a group
  has no configuration of its own beyond its members' `device_group` ("The configuration form"),
  and the section's nearest failure wait, `scheduling.fact_refresh.failure_backoff_ms`, has none
  either. A group of one never makes a switch, so it never meets the key.

## Identity on the device (MAPP, not built yet)

On the device, a member is its `application_id` (the package name), with its `nemu_app_index` as
today ([device-binding.md](device-binding.md), "Backends").

- **Start, kill and foreground are by package name.** The kill and the start go through ADB by
  package name (`force_stop`, `launch_package`, `crates/device/src/adb.rs`). The foreground answer
  keeps the package and drops the Android user (the `u<user>` behind `topResumedActivity`,
  [application-lifecycle.md](application-lifecycle.md)).
- **So a same-package clone cannot be a member in slice 1.** A clone with the same package name
  under another Android user is refused (two members with one `application_id`:
  `application_shared`). What would have to change for it is a later slice ("Later slices").
- **The emulator manager's mutable application commands stay unused**
  ([emulator-control.md](emulator-control.md)).

## Device-level actions (MAPP-1: the refusals at assembly; MAPP-1b: rows 5, 7, 10 and 15; MAPP-1c and MAPP-1d: rows 7 and 15; the switch's parts: MAPP-2a; their calls, row 4's included: MAPP-2b)

These act on the device, not on one application. The starting point: each such setting is read
from one place per group; a member that carries a conflicting one is refused at assembly; anything
not made group-aware yet is refused in a group of several rather than run per member. The table
lists each action, where it is today, and what slice 1 does in a group of several.

| # | Action | Where today | Slice 1 in a group of several |
|---|---|---|---|
| 1 | Configured start at daemon start (`start_emulator`) | `InstanceConfig::start_emulator`, config.rs:1118-1122; `resolve_emulator_autostart`, `enqueue_emulator_autostart`, `crates/runtime-host/src/host/emulator_autostart.rs:264`, `:350`; [emulator-control.md](emulator-control.md), "Configured start at daemon start" | **Refused**: `start_emulator` `true` (`device_action_unsupported`), and the instruction by `section_check` |
| 2 | Emulator watch: status read, restart, binding recognition | `crates/runtime-host/src/host/emulator_watch.rs:1-40`; `request_watch_autostart`, `request_watch_recognition`, emulator_autostart.rs:536, :575; "Emulator watch" | The status read stays per member (read-only, under the device's guard). A restart never happens, because it needs `start_emulator` (row 1). A recognition stays per member, on the device's key |
| 3 | Idle action (`close_app`, `stop_emulator`, `auto`) and its wake | `crates/runtime-host/src/host/idle.rs:1-125` (`idle_close_app` :1451, `idle_stop_emulator` :1623); [runtime-config.md](runtime-config.md), "The `idle` section" | **Refused**: `idle.action` in effect other than `none`. A `close_app` wake runs the member's startup package ahead of the device's pick, and a stop stops every member's device |
| 4 | Ladder rung `emulator_restart` | [emulator-control.md](emulator-control.md), "Rungs" and "Ladder profiles"; `crates/runtime-host/src/host/recovery_ladder.rs` | **Refused**: a profile in effect whose rungs hold it. The rungs `return_home` and `application_restart` stay per member on the device's key; before the latter stops and starts the member's own application, it stops the member in front, or every other member with the front unknown (MAPP-2b, "The switch before a run") |
| 5 | Operator emulator control (`actingctl emulator`, the console, MCP `ac_emulator`: start, stop, restart) | [emulator-control.md](emulator-control.md), "Operation and origin gate", "The operator's claim and the fence" (`ClaimKind::EmulatorControl`) | **Refused** before any device step with `device_group_action_refused` (registered by MAPP-1b; error, ladder class `none`, owner `runtime-host`; values `alias`, `operation` `control_emulator_instance`), before its intent. It would rebind and refresh only the asking member (`rebind_discovered_endpoint`, [device-binding.md](device-binding.md), "Backends") and leave the other members' endpoints and device facts as they were |
| 6 | Device binding: the handset's private file, and the discovery binding | `apps/actingd/src/config/device_binding.rs`; provider_startup.rs:105-141; `bound_alias`, `crates/runtime-host/src/host/instance_discovery.rs:171-194` | **Handsets refused** (`handset_member`). Discovery-bound members resolve the same index. The discovery answer and the startup record name one `bound_alias` per index (instance_discovery.rs takes the lowest instance id; provider_startup.rs keeps the last deferred entry). Slice 1 names the group's first member in alias order in both; the console slice decides how the others are shown |
| 7 | Capture and input sessions (retained session, Nemu pairing, the touch tool on the device) | `keeps_device_session`, the multi-Nemu gate, `crates/runtime-host/src/host/resource_close.rs:105-139` | A member of a group of several keeps no session across lease ends (`keeps_device_session` false): each member opens its own on its turn, so that two members' sessions are not open on one device at once. A member's lease end closes the member's session under that lease, before its release or its transfer (`end_lease_device_use`), when that close is admitted; a keyless observation closes its own before it lets the device go (MAPP-1c, "The session close of a keyless observation"), and so does the monitor's probe (MAPP-1d, "The monitor's probe in a device group"). A run's lease that lapses in its end phase (MAPP-1d): its close under the lapsed lease is refused and fences the member, as before (#411 A64, `fence_lapsed_lease_close`); that end then hands the device to no one (no transfer, whatever claim waits), and before the device's admission guard is let go the member's session is closed under a session close ahead ("The session close of a keyless observation"), which no waiting claim refuses (`HostShared::cleanup_token_guarded`, `close_lapsed_run_session_guarded`, `crates/runtime-host/src/host/lease.rs`, `resource_close.rs`); the claim that waited first is then granted in its place by the queue's own grant (its own member, holder and lease time). A close that cannot be confirmed fences the member (it is fenced already) and keeps its close lease, as every dedicated close lease is kept then, but only for that lease's time to live: the lease sweep then ends it (the fence keeps the member out of use) and hands the device to the next waiting member, whose session may open while the unconfirmed one is not confirmed closed ("Release."); nothing ends the Runtime. Whether two members' sessions could coexist on one emulator (two Nemu connections, two touch servers) is not verified, and slice 1 never tries |
| 8 | Backend switch (#402 HS-2) | `crates/runtime-host/src/host/backend_switch.rs`; [runtime-config.md](runtime-config.md), "Switching an instance's backends" | Per member, on the device's key: it closes the member's session and opens the new backends while no other member runs |
| 9 | Self-check, resume reconnect, connection preparation | [emulator-control.md](emulator-control.md), "Triggers", preparation stages | Per member, on the device's key, with the member's own session, closed at the lease end (row 7) |
| 10 | Resource fence (#403) and its recheck | [emulator-control.md](emulator-control.md), "Resource fence"; `crates/runtime-host/src/host/resource_fence.rs` | Per member as today (cause, ladder, recheck, lift). Every other member waits (`device_group_waiting` `fenced`) until the fence is lifted |
| 11 | Facts a device event invalidates (`device.connected`, `application.foreground`, `task.page` with `adb_unreachable`; `device_closed`) | [application-lifecycle.md](application-lifecycle.md), "The foreground gate"; [emulator-control.md](emulator-control.md), "The `device.connected` program fact" | Per member as today: the member that met the failure. Another member's `device.connected` is checked again by its own next gate or probe before any input. Fanning the invalidation out to the group is a later slice |
| 12 | Performance pacing per instance (level, factor, share, `R_eff`) | [performance-control.md](performance-control.md), "Directive", "Tracked value" | Per member as today. It acts on no device: it slows a member's own steps, and a group runs one member at a time |
| 13 | Shared run limits (`max_concurrent_runs`, heavy-scene slots) | [scheduling/README.md](scheduling/README.md), "Shared Run Limits" | Unchanged; a group adds at most one run |
| 14 | Handset device state (sampling, holds, start gate) | `crates/runtime-host/src/host/device_state.rs:1-30`, `:1017` | Not reached: handsets are refused (row 6) |
| 15 | Keyless read-only observations (`observe`, `capture_sequence`) | [emulator-control.md](emulator-control.md), "The operator's claim and the fence" | Under the device's admission guard. **Refused** with `device_group_action_refused` (`operation` `observe_readonly`, with `scheduler.denied` and the failure's record) while another member holds the device, because it would open a second member's session on the device (row 7). Served on a free device it keeps no session: for its duration (each frame's capture, under the device's guard) no other member's run starts, and its session is closed before the guard frees the device (MAPP-1b, the coordinator's ruling of 11-12, register §1.330), under a session close ahead that neither a claim waiting in the device's queue nor the device's takeover cooldown refuses (MAPP-1c, register §1.350, "The session close of a keyless observation"). The monitor's probe, a keyless capture of its own (`run_monitor_probe`, `crates/runtime-host/src/host/monitor_control.rs`), is made group-aware by MAPP-1d ("The monitor's probe in a device group"): under the device's guard it is judged as a keyless observation is; while another member holds the device the round is not made, and on a free device its session is closed before it lets the device go |

**The limits of slice 1 on a device with several members.** The Runtime does not start, stop or
restart that device's emulator (rows 1, 2 and 5); it does not close the application or the
emulator when the device is idle (row 3); and it does not restart the emulator after a major
failure (row 4). A person restarts it by hand in the emulator manager. These are limits of slice
1, not of the design: the later slice "device-level actions by device" ("Later slices") lifts
them, and it comes before the first real use of a group of several.

## The session close of a keyless observation (MAPP-1c, built in Runtime 0.12.60)

A keyless observation of a member on a free device (row 15) captures under the device's
admission guard, and every grant of the device's key either waits for that guard or tries it and
leaves it (the queue's own grant, a claim's or a client lease's immediate grant, a policy
admission's bounded try): so while the guard is held no lease of any member begins. Before the
observation lets the guard go, the member's session is closed (`close_keyless_session_of_group`,
`crates/runtime-host/src/host/resource_close.rs`) under a **session close ahead**
(`SeedScheduler::prepare_session_close_ahead`, `crates/scheduler/src/lib.rs`):

- a resource-close-only lease of the member (kind `resource_close`, the scheduler's lease TTL,
  the Runtime's close connection), prepared only on a free device: a holder of any member refuses
  it, so it never interrupts, shortens or ends a run's lease;
- granted whatever waits in the device's queue and while the device's takeover cooldown runs. It
  takes no entry out of the queue, ends no cooldown and carries no write authority
  (`validate_write` refuses it, `resource_close_only`); the cooldown's refusal is lifted for its
  own token's close only (`begin_resource_close`), and while the cooldown runs it hands the device
  to no one (no transfer at its end);
- released, once the close is confirmed, with `LeaseReleaseReason::SessionClosed`, which names no
  transfer. The guard's drop then grants the claim that waited first, in its place in the queue,
  by the queue's own grant (`scheduler.admitted`, `lease.granted` under that claim's request, its
  own member, holder and lease time), as it would have been granted had the observation not run;
  in a takeover cooldown the claim waits for the cooldown's end, as before;
- a close that fails fails the observation (fail loud), its records written by the close path; an
  unconfirmed close fences the member and keeps the lease, as every dedicated close lease is kept
  then. A member whose own lease holds the device (a run, a ladder's settle probe) leaves its
  session to that lease's end, which closes it under that lease when the close is admitted (row
  7; a run's lease that lapses in its end phase while another member's claim waits, which MAPP-1c
  left open there, is closed by MAPP-1d under the same session close ahead).

**Which way, and why.** The work order allowed two: close the observing member's session before
another member's lease begins, or never leave one open. MAPP-1c closes it before, under the guard
the observation already holds: the guard is what keeps every other grant out, so "the close,
then the waiting claim's grant" holds without touching the queue's order. Never opening one would
mean a capture under a lease of its own: a keyless observation would become a keyed one, queue
behind claims, be refused in a takeover cooldown and write a lease's records, which no released
reader of an observation expects. MAPP-1b already closed under a dedicated close lease; only the
two refusals below were in its way.

**The two cases MAPP-1b left open** (register §1.330, point 11), now closed:

- *A claim queued during the capture.* A claim of any member asked while the capture ran meets the
  guard and waits in the queue. The dedicated close lease was refused (`lease_transfer_not_safe`),
  the refusal recorded once and the session left to the member's next lease end, while the waiting
  claim, another member's included, was granted as the guard dropped: two members' sessions were
  open on the device for that claim's lease. The close ahead is not refused by the queue, so the
  observing member's session is closed before that claim's lease begins.
- *The device's takeover cooldown.* The cooldown refused the dedicated close lease
  (`lease_cooldown`): the observation failed after its capture and its session stayed open. The
  close ahead is granted through the cooldown.

**Why the cooldown allows the close.** A takeover makes every token of the earlier owner epoch
invalid, while a client of that epoch may still hold on to its lease until its next heartbeat; so
the start gives each device the earlier epoch held a cooldown longer than the longest client
heartbeat interval (`takeover_cooldown_ms`; a start with a shorter one is refused), during which no
lease is granted and no write admitted, so that no new holder writes to the device before such a
client has learnt its lease is gone. The close ahead does none of that: it retires only the session
this epoch's keyless observation opened in the same cooldown (an observation takes no key, and the
cooldown never held it back), sends no input and no application command, admits no write and
grants the device to no holder, whose first grant still waits for the cooldown's end. So the
cooldown keeps its purpose, and the close does not defeat it.

**What stays as it was.** An instance alone on its device keeps its session after a keyless
observation, as before, and meets none of this (`close_keyless_session_of_group` returns before
it). The capture-failure close of an observation and every other close path are unchanged. MAPP-1d
uses the same session close ahead for the monitor's probe ("The monitor's probe in a device
group") and for the session a lapsed run lease of a member left open (row 7).

**What was open, and is closed by MAPP-1d.** The session close of a keyless observation alone did
not make "two members' sessions are not open on one device at once" hold everywhere: a run's lease
that lapsed in its end phase while another member's claim waited, and the monitor's probe, left a
member's session open beside another member's lease, and the queue's grant judged the device's
queue by one member's gate (register §1.350, findings 2, 4 and 5). MAPP-1d (register §1.359)
closes the three (row 7, row 15, "One queue, one worker, one lease per device").

**The one lock order** of the paths that touch more than one member (register §1.350, "The lock
order", with file and line for each): an outer gate of the policy (`detection_write_gate`, or
`governance_write_gate` then `policy_outcome_gate` then `policy`) is taken before the device's
admission guard, which is only tried under them; then the device's admission guard, then the
device's queue-order lock, then the scheduler's lock, then the owner's. Every other lock of these
paths is taken and released on its own, never held while one of these is asked. The queue-order
lock is keyed by the device since MAPP-1c, as the queue it orders and the admission guard are; it
was keyed by the member, so a member's pump could grant another member's claim before that claim's
context was registered. MAPP-1d's paths (the monitor's probe, the lapsed run lease's end and the
close of its session, the gates of each member read before the queue-order lock) keep this order
(register §1.359, "The lock order").

## The monitor's probe in a device group (MAPP-1d, built in Runtime 0.12.60)

The monitor's probe (`run_monitor_probe`, `crates/runtime-host/src/host/monitor_control.rs`) is
a keyless capture of one instance on the resident monitor's thread, under the device's admission
guard, held to the probe's end (a plain guard, whose drop pumps nothing, as before). For a member
of a device group of several:

- **The judgement.** Under the guard, before anything is issued or recorded, the probe is judged
  as a keyless observation of the member is (`device_held_by_another_member`,
  `host/device_group.rs`, which `refuse_keyless_observation_of_group` uses too): another member
  holds the device when a lease of the device names another member (lapsed or not), or the
  device's occupancy names another member (its registered run, or its claim that waits first,
  each judged by its own member's gate).
- **The device held by another member: this round is not made.** Nothing is captured, no
  session is opened, no request, start, failure or recovery coordination is recorded, no lease is
  taken, and the member's monitor state is left as it is: its run count, its last verdict and its
  last error are not touched, so the round counts toward no failure and replaces no verdict. The
  probe stays due, and it is not passed over as a probe a blackout covers is
  ([scheduling-blackout.md](scheduling-blackout.md), "Complete silence": that one is set aside
  before the tick's bound and takes nothing): at every tick of the resident loop (every 25 ms,
  `MONITOR_POLL_INTERVAL`) the waiting probe takes one of the tick's 16 places
  (`MAX_MONITOR_PROBES_PER_TICK`) and the device's admission guard (a blocking lock) for its
  judgement, until the device is free. The monitor's status shows it as a probe that is due and
  has not run (`next_due_unix_ms` passed, the run count unchanged); the row has no member that
  says why a probe waits (`RuntimeMonitorInstanceStatus`, `runtime-client`), and none is added
  (what a person sees: "Later slices", "The console and the command line").
- **The device free: the probe runs and closes its session.** The probe runs as before, and
  before it lets the guard go the member's session is closed under a session close ahead
  (`close_keyless_session_of_group`, "The session close of a keyless observation"), which neither
  a claim that waited meanwhile nor the takeover cooldown refuses; a claim that waited is granted
  afterwards, in its place, by the queue's own grant (at the next pump: the sweep's tick, or
  another guard's drop). A device the member's own lease holds leaves the session to that lease's
  end, which closes it. A failed close is recorded by the close path, an unconfirmed one fences
  the member there; the probe keeps its own records, and only a failure that poisons the Runtime
  is returned.
- **Neither former fatal path is reached.** A capture that fails closes under the member's own
  lease or a dedicated close lease on a free device (a lease of another member, which refused
  that close busy and ended the Runtime, cannot hold the device under the guard once the round is
  made; a dedicated close lease that a waiting claim refuses, judged by the probing member's own
  gate as for an instance alone, so also a claim of another member that member's own gate holds,
  is recorded once, #411 A52, and the session close ahead closes the session); the recovery
  coordination (`monitor_recovery_admission`) runs under the same guard and reads no lease of
  another member (whose token made it fatal). The capture failure of a keyless
  capture during a takeover cooldown is not changed here (its own defect, board #456).
- **An instance alone on its device** meets none of this: the judgement and the close return
  before any lock, and its probe is exactly as before.

## A failure stays where it belongs (MAPP-1b: the group's waits, `held` and `fenced`)

- **An application's failure** (its run, its startup, its ladder up to the application restart)
  touches that member only: the run's failed settlement, the member's pairs, and the member's
  ladder on the device's key. The ladder holds the device for its climb, so the other members wait
  (`held`), as behind any claim.
- **A device's failure** (capture, input, ADB, the emulator) is met by the member whose work met it,
  and handled as today for that member: its run fails, its ladder runs on the device's key, a fence
  fences it.
- **A fence of one member holds every member of the group** (`device_group_waiting` `fenced`). The
  resources a fence is about (capture, input, ADB children, a session) are the device's, and slice
  1 does not tell a device's cause from an application's. The fence's ladder, recheck and lift stay
  the fenced member's, and the lift frees the group.
- **Other devices go on.** One device's fault never stops the Runtime or another device's group
  (#419). A failure of the whole Runtime stays the Runtime's, as today.

## Pause and hold (MAPP-1b: the pause's wait per member and the hold's gate of the group; the hold's acquire, its second-hold refusal and its release, not built yet: the player hold itself is Workflow #429 R-7b's, and the switch MAPP-2's)

- **A scheduling pause** stays per instance (alias) and global, as today
  ([scheduling-pause.md](scheduling-pause.md)). Pausing one member leaves the other members
  scheduled, and its drain waits only for the paused member's own work ("One queue, one worker,
  one lease per device").
- **A player hold** holds the device, not dispatch ([player-hold.md](player-hold.md): "a hold holds
  the device"). A hold on one member holds the device for the whole group: while a person uses
  one member by hand, the whole device waits.
  - The acquire drains the device's holder, whichever member's work it is, as it drains the
    member's own work today (finished or cancelled within `drain_timeout_ms`). Not built yet: it
    comes with the slice that builds player holds (Workflow #429 R-7b).
  - While held, no member is dispatched (the other members' candidates carry `device_group_waiting`
    `player_hold`, `alias` the holding member; MAPP-1b), and no claim of any member is granted
    except those a hold admits today (with R-7b).
  - A second hold on another member of the same group is to be refused, as a second hold of one
    instance is (`player_hold_already_held`), naming the hold. Not built yet: no build refuses it
    until the slice that builds player holds (R-7b).
  - The release runs the holding member's startup package as today, through the switch, since a
    person may have left any application in front. Not built yet (R-7b; the switch's stop part,
    `stop_others_before_start`, is built by MAPP-2b for it to call).
  - The hold's record and state stay the holding member's (`runtime-client` and `ledger` are
    unchanged). What it takes in code: the hold's drain and gate read the device's holder (the
    scheduler's per-device answer), and every member's control gates read the group's holds.

## Interfaces (MAPP, not built yet)

- **`actingd-config` 29** ([component-interfaces.md](component-interfaces.md), row 29 and
  "Planned: Workflow #437 MAPP"): the entry member `device_group` and check-config's report member
  `device_group` (MAPP-1, in force from Runtime 0.12.56). A build before it refuses a file that
  names the member (`config_decode_failed`: the entry denies unknown members). A file that names
  it nowhere keeps its revision, so the Runtime reads 5 to 29 and declares [5, 29]. The console
  and the installer, the other two parties, follow.
- **`actingd-config` 30** ([component-interfaces.md](component-interfaces.md), row 30; MAPP-2b,
  Runtime 0.12.60): the `scheduling` section's member `device_group.switch_failure_cooldown_ms`
  (0.12.56 shipped revision 29 without it, and MAPP-2a moved no revision). A build before it
  refuses a file that names the member (`config_decode_failed`: the section denies unknown
  members); a file that does not name it keeps its revision, so the Runtime reads 5 to 30 and
  declares [5, 30]. The console must reach revision 30 before it edits the key, and before a
  release pairs with this Runtime (the UI repository's work).
- **`ledger` does not move.** Every record that a group of several writes and a group of one never
  writes has an existing shape:
  - reasons with new codes and value words in decisions and their records
    (`decision_candidate_not_selected`, the not-run history's rows), which the reader only spells
    (the `ledger` rows 5 and 6 of [component-interfaces.md](component-interfaces.md); a received
    outcome is relayed unchanged and only an emission is checked against the catalog,
    `crates/actingcommand-contract/src/outcome/check.rs:3-6`);
  - the switch's host-minted `ApplicationLifecycle` records, the `runtime.lifecycle_observed` phase
    `startup_package_scheduled` and the startup run's `task.*` chain, all existing types, members
    and phases;
  - the failed settlement with `application_switch_failed`, an existing result kind with a new
    code;
  - the switch cooldown's reason `dispatch_instance_cooldown`, an existing code with one more
    optional value (`failure_code`, an existing key).

  `EventLinks` keep `instance_id` alone, the member's, and deny unknown members
  (`crates/actingcommand-contract/src/event/envelope.rs:46-49`), so no group member is added to
  them.
  An older reader (a Runtime that declares `ledger` [3, 8], the console, the Tools) reads each of
  these as it reads the same record with another code.
- **`runtime-client` does not move.** `Status` keeps its shape per instance: a waiting member's
  `not_running` row has `why` `device_group_waiting` with its values, and every request takes an
  alias, the member's. Showing the group in `Status` (a member `device_group?` per row) is proposed
  for the console slice, which then raises `runtime-client`.
- **`runtime-outcomes`:** new codes and words in the catalog only, and the optional value
  `failure_code` of the existing `dispatch_instance_cooldown`.
- **Untouched:** `package`, `install-selection`, `install-control`, `install-cli`, `tools-layout`
  and `player-hold`.

## Later slices (named, not written here)

- **Order within one device.** Its rules are written by ORD-1b ("Ranking across members" above;
  [scheduling/README.md](scheduling/README.md), "Order within one device"); what is still to come
  is the host's slice of the order, which passes the order's inputs. The time a switch takes counts
  in no order and not in a run's own duration (board #437, 2026-11-13).
- **The normalized weight.** Superseded by the order within one device (ORD-1b, "Ranking across
  members" above): no weight is compared across the members of one device.
- **Device-level actions by device.** It lifts the limits of slice 1 ("Device-level actions") and
  comes before the first real use of a group of several. Rows 1, 3, 4, 5, 11 and 15 of the table,
  per device:
  - one configured start and one watch restart per group;
  - the idle action per device: close the application in front, and stop the emulator only when no
    member has work soon;
  - the emulator restart rung and the operator's control, with every member rebound and its facts
    refreshed;
  - the invalidation fanned out to every member;
  - observation of a device held by another member.
- **The console and the command line.** How a device with its applications is shown and named, and
  `Status` with the group (`runtime-client`). While a member's monitor probe waits for its device
  ("The monitor's probe in a device group"), a person sees today only that the row's due time has
  passed and its run count is unchanged, with no reason, so the row has to say that the probe waits
  for the device.
- **Clones.** A member's identity becomes its package name with its Android user (or its clone
  index). The start, the force-stop and the foreground query then carry the user (keeping the
  `u<user>` the foreground parse drops today, `crates/device/src/adb.rs`). This waits for a test on
  a device of how a clone is addressed: whether a clone is another package or another user is not
  verified.

**Release.**

- MAPP-1a shipped in Runtime 0.12.56 with every group of several refused at load (corrected by the
  coordinator on 11-12, register §1.330).
- MAPP-1c is built in Runtime 0.12.60 (register §1.350): a member's keyless session closed under a
  session close ahead that neither a waiting claim nor the takeover cooldown refuses, and the
  device's queue-order lock keyed by the device; the load still refuses every group of several, so
  no installation reaches it, and nothing is added to the catalog, the configuration or an
  interface.
- MAPP-1d is built in Runtime 0.12.60 (register §1.359): the monitor's probe of a member made only
  on a free device and its session closed before it lets the device go, a lapsed run lease of a
  member handed on only after the member's session is closed, and each claim of a device's queue
  judged by its own member's gate; the load still refuses every group of several, so no
  installation reaches it, and nothing is added to the catalog, the configuration or an
  interface.
- MAPP-2a is built in Runtime 0.12.59 (register §1.342): the switch's parts, their one entry and
  the code `application_switch_failed` with the vocabulary `application_switch_step`, with no
  caller in the Runtime; the load still refuses every group of several, and the catalog gains one
  code and one vocabulary.
- MAPP-2b is built in Runtime 0.12.60 (register §1.353): the switch's callers, the run's clock that
  leaves the switch out, a member's lease sized for the startup run and the run, the failure's
  settlement and the member's switch cooldown with its key, in `actingd-config` revision 30; the
  load still refuses every group of several, so no installation reaches any of it.
- The release that first accepts a device group of more than one member carries the host's rows
  (MAPP-1b), the switch before a run (MAPP-2a and MAPP-2b) and the slice "order within one device"
  together, so that no released build runs a group of several without the switch. Until then
  every build refuses such a group at load (`device_group_config_invalid`,
  `several_members_unsupported`; "The configuration form"), and no release ships the switch after
  every task.
- **Before the load accepts a group of several, main must also end the member that cannot fit**
  (MAPP-2b's known gap, "The switch before a run", "The member that cannot fit, as built"): with
  today's request deadlines every run of a member of a group of several is refused by this bound;
  the lease across a switch is reworked in a later slice before any group of several is
  accepted, and a task that cannot fit never becomes a candidate (it is held where tasks are
  composed, in the slice that orders a device or in the one that lets the load accept a group),
  which replaces the MAPP-2b gate's test
  `mapp2b_known_gap_a_member_that_cannot_fit_is_selected_and_refused_every_round`.
- MAPP-2's key `scheduling.device_group.switch_failure_cooldown_ms` came with MAPP-2b, in
  `actingd-config` revision 30.
- The slice "device-level actions by device" comes before the first real use of a group of
  several.
- **Before the load accepts a group of several**, besides the member that cannot fit (above), the
  slice "order within one device" and the slice "device-level actions by device", these must be
  on main (found by MAPP-1d and its review, register §1.359; MAPP-1c's open findings 2, 4 and 5,
  register §1.350, are closed by MAPP-1d):
  - *A member's non-run lease that lapses while its session is open* (board #458): the sweep's
    close under the lapsed lease is refused, the sweep marks that error fatal and the whole Runtime
    stops, for every member of a group of several (and, as the review reads it, already today for
    instances alone when several of them share the emulator's native capture).
  - *An unconfirmed close of one member must keep the device from the other members until it is
    confirmed*: today the close lease holds the device only for its time to live and the device
    then passes on, so two members' sessions can be open together after an unconfirmed close (row
    7).
  - *A pump candidate blocked by its own member's performance directive or ladder wait* (`pump`)
    blocks the whole device's queue, so one member's performance pause stops the others.
  - *History (MAPP-1d, 0.12.60).* Before MAPP-1d this list named: a run's lease that lapses in
    its end phase while a claim of another member waits (the expiry transferred the device to that
    claim and left the member's session open under it, row 7); the monitor's probe, which was not
    refused while another member's lease held the device, did not close its session, and could end
    the whole Runtime by a close lease refused busy or by reading another member's token in its
    recovery coordination (row 15); and the queue's grant, which judged the device's whole queue by
    the gate of the member it was asked for, as the transfers and the queueing did. Each is closed
    as row 7, "The monitor's probe in a device group" and "One queue, one worker, one lease per
    device" now say; the dedicated close lease keeps the asking member's gate, as before (fix round
    1, register §1.359).

## Codes, value keys and words (MAPP-1 registers the first three codes and two vocabularies, MAPP-1b the fifth code, MAPP-2a the fourth code and the third vocabulary, MAPP-2b one optional value and the key)

| Code | Owner | Category, ladder class | Values | State |
|---|---|---|---|---|
| `device_group_config_invalid` | `actingd` | error, `none` | `aliases`, `config_key`, `device_group_problem` | registered, MAPP-1 |
| `device_group_assembled` | `actingd` | info, `none` | `aliases`, `device_label`? | registered, MAPP-1 |
| `device_group_waiting` | `policy` | info, `none` | `instance_id`, `alias`, `device_group_wait`, `task_id`?, `holder_kind`? | registered, MAPP-1 |
| `application_switch_failed` | `runtime-host` | error, `none` | `alias`, `application_switch_step`, `failure_code`, `application_id` | registered, MAPP-2a (0.12.59) |
| `device_group_action_refused` | `runtime-host` | error, `none` | `alias`, `operation` | registered, MAPP-1b |

New value keys, each with its vocabulary:

- `device_group_problem` (registered, MAPP-1): `name_invalid`, `device_mismatch`,
  `handset_member`, `fixture_member`, `application_shared`, `startup_package_missing`,
  `device_action_unsupported`, `undeclared_shared_device`, `several_members_unsupported`.
- `device_group_wait` (registered, MAPP-1): `selected`, `held`, `player_hold`, `fenced`.
- `application_switch_step` (registered, MAPP-2a): `foreground_read`, `stop`, `startup_package`.

Every other key is an existing key of the one key table
(`crates/actingcommand-contract/src/outcome/keys.rs`).

One existing code takes one more optional value (registered, MAPP-2b, 0.12.60):
`dispatch_instance_cooldown` (`runtime-host`, info) takes `failure_code`, an existing key, which is
`application_switch_failed` for a member's switch cooldown and is absent for a takeover cooldown.
Its description and causes name both cooldowns. No code is added.

One configuration key (registered, MAPP-2b, 0.12.60, `actingd-config` revision 30):
`scheduling.device_group.switch_failure_cooldown_ms`, milliseconds `1..=3600000`, default
`300000`, settable ("The switch before a run").

## Not verified

- What `am force-stop` does to a package that is not running, on an emulator or a phone.
- Whether two sessions of two members (two Nemu connections, two touch servers) can be open on one
  emulator at once; slice 1 never opens two.
- Whether an emulator's ADB port stays the same across its restart.
- How an application clone is addressed on an emulator or on a phone (another package, or the same
  package under another Android user), and which one `monkey` starts when both exist.
- Whether the readers of check-config's report (the installer, the Lab) deny unknown members.
