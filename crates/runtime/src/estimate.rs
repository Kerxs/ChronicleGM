//! 显存 / 内存需求估算：根据托管配置的启动参数（模型文件、上下文长度、KV 缓存类型）与 GGUF 元数据。
//! 这是估算，不是测量：用于启动前提示"可能放不下"，实际占用以资源监控为准。

use std::path::Path;

use chronicle_llm::gguf::read_gguf;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct MemEstimate {
    pub model_file: String,
    pub weights_mb: u64,
    pub kv_cache_mb: u64,
    /// 计算缓冲区（按批大小粗估）。
    pub compute_mb: u64,
    /// 多模态投影文件（--mmproj）。
    pub projector_mb: u64,
    pub total_mb: u64,
    pub context: u64,
    pub notes: Vec<String>,
}

fn arg_value<'a>(args: &'a [String], names: &[&str]) -> Option<&'a str> {
    args.iter().position(|a| names.contains(&a.as_str())).and_then(|i| args.get(i + 1)).map(String::as_str)
}

/// KV 缓存每个元素的字节数（llama.cpp 的 --cache-type-k/v）。
fn cache_bytes(t: Option<&str>) -> f64 {
    match t.unwrap_or("f16") {
        "f32" => 4.0,
        "f16" | "bf16" => 2.0,
        "q8_0" => 34.0 / 32.0,
        "q5_0" | "q5_1" => 22.0 / 32.0,
        "q4_0" | "q4_1" | "iq4_nl" => 18.0 / 32.0,
        _ => 2.0,
    }
}

/// 按 llama-server 风格的参数估算。识别不了模型文件时返回 None。
pub fn estimate_llama(args: &[String]) -> Option<MemEstimate> {
    let model = arg_value(args, &["-m", "--model"])?;
    let info = read_gguf(Path::new(model)).ok()?;
    let mut notes = vec![];
    let ctx: u64 = arg_value(args, &["-c", "--ctx-size"]).and_then(|v| v.parse().ok()).filter(|v| *v > 0).or(info.context_length).unwrap_or(4096);
    let ctk = arg_value(args, &["-ctk", "--cache-type-k"]);
    let ctv = arg_value(args, &["-ctv", "--cache-type-v"]);
    let kv_mb = match (info.block_count, info.head_count_kv.or(info.head_count), info.embedding_length) {
        (Some(layers), Some(kv_heads), Some(emb)) => {
            let head_dim = info.head_count.filter(|h| *h > 0).map(|h| emb / h).unwrap_or(128);
            let k = info.key_length.unwrap_or(head_dim) as f64;
            let v = info.value_length.unwrap_or(head_dim) as f64;
            // 混合架构（如 qwen35moe）只有每隔 N 层的完整注意力层有 KV 缓存。
            let kv_layers = match info.full_attention_interval {
                Some(n) if n > 1 => {
                    notes.push(format!("混合注意力架构：每 {n} 层中 1 层有 KV 缓存"));
                    layers.div_ceil(n)
                }
                _ => layers,
            };
            let bytes = kv_layers as f64 * ctx as f64 * kv_heads as f64 * (k * cache_bytes(ctk) + v * cache_bytes(ctv));
            (bytes / 1048576.0).round() as u64
        }
        _ => {
            notes.push("模型元数据不完整，KV 缓存未计入".into());
            0
        }
    };
    let ub: u64 = arg_value(args, &["-ub", "--ubatch-size"]).and_then(|v| v.parse().ok()).unwrap_or(512);
    // 计算缓冲区粗估：随微批大小增长（经验值，非测量）。
    let compute_mb = 256 + ub / 2;
    let projector_mb = arg_value(args, &["--mmproj", "-mm"]).and_then(|p| std::fs::metadata(p).ok()).map(|m| m.len() >> 20).unwrap_or(0);
    if projector_mb > 0 && args.iter().any(|a| a == "--no-mmproj-offload") {
        notes.push("多模态投影放在内存（--no-mmproj-offload）".into());
    }
    let weights_mb = info.file_size >> 20;
    Some(MemEstimate {
        model_file: model.to_string(),
        weights_mb,
        kv_cache_mb: kv_mb,
        compute_mb,
        projector_mb,
        total_mb: weights_mb + kv_mb + compute_mb + projector_mb,
        context: ctx,
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_args_and_rejects_missing_model() {
        let args: Vec<String> = ["-m", "a.gguf", "-c", "4096", "-ctk", "q8_0"].iter().map(|s| s.to_string()).collect();
        assert_eq!(arg_value(&args, &["-c", "--ctx-size"]), Some("4096"));
        assert_eq!(arg_value(&args, &["--model", "-m"]), Some("a.gguf"));
        assert_eq!(arg_value(&args, &["-ub"]), None);
        assert!(estimate_llama(&args).is_none(), "模型文件不存在时不给估算");
    }

    #[test]
    fn cache_type_sizes() {
        assert_eq!(cache_bytes(Some("f16")), 2.0);
        assert!(cache_bytes(Some("q8_0")) < 1.1);
        assert_eq!(cache_bytes(None), 2.0);
    }
}
