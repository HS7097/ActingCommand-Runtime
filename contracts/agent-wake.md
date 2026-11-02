# Agent wake: the Runtime's messages to registered channels

Workflow #432. The Runtime tells an agent session that something needs it by sending that session
one short message. The forwarding is the Runtime's own function, it has no sender identity, and the
message's first line is fixed. Client words live only in `crates/agent-forward`.

**State of this contract (AGT-1a, 0.12.37).** Only the configuration, the registry's shape and the
pure parts exist. No thread runs, nothing is sent, no body is written and `host.agent_wake` has no
writer yet. With no `agent_wake` section the only difference a Runtime shows is the one
`ledger_observer.tag.workbench_patterns` entry. Later slices fill in the worker, the transports and
the triggers, and say so here.

## The `agent_wake` section

The section is optional and is **whole or absent**: every key's default is none, neither code nor
the defaults file holds a value, and a section that leaves out a key is refused naming that key
(`required_missing`). An empty section is refused too, naming `agent_wake.enabled`. With no section
there are no keys and no rows, and `ConfigureSettings` answers `config_key_unknown`; turning it on
the first time is a file edit and a restart, a person's step. `actingd-config` revision 24
([component-interfaces.md](component-interfaces.md)) adds the section. An example with the shipped
values:

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
    { "id": "fence", "on": { "kinds": ["instance_fenced"] }, "instances": "*",
      "to": "<channel>", "cooldown_ms": 1800000, "daily_cap": 8 },
    { "id": "runs", "on": { "kinds": ["run_failed"], "codes": ["<registered code>"] },
      "instances": ["<alias>"], "to": "<channel>", "cooldown_ms": 1800000, "daily_cap": 8 }
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

A rule is `{id, on: {kinds, codes?}, instances, to, cooldown_ms, daily_cap}`. `id` is 1 to 64 bytes
of letters, digits, `_` and `-`. `kinds` come from the closed list `ladder_exhausted`,
`instance_fenced`, `run_failed`, `daily_summary`, `delivery_test`; these are configuration words,
not the ledger's `AgentWakeKind`. `instances` is `"*"` or a list of aliases. `to` is a channel name:
1 to 16 Unicode scalars with no control character, opaque to the section. `cooldown_ms` is at most
a week, and `daily_cap` is at least 1. An instruction may point a rule at another registered channel;
it can never add a channel.

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
| A code the catalog does not list, or has retired | `code_unknown`, `code_retired` |
| `instances` neither `"*"` nor a non-empty list of configured aliases | `instance_unknown` |
| `rule.daily_cap` above `budget.daily_cap`, or `budget.daily_cap` above `delivery.history_rows` | `caps_exceed` |
| `body.dir` inside `ledger`, `artifacts`, `release-blobs` or `kept` | `dir_reserved` |
| `body.dir` that is absolute, or holds `..` or anything but plain names | `out_of_range` |
| `panic_backoff_ms` above `panic_backoff_max_ms`; a bad quiet window | `section_check` |
| A registered channel whose shortest message does not fit `summary.max_chars` | `message_floor` |

Whether each alias is configured and whether each channel fits need the instances, the registry and
the state root, so only the start and `check-config` check them. A limit an instruction sets below a
channel's floor passes the section check; that send is then refused, loudly, and never carries a cut
path.

**Notes, never refusals (#419).** A registry that is missing or unreadable (`registry_unreadable`), a
rule's `to` the registry does not list (`channel_unknown`) and a program a channel names that is not
there (`channel_program_missing`) are `note` links of `agent_wake_config_invalid` on the
`agent_wake` subsystem row. None of them stops a start.

## The message

```text
ActingCommand to <channel>
<kind word> <instance alias> 正文 <absolute body path>
```

- Line 1 is fixed apart from the channel, and a channel name cannot forge it: it has no control
  character. There is one `\n`, no blank line, no footer.
- Kind words: `ladder_exhausted` 阶梯用尽, `instance_fenced` 实例隔离, `run_failed` 运行失败,
  `daily_summary` 今日汇总, `delivery_test` 发送测试. A wake with no instance omits the alias:
  `发送测试 正文 <path>`.
- **The limit counts the whole message in Unicode scalars,** header included.
- **Only the alias is ever cut,** ending in `…`; the body holds it whole. With no room for a space and
  the mark it goes altogether. The kind word and the path are never cut.
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
  test wake has `test: true` and no `trigger`.
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

Later slices write these in the rows of the tagged runtime fact `host.agent_wake`. No state claims a
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

`AgentForward` (`crates/agent-forward`) answers in neutral words, so its callers need no client word:
`Deferred` with a `reason`, `Delivered` with a `proof` (`written`, `accepted`, `recorded`,
`receipt`), `Failed` or `Unknown` with a `problem`. A transport commits the intent through
`before_write`, once, immediately before its first possible write, and writes nothing when that
fails. Until a transport exists, the implementation `Unavailable` answers `Deferred`.
