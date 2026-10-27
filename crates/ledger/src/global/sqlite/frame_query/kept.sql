-- SPDX-License-Identifier: AGPL-3.0-only
-- #422 LN-7c Q (rd-1026/ln7c/final-query.md §1.2, §4), statement K, per chunk of the kept listing:
-- the evidence-frame rule. A frame in kept\ is removed when an agent or a person confirmed reading
-- it (the frame, or its run, at or after its capture), or once it is :max_age_ms old by its
-- ingestion time (`frame_retention_evidence_max_age_days`, 15 days by default). :kept is a JSON
-- array of artifact ids; a kept file whose artifact has no `artifact.verified` row in this ledger
-- gives no row. Integers are ordered-u64 (`encode`).
WITH
kept(artifact_id) AS (SELECT value FROM json_each(:kept)),
k AS MATERIALIZED (
  SELECT a.artifact_id, a.frame_id, a.run_id, a.created_at_unix_ms AS at
  FROM kept
  CROSS JOIN ledger_artifacts a INDEXED BY ledger_artifacts_artifact ON a.artifact_id = kept.artifact_id
  CROSS JOIN ledger_events e ON e.sequence = a.sequence
  WHERE a.kind = 'capture.frame' AND +e.event_type = 'artifact.verified'
)
SELECT k.artifact_id, k.at, k.run_id,
  CASE WHEN k.at + :max_age_ms <= :now THEN 'remove_expired'
       WHEN EXISTS (SELECT 1 FROM ledger_frame_refs cr CROSS JOIN ledger_events ce ON ce.sequence = cr.sequence
                    WHERE cr.target_kind = 'artifact' AND cr.target_id = k.artifact_id
                      AND cr.pointer = 'read_confirmed' AND ce.timestamp_unix_ms >= k.at)
         OR EXISTS (SELECT 1 FROM ledger_frame_refs cr CROSS JOIN ledger_events ce ON ce.sequence = cr.sequence
                    WHERE cr.target_kind = 'run' AND cr.target_id = k.run_id
                      AND cr.pointer = 'read_confirmed' AND ce.timestamp_unix_ms >= k.at)
       THEN 'remove_confirmed'
       ELSE 'keep' END AS action
FROM k
ORDER BY k.artifact_id
