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
`resource verify` (Workflow #411 RV-1, [resource-verify.md](resource-verify.md)) needs `offline`,
is `available` and declares `refused` (`dry_run_unsupported`); its entry carries one more
member, `report_schema: "actinglab.resource-verify.v1"`, the schema of the report it writes, so a
repository wrapper reads from `capabilities` whether the Tools it pins can verify.
`standard-pack build` (Workflow #464 P2, [resource-verify.md](resource-verify.md), "`standard-pack
build`") needs `offline`, is `available` and declares `refused` (`dry_run_unsupported`); its entry
carries `build_schema: "actingcommand.standard-pack-build.v1"`, the schema of the `build.json` it
writes, so a wrapper reads from `capabilities` whether the Tools it pins can build.
`resource regression` (#463 RC-1, [resource-regression-case.md](resource-regression-case.md))
needs `offline` and `read_only`, is `available` and declares `no_effect`: it reads a checkout's
regression cases and their packages and writes nothing.
`resource check-release` was `retired`, with `reason_code` `resource_check_release_retired`; since
0.12.47 (Workflow #434 LAB-2) its entry and its word are gone, and the code is retired.
`package loading` (Workflow #407 LOAD-1, since 0.12.37; [package-reference.md](package-reference.md),
"Content-directory tools and the bundle index") needs `offline`, is `available` and declares
`mixed`: `--check` `no_effect`, `--write` a `dry_run` preview.
`session backend` (#402 HS-L, since 0.12.40; [runtime-config.md](runtime-config.md), "The Lab")
needs `running_runtime` and `device`, so its status is `unverified`, and declares `refused`
(`dry_run_unsupported`). `session instance list` and `session instance registry` stay `offline` and
`available`: their `runtime_backends` column reads the Runtime when it runs and says, beside the
list, when it could not (`runtime_backends_read`).
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
declares. An option a path takes once and that is given more than once is refused the same way,
with `argument_problem` `repeated`, instead of the last one winning; only the offline paths'
`--scene` may be given one to five times, oldest first, the last the current frame, as may the
`--scene` of `recognize`, `detect-page`, `current-page` and `is-visible`
([business-identity-consensus.md](business-identity-consensus.md); MCP's `sample_scenes` are
the earlier frames, passed before `scene`'s; #411 Lab B-2). `recognize`, `is-visible`,
`detect-page` and `current-page` refuse every other repeated option the same way, and
`tap-target` and `navigate`, which read one frame, every repeated option, `--scene` included,
each before it reads anything. `tap-target` and `navigate` only plan, on a saved scene with
`--dry-run` (`--capture` is refused, the retired legacy semantic capture); without `--dry-run`
they are refused `validation_failed`, the refusal naming what presses on an instance: `do
--capture` with the package and the target, an element, or `--tap x,y`, and `ensure --capture`
for a route (Workflow #434 LW-1, 0.12.67).

Workflow #392 DA-5, the train of DA-5ab and DA-5cd (0.12.67; [display-adapter.md](display-adapter.md)
sections 11 and 12): a frame of another size than its package's base is taken by one function,
whether it is a `--scene` file (the offline paths of the four verbs, `recognize`, `detect-page`,
`current-page`, `is-visible`, and the dry runs of `tap-target` and `navigate`) or a Runtime capture
(the `--capture` forms): a frame of the base's ratio, not smaller and in its orientation, is
reduced to the base once, and the answer names `frame_coordinates` in the base form; any other is
refused before anything is evaluated, `lab_scene_size_unsupported` (exit 2 on a `--via` line;
actinglab's own line and `serve --stdio --front mcp` exit 1 for every refusal) for a `--scene` file
and `display_map_unresolved` for a Runtime capture. A frame of the base's size answers as before.
Every `--record` form judges the recording and the frame it took before anything is sent or
written ([lab-recording.md](lab-recording.md), "A recording's size").

Workflow #392 DA-L0 (0.12.83; [display-adapter.md](display-adapter.md) section 14): `layout probe
--scene <png> (--package <path> | --zip <file>) [--page <id>] [--tolerance-px <n>]` (MCP
`ac_layout_probe`), a row of the authoring module, needs `offline` and `read_only`, is
`available` and declares `no_effect`. It reads its package and its scene as `recognize` does and
takes a frame of the base's orientation, at least its height and of its ratio or wider, brought to
the base's height and resampled once (a frame the run path's frame gate takes as the base's has no
room); for every template mark the chosen pages name, with the rule that names it, it answers where
the mark is found on that frame against the place the base gives it (`dx`, `dy`, the base place
taken per axis from the recording's default box, `unknown` where it cannot be), how many places
reach its threshold, a class (`start`, `centre`, `end`, `other`, `missing`, or `as_mapped` without
room; none for a found mark whose base place across is unknown, counted `unknown`), the counts,
the distinct `dx` values and one line of reading per page and overall, and lists every other mark
as not probed. Every option is given once; the line is parsed by its read list. A frame of another
orientation, lower than the base or narrower than its ratio is refused `lab_scene_size_unsupported`
with `orientation_mismatch`, `scale_below_base` or `ratio_below_base`.

#463 SEL-F (0.12.83; [lab-recording.md](lab-recording.md), "Converting a package from its own
files"): `package fix-select --package <directory> --variable <name> [--fixed <name,name,...>]`
(MCP `ac_package_fix_select`), a row of the authoring module beside `package edit`, needs
`offline`, is `available` and declares `preview` (`dry_run`). It converts a package's select whose
wanted item is a package variable into the fixed form from the package's own files, the converted
package admitted in memory before the files it changed are written as `package edit` writes them;
a refusal (`record_select_invalid` with its `select_problem`) writes nothing.

Workflow #503 LIST-A3 (0.12.91; [resource-readings.md](resource-readings.md), "List readings",
"Offline"): `package list-check --package <path> --frames <directory> [--step <operation id>]`
(MCP `ac_package_list_check`), a row of the authoring module, needs `offline` and `read_only`, is
`available` and declares `refused` with `offline_device_scope_forbidden`, as `replay` does: it reads
its package and its frames as `replay` does and writes nothing. For one `read_list` step, or every
one of the task, it answers the kernel's dry run of the list reading over the frames: every
position the scan read with the reads of its name (the text, its confidence, the mark, the nearest
wanted key and alias, the distance and the limit) and what it gives the list, and the wanted keys a
card showed and those none showed; without `--json` it prints that as text for a person. A list
not whole is `package_list_check_not_whole`; every option is given once, and the line refuses a
flag off its list and any positional. Workflow #503 LIST-B (0.12.93): for a position whose key is
a picture identity of a layout's pool, each read carries `pictures`, the identity mapping's
judgement of each sample (the mark, the best key and the best `other` entry with their targets and
scores, the threshold and the margin), which the check prints and compares nothing of.

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
as declared support with status `unverified`: `nemu_ipc` performs `tap`, (HS-4,
#402, since 0.12.39) `long_tap` and `swipe`, and the three segmented drags
(`single_touch_drag_with_vertical_brake_v1`,
`single_touch_drag_with_pause_and_brake_v1` and, Workflow #406 T3, the path drag
`single_touch_path_drag_v1`), only on a frame the Runtime captured
(`nemu_input_frame_required`), (Workflow #463 KB-1, 0.12.69, §1.447) the back key alone
(`back`: the emulator library's key down and key up, on such a frame too; a library without
those two calls sends no key: fix round 1, the key is refused when it is sent,
`nemu_input_capability_unsupported`, `denied`, nothing sent and nothing recorded as
indeterminate), and no `text`; its reset needs no
frame (`v012-register.md` §1.268). Workflow #434 LW-1 (0.12.67, §1.435): actinglab's `tap`,
`swipe` and `long-tap` without `--record` read the instance's status once, on every instance,
and, where its capability table names `nemu_ipc` as the input of the press, capture one frame
through the Runtime first (its reference alone) and send the press on it (the answer's
`input_frame`), as a recorded press does; on every other instance they capture nothing. Where the
Lab reads "this input needs a frame": the `evidence_ref` of an input row of the instance's
capability table (`input.tap`, `input.long_tap`, `input.swipe`, …, in `Status`) is the instance's
configured touch backend, and the Lab decides as the Runtime does, which presses only on a frame
when that backend is `nemu_ipc` (`runtime-host` `host/nemu_input.rs`, `nemu_input_check`, reads
the configured `input_backend`); the table has no member of its own for it (one would be an
interface revision). A status that cannot be read stops no press: it is sent without a frame, as
before LW-1, and the Runtime refuses it where the input needs one (`nemu_input_frame_required`);
the answer's `touch_backend_warnings` says the status was not read (fix round 2: a refusal's is
in its `details`, the Runtime's refusal of that press too). A frame the Runtime no longer
holds when the press names it (`input_frame_unavailable`) is captured once more and the press
sent once more under the same lease, once (`input_frame_captured_twice`). Every other key and
`text` stay refused there (`nemu_input_capability_unsupported`, nothing sent). Workflow #463 KB-1
(0.12.69, §1.447): actinglab's `key` is sent by that same path: where the capability table names
`nemu_ipc` as the input of `input.key`, `key back` reads the status, captures one frame and is
sent on it, with the one retry and the warnings of a press; a key that input does not send is
sent without a frame and refused; elsewhere nothing is captured. The refusal of `key` and `text`
carries `details.hint`, what presses instead, where the capability table says the instance's
input does not implement `input.key` or `input.text` (since KB-1 the table says `input.key`
supported on `nemu_ipc` and `adb_shell_input`, whose row's failure semantics name the back key
alone; KB-1 fix round 1: a refused key outside the key vocabulary there, such as `key home`,
carries the hint that this input sends only `back`, decided by the key vocabulary and the input
the table names, not by the refusal's code); `maatouch` performs `tap`, `long_tap`, `swipe`,
the three segmented drags, `key` (every key it is given) and `text`; `minitouch` the same without
`key` and `text` (a key answers a fatal device error before anything is sent, and the table says
`input.key` unsupported); `adb_shell_input` performs `tap`, `long_tap` (as a swipe that stays at
its point), `swipe` and (Workflow #406 T3, plan Q18 (a)) the three segmented drags
through `input motionevent`, from the device's Android SDK level 23 (Workflow #406
T4 corrected this list, which named no segmented drag for `adb_shell_input` and no
path drag), and (KB-1) the back key alone, as one `input keyevent KEYCODE_BACK` through the same
`adb shell` path its taps take (fix round 1: every other key is refused by the Runtime before
anything is sent and before an intent is written, as on `nemu_ipc`; since the key train with its
own code, `input_key_unsupported`, `nemu_ipc` keeping `nemu_input_capability_unsupported`).
The `gestures` lists name `key` only for the input that sends every key it is given
(`maatouch`); the back key of `nemu_ipc` and `adb_shell_input` is in the capability table's
`input.key` row. A key passes the Runtime's foreground gate as a tap does (KB-1). The value lists of `schema control`
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
