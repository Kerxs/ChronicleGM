-- v3：关系的组合索引（按两端查询、一致性检查自连接）。统计信息由打开数据库时的 PRAGMA optimize 维护。
-- 基准实测：5 万节点 / 20 万关系时，旧查询写法导致"节点的全部关系"需要 1.2 秒。
CREATE INDEX idx_relations_pair ON relations(src_id, dst_id, rel_type) WHERE deleted_at IS NULL;
CREATE INDEX idx_relations_dst_pair ON relations(dst_id, src_id, rel_type) WHERE deleted_at IS NULL;
CREATE INDEX idx_relations_type ON relations(rel_type, src_id, dst_id, valid_from, valid_to, id) WHERE deleted_at IS NULL;
