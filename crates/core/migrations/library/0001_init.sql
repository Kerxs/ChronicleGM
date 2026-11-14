-- Chronicle GM 资料库 schema v1：世界索引与应用设置。

CREATE TABLE worlds (
    id             TEXT PRIMARY KEY,
    name           TEXT NOT NULL,
    file_name      TEXT NOT NULL UNIQUE,
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL,
    last_opened_at INTEGER
);

CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
