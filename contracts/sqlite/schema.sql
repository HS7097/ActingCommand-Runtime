-- SPDX-License-Identifier: AGPL-3.0-only

PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;

CREATE TABLE IF NOT EXISTS schema_migrations (
    version TEXT PRIMARY KEY,
    applied_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    description TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS server_variants (
    id TEXT PRIMARY KEY,
    game TEXT NOT NULL CHECK (game IN ('Azur', 'Ark', 'BA')),
    upstream TEXT NOT NULL,
    server_key TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    locale TEXT,
    notes TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

INSERT OR IGNORE INTO server_variants (id, game, upstream, server_key, display_name, locale, notes) VALUES
    ('alas.cn', 'Azur', 'Alas', 'alas.cn', 'Alas CN', 'zh-CN', 'Azur Lane Alas cn server variant'),
    ('alas.en', 'Azur', 'Alas', 'alas.en', 'Alas EN', 'en', 'Azur Lane Alas en server variant'),
    ('alas.jp', 'Azur', 'Alas', 'alas.jp', 'Alas JP', 'ja-JP', 'Azur Lane Alas jp server variant'),
    ('alas.tw', 'Azur', 'Alas', 'alas.tw', 'Alas TW', 'zh-TW', 'Azur Lane Alas tw server variant'),
    ('baas.jp', 'BA', 'BAAS', 'baas.jp', 'BAAS JP', 'ja-JP', 'Blue Archive BAAS jp server variant'),
    ('baas.cn', 'BA', 'BAAS', 'baas.cn', 'BAAS CN', 'zh-CN', 'Blue Archive BAAS cn server variant'),
    ('baas.global_en', 'BA', 'BAAS', 'baas.global_en', 'BAAS Global EN', 'en', 'Blue Archive BAAS global_en server variant'),
    ('baas.ko', 'BA', 'BAAS', 'baas.ko', 'BAAS KO', 'ko-KR', 'Blue Archive BAAS ko server variant'),
    ('baas.zh_tw', 'BA', 'BAAS', 'baas.zh_tw', 'BAAS ZH TW', 'zh-TW', 'Blue Archive BAAS zh_tw server variant'),
    ('maa.bilibili', 'Ark', 'MAA', 'maa.bilibili', 'MAA Bilibili', 'zh-CN', 'Arknights MAA B server variant'),
    ('maa.official', 'Ark', 'MAA', 'maa.official', 'MAA Official', 'zh-CN', 'Arknights MAA official CN server variant'),
    ('maa.txwy', 'Ark', 'MAA', 'maa.txwy', 'MAA txwy', 'zh-CN', 'Arknights MAA txwy server variant'),
    ('maa.yostar_en', 'Ark', 'MAA', 'maa.yostar_en', 'MAA YoStar EN', 'en', 'Arknights MAA YoStarEN server variant'),
    ('maa.yostar_jp', 'Ark', 'MAA', 'maa.yostar_jp', 'MAA YoStar JP', 'ja-JP', 'Arknights MAA YoStarJP server variant'),
    ('maa.yostar_kr', 'Ark', 'MAA', 'maa.yostar_kr', 'MAA YoStar KR', 'ko-KR', 'Arknights MAA YoStarKR server variant');

CREATE TABLE IF NOT EXISTS profiles (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    game TEXT NOT NULL CHECK (game IN ('Azur', 'Ark', 'BA')),
    server TEXT NOT NULL,
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
    server TEXT NOT NULL,
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
    server TEXT NOT NULL,
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
    server TEXT NOT NULL,
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
    server TEXT NOT NULL,
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
    server TEXT NOT NULL,
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
    server TEXT NOT NULL,
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
CREATE INDEX IF NOT EXISTS idx_server_variants_game_key ON server_variants(game, server_key);
CREATE INDEX IF NOT EXISTS idx_runtime_events_profile_created ON runtime_events(profile_id, created_at);
CREATE INDEX IF NOT EXISTS idx_resource_history_profile_key_time ON resource_history(profile_id, key, observed_at);
CREATE INDEX IF NOT EXISTS idx_acquisition_profile_time ON acquisition_captures(profile_id, captured_at);
CREATE INDEX IF NOT EXISTS idx_acquisition_task_run ON acquisition_captures(task_run_id);
CREATE INDEX IF NOT EXISTS idx_asset_manifest_pack ON asset_manifest(resource_pack_id);
