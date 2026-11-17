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
  answer. No exception is registered: since LAB-6S0 (0.12.58) the server serves `deploy` and
  `rollback` too, on Windows, always detached (section 9).
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
  `ac_material` takes `export` and no `out`, `ac_frame` and `ac_report_export` take no `out`
  (section 6); `ac_run_show`
  and `ac_run_stop` take `wait_s`, `ac_events` and `ac_codes` take `cursor` (section 4);
  `ac_deploy` and `ac_rollback` take no `detach` (section 9); `ac_devices_bind` takes no
  `bound_via` (#398 DB-1, 0.12.71: the channel `ui` is the installer's; a binding made through this
  server records `cli`, as the row writes it without one), and takes `config`, a candidate
  configuration whose state root the binding goes into (fix round 1: then the install root's adb is
  that of the root this server's program runs from by its place, never read from an install
  selection); `ac_devices` and `ac_devices_bind` take neither `key_salt` nor `key_salt_env`
  (#398 DB-2, 0.12.72: a salt in a tool call stays in the client's transcript, and a variable of
  this server's process is no caller's to name; through this server a row's `device_key` is made
  with the installation's own secret, and `ac_devices_bind` takes `device_key` in place of
  `handle`); `ac_devices_bind` takes no `root` (#398 DB-2 fix round 1: the row's parameter for
  the command line's own `--root`; the server hands on the root it runs from). These are the only
  parameters of a row a tool leaves out; the command line also takes `actinglab`'s global
  options and a positional given as its flag (`--id`, `--instance`), which no tool needs. No tool takes `words` either (Workflow #434 LW-1):
  the further words a row handed on before Workflow #434 LAB-5B4, which no row declares and a row
  served in the server's own process refuses by name.
- **Closed word lists** (Workflow #434 LW-1, 0.12.67). Where an operation takes only the words of
  a closed list for a parameter the table types as text, the schema lists them (`enum`; for a
  list its items' `enum`): `ac_status`'s `sections` (the overview's blocks; `daemon` and
  `incomplete` are always answered and are no section) and `ac_events`' `type`, `event_type`,
  `exclude_event_types` (the ledger's event types) and `origin_module`. Another word is refused
  before anything is sent (section 7).
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
  (`answer_exported`, `client_action_not_recorded`; `mcp_identity_declaration_refused` until
  Runtime 0.12.63, which retires it with the identity card): report them with it.
- **`lab_operation_check_failed`**: the Runtime answered a Lab operation and a check after it
  failed. The operation may have taken effect: read its effect before you send it again.
- **Coordinates** (Workflow #392 DA-5ab, Runtime 0.12.67; `contracts/display-adapter.md`
  section 11): `ac_lab_observe` and `ac_do` answer what the command line answers, in the package's
  base. On a display of the base's size their `frame_coordinates` is `{device_pixels,
  orientation, applies_to}`, the device's own pixels. On a display that maps to the base (1920×1080
  or 3840×2160 for a 1280×720 package) it is `{space: "base", base_pixels, reduced_from,
  orientation, applies_to}`: every frame was reduced from the device size `reduced_from` to the
  base, every rectangle and point of the answer is the base's, `ac_do` takes `tap`, `tap_rect`,
  `swipe` and an element in the base, and its `frame_path` and `preview` are the reduced frames.
  A display the base does not map to (another ratio, a smaller one) is refused
  `display_map_unresolved`; a press at a ratio the input side does not send under (5/4) is refused
  `display_map_unresolved` (`input_scale_unsupported`), nothing sent. `ac_lab_observe` and `ac_do`
  with `record` on such a display are refused before anything is sent or written (the train of
  DA-5ab and DA-5cd; `contracts/lab-recording.md`, "A recording's size"):
  `record_display_not_base` when the recording has no frame yet, `record_frame_size_mismatch` when
  the display changed during the recording, `record_size_not_base` for a recording not at the base
  size; `ac_do` with `record` captures one frame before its press for this. A saved scene of such a
  size answers the same base form (below, "Offline scenes of another size"). `ac_frame` is one
  frame of the instance as the device gives it, of the device's size (3840×2160 on such a
  display), and the device commands (`ac_tap`, `ac_swipe`, `ac_long_tap`) take the device's own
  pixels.

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
- **A run's line** (Workflow #434 LW-1, 0.12.67). The line `run start` gives once its run ended
  holds every ledger record of the run (`events`, with payloads: 100 KB to 500 KB for a run that
  went far). This server never sends them: the job's `line` that `ac_run_show` carries and the
  answer of `ac_run_start` with `wait` are that line without `events`, with `events_total` (how
  many were left out), `truncated` (what was left out, as JSON pointers: `/events`, and
  `/official_ocr_projection` or `/official_ocr_fields_projection` when the run's OCR reads did
  not fit beside the rest) and `rest`, the calls that read it: `ac_run_evidence` with the run's
  request id (its steps, page by page) and `ac_events` with its correlation id and `detail`
  `payload` (its records). The receipt (state, outcome, `executed_steps`, `final_page`) and the
  line's outcome stay. So the answer of a finished run no longer grows with its steps or its
  records. What stays is never cut: the receipt and the line's `warnings` (and, with `ac_run_show`,
  the run's own line beside the job); a run whose answer still does not fit with them is refused
  `output_budget_exceeded` with its job's handle (section 5), never cut silently.
- **`ac_run_start`** without the row's `wait` answers at once `{handle, correlation_id,
  job_phase}` (`submitting`): the handle is the run's request id, and the run is submitted in a
  job. With `wait` the call waits up to that long, within the call budget, and answers the run's
  line (as above) when the run ended by then, else the same handle. #463 S3-9 (0.12.68;
  `contracts/runtime-interface.md`, "The variable values of a contained run"): `variable`, a list
  of `<name>=<json>` (one per variable, as `run start --variable` takes them), gives the run values
  for variables its package declares; a name given twice is `repeated` and a right side that is no
  JSON `not_recognised`, before anything is sent. The Runtime judges each value as a dispatch does
  and refuses one the package does not take (`run_variable_undeclared`, `run_variable_not_applying`,
  `run_variable_value_refused`, nothing run); a variable not given reads its default. A run with
  values needs a Runtime of `runtime-client` revision 11: one below it ends the connection and
  nothing runs. `ac_standard_pack_trial`'s run sends the `variable` its plan shows the same way.
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
  - a run's line, wherever this server gives it (`ac_run_start` with `wait`, the job of
    `ac_run_show`), leaves the run's ledger records out and says so (section 4, "A run's line");
  - a child's success line (section 8) that does not fit is written whole to the export directory
    and answered `{req_id, export, overflowed: true}`, `answer_exported` first among its
    `warnings`. `ac_record_stop` keeps its four binding parts together under `lab`, and
    `ac_lab_observe` and `ac_do` their frame parts (`frame_coordinates`, `frame_path`, `preview`,
    `next_frame`, `after_attachment`) each in turn, while the answer still fits. An exported `ac_record_stop`
    answer goes to `ac_record_draft` as `record_stop_export` (`{path, sha256}`). #463 RC-1
    (0.12.69): `ac_resource_regression` keeps `counts` the same way (its refusal lists at most 10
    rows and fits);
  - a rescue tool's success line (served in the server's own process, on Windows; section 8)
    that the next point could send neither way (Workflow #434 PP-1, 0.12.72) is written whole to
    the export directory and answered its header: the parts its tool keeps, `truncated`,
    `export`, `warnings` (`answer_exported` first, the line's own after it) and the line's
    `outcome`. `ac_place` (with `plan`, and a placement) keeps the plan's `schedule_changes` under
    `plan`, as the line holds it, and its `next` (what a placement with no Runtime running tells
    you to do next: `composition_at_start`, or `composition_at_start_once_bound` when the
    configuration binds no instance to the game); `ac_undo` and `ac_op_resume`
    their `reverses` (`ac_undo` with `plan`) and their `schedule_changes`, where the answer
    carries them; `ac_op_status` its `unfinished`, `last_operations` and, with `op`, `journal`;
    every other rescue tool, `ac_deploy` with `plan` among them, its outcome alone (with `plan`:
    `plan_id`, `commit_sha`). A part is kept whole while the header still fits; else
    `schedule_changes` without its rows `packages`, which `packages_total` counts, its four lists
    `enters`, `leaves`, `content_only` and `unread` kept; else every list counted the same way
    (`enters_total` and the others); a list (`reverses`, `last_operations`) is counted so too;
    else the part is left out. `truncated` names, as JSON pointers into the file, every member of
    the answer the header does not hold as the file does: each member left out (`/steps`, and
    beside `schedule_changes` the plan's other members, `/plan/items` among them) and what a
    reduced part left (`/plan/schedule_changes/packages`). The answer's own `warnings` and
    `outcome` are not named there: every header carries them (its `warnings` begin with
    `answer_exported`, or `material_export_failed`). Nothing is in the file that the header
    neither holds nor names there. Read `schedule_changes` before you place: in the header, or in
    the file where `truncated` names its lists. The call's job keeps the header, so `ac_run_show`
    of its handle answers it with its `export`. A refusal is never exported, and no line that the
    next point sends is;
  - any other line, a refusal and a child's failure line too, that does not fit with its text
    repeated is sent in `structuredContent` alone, beside a summary text `{outcome,
    structured_content_only, warnings?, warnings_omitted?, truncated?, narrow}` (from protocol
    2025-06-18 on);
  - a line that fits neither way, a rescue tool's header too, answers `output_budget_exceeded`
    (`actual_bytes`, `limit_bytes`) and, for a call that ran as a job, the job's `handle`: its
    work was done, the job keeps the line unread, and `ac_run_show` of the handle answers the
    same refusal with `job_phase` and `summary` (`{outcome, members, warnings_total,
    truncated?}`, the line's own outcome saying whether the work succeeded). Narrow the call
    (filters, a lower `limit`) to have the whole answer.

## 6 Exports

- The export directory is `%TEMP%\actingcommand-mcp\materials` of the server's process, the
  directory today's MCP server writes to. A file there is named by its content,
  `<sha256>.<extension>`, written through a partial file moved into place.
- An answer written there names its file: the warning `answer_exported` carries its `path`, and
  the answer's `export` is `{path, sha256, size}` where it has one. Read that file; never run an
  operation again to see its answer.
- `ac_material` takes `export` (a boolean): true writes the whole material there and answers the
  file; absent or false answers its text. `ac_frame` always writes the frame there and answers the
  file. `ac_report_export` (Workflow #467 RPT-1) always writes its problem report archive there,
  `problem-report-<YYYYMMDD-HHMMSS>Z-<8 hex>.zip`, and answers its `path`, `sha256` and `size`
  (`contracts/problem-report.md`). None of them takes `out`: the server gives its own directory.
- A file that cannot be written is `material_export_failed`; on a child's overflow and on a rescue
  tool's header (section 5; Workflow #434 PP-1) it is the first warning instead, and the answer
  has no `export`: the header's parts are all of the answer there is, and a header that does not
  fit even so answers `output_budget_exceeded` with its job's handle.
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
  write on a connection of its own, the write first after the handshake: since Runtime 0.12.63
  neither server sends the identity card or the client action before a write, the binding's
  `Lab-MCP` naming where it came from); and, on Windows, the rescue module's rows, for the root the
  server runs from, their journals recording `Lab-MCP` (`deploy` and `rollback` start a detached
  run of their own and answer at once, section 9). These requests keep the command line's
  envelope until slice LAB-6S2-E (`contracts/lab-two-entries.md` §2.4 and its slice table): so a
  run `ac_run_start` starts is listed with `origin` `cli` (`ac_runs`, `ac_run_show`), while its
  command record's channel is `mcp`; from that slice on it is `lab`.
- **Through a child**: every other row, by `actinglab.exe serve --stdio --front mcp`, the
  `actinglab.exe` beside the server's own file, as `Lab-MCP` (on each step and mark it adds to a
  recording). One call per child at a time. A child idle for `--child-idle` seconds (default 10)
  is ended by closing its input; no child is ever killed. After a call of `ac_lab_observe` or
  `ac_do` with `capture`, the server records the client action today's server records, naming the
  Lab request; its warnings join the line.
- **Presses** (Workflow #434 LW-1, 0.12.67). `ac_tap`, `ac_swipe` and `ac_long_tap` without
  `record` press on every instance. What one such press does: on every instance, one status read
  first (on a connection of its own), to learn whether the instance's input presses only on a
  frame the Runtime captured (its capability table names the input, `touch_backend` `nemu_ipc`,
  the emulator's own interface); on such an instance, one capture (an `ObserveReadonly`, of which
  the command takes the frame's reference alone) and the press sent on that frame, as a recorded
  press does, the answer naming the frame (`input_frame`); elsewhere nothing is captured. When the
  Runtime no longer holds that frame when the press names it (`input_frame_unavailable`: another
  capture took its place, nothing sent), the command captures once more and sends the press once
  more under the same lease, once (`input_frame_captured_twice`); a second refusal is the answer.
  A status read that fails stops no press: the press is sent without a frame, as before this
  slice (an input that needs one is refused by the Runtime itself, `nemu_input_frame_required`),
  and the answer's `touch_backend_warnings` says so (a refusal's is in its `details`, the
  Runtime's refusal of that press too). Such an instance sends the back key alone (Workflow #463
  KB-1, 0.12.69) and no text: `ac_key` with `back` takes the path of a press above (the status
  read, one capture, the key sent on that frame, the one retry, the warnings); `ac_key` with any
  other key and `ac_text` answer `nemu_input_capability_unsupported` there, nothing sent, and the
  Runtime has no other way to send another key on it; `adb_shell_input` sends the back key alone
  too and refuses another key before anything is sent, `input_key_unsupported` (KB-1 fix round 1,
  the key train). Where the instance's capability table says
  its input does not implement `input.key` or `input.text` (`minitouch` sends no key; no input but
  `maatouch` sends text), the refusal's `details.hint` says what to press instead (the screen's
  own control: `ac_tap`, or `ac_do` with `capture` on a package's element); where its input sends
  the back key alone, the hint of another key says that it sends only `back`, and the same.
  A `key back` is held by the foreground gate as a tap is: it cannot leave another application
  (`application_not_foreground`).
  `ac_tap_target` and
  `ac_navigate` only plan on a saved frame (`scene` with `dry_run`) and press nothing anywhere,
  as their command line does: `ac_do` with `capture` and the package's element, or `tap` with a
  point, presses a target on an instance, and `ac_ensure` with `capture` routes one to a page.
- **Recordings** (Workflow #449 REC-ARCH, 0.12.59; `contracts/lab-recording.md`, "Archiving"). A
  recording belongs to the instance: while one is active, every call with `record` on that
  instance, whoever sends it, adds a step to it, and its answer's `record` names the recording it
  entered (`record_id`, `record_started_at_unix_ms`) and the `step` it became. `ac_do` with
  `record` and `swipe` on a step without a click records that swipe as the step's. A recording kept
  only as material is ended with `ac_record_archive`: no marks are needed; it writes
  `archive-manifest-<n>.json` beside the frames and the step table (#463 S1-5, 0.12.64:
  `actingcommand.lab-recording-archive.v2`, whose `material` names each press with the frame it
  happened on and where it was pressed, and the final screen; `lab.warnings` holds
  `final_frame_missing` when it names none), keeps every file and ends the session (`archived`),
  and with `copy_to` (the absolute path of a directory that does not exist or is empty; a relative
  or empty one is `not_absolute`) copies the step table, the manifest and the frames `material`
  names, or with `all_frames` every frame the manifest lists, and checks every copy; the copy's
  manifest says which (`copy.frames`, `material` or `all`), a v1 manifest is copied as it is with
  every file it lists, and `all_frames` without `copy_to` is `validation_failed`
  (`copy_to_only`). With `record_id` it also archives a recording `record start --force` left
  behind, from its own step table, and leaves the session header, another recording's, as it is
  (`lab.session_header`).
  `ac_record_reopen` makes an archived recording active again, so that `ac_record_mark` and
  `ac_record_stop` can build its package later. #463 S2-1 (0.12.65; `contracts/lab-recording.md`,
  "What a recording still lacks"): `ac_record_ready` (`record_id`, `game`, `server`, `locale`,
  `state_dir`) lists at once
  everything a recording (the instance's, or the active, archived or stopped one `record_id`
  names) still lacks before `ac_record_stop` can make its package: every pre-check it fails, each
  with its word (`what`) and the code `ac_record_stop` would answer, or, once none fails, every
  failure of each self-check (#463 S2-1c, 0.12.68: `ac_record_stop` answers the first; `checks`
  says for each self-check that ran whether its list is complete, and why not); and its state,
  `material`, `draft`, `ready` or `packaged`. It only
  reads, under no lock, also while another call holds the recording; it opens no Runtime
  connection and neither pauses nor releases the instance. #463 S2-1b (0.12.67): it also takes
  `game`, `server` and `locale` as `ac_record_stop` takes them (one out of its form is refused as
  there, `validation_failed`); each part is the one given, else what the recording holds, else
  (game and server) the Lab's instance configuration, else (game and server) the instance's row of
  `policy.facts.instances` in the configuration of the installation the program runs from, which
  `ac_record_stop` reads too; that row gives both game and server or neither (when the others give
  one of the two, the row gives the other only if it holds the same value for the given one; the
  configuration is opened only when the others do not give both); `sources` names where each came
  from (`command_line`, `recording`, `lab_config`, `installation_config`, or `null` when not known,
  then it is missing). It writes none of them on the recording: give `ac_record_stop` the ones you
  gave it. On a packaged recording `sources` names what its package holds, and a given part with
  another value is the warning `record_stop_option_conflict` (`{code, field, recorded, given}`),
  which `ac_record_stop` would refuse. #463 L1 (0.12.60;
  `contracts/lab-recording.md`, "One call per step"): `ac_do` with `capture` and `record` records a
  whole step in one call:
  `mark_request_json` (or `mark_request`, a file) is the `record mark` request of the open step,
  applied and self-tested as `ac_record_mark` applies it before the press (a refusal is its own, and
  nothing is pressed or written; a request for anything but the open step, a step operation, a
  transition, another step or a frame file, is `validation_failed`: send it with `ac_record_mark`),
  and `next_frame_after_ms` (0 to 60000) takes the next frame after
  a press performed without a failure, as `capture --record` takes it, opening the next step
  (`next_frame_out` writes its PNG); `dry_run` with `record` is refused by the command. #463 S1-7
  (0.12.64; `contracts/lab-recording.md`, "A recording holds its instance"): a recording holds its
  instance, so no pause is asked of the agent: `ac_record_start` pauses the instance's scheduling
  with the recording's own reason `lab-record-<ms>` (an instance already paused by another party is
  held by that pause, which the recording never resumes; with nothing listening where the Runtime
  would, it starts holding nothing, `hold.warnings` `no_runtime_listening`; a Runtime that listens
  but does not complete the handshake fails the start, and so do a state root that cannot be
  resolved and a connection that times out). **`ac_record_start` waits until the
  instance's running task has ended**: up to the pause's drain (300 s), its checkpoint grace (30 s)
  and the IO timeout (5 s), 335 s in all; a caller with a shorter time limit must give it that time.
  Should the start end before the pause answers,
  the session header already names the hold and `ac_record_stop` releases it. `ac_record_archive`
  and `ac_record_stop` resume only the recording's own pause, found by its reason. A release that
  fails keeps the hold: the archive or the stop stands, and the answer is the failure with
  `hold.kept` and `next.release`, the call that tries the release again (`record stop` again, or
  `record archive` with `record_id`), never an unconditional resume. A press with `record` on an
  instance that is not held is made and recorded with `record.warnings` `instance_not_held`
  (`instance_hold_unread` when the status read got no answer); `ac_record_status` shows the hold.
  #463 S2-13 (0.12.65; `contracts/lab-recording.md`, "The frames around a press"):
  `ac_record_frames` (`step`, `before` 0 to 100, default 3, `after` 0 to 100, default 10,
  `record_id`) lists the stored frames captured just before the frame a step's press was sent on
  and just after the press, with their paths, capture times and offsets from the press; it reads
  only and takes no lock.
  #463 S2-2 (0.12.68; `contracts/lab-recording.md`, "Reviewing a recording step by step"):
  `ac_record_review` (`record_id`, `step`, `no_preview`, `game`, `server`, `locale`, `state_dir`)
  answers each step of the recording `ac_record_ready` reads as the screen before (`before`), what
  was pressed (`press`) and the screen after (`after`, with `still` and `waited_ms` where the step
  table holds them), the frames stored between, the step's marks and its items of
  `ac_record_ready` (`missing`, given `game`, `server` and `locale` as `ac_record_ready` takes
  them: a recording without a locale of its own runs no self-check until `locale` is given); a
  member the step table does not hold is absent. The train of #463 stage two (0.12.68): `missing`
  is every item of `ac_record_ready`'s one judgement that names the step, a pre-check's
  (`select_unrefined` among them) or, once no pre-check fails, each self-check failure; an item
  that names no step is no step's; `choice` is the step's choice mark on a step marked as holding a
  choice; a step dropped before the last is not listed, one `restore_step` gave back is. An
  application operation's `outcome` is its manifest row's: `indeterminate` with attempts and no
  completed receipt. Unless `no_preview`, it writes a preview of each screen under the recording's
  `lab/preview/` (`s<nn>-before.png`, `s<nn>-after.png`, their absolute paths in `preview`): a copy
  of the stored frame with the press and the step's boxes drawn (recognize boxes green, the press
  box orange, other marks blue; on the screen after the next step's). Nothing recorded changes and
  no stored frame carries a box; a frame whose bytes do not match its sha256 is
  `record_frame_hash_mismatch` and no preview is written; every call writes its previews anew and
  none is ever removed. A frame whose file is not there has `path` `null` and is named in the
  warning `frame_file_missing`, as `ac_record_frames` answers it. It takes no lock. `steps_total`
  counts every effective step: an answer over the budget is written whole to the export directory
  (section 5); `step` answers one step, so read a long recording step by step (each call evaluates
  the whole recording's readiness, its self-checks too).
  #463 S2-5 (0.12.65; `contracts/lab-recording.md`, "Drawing a box"): stage two marks a draft with
  boxes: `ac_record_mark` with `box` (`recognize=x1,y1:x2,y2` or `press=x1,y1:x2,y2`, two opposite
  corners in the frame's pixels; or `boxes` in `request_json`) on any step. A recognize box is a
  template mark of its own (`box/<nn>/<x>_<y>_<w>_<h>`), whose base images are the step's frames
  and the captures stage one took of it before its press that reach the threshold; the answer's
  `boxes[].frames` says of each frame `own`, `added`, `not_added` (with `because`) or `elsewhere`.
  A capture whose whole frame changed from the step's live frame (the previous screen, the middle
  of a transition) is another state of the screen and never a base image (`not_added`, `because`
  `frame_changed` with `frame_change_milli`). Give the recognize boxes of a step in one request:
  a later box is tested on the samples the earlier ones admitted.
  A press box is the step's click region; on a tap or a long press recorded with `record` it must
  hold the point pressed (`record_click_outside_step_rect` with `recorded_point_outside_box`
  otherwise) and the press stays as recorded; `ac_record_stop` then writes the package of the
  boxes, and a later `record archive` lists the box as the press's `rect`.
  Workflow #392 DA-5cd (0.12.67; `contracts/lab-recording.md`, "A recording's size"): a recording
  is made at the base size, 1280x720. `ac_record_start` first reads the instance's display as the
  Runtime last observed it (the device frame of its latest task) and refuses one of another size,
  `record_display_not_base` with `frame_width`, `frame_height`, `base_width` and `base_height`,
  before anything is written or paused; its answer names `display_last_observed` (`{width,
  height, observed_at_unix_ms}`, not the display now; `null` when none was observed). That is an
  early warning: the start writes the base size in the recording's header, and the recording's
  first frame of any other size is refused `record_display_not_base` before it is stored, nothing
  written. `ac_record_stop` refuses a recording whose frames are not the base size,
  `record_size_not_base`, and converts nothing; `ac_record_ready` lists it first as
  `size_not_base`, a frame added to it is refused with the same code, and `ac_record_status`,
  `ac_record_frames` and `ac_record_archive` read it as before: archive it and record again. The
  train of DA-5ab and DA-5cd: every tool that records (`ac_tap`, `ac_swipe` and `ac_long_tap` with
  `record`, `ac_do` and `ac_lab_observe` with `record`) judges the recording and the frame it took
  before it sends or writes anything, with these codes and `record_frame_size_mismatch` for a
  frame of another size than the recording already has; nothing is pressed first.
  #463 S2-7 (0.12.68; `contracts/lab-recording.md`, "Transitions", "Steps, frames and
  remediation"): in `request_json`, a page transition (`transition` `{"kind": "page", …}`) takes
  `frame_id` in place of `frame` and `sample_frame_ids` beside `samples`: frames the recording
  already stored, as `ac_record_frames` and `ac_record_status` list them (a loading screen stage
  one captured between a press and the next step), read as stored with their sha256 checked; no
  file is copied, and the package is the one the same image given as a file makes. An id the
  recording does not hold is `record_frame_unknown`. `drop_step` (`step_action` `{"kind":
  "drop_step", "step": n}`, or the row's `drop_step`) takes a step before the last when the stored
  frames do not contradict that its press changed nothing: the frame after it, and the frame the
  next step was pressed on, are the frame before it by two comparisons, each by the frame-change
  measure stage one calls a screen still by, the whole frame and (fix round 1) the neighbourhood of
  the press (its rectangle grown by one cell of the whole-frame grid, 80 pixels at 1280x720, on
  every side). Each comparison averages 16 by 9 cells of its region, so a change confined to one or
  two of them is not seen: a small change away from the press (a counter, a badge) is not seen.
  Whether the press mattered is the agent's judgement; the tool refuses only what the frames
  contradict: `record_drop_unproven` with `drop_problem` `frames_not_recorded`, `screen_changed`
  or `next_screen_differs`, and for the last two `drop_comparison` `frame` or `press_region`
  (#463 KR-1 fix round 1: `key_declared` for a key step, whose key is taken back first).
  `{"kind": "restore_step", "step": n}` gives a dropped step back (`record_mark_rejected` when a
  mark it reuses or forbids is gone since). Nothing is proposed: the agent names the frame and the
  step. The train's fix round (0.12.68): a step that holds a choice (marked with `choice`, or
  refined by a `select`) is not dropped, the last step too, before any frame is compared
  (`record_select_invalid` with `select_problem` `drop_of_choice`): its press may have chosen the
  item already selected, which the frames do not show and which still matters to a run handed
  another value. Nor is it turned into a page transition (`to_transition`,
  `transition_of_choice`). Nothing is written; withdraw the choice first (`choice: false` where
  the step carries the mark, `clear_select` where a select refines it), then drop or convert it.
  #463 S2-6 (0.12.68; `contracts/lab-recording.md`, "Marking a choice first"): a step that chooses
  one item among several is marked first and refined later. `ac_record_mark` with a
  `request_json` that names the step and holds `"choice": true` (beside the step's marks if you
  like) marks it; the
  press stage one recorded on the item you chose, a point tap included, is its demonstration.
  Refine it later, on the active or the stopped recording, by declaring the step's `select` in a
  request of its own ("Selecting in a list": the slots, how each name is read, the names, the match
  and the wanted value; `pressed` the name the recorded press chose). Until then `ac_record_ready`
  lists the step as `select_unrefined` (code `record_select_invalid`) and `ac_record_stop` refuses
  it with `choice_not_refined`; `choice: false` takes the mark off (on a step without the mark it
  is refused, `no_choice`; on a refined step the select stays). The mark only says the choice is
  not refined yet: the wanted value of every select, marked first or given whole, is the package's
  `ordered` variable of the select's wanted name, its default the select's default (a run given no
  value chooses as before), which `ac_goals_set` sets on an application by name; a package
  generated again from an older recording with a select gains that variable, so its content hash
  changes and its bundle declares `package` 14.
  #463 S2-10 (0.12.70; `contracts/lab-recording.md`, "Regression cases written with the package"):
  `ac_record_stop` writes the package's regression cases beside it,
  `lab/out/<package_id>.cases/<case id>/` (`case.json` and `scene.png`, the case contract's form):
  one per step with a press and a page, on the frame its press was sent on, one for the final
  screen and one per ending, each a black canvas with exactly the regions the package's
  recognition targets read pasted where they are; where a target reads the whole frame (its region
  or a template's search area is the whole frame), the scene is the whole frame, and the answer
  names each such case with the warning `record_case_whole_frame` and those targets (nothing is
  refused for it). A case's `source` names the recording's id and the step (or `final`, or the
  ending) and nothing else; a recording id the Lab made by default holds the time the recording
  began and a process number, never a path, an instance, an account or a device. Its last
  self-check, `cases`, runs each on the package first: a real miss (the page not recognised on its
  scene, a target it expects failing) is not written and the stop refuses `record_case_failed`
  (`regression_problem`; its `page` as the page detection names it, `<game>/<page>`), nothing
  written; `ac_record_ready` lists every such case (`what` `cases`, with the case's id and the
  runner's failed checks in `details`). A page recognised beside another page of the package is no
  miss (a linear run waits for each step's own page): the case is written without
  `expect.observe`, every target its page requires passing and every target it forbids failing
  (it states that this page holds on this scene), run again in that form first, and both tools
  name it with the warning `record_case_page_ambiguous` and the other pages; such a page with no
  target to pin gives no case and the warning `record_case_ambiguous_not_written` (mark a target
  on that page's screen to have it written). Without a text-recognition model for a text target
  the page detection reads, no case's page is judged, which the warning
  `record_cases_ocr_unavailable` says once. The answer's
  `lab.cases` names the directory, `counts` by kind, the `ids`, `not_proven` (a case run in part,
  with the runner's reasons, `ocr_unavailable` without an OCR model), `warnings`
  (`record_cases_ocr_unavailable` first, then each case's: `record_case_page_ambiguous` or
  `record_case_ambiguous_not_written`, `record_case_bare_canvas` (its page is also what the bare
  canvas answers), `record_case_whole_frame`; each warns, exit 0) and `limits`
  `["self_agreement"]`: the cases were made from the frames the package was built from and say
  nothing about screens it did not meet. A regeneration replaces the set whole and keeps the
  earlier one beside it; `dry_run` writes nothing, and its `lab.would_write` names the cases'
  directory and each case after the package. The cases are written beside the package only;
  adoption does not take them into a repository yet (#463 AC-1, another slice).
  #463 S2-8' (0.12.70; `contracts/lab-recording.md`, "Folding swipes into a scrolling search"): a
  choice in a scrolling list is recorded as several swipes, each a step, then the press on the
  wanted row. `ac_record_mark` with `step_action` `{"kind": "fold_scan", "step": b, "first": a}`
  (or the row's `fold_scan` and `fold_first`), on the active recording or, as an amendment like a
  mark after stop, on a stopped one (fix round 1: the next `ac_record_stop` generates again; no
  other step action is taken there), folds the swipes `a` to
  `b - 1` into the scrolling search of the press `b`: they stay in the file whole, each with
  `folded_into: b`, and are no steps of the package any more (no page, no operation, no case, no
  item of `ac_record_ready`); `b` holds `scan_fold` (its first step, the axis and each swipe
  `forward` or `back`). You name the steps; nothing finds them for you. Swipes of different lengths
  along one axis in one direction are accepted; the demonstration is forward as often as needed,
  then back as often as needed, then the press. Every refusal is `record_scan_invalid`, nothing
  written, its `scan_problem`: `fold_range_invalid` (the range is no run of executed swipes before
  an executed tap), `inserted_in_range` (a step of it is an optional step inserted after the stop),
  `would_lose_marks`, `would_lose_page`, `would_lose_optional`, `would_lose_choice`,
  `would_lose_transition`, `would_lose_inserted` (an optional step inserted after a swipe of it
  after the stop; take that off the swipe first, or give the inserted step up), `move_axis_unclear`
  (a swipe whose move along one axis is not at least twice its move across, or along another axis),
  `move_after_back`. `{"kind": "unfold_scan", "step": b}` (`unfold_scan`) gives the recording back
  as it was (`not_folded` for a step that holds no fold). Any other step action on a folded swipe or
  its press step, and marks on a folded swipe, are `step_in_fold`: unfold first. `ac_record_status`
  shows `folded_into` and `scan_fold`; `ac_record_review`'s sheet of the press step lists the
  folded swipes in order with their screens (`folded`); `ac_record_frames` reads a folded swipe as
  any step. Until a scrolling select is declared on the press step (S2-9a below),
  `ac_record_ready` lists a fold as `scan_without_select` and `ac_record_stop` refuses it
  (`scan_not_selected`); declare the select, or unfold it to make the package without it.
  #463 S2-9a (0.12.70; `contracts/lab-recording.md`, "A scrolling choice: reading the list to its
  end"): that scan is the press step's select. Send `ac_record_mark` with `request_json`
  `{"step": b, "select": {"rows": {"row", "anchor", "name", "click"?}, "scan": {"list", "start",
  "end", "stop": "end"}, "read", "names", "match", "wanted", "pressed"?}}`: every rectangle is
  yours, drawn on the list's first screen (the first swipe's screen before it in `ac_record_review`'s
  `folded`), but the end mark, drawn on the last screen after a forward swipe. Scroll to the list's
  end once while recording: a select without `end` stands, `ac_record_ready` lists
  `scan_end_not_recorded` and `ac_record_stop` refuses it (`end_not_recorded`). The Lab measures the
  scan's numbers from your regions and the demonstration and keeps each with what it was measured
  from; give one in `scan.numbers` by its name to replace it (`origin: "given"`). The self-test reads
  every screen, checks the start and end marks and the press, and runs the kernel's offline entry
  over the forward screens for the pressed name and for a name no screen shows; its refusals are
  `record_mark_rejected` reasons (`select_name_unread`, `scan_start_mark_repeated`,
  `scan_step_outside_window`, `scan_decision_mismatch`, …), the request's own are
  `record_select_invalid` (`scan_without_fold`, `rows_with_slots`, `confirm_with_rows`,
  `scan_stop_missing`, `scan_region_outside_frame`, `scan_region_outside_list`,
  `scan_region_outside_row`, `scan_number_not_measured`). `ac_record_review` shows the step's
  `scan` (screens, names read, numbers, the offline entry's decision); `unfold_scan` is refused
  `scan_selected` while the select stands: `clear_select` first, which keeps the fold. Inertia,
  settling, a list opened in its middle, the drag's real displacement and a list that loads while
  it scrolls are judged only by the trial on the device. Fix round 1: swipe shorter than the
  list's visible length, so that neighbouring screens share a row: a first swipe longer than the
  kernel admits is `record_select_invalid` `scan_swipe_too_long` (record the scroll again), which no
  `scan.numbers` cures. Cut the end mark from the list's true last screen: one that passes on an
  earlier forward screen is `scan_end_mark_early`. Each number in `scan.numbers` says what it is
  (`origin`: `measured`, `default`, `derived` or `given`).
  #463 S2-9b (0.12.70; `contracts/lab-recording.md`, "A scrolling choice: straight to the wanted
  item"): when the wanted item is known, swipe forward until its row shows and press it; mark the
  select with `"stop": "wanted"`. `end` is then optional: given, the package declares it; left out,
  `ac_record_ready` lists no `scan_end_not_recorded`, and `ac_record_stop` generates the scan with
  `early_stop` and without `end` or `on_empty`: found, the run presses; not found, it fails
  `selection_scan_early_stop_unreached` where the list stops moving. So `on_empty_key` beside it is
  refused (`record_select_invalid`, `on_empty_without_end`): where the item may be absent some days,
  record the list's end once and declare the end mark, and the run ends normally. Without an end the
  numbers taken from the screens up to the list's end are `derived` from what the screens show
  (`origin: "derived"`: trailing gap the list's length, anchors `b / e + 2` per column,
  `max_viewports` `2V + 2`, return viewports `2V`, `max_ms`), and the Lab refuses a given
  `advance_min_content_step_px` above twice
  `position_tolerance_px` plus one (`scan_min_step_too_large`); the offline entry must stop early on
  the pressed name, no absent name is run (that is judged only on the device), and the self-check
  refuses a wanted default no forward screen shows (`scan_default_not_shown`). `ac_record_review`'s
  `scan.when_absent` says the kind, whether an end is declared and what a run does without the item.
  #463 KR-1 (0.12.70; `contracts/lab-recording.md`, "Key steps"): the back key is not recorded
  while playing; stage two or three adds it. `ac_record_mark` with `key` `back` (`request_json`
  `"key": {"key": "back"}`) makes the step's one effect the back key, declared and never sent: on a
  step without an effect it is declared; a click of the step, executed or declared, of any form, is
  replaced only with `replace_click` (`record_step_effect_exists` otherwise) and kept whole inside
  the key, the step's frames standing, and `no_key` (`"key": false`) takes the key back and gives
  the click back exactly, with the `click_guard` the step had before the key (fix round 1). It
  works on the active and the stopped recording (an archived one is reopened first). Its guard (fix
  round 1) is a click step's: the `click_guard` you name, else its page's first template, colour or
  colour digest, or check mark you drew; only a page of text or value-test marks alone leaves it
  none, and then `ac_record_ready` lists it (`key_guard_missing`, add a template to the page) and
  `ac_record_stop` refuses it (`record_key_invalid`, `guard_missing`). A key step whose replaced
  press needs review still needs review (`lab.steps_needing_review`); two optional steps closed by
  the key are one pop-up only on one page (otherwise `record_optional_step_ambiguous`); dropping a
  key step before the last is `record_drop_unproven` with `drop_problem` `key_declared` (take the
  key back first); a `key` that is no `{"key": "back"}`, `false` or `null` is `validation_failed`
  saying the three forms. Refused, nothing written: `record_key_invalid`
  with `key_problem` `word_unknown` (back alone), `with_click`, `with_application`, `with_retry`
  (send the key alone), `application_entry`, `application_step`, `holds_choice` (a select or a
  choice mark on the step) or `step_not_effective`. A pop-up a run met is closed by the key with
  `optional_after` and its `key` in place of `click` (both `with_click`; neither
  `record_optional_after_invalid` `no_effect`, the word that replaces `no_click`). `ac_record_stop`
  generates `step_<nn>_key`; its first decision at step 1 is `would_press_key`; the step has its
  regression case on the frame of the press it replaced; `ac_record_review` answers its `press`
  with `key` and `replaced` and draws no press; `ac_record_status` shows its `key`. Beside a fold
  (the train of 0.12.70): a key on a folded swipe is the fold's `record_scan_invalid`
  `step_in_fold`, on the press step of a fold `record_key_invalid` `holds_fold` (unfold first), on
  one whose scrolling select reads the fold `holds_choice`; a fold over a key step, or onto a press
  whose effect is a key, is `fold_range_invalid`; and while a key stands the guard it gives back is
  in use (`record_mark_in_use` for its `remove`).
- **Offline scenes of another size** (Workflow #392 DA-5cd, 0.12.67, as the train of DA-5ab and
  DA-5cd leaves it; `contracts/display-adapter.md` section 12). `ac_recognize`, `ac_detect_page`,
  `ac_current_page`, `ac_is_visible`, `ac_lab_observe` on a saved scene, `ac_tap_target` and
  `ac_navigate` with `dry_run`, and the offline dry runs and waits of `ac_do`, `ac_ensure` and
  `ac_wait`, take a scene of another size exactly as the live observation takes a frame of that
  size, by the same function: a scene of the package's ratio, not smaller and in its orientation
  (1600x900, 1920x1080, 3840x2160 for 1280x720), is reduced to the package's base, every rectangle
  and point of the answer is the base's, and the answer names `frame_coordinates` in the base form
  (`{space: "base", base_pixels, reduced_from, orientation, applies_to}`, `reduced_from` the
  scene's own size). A scene of the base's size answers as before. Any other scene is refused
  `lab_scene_size_unsupported` (`display_problem`, `frame_width`, `frame_height`, `base_width`,
  `base_height`); a live capture the map cannot take is `display_map_unresolved` instead.
- **Adoption** (Workflow #464 P1, 0.12.61; `contracts/standard-pack-authoring.md`).
  `ac_standard_pack_adopt` (`repo`, `record_id` or `package`, `decide`, or `held_reason` with an
  optional `held_reference`, `replace`, `state_dir`, and, #463 AD-1, 0.12.72, `session_state_dir`
  beside `record_id`: the directory the recording is read from, only read, so that `state_dir`
  may lie on the checkout's volume) makes a stopped recording's package, or any
  package, a member of its game's standard pack in a resource repository's checkout, through a
  child: the pack directory, its rule (or held entry, or maintenance entry) and the `requires`
  words, each derived member answered with where it came from. `held_reason` and `held_reference`
  (Workflow #463 S3-4, 0.12.63) are the decision of the class `held` without a document, read as a
  document given by `decide` is; not with `decide`, and `held_reference` only with `held_reason`.
  Every check runs before the first write; a refusal is `adopt_refused`, its `adopt_problem` naming
  the check, and wrote nothing; with `dry_run` nothing is written in the checkout. Its `value` is
  `"pending"` or a number of the pack's own: a decision that names the origin `model` is refused
  (`value_origin_model`). Its `warnings` are those of the adopted package and of no package
  (#463 AD-1, 0.12.72); those of the root's other packages are counted by code in
  `other_packages_warnings` (`total`, `counts`), unless the split could not be made: then every
  warning is in `warnings` whole, `adopt_warnings_not_split` last, and no `other_packages_warnings`.
- **Where a pack stands** (#463 S3-14, 0.12.68; `contracts/standard-pack-schedule.md`, "Where a
  pack stands"). `ac_standard_pack_status` (`root`, `repo`, `package_id`, `state_dir`,
  `installation`) answers, for
  each pack of a game root, through a child and reading only: `formed` (admitted, and its content
  hash prefix as the trial names a content), `conditions` (its readiness items as
  `ac_standard_pack_ready` answers them) and `missing` (each failing one, a hold with its reason and
  reference), `held` (a pack's hold whenever the schedule holds it), `trial` (the trials of this
  content: `passed`, `failed` or `none`, the latest record and `passed_on_record`, the same summary
  as `ac_standard_pack_adopt`'s `review.trial`; `trial_not_read` instead for a pack whose records
  cannot be read), `enabled` (whether the root's schedule enables it) and `stage_state`
  (`not_admitted`, `not_judged`, `can_be_tried`, `supporting` or `enabled`), with the root's
  counts. Without `installation` the placement journals are not read: `enabled.authorization` is
  `not_recorded` with `journals_not_read`, never an authorization guessed. With `installation`
  (#463 S3-15a, 0.12.69; served where the rescue module is built, Windows) the child reads the
  installation it runs from, and `root` must be that installation's game root of its game
  (`package_status_root_not_installed` otherwise, before anything is read; a build without the
  rescue module answers `package_status_installation_unbuilt`): `enabled.authorization` is the
  placement that last enabled the pack (`reference`, `initiator`, `at_unix_ms`, `operation`) or
  `not_recorded` with `no_journal_names_it` or `journals_unreadable`; each pack answers `pairs`
  (each task and instance the installation's one `actingd suspended` names: its `suspended`
  `pause_cause` and its `break_in` row, as the report names it), or in their place the word
  `suspension_report_not_read` or `bindings_not_read`, or (fix round 1) for a pack no composed task
  runs `no_task_bound`; the root answers `installation` (`operations_read`, `operations_unread`,
  `suspension_report_read`, `report_warnings`, the report's `ledger_read_incomplete` passed on, and
  the word's outcome, in the detail `ACTINGCOMMAND_DETAIL` sets) and, in `warnings`, outcomes
  only: the composition's warnings that concern the game. The child is started before the
  journals are read and runs beside them. The readiness items of an installation (I1 to I6) are
  not read. `packs`, `pairs` within them, grows with the root: an answer over the budget is
  written whole to the export directory; `package_id` reads one pack.
- **The package declaration** (Workflow #477 PC-1, 0.12.68; `contracts/pack-catalog.md`).
  `ac_standard_pack_catalog` (`root`, `plan`) builds or brings up to date a game root's
  `pack-catalog.json`, through a child: an entry for every package of `bundle.json`, a new entry's
  display from a leftover `ui.json` and its `consumes` and `produces` the union over the package's
  rules in `schedule.json`; an entry already there is kept as written and its differences from the
  rules are answered (`differences`, each also a `standard_pack_catalog_schedule_differs`
  warning). With `plan` it writes nothing and answers the `document` it would write; without it,
  it writes the file atomically (`status` `written`, or `unchanged` when the file holds those
  bytes). `root` is a resource repository's checkout (`applications.json` and `packs/`: the game
  from `applications.json`, the registry the build's `package_bundle` makes, the file written at
  the checkout's root, to be committed there) or a built game root (`bundle.json`), never a root
  inside an installation (`standard_pack_catalog_root_refused`): an installed root takes the file
  through `ac_standard_pack_build` and `ac_place`. What it leaves out of an existing file (an entry
  of a package no longer registered, a pool `schedule.json` no longer declares) is answered in
  `removed` and `dropped` and warned of (`standard_pack_catalog_left_out`). Do not edit the file by
  hand. Workflow #477 PC-2 (0.12.71): `refresh`, a list of package ids, each once, brings each
  named entry's `consumes` and `produces` up to date from its rules (the one way an entry's business
  part is brought back to the rules), its display and its place kept, and answers `refreshed` (each
  `{package_id, state replaced|agrees|added, before?, after}`); the order agents use is adopt held,
  try, write the rule, then call with `refresh`. `displays`, an absolute path of a file
  `{"schema_version": "actingcommand.pack-catalog-displays.v1", "groups"?: [{id, title}], "packs":
  [{package_id, title, summary, group?, risk?}]}`, replaces each named entry's display whole and
  answers `display_written`; a group the catalog does not hold is added after its groups and
  answered in `groups_added`, one it holds with the same title is there already, one it holds with
  another title is refused (`catalog_group_title_differs`), and no group is removed or renamed.
  The file is checked by the catalog's own checks first. A package the registry does not hold
  (`package_not_listed`), or named twice, is refused (`standard_pack_catalog_refresh_refused`,
  `standard_pack_catalog_display_refused`) before anything is written. `ac_packs` answers each
  game's `index` (per pool the packages that produce and consume it) and each composed row's
  `catalog`; `ac_standard_pack_check` answers the root's `catalog` note and the file's warnings.
- **Regression cases** (#463 RC-1, 0.12.69; `contracts/resource-regression-case.md`).
  `ac_resource_regression` (`repo`, `game`, `case`, `package_id`) runs the regression cases of a
  resource repository's checkout offline, through a child: each case `regression/<game>/<case>/`
  on its package `packs/<package_id>/`, with the kernel's page detection and target evaluation,
  and answers the counts of the verdicts (`passed`, `failed`, `partial`, `not_run`); with `game`,
  `case` or `package_id` also a short row of each case asked for (`path`, `id`, `package_id`,
  `verdict`, the checks that did not pass with their words, its warnings). It reads only. A failed
  case makes the call the refusal `resource_regression_failed`, whose details are the counts,
  `failed_total`, `listed` and the rows of the first 10 cases that did not pass, the failed ones
  first, so it always fits; partial cases add the warning `resource_regression_partial` with their
  count; a case of another schema is `not_run` with the warning `case_schema_unsupported` and fails
  nothing.
- **The undo of a placement** (Workflow #463 S3-18, 0.12.69; `contracts/install-cli.md`, "The
  packages the undo of a placement makes enter or leave scheduling"). On Windows `ac_undo` of a
  `pack_place` operation answers `schedule_changes`, the packages it makes enter or leave
  scheduling, beside `kept`; `plan` true (that kind of operation only) answers them with
  `reverses` and undoes nothing. Packages that enter are the warning `undo_enters_scheduling`
  (`names`), a list that cannot be known `undo_scheduling_unknown`, each a note of the answer's
  outcome (`outcome.causes`), not a refusal: report them with the answer. It asks for no
  authorization. An undo run again after it stopped, its `plan`, and `ac_op_resume` of an undo
  that ended its steps answer the lists its undo record holds (`{enters, leaves}`) and their
  warnings, never lists computed again from a half-undone root.
- **Busy** is `job_capacity_exhausted` with `limit_count`: a call that would start a job while
  `--jobs` jobs run (default 8), or that needs a child while all `--children` children (default 8)
  are working. It is refused, not queued, and nothing is sent: call again later. More calls at
  once than `--workers` (default 8) wait for a worker, within their budget.
- **A child that fails** before its answer is `lab_process_failed`, with the tail of its stderr.
  The call is never sent again, and the next call starts a new child; read the call's effect
  before you send it again.

## 9 Deploy and rollback

- **Always detached.** On Windows, where the rescue module runs, `ac_deploy` (`target`, `dir`,
  `plan`) and `ac_rollback` (`to`, `plan`) are served as every other rescue row is, and always
  run detached: they replace the programs the server itself runs. The server gives the row's
  `detach` itself (neither tool takes it), and the rescue module starts the operation again in a
  process of its own, the `actinglab.exe` beside the server, with the row's words, and answers
  at once. Elsewhere the rescue module does not run, and neither tool is offered.
- **The answer** is `install_command_done` (`isError` false) with `detached: true`, the detached
  run's `pid` and `log` (where its output goes, `install\detached-<ms>.log` under the root), and
  `next` naming `op status`. It does not wait for the operation and says nothing of how it ends.
  A refusal before the start (an unfinished operation, a root whose installation state cannot
  be read, no `actinglab.exe` beside the server to run it) answers at once, and nothing starts. With `plan` true the call answers the plan only,
  in the server's process, and nothing starts.
- **Follow it with `ac_op_status`.** Without arguments it answers the unfinished operation
  (`unfinished`, its `operation_id` among its members) while the detached run works, and the last
  operations (`last_operations`, each with its `operation_id`, `kind` and result) once it ended;
  with `op` set to an operation id it answers that operation's `journal`. Read it before you call
  either tool again: a call made while an operation is unfinished is refused.
- **Who started it.** The detached run records `Lab-MCP` in its journal, as the rescue rows the
  server serves in its own process do: its start sets the initiator's variable and, beside it, a
  marker only that start sets, without which a Lab process refuses `Lab-MCP`
  (`contracts/lab-two-entries.md` §4.2). Its requests to a running Runtime, as every rescue row's
  of this server, bind the Lab channel with `Lab-MCP` and keep the command line's envelope (since
  Workflow #434 LAB-6S2-B1, 0.12.59; on the CLI channel before): none writes a record that names
  a channel, and their answers name the channel `mcp` where they named `cli`.
- **Production.** Until LAB-7 a production installation deploys and rolls back through
  `actingctl install` (the coordinator's deploy kit), not through the rescue module and not
  through this server: the module's `deploy` and `rollback` are not used on the production root
  (`contracts/install-cli.md`, "Operational rule until LAB-7"). Whether either runs at all is the
  person's decision (section 10).

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
  changes, and `ac_deploy` and `ac_rollback` replace the installed build (section 9); on every
  platform `ac_config_set` and `ac_config_clear` change settings the Runtime may change while it
  runs, each change recorded. The server asks nothing before any of them: an
  irreversible one records `Lab-MCP` as its initiator, and the tool of an irreversible or a
  disruptive row carries `destructiveHint`. Whether one runs is not the server's decision.
- **Whose decision it is.** Starting, stopping or restarting the Runtime, replacing or restoring
  its configuration file, adding an instance, deploying and rolling back a build stop the
  installation's work or change what it runs: they are the person's to decide. An agent
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
