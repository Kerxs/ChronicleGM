-- v2：模型服务配置、任务路由、请求统计。
-- API Key 不在这里，保存在 Windows 凭据管理器。

CREATE TABLE providers (
    id         TEXT PRIMARY KEY,
    config     TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE task_routes (
    task        TEXT PRIMARY KEY,
    provider_id TEXT NOT NULL REFERENCES providers(id) ON DELETE CASCADE
);

-- 只记录元数据，不记录提示词与模型输出内容。
CREATE TABLE llm_requests (
    seq               INTEGER PRIMARY KEY AUTOINCREMENT,
    provider_id       TEXT NOT NULL,
    model             TEXT NOT NULL,
    task              TEXT NOT NULL,
    job_id            TEXT,
    ok                INTEGER NOT NULL,
    attempts          INTEGER NOT NULL DEFAULT 1,
    prompt_tokens     INTEGER,
    completion_tokens INTEGER,
    latency_ms        INTEGER,
    error             TEXT,
    created_at        INTEGER NOT NULL
);
CREATE INDEX idx_llm_requests_provider ON llm_requests(provider_id, created_at);
