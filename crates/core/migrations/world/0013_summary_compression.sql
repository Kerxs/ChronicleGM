-- 上下文压缩：自动摘要（status = 'auto'）的来源与压缩前后的 token 数。
ALTER TABLE session_summaries ADD COLUMN source TEXT NOT NULL DEFAULT 'manual';
ALTER TABLE session_summaries ADD COLUMN tokens_before INTEGER;
ALTER TABLE session_summaries ADD COLUMN tokens_after INTEGER;
ALTER TABLE session_summaries ADD COLUMN token_method TEXT NOT NULL DEFAULT '';
ALTER TABLE session_summaries ADD COLUMN from_seq INTEGER NOT NULL DEFAULT 0;
