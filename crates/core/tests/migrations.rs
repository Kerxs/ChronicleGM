use chronicle_core::db::{self, Migration, WORLD_MIGRATIONS};
use chronicle_core::*;

#[test]
fn fresh_world_is_migrated_without_backup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("w.cgw");
    let w = World::create(&path, &dir.path().join("b"), "x").unwrap();
    assert_eq!(db::user_version(w.conn()).unwrap(), 10);
    assert_eq!(w.name().unwrap(), "x");
    drop(w);
    let (_, rep) = World::open(&path, &dir.path().join("b")).unwrap();
    assert_eq!((rep.from, rep.to, rep.backup), (10, 10, None));
    assert!(!dir.path().join("b").exists() || std::fs::read_dir(dir.path().join("b")).unwrap().next().is_none());
}

#[test]
fn create_refuses_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("w.cgw");
    std::fs::write(&path, b"user data").unwrap();
    assert!(World::create(&path, dir.path(), "x").is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"user data");
}

const V2_OK: Migration = Migration { version: 2, name: "add_col", sql: "ALTER TABLE nodes ADD COLUMN extra TEXT;" };
const V2_BAD: Migration = Migration {
    version: 2,
    name: "broken",
    sql: "ALTER TABLE nodes ADD COLUMN extra TEXT; THIS IS NOT SQL;",
};

/// 构造一个 v1 版本、含数据的世界文件（只执行第一个迁移，模拟旧版程序创建的文件）。
fn v1_db_with_data(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("old.cgw");
    let mut conn = db::open_connection(&path).unwrap();
    db::migrate(&mut conn, &[WORLD_MIGRATIONS[0].clone_static()], &dir.join("b"), "old").unwrap();
    conn.execute_batch(
        "INSERT INTO world_meta VALUES ('name', '旧世界');
         INSERT INTO nodes (id, kind, sort_key, path, title, created_at, updated_at)
             VALUES ('N1', 'city', 'V', '/N1/', '白石城', 0, 0);
         INSERT INTO scripts (id, title, created_at, updated_at) VALUES ('S1', '旧剧本', 0, 0);
         INSERT INTO script_items (id, script_id, kind, sort_key, title, player_text, created_at, updated_at)
             VALUES ('I1', 'S1', 'scene', 'V', '灯塔之夜', '海浪拍打着礁石', 0, 0);",
    )
    .unwrap();
    path
}

#[test]
fn real_upgrade_v1_to_v2_backfills_script_search() {
    let dir = tempfile::tempdir().unwrap();
    let path = v1_db_with_data(dir.path());
    let (w, rep) = World::open(&path, &dir.path().join("b")).unwrap();
    assert_eq!((rep.from, rep.to), (1, 10));
    assert!(rep.backup.as_ref().unwrap().exists());
    // 旧数据在新的剧本搜索中可以找到（迁移时回填了索引）。
    let hits = chronicle_core::search::search_script_items(w.conn(), Some("S1"), "海浪拍打", false, 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].title, "灯塔之夜");
    assert_eq!(chronicle_core::nodes::get_node(w.conn(), "N1").unwrap().unwrap().title, "白石城");
    let note: String = w.conn().query_row("SELECT note FROM gen_jobs LIMIT 1", [], |r| r.get(0)).unwrap_or_default();
    assert_eq!(note, "");
}

#[test]
fn upgrade_backs_up_and_preserves_data() {
    let dir = tempfile::tempdir().unwrap();
    let path = v1_db_with_data(dir.path());
    let mut conn = db::open_connection(&path).unwrap();
    let migrations = [WORLD_MIGRATIONS[0].clone_static(), V2_OK];
    let rep = db::migrate(&mut conn, &migrations, &dir.path().join("b"), "old").unwrap();
    assert_eq!((rep.from, rep.to), (1, 2));
    let backup = rep.backup.expect("升级前应生成备份");
    assert!(backup.exists());
    // 升级后数据仍在，新列可用。
    let title: String = conn.query_row("SELECT title FROM nodes", [], |r| r.get(0)).unwrap();
    assert_eq!(title, "白石城");
    conn.execute("UPDATE nodes SET extra = 'x'", []).unwrap();
    // 备份是 v1 且包含数据。
    let b = rusqlite::Connection::open(&backup).unwrap();
    assert_eq!(db::user_version(&b).unwrap(), 1);
    let n: i64 = b.query_row("SELECT COUNT(*) FROM nodes", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1);
}

#[test]
fn failed_migration_rolls_back() {
    let dir = tempfile::tempdir().unwrap();
    let path = v1_db_with_data(dir.path());
    let mut conn = db::open_connection(&path).unwrap();
    let migrations = [WORLD_MIGRATIONS[0].clone_static(), V2_BAD];
    let err = db::migrate(&mut conn, &migrations, &dir.path().join("b"), "old").unwrap_err();
    assert!(matches!(err, CoreError::Migration(_)), "{err}");
    assert!(err.to_string().contains("迁移前备份"));
    assert_eq!(db::user_version(&conn).unwrap(), 1);
    // 部分执行的 ALTER 也被回滚。
    let has_extra: bool = conn
        .prepare("SELECT extra FROM nodes")
        .is_ok();
    assert!(!has_extra);
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM nodes", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1);
}

#[test]
fn newer_database_is_refused_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let path = v1_db_with_data(dir.path());
    {
        let conn = db::open_connection(&path).unwrap();
        conn.pragma_update(None, "user_version", 99).unwrap();
    }
    let err = World::open(&path, &dir.path().join("b")).err().unwrap();
    assert!(err.to_string().contains("高于程序支持的版本"));
    let conn = db::open_connection(&path).unwrap();
    assert_eq!(db::user_version(&conn).unwrap(), 99);
}

#[test]
fn library_creates_and_lists_worlds() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open(dir.path()).unwrap();
    let (info, _w) = lib.create_world("艾泽大陆").unwrap();
    assert!(lib.world_path(&info).exists());
    let list = lib.list_worlds().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "艾泽大陆");
    lib.rename_world(&info.id, "新名字").unwrap();
    assert_eq!(lib.open_world(&info.id).unwrap().name().unwrap(), "新名字");
    lib.set_setting("theme", "dark").unwrap();
    assert_eq!(lib.setting("theme").unwrap().as_deref(), Some("dark"));
    assert!(lib.create_world("  ").is_err());
}

trait CloneStatic {
    fn clone_static(&self) -> Migration;
}
impl CloneStatic for Migration {
    fn clone_static(&self) -> Migration {
        Migration { version: self.version, name: self.name, sql: self.sql }
    }
}

#[test]
fn library_providers_routes_and_stats() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open(dir.path()).unwrap();
    lib.save_provider("p1", &serde_json::json!({"id": "p1", "name": "本地"})).unwrap();
    lib.save_provider("p1", &serde_json::json!({"id": "p1", "name": "本地（改名）"})).unwrap();
    assert_eq!(lib.providers().unwrap().len(), 1);
    assert_eq!(lib.provider("p1").unwrap().unwrap()["name"], "本地（改名）");
    assert!(lib.set_route("script_gen", Some("nope")).is_err());
    lib.set_route("default", Some("p1")).unwrap();
    assert_eq!(lib.route_for("script_gen").unwrap().as_deref(), Some("p1"), "回落到默认路由");
    lib.log_request(&chronicle_core::library::LlmRequestRecord {
        provider_id: "p1".into(), model: "m".into(), task: "script_gen".into(), ok: false, attempts: 2,
        latency_ms: Some(100), error: Some("超时".into()), ..Default::default()
    }).unwrap();
    let stats = lib.request_stats().unwrap();
    assert_eq!((stats[0].requests, stats[0].failures), (1, 1));
    lib.delete_provider("p1").unwrap();
    assert!(lib.route_for("script_gen").unwrap().is_none(), "删除配置时路由一并移除");
}

#[test]
fn core_canon_query_uses_index() {
    let dir = tempfile::tempdir().unwrap();
    let w = World::create(&dir.path().join("w.cgw"), &dir.path().join("b"), "x").unwrap();
    let plan: Vec<String> = {
        let mut st = w
            .conn()
            .prepare("EXPLAIN QUERY PLAN SELECT * FROM nodes WHERE canon_level = 'core' AND deleted_at IS NULL ORDER BY path LIMIT 30")
            .unwrap();
        st.query_map([], |r| r.get::<_, String>(3)).unwrap().map(|r| r.unwrap()).collect()
    };
    assert!(plan.iter().any(|p| p.contains("idx_nodes_core")), "{plan:?}");
}

#[test]
fn library_v2_upgrades_to_v3_keeping_providers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.db");
    {
        let mut conn = db::open_connection(&path).unwrap();
        let first_two: Vec<Migration> = db::LIBRARY_MIGRATIONS[..2].iter().map(|m| m.clone_static()).collect();
        db::migrate(&mut conn, &first_two, &dir.path().join("backups"), "library").unwrap();
        conn.execute("INSERT INTO providers (id, config, created_at, updated_at) VALUES ('p1', '{\"name\":\"本地\"}', 0, 0)", []).unwrap();
    }
    let lib = Library::open(dir.path()).unwrap();
    assert_eq!(lib.providers().unwrap().len(), 1, "旧的模型服务配置保留");
    lib.save_managed_model("m1", &serde_json::json!({"id": "m1", "exe": "llama-server.exe"})).unwrap();
    assert_eq!(lib.managed_models().unwrap()[0]["exe"], "llama-server.exe");
    lib.delete_managed_model("m1").unwrap();
    assert!(lib.managed_models().unwrap().is_empty());
    assert!(std::fs::read_dir(dir.path().join("backups")).unwrap().next().is_some(), "升级前有备份");
}

/// v8 → v9：已有数据的世界升级后可以给节点加图片，原有节点不受影响，升级前自动备份。
#[test]
fn upgrade_v8_to_v9_adds_node_images() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("w.cgw");
    {
        let mut conn = db::open_connection(&path).unwrap();
        let v8: Vec<Migration> = WORLD_MIGRATIONS.iter().filter(|m| m.version <= 8).map(|m| m.clone_static()).collect();
        db::migrate(&mut conn, &v8, &dir.path().join("b"), "old").unwrap();
        conn.execute_batch(
            "INSERT INTO world_meta VALUES ('name', '旧世界');
             INSERT INTO nodes (id, kind, sort_key, path, title, created_at, updated_at) VALUES ('n1', 'city', 'V', '/n1/', '雾港', 0, 0);",
        )
        .unwrap();
    }
    let (w, rep) = World::open(&path, &dir.path().join("b")).unwrap();
    assert_eq!((rep.from, rep.to), (8, 10));
    assert!(rep.backup.as_ref().unwrap().exists(), "升级前自动备份");
    assert_eq!(nodes::get_node(w.conn(), "n1").unwrap().unwrap().title, "雾港");
    let png = [0x89u8, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0];
    let img = images::add_image(w.conn(), "n1", &png, 1, 1, "").unwrap();
    assert_eq!(images::list_images(w.conn(), "n1").unwrap()[0].id, img.id);
}

/// v9 → v10：已有图片保留，缩略图为空，升级后可以保存缩略图。
#[test]
fn upgrade_v9_to_v10_adds_thumbs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("w.cgw");
    let png = [0x89u8, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0];
    let img_id;
    {
        let mut conn = db::open_connection(&path).unwrap();
        let v9: Vec<Migration> = WORLD_MIGRATIONS.iter().filter(|m| m.version <= 9).map(|m| m.clone_static()).collect();
        db::migrate(&mut conn, &v9, &dir.path().join("b"), "old").unwrap();
        conn.execute_batch(
            "INSERT INTO world_meta VALUES ('name', '旧世界');
             INSERT INTO nodes (id, kind, sort_key, path, title, created_at, updated_at) VALUES ('n1', 'city', 'V', '/n1/', '雾港', 0, 0);",
        )
        .unwrap();
        img_id = images::add_image(&conn, "n1", &png, 1, 1, "旧图").unwrap().id;
    }
    let (w, rep) = World::open(&path, &dir.path().join("b")).unwrap();
    assert_eq!((rep.from, rep.to), (9, 10));
    assert!(rep.backup.as_ref().unwrap().exists(), "升级前自动备份");
    let cov = images::cover_thumbs(w.conn(), &["n1".to_string()]).unwrap();
    assert_eq!((cov[0].image_id.as_str(), cov[0].thumb.is_none(), cov[0].caption.as_str()), (img_id.as_str(), true, "旧图"));
    images::set_thumb(w.conn(), &img_id, &png).unwrap();
    assert!(images::cover_thumbs(w.conn(), &["n1".to_string()]).unwrap()[0].thumb.is_some());
    assert_eq!(images::image_data(w.conn(), &img_id).unwrap().1, png.to_vec(), "原图不变");
}
