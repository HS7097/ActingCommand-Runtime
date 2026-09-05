# Read-only ledger performance export

`actingledger --state-root <root> export --performance [--after <sequence>]
[--through <sequence>] [--limit <count>]` emits one JSON report with
`command: "performance"`. Plain `export` retains its existing human report.
Performance mode accepts only these sequence and limit options; it rejects
duplicates, unknown options, reversed ranges and limits outside 1–1024.

The sole input is the existing `GlobalLedger::open_read_only` snapshot. Opening
that snapshot retains the ledger's existing full source validation. The report
then scans at most `limit` raw facts in sequence order, including facts that do
not match a selected performance variant. It creates no samples or persistent state
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
`corrupt_tail`, `through_sequence_unavailable`, and/or `storage_read_incomplete`; a readable prefix is never
reported as a complete requested window. A corrupt tail conservatively keeps
`window_complete` false even for an earlier sequence bound.

Each row contains the original persisted `event`, including its id, sequence,
timestamp, origin, links and complete typed payload. Selection uses only
`PerformancePayload::Summary`, `PerformancePayload::StutterDetected`, and
`PerformancePayload::BalanceChanged` with reason `ClockJump`:

- `observation.kind: "stutter"` repeats the recorded `frame_gap_ms` and the
  capture, recognition and action-effect latency values. Missing latency is
  explicit JSON null, never zero or a value estimated from the frame gap.
- `observation.kind: "clock_jump"` exposes the recorded optional instance and
  responsiveness/pressure metrics. The original event preserves the control
  reason, previous/current level, recovery and optional deadline disposition.
  A missing deadline disposition means no disposition was recorded.
  `magnitude_ms` is null because the existing clock-jump payload has no magnitude.
- `observation.kind: "resource_sample"` exposes the persisted summary's sample
  time, `ledger_commits`, and foreground/owned/third-party process memory rows.
  These rows retain PID, process name, ownership, current working-set bytes,
  optional OS peak working-set bytes, and optional process creation FILETIME.
  Missing optional source fields become explicit JSON null in the projection.
- Every row has `thread_identity: null`: these payloads do not identify an OS
  thread. Instance ids, origin modules and free text are not thread evidence.
  Null observation metrics mean unknown/unrecorded, not a measured zero.

`summary_count`, `stutter_count`, and `clock_jump_count` are page-local counts of these typed rows.
No thread attribution, cross-page aggregate, inferred latency, clock-jump size,
or additional host observation is produced.

## Process memory source

HostMetrics reads `WorkingSetSize` and `PeakWorkingSetSize` from the same Windows
`GetProcessMemoryInfo` call. The latter is the OS process-lifetime high-water
value in bytes. It is paired with the creation time already read by
`GetProcessTimes`: `process_created_at_windows_100ns` is the unsigned FILETIME
count of 100-nanosecond intervals since 1601-01-01 UTC. PID plus creation time
identifies the sampled process lifetime. The peak is passed through unchanged;
the sampling history does not calculate it. Existing process-access coverage
and sampler-unavailable paths retain their failure meanings. Unsupported or
missing observations have no peak or creation value.

The optional peak and creation fields are paired and validated. A recorded
peak must be at least the current working set. Old `perf.summary` events can
omit them, and are read without migration or a change to
`GLOBAL_EVENT_SCHEMA_VERSION`.

Native definitions: [PROCESS_MEMORY_COUNTERS](https://learn.microsoft.com/en-us/windows/win32/api/psapi/ns-psapi-process_memory_counters)
and [GetProcessTimes](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-getprocesstimes).

## Commit sampling

GlobalLedger's current writer owns an in-memory correlation identity, an
`Instant` origin, successful commit count, committed-through sequence, cumulative
write-and-sync nanoseconds and the writer-lifetime maximum of that duration.
The identity is informational and uses the existing identifier issuer. Counters
start with this writer lifetime; reopening creates a new identity.

`persist_event` measures precisely its `write_all` plus `sync_all`, after
serialization and any segment rotation, and updates counters only when both
operations succeed. Original append/sync failures remain fatal and do not add
a successful commit. Counter or clock arithmetic that cannot be represented
marks statistics unavailable. Statistics are published under a short lock after
I/O; readers use `try_lock`, never the writer command queue, and retain the
existing nonblocking writer-health check. The statistics lock spans no I/O or
PerformanceMonitor work.

PerformanceMonitor samples these counters only when its existing periodic
summary is due, before appending that tick's events. Its own summary is therefore
included in the next window. There is one previous sample, no additional queue,
thread, per-commit event or persistent statistics store.

The optional `ledger_commits` field in `perf.summary` has `status: "available"`
with a `window`, or `status: "unavailable"` with an explicit reason. Available
windows bind one writer identity, start/end monotonic nanoseconds relative to
that writer, first/last successful commit sequences, successful commit count,
and the window's write-and-sync duration total. The maximum is explicitly named
`writer_lifetime_write_sync_max_ns`; it covers the writer lifetime at the end
sample. The integer rate is
`floor(successful_commits * 1_000_000_000_000 / window_nanoseconds)` in
`commits_per_second_milli`. Zero commits in a positive window produce a real
zero rate, zero duration total and null first/last sequence.

The window must be positive and at most one hour. No baseline, busy sampling,
counter overflow, writer change, nonpositive time, an over-wide window and a
counter reset remain explicit unavailable states. A missing sample clears the
baseline. A valid new sample establishes the next baseline even when it cannot
form a window with its predecessor. Old summaries lacking `ledger_commits`
project null. Forensic commands read persisted samples without contacting a
live process or Runtime.

## Physical storage snapshot

`open` adds `storage_snapshot`; plain human `export` adds a JSON-valued
`storage_snapshot:` line. Both copy GlobalLedger's existing read-only snapshot:

- `segment_count` counts the actual files in the one validated segment listing.
- `observed_bytes` sums each listed file's length when opened for that read.
- `read_bytes` sums the bytes actually returned by those bounded reads, including
  a corrupt tail and later physical segments whose events were not validated.
- `verified_prefix_bytes` covers only the contiguous validated event prefix.
- `read_complete` records whether every bounded read reached its observed
  length. A short read remains incomplete, including when it ends at a newline.
- `atomic` is false: listing and per-file reads are not an atomic view of a live
  writer. Segments created after listing and bytes appended beyond a file's
  observed length are outside this snapshot.

These are logical file byte lengths, separate from valid event count/sequence,
artifact storage, repair/quarantine files and allocated filesystem blocks.
The owner performs no extra listing, source rescan or repair for these fields.
