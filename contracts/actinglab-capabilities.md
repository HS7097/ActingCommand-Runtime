# ActingLab capability declarations

`capabilities`, help and `list commands` use the existing command inventory.
Lab2 verb/schema summaries consume the same command declarations. These queries
do not connect to Runtime, start a provider, inspect a device, load a model or
fetch backend assets. The Lab recording entries (`record mark`, `capture --record`,
`session capture --record`, `observe --capture --record`, `do --capture --record`,
`session app --record` and `session instance app --record`, Workflow #336) are
declared like any other command; their behaviour is `lab-recording.md`.
Both `record mark` aliases and the L4 `record stop` aliases declare a `dry_run` preview;
`resource catalog`, `package preflight` and `package check` declare `no_effect`. Their metadata
is shared by capabilities, help and command listing. `package check` (Workflow #411 A5) needs
`offline` and `read_only` and is `available`.
The six device `--record` forms declare `refused`. Capture and its session alias use
`dry_run_unsupported`; observe/do with `--capture --record` and both session application
recording forms use `record_flag_unsupported`, before recording locks or Runtime access.

The Lab2 verbs `observe`, `do`, `ensure` and `wait` declare only options some path of them
reads (#411 Lab L1). Each path (offline with `--scene`; online with `--capture`, and for `do` and
`ensure` with `--capture --dry-run`) refuses every option it does not read with
`validation_failed` (exit 2) before it loads, captures or presses anything: the details name the
first refused option as `argument` with its `argument_problem` (`conflicting` when another path
of the verb reads it, otherwise `unknown`) and every refused option in `arguments`. Global
options are not verb options. MCP `ac_lab_do` and `ac_lab_observe` take only arguments the CLI
declares.

Each command has `status`, `available`, `reason_code`, `needs`,
`availability_scope: offline_declaration` and `dry_run_mode`, with
`dry_run_refusal_code`, `dry_run_forms` or `dry_run_marker` as that mode requires
(`actinglab-dry-run.md`). `available` is true only for the
`available` status. Status values are:

- `available`: an implemented offline handler has a normal path with its caller
  supplied inputs; this does not promise a particular input will validate.
- `retired`: the current route returns its defined retirement error.
- `reserved`: the route has no implemented normal operation.
- `unavailable`: the current handler always refuses the requested operation,
  with the existing failure code stated in `reason_code`.
- `unverified`: required Runtime, device, provider, configuration or material
  availability is not established by this offline query.

Runtime-dependent entries remain unverified even when the client code is
compiled. Their normal execution still requires the existing admission and
error handling. Retired command entries remain discoverable; the query does
not invoke their handlers. Session access/transport summaries mark the retired
file queue and daemon authority explicitly and identify Runtime as the current
execution authority. Lease, authentication and direct-device-access constraints
remain in their existing contracts.

Capture backends retain their declared choices and asset requirements, with
`declared_support` separate from `available`. Client configuration and asset
names do not establish the Runtime's actual backend or provider readiness.
Backend availability is unverified and `availability_checked` is false.
Workflow #411 Lab L4: the backend listing names what the Runtime can use.
`capture_backends` lists the capture choices an actingd instance may name
(`capture_backend`): `adb`, `droidcast_raw`, `nemu_ipc` and `scrcpy`, the last
with `device_kinds: ["handset"]` since only a handset selects it
(`device-binding.md`); each has `declared_support: true` and status
`unverified`. The automatic choices `auto` and `auto-fastest` stay listed with
`declared_support: false`, status `unavailable` and `reason_code`
`capture_backend_must_be_explicit`, because actingd refuses an instance that
names one. `touch_backends` lists the four input backends an instance may name
(`touch_backend`), each with `gestures`, the Runtime input actions it performs,
as declared support with status `unverified`: `nemu_ipc` performs `tap` and the
two segmented drags (`single_touch_drag_with_vertical_brake_v1`,
`single_touch_drag_with_pause_and_brake_v1`), only on a frame the Runtime
captured (`nemu_input_frame_required`); `maatouch` performs `tap`, `long_tap`,
`swipe`, the two segmented drags, `key` and `text`; `minitouch` the same without
`key` and `text`; `adb_shell_input` performs `tap`, `long_tap` (as a swipe that
stays at its point) and `swipe`. The value lists of `schema control`
(`capture_backend`, `touch_backend`) and `session instance registry`
(`capture_backends`, `touch_backends`) name the same explicit choices, without
the automatic ones.
Discovered recognition-pack metadata is likewise unverified until normal
resource admission. Built-in template/color operations and Runtime provider
requirements are distinguished in the engine summary.

`schema_domains` and Lab2 `schema_versions` describe separate contract domains:
CLI envelope, legacy task, task operation, recognition pack, control and package
locator. No common highest version is defined. Each resource still passes
its existing version-specific validation; schema support does not prove an
installed resource or provider is usable. Workflow #336 PKG-S4 (option 丙): the package
locator domain, `package_locator`, says a package is found `by` its `path` and lists the
`readings` its path may choose (`["directory", "zip", "json"]`, `package-reference.md`
"Containers"); no package reference or digest form is offered. The recognition pack domain lists `0.1` and `0.3` through
`0.7`; a pack is written at `0.7` only when it declares a `color_digest` or
`composite` target or a per-target color `max_distance` (`selection-graph.md`).

`lab unpin --artifact-id <artifact-id> --pin-sequence <sequence>
--pin-event-id <event-id>` requires `running_runtime`. It releases only the named
object's exact Lab pin through RuntimeDebugSession and the existing Runtime
channel. Repeated flags, extra targets, and noncanonical identifiers are rejected.
The Lab/Lab request carries its own request/correlation identity; Runtime and the
Ledger resolve the original material identity rather than trusting a supplied
identity or requiring the old capture correlation.

The result's receipt contains the verified identity, original pin, durable Lab
request and durable PinReleased reference. `already_released` returns the original
release without appending another release. Unknown or non-Lab pins and sealed
eviction objects are denied. Communication timeout remains an unknown result
associated with the original request; the command does not retry a state change
or require a follow-up event query after a successful response. A release proves
neither deletion nor current eligibility: other pins, permanent evidence, Lab
references after the release prefix, close/summary/settlement and success
conditions still govern the original eviction path.
