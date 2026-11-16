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
  `ac_deploy` and `ac_rollback` take no `detach` (section 9). These are the only parameters of a
  row a tool leaves out; the command line also takes `actinglab`'s global options, a positional
  given as its flag (`--id`, `--instance`) and `devices bind`'s own `--root` (the server hands on
  the root it runs from), which no tool needs. No tool takes `words` either (Workflow #434 LW-1):
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
  line (as above) when the run ended by then, else the same handle.
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
  file. `ac_report_export` (Workflow #467 RPT-1) always writes its problem report archive there,
  `problem-report-<YYYYMMDD-HHMMSS>Z-<8 hex>.zip`, and answers its `path`, `sha256` and `size`
  (`contracts/problem-report.md`). None of them takes `out`: the server gives its own directory.
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
  Runtime's refusal of that press too). Such an instance sends no key and no text:
  `ac_key` and `ac_text` answer `nemu_input_capability_unsupported` there, nothing sent, and the
  Runtime has no other way to send a key on it; the refusal's `details.hint` says what to press
  instead (the screen's own control: `ac_tap`, or `ac_do` with `capture` on a package's element).
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
  with its word (`what`) and the code `ac_record_stop` would answer, or, once none fails, the
  refusal of a self-check; and its state, `material`, `draft`, `ready` or `packaged`. It only
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
  optional `held_reference`, `replace`, `state_dir`) makes a stopped recording's package, or any
  package, a member of its game's standard pack in a resource repository's checkout, through a
  child: the pack directory, its rule (or held entry, or maintenance entry) and the `requires`
  words, each derived member answered with where it came from. `held_reason` and `held_reference`
  (Workflow #463 S3-4, 0.12.63) are the decision of the class `held` without a document, read as a
  document given by `decide` is; not with `decide`, and `held_reference` only with `held_reason`.
  Every check runs before the first write; a refusal is `adopt_refused`, its `adopt_problem` naming
  the check, and wrote nothing; with `dry_run` nothing is written in the checkout. Its `value` is
  `"pending"` or a number of the pack's own: a decision that names the origin `model` is refused
  (`value_origin_model`).
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
