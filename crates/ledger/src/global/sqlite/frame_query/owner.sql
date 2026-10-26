-- SPDX-License-Identifier: AGPL-3.0-only
-- #422 LN-7c Q (rd-1026/ln7c/final-query.md §1.2, E16): the owner point of one frame the cleaner
-- moves into an error folder, by keyed reads of that frame only: the lowest-sequence point whose
-- window holds the frame, among the points statement L reads for it (a point naming the frame, a
-- point on its instance in [at, at + 30 s], and the epoch-end point of a run cut by a start whose
-- last frame lies in [at, at + 30 s]). Answers its sequence, its time, whether it is an epoch end
-- with the type of the start that ended the epoch, and the sequence its instance's alias is read
-- at. :artifact, :frame and :instance are the frame's ids, :at its capture time. Integers are
-- ordered-u64 (`encode`).
WITH
named AS MATERIALIZED (
  SELECT e.sequence, e.timestamp_unix_ms AS at
  FROM ledger_links l INDEXED BY ledger_links_frame
  CROSS JOIN ledger_events e ON e.sequence = l.sequence
  WHERE l.frame_id = :frame
    AND e.payload_schema <> 'actingcommand.payload.artifact_retention.v1'
    AND (+e.event_type = 'task.failed' OR +e.severity IN ('warning', 'error', 'fatal'))
  UNION ALL
  SELECT e.sequence, e.timestamp_unix_ms
  FROM ledger_artifacts na INDEXED BY ledger_artifacts_artifact
  CROSS JOIN ledger_events e ON e.sequence = na.sequence
  WHERE na.artifact_id = :artifact
    AND (+e.event_type = 'task.failed' OR +e.severity IN ('warning', 'error', 'fatal'))
  UNION ALL
  SELECT e.sequence, e.timestamp_unix_ms
  FROM ledger_frame_refs nr CROSS JOIN ledger_events e ON e.sequence = nr.sequence
  WHERE nr.target_kind = 'frame' AND nr.target_id = :frame
    AND (+e.event_type = 'task.failed' OR +e.severity IN ('warning', 'error', 'fatal'))
  UNION ALL
  SELECT e.sequence, e.timestamp_unix_ms
  FROM ledger_frame_refs nr CROSS JOIN ledger_events e ON e.sequence = nr.sequence
  WHERE nr.target_kind = 'artifact' AND nr.target_id = :artifact
    AND (+e.event_type = 'task.failed' OR +e.severity IN ('warning', 'error', 'fatal'))
),
windowed AS MATERIALIZED (
  SELECT e.sequence, e.timestamp_unix_ms AS at
  FROM ledger_events e INDEXED BY ledger_view_index_time_v1
  CROSS JOIN ledger_links l ON l.sequence = e.sequence
  WHERE e.timestamp_unix_ms BETWEEN :at AND :at + :window_ms
    AND COALESCE(l.instance_id, (SELECT il.instance_id FROM ledger_links il INDEXED BY ledger_links_run
                                 WHERE il.run_id = l.run_id AND il.instance_id IS NOT NULL
                                 ORDER BY il.sequence LIMIT 1)) = :instance
    AND e.payload_schema <> 'actingcommand.payload.artifact_retention.v1'
    AND e.event_type NOT GLOB 'perf.*'
    AND ((e.event_type = 'task.failed'
          AND json_extract(CAST(e.canonical_record AS TEXT), '$.payload.payload.kind') = 'semantic')
         OR (l.run_id IS NOT NULL AND e.severity IN ('warning', 'error', 'fatal'))
         OR (l.run_id IS NULL AND e.severity IN ('error', 'fatal')))
),
wr AS MATERIALIZED (
  SELECT DISTINCT wa.run_id AS run_id
  FROM ledger_artifacts wa INDEXED BY ledger_artifacts_created
  CROSS JOIN ledger_links wl ON wl.sequence = wa.sequence
  WHERE wa.kind = 'capture.frame' AND wa.created_at_unix_ms BETWEEN :at AND :at + :window_ms
    AND wa.run_id IS NOT NULL AND +wl.instance_id = :instance
),
rw AS MATERIALIZED (
  SELECT r.run_id,
    MIN(rl.sequence) AS first_seq,
    MAX(CASE WHEN re.event_type = 'task.requested'
              AND json_extract(CAST(re.canonical_record AS TEXT), '$.payload.payload.data.action')
                  = 'runtime.debug_package' THEN 1 ELSE 0 END) AS debug,
    MIN(CASE WHEN re.event_type IN ('task.completed', 'task.failed', 'task.cancelled')
             THEN rl.sequence END) AS any_term_seq,
    MIN(CASE WHEN re.event_type IN ('task.completed', 'task.failed', 'task.cancelled')
              AND json_extract(CAST(re.canonical_record AS TEXT), '$.payload.payload.kind') = 'semantic'
             THEN rl.sequence END) AS sem_term_seq,
    MAX(ra.created_at_unix_ms) AS last_at,
    MIN(CASE WHEN rl.instance_id IS NOT NULL THEN rl.sequence END) AS instance_seq
  FROM wr r
  CROSS JOIN ledger_links rl INDEXED BY ledger_links_run ON rl.run_id = r.run_id
  CROSS JOIN ledger_events re ON re.sequence = rl.sequence
  LEFT JOIN ledger_artifacts ra ON ra.sequence = rl.sequence AND ra.kind = 'capture.frame'
  GROUP BY r.run_id
),
rx AS MATERIALIZED (
  SELECT w.run_id, w.last_at,
    CASE WHEN w.debug THEN w.any_term_seq ELSE w.sem_term_seq END AS term_seq,
    (SELECT MIN(se.sequence) FROM ledger_events se INDEXED BY ledger_view_index_type_v1
     WHERE se.event_type IN ('runtime.started', 'runtime.takeover') AND se.sequence > w.first_seq) AS end_seq,
    (SELECT il.instance_id FROM ledger_links il WHERE il.sequence = w.instance_seq) AS instance_id
  FROM rw w
),
cut AS MATERIALIZED (
  SELECT r.end_seq AS sequence, r.last_at AS at,
    (SELECT ee.event_type FROM ledger_events ee WHERE ee.sequence = r.end_seq) AS end_type,
    (SELECT MAX(la.sequence) FROM ledger_links ll INDEXED BY ledger_links_run
     CROSS JOIN ledger_artifacts la ON la.sequence = ll.sequence
     CROSS JOIN ledger_events le ON le.sequence = la.sequence
     WHERE ll.run_id = r.run_id AND la.kind = 'capture.frame' AND la.created_at_unix_ms = r.last_at
       AND +le.event_type = 'artifact.verified') AS alias_sequence
  FROM rx r
  WHERE r.end_seq IS NOT NULL AND NOT (r.term_seq IS NOT NULL AND r.term_seq < r.end_seq)
    AND r.last_at IS NOT NULL AND r.instance_id = :instance
    AND r.last_at BETWEEN :at AND :at + :window_ms
)
SELECT sequence, at, 0 AS epoch_end, NULL AS end_type, sequence AS alias_sequence FROM named
UNION ALL
SELECT sequence, at, 0, NULL, sequence FROM windowed
UNION ALL
SELECT sequence, at, 1, end_type, COALESCE(alias_sequence, sequence) FROM cut
ORDER BY 1, 2
LIMIT 1
