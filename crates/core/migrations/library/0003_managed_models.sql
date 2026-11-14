-- v3：托管模式的模型程序配置（程序路径、启动参数、端口、角色等，内容为 JSON）。
-- 只是配置；软件不会下载或修改模型文件。
CREATE TABLE managed_models (
    id         TEXT PRIMARY KEY,
    config     TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
