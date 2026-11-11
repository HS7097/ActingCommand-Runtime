# Agent wake: the Runtime's messages to registered channels

Workflow #432. The Runtime tells an agent session that something needs it by sending that session
one short message. The forwarding is the Runtime's own function, it has no sender identity, and the
message's first line is fixed. Client words live only in `crates/agent-forward`.

**State of this contract (AGT-2b, 0.12.38).** The configuration, the registry's shape, the worker, the
rows, the read faces, a person's test, the first triggers and the daily summary exist. **Wakes happen
automatically once a section with rules exists** ("Triggers, rules and guards" below), and **a
`daily_summary` rule sends one summary a day at its minute** ("The daily summary" below); nothing is
configured in production, so nothing wakes there until a person adds the section. **Three transports
exist** ([agent-forward.md](agent-forward.md)): Claude Code (AGT-T1), Codex (AGT-T2) and DSH (AGT-T3)
channels are forwarded to. An Antigravity channel can be registered and answers `deferred` until its
own transport lands (AGT-T4), so its message waits and ends `expired` at give-up. With no
`agent_wake` section the only difference a Runtime shows is the one
`ledger_observer.tag.workbench_patterns` entry: no thread, no file, no row and no status member.

## The `agent_wake` section

The section is optional and is **whole or absent**: every key's default is none, neither code nor
the defaults file holds a value, and a section that leaves out a key is refused naming that key
(`required_missing`). An empty section is refused too, naming `agent_wake.enabled`. With no section
there are no keys and no rows, and `ConfigureSettings` answers `config_key_unknown`; turning it on
the first time is a file edit and a restart, a person's step. `actingd-config` revision 24
([component-interfaces.md](component-interfaces.md)) adds the section, and revision 25 (0.12.38) a
rule's `on.at_minute_of_day` and `on.utc_offset_minutes`.

**Turning it on, in this order: first update the console, then add the section.** `runtime-client`
stays [3, 5], so the pairing check passes for a console whose `max` is 3 or 4, but the status denies
unknown members: with the section present such a console cannot read the status at all.

An example with the shipped values:

```json
"agent_wake": {
  "enabled": true,
  "summary": { "max_chars": 100 },
  "body": { "dir": "agent-wakes", "max_bytes": 65536, "retention_days": 14, "max_files": 512 },
  "delivery": { "queue_capacity": 64, "coalesce_ms": 3000, "send_deadline_ms": 30000,
                "retry_interval_ms": 300000, "give_up_ms": 86400000, "channel_gap_ms": 5000,
                "history_rows": 32, "panic_backoff_ms": 60000, "panic_backoff_max_ms": 900000 },
  "quiet": { "windows": [ { "id": "restart", "weekdays": [1, 4], "utc_offset_minutes": 540,
                            "start_minute_of_day": 280, "end_minute_of_day": 480 } ],
             "hold_until_unix_ms": 0 },
  "budget": { "daily_cap": 24 },
  "rules": [
    { "id": "ladder", "on": { "kinds": ["ladder_exhausted"] }, "instances": "*",
      "to": "<channel>", "cooldown_ms": 1800000, "daily_cap": 8 },
    { "id": "fence", "on": { "kinds": ["instance_fenced"] }, "instances": "*",
      "to": "<channel>", "cooldown_ms": 1800000, "daily_cap": 8 },
    { "id": "runs", "on": { "kinds": ["run_failed"], "codes": ["<registered code>"] },
      "instances": ["<alias>"], "to": "<channel>", "cooldown_ms": 1800000, "daily_cap": 8 },
    { "id": "daily", "on": { "kinds": ["daily_summary"], "at_minute_of_day": 1260,
      "utc_offset_minutes": 540 }, "instances": "*", "to": "<channel>", "cooldown_ms": 0,
      "daily_cap": 1 }
  ]
}
```

| Key under `agent_wake.` | Type and range | Settable |
|---|---|---|
| `enabled` | boolean | yes |
| `summary.max_chars` | integer, 48 to 2048: the longest message in Unicode scalars, its header included | yes |
| `body.dir` | a relative folder of plain names below the state root | no, read once at assembly: open rows name files under the old folder |
| `body.max_bytes` | integer, 4096 to 1048576 | yes |
| `body.retention_days` | integer, 1 to 365 | yes |
| `body.max_files` | integer, 8 to 65536 | yes |
| `delivery.queue_capacity` | integer, 1 to 1024 | no, read once at assembly: sized at start |
| `delivery.coalesce_ms` | milliseconds, 0 to 60000 | yes |
| `delivery.send_deadline_ms` | milliseconds, 1000 to 120000 | yes |
| `delivery.retry_interval_ms` | milliseconds, 1000 to 86400000 | yes |
| `delivery.give_up_ms` | milliseconds, 60000 to 2419200000 | yes |
| `delivery.channel_gap_ms` | milliseconds, 0 to 86400000 | yes |
| `delivery.history_rows` | integer, 1 to 256 (one record list of `host.agent_wake`) | yes |
| `delivery.panic_backoff_ms` | milliseconds, 1000 to 86400000 | yes |
| `delivery.panic_backoff_max_ms` | milliseconds, 1000 to 86400000 | yes |
| `quiet.windows` | list: at most 64 blackout entries, read by the same rules as [scheduling-blackout.md](scheduling-blackout.md), "Configuration blackouts" | yes |
| `quiet.hold_until_unix_ms` | integer: no message before this time; 0 holds nothing | yes |
| `budget.daily_cap` | integer, 1 to 256: the most wakes of one local day over all rules | yes |
| `rules` | list of at most 64 rules | yes |

A rule is `{id, on: {kinds, codes?, at_minute_of_day?, utc_offset_minutes?}, instances, to,
cooldown_ms, daily_cap}`. `id` is 1 to 64 bytes of letters, digits, `_` and `-`. `kinds` come from
the closed list `ladder_exhausted`, `instance_fenced`, `run_failed`, `daily_summary`,
`delivery_test` and (HELD, not built yet; Workflow #438 HELD-2) `task_held`; these are
configuration words, not the ledger's `AgentWakeKind`. A Runtime before HELD-2 refuses a rule that
names `task_held` (`agent_wake_config_invalid`, `kind_unknown`), so the word raises `actingd-config`
([component-interfaces.md](component-interfaces.md), "Planned: Workflow #438 HELD"). `codes` applies to
`run_failed` only, which needs it. `instances` is `"*"` or a list of aliases. `to` is a channel name:
1 to 16 Unicode scalars with no control character, opaque to the section. `cooldown_ms` is at most a
week, and `daily_cap` is at least 1. **A `daily_summary` rule** (AGT-2b) names that kind alone, no
`codes`, `at_minute_of_day` (0 to 1439) and `utc_offset_minutes` (−720 to 840, minutes east of UTC,
the form of a quiet window), and `daily_cap` 1; its `cooldown_ms` is not read. A rule of another kind
names neither member. An instruction may point a rule at another registered channel; it can never
add a channel. What the rules do is in "Triggers, rules and guards" and "The daily summary".

### Refusals

A value of the wrong type or outside its key's range is `config_value_invalid` as the file layer is
rendered, as in every section. A broken rule below is `agent_wake_config_invalid` (`config_key`,
`config_value_problem`) at start and in `check-config`; through an instruction the same values are
`config_value_invalid` with the same key and problem, and the configuration in effect stays.

| Rule | `config_value_problem` |
|---|---|
| A key left out; an empty section | `required_missing` |
| Two rules with one id | `duplicate_id` |
| A kind outside the closed list | `kind_unknown` |
| A rule with no kind; `run_failed` with no `codes` | `match_missing` |
| A `daily_summary` rule with another kind or `codes`, without its minute or offset or with one out of range, or a `daily_cap` other than 1; another kind's rule that names a minute or an offset | `daily_summary_shape` |
| A code the catalog does not list, or has retired | `code_unknown`, `code_retired` |
| `instances` neither `"*"` nor a non-empty list of configured aliases | `instance_unknown` |
| `rule.daily_cap` above `budget.daily_cap`, or `budget.daily_cap` above `delivery.history_rows` | `caps_exceed` |
| `body.dir` inside `ledger`, `artifacts`, `release-blobs` or `kept` | `dir_reserved` |
| `body.dir` that is absolute, or holds `..` or anything but plain names | `out_of_range` |
| `panic_backoff_ms` above `panic_backoff_max_ms`; a bad quiet window | `section_check` |
| A registered channel whose shortest message does not fit `summary.max_chars` | `message_floor` |

Whether each channel fits needs the registry and the state root, so only the start and `check-config`
check it. A limit an instruction sets below a channel's floor passes the section check; that send is then
refused, loudly (`agent_wake_message_too_long`), and never carries a cut path. Whether each alias is
configured needs the instances: the start, `check-config` and, for an instruction that changes
`agent_wake.rules`, the host check it (`config_value_invalid`, `instance_unknown`).

**Notes, never refusals (#419).** A registry that is missing or unreadable (`registry_unreadable`), a
rule's `to` the registry does not list (`channel_unknown`) and a program a channel names that is not
there (`channel_program_missing`) are `note` links of `agent_wake_config_invalid` on the
`agent_wake` subsystem row. None of them stops a start.

## Triggers, rules and guards

Workflow #432 AGT-2. A wake is the Runtime's own function and independent of scheduling: a scheduling
pause stops no wake, and a wake never touches a pause, a lease, a queue or a fence.

**The tap.** One call beside direction ①'s count on the append path sees each committed record once.
It works in memory only, under the bounded queue's own leaf lock. A full queue drops the candidate and
counts it (`delivery.queue_capacity`); the worker then says it once (`agent_wake_delivery_failed`,
`wake_queue_full`). Nothing is recorded and nothing can fail on the write path. The queue is built with
the host's shared state, so a record committed during the start, before the worker's thread runs,
waits for it. Candidates live in memory: a crash loses those not yet made rows.

**The first triggers.** A candidate needs an instance link, except a held record (HELD, not built
yet): a planning signal is appended with the system links and no instance link, so its instance is
the registered instance whose alias the record's `instance_id` names, and a record whose alias no
registered instance has is no candidate (dropped, not counted; never a failure, #419).

| Kind | Record | Word | Never a candidate |
|---|---|---|---|
| `ladder_exhausted` | the end (`recovery_ladder_finished`, `exhausted`) of a ladder a trigger started, with `recovery_ladder_exhausted` | 阶梯用尽 | an interrupted ladder (its code is `recovery_ladder_interrupted`), a person's drill (tagged `drill`), a plan drill (`planned`) |
| `instance_fenced` | `runtime.failed` with `instance_resources_fence_awaiting_person`: a fence began to wait for a person | 实例隔离 | `instance_resources_fenced`, the code of a fence's own record and of the failure of work that met it |
| `run_failed` | a run's failed terminal (`task.failed`, `terminal_committed`), by the run's failure code (a relayed spelling the catalog does not register is its `raw_code`) | 运行失败 | a code no rule lists; another writer's `task.failed`, which is no terminal |
| `task_held` (HELD, not built yet; HELD-2) | a held record: `policy.planning_signal_observed` of kind `goal_missed` whose outcome is `task_held` ([scheduling/README.md](scheduling/README.md), "Held records") | 任务滞留 | its end (`task_held_ended`), and every other planning signal |

The worker's own codes (`agent_wake_*`) and `agent_budget_exhausted` are never candidates, so a
delivery failure or a used-up budget cannot make a wake. `daily_summary` is no record's kind: its
rule's clock makes it ("The daily summary").

**Rules.** A rule matches a candidate of one of its `kinds` on an instance its `instances` covers;
`run_failed` also needs the candidate's code in the rule's `codes`: no list, no wake. A candidate no
rule matches is dropped and not counted. A `task_held` rule (HELD, not built yet) names no `codes`,
as a `ladder_exhausted` rule: it matches every held record of the instances it covers, whatever
the reason (`gate_code`) or the task.

**Coalescing.** One instance's matched candidates are gathered for `delivery.coalesce_ms` from the
first one (0: those taken in the same pass). Then each channel receives at most one wake: of the rules
that send to it, the one whose matched candidate is the strongest (`ladder_exhausted`, then
`instance_fenced`, then `run_failed`, then, the weakest (HELD, not built yet), `task_held`; the
earlier one on a tie, then the rule written first). The
burst's other candidates are listed in the body's `related` as `{kind, event_id, code}`. A ladder's
end followed by its fence's wait for a person is therefore one wake; the burst is by instance, not by
correlation, because the person-wait record has fresh links. A burst holds at most
`delivery.queue_capacity` candidates; past that a stronger one takes the weakest one's place, and the
one left out is counted as suppressed.

**Guards,** in this order, each read from the rows of `host.agent_wake`, which a start restores, so a
restart forgets none of them:

1. **Cool-down:** no second wake of one rule, instance and code within the rule's `cooldown_ms`.
   A held record's code is `task_held` whatever its reason (HELD, not built yet), so one instance's
   held records wake a rule at most once per `cooldown_ms`, whatever the task or the reason.
2. **Rule cap:** at most the rule's `daily_cap` wakes per local day.
3. **Budget:** at most `budget.daily_cap` wakes per local day over all rules.

A day is the **host's local day**: the machine's zone at that instant, daylight saving time included.
A held candidate is counted in `suppressed_today` and never recorded. A person's test skips the
guards and counts toward no cap.

**A used-up cap is said once a day.** The wake that fills a rule's cap, or the budget, writes one
`agent_budget_exhausted` (a warning with no instance link: `config_key` `agent_wake.rules` with
`label` the rule's id and `agent_channel` its channel, or `agent_wake.budget.daily_cap`; `limit_count`
the cap; `alias` the instance). Wakes of that rule, or of every rule, then stop until the next local
day, and `Status.agent_wake.attention` shows it. A cap an instruction lowers below the day's count
holds at once and says nothing more that day. If the host's local day cannot be read, the iteration
fails (`agent_wake_delivery_failed`, `local_time_unknown`) before it takes anything, and the
candidates wait in the queue.

**The body of an automatic wake** carries `rule`, `instance_alias`, `trigger` (the record's type, id,
sequence and links, its outcome codes only) and `related`, and `next_reads`: for a failed run first
`ac_get_run` with the run's request id, then `ac_events` with the instance and `after_sequence` just
before the burst's first record. For a held record (HELD, not built yet): first `ac_schedule` with
the instance (the not-run explanation of that instance, whose row carries the hold's
`since_unix_ms`), then `ac_events` with `event_types` `["policy.planning_signal_observed"]` and
`after_sequence` just before the burst's first record and no instance, since a planning signal has
no instance link and a read filtered to the instance would not return it. Its `trigger` has no
request, run or correlation id (the system links carry none).

## The daily summary

Workflow #432 AGT-2b (Alice 11-02: the timed report goes through this channel). A `daily_summary`
rule's clock makes the wake; no record does. Its day is the **local day of the rule's own
`utc_offset_minutes`**, not the host's.

- **Once a local day, at its minute.** From that day's `at_minute_of_day` to its end the day's summary
  is due; the worker looks at least once a second and makes it once, for the rule's `instances`, to
  its `to`. The rows of `host.agent_wake` say whether the day has one, so a restart makes no second.
- **Missed.** If the Runtime was not running at that minute, it makes the summary once after the
  start, the same local day, if the day has none. Never one for an earlier day.
- **Quiet and off.** While a quiet window or the hold covers now, or the section is off, no summary
  is made: it waits for the end and is made then, once. If the day ends first, that day has none,
  and the next summary names it (`skipped`).
- **Guards.** Once a day replaces the cool-down and the rule's cap; its `daily_cap` 1 is never a cap
  used up. The budget holds it and counts it like every wake: a used-up budget holds that day's
  summary, counted once in `suppressed_today`.
- **The message:** `今日汇总 <date> 正文 <path>`, the rule's local date `YYYY-MM-DD` where an alias
  stands. The limit holds as for every wake: only the date is ever cut, never the kind word or the
  path. The row has no `instance_alias` and no `code`; `alias_cut` says whether the date was cut.
- **The body** (`actingcommand.agent-wake-body.v1`, kind `daily_summary`, `rule`, no
  `instance_alias`, no `trigger`, empty `related` and `next_reads`) carries `summary`:

  ```json
  "summary": {
    "date": "2026-11-03", "at_minute_of_day": 1260, "utc_offset_minutes": 540,
    "runtime": { "owner_epoch": "epoch_…", "started_at_unix_ms": 0 },
    "instances": [ { "instance_alias": "…", "routine_summary_refused": { "…": "codes only" } } ],
    "wakes": [ { "kind": "instance_fenced", "state": "delivered", "count": 1 } ],
    "previous": { "date": "2026-11-01", "state": "delivered" },
    "skipped": { "from": "2026-11-02", "through": "2026-11-02", "days": 1 } }
  ```

  - `instances`: one entry per instance of the rule (every registered alias, sorted, for `"*"`), each
    with what `RoutineSummary` ([runtime-interface.md](runtime-interface.md), `granularity` `day`)
    answers for it from the host's memory, through the read's own function: its page as
    `routine_summary_page`, or its refusal, codes only, as `routine_summary_refused`. No ledger read
    beyond what that read does. From R-12 (Workflow #429, since 0.12.38) the read answers its page,
    and the page in the body keeps only the buckets that overlap the summary's own local day (the
    rule's offset; for a rule of the host's zone, that day's one bucket), its other members as the
    read answered them; before 0.12.38 the read answered `not_implemented`, and each entry carried
    that refusal.
  - `wakes`: the rows of that local day before this summary, by `kind` and the `state` each is in now
    (tests and other summaries included), as `host.agent_wake` holds them (`delivery.history_rows`).
  - `runtime`: the Runtime's owner epoch and when this start began, as `StartReport` says them.
  - `previous`: the rule's newest earlier summary the rows hold, with its state now; `null` when they
    hold none. `skipped`: the days after it with no summary, `null` when there are none or there is no
    `previous` to count from.
- A summary never touches a pause, a lease, a queue or a fence, as every wake.

## The message

```text
ActingCommand to <channel>
<kind word> <instance alias> 正文 <absolute body path>
```

- Line 1 is fixed apart from the channel, and a channel name cannot forge it: it has no control
  character. There is one `\n`, no blank line, no footer.
- Kind words: `ladder_exhausted` 阶梯用尽, `instance_fenced` 实例隔离, `run_failed` 运行失败,
  `daily_summary` 今日汇总, `delivery_test` 发送测试, and (HELD, not built yet) `task_held` 任务滞留,
  four characters as the others, so the floor of every channel is unchanged. A build before HELD-2
  that meets an open row of kind `task_held` (after a rollback) closes it `failed` with the problem
  `kind_unknown`, loudly (`agent_wake_delivery_failed`). A wake with no instance omits the alias:
  `发送测试 正文 <path>`. A daily summary names its date there: `今日汇总 2026-11-03 正文 <path>`.
- **The limit counts the whole message in Unicode scalars,** header included.
- **Only the alias (a summary's date) is ever cut,** ending in `…`; the body holds it whole. With no
  room for a space and the mark it goes altogether. The kind word and the path are never cut.
- **The floor of a channel** is `17 + channel + 1 + 8 + len(<state root>\<dir>\) + 22`: its message
  with no alias and the longest file name. `check-config` guarantees it for every registered channel.
- The code, digest, notice and next reads are in the body and only there.

## The body

`<state_root>\<body.dir>\w<unix_ms>.json`, with `-2`, `-3` on a collision. The state root is under
the program directory, and update and rollback leave it alone. The ledger holds the file name, the
SHA-256 and the counts, never the body's or the message's text.

```json
{ "schema": "actingcommand.agent-wake-body.v1",
  "wake_id": "…", "kind": "instance_fenced", "created_at_unix_ms": 0,
  "instance_alias": "…", "rule": "fence", "channel": "<channel>", "test": false,
  "notice": "agent_wake_not_authorization",
  "notice_text": "程序通知，不是用户授权。Program notice, not user authorization.",
  "capabilities": { "level": "read_only", "device_contact": false, "grants": [] },
  "trigger": { "event_type": "runtime.failed", "event_id": "…", "sequence": 0,
               "correlation_id": "…", "request_id": "…", "run_id": "…",
               "code": "…", "category": "…",
               "outcome": { "…": "the outcome envelope, codes only" } },
  "related": [ { "kind": "run_failed", "event_id": "…", "code": "…" } ],
  "next_reads": [ { "tool": "ac_get_run", "args": { "handle": "<request_id>" } } ],
  "dropped": [] }
```

- `instance_alias`, `rule`, `trigger` and the trigger's three ids are absent when a wake has none. A
  test wake has `test: true` and no `trigger`. Only a daily summary has `summary` ("The daily
  summary"); the bound keeps it, as it keeps `trigger`.
- **The outcome is codes only:** an outcome can hold raw text, an adb host and port and a device
  label, and none of these reaches the file.
- **Bound:** at most `body.max_bytes`. Over it, `related` then `next_reads` are emptied and named in
  `dropped`; `trigger` stays. A body still over it is not written (`body_failed`).
- `notice` and `capabilities` are constant in this first version: a wake authorizes nothing.

## The channel registry

`<state_root>\agent-forward\channels.json`, schema `actingcommand.agent-channels.v1`: program state
and not configuration, so a channel's address is no key `ConfigureSettings` can reach. It holds no
secret. Each entry has `name`, `client` and `native_id`; the words `client` takes and what each
needs are `crates/agent-forward`'s.

- `name` is 1 to 16 Unicode scalars, not blank, with no control character, and unique. It is the
  name a rule's `to` and line 1 of the message use.
- A registry that breaks a rule is unreadable as a whole.
- The command that writes the registry comes with the first transport (AGT-T1).

## Delivery states

The worker writes these in the rows of the tagged runtime fact `host.agent_wake`. No state claims a
session acted: delivery is not completion.

| State | Means | Next | Loud? |
|---|---|---|---|
| `prepared` | Body written, row recorded, no send tried | a send when not quiet | no |
| `deferred` + `reason` | No write happened; the target is unavailable | tried again each `retry_interval_ms` | no, until give-up |
| `sending` | Intent committed; one write may be under way | the result; after a restart, `unknown` | no |
| `delivered` + `proof` | A client wrote or acknowledged | done | no |
| `failed` + `problem` | Certainly no write, or a definite refusal | stop | yes |
| `unknown` + `problem` | A write may have happened | stop, never resent | yes |
| `expired` | `give_up_ms` passed while deferred | stop | yes |
| `body_failed` | The body could not be written | stop | yes |

**Rules.** The intent is durable before the write: a transport commits it through `before_write`, which
records `sending`. Once that returned, the row never writes again; after a restart `sending` is
`unknown`, never resent. Only `prepared` and `deferred` are ever tried, oldest first per channel, at
least `delivery.channel_gap_ms` apart. A row takes at most four records: `prepared`, the first
`deferred`, `sending` and the result; a later reason goes only to the detail file and the worker's
memory. After a stop, no new `sending`; `prepared` and `deferred` rows carry on at the next start.

`AgentForward` (`crates/agent-forward`) answers in neutral words, so its callers need no client word:
`Deferred` with a `reason`, `Delivered` with a `proof` (`written`, `accepted`, `recorded`,
`receipt`), `Failed` or `Unknown` with a `problem`. A transport commits the intent through
`before_write`, once, immediately before its first possible write, and writes nothing when that
fails. The Runtime assembles `RealForward`: a channel of a client with no transport in this build
answers `Deferred` and nothing is written ([agent-forward.md](agent-forward.md)).

## The fact `host.agent_wake`

Scope Runtime, a record list, source `runtime`, tagged so a start loads it, the newest
`delivery.history_rows` rows (open rows are never trimmed first). One row per wake:

`wake_id, kind, rule?, test, instance_alias?, code?, channel, created_at_unix_ms, updated_at_unix_ms,
state, reason?, proof?, problem?, body_file?, body_sha256?, body_chars?, message_chars?, alias_cut,
trigger_event_id?, correlation_id?`

It holds names, a digest and counts, never a body's or a message's text and never a channel's address.
A record of a shape this build does not read is said once (`invalid_runtime_state_fact`) and the start
goes on with no rows; the next write replaces it, and its files go through the retention sweep.

**The delivery detail file** `w<...>.delivery.json`, beside the body, is rewritten at each state and
retry: `{schema: "actingcommand.agent-wake-delivery.v1", wake_id, channel, state, reason, proof, problem,
message_text, detail, updated_at_unix_ms}`. This is where a person reads what went wrong: `detail` is the
client's own words, at most 4 KiB, and they reach no ledger field.

**Retention.** After each wake and hourly, by `body.retention_days`, then by `body.max_files`, oldest
first. It never removes an open row's files, touches only the worker's own file names and writes no
record but a failure's.

## Quiet and off

| Switch | New candidates | Sends | Open rows |
|---|---|---|---|
| `quiet.windows` | become rows and bodies | none | wait; `prepared` stays `prepared` |
| `quiet.hold_until_unix_ms` | become rows and bodies | none until that time | wait; `0` holds nothing |
| `enabled` false | dropped, counted | none | `deferred`, reason `disabled`; a row in `sending` finishes |

The give-up clock is read only when a send is due, so a quiet time causes no expiry noise: when it ends,
rows past `delivery.give_up_ms` become `expired`, loud once each, and younger ones go. Quiet writes no
record per tick. The Runtime does not read quota: whether a silence applies is a person's act.

A send under way is handed the worker's judgement of quiet at the moment a transport asks
(`SendLimits::quiet`, since 0.12.37): a transport opens no stopped session while a window or the hold
covers now, even one that began during the pass, and a judgement that fails counts as quiet
([agent-forward.md](agent-forward.md)).

## Failures

A delivery failure is a non-fatal `runtime.failed` with system links and no instance link, at most one per
wake: `agent_wake_delivery_failed` (`agent_delivery_state`, `agent_delivery_problem`, `agent_channel`,
`alias`, `file`), `agent_wake_message_too_long`, `agent_wake_channel_unknown`. It reaches no ladder, fence
or pause, and it can never become a wake. `agent_budget_exhausted` (a warning, "Triggers, rules and
guards") takes the same path. Every failure of the worker is `Operation`; only a real ledger
failure stays `Global`. A panic or an error of an iteration writes one such record (`worker_panicked`, or
the error) and waits `delivery.panic_backoff_ms`, doubling to `delivery.panic_backoff_max_ms`, reset by a
clean iteration. A worker thread that cannot start turns the feature off and says so
(`worker_spawn_failed`); it never stops a start. Candidates live in memory, so a crash may lose those not
yet made rows; the crash itself is loud.

## Read faces

- **`Status.agent_wake`** (`runtime-client` revision 5), from memory, absent with no section: `{enabled,
  worker: running|backing_off|unavailable, quiet?: window|hold, open, last_wake_at_unix_ms?, failed_today,
  suppressed_today, attention}`. `suppressed_today` counts the candidates dropped with the feature off, a
  full queue or burst, a cool-down or a cap, and a daily summary the budget held. `attention` is true
  only for a row that failed today, a cap used up today (a daily summary rule's cap of 1 never counts as
  used up), or an unreadable registry. A day is the host's local day (AGT-2; until then it was a UTC
  day).
- **`actingctl overview`, `ac_overview`:** the block `agent_wake`, that member of the one Status read; MCP
  never cuts it.
- **Rows:** `actingctl facts`, `ac_facts`, key `host.agent_wake`. **History:** `ac_events` by type, always
  with `after_sequence`.
- **A console whose `runtime-client` `max` is below 5 refuses a status that carries the member** (the status
  denies unknown members), so update the console before adding the section.

## A person's test

```text
actingctl agent-wake-test --state-root <root> --to <channel> [--instance <alias>] [--wait-s <seconds>]
```

The request `AgentWakeTest{to, instance_alias?}` takes the origins (User, Ui) and (Cli, Cli) only, else
`invalid_agent_wake_test_origin`, the same gate as a pause. **It is not an MCP tool:** an agent cannot use the
Runtime to ping others. It goes through the real worker, file, row and transport with kind `delivery_test`,
word 发送测试, `test: true` and no `trigger`, and it obeys quiet: the answer `{wake_id, channel, quiet?}` names
what holds it. Refusals: `agent_wake_disabled`, `agent_wake_channel_unknown` (a readable registry that does not
list the name, or a name of no channel's shape). A missing or unreadable registry takes the test, whose row
waits `deferred`, reason `registry_unreadable`. The line's outcome is `agent_wake_test_queued`; `--wait-s`
adds the wake's `row`.
