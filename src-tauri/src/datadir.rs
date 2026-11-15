//! 数据目录：默认 `%LOCALAPPDATA%\ChronicleGM`，可以在设置里迁移到别的位置。
//!
//! - 位置记在数据目录之外的指针文件 `%LOCALAPPDATA%\ChronicleGM-location.txt`（迁移后旧目录里的设置不会再被读到）；
//! - 迁移 = 复制 → 校验 → 切换（重启后生效）。数据库用 SQLite 的 VACUUM INTO 复制一致的快照，其他文件按字节复制并比对大小；
//! - 目标文件夹必须是空的（或不存在），不在当前数据目录里面，并且能写入；旧目录原样保留，由用户确认后自己删除。

use std::path::{Path, PathBuf};

use serde::Serialize;

fn local_appdata() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

pub fn default_dir() -> PathBuf {
    local_appdata().join("ChronicleGM")
}

fn pointer_file() -> PathBuf {
    // 测试用：CHRONICLE_POINTER_FILE 指定别的位置，避免改到真实设置。
    if let Some(p) = std::env::var_os("CHRONICLE_POINTER_FILE") {
        return PathBuf::from(p);
    }
    local_appdata().join("ChronicleGM-location.txt")
}

/// 指针文件里记的位置（没有或读不到为 None）。
pub fn pointer() -> Option<PathBuf> {
    let s = std::fs::read_to_string(pointer_file()).ok()?;
    let p = PathBuf::from(s.trim());
    (!s.trim().is_empty()).then_some(p)
}

/// 启动时用的数据目录与警告（指针指向的目录不存在时回到默认目录，并提示）。
pub fn resolve() -> (PathBuf, Option<String>) {
    if let Some(d) = std::env::var_os("CHRONICLE_DATA_DIR") {
        return (PathBuf::from(d), None);
    }
    match pointer() {
        Some(p) if p.join("library.db").exists() => (p, None),
        Some(p) => (
            default_dir(),
            Some(format!("设置的数据目录「{}」找不到资料库（可能是移动硬盘没有连接，或文件夹被移动了），这次先使用默认目录。重新连接后重启程序即可。", p.display())),
        ),
        None => (default_dir(), None),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MigrateReport {
    pub files: usize,
    pub bytes: u64,
    pub databases: usize,
    pub target: String,
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        let p = e.path();
        if e.file_type()?.is_dir() {
            walk(&p, out)?;
        } else {
            out.push(p);
        }
    }
    Ok(())
}

fn is_sqlite(p: &Path) -> bool {
    matches!(p.extension().and_then(|e| e.to_str()), Some("db" | "cgw"))
}

/// 规范化路径：目标可能还不存在，就规范化最近的已存在的上级，再接上剩下的部分。
fn canon(p: &Path) -> PathBuf {
    let mut rest = vec![];
    let mut cur = p.to_path_buf();
    loop {
        if let Ok(c) = cur.canonicalize() {
            let mut out = c;
            for r in rest.iter().rev() {
                out.push(r);
            }
            return out;
        }
        match (cur.file_name().map(|f| f.to_os_string()), cur.parent().map(Path::to_path_buf)) {
            (Some(f), Some(parent)) => {
                rest.push(f);
                cur = parent;
            }
            _ => return p.to_path_buf(),
        }
    }
}

/// 检查目标位置能不能用（不复制）。
pub fn check_target(current: &Path, target: &Path) -> Result<(), String> {
    if !target.is_absolute() {
        return Err("请选择一个完整的文件夹路径".into());
    }
    let cur = canon(current);
    let tgt = canon(target);
    if tgt == cur {
        return Err("这就是现在的数据目录".into());
    }
    if tgt.starts_with(&cur) {
        return Err("不能迁移到现在的数据目录里面".into());
    }
    if target.exists() {
        if !target.is_dir() {
            return Err("目标位置是一个文件，不是文件夹".into());
        }
        if std::fs::read_dir(target).map_err(|e| format!("读不了目标文件夹：{e}"))?.next().is_some() {
            return Err("目标文件夹不是空的：为了不覆盖里面的文件，请选一个空文件夹（或新建一个）".into());
        }
    } else {
        std::fs::create_dir_all(target).map_err(|e| format!("无法创建目标文件夹（可能没有权限）：{e}"))?;
    }
    let probe = target.join(".chronicle-write-test");
    std::fs::write(&probe, b"ok").map_err(|e| format!("目标文件夹不能写入（可能没有权限或是只读的）：{e}"))?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

/// 复制并校验；成功后写指针文件（重启后生效）。失败时删掉已经复制的部分，指针不变。
pub fn migrate(current: &Path, target: &Path) -> Result<MigrateReport, String> {
    check_target(current, target)?;
    let mut files = vec![];
    walk(current, &mut files).map_err(|e| format!("读取当前数据目录失败：{e}"))?;
    let mut report = MigrateReport { files: 0, bytes: 0, databases: 0, target: target.display().to_string() };
    let res = (|| -> Result<(), String> {
        for src in &files {
            let rel = src.strip_prefix(current).map_err(|e| e.to_string())?;
            // 数据库的临时文件（-wal / -shm / -journal）不复制：VACUUM INTO 得到的是完整一致的快照。
            let name = rel.to_string_lossy();
            if name.ends_with("-wal") || name.ends_with("-shm") || name.ends_with("-journal") {
                continue;
            }
            let dst = target.join(rel);
            if let Some(p) = dst.parent() {
                std::fs::create_dir_all(p).map_err(|e| format!("创建文件夹失败：{e}"))?;
            }
            if is_sqlite(src) {
                let conn = chronicle_core::rusqlite::Connection::open_with_flags(src, chronicle_core::rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| format!("打开 {name} 失败：{e}"))?;
                conn.execute("VACUUM INTO ?1", [dst.to_string_lossy().as_ref()]).map_err(|e| format!("复制数据库 {name} 失败：{e}"))?;
                drop(conn);
                let check = chronicle_core::rusqlite::Connection::open(&dst).map_err(|e| format!("校验 {name} 失败：{e}"))?;
                let ok: String = check.query_row("PRAGMA integrity_check", [], |r| r.get(0)).map_err(|e| format!("校验 {name} 失败：{e}"))?;
                if ok != "ok" {
                    return Err(format!("复制后的数据库 {name} 校验没有通过：{ok}"));
                }
                report.databases += 1;
            } else {
                std::fs::copy(src, &dst).map_err(|e| format!("复制 {name} 失败（空间不够或没有权限？）：{e}"))?;
                let (a, b) = (std::fs::metadata(src).map(|m| m.len()).unwrap_or(0), std::fs::metadata(&dst).map(|m| m.len()).unwrap_or(1));
                if a != b {
                    return Err(format!("复制后的文件 {name} 大小不一致"));
                }
            }
            report.files += 1;
            report.bytes += std::fs::metadata(&dst).map(|m| m.len()).unwrap_or(0);
        }
        if !target.join("library.db").exists() {
            return Err("复制后没有找到资料库文件".into());
        }
        std::fs::write(pointer_file(), target.display().to_string()).map_err(|e| format!("保存新位置失败：{e}"))?;
        Ok(())
    })();
    if let Err(e) = res {
        // 清理复制了一半的内容（目标原来是空的）。
        if let Ok(rd) = std::fs::read_dir(target) {
            for e in rd.flatten() {
                let p = e.path();
                let _ = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
            }
        }
        return Err(format!("迁移没有完成，数据仍在原来的位置：{e}"));
    }
    Ok(report)
}

/// 回到默认目录（删掉指针文件；重启后生效，不复制数据）。
pub fn reset() -> Result<(), String> {
    match std::fs::remove_file(pointer_file()) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_must_be_empty_writable_and_outside() {
        let cur = tempfile::tempdir().unwrap();
        std::fs::write(cur.path().join("library.db"), b"x").unwrap();
        assert!(check_target(cur.path(), Path::new("relative")).is_err());
        assert!(check_target(cur.path(), cur.path()).is_err());
        assert!(check_target(cur.path(), &cur.path().join("sub")).unwrap_err().contains("里面"));
        let other = tempfile::tempdir().unwrap();
        std::fs::write(other.path().join("a.txt"), b"x").unwrap();
        assert!(check_target(cur.path(), other.path()).unwrap_err().contains("不是空的"));
        let empty = tempfile::tempdir().unwrap();
        assert!(check_target(cur.path(), empty.path()).is_ok());
        assert!(check_target(cur.path(), &empty.path().join("new")).is_ok(), "不存在的文件夹会创建");
    }

    #[test]
    fn migrate_copies_verifies_and_switches_pointer() {
        let base = tempfile::tempdir().unwrap();
        // SAFETY: 只有这个测试读写这个环境变量。
        unsafe { std::env::set_var("CHRONICLE_POINTER_FILE", base.path().join("ptr.txt")) };
        let cur = base.path().join("old");
        std::fs::create_dir_all(cur.join("worlds")).unwrap();
        let conn = chronicle_core::rusqlite::Connection::open(cur.join("library.db")).unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE t(x); INSERT INTO t VALUES (42);").unwrap();
        std::fs::write(cur.join("worlds").join("notes.txt"), "笔记").unwrap();
        let target = base.path().join("new");
        let rep = migrate(&cur, &target).unwrap();
        assert_eq!(rep.databases, 1);
        let c2 = chronicle_core::rusqlite::Connection::open(target.join("library.db")).unwrap();
        assert_eq!(c2.query_row("SELECT x FROM t", [], |r| r.get::<_, i64>(0)).unwrap(), 42, "数据库内容完整（包括还在 WAL 里的）");
        assert_eq!(std::fs::read_to_string(target.join("worlds").join("notes.txt")).unwrap(), "笔记");
        assert_eq!(pointer().unwrap(), target);
        assert!(cur.join("library.db").exists(), "旧目录原样保留");
        drop(conn);
        reset().unwrap();
        assert!(pointer().is_none());
    }
}
