-- v2：剧本条目全文搜索（trigram，与世界树一致）。

CREATE VIRTUAL TABLE script_items_fts USING fts5(
    title, player_text, gm_text,
    content = 'script_items', content_rowid = 'rowid', tokenize = 'trigram'
);
CREATE TRIGGER script_items_fts_ai AFTER INSERT ON script_items BEGIN
    INSERT INTO script_items_fts(rowid, title, player_text, gm_text)
    VALUES (new.rowid, new.title, new.player_text, new.gm_text);
END;
CREATE TRIGGER script_items_fts_ad AFTER DELETE ON script_items BEGIN
    INSERT INTO script_items_fts(script_items_fts, rowid, title, player_text, gm_text)
    VALUES ('delete', old.rowid, old.title, old.player_text, old.gm_text);
END;
CREATE TRIGGER script_items_fts_au AFTER UPDATE OF title, player_text, gm_text ON script_items BEGIN
    INSERT INTO script_items_fts(script_items_fts, rowid, title, player_text, gm_text)
    VALUES ('delete', old.rowid, old.title, old.player_text, old.gm_text);
    INSERT INTO script_items_fts(rowid, title, player_text, gm_text)
    VALUES (new.rowid, new.title, new.player_text, new.gm_text);
END;
-- 回填已有数据。
INSERT INTO script_items_fts(script_items_fts) VALUES ('rebuild');

-- 生成任务的进度说明（例如"第 2 次尝试：修复 JSON"）。
ALTER TABLE gen_jobs ADD COLUMN note TEXT NOT NULL DEFAULT '';
