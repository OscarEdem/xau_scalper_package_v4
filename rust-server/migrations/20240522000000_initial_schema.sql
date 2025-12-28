CREATE TABLE IF NOT EXISTS signals (
    signal_id TEXT PRIMARY KEY,
    symbol TEXT NOT NULL,
    entry_type TEXT NOT NULL,
    timestamp BIGINT NOT NULL,
    data JSONB NOT NULL
);

CREATE TABLE IF NOT EXISTS sessions (
    symbol TEXT PRIMARY KEY,
    data JSONB NOT NULL,
    updated_at BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY,
    value JSONB NOT NULL
);

CREATE TABLE IF NOT EXISTS push_tokens (
    token TEXT PRIMARY KEY,
    created_at BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS news_events (
    event TEXT NOT NULL,
    timestamp BIGINT NOT NULL,
    currency TEXT NOT NULL,
    impact TEXT NOT NULL,
    data JSONB NOT NULL,
    PRIMARY KEY (event, timestamp, currency)
);

CREATE TABLE IF NOT EXISTS analysis_reports (
    id SERIAL PRIMARY KEY,
    symbol TEXT NOT NULL,
    period TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    report JSONB NOT NULL,
    events_hash TEXT NOT NULL
);