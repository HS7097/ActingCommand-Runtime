# ADB target recovery during input open

The Runtime backend registry enables target recovery only for its explicit ADB
shell input backend. The execution session opens that backend inside the existing
Scheduler-admitted and fenced input operation. General touch probes, capture,
application control and ordinary ADB callers retain their existing connection
entry. There is no background recovery owner or persistent recovery configuration.

With connect enabled and an explicit HOST:PORT serial, the input owner observes
get-state, performs the original connect and observes get-state again. Device
state completes connection; command exit success alone does not. A targeted
disconnect/connect is permitted once only when the post-connect state explicitly
identifies offline, the original connect command succeeded, and child cleanup has
no unconfirmed resource. An unauthorized observation, unknown post-connect state
or non-TCP target cannot authorize this sequence. Every transport command names
the same serial. The first query's error remains evidence even when the later
query establishes an unambiguous current offline state.

The owner uses its configured ADB command_timeout as one connection deadline,
passing only the remaining time to each existing child command. After the single
disconnect/connect, it reads state at most twice, with at most 100ms before the
second read and within that deadline. Only a still-offline first verification
permits the second. Child cleanup retains its original bounded close behavior.
No command is started after deadline exhaustion. Geometry, rotation, bounds,
gesture timeouts and the single action submission keep their existing rules.

The connection result retains at most seven stage observations. Endpoint, initial
error and each command's stdout/stderr/error text are capped at 1024 UTF-8 bytes
with explicit truncation and lossy-decode flags. Phase, attempt, result code,
elapsed time, chosen path and last observed transport state remain typed fields.
No periodic samples are collected. An unsuccessful or unconfirmed transport
operation does not establish device state, release or an external root cause.

Kernel carries the one-time open report with the input result or original error.
Host commits a WARNING lifecycle observation for successful transport recovery,
including when the later geometry/action fails. It does so before input.committed
on a successful action. Failed recovery retains its report in the original fatal
lifecycle record and stops before an input backend is returned. Ledger failure
is a Runtime failure, not a successful warning or input receipt. All original
lease and resource-close handling remains applicable.

Reports use the existing Runtime lifecycle event family and owner epoch/request/
instance/lease/action links. Raw endpoint and command details are Sensitive and
absent from public projections. The report establishes only the observed target
transport sequence; it does not diagnose a past outage or other process ownership.
