-- SPDX-License-Identifier: AGPL-3.0-only
-- Logical u64 columns use ordered-u64-v1 at the private SQL boundary.
CREATE TABLE ledger_events (
    sequence INTEGER PRIMARY KEY,
    event_id TEXT UNIQUE NOT NULL,
    timestamp_unix_ms INTEGER NOT NULL,
    event_type TEXT NOT NULL,
    severity TEXT NOT NULL,
    sensitivity TEXT NOT NULL,
    origin_source TEXT NOT NULL,
    origin_module TEXT NOT NULL,
    origin_actor TEXT NOT NULL,
    payload_schema TEXT NOT NULL,
    canonical_record BLOB NOT NULL,
    record_sha256 TEXT NOT NULL,
    previous_record_sha256 TEXT,
    integrity_tag TEXT NOT NULL
) STRICT;
CREATE TABLE ledger_links (
    sequence INTEGER PRIMARY KEY REFERENCES ledger_events(sequence),
    instance_id TEXT, request_id TEXT, correlation_id TEXT, causation_id TEXT,
    task_id TEXT, run_id TEXT, lease_id TEXT, frame_id TEXT, action_id TEXT,
    recognition_id TEXT
) STRICT;
CREATE INDEX ledger_links_instance ON ledger_links(instance_id, sequence);
CREATE INDEX ledger_links_request ON ledger_links(request_id, sequence);
CREATE INDEX ledger_links_correlation ON ledger_links(correlation_id, sequence);
CREATE INDEX ledger_links_causation ON ledger_links(causation_id, sequence);
CREATE INDEX ledger_links_task ON ledger_links(task_id, sequence);
CREATE INDEX ledger_links_run ON ledger_links(run_id, sequence);
CREATE INDEX ledger_links_lease ON ledger_links(lease_id, sequence);
CREATE INDEX ledger_links_frame ON ledger_links(frame_id, sequence);
CREATE INDEX ledger_links_action ON ledger_links(action_id, sequence);
CREATE INDEX ledger_links_recognition ON ledger_links(recognition_id, sequence);
CREATE INDEX ledger_events_sensitivity ON ledger_events(sensitivity, sequence);
CREATE TABLE ledger_artifacts (
    sequence INTEGER NOT NULL REFERENCES ledger_events(sequence),
    ordinal INTEGER NOT NULL,
    artifact_id TEXT NOT NULL, kind TEXT NOT NULL,
    run_id TEXT, frame_id TEXT, correlation_id TEXT,
    object_key TEXT NOT NULL, media_type TEXT NOT NULL,
    byte_count INTEGER NOT NULL, sha256 TEXT NOT NULL,
    created_at_unix_ms INTEGER NOT NULL, producer TEXT NOT NULL,
    retention_class TEXT NOT NULL, redaction_state TEXT NOT NULL,
    PRIMARY KEY (sequence, ordinal)
) STRICT;
-- #422 LN-5 (LEAN-L §3.3): artifact identities by their index; an older database gets it at
-- its first writer open (`CREATE INDEX IF NOT EXISTS`).
CREATE INDEX ledger_artifacts_artifact ON ledger_artifacts(artifact_id, sequence);
CREATE TABLE ledger_meta (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    schema_version TEXT NOT NULL,
    next_sequence INTEGER NOT NULL,
    head_sequence INTEGER NOT NULL,
    head_record_sha256 TEXT,
    storage_backend TEXT NOT NULL,
    migration_id TEXT,
    cutover_state TEXT NOT NULL,
    integer_encoding TEXT NOT NULL,
    integrity_tag TEXT NOT NULL,
    migration_record TEXT
) STRICT;
-- Workflow #383 AO-2 (ledger observer §11.1, register L11): the index of tagged keys, open
-- intents, write requests and lifecycle records, maintained in each batch's transaction.
CREATE TABLE ledger_index (
    role TEXT NOT NULL CHECK (role IN ('state', 'open', 'request', 'lifecycle')),
    key TEXT NOT NULL,
    sequence INTEGER NOT NULL,
    event_id TEXT NOT NULL,
    value_sha256 TEXT NOT NULL,
    source_lineage TEXT,
    source_revision INTEGER,
    PRIMARY KEY (role, key)
) STRICT, WITHOUT ROWID;
-- #422 LN-7c R (final-query.md §3): the frame index links of the provenance pointers a record
-- carries only in its payload, written in the record's batch transaction, and the sequences whose
-- records' links are all written (`from_sequence..=through_sequence`). An older database gets them
-- at its first writer open (`IF NOT EXISTS`), which fills them. No `ledger_view_*` name.
CREATE TABLE IF NOT EXISTS ledger_frame_refs (
    target_kind TEXT NOT NULL CHECK (target_kind IN ('frame', 'artifact', 'run')),
    target_id TEXT NOT NULL,
    sequence INTEGER NOT NULL,
    pointer TEXT NOT NULL CHECK (pointer IN ('summary_frame', 'summary_pin', 'published_artifact',
        'published_frame', 'reading', 'before_frame', 'preserved_frame', 'read_confirmed')),
    run_id TEXT,
    PRIMARY KEY (target_kind, target_id, pointer, sequence)
) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS ledger_frame_refs_sequence ON ledger_frame_refs(sequence);
CREATE TABLE IF NOT EXISTS ledger_frame_refs_extent (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    from_sequence INTEGER NOT NULL,
    through_sequence INTEGER NOT NULL
) STRICT;
