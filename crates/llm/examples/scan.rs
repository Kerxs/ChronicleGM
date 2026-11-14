//! 开发工具：扫描文件夹中的 GGUF 模型与服务程序（只读）。
//! 用法：cargo run -p chronicle-llm --example scan -- <文件夹>...

fn main() {
    for dir in std::env::args().skip(1) {
        println!("=== {dir}");
        match chronicle_llm::gguf::scan_folder(std::path::Path::new(&dir), 3) {
            Ok(r) => println!("{}", serde_json::to_string_pretty(&serde_json::json!({
                "models": r.models.iter().map(|m| serde_json::json!({
                    "file": m.path.file_name().map(|f| f.to_string_lossy().to_string()),
                    "size_gb": (m.file_size as f64 / 1e9 * 100.0).round() / 100.0,
                    "gguf_version": m.version,
                    "architecture": m.architecture,
                    "name": m.name,
                    "size_label": m.size_label,
                    "quantization": m.quantization,
                    "context_length": m.context_length,
                    "block_count": m.block_count,
                    "expert_count": m.expert_count,
                    "has_chat_template": m.has_chat_template,
                    "is_projector": m.is_projector,
                })).collect::<Vec<_>>(),
                "errors": r.errors,
                "executables": r.executables,
            })).unwrap()),
            Err(e) => println!("扫描失败: {e}"),
        }
    }
}
