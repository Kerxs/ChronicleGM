//! 阶段 3：下载任务管理器（模拟服务器，不真实下载）。

mod common;

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chronicle_models::download::{DownloadManager, NewTask, TaskFile, TaskState};
use common::*;

fn mgr(root: &std::path::Path, models: &std::path::Path) -> Arc<DownloadManager> {
    let m = DownloadManager::new(root, models, chronicle_models::no_log(), Arc::new(|src: &str| if src == "hf" { (Some("hf_tok".into()), None) } else { (None, None) }));
    *m.retry_base.lock().unwrap() = Duration::from_millis(20);
    m
}

fn wait(m: &DownloadManager, id: &str, secs: u64, f: impl Fn(&TaskState) -> bool) -> TaskState {
    let until = Instant::now() + Duration::from_secs(secs);
    loop {
        let st = m.get(id).unwrap().state;
        if f(&st) {
            return st;
        }
        assert!(Instant::now() < until, "等待超时，当前状态 {st:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn file(m: &Mock, path: &str, body: &[u8], sha: bool) -> TaskFile {
    TaskFile { path: path.into(), url: m.url(&format!("/f/{path}")), size: body.len() as u64, sha256: sha.then(|| sha256(body)), done: false, mmproj: path.contains("mmproj") }
}

#[test]
fn progress_pause_resume_and_restart_recovery() {
    let srv = Mock::start();
    let root = tempfile::tempdir().unwrap();
    let models = tempfile::tempdir().unwrap();
    let model = gguf_bytes("llama", 4096, 3_000_000);
    let proj = gguf_bytes("clip", 0, 10_000);
    srv.route("/f/m.Q4_K_M.gguf", Route { body: model.clone(), content_type: "application/octet-stream".into(), throttle_ms: 60, token: Some("hf_tok".into()), ..Default::default() });
    srv.route("/f/mmproj-f16.gguf", Route { body: proj.clone(), content_type: "application/octet-stream".into(), token: Some("hf_tok".into()), ..Default::default() });
    let m = mgr(root.path(), models.path());
    let done: Arc<Mutex<Vec<String>>> = Arc::default();
    let d2 = done.clone();
    m.set_on_done(Arc::new(move |t| d2.lock().unwrap().push(t.id.clone())));
    m.start();
    let id = m.add(NewTask { source: "hf".into(), repo: "me/model-GGUF".into(), files: vec![file(&srv, "m.Q4_K_M.gguf", &model, true), file(&srv, "mmproj-f16.gguf", &proj, true)] }).unwrap();
    // 有速度和剩余时间；下载中的文件只在 .incoming 里，不在正式位置。
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        let t = m.get(&id).unwrap();
        if t.downloaded > 300_000 && t.speed > 0.0 {
            assert!(t.eta_secs().is_some());
            break;
        }
        assert!(Instant::now() < until, "{t:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
    let final_path = models.path().join("hf").join("me").join("model-GGUF").join("m.Q4_K_M.gguf");
    assert!(!final_path.exists(), "没下完的文件不能出现在模型库位置");
    // 暂停：停下来，临时文件保留。
    m.pause(&id).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let at = srv.served.load(Ordering::Relaxed);
    std::thread::sleep(Duration::from_millis(300));
    assert!(srv.served.load(Ordering::Relaxed) <= at + 64 * 1024, "暂停后不再下载");
    assert_eq!(m.get(&id).unwrap().state, TaskState::Paused);
    // 模拟程序关闭再打开：先继续让它跑起来，然后关掉管理器，用同一个任务文件重新建一个。
    m.resume(&id).unwrap();
    wait(&m, &id, 5, |s| *s == TaskState::Running);
    std::thread::sleep(Duration::from_millis(200));
    m.shutdown();
    drop(m);
    std::thread::sleep(Duration::from_millis(300));
    let m2 = mgr(root.path(), models.path());
    let d3 = done.clone();
    m2.set_on_done(Arc::new(move |t| d3.lock().unwrap().push(t.id.clone())));
    assert_eq!(m2.get(&id).unwrap().state, TaskState::Queued, "重启后未完成的任务自动排队");
    m2.start();
    wait(&m2, &id, 30, |s| *s == TaskState::Done);
    assert_eq!(std::fs::read(&final_path).unwrap(), model, "续传后的文件和原文件一样");
    assert!(!models.path().join(".incoming").join(&id).exists(), "临时目录清理掉了");
    assert_eq!(done.lock().unwrap().as_slice(), std::slice::from_ref(&id), "完成后通知模型库一次");
    let ranges = srv.hits.lock().unwrap().iter().filter(|h| h.0.contains("m.Q4_K_M") && h.1.is_some()).count();
    assert!(ranges >= 1, "继续时用 Range 断点续传");
    assert!(srv.hits.lock().unwrap().iter().all(|h| h.2.as_deref() == Some("Bearer hf_tok")), "私有仓库的令牌只在请求头");
    assert!(!std::fs::read_to_string(root.path().join("downloads.json")).unwrap().contains("hf_tok"), "令牌不写进任务文件");

    // 去重：同样的文件再加一次，直接完成，不再下载。
    let hits = srv.hits.lock().unwrap().len();
    let id2 = m2.add(NewTask { source: "hf".into(), repo: "me/model-GGUF".into(), files: vec![file(&srv, "m.Q4_K_M.gguf", &model, true)] }).unwrap();
    assert_ne!(id2, id);
    assert_eq!(m2.get(&id2).unwrap().state, TaskState::Done);
    assert_eq!(srv.hits.lock().unwrap().len(), hits, "没有重新下载");
    m2.shutdown();
}

#[test]
fn retries_checksum_gguf_check_concurrency_and_cancel() {
    let srv = Mock::start();
    let root = tempfile::tempdir().unwrap();
    let models = tempfile::tempdir().unwrap();
    let m = mgr(root.path(), models.path());
    m.max_concurrent.store(1, Ordering::Relaxed);
    m.start();
    // 1) 服务器老是断：自动重试并续传，最后成功。
    let flaky = gguf_bytes("llama", 2048, 500_000);
    srv.route("/f/flaky.gguf", Route { body: flaky.clone(), content_type: "x".into(), cut_after: Some(150_000), ..Default::default() });
    let a = m.add(NewTask { source: "url".into(), repo: "x/flaky".into(), files: vec![file(&srv, "flaky.gguf", &flaky, false)] }).unwrap();
    // 2) 同时加的第二个任务：并发上限 1，要排队。
    let slow = gguf_bytes("llama", 2048, 200_000);
    srv.route("/f/slow.gguf", Route { body: slow.clone(), content_type: "x".into(), ..Default::default() });
    let b = m.add(NewTask { source: "url".into(), repo: "x/slow".into(), files: vec![file(&srv, "slow.gguf", &slow, true)] }).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    let running = m.list().iter().filter(|t| matches!(t.state, TaskState::Running | TaskState::Retrying { .. })).count();
    assert!(running <= 1, "并发上限生效");
    wait(&m, &a, 30, |s| *s == TaskState::Done);
    assert!(m.get(&a).unwrap().retries >= 2, "重试过");
    wait(&m, &b, 30, |s| *s == TaskState::Done);
    // 3) 校验值不对：失败，文件不进模型库。
    let body = gguf_bytes("llama", 2048, 1000);
    srv.file("/f/bad-sha.gguf", body.clone());
    let mut f = file(&srv, "bad-sha.gguf", &body, false);
    f.sha256 = Some("0".repeat(64));
    let c = m.add(NewTask { source: "url".into(), repo: "x/badsha".into(), files: vec![f] }).unwrap();
    let st = wait(&m, &c, 10, |s| matches!(s, TaskState::Failed { .. }));
    assert!(format!("{st:?}").contains("SHA256"), "{st:?}");
    assert!(!models.path().join("url/x/badsha/bad-sha.gguf").exists());
    // 4) 不是 GGUF（比如下到了一个网页）：失败。
    srv.file("/f/fake.gguf", b"<html>login</html>".to_vec());
    let d = m.add(NewTask { source: "url".into(), repo: "x/fake".into(), files: vec![file(&srv, "fake.gguf", b"<html>login</html>", false)] }).unwrap();
    let st = wait(&m, &d, 10, |s| matches!(s, TaskState::Failed { .. }));
    assert!(format!("{st:?}").contains("GGUF"), "{st:?}");
    // 5) 服务器 500：重试到上限后失败；修好后手动重试成功。
    srv.route("/f/e500.gguf", Route { status: Some(500), ..Default::default() });
    let ok = gguf_bytes("llama", 2048, 100);
    let mut tf = file(&srv, "e500.gguf", &ok, false);
    tf.size = ok.len() as u64;
    let e = m.add(NewTask { source: "url".into(), repo: "x/e500".into(), files: vec![tf] }).unwrap();
    wait(&m, &e, 30, |s| matches!(s, TaskState::Failed { .. }));
    srv.file("/f/e500.gguf", ok);
    m.resume(&e).unwrap();
    wait(&m, &e, 10, |s| *s == TaskState::Done);
    // 6) 取消：删除临时文件。
    let big = gguf_bytes("llama", 2048, 2_000_000);
    srv.route("/f/big.gguf", Route { body: big.clone(), content_type: "x".into(), throttle_ms: 20, ..Default::default() });
    let g = m.add(NewTask { source: "url".into(), repo: "x/big".into(), files: vec![file(&srv, "big.gguf", &big, false)] }).unwrap();
    wait(&m, &g, 10, |s| *s == TaskState::Running);
    std::thread::sleep(Duration::from_millis(300));
    m.cancel(&g).unwrap();
    assert_eq!(m.get(&g).unwrap().state, TaskState::Cancelled);
    assert!(!models.path().join(".incoming").join(&g).exists(), "取消后临时文件删掉");
    // 记录可以移除；进行中的不行。
    m.remove(&g).unwrap();
    assert!(m.get(&g).is_none());
    m.shutdown();
}
