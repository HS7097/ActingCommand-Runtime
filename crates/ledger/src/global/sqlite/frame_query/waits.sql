-- SPDX-License-Identifier: AGPL-3.0-only
-- #422 LN-7c Q (rd-1026/ln7c/final-query.md §1.2), statement W, once per pass: the open runs of
-- the current owner epoch, each with its instance and first frame time. A frame of that instance
-- captured at most 30 s before an open run's first frame waits (running). Bounded by the frames
-- and the terminals of the current epoch. Integers are ordered-u64 (`encode`).
WITH
-- the current epoch's start: two one-entry seeks (MAX on the type index), then its row
st AS MATERIALIZED (
  SELECT e.sequence AS s, e.timestamp_unix_ms AS t
  FROM ledger_events e
  WHERE e.sequence = MAX(
    COALESCE((SELECT MAX(sx.sequence) FROM ledger_events sx INDEXED BY ledger_view_index_type_v1
              WHERE sx.event_type = 'runtime.started'), -9223372036854775808),
    COALESCE((SELECT MAX(sy.sequence) FROM ledger_events sy INDEXED BY ledger_view_index_type_v1
              WHERE sy.event_type = 'runtime.takeover'), -9223372036854775808))
),
cand AS MATERIALIZED (
  SELECT a.run_id, MIN(a.created_at_unix_ms) AS first_at
  FROM st
  CROSS JOIN ledger_artifacts a INDEXED BY ledger_artifacts_created
  WHERE a.kind = 'capture.frame' AND a.created_at_unix_ms >= st.t AND a.run_id IS NOT NULL
  GROUP BY a.run_id
),
term AS MATERIALIZED (
  SELECT DISTINCT tl.run_id
  FROM st
  CROSS JOIN ledger_events te INDEXED BY ledger_view_index_type_v1
  CROSS JOIN ledger_links tl ON tl.sequence = te.sequence
  WHERE te.event_type IN ('task.completed', 'task.failed', 'task.cancelled') AND te.sequence > st.s
    AND tl.run_id IS NOT NULL
    AND json_extract(CAST(te.canonical_record AS TEXT), '$.payload.payload.kind') = 'semantic'
)
SELECT
  (SELECT il.instance_id FROM ledger_links il INDEXED BY ledger_links_run
   WHERE il.run_id = c.run_id AND il.instance_id IS NOT NULL ORDER BY il.sequence LIMIT 1) AS instance_id,
  c.first_at
FROM cand c CROSS JOIN st
WHERE c.run_id NOT IN (SELECT run_id FROM term)
  AND (SELECT MIN(fl.sequence) FROM ledger_links fl INDEXED BY ledger_links_run WHERE fl.run_id = c.run_id) > st.s
  AND NOT EXISTS (SELECT 1 FROM ledger_links dl INDEXED BY ledger_links_run
                  CROSS JOIN ledger_events de ON de.sequence = dl.sequence
                  WHERE dl.run_id = c.run_id AND +de.event_type = 'task.requested'
                    AND json_extract(CAST(de.canonical_record AS TEXT), '$.payload.payload.data.action')
                        = 'runtime.debug_package')
