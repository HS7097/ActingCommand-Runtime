# Contained page observation

`RuntimeDebugSession::observe_contained_page` submits `ObserveContainedPage` with an
instance alias, an absolute package path, an external SHA-256 and up to 64 explicit
target identities. Runtime verifies the external hash before Containment opens the
Lab package. The package supplies `control.json`, `resources/manifest.json`, the
recognition pack, pages, navigation and optional projection metadata. Observation
does not require a task program. Module packages do not supply this observation
pipeline.

Runtime admits a read-only capture capability, captures one frame, and evaluates
the existing page detector over that frame. Every actual target result retains its
page, role, group and target position. Explicit targets reuse all corresponding
results from this frame; an explicitly requested target with no previous result or
failure is evaluated once. An ambiguous set of actual results remains ambiguous.
Navigation geometry is descriptive and does not grant input authority. This
operation acquires no input lease and performs no input.

The receipt state is `observed`, with a `ContainedPageObserved` result whose status
is `recognized`, `no_match`, `conflict` or `partial`. The last two use a native
recognition failure terminal. No match uses `PageUnmatched`. The frame's own
recognition fact remains `FrameDecoded`.

One `DiagnosticJson` artifact binds the request, correlation, instance, expected
and actual package hashes, original frame reference, RGB8 hash, shared
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
the enclosing diagnostic artifact is at most 256 KiB. Counts include omitted rows
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
