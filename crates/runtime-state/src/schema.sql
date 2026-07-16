-- SPDX-License-Identifier: AGPL-3.0-only

-- This authoritative schema is embedded by crates/runtime-state and mirrored at
-- contracts/sqlite/schema.sql; the two files must remain byte-for-byte equivalent.

PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS state_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
) STRICT;

INSERT OR IGNORE INTO state_meta (key, value)
VALUES ('schema_version', 'actingcommand.runtime-state.v1');

CREATE TABLE IF NOT EXISTS state_document_history (
    state_key TEXT NOT NULL,
    schema_version TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision > 0),
    payload BLOB NOT NULL,
    payload_sha256 TEXT NOT NULL,
    previous_payload_sha256 TEXT,
    integrity_tag TEXT NOT NULL,
    PRIMARY KEY (state_key, revision)
) STRICT;

CREATE TABLE IF NOT EXISTS state_documents (
    state_key TEXT PRIMARY KEY,
    schema_version TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision > 0),
    payload BLOB NOT NULL,
    payload_sha256 TEXT NOT NULL,
    previous_payload_sha256 TEXT,
    integrity_tag TEXT NOT NULL,
    FOREIGN KEY (state_key, revision)
        REFERENCES state_document_history(state_key, revision)
) STRICT;

CREATE TABLE IF NOT EXISTS state_migrations (
    migration_id TEXT PRIMARY KEY,
    data_json BLOB NOT NULL,
    integrity_tag TEXT NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS release_generations (
    release_id TEXT PRIMARY KEY,
    manifest_json BLOB NOT NULL,
    manifest_sha256 TEXT NOT NULL,
    integrity_tag TEXT NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS release_pointer_history (
    revision INTEGER PRIMARY KEY CHECK (revision > 0),
    release_id TEXT NOT NULL REFERENCES release_generations(release_id),
    previous_release_id TEXT REFERENCES release_generations(release_id),
    integrity_tag TEXT NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS release_pointer (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    revision INTEGER NOT NULL CHECK (revision > 0),
    release_id TEXT NOT NULL REFERENCES release_generations(release_id),
    previous_release_id TEXT REFERENCES release_generations(release_id),
    integrity_tag TEXT NOT NULL,
    FOREIGN KEY (revision) REFERENCES release_pointer_history(revision)
) STRICT;

CREATE TABLE IF NOT EXISTS release_transitions (
    transition_id TEXT PRIMARY KEY,
    data_json BLOB NOT NULL,
    integrity_tag TEXT NOT NULL
) STRICT;
