-- Chronicle GM 世界文件 schema v1
-- 时间戳均为 Unix 毫秒；ID 为 ULID 文本；JSON 字段存文本。

CREATE TABLE world_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- 每次写入（手动编辑、AI 变更集、导入、撤销）都是一个 commit，revisions 记录每个实体的前后内容。
CREATE TABLE commits (
    id           TEXT PRIMARY KEY,
    author       TEXT NOT NULL,
    changeset_id TEXT,
    message      TEXT NOT NULL DEFAULT '',
    created_at   INTEGER NOT NULL,
    undone_by    TEXT,
    undo_of      TEXT
);

CREATE TABLE revisions (
    seq         INTEGER PRIMARY KEY AUTOINCREMENT,
    commit_id   TEXT NOT NULL REFERENCES commits(id),
    entity      TEXT NOT NULL,
    entity_id   TEXT NOT NULL,
    before_json TEXT,
    after_json  TEXT
);
CREATE INDEX idx_revisions_commit ON revisions(commit_id);
CREATE INDEX idx_revisions_entity ON revisions(entity, entity_id);

-- ===== 世界树 =====
CREATE TABLE nodes (
    id          TEXT PRIMARY KEY,
    kind        TEXT NOT NULL,
    parent_id   TEXT REFERENCES nodes(id),
    sort_key    TEXT NOT NULL,
    path        TEXT NOT NULL,
    title       TEXT NOT NULL,
    aliases     TEXT NOT NULL DEFAULT '[]',
    summary     TEXT NOT NULL DEFAULT '',
    player_text TEXT NOT NULL DEFAULT '',
    gm_text     TEXT NOT NULL DEFAULT '',
    payload     TEXT NOT NULL DEFAULT '{}',
    tags        TEXT NOT NULL DEFAULT '[]',
    canon_level TEXT NOT NULL DEFAULT 'draft',
    locked      INTEGER NOT NULL DEFAULT 0,
    status      TEXT NOT NULL DEFAULT 'draft',
    source      TEXT NOT NULL DEFAULT 'user',
    rev         INTEGER NOT NULL DEFAULT 1,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    deleted_at  INTEGER
);
CREATE INDEX idx_nodes_children ON nodes(parent_id, sort_key) WHERE deleted_at IS NULL;
CREATE INDEX idx_nodes_path ON nodes(path);
CREATE INDEX idx_nodes_kind ON nodes(kind) WHERE deleted_at IS NULL;

-- 规范化名称（标题 + 别名），用于重复检测与按名称解析引用。
CREATE TABLE node_names (
    node_id  TEXT NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    name_key TEXT NOT NULL,
    PRIMARY KEY (node_id, name_key)
) WITHOUT ROWID;
CREATE INDEX idx_node_names_key ON node_names(name_key);

-- 中文全文搜索：trigram 分词（查询少于 3 个字时回退到 LIKE）。
CREATE VIRTUAL TABLE nodes_fts USING fts5(
    title, aliases, summary, player_text, gm_text,
    content = 'nodes', content_rowid = 'rowid', tokenize = 'trigram'
);
CREATE TRIGGER nodes_fts_ai AFTER INSERT ON nodes BEGIN
    INSERT INTO nodes_fts(rowid, title, aliases, summary, player_text, gm_text)
    VALUES (new.rowid, new.title, new.aliases, new.summary, new.player_text, new.gm_text);
END;
CREATE TRIGGER nodes_fts_ad AFTER DELETE ON nodes BEGIN
    INSERT INTO nodes_fts(nodes_fts, rowid, title, aliases, summary, player_text, gm_text)
    VALUES ('delete', old.rowid, old.title, old.aliases, old.summary, old.player_text, old.gm_text);
END;
CREATE TRIGGER nodes_fts_au AFTER UPDATE OF title, aliases, summary, player_text, gm_text ON nodes BEGIN
    INSERT INTO nodes_fts(nodes_fts, rowid, title, aliases, summary, player_text, gm_text)
    VALUES ('delete', old.rowid, old.title, old.aliases, old.summary, old.player_text, old.gm_text);
    INSERT INTO nodes_fts(rowid, title, aliases, summary, player_text, gm_text)
    VALUES (new.rowid, new.title, new.aliases, new.summary, new.player_text, new.gm_text);
END;

CREATE TABLE relations (
    id         TEXT PRIMARY KEY,
    src_id     TEXT NOT NULL REFERENCES nodes(id),
    dst_id     TEXT NOT NULL REFERENCES nodes(id),
    rel_type   TEXT NOT NULL,
    directed   INTEGER NOT NULL DEFAULT 1,
    label      TEXT NOT NULL DEFAULT '',
    attrs      TEXT NOT NULL DEFAULT '{}',
    visibility TEXT NOT NULL DEFAULT 'player',
    valid_from INTEGER,
    valid_to   INTEGER,
    source     TEXT NOT NULL DEFAULT 'user',
    rev        INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    deleted_at INTEGER
);
CREATE INDEX idx_relations_src ON relations(src_id) WHERE deleted_at IS NULL;
CREATE INDEX idx_relations_dst ON relations(dst_id) WHERE deleted_at IS NULL;

-- ===== 剧本 =====
CREATE TABLE scripts (
    id             TEXT PRIMARY KEY,
    title          TEXT NOT NULL,
    genre          TEXT NOT NULL DEFAULT '',
    premise        TEXT NOT NULL DEFAULT '',
    background     TEXT NOT NULL DEFAULT '',
    core_setting   TEXT NOT NULL DEFAULT '',
    player_count   INTEGER NOT NULL DEFAULT 4,
    duration_hours REAL NOT NULL DEFAULT 4,
    tone           TEXT NOT NULL DEFAULT '',
    scale_profile  TEXT NOT NULL DEFAULT '{}',
    status         TEXT NOT NULL DEFAULT 'draft',
    source         TEXT NOT NULL DEFAULT 'user',
    rev            INTEGER NOT NULL DEFAULT 1,
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL,
    deleted_at     INTEGER
);

CREATE TABLE script_items (
    id          TEXT PRIMARY KEY,
    script_id   TEXT NOT NULL REFERENCES scripts(id),
    kind        TEXT NOT NULL,
    parent_id   TEXT REFERENCES script_items(id),
    sort_key    TEXT NOT NULL,
    title       TEXT NOT NULL,
    player_text TEXT NOT NULL DEFAULT '',
    gm_text     TEXT NOT NULL DEFAULT '',
    payload     TEXT NOT NULL DEFAULT '{}',
    locked      INTEGER NOT NULL DEFAULT 0,
    status      TEXT NOT NULL DEFAULT 'draft',
    source      TEXT NOT NULL DEFAULT 'user',
    rev         INTEGER NOT NULL DEFAULT 1,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    deleted_at  INTEGER
);
CREATE INDEX idx_script_items_tree ON script_items(script_id, parent_id, sort_key) WHERE deleted_at IS NULL;

-- 剧本中的 NPC：引用世界树 character 节点，叠加剧本内的定位与秘密。
CREATE TABLE script_cast (
    id                  TEXT PRIMARY KEY,
    script_id           TEXT NOT NULL REFERENCES scripts(id),
    node_id             TEXT NOT NULL REFERENCES nodes(id),
    role                TEXT NOT NULL DEFAULT '',
    motive              TEXT NOT NULL DEFAULT '',
    secret              TEXT NOT NULL DEFAULT '',
    relation_to_players TEXT NOT NULL DEFAULT '',
    payload             TEXT NOT NULL DEFAULT '{}',
    locked              INTEGER NOT NULL DEFAULT 0,
    source              TEXT NOT NULL DEFAULT 'user',
    rev                 INTEGER NOT NULL DEFAULT 1,
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL,
    deleted_at          INTEGER
);
CREATE INDEX idx_script_cast_script ON script_cast(script_id) WHERE deleted_at IS NULL;

-- ===== 剧情图 =====
CREATE TABLE plot_nodes (
    id              TEXT PRIMARY KEY,
    script_id       TEXT NOT NULL REFERENCES scripts(id),
    kind            TEXT NOT NULL,
    title           TEXT NOT NULL,
    player_text     TEXT NOT NULL DEFAULT '',
    gm_text         TEXT NOT NULL DEFAULT '',
    scene_ref       TEXT,
    script_item_ref TEXT,
    status          TEXT NOT NULL DEFAULT 'planned',
    payload         TEXT NOT NULL DEFAULT '{}',
    locked          INTEGER NOT NULL DEFAULT 0,
    source          TEXT NOT NULL DEFAULT 'user',
    rev             INTEGER NOT NULL DEFAULT 1,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    deleted_at      INTEGER
);
CREATE INDEX idx_plot_nodes_script ON plot_nodes(script_id) WHERE deleted_at IS NULL;

CREATE TABLE plot_edges (
    id         TEXT PRIMARY KEY,
    script_id  TEXT NOT NULL REFERENCES scripts(id),
    from_id    TEXT NOT NULL REFERENCES plot_nodes(id),
    to_id      TEXT NOT NULL REFERENCES plot_nodes(id),
    label      TEXT NOT NULL DEFAULT '',
    condition  TEXT,
    effects    TEXT NOT NULL DEFAULT '[]',
    priority   INTEGER NOT NULL DEFAULT 0,
    once       INTEGER NOT NULL DEFAULT 0,
    locked     INTEGER NOT NULL DEFAULT 0,
    source     TEXT NOT NULL DEFAULT 'user',
    rev        INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    deleted_at INTEGER
);
CREATE INDEX idx_plot_edges_from ON plot_edges(from_id) WHERE deleted_at IS NULL;
CREATE INDEX idx_plot_edges_to ON plot_edges(to_id) WHERE deleted_at IS NULL;

-- ===== AI 生成控制 =====
CREATE TABLE gen_jobs (
    id               TEXT PRIMARY KEY,
    kind             TEXT NOT NULL,
    target           TEXT NOT NULL DEFAULT '',
    params           TEXT NOT NULL DEFAULT '{}',
    context_digest   TEXT NOT NULL DEFAULT '',
    provider         TEXT NOT NULL DEFAULT '',
    model            TEXT NOT NULL DEFAULT '',
    template_version TEXT NOT NULL DEFAULT '',
    status           TEXT NOT NULL DEFAULT 'queued',
    attempts         INTEGER NOT NULL DEFAULT 0,
    raw_output       TEXT,
    error            TEXT,
    created_at       INTEGER NOT NULL,
    updated_at       INTEGER NOT NULL
);

CREATE TABLE changesets (
    id                TEXT PRIMARY KEY,
    job_id            TEXT REFERENCES gen_jobs(id),
    scope             TEXT NOT NULL,
    mode              TEXT NOT NULL DEFAULT 'normal',
    author            TEXT NOT NULL,
    title             TEXT NOT NULL DEFAULT '',
    ops               TEXT NOT NULL,
    validation_report TEXT NOT NULL DEFAULT '{}',
    status            TEXT NOT NULL DEFAULT 'pending',
    accepted_ops      TEXT,
    commit_id         TEXT,
    created_at        INTEGER NOT NULL,
    updated_at        INTEGER NOT NULL
);
CREATE INDEX idx_changesets_status ON changesets(status);

CREATE TABLE world_versions (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    note       TEXT NOT NULL DEFAULT '',
    commit_id  TEXT,
    file_name  TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

-- ===== 战役与世界状态（与世界正典分离） =====
CREATE TABLE campaigns (
    id                TEXT PRIMARY KEY,
    script_id         TEXT REFERENCES scripts(id),
    name              TEXT NOT NULL,
    base_commit       TEXT,
    rng_seed          INTEGER NOT NULL,
    roll_counter      INTEGER NOT NULL DEFAULT 0,
    turn              INTEGER NOT NULL DEFAULT 0,
    current_plot_node TEXT REFERENCES plot_nodes(id),
    created_at        INTEGER NOT NULL,
    updated_at        INTEGER NOT NULL
);

CREATE TABLE state_vars (
    campaign_id TEXT NOT NULL REFERENCES campaigns(id),
    key         TEXT NOT NULL,
    value       TEXT NOT NULL,
    PRIMARY KEY (campaign_id, key)
) WITHOUT ROWID;

CREATE TABLE quest_states (
    campaign_id TEXT NOT NULL REFERENCES campaigns(id),
    quest_id    TEXT NOT NULL,
    state       TEXT NOT NULL,
    PRIMARY KEY (campaign_id, quest_id)
) WITHOUT ROWID;

CREATE TABLE clue_reveals (
    campaign_id TEXT NOT NULL REFERENCES campaigns(id),
    clue_id     TEXT NOT NULL,
    turn        INTEGER NOT NULL,
    PRIMARY KEY (campaign_id, clue_id)
) WITHOUT ROWID;

CREATE TABLE inventory (
    campaign_id TEXT NOT NULL REFERENCES campaigns(id),
    holder      TEXT NOT NULL,
    item        TEXT NOT NULL,
    qty         INTEGER NOT NULL,
    PRIMARY KEY (campaign_id, holder, item)
) WITHOUT ROWID;

CREATE TABLE relation_overrides (
    campaign_id TEXT NOT NULL REFERENCES campaigns(id),
    a           TEXT NOT NULL,
    b           TEXT NOT NULL,
    rel_type    TEXT NOT NULL,
    active      INTEGER NOT NULL,
    PRIMARY KEY (campaign_id, a, b, rel_type)
) WITHOUT ROWID;

CREATE TABLE entity_overrides (
    campaign_id TEXT NOT NULL REFERENCES campaigns(id),
    entity_id   TEXT NOT NULL,
    field       TEXT NOT NULL,
    value       TEXT NOT NULL,
    PRIMARY KEY (campaign_id, entity_id, field)
) WITHOUT ROWID;

CREATE TABLE plot_visits (
    campaign_id  TEXT NOT NULL REFERENCES campaigns(id),
    plot_node_id TEXT NOT NULL,
    count        INTEGER NOT NULL,
    first_turn   INTEGER NOT NULL,
    PRIMARY KEY (campaign_id, plot_node_id)
) WITHOUT ROWID;

CREATE TABLE plot_unlocks (
    campaign_id  TEXT NOT NULL REFERENCES campaigns(id),
    plot_node_id TEXT NOT NULL,
    PRIMARY KEY (campaign_id, plot_node_id)
) WITHOUT ROWID;

CREATE TABLE edge_uses (
    campaign_id TEXT NOT NULL REFERENCES campaigns(id),
    edge_id     TEXT NOT NULL,
    count       INTEGER NOT NULL,
    PRIMARY KEY (campaign_id, edge_id)
) WITHOUT ROWID;

-- 只追加的事件日志：玩家行动、掷骰（含种子与流号）、效果、剧情跳转。
CREATE TABLE event_log (
    seq         INTEGER PRIMARY KEY AUTOINCREMENT,
    campaign_id TEXT NOT NULL REFERENCES campaigns(id),
    turn        INTEGER NOT NULL,
    kind        TEXT NOT NULL,
    payload     TEXT NOT NULL,
    created_at  INTEGER NOT NULL
);
CREATE INDEX idx_event_log_campaign ON event_log(campaign_id, seq);
CREATE TRIGGER event_log_no_update BEFORE UPDATE ON event_log BEGIN
    SELECT RAISE(ABORT, 'event_log is append-only');
END;
CREATE TRIGGER event_log_no_delete BEFORE DELETE ON event_log BEGIN
    SELECT RAISE(ABORT, 'event_log is append-only');
END;

CREATE TABLE npc_memories (
    id          TEXT PRIMARY KEY,
    campaign_id TEXT NOT NULL REFERENCES campaigns(id),
    npc_id      TEXT NOT NULL,
    content     TEXT NOT NULL,
    importance  INTEGER NOT NULL DEFAULT 1,
    turn        INTEGER NOT NULL,
    created_at  INTEGER NOT NULL
);
CREATE INDEX idx_npc_memories ON npc_memories(campaign_id, npc_id, turn);

CREATE TABLE session_summaries (
    id          TEXT PRIMARY KEY,
    campaign_id TEXT NOT NULL REFERENCES campaigns(id),
    session_no  INTEGER NOT NULL,
    summary     TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'draft',
    created_at  INTEGER NOT NULL
);
