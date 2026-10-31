# Contained page observation

`RuntimeDebugSession::observe_contained_page` submits `ObserveContainedPage` with an
instance alias, an absolute package path, an external SHA-256 and up to 64 explicit
target identities. Runtime verifies the external hash before Containment opens the
Lab package. The package supplies `control.json`, `resources/manifest.json`, the
recognition pack, pages, navigation and optional projection metadata. Observation
does not require a task program. Module packages do not supply this observation
pipeline.

Runtime admits a read-only capture capability for each frame, captures one
observation transaction and evaluates the existing page detector over it. Without
target consensus the transaction is one frame. When the pack's target consensus
needs F frames (2–5), Runtime captures F − 1 sample frames and then the current
frame, the declared interval apart, and hands the set to the recognition pack's
aggregation owner as a task's observation does ([linear-steps.md](linear-steps.md),
"Target consensus"). It holds no lock while waiting; an input on the instance
between the first and the last capture fails the observation with
`observation_samples_interrupted`. The last frame is the observation's frame;
every frame is persisted by the capture owner under its own frame ID (#411 Lab L5).
When `observe` recognises one page completely, each candidate layout of that page
that needs more frames, in catalog order, gets its further page observations (each
a transaction as above) the layout's interval apart after the current frame, each
on the same page and frame size (else `candidate_sample_page_or_geometry_changed`,
a partial observation), and is projected over the current frame and each further
observation's current frame as a select step projects it, its row's coverage
`{provided_frames: F_l, required_frames: F_l, aggregation: production_owner}`; at
most 25 frames per observation, each admitted series counted whole (a layout beyond
that is projected over the current frame alone and refused with
`candidate_samples_unavailable`); only `observe` projects them. The input check then
runs from the first capture to the last, and the observation's frame, projection
and every other row stay those of its own transaction (#411 Lab L5b).
Every actual target result retains its page, role, group and target position.
Explicit targets reuse all corresponding results from this observation; an
explicitly requested target with no previous result or failure is evaluated once. An ambiguous set of actual results remains ambiguous.
Navigation geometry is descriptive and does not grant input authority. This
operation acquires no input lease and performs no input.

The receipt state is `observed`, with a `ContainedPageObserved` result whose status
is `recognized`, `no_match`, `conflict` or `partial`. The last two use a native
recognition failure terminal. No match uses `PageUnmatched`. The frame's own
recognition fact remains `FrameDecoded`.

One `DiagnosticJson` artifact binds the request, correlation, instance, expected
and actual package hashes, the current frame's reference (sample frames appear
only as the RGB8 hashes of the sample rows), RGB8 hash, shared
`PageProjection` and actual evaluation facts. Page summary rows and target rows
retain the original evaluation relationships, including results completed before a
failure. Remaining targets after a page failure carry explicit `not_evaluated` rows
with their original role, group and target position; these rows do not count as
actual evaluations. The projection's artifact-kind frame identity uses the PNG
artifact hash; the separately named RGB8 hash covers decoded RGB8 pixels.

The diagnostic artifact is created and verified before the final receipt.
`projection_sequence` and `projection_event_id` identify that artifact's actual
`ArtifactVerified` event. The receipt terminal identifies the later final event.
The projection's `content_sha256` excludes itself and remains separate from the
outer artifact hash and lifecycle sequence.

RuntimeClient checks the requested instance against Runtime's registry, validates
the receipt and package identity, and verifies both artifact hashes against their
native created/verified lifecycle. It checks the exact projection sequence and
terminal, and rejects input authority in the observation request's events. Only
this verified result supplies online Lab page semantics and optional frame bytes.

Each public or controlled raw fact collection has a limit of 256 rows and 64 KiB;
the enclosing diagnostic artifact is at most 256 KiB; an observation stores at
most five frames for its own transaction (#411 Lab L5) and 25 with its candidate
layouts' series (#411 Lab L5b). Counts include omitted rows
and omitted actual target evaluations. The public shared projection retains its
64-item, 32-KiB limits. Min output can omit auxiliary rows and reduce shared
projection entries, with explicit counts and the persisted source reference.

#411 A21: when rows do not all fit a collection, the Runtime keeps first the
evidence a recognition requires (the matched page's candidate samples and
projection, and target-consensus samples), then the rows of explicitly requested
targets (with the page target evaluations they reuse), then the others in their
order; the rest is counted as omitted. Kept rows stay in their order, and both
collections follow this rule, each with its own row sizes. A candidate sample is
kept only while its projection's failure row still fits after it, so candidate
samples that do not fit end that projection as `candidate_sample_record_failed`
(a partial observation). Explicit targets never change the observation's status
or projection. The observation fails only when the required evidence alone
exceeds the budget.

The verified catalog includes operation field declarations with or without a
projection companion. Any personal classification for a target from an operation,
companion or result restricts its raw observation, including `field=None`. Bound
field entries carry their complete `FieldKey`; they describe recognition values
with `parsed=false`, without executing an operation's field parser. Public output
redacts personal raw text, values and details. Original bounded facts remain only
in the controlled `DebugFull`, redaction-pending artifact.

`actinglab observe --capture` keeps the published generation reader alive through
the RPC and output processing, and reports reader-close failures. An artifact or
ledger fatal does not trigger another Lab diagnostic append. Ordinary package,
recognition and capture failures remain visible. Raw `ObserveReadonly` consumers
and offline observation retain their existing entry points.

#411 A21: a failed `observe --capture` answers `device_error` (exit 4) with
`details {error, state, req_id, receipt_state, runtime_outcome, correlation_id,
hint}`: `error` is the Runtime outcome's code, `state` is `failed`, `req_id` and
`receipt_state` are the failed receipt's when one was received (absent
otherwise), `runtime_outcome` is that outcome, `correlation_id` the debug
session's and `hint` `inspect-runtime-ledger`. A failed terminal ledger append
keeps its message and carries the same details.

LAB-OBS-1 (register §1.228, since 0.12.33): an `observe --capture` whose
observation stands closes its debug session's `runtime.debug_observe` with
`completed` (recorded `command.validated`) whatever its status: `no_match`,
`conflict` and `partial` are the answer's `state` and the observation's own
terminal above, never a failed command, as `do`, `ensure` and `wait` close an
answer that stands. Before, every status but `recognized` closed it `failed`,
which the Runtime recorded as `command.rejected` with no reason. A failed
`observe`, `do`, `ensure` or `wait` closes it `failed` and names the code it
failed with (`RuntimeDebugEvent.failure_code`: the Runtime outcome's code, or the
Lab answer's); the Runtime records `command.rejected` whose outcome is
`command_rejected` with that code as `failure_code`. A failed command event that
names no registered code, and any other event that names one, is refused
(`invalid_runtime_debug_event`) and records nothing.

Workflow #410 item 1: every `observe` answer, offline and with `--capture`,
states the frame's coordinate system as `frame_coordinates {device_pixels:
[width, height], orientation, applies_to: ["tap", "tap_rect", "swipe"]}`
(`do`'s coordinates are these frame pixels, from the top-left pixel;
[contained-lab-operation.md](contained-lab-operation.md)). With `--preview` it
also writes a JPEG preview of the frame, long edge at most 1024 pixels, into the
Lab frame directory and answers `preview {path, width, height, scale}`, or
`{failure}` (`lab_frame_export_failed`) when it could not be written; the image
itself is never in the answer. Both members are kept by Min and count toward its
budget. `--preview` under the global `--dry-run` is refused like `--with-frame`
(`dry_run_unsupported`).

0.12.28 LAB-SMALL (offline `observe`; `observe --capture` where named):
- #411 B47: a candidate layout of the recognised page whose feature needs an OCR
  or NN provider the Tools do not hold (`candidate_feature_provider_missing`)
  is not projected; the page is still answered, as an action's OCR target is
  not evaluated without failing (Workflow #396 LAB-3).
  `candidate_not_projected` lists each such layout in declaration order,
  `{layout_id, item, code, detail}`, and `ocr` gives the OCR state. Min keeps
  it. Any other projection failure still refuses the frame (`package_invalid`).
- #411 A62: each entry of `targets` (`--targets`) carries `target_center
  {x, y}`, the centre of the matched template box on the current frame
  (`x + width / 2`, `y + height / 2`, integer division): where the kernel
  presses a `target_center` click without `offset`. It is not the tap point
  after a tap `offset`: a `target_center` click with `offset` presses the
  centre of its offset box, which this member does not give. `null`
  when the target did not pass or matched no template box. A Min answer over
  its soft limit summarises `targets` as before (`id`, `passed`, `score`), so
  the member is read with `--verbose`.
- #411 A69: the `--with-frame` file (offline and with `--capture`) is written
  only after the answer stands; an answer refused as
  `observe_identity_limit_exceeded` writes no `--with-frame` file and creates
  no directory for it. Writes made before the answer is projected still
  happen: with `--preview` the preview is written into the Lab frame
  directory, and with `--capture --record` the frame is attached to the
  active recording.
- #411 A70: a page not recognised is never `standby`: the projection's
  `standby` (offline and with `--capture`, written by the shared projection
  owner) and the answer's `standby` are `false`; `matched` and `state` say
  whether a page was recognised. `detect-page` and `current-page` keep the
  kernel's sentinel: when no page matched they answer `page: "standby"` with
  `standby: true`, which only means that no page matched and is what
  `session recover` wakes the session on, never that the screen is ready for
  input; observe's `standby` is always `false`, so read `matched` and `state`
  there.
