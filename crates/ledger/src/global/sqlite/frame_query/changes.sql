-- SPDX-License-Identifier: AGPL-3.0-only
-- Workflow #503 FR-1, statement C, once per pass after a process's first completed pass: what was
-- recorded after :after (the head statement W of the last completed pass read at) that can end
-- the wait of a running frame or the keep of a kept frame: the runs a terminal or a capture
-- summary names, the leases a release names, the frames whose capture completed, the frames and
-- runs a read confirmation names, and whether a start or a takeover was recorded. It reads only
-- the records after :after: the type index and the frame index's sequence index, each from
-- :after on, and the link row of each record found. Integers are ordered-u64 (`encode`).
SELECT 'run' AS kind, l.run_id AS id
FROM ledger_events e INDEXED BY ledger_view_index_type_v1
CROSS JOIN ledger_links l ON l.sequence = e.sequence
WHERE e.event_type IN ('task.completed', 'task.failed', 'task.cancelled', 'capture.summary_committed')
  AND e.sequence > :after AND l.run_id IS NOT NULL
UNION ALL
SELECT 'lease', l.lease_id
FROM ledger_events e INDEXED BY ledger_view_index_type_v1
CROSS JOIN ledger_links l ON l.sequence = e.sequence
WHERE e.event_type = 'lease.released' AND e.sequence > :after AND l.lease_id IS NOT NULL
UNION ALL
SELECT 'frame', l.frame_id
FROM ledger_events e INDEXED BY ledger_view_index_type_v1
CROSS JOIN ledger_links l ON l.sequence = e.sequence
WHERE e.event_type = 'capture.completed' AND e.sequence > :after AND l.frame_id IS NOT NULL
UNION ALL
SELECT CASE WHEN cr.target_kind = 'artifact' THEN 'confirmed_artifact' ELSE 'confirmed_run' END,
       cr.target_id
FROM ledger_frame_refs cr INDEXED BY ledger_frame_refs_sequence
WHERE cr.sequence > :after AND cr.pointer = 'read_confirmed'
  AND cr.target_kind IN ('artifact', 'run')
UNION ALL
SELECT 'start', NULL
FROM ledger_events s INDEXED BY ledger_view_index_type_v1
WHERE s.event_type IN ('runtime.started', 'runtime.takeover') AND s.sequence > :after
