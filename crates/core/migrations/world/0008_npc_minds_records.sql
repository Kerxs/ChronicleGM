-- v8：NPC 心理与战役记录。
-- 两者都只通过效果改变（adjust_mood / upsert_record / delete_record），每次变化都记在事件日志里，
-- 随时间线回溯恢复并参与重放校验。

-- NPC 情绪（各 0–100）。对玩家的信任在 campaign_bonds 里。
CREATE TABLE campaign_moods (
    campaign_id TEXT NOT NULL REFERENCES campaigns(id),
    npc_id      TEXT NOT NULL,
    fear        INTEGER NOT NULL DEFAULT 0,
    anger       INTEGER NOT NULL DEFAULT 0,
    suspicion   INTEGER NOT NULL DEFAULT 0,
    stress      INTEGER NOT NULL DEFAULT 0,
    loyalty     INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (campaign_id, npc_id)
) WITHOUT ROWID;

-- 战役记录：调查板（线索、关联、推理）、游戏中产生的动态任务等。data 为 JSON。
CREATE TABLE campaign_records (
    campaign_id TEXT NOT NULL REFERENCES campaigns(id),
    kind        TEXT NOT NULL,
    id          TEXT NOT NULL,
    data        TEXT NOT NULL,
    PRIMARY KEY (campaign_id, kind, id)
) WITHOUT ROWID;
