-- SPDX-License-Identifier: AGPL-3.0-only

PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;

CREATE TABLE IF NOT EXISTS schema_migrations (
    version TEXT PRIMARY KEY,
    applied_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    description TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS profiles (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    game TEXT NOT NULL CHECK (game IN ('Azur', 'Ark', 'BA')),
    server TEXT NOT NULL CHECK (server IN ('.jp', '.cn', '.gb')),
    locale TEXT,
    resolution_width INTEGER NOT NULL CHECK (resolution_width > 0),
    resolution_height INTEGER NOT NULL CHECK (resolution_height > 0),
    resolution_scale REAL,
    resolution_dpi INTEGER,
    runtime_state TEXT NOT NULL DEFAULT 'stopped',
    config_ref TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE IF NOT EXISTS runtime_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    event_type TEXT NOT NULL,
    profile_id TEXT REFERENCES profiles(id) ON DELETE SET NULL,
    severity TEXT NOT NULL CHECK (severity IN ('info', 'warning', 'error', 'fatal', 'degraded')),
    payload_json TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE IF NOT EXISTS logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    profile_id TEXT REFERENCES profiles(id) ON DELETE SET NULL,
    task_run_id TEXT,
    level TEXT NOT NULL CHECK (level IN ('info', 'warning', 'error', 'fatal', 'degraded')),
    source TEXT NOT NULL,
    message TEXT NOT NULL,
    context_json TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE IF NOT EXISTS scheduler_state (
    profile_id TEXT PRIMARY KEY REFERENCES profiles(id) ON DELETE CASCADE,
    alive INTEGER NOT NULL DEFAULT 0 CHECK (alive IN (0, 1)),
    current_task_id TEXT,
    next_task_id TEXT,
    next_run_at TEXT,
    pending_count INTEGER NOT NULL DEFAULT 0 CHECK (pending_count >= 0),
    waiting_count INTEGER NOT NULL DEFAULT 0 CHECK (waiting_count >= 0),
    last_severity TEXT NOT NULL DEFAULT 'info',
    state TEXT NOT NULL DEFAULT 'stopped',
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE IF NOT EXISTS task_definitions (
    id TEXT PRIMARY KEY,
    flow_id TEXT NOT NULL,
    game TEXT NOT NULL CHECK (game IN ('Azur', 'Ark', 'BA')),
    server TEXT NOT NULL CHECK (server IN ('.jp', '.cn', '.gb')),
    name TEXT NOT NULL,
    schema_version TEXT NOT NULL,
    definition_json TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE IF NOT EXISTS task_runs (
    id TEXT PRIMARY KEY,
    profile_id TEXT NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
    task_id TEXT NOT NULL,
    flow_id TEXT NOT NULL,
    state TEXT NOT NULL,
    started_at TEXT NOT NULL,
    ended_at TEXT,
    last_error_json TEXT,
    command_request_id TEXT,
    metadata_json TEXT
);

CREATE TABLE IF NOT EXISTS resource_packs (
    id TEXT PRIMARY KEY,
    game TEXT NOT NULL CHECK (game IN ('Azur', 'Ark', 'BA')),
    server TEXT NOT NULL CHECK (server IN ('.jp', '.cn', '.gb')),
    locale TEXT,
    resolution_width INTEGER NOT NULL CHECK (resolution_width > 0),
    resolution_height INTEGER NOT NULL CHECK (resolution_height > 0),
    resolution_scale REAL,
    resource_repo TEXT NOT NULL,
    resource_commit TEXT NOT NULL,
    license_notice_ref TEXT,
    provenance_json TEXT NOT NULL,
    validation_state TEXT NOT NULL DEFAULT 'pending',
    ingested_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE IF NOT EXISTS asset_manifest (
    id TEXT PRIMARY KEY,
    resource_pack_id TEXT NOT NULL REFERENCES resource_packs(id) ON DELETE CASCADE,
    asset_type TEXT NOT NULL,
    path TEXT NOT NULL,
    hash TEXT,
    game TEXT NOT NULL CHECK (game IN ('Azur', 'Ark', 'BA')),
    server TEXT NOT NULL CHECK (server IN ('.jp', '.cn', '.gb')),
    locale TEXT,
    resolution_width INTEGER NOT NULL CHECK (resolution_width > 0),
    resolution_height INTEGER NOT NULL CHECK (resolution_height > 0),
    metadata_json TEXT
);

CREATE TABLE IF NOT EXISTS resource_snapshots (
    id TEXT PRIMARY KEY,
    profile_id TEXT NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
    task_run_id TEXT REFERENCES task_runs(id) ON DELETE SET NULL,
    game TEXT NOT NULL CHECK (game IN ('Azur', 'Ark', 'BA')),
    server TEXT NOT NULL CHECK (server IN ('.jp', '.cn', '.gb')),
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    source TEXT NOT NULL,
    observed_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS resource_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    profile_id TEXT NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
    task_run_id TEXT REFERENCES task_runs(id) ON DELETE SET NULL,
    game TEXT NOT NULL CHECK (game IN ('Azur', 'Ark', 'BA')),
    server TEXT NOT NULL CHECK (server IN ('.jp', '.cn', '.gb')),
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    source TEXT NOT NULL,
    observed_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS acquisition_captures (
    id TEXT PRIMARY KEY,
    profile_id TEXT NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
    task_id TEXT NOT NULL,
    task_run_id TEXT NOT NULL REFERENCES task_runs(id) ON DELETE CASCADE,
    game TEXT NOT NULL CHECK (game IN ('Azur', 'Ark', 'BA')),
    server TEXT NOT NULL CHECK (server IN ('.jp', '.cn', '.gb')),
    locale TEXT,
    resolution_width INTEGER NOT NULL CHECK (resolution_width > 0),
    resolution_height INTEGER NOT NULL CHECK (resolution_height > 0),
    resolution_scale REAL,
    image_ref TEXT NOT NULL,
    image_path TEXT,
    image_hash TEXT,
    source_trigger TEXT NOT NULL,
    recognition_state TEXT NOT NULL CHECK (recognition_state IN ('pending', 'recognized', 'failed', 'manual')),
    retention_class TEXT,
    captured_at TEXT NOT NULL,
    metadata_json TEXT
);

CREATE TABLE IF NOT EXISTS acquisition_labels (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    acquisition_id TEXT NOT NULL REFERENCES acquisition_captures(id) ON DELETE CASCADE,
    label TEXT NOT NULL,
    confidence REAL,
    source TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_logs_profile_created ON logs(profile_id, created_at);
CREATE INDEX IF NOT EXISTS idx_runtime_events_profile_created ON runtime_events(profile_id, created_at);
CREATE INDEX IF NOT EXISTS idx_resource_history_profile_key_time ON resource_history(profile_id, key, observed_at);
CREATE INDEX IF NOT EXISTS idx_acquisition_profile_time ON acquisition_captures(profile_id, captured_at);
CREATE INDEX IF NOT EXISTS idx_acquisition_task_run ON acquisition_captures(task_run_id);
CREATE INDEX IF NOT EXISTS idx_asset_manifest_pack ON asset_manifest(resource_pack_id);

