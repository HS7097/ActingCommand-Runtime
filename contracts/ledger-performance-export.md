# Read-only ledger performance export

`actingledger --state-root <root> export --performance [--after <sequence>]
[--through <sequence>] [--limit <count>]` emits one JSON report with
`command: "performance"`. Plain `export` retains its existing human report.
Performance mode accepts only these sequence and limit options; it rejects
duplicates, unknown options, reversed ranges and limits outside 1–1024.

The sole input is the existing `GlobalLedger::open_read_only` snapshot. Opening
that snapshot retains the ledger's existing full source validation. The report
then scans at most `limit` raw facts in sequence order, including facts that do
not match either performance family. It creates no samples or persistent state
and does not acquire or modify the writer lock, repair a tail, or change source
files. This page limit bounds report projection, not snapshot validation I/O.

The sequence interval is `(after_sequence, through_sequence]`. Omitted `after`
means zero. Omitted `through` freezes the snapshot's latest readable sequence;
subsequent requests must reuse the returned `through_sequence`. The page's
counts and rows cover only `(after_sequence, scanned_through_sequence]`.
`scanned_event_count` counts raw facts. An empty matching page can still have
`has_more: true`; resume with `next_after_sequence`, which is the last raw fact
scanned, even when no performance row was emitted. Later appended facts outside
the frozen bound are excluded.

`has_more` indicates pagination truncation of the readable interval. A null
next cursor means there are no further verified facts in that interval, and
does not certify an unreadable suffix. `window_complete` is true only when this
request's interval has been exhausted, the frozen upper bound is readable, and
the snapshot has no corrupt tail. `corrupt_tail` preserves the ledger's code,
segment, byte offset, byte count and tail hash. Unsupported source fields are
handled by the canonical reader's corrupt-tail report. `gaps` explicitly names
`corrupt_tail` and/or `through_sequence_unavailable`; a readable prefix is never
reported as a complete requested window. A corrupt tail conservatively keeps
`window_complete` false even for an earlier sequence bound.

Each row contains the original persisted `event`, including its id, sequence,
timestamp, origin, links and complete typed payload. Selection uses only
`PerformancePayload::StutterDetected` and
`PerformancePayload::BalanceChanged` with reason `ClockJump`:

- `observation.kind: "stutter"` repeats the recorded `frame_gap_ms` and the
  capture, recognition and action-effect latency values. Missing latency is
  explicit JSON null, never zero or a value estimated from the frame gap.
- `observation.kind: "clock_jump"` exposes the recorded optional instance and
  responsiveness/pressure metrics. The original event preserves the control
  reason, previous/current level, recovery and optional deadline disposition.
  A missing deadline disposition means no disposition was recorded.
  `magnitude_ms` is null because the existing clock-jump payload has no magnitude.
- Every row has `thread_identity: null`: these payloads do not identify an OS
  thread. Instance ids, origin modules and free text are not thread evidence.
  Null observation metrics mean unknown/unrecorded, not a measured zero.

`stutter_count` and `clock_jump_count` are page-local counts of these typed rows.
No thread attribution, cross-page aggregate, inferred latency, clock-jump size,
or additional host observation is produced.
