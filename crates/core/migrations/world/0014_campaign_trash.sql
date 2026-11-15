-- 删除战役先进回收站（可以恢复；清空回收站才真正删除）。
ALTER TABLE campaigns ADD COLUMN deleted_at INTEGER;
