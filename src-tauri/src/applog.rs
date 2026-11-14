//! 错误日志：写到数据目录的 logs\chronicle.log，超过 1 MB 轮换，保留 5 个旧文件。
//! 只记录程序错误与异常退出，不记录剧本或战役内容（模型请求日志另存在资料库并已脱敏）。

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const MAX_BYTES: u64 = 1 << 20;
const KEEP: usize = 5;

static LOG_FILE: OnceLock<PathBuf> = OnceLock::new();
static WRITE_LOCK: Mutex<()> = Mutex::new(());

pub fn log_path() -> Option<&'static Path> {
    LOG_FILE.get().map(|p| p.as_path())
}

/// 初始化日志目录并安装崩溃钩子（崩溃信息写入日志，再交给默认处理）。
pub fn init(data_dir: &Path) {
    let dir = data_dir.join("logs");
    let _ = fs::create_dir_all(&dir);
    let _ = LOG_FILE.set(dir.join("chronicle.log"));
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let bt = std::backtrace::Backtrace::force_capture();
        write("PANIC", &format!("{info}\n{bt}"));
        default(info);
    }));
}

pub fn error(msg: &str) {
    write("ERROR", msg);
}

pub fn info(msg: &str) {
    write("INFO", msg);
}

pub fn warn(msg: &str) {
    write("WARN", msg);
}

fn write(level: &str, msg: &str) {
    eprintln!("[{level}] {msg}");
    let Some(path) = LOG_FILE.get() else { return };
    let _g = WRITE_LOCK.lock();
    rotate_if_needed(path, MAX_BYTES, KEEP);
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{} [{level}] {msg}", utc_now());
    }
}

/// chronicle.log → chronicle.1.log → … → chronicle.{keep}.log（最旧的删除）。
pub(crate) fn rotate_if_needed(path: &Path, max: u64, keep: usize) {
    if fs::metadata(path).map(|m| m.len() < max).unwrap_or(true) {
        return;
    }
    let numbered = |i: usize| path.with_extension(format!("{i}.log"));
    let _ = fs::remove_file(numbered(keep));
    for i in (1..keep).rev() {
        let _ = fs::rename(numbered(i), numbered(i + 1));
    }
    let _ = fs::rename(path, numbered(1));
}

/// UTC 时间 `YYYY-MM-DD HH:MM:SS`（不引入时间库）。
fn utc_now() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // Howard Hinnant 的 civil_from_days 算法。
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_and_keeps_limited_files() {
        let dir = std::env::temp_dir().join(format!("cg-log-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("chronicle.log");
        for round in 0..5 {
            fs::write(&path, vec![b'a' + round; 20]).unwrap();
            rotate_if_needed(&path, 10, 3);
        }
        assert!(!path.exists(), "超过上限的当前日志已轮换");
        assert_eq!(fs::read(dir.join("chronicle.1.log")).unwrap()[0], b'e', "最新的在 .1");
        assert_eq!(fs::read(dir.join("chronicle.3.log")).unwrap()[0], b'c');
        assert!(!dir.join("chronicle.4.log").exists(), "只保留 3 个");
        // 没超过上限不轮换。
        fs::write(&path, b"x").unwrap();
        rotate_if_needed(&path, 10, 3);
        assert!(path.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn utc_format_shape() {
        let s = utc_now();
        assert_eq!(s.len(), 23, "{s}");
        assert!(s.starts_with("20"));
    }
}
