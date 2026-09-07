CREATE TABLE IF NOT EXISTS nodes (
    id                TEXT PRIMARY KEY NOT NULL,
    address           TEXT NOT NULL,
    status            TEXT NOT NULL DEFAULT 'starting',
    last_heartbeat    DATETIME,
    max_connections   INTEGER NOT NULL DEFAULT 0,
    active_connections INTEGER NOT NULL DEFAULT 0,
    bandwidth_limit   INTEGER NOT NULL DEFAULT 0,
    bytes_up          INTEGER NOT NULL DEFAULT 0,
    bytes_down        INTEGER NOT NULL DEFAULT 0,
    latency_ms        INTEGER NOT NULL DEFAULT 0,
    created_at        DATETIME NOT NULL,
    updated_at        DATETIME NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_nodes_status ON nodes (status);
