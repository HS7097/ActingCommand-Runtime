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
failure. The projection's artifact-kind frame identity uses the PNG artifact hash;
the separately named RGB8 hash covers decoded RGB8 pixels.

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
