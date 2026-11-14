-- v4：AI GM 整合需要的战役数据。

-- 游戏中由 AI 提议的新人物/地点：先只属于本战役（GM 可以继续使用），
-- 同时生成一个待审变更集；用户在审阅台确认后才成为世界正典。
CREATE TABLE campaign_entities (
    id           TEXT PRIMARY KEY,
    campaign_id  TEXT NOT NULL REFERENCES campaigns(id),
    kind         TEXT NOT NULL,
    name         TEXT NOT NULL,
    summary      TEXT NOT NULL DEFAULT '',
    gm_text      TEXT NOT NULL DEFAULT '',
    changeset_id TEXT,
    created_turn INTEGER NOT NULL,
    created_at   INTEGER NOT NULL
);
CREATE INDEX idx_campaign_entities ON campaign_entities(campaign_id, created_turn);

-- 会话摘要覆盖到哪一条事件为止（下一次摘要从这里继续）。
ALTER TABLE session_summaries ADD COLUMN upto_seq INTEGER NOT NULL DEFAULT 0;
