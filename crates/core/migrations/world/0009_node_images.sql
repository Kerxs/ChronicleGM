-- v9：世界树节点的图片（人物立绘、地点插图、地图等）。
-- 图片以二进制保存在世界文件里：自动备份、从备份还原、迁移前备份都会一并包含。
-- 删除节点（清空回收站）时图片随之删除。图片不是世界正典内容，不经过变更集与修订。
CREATE TABLE node_images (
    id          TEXT PRIMARY KEY,
    node_id     TEXT NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    mime        TEXT NOT NULL,
    width       INTEGER NOT NULL DEFAULT 0,
    height      INTEGER NOT NULL DEFAULT 0,
    bytes       BLOB NOT NULL,
    caption     TEXT NOT NULL DEFAULT '',
    sort        INTEGER NOT NULL DEFAULT 0,
    created_at  INTEGER NOT NULL
);
CREATE INDEX idx_node_images_node ON node_images(node_id, sort);
