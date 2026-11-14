mod common;

use chronicle_core::backup::*;
use chronicle_core::nodes::*;
use chronicle_core::*;
use common::*;
use serde_json::json;

const DAY: i64 = 24 * 3600 * 1000;

fn names(w: &World) -> Vec<String> {
    list_backups(w).unwrap().into_iter().map(|b| b.file_name).collect()
}

#[test]
fn auto_backup_respects_interval_and_rotates_only_auto_files() {
    let e = env();
    std::fs::create_dir_all(e.backups()).unwrap();
    // 预先放一个迁移前备份和一个回滚前备份：轮换绝不能删它们。
    std::fs::write(e.backups().join("test-v1-100.cgw"), b"x").unwrap();
    std::fs::write(e.backups().join("test-before-restore-200.cgw"), b"x").unwrap();
    // 造 5 份很早以前的自动备份。
    for i in 1..=5 {
        std::fs::write(e.backups().join(format!("test-auto-{i}000.cgw")), b"x").unwrap();
    }
    let made = auto_backup(&e.world, 3, DAY).unwrap().expect("距离上次超过一天，应备份");
    assert_eq!(made.kind, "auto");
    let autos: Vec<_> = list_backups(&e.world).unwrap().into_iter().filter(|b| b.kind == "auto").collect();
    assert_eq!(autos.len(), 3, "只保留最近 3 份");
    assert_eq!(autos[0].file_name, made.file_name);
    let all = names(&e.world);
    assert!(all.contains(&"test-v1-100.cgw".into()));
    assert!(all.contains(&"test-before-restore-200.cgw".into()));
    assert!(!all.contains(&"test-auto-1000.cgw".into()));
    // 新备份是可用的世界文件。
    verify_world_file(&e.backups().join(&made.file_name)).unwrap();
    // 一天内再次打开不备份。
    assert!(auto_backup(&e.world, 3, DAY).unwrap().is_none());
}

#[test]
fn restore_from_backup_keeps_current_state_aside() {
    let mut e = env();
    let a = mk(&mut e.world, NewNode::new(NodeKind::City, "白石城"));
    let b = auto_backup(&e.world, 10, DAY).unwrap().unwrap();
    let later = mk(&mut e.world, NewNode::new(NodeKind::City, "备份之后新建"));
    let (w, before) = restore_backup(e.world, &b.file_name).unwrap();
    assert!(get_node(w.conn(), &a.id).unwrap().is_some());
    assert!(get_node(w.conn(), &later.id).unwrap().is_none(), "还原到备份时的状态");
    // 还原前的状态另存了一份，里面有后来新建的节点。
    verify_world_file(&before).unwrap();
    let (old, _) = World::open(&before, &e.dir.path().join("tmp")).unwrap();
    assert!(get_node(old.conn(), &later.id).unwrap().is_some());
    // 非法文件名与损坏文件都被拒绝，且不影响当前世界。
    assert!(restore_backup(w, "../x.cgw").is_err());
}

#[test]
fn restore_rejects_corrupt_backup_without_touching_world() {
    let mut e = env();
    let a = mk(&mut e.world, NewNode::new(NodeKind::City, "白石城"));
    std::fs::create_dir_all(e.backups()).unwrap();
    std::fs::write(e.backups().join("test-auto-1.cgw"), b"not a database at all").unwrap();
    let path = e.path();
    let backups = e.backups();
    assert!(restore_backup(e.world, "test-auto-1.cgw").is_err());
    let (w, _) = World::open(&path, &backups).unwrap();
    assert!(get_node(w.conn(), &a.id).unwrap().is_some());
}

#[test]
fn integrity_detects_and_repairs_derived_data() {
    let mut e = env();
    let root = mk(&mut e.world, NewNode::new(NodeKind::World, "大陆"));
    let city = mk(&mut e.world, NewNode::new(NodeKind::City, "白石城").under(&root.id));
    let inn = mk(&mut e.world, NewNode::new(NodeKind::Site, "酒馆").under(&city.id));
    let lost = mk(&mut e.world, NewNode::new(NodeKind::Site, "孤儿").under(&root.id));
    assert!(integrity::check(&e.world).unwrap().ok);

    // 注入各种派生数据损坏。
    let c = e.world.conn();
    c.execute_batch("INSERT INTO nodes_fts(nodes_fts) VALUES('delete-all');").unwrap();
    c.execute("UPDATE nodes SET path = '/broken/' WHERE id = ?1", [&inn.id]).unwrap();
    c.execute("UPDATE nodes SET sort_key = '###' WHERE id = ?1", [&city.id]).unwrap();
    c.execute("DELETE FROM node_names WHERE node_id = ?1", [&root.id]).unwrap();
    c.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
    c.execute("UPDATE nodes SET parent_id = 'GONE' WHERE id = ?1", [&lost.id]).unwrap();
    c.execute_batch("PRAGMA foreign_keys = ON;").unwrap();

    let rep = integrity::check(&e.world).unwrap();
    let codes: Vec<&str> = rep.issues.iter().map(|i| i.code.as_str()).collect();
    for code in ["fts", "node_paths", "sort_keys", "name_index", "orphan_nodes"] {
        assert!(codes.contains(&code), "应发现 {code}：{codes:?}");
    }
    assert!(!codes.contains(&"foreign_key"), "孤儿节点不应重复报为外键问题");
    assert!(rep.issues.iter().all(|i| i.repairable));

    let fixed = integrity::repair(&mut e.world).unwrap();
    assert!(fixed.after.ok, "修复后应无问题：{:?}", fixed.after.issues);
    assert!(e.backups().join(&fixed.backup).is_file(), "修复前先备份");
    let n = get_node(e.world.conn(), &lost.id).unwrap().unwrap();
    assert_eq!(n.parent_id, None, "孤儿节点移到根级");
    let n = get_node(e.world.conn(), &inn.id).unwrap().unwrap();
    assert_eq!(n.path, format!("/{}/{}/{}/", root.id, city.id, inn.id));
    assert_eq!(search::search_nodes(e.world.conn(), "白石", &search::SearchOptions { limit: 10, ..Default::default() }).unwrap().len(), 1, "全文索引已重建");
    // 移动有修订记录；但撤销会把节点指回不存在的父节点，数据库外键会拒绝，节点保持在根级。
    let commit: String = e.world.conn().query_row("SELECT id FROM commits WHERE message LIKE '修复%' ORDER BY created_at DESC LIMIT 1", [], |r| r.get(0)).unwrap();
    assert!(e.world.undo(&commit, &user()).is_err());
    assert_eq!(get_node(e.world.conn(), &lost.id).unwrap().unwrap().parent_id, None);
    assert!(integrity::check(&e.world).unwrap().ok);
}

#[test]
fn corrupt_file_is_reported_not_repaired() {
    let e = env();
    let path = e.path();
    drop(e.world);
    // 破坏文件中间的页面。
    let mut bytes = std::fs::read(&path).unwrap();
    assert!(bytes.len() > 8192);
    for b in &mut bytes[4096..8192] {
        *b = 0xAB;
    }
    std::fs::write(&path, &bytes).unwrap();
    match World::open(&path, &e.dir.path().join("backups")) {
        Err(_) => {} // 打不开同样会被报告给用户
        Ok((w, _)) => {
            let rep = integrity::check(&w).unwrap_or_else(|_| integrity::IntegrityReport { ok: false, issues: vec![], checked_at: 0 });
            assert!(!rep.ok);
        }
    }
}

#[test]
fn interrupted_jobs_are_marked_failed_on_restart() {
    let e = env();
    let c = e.world.conn();
    let running = jobs::create_job(c, "outline", "s1", &json!({})).unwrap();
    jobs::mark_running(c, &running.id, "p", "m", "d", "t").unwrap();
    let done = jobs::create_job(c, "outline", "s1", &json!({})).unwrap();
    jobs::finish(c, &done.id, jobs::JobStatus::Succeeded, Some("{}"), None).unwrap();
    assert_eq!(jobs::recover_interrupted(c).unwrap(), 1);
    let j = jobs::get_job(c, &running.id).unwrap();
    assert_eq!(j.status, jobs::JobStatus::Failed);
    assert!(j.error.unwrap_or_default().contains("中断"));
    assert_eq!(jobs::get_job(c, &done.id).unwrap().status, jobs::JobStatus::Succeeded);
}
