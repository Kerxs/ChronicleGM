-- v7：战役中的人物关系（有向：from 对 to 的看法，双方可以不同）。
-- 只通过 adjust_bond 效果改变，每次变化都记录在事件日志里（关系历史从事件日志读取，随时间线回溯一起正确恢复）。
CREATE TABLE campaign_bonds (
    campaign_id TEXT NOT NULL REFERENCES campaigns(id),
    from_id     TEXT NOT NULL,
    to_id       TEXT NOT NULL,
    kind        TEXT NOT NULL DEFAULT '',
    strength    INTEGER NOT NULL DEFAULT 0,
    trust       INTEGER NOT NULL DEFAULT 0,
    affection   INTEGER NOT NULL DEFAULT 0,
    hostility   INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (campaign_id, from_id, to_id)
) WITHOUT ROWID;
CREATE INDEX idx_campaign_bonds_to ON campaign_bonds(campaign_id, to_id);
