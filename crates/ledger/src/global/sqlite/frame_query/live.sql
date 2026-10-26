-- SPDX-License-Identifier: AGPL-3.0-only
-- #422 LN-7c Q (rd-1026/ln7c/final-query.md §1.2), statement L, per chunk of the live listing (at
-- most `ledger_observer.frame_query_chunk_frames` frames): each frame's class and action at :now,
-- from index probes keyed by the chunk's frames, their runs, the runs with frames in their 30 s
-- windows (F6 index), their scopes and their frame links (F5 table). :live is a JSON array of
-- artifact ids, :waits statement W's answer, [[instance_id, first_at], ...]. A frame not settled
-- (captured less than :settle_ms before :now), without a pin, with an eviction intent, or unknown
-- to the ledger gives no row. Integers are ordered-u64 (`encode`).
WITH
live(artifact_id) AS (SELECT value FROM json_each(:live)),
waits(instance_id, first_at) AS (
  SELECT json_extract(value, '$[0]'), json_extract(value, '$[1]') FROM json_each(:waits)),
-- the frames past their earliest action age (settle, 60 s), with a pin and no eviction intent
f AS MATERIALIZED (
  SELECT a.artifact_id, a.frame_id, a.created_at_unix_ms AS at,
         l.instance_id, l.run_id, l.lease_id, l.request_id, l.correlation_id
  FROM live
  CROSS JOIN ledger_artifacts a INDEXED BY ledger_artifacts_artifact ON a.artifact_id = live.artifact_id
  CROSS JOIN ledger_events e ON e.sequence = a.sequence
  CROSS JOIN ledger_links l ON l.sequence = a.sequence
  WHERE a.kind = 'capture.frame' AND +e.event_type = 'artifact.verified'
    AND a.created_at_unix_ms <= :now - :settle_ms
    AND EXISTS (SELECT 1 FROM ledger_links pl INDEXED BY ledger_links_frame
                CROSS JOIN ledger_events pe ON pe.sequence = pl.sequence
                WHERE pl.frame_id = a.frame_id AND +pe.event_type = 'artifact.pin_recorded')
    AND NOT EXISTS (SELECT 1 FROM ledger_links xl INDEXED BY ledger_links_frame
                CROSS JOIN ledger_events xe ON xe.sequence = xl.sequence
                WHERE xl.frame_id = a.frame_id AND +xe.event_type = 'artifact.eviction_intent')
),
-- the runs to decide: each frame's own run and every run with a frame on its instance in
-- [at, at + 30 s] (F6: a cut run with no live frame left still dates its epoch-end point)
wr AS MATERIALIZED (
  SELECT wa.run_id AS run_id
  FROM f
  CROSS JOIN ledger_artifacts wa INDEXED BY ledger_artifacts_created
  CROSS JOIN ledger_links wl ON wl.sequence = wa.sequence
  WHERE wa.kind = 'capture.frame' AND wa.created_at_unix_ms BETWEEN f.at AND f.at + :window_ms
    AND wa.run_id IS NOT NULL AND +wl.instance_id = f.instance_id
  UNION SELECT run_id FROM f WHERE run_id IS NOT NULL
),
-- one walk over each run's link rows
rw AS MATERIALIZED (
  SELECT r.run_id,
    MIN(rl.sequence) AS first_seq,
    MAX(CASE WHEN re.event_type = 'capture.summary_committed' THEN rl.sequence END) AS summary_seq,
    MAX(CASE WHEN re.event_type = 'task.requested'
              AND json_extract(CAST(re.canonical_record AS TEXT), '$.payload.payload.data.action')
                  = 'runtime.debug_package' THEN 1 ELSE 0 END) AS debug,
    MIN(CASE WHEN re.event_type IN ('task.completed', 'task.failed', 'task.cancelled')
             THEN rl.sequence END) AS any_term_seq,
    MIN(CASE WHEN re.event_type IN ('task.completed', 'task.failed', 'task.cancelled')
              AND json_extract(CAST(re.canonical_record AS TEXT), '$.payload.payload.kind') = 'semantic'
             THEN rl.sequence END) AS sem_term_seq,
    MIN(ra.created_at_unix_ms) AS first_at,
    MAX(ra.created_at_unix_ms) AS last_at,
    MIN(CASE WHEN rl.instance_id IS NOT NULL THEN rl.sequence END) AS instance_seq
  FROM wr r
  CROSS JOIN ledger_links rl INDEXED BY ledger_links_run ON rl.run_id = r.run_id
  CROSS JOIN ledger_events re ON re.sequence = rl.sequence
  LEFT JOIN ledger_artifacts ra ON ra.sequence = rl.sequence AND ra.kind = 'capture.frame'
  GROUP BY r.run_id
),
rx AS MATERIALIZED (
  SELECT w.*,
    CASE WHEN w.debug THEN w.any_term_seq ELSE w.sem_term_seq END AS term_seq,
    (SELECT MIN(se.sequence) FROM ledger_events se INDEXED BY ledger_view_index_type_v1
     WHERE se.event_type IN ('runtime.started', 'runtime.takeover') AND se.sequence > w.first_seq) AS end_seq,
    (SELECT il.instance_id FROM ledger_links il WHERE il.sequence = w.instance_seq) AS instance_id
  FROM rw w
),
ry AS MATERIALIZED (
  SELECT r.*,
    (SELECT timestamp_unix_ms FROM ledger_events WHERE sequence = r.term_seq) AS term_at,
    (SELECT timestamp_unix_ms FROM ledger_events WHERE sequence = r.end_seq) AS end_at,
    (r.end_seq IS NOT NULL AND NOT (r.term_seq IS NOT NULL AND r.term_seq < r.end_seq)
     AND r.last_at IS NOT NULL) AS cut
  FROM rx r
),
-- Lab uses in the scopes of the frames' runs
lab_runs AS MATERIALIZED (
  SELECT x.run_id FROM (SELECT DISTINCT run_id FROM f WHERE run_id IS NOT NULL) x
  WHERE EXISTS (SELECT 1 FROM ledger_links al INDEXED BY ledger_links_request
                CROSS JOIN ledger_events ae ON ae.sequence = al.sequence
                WHERE al.request_id IN (SELECT rl.request_id FROM ledger_links rl INDEXED BY ledger_links_run
                                        WHERE rl.run_id = x.run_id AND rl.request_id IS NOT NULL)
                  AND (+ae.origin_source = 'lab' OR +ae.event_type = 'lab.request'))
     OR EXISTS (SELECT 1 FROM ledger_links al INDEXED BY ledger_links_correlation
                CROSS JOIN ledger_events ae ON ae.sequence = al.sequence
                WHERE al.correlation_id IN (SELECT rl.correlation_id FROM ledger_links rl INDEXED BY ledger_links_run
                                            WHERE rl.run_id = x.run_id AND rl.correlation_id IS NOT NULL)
                  AND (+ae.origin_source = 'lab' OR +ae.event_type = 'lab.request'))
),
-- entry: the run's close (terminal, summary and a later Performed release, all in its epoch),
-- else its epoch's end; a run-less frame at its capture once capture.completed exists
fe AS MATERIALIZED (
  SELECT f.*, r.summary_seq, (lr.run_id IS NOT NULL) AS lab_run,
    CASE WHEN f.run_id IS NULL THEN
      CASE WHEN EXISTS (SELECT 1 FROM ledger_links cl INDEXED BY ledger_links_frame
                        CROSS JOIN ledger_events ce ON ce.sequence = cl.sequence
                        WHERE cl.frame_id = f.frame_id AND cl.run_id IS NULL
                          AND +ce.event_type = 'capture.completed') THEN f.at END
    ELSE (
      SELECT CASE WHEN c.closed IS NOT NULL AND (r.end_seq IS NULL OR c.closed <= r.end_seq) THEN r.term_at
                  ELSE r.end_at END
      FROM (SELECT MAX(r.term_seq, r.summary_seq,
                       CASE WHEN f.lease_id IS NULL THEN r.term_seq ELSE
                         (SELECT MIN(ll.sequence) FROM ledger_links ll INDEXED BY ledger_links_lease
                          CROSS JOIN ledger_events le ON le.sequence = ll.sequence
                          WHERE ll.lease_id = f.lease_id AND ll.sequence > r.term_seq
                            AND (ll.run_id IS NULL OR ll.run_id = f.run_id) AND +le.event_type = 'lease.released'
                            AND json_extract(CAST(le.canonical_record AS TEXT),
                                             '$.payload.payload.data.effect_disposition') = 'performed') END) AS closed) c
    ) END AS entry
  FROM f LEFT JOIN ry r ON r.run_id = f.run_id LEFT JOIN lab_runs lr ON lr.run_id = f.run_id
),
-- marker neighbours (F5: the preserved frame is a link now). A marker counts only when it is the
-- first marker of its representative and the first of its preserved frame.
nb AS MATERIALIZED (
  SELECT fe.artifact_id,
    (SELECT pm.sequence FROM ledger_frame_refs pm
     WHERE pm.target_kind = 'frame' AND pm.target_id = fe.frame_id AND pm.pointer = 'preserved_frame'
     ORDER BY pm.sequence LIMIT 1) AS pred_marker,
    (SELECT sl.sequence FROM ledger_links sl INDEXED BY ledger_links_frame
     CROSS JOIN ledger_events se ON se.sequence = sl.sequence
     WHERE sl.frame_id = fe.frame_id AND +se.event_type = 'capture.dedup_window'
     ORDER BY sl.sequence LIMIT 1) AS succ_marker
  FROM fe
),
nb2 AS MATERIALIZED (
  SELECT n.artifact_id,
    (SELECT ml.frame_id FROM ledger_links ml WHERE ml.sequence = n.pred_marker) AS pred_frame,
    (SELECT sr.target_id FROM ledger_frame_refs sr INDEXED BY ledger_frame_refs_sequence
     WHERE sr.sequence = n.succ_marker AND sr.pointer = 'preserved_frame' LIMIT 1) AS succ_frame,
    n.pred_marker, n.succ_marker
  FROM nb n
),
nb3 AS MATERIALIZED (
  SELECT n.artifact_id,
    (n.pred_marker IS NOT NULL AND n.pred_marker =
       (SELECT MIN(xl.sequence) FROM ledger_links xl INDEXED BY ledger_links_frame
        CROSS JOIN ledger_events xe ON xe.sequence = xl.sequence
        WHERE xl.frame_id = n.pred_frame AND +xe.event_type = 'capture.dedup_window')) AS pred_ok,
    (n.succ_marker IS NOT NULL AND n.succ_marker =
       (SELECT MIN(xr.sequence) FROM ledger_frame_refs xr
        WHERE xr.target_kind = 'frame' AND xr.target_id = n.succ_frame AND xr.pointer = 'preserved_frame')) AS succ_ok,
    (SELECT na.created_at_unix_ms FROM ledger_links nl INDEXED BY ledger_links_frame
     CROSS JOIN ledger_artifacts na ON na.sequence = nl.sequence
     WHERE nl.frame_id = n.pred_frame AND na.kind = 'capture.frame' LIMIT 1) AS prev_at,
    (SELECT na.created_at_unix_ms FROM ledger_links nl INDEXED BY ledger_links_frame
     CROSS JOIN ledger_artifacts na ON na.sequence = nl.sequence
     WHERE nl.frame_id = n.succ_frame AND na.kind = 'capture.frame' LIMIT 1) AS next_at
  FROM nb2 n
),
-- an error point that names the frame: a frame link, a declared artifact, or a frame index link
named AS MATERIALIZED (
  SELECT fe.artifact_id FROM fe
  WHERE EXISTS (SELECT 1 FROM ledger_links l INDEXED BY ledger_links_frame
                CROSS JOIN ledger_events e ON e.sequence = l.sequence
                WHERE l.frame_id = fe.frame_id
                  AND e.payload_schema <> 'actingcommand.payload.artifact_retention.v1'
                  AND (+e.event_type = 'task.failed' OR +e.severity IN ('warning', 'error', 'fatal')))
     OR EXISTS (SELECT 1 FROM ledger_artifacts na INDEXED BY ledger_artifacts_artifact
                CROSS JOIN ledger_events e ON e.sequence = na.sequence
                WHERE na.artifact_id = fe.artifact_id
                  AND (+e.event_type = 'task.failed' OR +e.severity IN ('warning', 'error', 'fatal')))
     OR EXISTS (SELECT 1 FROM ledger_frame_refs nr CROSS JOIN ledger_events e ON e.sequence = nr.sequence
                WHERE nr.target_kind = 'frame' AND nr.target_id = fe.frame_id
                  AND (+e.event_type = 'task.failed' OR +e.severity IN ('warning', 'error', 'fatal')))
     OR EXISTS (SELECT 1 FROM ledger_frame_refs nr CROSS JOIN ledger_events e ON e.sequence = nr.sequence
                WHERE nr.target_kind = 'artifact' AND nr.target_id = fe.artifact_id
                  AND (+e.event_type = 'task.failed' OR +e.severity IN ('warning', 'error', 'fatal')))
),
dx AS MATERIALIZED (
  SELECT fe.*, n.prev_at, n.next_at, (nm.artifact_id IS NOT NULL) AS named,
    (n.pred_ok AND n.succ_ok AND NOT (fe.run_id IS NOT NULL AND fe.summary_seq IS NULL)
     AND nm.artifact_id IS NULL
     AND NOT EXISTS (SELECT 1 FROM ledger_frame_refs xr
                     WHERE xr.target_kind = 'artifact' AND xr.target_id = fe.artifact_id
                       AND xr.pointer IN ('summary_pin', 'published_artifact'))
     AND NOT EXISTS (SELECT 1 FROM ledger_frame_refs xr
                     WHERE xr.target_kind = 'frame' AND xr.target_id = fe.frame_id
                       AND xr.pointer IN ('before_frame', 'published_frame', 'reading'))
     AND NOT (fe.lease_id IS NOT NULL
              AND NOT EXISTS (SELECT 1 FROM ledger_frame_refs sr
                              WHERE sr.target_kind = 'artifact' AND sr.target_id = fe.artifact_id
                                AND sr.pointer = 'summary_frame'))) AS duplicate
  FROM fe JOIN nb3 n ON n.artifact_id = fe.artifact_id
  LEFT JOIN named nm ON nm.artifact_id = fe.artifact_id
),
-- materialized as every CTE is (review F2): flattened, each CASE below that names a column ran
-- its correlated subqueries again
decided AS MATERIALIZED (
  SELECT d.artifact_id, d.at, d.entry,
    (d.entry IS NULL
     OR EXISTS (SELECT 1 FROM waits w WHERE w.instance_id = d.instance_id AND w.first_at <= d.at + :window_ms)) AS running,
    ((d.lab_run
      OR EXISTS (SELECT 1 FROM ledger_links pl INDEXED BY ledger_links_frame
                 CROSS JOIN ledger_events pe ON pe.sequence = pl.sequence
                 WHERE pl.frame_id = d.frame_id AND +pe.event_type = 'artifact.pin_recorded'
                   AND json_extract(CAST(pe.canonical_record AS TEXT),
                                    '$.payload.payload.data.record.data.reason') = 'lab')
      OR (d.run_id IS NULL AND (
            EXISTS (SELECT 1 FROM ledger_links al INDEXED BY ledger_links_request
                    CROSS JOIN ledger_events ae ON ae.sequence = al.sequence
                    WHERE al.request_id = d.request_id AND (+ae.origin_source = 'lab' OR +ae.event_type = 'lab.request'))
         OR EXISTS (SELECT 1 FROM ledger_links al INDEXED BY ledger_links_correlation
                    CROSS JOIN ledger_events ae ON ae.sequence = al.sequence
                    WHERE al.correlation_id = d.correlation_id AND (+ae.origin_source = 'lab' OR +ae.event_type = 'lab.request'))))
      -- F5: Lab uses in the scopes of the records that name the frame only in their payload
      OR EXISTS (SELECT 1 FROM ledger_frame_refs fr CROSS JOIN ledger_links frl ON frl.sequence = fr.sequence
                 WHERE ((fr.target_kind = 'frame' AND fr.target_id = d.frame_id)
                        OR (fr.target_kind = 'artifact' AND fr.target_id = d.artifact_id))
                   AND fr.pointer <> 'read_confirmed'
                   -- a record of the frame's own run: its scopes are the run's (lab_runs)
                   AND (frl.run_id IS NULL OR d.run_id IS NULL OR frl.run_id <> d.run_id)
                   AND (EXISTS (SELECT 1 FROM ledger_links al INDEXED BY ledger_links_request
                                CROSS JOIN ledger_events ae ON ae.sequence = al.sequence
                                WHERE al.request_id = frl.request_id AND (+ae.origin_source = 'lab' OR +ae.event_type = 'lab.request'))
                        OR EXISTS (SELECT 1 FROM ledger_links al INDEXED BY ledger_links_correlation
                                CROSS JOIN ledger_events ae ON ae.sequence = al.sequence
                                WHERE al.correlation_id = frl.correlation_id AND (+ae.origin_source = 'lab' OR +ae.event_type = 'lab.request')))))
     AND NOT (:dedup_lab AND d.duplicate)) AS lab,
    -- kept by a window: a point on its instance in [at, at + 30 s], a point naming it, or the
    -- epoch-end point of a cut run whose last frame is in [at, at + 30 s]; with dedup_error, a
    -- window whose t has both marker neighbours within 30 s before it does not keep a duplicate
    (d.named
     OR EXISTS (SELECT 1 FROM ledger_events e INDEXED BY ledger_view_index_time_v1
                CROSS JOIN ledger_links l ON l.sequence = e.sequence
                WHERE e.timestamp_unix_ms BETWEEN d.at AND d.at + :window_ms
                  AND COALESCE(l.instance_id, (SELECT il.instance_id FROM ledger_links il INDEXED BY ledger_links_run
                                               WHERE il.run_id = l.run_id AND il.instance_id IS NOT NULL
                                               ORDER BY il.sequence LIMIT 1)) = d.instance_id
                  AND e.payload_schema <> 'actingcommand.payload.artifact_retention.v1'
                  AND e.event_type NOT GLOB 'perf.*'
                  AND ((e.event_type = 'task.failed'
                        AND json_extract(CAST(e.canonical_record AS TEXT), '$.payload.payload.kind') = 'semantic')
                       OR (l.run_id IS NOT NULL AND e.severity IN ('warning', 'error', 'fatal'))
                       OR (l.run_id IS NULL AND e.severity IN ('error', 'fatal')))
                  AND NOT COALESCE(:dedup_error AND d.duplicate
                           AND d.prev_at BETWEEN e.timestamp_unix_ms - :window_ms AND e.timestamp_unix_ms
                           AND d.next_at BETWEEN e.timestamp_unix_ms - :window_ms AND e.timestamp_unix_ms, 0))
     OR EXISTS (SELECT 1 FROM ry r
                WHERE r.cut AND r.instance_id = d.instance_id AND r.last_at BETWEEN d.at AND d.at + :window_ms
                  AND NOT COALESCE(:dedup_error AND d.duplicate
                           AND d.prev_at BETWEEN r.last_at - :window_ms AND r.last_at
                           AND d.next_at BETWEEN r.last_at - :window_ms AND r.last_at, 0))) AS error,
    EXISTS (SELECT 1 FROM ledger_frame_refs rr
            WHERE rr.target_kind = 'frame' AND rr.target_id = d.frame_id AND rr.pointer = 'reading'
              AND rr.run_id = d.run_id) AS resource,
    d.duplicate,
    (EXISTS (SELECT 1 FROM ledger_frame_refs cr CROSS JOIN ledger_events ce ON ce.sequence = cr.sequence
             WHERE cr.target_kind = 'artifact' AND cr.target_id = d.artifact_id
               AND cr.pointer = 'read_confirmed' AND ce.timestamp_unix_ms >= d.at)
     OR EXISTS (SELECT 1 FROM ledger_frame_refs cr CROSS JOIN ledger_events ce ON ce.sequence = cr.sequence
             WHERE cr.target_kind = 'run' AND cr.target_id = d.run_id
               AND cr.pointer = 'read_confirmed' AND ce.timestamp_unix_ms >= d.at)) AS confirmed
  FROM dx d
)
-- the class (in the view's order of precedence) and the action at :now
SELECT artifact_id, at,
  CASE WHEN running THEN 'running'
       WHEN lab THEN 'lab'
       WHEN error THEN 'error'
       WHEN resource THEN 'resource'
       WHEN duplicate THEN 'duplicate'
       ELSE 'default' END AS class,
  CASE WHEN running THEN 'running'
       WHEN (lab OR error) AND confirmed THEN 'remove_confirmed'
       WHEN lab THEN 'move_lab'
       WHEN error THEN 'move_error'
       WHEN resource THEN CASE WHEN at + :resource_ms <= :now THEN 'remove' ELSE 'keep_resource' END
       WHEN duplicate THEN 'remove_duplicate'
       ELSE CASE WHEN at + :default_ms <= :now THEN 'remove' ELSE 'keep' END
  END AS action
FROM decided
ORDER BY artifact_id
