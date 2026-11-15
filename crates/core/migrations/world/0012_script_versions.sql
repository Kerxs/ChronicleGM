-- 剧本版本快照：剧本、条目、角色、剧情图的完整 JSON，可对比、恢复。
CREATE TABLE script_versions (
    id          TEXT PRIMARY KEY,
    script_id   TEXT NOT NULL,
    name        TEXT NOT NULL,
    note        TEXT NOT NULL DEFAULT '',
    data        TEXT NOT NULL,
    created_at  INTEGER NOT NULL
);
CREATE INDEX idx_script_versions ON script_versions(script_id, created_at);
