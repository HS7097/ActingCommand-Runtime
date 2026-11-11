# Device groups: several applications on one device (MAPP, not built yet)

Workflow #437 MAPP-C (contracts only; [v012-register.md](v012-register.md) §1.323). **Every rule of
this file is (MAPP, not built yet)**: no code, key, code, value key or word named here exists until
the code slices of §1.323 build it, and every code, value key and vocabulary word below is
proposed. The other contracts point here instead of repeating the rules.

Alice settled the direction on 2026-11-11 (board #437): one device may hold several applications
that the scheduler serves in turn; one application is in front at any time; one device has one
queue, and a task of another application that comes due waits in it even where waiting loses some
yield; a task package that is running is never interrupted; the queue's order is by weight and by
time; and 「切换到另一个应用时，原先的应用直接杀掉就行」, starting and killing by package name through
ADB. Everything else in this file is the coordinator's rulings of 11-11 and 11-12, written as the
code allows them, not her words.

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

## The configuration form (MAPP, not built yet)

An `instances[]` entry takes one optional member, `device_group` (`actingd-config` revision 29,
planned: [component-interfaces.md](component-interfaces.md), "Planned: Workflow #437 MAPP"):

```json
{"alias": "neutral.a", "instance_id": "instance_<32 hex>",
 "application_id": "com.example.fixture.a", "instance_index": 2,
 "touch_backend": "adb_shell_input", "capture_backend": "adb",
 "startup_package": {"package": "packages/neutral-a-startup.zip"},
 "idle": {"action": "none"}, "recovery_ladder_profile": "handset",
 "device_group": "device.one"}
```

- `device_group` is a name of 1 to 64 bytes under the alias rules (`validate_alias`,
  `crates/runtime-host/src/provider.rs`). It names no device value: no serial, no port, no index.
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
and at start alike, with `device_group_config_invalid`. That code is proposed: error, ladder class
`none`, owner `actingd`. Its values are `aliases` (the entries concerned), `config_key` (the member
that breaks the rule) and `device_group_problem` (a proposed vocabulary).

| Rule | `config_key` | `device_group_problem` |
|---|---|---|
| Every member names the same device by the same key with the same value: all the same `instance_index`, all the same `instance_name`, or all the same `host` and `port` (and the same `serial` when one is named). check-config never runs discovery ([actingd-check-config.md](actingd-check-config.md), "Discovery-bound instances"), so it cannot prove that an index and a name reach one emulator | the key that differs | `device_mismatch` |
| No member is a handset (`device.kind` `handset`). The private binding file binds one serial to one alias ([device-binding.md](device-binding.md), "The private binding file", `duplicate_serial`); a group-aware binding is a later slice | `device` | `handset_member` |
| No member is a fixture entry | `fixture_backend` | `fixture_member` |
| Every member has its own `application_id`. The kill and the foreground gate tell members apart by package name ("Identity on the device") | `application_id` | `application_shared` |
| Every member declares a `startup_package`. The switch reaches the member's home only through it ("The switch before a run") | `startup_package` | `startup_package_missing` |
| No member turns on a device-level action that slice 1 does not make group-aware ("Device-level actions"): `start_emulator` `true`; an `idle.action` in effect other than `none` (the defaults file's `close_app` included, so a member names `none`); a ladder profile in effect whose rungs hold `emulator_restart`. The profile in effect is the member's `recovery_ladder_profile`, else its kind's profile, else the section's `rungs` ([emulator-control.md](emulator-control.md), "Ladder profiles"). An emulator names no profile by default and climbs the section's `rungs`, which hold that rung, so a member names a profile such as the defaults file's `handset` (`return_home`, `application_restart`). Under the default `rules.start_beyond_profile` `exhaust`, a trigger that starts beyond it (`navigation_exhausted`, start rung 3) ends `exhausted`, as on a handset | `start_emulator`, `idle.action`, `recovery_ladder_profile` | `device_action_unsupported` |

While the Runtime runs, an instruction that would break the last rule on a member of a group of
several is refused by the whole-instance check: `config_value_invalid` with `config_value_problem`
`section_check` on the key ([runtime-config.md](runtime-config.md), "`ConfigureSettings`"). The
keys are `instance.<id>.start_emulator`, `instance.<id>.idle.action`, the section's `idle.action`
where a member takes it, `instance.<id>.recovery_ladder_profile`, and the `recovery_ladder`
profile a member climbs by.

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
  `instance_name`, `serial`, or `host` and `port`.
- **At start, after the discovery**, when entries of different groups resolve to the same
  discovered index or ADB port: an explicit `HOST:PORT` beside a discovery-bound entry, or an index
  and a name of one emulator. It is recorded as every discovery refusal is, as a Provider startup
  failure ([provider-startup.md](provider-startup.md)).

Handsets are already covered: the private binding file refuses one serial bound to two aliases.
This refusal is the one difference a configuration without `device_group` can see, and only a
configuration that drives one device from two workers today meets it.

**What check-config and the start print.**

- check-config: the `instances[]` report row of a member that names `device_group` carries
  `device_group`, the name. A row of an entry without it is as today.
- The start: one daemon line per group of several, `device_group_assembled` (proposed; info;
  values `aliases`, `device_label`). It goes to the daemon log only, and nothing new is recorded in
  the ledger: each member records its own `runtime.instance_bound`, with the same endpoint.
- A group of one prints nothing new.

## A group of one is today (MAPP, not built yet)

**The rule the code slices are tested against.** With no entry declaring a shared device, every
decision, every record, every key and every byte the Runtime persists is what it is today, and no
data is migrated. It holds because:

- The group is derived from the file at assembly ("Derived, never stored") and is the member
  alone. Its device key is the member's own instance id, so every map keyed by the device key has
  today's keys and values ("One queue, one worker, one lease per device").
- The new reason (`device_group_waiting`) and the switch exist only when another member has the
  device, which a group of one never has. The group rules of the form apply only to a group of
  several.
- No record type, payload member, phase word, fact key, tag or closed vocabulary of the ledger is
  added ("Interfaces").
- A file without `device_group` keeps its revision (28 or lower). check-config's report and the
  manifest gain nothing for it. The worker thread keeps its name, `actingd-instance-<alias>`.
- The owner journal's active instances are the same ids (`protected_instance_ids`,
  `crates/scheduler/src/lib.rs:2585-2593`, written by `persist_active_instances`,
  `crates/runtime-host/src/host/lease.rs:3242-3247`).

The one exception is the refusal of undeclared sharing above.

## Derived, never stored (MAPP, not built yet)

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

## One queue, one worker, one lease per device (MAPP, not built yet)

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

## One pick per device per round (MAPP, not built yet)

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

**Ranking across members, as it is today.** Slice 1 is correct, not optimized: members rank by
today's scores, unchanged.

- **Unjudged candidates.** The total is `p*1_000_000 + aging_ms + w*1000 + u*1000 - c (+ e*1000)`
  ([scheduling/README.md](scheduling/README.md), "Dispatch Decision Record", `rank_breakdown`). The
  declared priorities of two members' catalogs are not calibrated against each other, so the
  member whose packs declare higher priorities wins the device more often. A lower one wins once
  its aging (one point per millisecond of eligibility) makes up the gap, about 1 000 s per priority
  step, so no member waits forever.
- **Judged candidates.** In the business tier, candidates of different instances go by their
  instances' turns before `B ÷ E` ("Turns across instances": the instance whose most recent policy
  dispatch is the earliest goes first). So members of one device alternate in that tier, and slice
  1 may switch at every dispatch there.
- **Values.** The values of two members (`B`, the resource terms) are never compared, as today.

None of this is a defect of slice 1: it is today's ranking applied to one device. The normalized
weight and the switch's cost are later slices.

**The reason a member waits: `device_group_waiting`** (proposed; info, ladder class `none`, owner
`policy`). A candidate of a member that does not get the device because another member has it
carries this reason. It takes the place of the code a lone instance would give
(`instance_already_selected`, `dispatch_instance_held`, the control gates' codes) and has no wake
of its own: the other member's settlement, release or lifting wakes the next evaluation, as today.
Its values:

- `instance_id` (required): the candidate's alias, as the policy's reasons name it.
- `alias` (required): the member that has the device.
- `device_group_wait` (required; proposed vocabulary):
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

## Never interrupted (MAPP, not built yet)

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
  it does.
- An owed rerun, an immediate retry and a remedy's request. These are candidates of the member with
  their own rank: they keep the device when they win it in the round, as on a lone instance they
  win the instance.

Another member's due task queues behind: it waits with `device_group_waiting` until the device is
free and it wins a round.

## The switch before a run (MAPP, not built yet)

In a group of several, before a run of a member, the Runtime brings that member's application to
the front, under the device's lease.

**Where the switch runs.**

- For a policy run: inside `run_scheduled_contained_task`
  (`crates/runtime-host/src/host/contained_task.rs:7812`), after the run's context, identity,
  package and outcome checks (through 8079) and before the run's request lifecycle is appended
  (8080). So a run that its own checks refuse kills nothing. It runs on the device's worker, or on
  the driver's thread with `scheduling.parallel.enabled` off.
- For a run a person starts (a direct task run from the Lab, the console or MCP `ac_run_pack`): at
  the same place of the direct path, under its claim.
- Before every start of a member's application outside a switched run: a startup package after an
  emulator start, the release of a player hold, the ladder's application restart rung
  ([emulator-control.md](emulator-control.md), "Rungs"), and an `ApplicationLifecycle` `launch` or
  `restart` a person asks for (`ac_app`).
- Never inside a run. A package's own `application` effect restarts the member's application,
  which is in front already.

**Its steps, on the device's key.**

1. **Read.** One foreground query through the member's registration: the read-only query of the
   foreground gate ([application-lifecycle.md](application-lifecycle.md), "The foreground gate";
   `observe_foreground_application`, provider.rs:972-988). When the package in front is the
   member's own `application_id`, nothing more is done and the run starts. No record is written.
2. **Stop.** When the package in front is another member's `application_id`, that member's
   application is force-stopped by a host-minted `ApplicationLifecycle { stop }` request for that
   member. This is how the ladder's application restart rung stops its application:
   `command.received`, `command.validated`, `application.intent`, `application.completed`, under
   that member's links. When the package in front is no member's, or no resumed activity is
   reported, every other member's application is stopped the same way, one request each, because
   the Runtime cannot tell which one may still run. What `am force-stop` does to a package that is
   not running is not verified on the devices.
3. **Start and home.** The member's startup package runs on the key as a host run, exactly as after
   an emulator start ([application-lifecycle.md](application-lifecycle.md), "The startup package
   hook").
   - It writes `runtime.lifecycle_observed` phase `startup_package_scheduled` under the run's
     request links with a fresh causation id C, as the ladder's rung writes it under the ladder's
     links. The stops of step 2 carry the same causation id C.
   - The startup run then runs under C: self-minted request and correlation ids, origin
     `(Agent, Adapter)`, its full `task.*` chain.
   - Its first step is its `application` effect, `restart`: a force-stop and then a `monkey` launch
     of the member's `application_id` (provider.rs:902-929). So the application is started by its
     package name through the one existing path, and the run succeeds only on the home page its
     package declares.
   - The startup run's end hands the key back to the run, never to another claim, as a rung run's
     release hands it to the ladder's continuation ([emulator-control.md](emulator-control.md),
     "Rungs").
4. **Run.** The member's run starts on the same key.

After a switch the earlier application is not running, so at most one member's application is
alive on a device. A person who starts another application by hand is outside this rule ("Pause
and hold").

**The foreground gate** is unchanged: each member's pointer input is compared with that member's
own `application_id`. The switch sends no pointer input, so the gate does not run during it; any
inputs of the startup run meet the gate as every startup run's do.

**When the switch fails.** Each failure fails the member's run only, before any effect of the run,
with `application_switch_failed`. That code is proposed: error, ladder class `none`, owner
`runtime-host`, caused by the step's failure record. Its values are `alias` (the member to run),
`application_switch_step` (a proposed vocabulary: `foreground_read`, `stop`, `startup_package`),
`failure_code` (the step's own code) and `application_id` (the package the step concerned).

A policy run is settled as every run refused before its request lifecycle is: a failed settlement
of the pair, with its lease released (`run_scheduled_policy_task`,
`crates/runtime-host/src/host/policy_lanes.rs:547-584`, then `record_scheduled_policy_failure` and
`complete_scheduled_policy_failure`, `crates/runtime-host/src/host/policy_outcome.rs:1544-1567`). A
direct run's receipt is failed with the code.

| Step | What its failure is and does |
|---|---|
| `foreground_read` | The query's ADB failure is handled as the gate's: the member's `device.connected`, `application.foreground` and `task.page` are invalidated with `adb_unreachable`, and an unconfirmed ADB cleanup fences the member. `failure_code` is `application_foreground_unknown` or `instance_resources_fenced` |
| `stop` | The stop's own failure records, as the rung's (`command.rejected` + `runtime.failed`). A fatal device error with confirmed resources fences the member whose application was being stopped (#411 A46) |
| `startup_package` | The startup run's admission refusal (`startup_package_missing`, `startup_package_admission_failed`, `startup_package_adb_not_ready`) or its failed terminal. A home that is not reached is the step's timeout. The stopped member's application stays stopped |

What a failed switch does to each party:

- **To the task:** one failed settlement of its (task, member) pair, with its streak and retry as
  for any failure.
- **To the member's ladder:** nothing. The run has no `task.failed` terminal at the task stage, and
  startup runs never start a ladder ([emulator-control.md](emulator-control.md), "Triggers"). So a
  member whose application does not start backs off through its pairs' failures.
- **To the other members:** their applications are as the step left them; a stopped member is
  started again by its own next switch. A fence holds the whole group ("A failure stays where it
  belongs").

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

## Device-level actions (MAPP, not built yet)

These act on the device, not on one application. The starting point: each such setting is read
from one place per group; a member that carries a conflicting one is refused at assembly; anything
not made group-aware yet is refused in a group of several rather than run per member. The table
lists each action, where it is today, and what slice 1 does in a group of several.

| # | Action | Where today | Slice 1 in a group of several |
|---|---|---|---|
| 1 | Configured start at daemon start (`start_emulator`) | `InstanceConfig::start_emulator`, config.rs:1118-1122; `resolve_emulator_autostart`, `enqueue_emulator_autostart`, `crates/runtime-host/src/host/emulator_autostart.rs:264`, `:350`; [emulator-control.md](emulator-control.md), "Configured start at daemon start" | **Refused**: `start_emulator` `true` (`device_action_unsupported`), and the instruction by `section_check` |
| 2 | Emulator watch: status read, restart, binding recognition | `crates/runtime-host/src/host/emulator_watch.rs:1-40`; `request_watch_autostart`, `request_watch_recognition`, emulator_autostart.rs:536, :575; "Emulator watch" | The status read stays per member (read-only, under the device's guard). A restart never happens, because it needs `start_emulator` (row 1). A recognition stays per member, on the device's key |
| 3 | Idle action (`close_app`, `stop_emulator`, `auto`) and its wake | `crates/runtime-host/src/host/idle.rs:1-125` (`idle_close_app` :1401, `idle_stop_emulator` :1573); [runtime-config.md](runtime-config.md), "The `idle` section" | **Refused**: `idle.action` in effect other than `none`. A `close_app` wake runs the member's startup package ahead of the device's pick, and a stop stops every member's device |
| 4 | Ladder rung `emulator_restart` | [emulator-control.md](emulator-control.md), "Rungs" and "Ladder profiles"; `crates/runtime-host/src/host/recovery_ladder.rs` | **Refused**: a profile in effect whose rungs hold it. The rungs `return_home` and `application_restart` stay per member on the device's key; the latter's start goes through the switch |
| 5 | Operator emulator control (`actingctl emulator`, the console, MCP `ac_emulator`: start, stop, restart) | [emulator-control.md](emulator-control.md), "Operation and origin gate", "The operator's claim and the fence" (`ClaimKind::EmulatorControl`) | **Refused** before any device step with `device_group_action_refused` (proposed; error, ladder class `none`, owner `runtime-host`; values `alias`, `operation`). It would rebind and refresh only the asking member (`rebind_discovered_endpoint`, [device-binding.md](device-binding.md), "Backends") and leave the other members' endpoints and device facts as they were |
| 6 | Device binding: the handset's private file, and the discovery binding | `apps/actingd/src/config/device_binding.rs`; provider_startup.rs:105-141; `bound_alias`, `crates/runtime-host/src/host/instance_discovery.rs:171-194` | **Handsets refused** (`handset_member`). Discovery-bound members resolve the same index. The discovery answer and the startup record name one `bound_alias` per index (instance_discovery.rs takes the lowest instance id; provider_startup.rs keeps the last deferred entry). Slice 1 names the group's first member in alias order in both; the console slice decides how the others are shown |
| 7 | Capture and input sessions (retained session, Nemu pairing, the touch tool on the device) | `keeps_device_session`, the multi-Nemu gate, `crates/runtime-host/src/host/resource_close.rs:105-139` | A member of a group of several keeps no session across lease ends (`keeps_device_session` false): each member opens its own on its turn, and two members' sessions are never open on one device at once. Whether two members' sessions could coexist on one emulator (two Nemu connections, two touch servers) is not verified, and slice 1 never tries |
| 8 | Backend switch (#402 HS-2) | `crates/runtime-host/src/host/backend_switch.rs`; [runtime-config.md](runtime-config.md), "Switching an instance's backends" | Per member, on the device's key: it closes the member's session and opens the new backends while no other member runs |
| 9 | Self-check, resume reconnect, connection preparation | [emulator-control.md](emulator-control.md), "Triggers", preparation stages | Per member, on the device's key, with the member's own session, closed at the lease end (row 7) |
| 10 | Resource fence (#403) and its recheck | [emulator-control.md](emulator-control.md), "Resource fence"; `crates/runtime-host/src/host/resource_fence.rs` | Per member as today (cause, ladder, recheck, lift). Every other member waits (`device_group_waiting` `fenced`) until the fence is lifted |
| 11 | Facts a device event invalidates (`device.connected`, `application.foreground`, `task.page` with `adb_unreachable`; `device_closed`) | [application-lifecycle.md](application-lifecycle.md), "The foreground gate"; [emulator-control.md](emulator-control.md), "The `device.connected` program fact" | Per member as today: the member that met the failure. Another member's `device.connected` is checked again by its own next gate or probe before any input. Fanning the invalidation out to the group is a later slice |
| 12 | Performance pacing per instance (level, factor, share, `R_eff`) | [performance-control.md](performance-control.md), "Directive", "Tracked value" | Per member as today. It acts on no device: it slows a member's own steps, and a group runs one member at a time |
| 13 | Shared run limits (`max_concurrent_runs`, heavy-scene slots) | [scheduling/README.md](scheduling/README.md), "Shared Run Limits" | Unchanged; a group adds at most one run |
| 14 | Handset device state (sampling, holds, start gate) | `crates/runtime-host/src/host/device_state.rs:1-30`, `:1017` | Not reached: handsets are refused (row 6) |
| 15 | Keyless read-only observations (`observe`, `capture_sequence`) | [emulator-control.md](emulator-control.md), "The operator's claim and the fence" | Under the device's admission guard. **Refused** with `device_group_action_refused` while another member holds the device, because it would open a second member's session on the device (row 7) |

## A failure stays where it belongs (MAPP, not built yet)

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

## Pause and hold (MAPP, not built yet)

- **A scheduling pause** stays per instance (alias) and global, as today
  ([scheduling-pause.md](scheduling-pause.md)). Pausing one member leaves the other members
  scheduled, and its drain waits only for the paused member's own work ("One queue, one worker,
  one lease per device").
- **A player hold** holds the device, not dispatch ([player-hold.md](player-hold.md): "a hold holds
  the device"). Proposed: a hold on one member holds the device for the whole group.
  - The acquire drains the device's holder, whichever member's work it is, as it drains the
    member's own work today (finished or cancelled within `drain_timeout_ms`).
  - While held, no member is dispatched (the other members' candidates carry `device_group_waiting`
    `player_hold`, `alias` the holding member), and no claim of any member is granted except those
    a hold admits today.
  - A second hold on another member of the same group is refused, as a second hold of one instance
    is (`player_hold_already_held`), naming the hold.
  - The release runs the holding member's startup package as today, through the switch, since a
    person may have left any application in front.
  - The hold's record and state stay the holding member's (`runtime-client` and `ledger` are
    unchanged). What it takes in code: the hold's drain and gate read the device's holder (the
    scheduler's per-device answer), and every member's control gates read the group's holds.

## Interfaces (MAPP, not built yet)

- **`actingd-config` 29** (planned; [component-interfaces.md](component-interfaces.md), "Planned:
  Workflow #437 MAPP"): the entry member `device_group` and check-config's report member
  `device_group`. A build before it refuses a file that names the member (`config_decode_failed`).
  A file that does not name it keeps its revision, so the Runtime reads 5 to 29 and declares
  [5, 29]. The console and the installer, the other two parties, follow.
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
    code.

  `EventLinks` keep `instance_id` alone, the member's, and deny unknown members
  (`crates/actingcommand-contract/src/event/envelope.rs:46-49`), so no group member is added to
  them.
  An older reader (a Runtime that declares `ledger` [3, 8], the console, the Tools) reads each of
  these as it reads the same record with another code.
- **`runtime-client` does not move.** `Status` keeps its shape per instance: a waiting member's
  `not_running` row has `why` `device_group_waiting` with its values, and every request takes an
  alias, the member's. Showing the group in `Status` (a member `device_group?` per row) is proposed
  for the console slice, which then raises `runtime-client`.
- **`runtime-outcomes`:** new codes and words in the catalog only.
- **Untouched:** `package`, `install-selection`, `install-control`, `install-cli`, `tools-layout`
  and `player-hold`.

## Later slices (named, not written here)

- **The switch's cost.** The expected duration of a task whose run needs a switch includes the
  switch (the stops, the start and the home: the mean of that member's last five switches). Slice 1
  leaves room: each switch's records share one causation id, which gives its time. Note: `E` today
  orders only a member's own judged candidates (`B ÷ E` within an instance) and the reservations.
  Across instances the turns decide first, so "finish one application's due work before switching"
  also needs the turns of the members of one device to be read per device.
- **The normalized weight.** A task's weight divided by the largest weight of its own catalog, 0.5
  for a task without a price, with a per-application preference factor on top, default 1 (board
  #437, comment 6799803829). Slice 1 leaves room: members already rank in one ordered list, by
  today's scores.
- **Device-level actions made group-aware.** Rows 1, 3, 4, 5, 11 and 15 of the table, per device:
  - one configured start and one watch restart per group;
  - the idle action per device: close the application in front, and stop the emulator only when no
    member has work soon;
  - the emulator restart rung and the operator's control, with every member rebound and its facts
    refreshed;
  - the invalidation fanned out to every member;
  - observation of a device held by another member.
- **The console and the command line.** How a device with its applications is shown and named, and
  `Status` with the group (`runtime-client`).
- **Clones.** A member's identity becomes its package name with its Android user (or its clone
  index). The start, the force-stop and the foreground query then carry the user (keeping the
  `u<user>` the foreground parse drops today, `crates/device/src/adb.rs`). This waits for a test on
  a device of how a clone is addressed: whether a clone is another package or another user is not
  verified.

## Proposed codes, value keys and words

| Code | Owner | Category, ladder class | Values |
|---|---|---|---|
| `device_group_config_invalid` | `actingd` | error, `none` | `aliases`, `config_key`, `device_group_problem` |
| `device_group_assembled` | `actingd` | info, `none` | `aliases`, `device_label` |
| `device_group_waiting` | `policy` | info, `none` | `instance_id`, `alias`, `device_group_wait`, `task_id`?, `holder_kind`? |
| `application_switch_failed` | `runtime-host` | error, `none` | `alias`, `application_switch_step`, `failure_code`, `application_id` |
| `device_group_action_refused` | `runtime-host` | error, `none` | `alias`, `operation` |

New value keys, each with a proposed vocabulary:

- `device_group_problem`: `device_mismatch`, `handset_member`, `fixture_member`,
  `application_shared`, `startup_package_missing`, `device_action_unsupported`,
  `undeclared_shared_device`.
- `device_group_wait`: `selected`, `held`, `player_hold`, `fenced`.
- `application_switch_step`: `foreground_read`, `stop`, `startup_package`.

Every other key is an existing key of the one key table
(`crates/actingcommand-contract/src/outcome/keys.rs`).

## Not verified

- What `am force-stop` does to a package that is not running, on an emulator or a phone.
- Whether two sessions of two members (two Nemu connections, two touch servers) can be open on one
  emulator at once; slice 1 never opens two.
- Whether an emulator's ADB port stays the same across its restart.
- How an application clone is addressed on an emulator or on a phone (another package, or the same
  package under another Android user), and which one `monkey` starts when both exist.
- Whether the readers of check-config's report (the installer, the Lab) deny unknown members.
