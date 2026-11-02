# Agent forward: how the Runtime reaches each client's session

Workflow #432. [agent-wake.md](agent-wake.md) says what a wake is and in which states it ends, in
neutral words. This contract says how a message reaches a session of each client, so **client words
are allowed here and in `crates/agent-forward`, and nowhere else in the Runtime's sources.**

**State of this contract (AGT-T3, 0.12.37).** Claude Code (AGT-T1) and DSH (AGT-T3) have
transports. The other clients' channels can be registered and wait as `Deferred` (`ipc_unavailable`).
The Runtime's assembly uses `RealForward::new(state_root)`. Nothing here starts a stopped session: its
message waits and fails loudly at give-up ([agent-wake.md](agent-wake.md), "Delivery states").

## `actingctl agent-channel`

```text
actingctl agent-channel add --state-root <path> --name <name> --client <claude|codex|dsh|antigravity> --native-id <id> [--dsh-app-root <path> --dsh-home <path> --dsh-node <path>]
actingctl agent-channel remove --state-root <path> --name <name>
actingctl agent-channel list --state-root <path>
actingctl agent-channel claude-hook <start|end> --state-root <path>
actingctl agent-channel claude-register --state-root <path> --name <name>
actingctl agent-channel notice
```

- None of them needs the Runtime or an installation slot, and none is an MCP tool. They touch only
  `<state_root>\agent-forward\`, under one file lock, each file through `.part`, flush and rename.
- `add`, `remove`, `list` and `claude-register` print the one result line, with
  `agent_channel_reported` or `agent_channel_failed`; a bad command line is
  `actingctl_usage_invalid`.
- **`add` checks the registry's rules** and, for a Claude channel, that the id is a session's UUID.
  For a DSH channel it checks that the id is one DSH session id (a letter or a digit, then letters,
  digits, `-` or `_`, 256 bytes at most) and, by the registry's rules, that all three paths are given
  and absolute. It does **not** check the message floor of [agent-wake.md](agent-wake.md): it has only
  the state root and cannot see `summary.max_chars`. The start and `check-config` check the floor,
  and the send is refused loudly below it.
- **`remove`** also rewrites a removed Claude channel's live record with no pipe and no token.
- **`notice`** prints the MIT text of the adapted source, as plain text. See "Licence".

## Claude Code

### What a channel stores

`<state_root>\agent-forward\claude-sessions\<native id>.json`, schema
`actingcommand.agent-claude-session.v1`: the owner's process id and creation time, `online`, the pipe's
name, and the token as a DPAPI blob for the current Windows user.

**The token** is in that blob and briefly in memory, and nowhere else: not in a `detail`, a log, an
error, a ledger field, a configuration key or a test snapshot. The frame is zeroed after the write.
A `detail` also names neither the pipe nor the native id.

### The hook

```json
"SessionStart": [{ "hooks": [{ "type": "command", "timeout": 30, "command": "<root>\\runtime\\actingctl.exe agent-channel claude-hook start --state-root <root>\\state" }] }],
"SessionEnd":   [{ "hooks": [{ "type": "command", "timeout": 30, "command": "<root>\\runtime\\actingctl.exe agent-channel claude-hook end --state-root <root>\\state" }] }]
```

| Case | `start` | `end` | Exit | Prints |
|---|---|---|---|---|
| Not a registered Claude channel, or no registry file | stores nothing, leaves the session's variables and processes unread | | 0 | nothing |
| A channel | stores the record | rewrites it `online: false` with no pipe and no token | 0 | nothing |
| No record at the end | | nothing to do | 0 | nothing |
| The end comes from another process than the record's owner | | the record stays | 0 | nothing |
| A channel, and something is wrong: a variable missing, a bad pipe name or token, no Claude Code parent, a file that cannot be written | stores nothing | the record stays | **2** | one stderr line |
| Input that is not the hook's shape; an unreadable registry at `start`; an unreadable record at `end` | stores nothing | the record stays | **2** | one stderr line |

- **stdout stays empty on purpose.** Claude Code adds a SessionStart hook's plain stdout to the
  session's context. This is why these two commands print no result line.
- **Exit 2, not 1.** Claude Code's page says exit 2 on these events shows stderr to the person as a
  hook error, and the session goes on. The stderr line never holds the token.
- **An unreadable registry at `start` fails even for a session that may be no channel:** the hook cannot
  tell, and a silent exit would lose a channel with no sign.
- `end` reads the record and not the registry, so a token does not outlive a removed channel's end.
- It never needs the Runtime.

### A session already running

`claude-register` stores the same record, from inside that session. It needs the channel to exist and
`CLAUDE_CODE_SESSION_ID` to be that channel's id, so a credential is not stored under another session's
channel. **It is the person's step:** a command line cannot enforce who runs it, so the rule that no
agent runs it for her lives in the skill. A session restart makes the record stale, since the pipe and
the token change, and nothing marks it ended; the owner check then finds it gone.

### One send

| Step | Result when it fails |
|---|---|
| The id is a session's UUID | `Failed`, `channel_unknown` |
| Not stopped, not out of time | `Deferred`, `target_unresponsive` |
| The record exists, is readable, and is `online` with a pipe and a token | `Deferred`, `target_offline`; a record of another shape is `Failed`, `protocol_changed` |
| The owner runs with the same creation time | `Deferred`, `target_offline`; it cannot be checked: `Failed`, `owner_mismatch` |
| The pipe's name is `\\.\pipe\cc-msg-…` or `\\.\pipe\LOCAL\cc-msg-…` | `Failed`, `protocol_changed` |
| The owner is the same Windows user | `Failed`, `owner_mismatch` |
| The pipe opens | errors 2 and 231: `Deferred`, `target_offline`; another error: `Failed`, `write_failed` |
| The pipe's server is the owner | `Failed`, `owner_mismatch` |
| The token decrypts | `Failed`, `rejected`. A Runtime under another Windows user ends here or at the user check |
| `before_write` commits the intent | `Failed`, `write_failed`: nothing was written |
| One overlapped write | deadline: `Unknown`, `write_timed_out`; a stop or an error: `Unknown`, `write_failed`. Never tried again |
| | `Delivered`, `written` |

- **The frame:** `{"type":"auth","token":"…"}\n{"type":"user","message":{"role":"user","content":"…"}}\n`.
- **The write's deadline** is the send's, at most 25 seconds. The connection is opened only when the
  message is ready: Claude Code closes a connection with no complete line after 30 seconds.
- **There is no flush.** `FlushFileBuffers` waits for the reader and cannot be cancelled, so it would
  break the deadline. A finished write is in the pipe, and closing the client leaves it there.

## What this costs

The Runtime now depends on a private client protocol that can change.

| What can change | What happens | Loud? |
|---|---|---|
| The `cc-msg-` shape, the variables | Refused at the hook or the preflight | Yes: a hook error in her session; `failed` or `expired`; `attention` |
| **The second frame line; the token's rules; an inbound `hold` or `refuse`** | The write succeeds and nothing arrives | **No. This is the one silent case:** the row says `delivered / written`. Only a person's eyes on a test wake show it. **Run a test wake after each Claude Code update** |
| A resume or `/clear` gives the session a new id | The channel goes offline | Late: `expired` at give-up; she updates the channel |

**What `written` does not show.** A finished write proves only the write. A `hold`, a `refuse`, a
held message and an invalid token all look the same from here.

**Where each fact comes from.** The pipe, the token, the auth line and its requirement on native
Windows are on Claude Code's public pages. **The second line's shape is not:** it comes from the
adapted source, which cites a third-party repository. Whether a raw frame gets the "from another
session" framing has not been checked on a live session.

**Other costs:**

- **The Runtime holds a credential.** Each registered session's token sits in a DPAPI file. Anyone
  running as her Windows user can decrypt it. It is limited to registered channels and cleared at
  SessionEnd. The folder gets no ACL of its own beyond what the state root has.
- **The token makes the Runtime's message count as the session's own child.** On native Windows the
  token is the only evidence Claude Code has for that. Using it from a process that is not a child
  stretches the documented purpose ("a script posting to its own session's socket"). Anthropic could
  narrow it; then the session holds or drops the message, the silent row above.
- **A wake spends the recipient's quota** like a typed prompt. Caps and quiet bound it.
- **The Runtime must run as her Windows user,** or the token fails to decrypt, loudly.
- **Claude Code's version:** v2.1.234 or later on native Windows; the token needs v2.1.228.
- **`unsafe`:** three modules (`pipe`, `dpapi`, `ancestor`), each new to review. The crate denies it
  everywhere else, and `actingctl` stays `forbid(unsafe_code)`.

**#419 holds.** A client protocol failure is a value of `ForwardOutcome`. A hung client costs at most
one deadline, in the calling thread. The hook is another process, and its failure leaves the Runtime
unaffected.

## DSH

### What a channel stores

Only its registry line: the session id and three absolute paths. `app_root` is DSH's installed app,
whose `node_modules` the helper loads; `home_root` is the DSH home, with `sessions\` and `.web-url`;
`node_executable` is the Node that runs the helper.

```text
actingctl agent-channel add --state-root <path> --name <name> --client dsh --native-id <session id> --dsh-app-root <path> --dsh-home <path> --dsh-node <path>
```

**No credential reaches the Runtime.** The local server's address is in `<home>\.web-url` and the
sign-in gives cookies; only the helper reads either, and it removes the address, its query values and
the cookies from every error it writes.

### The helper

`crates/agent-forward/src/dsh_notify.mjs`, embedded in the binaries and run as
`<node> --input-type=module -e <helper>`, with no window and the app root as its directory. Its input
is one JSON object on stdin; its answer is one JSON object on stdout, of schema
`dsh-native-notification/v1`, naming the session. `target` is
`{app_root, home_root, node_executable, session_id}`.

| Run | Input | What it does |
|---|---|---|
| `check` | `{mode, target, timeout_ms}` | Reads the session's durable log through DSH's own modules, reads `.web-url` (a loopback address only) and signs in. Answers `ready`. Submits nothing |
| `send` | `{mode, target, request_id, rpc_id, text, timeout_ms}` | Reads the log: text already there under the request id is `recorded` (`already_recorded`) with no new submission. Otherwise it signs in, posts `session/prompt` once with `mode: "queue"`, needs an acknowledgement with the same RPC id that says accepted, and reads the log until the exact text is there under the request id (`submitted_once`). A lost acknowledgement or readback never repeats the post |

- **Request ids.** SHA-256 of the message's 16-byte id and a fixed label, its first 16 bytes written
  as a UUID of version 8; one label gives the request id, another the RPC id. Every try of one message
  has the same two.
- **Time.** One deadline, the send's, covers both runs. A run's own limit, in its input, is the time
  left less 1 s, at most 120 s; with less than 2 s left nothing is started. A run still going at the
  deadline, or when the Runtime stops, is killed and waited for in the calling thread.
- **Bounds.** 2 MiB on stdout and 1 MiB on stderr; one byte more kills the run.
- **Details.** A `detail` holds the helper's error code and message, and its stderr when its answer
  cannot be read; never a credential and never the session's log.
- **The adapted source's unit test of the helper is not carried:** CI runs no Node. The acceptance run
  on a live session is the helper's check.

### One send

| Step | Result when it fails |
|---|---|
| The channel has its three paths and its id is one DSH session id | `Failed`, `channel_unknown` |
| Not stopped, and at least 2 s left | `Deferred`, `target_unresponsive` |
| `check` starts | `Failed`, `helper_failed`: Node or the app root is missing |
| `check` ends before the deadline and the stop | `Deferred`, `target_unresponsive`; the helper is killed |
| Its answer is within the bounds, JSON of the schema, for this session | `Failed`, `helper_failed` |
| It answers `ready` with exit 0 | `Deferred`, `target_offline`, with the helper's code and message: the server is down, the session has no durable log, DSH's modules moved |
| Not stopped, and at least 2 s left | `Deferred`, `target_unresponsive` |
| `before_write` commits the intent | `Failed`, `write_failed`: nothing was submitted |
| `send` starts | `Failed`, `helper_failed`: it never ran |
| `send` ends before the deadline and the stop | deadline: `Unknown`, `write_timed_out`; stop: `Unknown`, `write_failed`. The helper is killed |
| Its answer is within the bounds, JSON of the schema, for this session | `Unknown`, `helper_failed` |
| It names this message's request id and RPC id | `Unknown`, `receipt_mismatch` |
| It is neither `not_submitted` nor `rejected` | `Failed`, `rejected` |
| It is `recorded`, accepted, with at least one log event, and exit 0 | `Unknown`, `receipt_mismatch`: the helper's `unknown` (it posted, then lost the acknowledgement or the readback, `READBACK_PENDING` included) and every partial proof. Never sent again |
| | `Delivered`, `recorded` |

- **`not_submitted` after the intent is `failed`,** although nothing was posted: once the intent is
  committed the row never goes back to waiting. A server that goes down between the two runs ends the
  message loudly.
- **What `recorded` shows:** the exact text is in the session's durable log under the request id. It
  does not show that the session's turn ran or finished.

### What DSH costs

| What can change | What happens | Loud? |
|---|---|---|
| DSH's private module paths, `node_modules\@deepseek-ai\cordis` and `node_modules\@deepseek-ai\dsh-session-persistence-jsonl` under the app root, or their interface | `check` answers not ready, with the import's error | Late: `deferred`, then `expired` at give-up |
| `.web-url`, the sign-in, `/api/session/prompt`, `session/prompt`, the log's event shapes | `check` not ready; or `send` answers `not_submitted`, `rejected` or `unknown` | Yes: `failed` or `unknown`; a `check` that keeps failing is `expired` at give-up |
| Node moves | The helper cannot start | Yes: `failed`, `helper_failed` |

- **Each `check` starts Node and reads the session's log,** so a channel's tries stay at
  `retry_interval_ms`.
- **The text arrives as user input.** Nothing structural marks it as a program notice; the fixed
  first line and the skill's section are all there is.
- **Not verified on a live session.** Every row above rests on the adapted source. Whether a prompt
  reaches an idle old session while its server is up is not verified either.

## Licence

The method and parts of the code come from `HS7097/codex-goal-lifecycle` at `c9538cc5`, MIT, copyright
2026 HS7097. `crates/agent-forward/LICENSE-codex-goal-lifecycle.txt` holds the text verbatim, each
adapted file says so at its top (the embedded DSH helper too, so the line travels with it),
`NOTICE.md` lists it, and `actingctl agent-channel notice` prints it, since binaries carry the code.
