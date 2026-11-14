-- v6：剧情回溯与多时间线。
-- 每条时间线是一个战役：回溯时从已结束战役的某个历史检查点复制完整状态与之前的事件，
-- 生成独立的新战役；原时间线只读取、不修改。

ALTER TABLE campaigns ADD COLUMN parent_id TEXT REFERENCES campaigns(id);
-- 在父时间线中的分叉位置（父时间线的事件序号与回合数）。
ALTER TABLE campaigns ADD COLUMN fork_seq INTEGER;
ALTER TABLE campaigns ADD COLUMN fork_turn INTEGER;
-- active（进行中）/ ended（已结束，只能查看或从历史节点开出新时间线）
ALTER TABLE campaigns ADD COLUMN status TEXT NOT NULL DEFAULT 'active';
ALTER TABLE campaigns ADD COLUMN archived INTEGER NOT NULL DEFAULT 0;
ALTER TABLE campaigns ADD COLUMN ended_at INTEGER;
-- 结局说明（结局节点名或用户填写的说明）。
ALTER TABLE campaigns ADD COLUMN ending TEXT NOT NULL DEFAULT '';

-- 状态检查点：每个完成的回合、手动剧情推进、战役开始与结束时保存完整战役状态。
-- state 为 JSON（状态表全部行 + 回合数、骰子计数、当前剧情节点 + NPC 记忆与临时实体数量 + 会话摘要），
-- checksum 用于发现检查点被意外修改；app_version 记录写入时的程序版本（规则变化时便于判断重放差异）。
CREATE TABLE campaign_checkpoints (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    campaign_id TEXT NOT NULL REFERENCES campaigns(id),
    seq         INTEGER NOT NULL,
    turn        INTEGER NOT NULL,
    label       TEXT NOT NULL,
    state       TEXT NOT NULL,
    checksum    TEXT NOT NULL,
    app_version TEXT NOT NULL,
    created_at  INTEGER NOT NULL
);
CREATE INDEX idx_campaign_checkpoints ON campaign_checkpoints(campaign_id, seq);
CREATE INDEX idx_campaigns_parent ON campaigns(parent_id);
