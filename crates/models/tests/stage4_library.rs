//! 阶段 4：模型库与文件完整性检查。

mod common;

use std::path::Path;

use chronicle_models::download::{Task, TaskFile, TaskState};
use chronicle_models::library::{Integrity, Library};
use common::*;

fn write(p: &Path, b: &[u8]) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, b).unwrap();
}

#[test]
fn library_add_info_mmproj_default_check_remove_and_move() {
    let root = tempfile::tempdir().unwrap();
    let models = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let mut lib = Library::open(root.path(), models.path(), chronicle_models::no_log());

    // 添加已有文件（在模型目录外）：不复制，读出 GGUF 信息；第一个自动成为默认模型；重复添加只记一次。
    let ext = outside.path().join("my-12b.Q5_K_M.gguf");
    write(&ext, &gguf_bytes("llama", 8192, 100));
    let a = lib.add_file(&ext, None).unwrap();
    assert_eq!((a.arch.as_deref(), a.context_length, a.quant.as_deref(), a.source.as_str()), (Some("llama"), Some(8192), Some("Q5_K_M"), "local"));
    assert_eq!(lib.default_model().unwrap().id, a.id);
    assert_eq!(lib.add_file(&ext, None).unwrap().id, a.id);
    assert_eq!(lib.data.models.len(), 1);
    // 不是 GGUF / 是 mmproj：拒绝单独添加。
    let junk = outside.path().join("junk.gguf");
    write(&junk, b"not a model");
    assert!(lib.add_file(&junk, None).unwrap_err().contains("GGUF"));
    let proj = outside.path().join("mmproj-f16.gguf");
    write(&proj, &gguf_bytes("clip", 0, 10));
    assert!(lib.add_file(&proj, None).unwrap_err().contains("mmproj"));
    lib.link_mmproj(&a.id, Some(&proj)).unwrap();
    assert!(lib.get(&a.id).unwrap().mmproj.is_some());
    assert!(lib.link_mmproj(&a.id, Some(&ext)).unwrap_err().contains("不是多模态"));

    // 下载完成的任务：主模型加入，mmproj 自动关联。
    let dl_main = models.path().join("hf/me/x-GGUF/x.Q4_K_M.gguf");
    let dl_proj = models.path().join("hf/me/x-GGUF/mmproj-x-f16.gguf");
    write(&dl_main, &gguf_bytes("qwen3", 32768, 1000));
    write(&dl_proj, &gguf_bytes("clip", 0, 10));
    let t = Task {
        id: "d1".into(),
        source: "hf".into(),
        repo: "me/x-GGUF".into(),
        files: vec![
            TaskFile { path: "x.Q4_K_M.gguf".into(), url: "http://x/a".into(), size: std::fs::metadata(&dl_main).unwrap().len(), sha256: None, done: true, mmproj: false },
            TaskFile { path: "mmproj-x-f16.gguf".into(), url: "http://x/b".into(), size: 1, sha256: None, done: true, mmproj: true },
        ],
        state: TaskState::Done,
        created_at: 0,
        finished_at: None,
        outputs: vec![dl_main.display().to_string(), dl_proj.display().to_string()],
        downloaded: 0,
        speed: 0.0,
        retries: 0,
    };
    let b = lib.add_from_task(&t).unwrap().unwrap();
    assert_eq!((b.source.as_str(), b.repo.as_str(), b.arch.as_deref()), ("hf", "me/x-GGUF", Some("qwen3")));
    assert!(b.mmproj.as_deref().unwrap().ends_with("mmproj-x-f16.gguf"));
    lib.set_default(&b.id).unwrap();
    assert_eq!(lib.default_model().unwrap().id, b.id);
    assert_eq!(lib.info(&b.id).unwrap().context_length, Some(32768));

    // 完整性检查：好的 / 被截断 / 被删。
    assert_eq!(lib.check(&b.id), Integrity::Ok);
    let bytes = std::fs::read(&ext).unwrap();
    std::fs::write(&ext, &bytes[..bytes.len() - 50]).unwrap();
    assert!(matches!(lib.check(&a.id), Integrity::SizeChanged { .. }));
    std::fs::write(&ext, b"GGUF garbage").unwrap();
    let mut len_fix = std::fs::read(&ext).unwrap();
    len_fix.resize(bytes.len(), 0);
    std::fs::write(&ext, &len_fix).unwrap();
    assert!(matches!(lib.check(&a.id), Integrity::Corrupt { .. }));
    std::fs::write(&ext, &bytes).unwrap();

    // 修改保存位置：模型目录里的文件搬走，外部文件不动，记录更新，重开后还在。
    let new_dir = tempfile::tempdir().unwrap();
    let n = lib.move_dir(new_dir.path()).unwrap();
    assert_eq!(n, 2, "主模型和 mmproj");
    let moved = lib.get(&b.id).unwrap().clone();
    assert!(Path::new(&moved.path).exists() && moved.path.contains(&*new_dir.path().file_name().unwrap().to_string_lossy()));
    assert!(!dl_main.exists());
    assert!(ext.exists(), "外部添加的文件不搬");
    let lib2 = Library::open(root.path(), models.path(), chronicle_models::no_log());
    assert_eq!(lib2.get(&b.id).unwrap().path, moved.path);

    // 删除：正在使用的不能删；删文件只删模型目录里的；外部文件只删记录。
    assert!(lib.remove(&b.id, true, std::slice::from_ref(&moved.path)).unwrap_err().contains("正在使用"));
    let deleted = lib.remove(&b.id, true, &[]).unwrap();
    assert_eq!(deleted.len(), 2, "主模型和它独占的 mmproj");
    assert!(!Path::new(&moved.path).exists());
    let deleted = lib.remove(&a.id, true, &[]).unwrap();
    assert!(deleted.is_empty() && ext.exists(), "模型目录外的文件不删");
    assert!(lib.data.models.is_empty() && lib.data.default.is_none());
}
