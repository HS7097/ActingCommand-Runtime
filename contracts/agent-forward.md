# Agent forward: how the Runtime reaches each client's session

Workflow #432. [agent-wake.md](agent-wake.md) says what a wake is and in which states it ends, in
neutral words. This contract says how a message reaches a session of each client, so **client words
are allowed here and in `crates/agent-forward`, and nowhere else in the Runtime's sources.**

**State of this contract (AGT-T1 and AGT-T2, 0.12.37).** Claude Code and Codex are the transports.
DSH and Antigravity channels can be registered and wait as `Deferred` (`ipc_unavailable`). The
Runtime's assembly uses `RealForward::new(state_root)`. Nothing here starts a stopped session or
opens a task that is not loaded: its message waits and fails loudly at give-up
([agent-wake.md](agent-wake.md), "Delivery states").

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
- **`add` checks the registry's rules** and, for a Claude channel, that the id is a session's UUID. It
  does **not** check the message floor of [agent-wake.md](agent-wake.md): it has only the state root
  and cannot see `summary.max_chars`. The start and `check-config` check the floor, and the send is
  refused loudly below it.
- **A Codex channel** is `add --client codex --native-id <task UUID>` and nothing more. `add` keeps the
  id as typed; the send refuses one that is not a task's UUID (see "Codex").
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

## Codex

### What a channel stores

Nothing but its registry line. The native id is the task's UUID, the pipe is fixed
(`\\.\pipe\codex-ipc`), and there is no credential and no live record. A `detail` never names the
task's id: where the desktop's own text holds it, it is replaced by `<task>`.

### One send

Every frame is a 4-byte little-endian length, then JSON: at most 64 MiB written, 256 MiB read (a
snapshot carries the task's history). The whole send runs to the send's deadline in the calling
thread; `initialize` takes at most 5 seconds of it.

| Step | Result when it fails |
|---|---|
| The id is a non-nil UUID, hyphenated | `Failed`, `channel_unknown` |
| Not stopped, not out of time | `Deferred`, `target_unresponsive` |
| The message is not blank and at most 1 MiB | `Failed`, `write_failed` |
| The pipe opens | errors 2, 3 and 231: `Deferred`, `ipc_unavailable`; another error: `Failed`, `write_failed` |
| `initialize`, `clientType` `actingcommand-runtime`, gives this client's id | an error answer: `Failed`, `rejected`; no id or another shape: `Failed`, `protocol_changed` |
| `thread-owner-discovery` names the task's owner | `no-client-found`: `Deferred`, `target_not_loaded`; another error answer: `Failed`, `rejected` |
| A subscription (`thread-stream-following-changed`) gives the owner's snapshot, then ends | a state stream version other than 11; a resume state, runtime status or turn status this build does not know; no absolute working directory: `Failed`, `protocol_changed`. `resuming`, `needs_resume`: `Deferred`, `target_not_loaded` |
| The task waits for no approval and no input | `Deferred`, `target_awaiting_input` |
| Still not stopped, not out of time | `Deferred`, `target_unresponsive` |
| `before_write` commits the intent | `Failed`, `write_failed`: nothing was submitted |
| One submission to the owner: `thread-follower-start-turn` (version 2) for an idle task, `thread-follower-steer-turn` (version 1) while a turn runs | see below |
| | `Delivered`, `accepted` |

- **Before the intent,** the deadline or a stop at any step is `Deferred`, `target_unresponsive`, and
  a pipe that breaks or closes is `Deferred`, `ipc_unavailable`. Nothing was submitted, so the row
  waits and is tried again.
- **After the intent:**
  - an answer with a turn id: `Delivered`, `accepted`;
  - a definite error answer: `Failed`, `rejected`;
  - no answer by the deadline: `Unknown`, `write_timed_out`;
  - the pipe failed or the Runtime stopped while waiting: `Unknown`, `write_failed`;
  - an answer of another shape, or with no turn id: `Unknown`, `protocol_changed`.

  None of them is tried again.
- **The one fallback.** A steer refused with exactly `Cannot steer conversation <id> because its
  active turn already ended` reads the task once more. If the same owner shows it idle in the same
  directory, one start follows with the same message id; otherwise the row is `Failed`, `rejected`.
  The start's answer ends the send by the list above. Nothing is tried a third time.
- **Discovery questions.** The desktop asks connected clients `client-discovery-request` at any time;
  this client answers `canHandle: false` every time, at every step.
- **Ids.** `clientUserMessageId` (and a steer's `restoreMessage.id`) is the wake's 16 bytes as a
  UUID of version 8 with the variant bits set: the same on every try and in the fallback. Request
  ids come from it and the time, in the same form.
- **No rollout files.** The adapted source confirms an idle task from the task's rollout files under
  the client's home before its own waits. This transport never reads the client's home: a message
  does not need them, and the fallback's idle is the snapshot's (no turn in progress, nothing
  waiting).
- **A task that is not loaded is not opened.** Its message waits as `target_not_loaded`.

### The accepted race

The adapted source's own sender and the Runtime hold no shared lock per task. If both start a turn at
once, the loser gets a definite refusal, or, for a steer whose turn just ended, takes the one
fallback. That is a rare loud `failed`, never a duplicate: nothing is resent after the intent.

### What `accepted` does not show

The owner returned a turn id: it took the message. That does not show the turn finished or acted.

## What this costs

The Runtime now depends on private client protocols that can change.

| Client | What can change | What happens | Loud? |
|---|---|---|---|
| Claude Code | The `cc-msg-` shape, the variables | Refused at the hook or the preflight | Yes: a hook error in her session; `failed` or `expired`; `attention` |
| Claude Code | **The second frame line; the token's rules; an inbound `hold` or `refuse`** | The write succeeds and nothing arrives | **No. This is the one silent case:** the row says `delivered / written`. Only a person's eyes on a test wake show it. **Run a test wake after each Claude Code update** |
| Claude Code | A resume or `/clear` gives the session a new id | The channel goes offline | Late: `expired` at give-up; she updates the channel |
| Codex | Method names, the frame, the state stream version, the snapshot's shape | The version check and the strict parse refuse | Yes: `failed / protocol_changed` |
| Codex | `clientType` refused; a version 8 id refused | A refusal | Yes: `failed / rejected` |
| Codex | The ended-turn text | The fallback no longer applies: a steer whose turn just ended fails | Yes: `failed / rejected` |
| Codex | The pipe's name | The pipe is never found | Late: `expired` at give-up, reason `ipc_unavailable` |

**What `written` does not show.** A finished write proves only the write. A `hold`, a `refuse`, a
held message and an invalid token all look the same from here.

**Where each fact comes from.** Claude Code: the pipe, the token, the auth line and its requirement
on native Windows are on its public pages. **The second line's shape is not:** it comes from the
adapted source, which cites a third-party repository. Whether a raw frame gets the "from another
session" framing has not been checked on a live session. Codex: the desktop's IPC has no public
page; every method, member and text here comes from the adapted source. Whether the desktop takes
`clientType` `actingcommand-runtime` and a version 8 id has not been checked on a live desktop.

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
- **A Codex task sees the person's input.** The message arrives as a turn's input, as if typed.
  Nothing structural marks it as a program notice: the fixed first line and the skill's section are
  all there is, and the body says it authorizes nothing.
- **A Codex send reads the task's snapshot,** which holds its history: memory for the length of one
  send, at most 256 MiB a frame.
- **`unsafe`:** three modules (`pipe`, `dpapi`, `ancestor`), each new to review. The crate denies it
  everywhere else, and `actingctl` stays `forbid(unsafe_code)`. The Codex transport adds the pipe's
  overlapped read to `pipe` and no `unsafe` of its own.

**#419 holds.** A client protocol failure is a value of `ForwardOutcome`. A hung client costs at most
one deadline, in the calling thread. The hook is another process, and its failure leaves the Runtime
unaffected.

## Licence

The method and parts of the code come from `HS7097/codex-goal-lifecycle` at `c9538cc5`, MIT, copyright
2026 HS7097: Claude Code's from `bridge/claude.rs` and `bridge/security.rs`, Codex's from `desktop.rs`
and `relay.rs`. `crates/agent-forward/LICENSE-codex-goal-lifecycle.txt` holds the text verbatim, each
adapted file says so at its top, `NOTICE.md` lists it, and `actingctl agent-channel notice` prints it,
since binaries carry the code.
