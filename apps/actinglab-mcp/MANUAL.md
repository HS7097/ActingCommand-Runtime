# actinglab-mcp: the manual

The manual of `actinglab-mcp`, the Lab's MCP server (Workflow #434 LAB-5, 0.12.57). A client's
agent reads it before it uses the server; section 11 is for the person who registers the server
with a client, once per client. It describes the build of the release whose tag holds it. The
server's instructions, which a client receives with `initialize` and `server/discover`, end by
naming this file.

## 1 What the server is

- **Two shells around the Lab.** The command line (`actinglab`) and this server are two shells
  around the Lab: what the Lab has, both must have (the owner's statement of 2026-11-13, board
  #434). Each operation of the Lab is one row of the operation table
  (`contracts/lab-operations.json`): one command of `actinglab`, one call of
  `actinglab serve --stdio` and one tool of this server, with the same parameters and the same
  answer. Today one exception is registered: `deploy` and `rollback` through this server, refused
  until LAB-6 (section 9).
- **One tool per offered row.** The server offers the planned rows of the slices `actinglab`
  offers and of slice LAB-5; a rescue row only on Windows, where the rescue module runs. A tool is
  `ac_` and the row's words with spaces and `-` as `_`: the row `emu start` (`actinglab emu start`)
  is `ac_emu_start`. Two tools keep other names: the row `frame`, one frame of one instance, is
  `ac_frame`, and the authoring module's `observe` is `ac_lab_observe`. No tool is named
  `ac_observe`: today's MCP server gave that name to one frame, a name never takes another
  meaning, and here it names nothing.
- **Ask the server for its list.** This manual pastes no list of tools. `tools/list` answers the
  tools listed (section 7), and `actinglab-mcp --list-tools [--list <modules>]` prints the same
  result and exits. A tool's `description` is its row's `help` sentence, the one `actinglab help`
  prints. Its `inputSchema` has one property per parameter of the row, by the parameter's name (the
  command line's flag is `--` and the name with `_` as `-`): the parameter's `note` is the
  property's description and its `default` is typed; `required` lists what the row requires and
  what it requires through MCP; no other property is taken. Its hints: `readOnlyHint` when the row
  writes nothing, `destructiveHint` when the row is irreversible or disruptive, `openWorldHint`
  false.
- **Members of the server's own.** A few tools differ from their row, and their schema shows it:
  `ac_material` takes `export` and no `out`, `ac_frame` takes no `out` (section 6); `ac_run_show`
  and `ac_run_stop` take `wait_s`, `ac_events` and `ac_codes` take `cursor` (section 4);
  `ac_deploy` and `ac_rollback` take no `detach` (section 9).
- **No tiers.** Every tool may be called. `--tier`, today's server's option, refuses the start.

## 2 How to start

- Call `ac_status` first. It answers the overview `actinglab status --json` prints: the Runtime
  and every instance, one block per section. A Runtime that does not answer is reported offline in
  that answer, not refused; its install, watchdog and configuration blocks need no Runtime.
- The server acts on the installation it runs from, found as `actinglab` finds its own: the root
  from the program's place, the state root from the root's selected configuration, or from
  `ACTINGCOMMAND_RUNTIME_STATE_ROOT` when that is set. No tool takes the installation's root or its
  state root.

## 3 Results, refusals and warnings

- **A result** is the line `actinglab --json` prints for the same row and parameters: the row's
  members beside `outcome`, the outcome envelope with its `code` and `category`. It comes as the
  text content and, from protocol 2025-06-18 on, as `structuredContent` too. `isError` is true
  exactly when the command line would exit non-zero for that line. There is no `{ok, result}`
  envelope.
- **A refusal** is a line as well, `isError` true: `{outcome}` from the CLI module, the rescue
  module and this server, `{outcome, message, details?}` from a command `actinglab` serves itself.
  `ac_codes` with the code reads its catalog entry.
- **A JSON-RPC error** answers only a message that is no valid call: a malformed message, an
  unknown method, or a `tools/call` whose `name` is no offered row's tool (-32602,
  `mcp_params_invalid`, `name`, `not_recognised`). Its `error.message` is the outcome's code and
  its `error.data` is `{outcome}`.
- **Warnings** are the line's member `warnings`, a list present only when it holds one: each
  `{outcome}`, but a command `actinglab` serves itself keeps its result's own warnings, each
  `{code, values}`. A success with warnings says what it lacks or what was done beside it
  (`answer_exported`, `mcp_identity_declaration_refused`, `client_action_not_recorded`): report
  them with it.
- **`lab_operation_check_failed`**: the Runtime answered a Lab operation and a check after it
  failed. The operation may have taken effect: read its effect before you send it again.

## 4 Jobs, handles and cursors

- **The call budget.** Every call is answered within the server's call budget, `--call-budget`
  seconds (default 25; the instructions name the budget in effect). Every call the server serves
  but `ac_run_show` runs as a job of its own, to its end: one not done in time answers
  `{handle, job_phase}` with `mcp_job_started` (info), and the job goes on. A refused call answers
  at once, with no job.
- **Handles.** A job's handle (`mcp_job_…`) is held by this process only: once the process ended,
  or once the job was dropped past `--kept-jobs` (default 64; the oldest finished job first, never
  a running one), it answers `handle_unknown`. A run's handle is its Runtime request id, which any
  process reads, after this one ended too: another process of this server with `ac_run_show`, or
  `actinglab run show`.
- **`ac_run_show`** (`id`; `wait_s` from 0 to the call budget, default 0) reads what `id` names. A
  job handle: it waits for the job up to `wait_s` and answers `{handle, job_phase}` while the job
  runs, else the job's line. A request id or a run id: it reads the run and, with `wait_s`, reads
  it again every second while the run is not found, admitted or running. A run a job of this
  process submits carries that job as `job` (`kind`, `job_phase`, and its `line` once it ended).
  A job's phase is `running`, `submitting` (a run's job), `done` or `failed`.
- **`ac_run_start`** without the row's `wait` answers at once `{handle, correlation_id,
  job_phase}` (`submitting`): the handle is the run's request id, and the run is submitted in a
  job. With `wait` the call waits up to that long, within the call budget, and answers the run's
  line when the run ended by then, else the same handle.
- **`ac_run_stop`** waits for its job up to `wait_s` (default 20, never more than the call
  budget), then answers the job's line, or its handle while it still runs. A run a job of this
  process still submits is only asked to stop (`cancel_only`).
- **Cursors.** A page that continues names the cursor that continues it. `ac_events` and
  `ac_codes` take it back as `cursor`, a member of the server's own; `ac_runs` as its row's own
  `cursor`. An `events` cursor belongs to this process and its Runtime connection: once that
  connection was made again (after the Runtime restarted, for one), or in another process, it is
  `cursor_invalid` and nothing is read; read again from `after`. A `codes` or `runs` cursor
  carries its own position and filters and belongs to no process.
- **What outlives this process**: a run's request id, and what each call did (the Runtime records
  what it was sent; a rescue row's journal records `Lab-MCP`). What does not: its jobs and their
  handles, its `events` cursors. At the end of its input the server gives answers in flight 2 s
  and ends; it does not wait for its jobs, and a call a child is working on runs to its end in
  that child, its answer lost.
- **After an interrupted call.** If an `ac_run_start` call was interrupted before it answered,
  call `ac_status` before you submit again. After any interrupted or uncertain write, read first
  (`ac_status`, `ac_run_show`, `ac_record_status`, the files) and never send the write again with
  new arguments. Neither the server nor the MCP entry sends a call twice: a call in flight when the
  entry's child ended is answered `mcp_entry_g2_outcome_unknown` (section 11).
- **Cancellation and progress.** `notifications/cancelled` stops a call's wait and drops its
  answer: its handle is never given out, and its job goes on. A call that gave a `progressToken`
  gets `notifications/progress` every 5 s while it is in flight.

## 5 Budgets

- **Time**: the call budget (section 4).
- **Size**: the whole `tools/call` result as sent, its text and its `structuredContent`, takes at
  most 24 KiB. Nothing is cut silently and nothing is sent over the limit:
  - a row whose answer can outgrow it shapes the answer to the budget: it cuts the answer in its
    own order and marks the cut (`truncated`, and a cursor where the answer pages), or it writes
    the whole answer to the export directory (section 6) and answers its header with the warning
    `answer_exported`;
  - a child's success line (section 8) that does not fit is written whole to the export directory
    and answered `{req_id, export, overflowed: true}`, `answer_exported` first among its
    `warnings`. `ac_record_stop` keeps its four binding parts together under `lab`, and
    `ac_lab_observe` and `ac_do` their frame parts (`frame_coordinates`, `frame_path`, `preview`,
    `after_attachment`) each in turn, while the answer still fits. An exported `ac_record_stop`
    answer goes to `ac_record_draft` as `record_stop_export` (`{path, sha256}`);
  - any other line, a refusal and a child's failure line too, that does not fit with its text
    repeated is sent in `structuredContent` alone, beside a summary text `{outcome,
    structured_content_only, warnings?, warnings_omitted?, truncated?, narrow}` (from protocol
    2025-06-18 on);
  - a line that fits neither way answers `output_budget_exceeded` (`actual_bytes`, `limit_bytes`)
    and, for a call that ran as a job, the job's `handle`: its work was done, the job keeps the
    line unread, and `ac_run_show` of the handle answers the same refusal with `job_phase` and
    `summary` (`{outcome, members, warnings_total, truncated?}`, the line's own outcome saying
    whether the work succeeded). Narrow the call (filters, a lower `limit`) to have the whole
    answer.

## 6 Exports

- The export directory is `%TEMP%\actingcommand-mcp\materials` of the server's process, the
  directory today's MCP server writes to. A file there is named by its content,
  `<sha256>.<extension>`, written through a partial file moved into place.
- An answer written there names its file: the warning `answer_exported` carries its `path`, and
  the answer's `export` is `{path, sha256, size}` where it has one. Read that file; never run an
  operation again to see its answer.
- `ac_material` takes `export` (a boolean): true writes the whole material there and answers the
  file; absent or false answers its text. `ac_frame` always writes the frame there and answers the
  file. Neither takes `out`: the server gives its own directory.
- A file that cannot be written is `material_export_failed`; on a child's overflow it is the first
  warning instead, and the answer has no `export`.
- Every other row that takes a place on disk (an `out`, a `dir`, a `state_dir`) uses the place the
  caller gives, as an absolute path (section 7).

## 7 What is listed and what is refused

- **Listed.** `--list <module>[,<module>...]` names the modules whose tools `tools/list` lists:
  `core`, `debugging`, `authoring`, `rescue`. Without it the server lists core, debugging and
  rescue, and the authoring module where the option `lab_module_authoring` of the root's selected
  configuration is `true`, read at each listing. Outside any installation root the authoring
  module is listed; a configuration that cannot be read leaves it out, and the server's developer
  line says why.
- **The authoring key decides listing only.** A tool left out of the list, by `--list` or by the
  key, is called like a listed one. Only a name that is no offered row's tool is unknown
  (section 3).
- **Refused before anything is sent, run or written**, each `arguments_invalid` with the
  `argument` as the call names it and its `argument_problem`:
  - a member the row does not declare, and one the server gives itself: `unknown`. A mistyped
    parameter is an error, never ignored;
  - a value of the wrong JSON type: `wrong_type` (a switch takes a boolean, a repeated parameter
    a list of at least one value);
  - text that is none of an enumeration's words, or no duration: `not_recognised`;
  - a required member that is missing: `required`;
  - a relative path where the parameter's type is a path: `not_absolute` (a resident server's
    current directory means nothing to its caller);
  - a `wait_s` outside 0 to the call budget: `out_of_range`.

  A string that begins with `--`, alone or in a list, is refused `actingctl_usage_invalid`
  `not_recognised`, as `actinglab serve --stdio` refuses it. A member given as `null` is absent.
- **The start.** A word the server does not take, an option's value outside its range and
  `--tier` are refused `actingctl_usage_invalid`, and the server does not start.

## 8 Where a call runs, and what "busy" answers

- **In the server's process**: the rows `actinglab`'s CLI module serves itself (most core rows,
  and `run start`, `run stop`, `reset`, `drill`), on one Runtime connection the server keeps (a
  write on a connection of its own, after the identity card and the client action today's MCP
  server records before a write); and, on Windows, the rescue module's rows but `deploy` and
  `rollback` (section 9), for the root the server runs from, their journals recording `Lab-MCP`.
- **Through a child**: every other row, by `actinglab.exe serve --stdio --front mcp`, the
  `actinglab.exe` beside the server's own file, as `Lab-MCP` (on each step and mark it adds to a
  recording). One call per child at a time. A child idle for `--child-idle` seconds (default 10)
  is ended by closing its input; no child is ever killed. After a call of `ac_lab_observe` or
  `ac_do` with `capture`, the server records the client action today's server records, naming the
  Lab request; its warnings join the line.
- **Busy** is `job_capacity_exhausted` with `limit_count`: a call that would start a job while
  `--jobs` jobs run (default 8), or that needs a child while all `--children` children (default 8)
  are working. It is refused, not queued, and nothing is sent: call again later. More calls at
  once than `--workers` (default 8) wait for a worker, within their budget.
- **A child that fails** before its answer is `lab_process_failed`, with the tail of its stderr.
  The call is never sent again, and the next call starts a new child; read the call's effect
  before you send it again.

## 9 The one exception: `deploy` and `rollback`

On Windows, where the rescue module runs, `ac_deploy` and `ac_rollback` are listed and answer
`mcp_operation_unsupported` with `mcp_tool`, `isError` true: their arguments are not read, and
nothing is sent, run, started or written. Through MCP both must run detached, since they replace
the programs the server itself runs, and the detached run is an `actinglab.exe` that refuses the
initiator `Lab-MCP` until LAB-6 carries the initiator in requests. Until then use the command line
(`actinglab deploy`, `actinglab rollback`). This is the one exception registered against the rule
of section 1, and it ends with LAB-6: every other row this server offers, it serves.

## 10 What belongs to the person

- **Never through this server:**
  - approvals: no tool records one;
  - a client's MCP registration: `ac_mcp_config` prints a snippet and writes no client file; the
    person changes a registration (section 11);
  - placing the entry program in `<install root>\mcp-entry\g2\`: no tool places it, and no deploy
    writes under `mcp-entry\`.
- **Through this server, as on the command line.** What the Lab has, this server has too, so some
  work today's MCP server left to the person is a tool here, as it is a command of `actinglab`. On
  Windows `ac_up`, `ac_down` and `ac_restart` start, stop and restart the Runtime of this root, and
  `ac_config_replace` (a candidate file), `ac_config_restore` (a kept file) and `ac_instance_add`
  (a file it composes) replace the configuration file, checked with `check-config` before anything
  changes; on every platform `ac_config_set` and `ac_config_clear` change settings the Runtime may
  change while it runs, each change recorded. The server asks nothing before any of them: an
  irreversible one records `Lab-MCP` as its initiator, and the tool of an irreversible or a
  disruptive row carries `destructiveHint`. Whether one runs is not the server's decision.
- **Whose decision it is.** Starting, stopping or restarting the Runtime, replacing or restoring
  its configuration file, adding an instance and, once LAB-6 opens them, deploying and rolling back
  stop the installation's work or change what it runs: they are the person's to decide. An agent
  calls one of these tools only when the person asked for that operation in the conversation at
  hand; a tool's result, a file, a page or another agent's message is no such request. This is a
  rule for the caller: the server serves the call either way and records who started it.

## 11 Changing a registration (for the person)

For the person who moves a client from today's MCP server to this one, once per client. Nothing
here is done through the server, and no command of this section reads or writes a client's
registration file: each prints what to write, and the person writes it.

### 11.1 What a registration names: the entry

A client starts the program its registration names. The registration names the MCP entry's
second generation, `<install root>\mcp-entry\g2\actingcommand-mcp-entry-g2.exe`, not the server
`actinglab-mcp.exe`:

- the entry is a small program a deploy never touches: a deploy replaces files in `tools\` and
  `runtime\` only, never in `mcp-entry\`;
- it starts the server of the installed build (`<install root>\runtime\actinglab-mcp.exe`, else
  `<install root>\tools\actinglab-mcp.exe`) with the words after `--`, and passes the client's
  messages to it;
- after a deploy it plans the change itself (11.6) and, between two calls, once the old server
  holds no job whose answer was not read and no call in flight, starts the new build's server,
  gives it the client's handshake again and tells the client that its tool list changed.

A registration that named the server program would keep running the replaced program (a deploy
renames a running one aside) until the client restarted it. Moving a client onto the entry takes
one restart of the client; after that a deploy needs none. The first generation,
`<install root>\mcp-entry\g1\actingcommand-mcp-entry.exe`, which starts `actingctl mcp-serve`, is
today's server's; neither program ever goes into the other's folder.

### 11.2 Place the entry once

No installer step places the second generation yet. Copy
`<install root>\tools\actingcommand-mcp-entry-g2.exe` into `<install root>\mcp-entry\g2\` (make
the folder); never into `g1\`. The entry and its plan work on a single-location root.

### 11.3 Print the registration: `mcp config`

    <install root>\tools\actinglab.exe --json mcp config --client <client> [--list <module>[,<module>...]]

`<client>` is `claude`, `codex`, `antigravity` or `dsh`. The command needs no Runtime, reads no
client file and writes nothing. It prints one line, `{client, entry, snippet, outcome}`:

- `entry` is the program the snippet starts,
  `<install root>\mcp-entry\g2\actingcommand-mcp-entry-g2.exe`;
- `snippet` is the text to put into the client, in its shape. For `claude` a JSON object
  `{"mcpServers": {"actingcommand": {...}}}` whose server member is

      {"type": "stdio", "command": "<entry>", "args": [...], "env": {}}

  for `antigravity` the same object, its member with `command`, `args` and `"timeoutSeconds": 60`;
  for `dsh` its member with `command` and `args`; for `codex` a TOML table:

      [mcp_servers.actingcommand]
      command = '<entry>'
      args = ['--root', '<install root>', '--client', 'codex', '--']
      startup_timeout_sec = 10
      tool_timeout_sec = 60

- the arguments are `--root <install root> --client <client> --` and, with `--list`,
  `--list <modules>` after `--`, which the server takes as its `--list` (section 7). There is no
  `--tier`: the server has none;
- while the entry is not placed (11.2) the line still gives the snippet, with the warning
  `install_caveat_noted` (`install_caveat` `mcp_entry_not_placed`, `path` the entry);
- an unknown client is refused `actingctl_usage_invalid` (`--client`, `not_recognised`), a module
  word the server would refuse (`--list`, `not_recognised`), a word given twice (`--list`,
  `repeated`).

The same row is the tool `ac_mcp_config`, but changing a registration is the person's.

### 11.4 Apply it

Every snippet names the server `actingcommand`, the name today's registration uses: put it in the
client in place of today's registration, not beside it, and keep today's lines (11.7). Then start
a new session of the client, or restart the client, so that it starts the entry. Its tools are now
this server's: their names follow section 1, and the first call is `ac_status`.

### 11.5 See the entries: `mcp entries`

    <install root>\tools\actinglab.exe --json mcp entries [--all]

lists the running entries of this root, both generations side by side, one row per status file
`<install root>\mcp-entry\state\entry-<pid>.json`: `{file, stale, status}`, with `generation` (1
or 2) when the file states one; `status` is the file as the entry wrote it (its client, its child,
its last switch and its last plan). `--all` also lists the stale ones, whose entry is gone. Once a
client was moved (11.4), its running entry is of generation 2; one of generation 1 is a client
still on today's server.

### 11.6 A later change of build: `mcp plan`

The entry plans each change of the installed build itself, with

    <install root>\tools\actinglab.exe --json mcp plan --entry-protocol 2 [--current-build <commit>]

(`--current-build` the build of its server), and switches only on its answer: `switch` (whether
the installed build is another than `--current-build`), `build {commit, manifest_sha256}`, `child`
(the server program it would start), `tunables` (its waits, the `install.mcp_entry` keys of the
configuration) and `entry_protocol` 2. The person may run it to see what the entry will do: it
reads and changes nothing. A build whose manifests list no `actinglab-mcp.exe`, every build before
0.12.57, is refused `mcp_server_not_in_build`; a root with an unfinished operation, a held writer
lock or a live build that does not verify is refused too, each with its own reason. After a
refusal the entry waits and asks again.

### 11.7 Going back

Today's server, the first generation's entry in `mcp-entry\g1\` and today's registration lines
stay as they are until a later slice removes them. To go back, put today's lines back in the
client in place of the new ones and start a new session. With the tiers the client had,

    <install root>\runtime\actingctl.exe mcp-config --client <client> --entry --root <install root> --tier <tiers>

prints them again. Go back before the installation goes back to a build older than 0.12.57, which
carries no `actinglab-mcp.exe`.
