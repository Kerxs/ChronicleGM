-- v5：核心设定的部分索引。GM 每回合、生成每次都要按层级顺序读取核心设定；
-- 基准实测：5 万节点时全表扫描 + 排序需要约 305 ms。
CREATE INDEX idx_nodes_core ON nodes(path) WHERE canon_level = 'core' AND deleted_at IS NULL;
