//! llama-server 自动参数与启动前兼容性检查，以及运行状态读取。
//!
//! 按模型（GGUF 元数据）和硬件（空闲显存、内存、CPU 线程）算：
//! GPU 层数、上下文长度、CPU 线程、批大小、KV 缓存类型、Flash Attention，并估算显存 / 内存。
//! 这是估算：启动后以实际占用为准；用户可以在界面上改任何一项。

use std::path::Path;

use chronicle_llm::gguf::GgufInfo;
use serde::{Deserialize, Serialize};

use crate::hw::Hardware;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TuneInput {
    pub model: String,
    #[serde(default)]
    pub mmproj: Option<String>,
    pub port: u16,
    /// 想要的上下文长度（0 = 自动）。
    #[serde(default)]
    pub ctx: u64,
    /// 给其他程序（语音模型、游戏画面）留的显存（MiB）。
    #[serde(default = "reserve")]
    pub vram_reserve_mb: u64,
    /// 手动覆盖（None = 自动）。
    #[serde(default)]
    pub gpu_layers: Option<u32>,
    #[serde(default)]
    pub threads: Option<usize>,
    #[serde(default)]
    pub cache_type: Option<String>,
    #[serde(default)]
    pub flash_attn: Option<bool>,
    #[serde(default)]
    pub batch: Option<u32>,
}

fn reserve() -> u64 {
    1024
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Tuned {
    pub args: Vec<String>,
    pub gpu_layers: u32,
    pub total_layers: u32,
    pub ctx: u64,
    pub threads: usize,
    pub batch: u32,
    pub ubatch: u32,
    pub cache_type: String,
    pub flash_attn: bool,
    pub est_vram_mb: u64,
    pub est_ram_mb: u64,
    /// 能不能跑（内存明显不够时为 false，并在 problems 里说明）。
    pub ok: bool,
    pub problems: Vec<String>,
    pub notes: Vec<String>,
}

const MIB: f64 = 1024.0 * 1024.0;

fn cache_bytes(t: &str) -> f64 {
    match t {
        "q8_0" => 34.0 / 32.0,
        "q4_0" => 18.0 / 32.0,
        _ => 2.0,
    }
}

/// KV 缓存（MiB）。
pub fn kv_mb(info: &GgufInfo, ctx: u64, cache: &str) -> f64 {
    let layers = info.block_count.unwrap_or(32);
    let heads = info.head_count.unwrap_or(32).max(1);
    let kv_heads = info.head_count_kv.unwrap_or(heads);
    let emb = info.embedding_length.unwrap_or(4096);
    let hd = emb / heads;
    let k = info.key_length.unwrap_or(hd) as f64;
    let v = info.value_length.unwrap_or(hd) as f64;
    let kv_layers = match info.full_attention_interval {
        Some(n) if n > 1 => layers.div_ceil(n),
        _ => layers,
    } as f64;
    kv_layers * kv_heads as f64 * (k + v) * ctx as f64 * cache_bytes(cache) / MIB
}

/// 算参数。`weights_bytes`：模型文件总大小（分片要加起来）；`proj_bytes`：mmproj 大小。
pub fn tune(input: &TuneInput, info: &GgufInfo, weights_bytes: u64, proj_bytes: u64, hw: &Hardware) -> Tuned {
    let mut t = Tuned { ok: true, ..Default::default() };
    let layers = info.block_count.unwrap_or(32) as u32;
    t.total_layers = layers + 1;
    let weights = weights_bytes as f64 / MIB;
    let proj = proj_bytes as f64 / MIB;
    let per_layer = weights * 0.96 / layers.max(1) as f64;
    let model_ctx = info.context_length.unwrap_or(8192).max(2048);
    // 上下文：用户指定的，否则 16K（模型支持的话），显存紧张时再往下调。
    let mut ctx = if input.ctx > 0 { input.ctx.min(model_ctx) } else { model_ctx.min(16384) };
    let gpu = !hw.nvidia.is_empty() || hw.vulkan;
    let vram = if hw.nvidia.is_empty() { 0.0 } else { hw.vram_free_mb() as f64 - input.vram_reserve_mb as f64 };
    let mut compute = 400.0 + ctx as f64 / 1024.0 * 20.0;
    // Flash Attention：有显卡就开（KV 量化也需要它）。
    t.flash_attn = input.flash_attn.unwrap_or(gpu && vram > 0.0);
    let mut cache = input.cache_type.clone().unwrap_or_else(|| "f16".into());
    let full = |ctx: u64, cache: &str| weights + proj + kv_mb(info, ctx, cache) + compute;
    let mut gl = 0u32;
    if let Some(n) = input.gpu_layers {
        gl = n.min(t.total_layers);
    } else if vram > 0.0 {
        if full(ctx, &cache) <= vram {
            gl = t.total_layers;
        } else if input.cache_type.is_none() && t.flash_attn && full(ctx, "q8_0") <= vram {
            cache = "q8_0".into();
            gl = t.total_layers;
            t.notes.push("显存刚好不够：KV 缓存改用 q8_0（质量几乎没有损失）".into());
        } else if input.ctx == 0 && full(8192, if t.flash_attn { "q8_0" } else { "f16" }) <= vram {
            ctx = 8192;
            compute = 400.0 + ctx as f64 / 1024.0 * 20.0;
            if t.flash_attn {
                cache = "q8_0".into();
            }
            gl = t.total_layers;
            t.notes.push("显存不够 16K 上下文：上下文改为 8K".into());
        } else {
            // 部分层放显卡，其余在内存。自动模式下先把上下文降到 8K、KV 用 q8_0，好多放几层到显卡。
            if input.ctx == 0 {
                ctx = 8192;
            }
            if input.cache_type.is_none() && t.flash_attn {
                cache = "q8_0".into();
            }
            compute = 400.0 + ctx as f64 / 1024.0 * 20.0;
            let kv = kv_mb(info, ctx, &cache);
            if weights + proj + kv + compute <= vram {
                gl = t.total_layers;
                t.notes.push(format!("显存不够 16K 上下文：上下文改为 {ctx}，KV 缓存用 {cache}，模型全部放显卡"));
            } else {
                let room = vram - kv - compute - proj;
                gl = if room > per_layer { ((room / per_layer) as u32).min(layers) } else { 0 };
                t.notes.push(if gl >= layers { format!("显存刚好：{layers} 层都放显卡，只有输出层在内存") } else { format!("显存放不下整个模型：{gl}/{layers} 层放显卡，其余在内存（会慢一些）") });
            }
        }
    } else if hw.vulkan && hw.nvidia.is_empty() {
        // 查不到显存的显卡（Vulkan 核显 / AMD）：保守地全部放 GPU，失败时用户可手动调低。
        gl = input.gpu_layers.unwrap_or(t.total_layers);
        t.notes.push("查不到显存大小：默认全部放 GPU，启动失败请调低 GPU 层数".into());
    }
    if (cache == "q8_0" || cache == "q4_0") && !t.flash_attn {
        cache = "f16".into();
        t.notes.push("KV 缓存量化需要 Flash Attention，已改回 f16".into());
    }
    let kv = kv_mb(info, ctx, &cache);
    // 显卡上的权重：放上去的层，全部放下时再加上输出层（约 4%）。
    let w_gpu = (per_layer * gl.min(layers) as f64 + if gl >= t.total_layers { weights * 0.04 } else { 0.0 }).min(weights);
    t.est_vram_mb = if gl > 0 { (w_gpu + proj + kv + compute) as u64 } else { 0 };
    t.est_ram_mb = (weights - w_gpu + if gl == 0 { kv + proj } else { 0.0 } + 300.0) as u64;
    // 兼容性：内存不够就不让启动。
    let ram_budget = hw.ram_mb as f64 * 0.85;
    if hw.ram_mb > 0 && t.est_ram_mb as f64 > ram_budget {
        t.ok = false;
        t.problems.push(format!("内存不够：估计需要 {} MB，可用约 {} MB。换小一点的量化版本，或减少上下文", t.est_ram_mb, ram_budget as u64));
    }
    if info.is_projector {
        t.ok = false;
        t.problems.push("这是 mmproj 投影文件，不能单独作为模型运行".into());
    }
    if !info.has_chat_template {
        t.notes.push("模型里没有聊天模板，对话格式可能不对".into());
    }
    t.ctx = ctx;
    t.gpu_layers = gl;
    t.cache_type = cache;
    t.threads = input.threads.unwrap_or_else(|| (hw.cpu_threads / 2).clamp(1, 16));
    t.batch = input.batch.unwrap_or(if gl == t.total_layers { 2048 } else { 512 });
    t.ubatch = t.batch.min(512);
    let mut a: Vec<String> = vec!["-m".into(), input.model.clone()];
    if let Some(p) = input.mmproj.as_deref().filter(|p| Path::new(p).exists()) {
        a.extend(["--mmproj".into(), p.into()]);
    }
    a.extend([
        "--host".into(),
        "127.0.0.1".into(),
        "--port".into(),
        input.port.to_string(),
        "-c".into(),
        t.ctx.to_string(),
        "-ngl".into(),
        t.gpu_layers.to_string(),
        "-t".into(),
        t.threads.to_string(),
        "-b".into(),
        t.batch.to_string(),
        "-ub".into(),
        t.ubatch.to_string(),
        "-fa".into(),
        if t.flash_attn { "on".into() } else { "off".into() },
        "-ctk".into(),
        t.cache_type.clone(),
        "-ctv".into(),
        t.cache_type.clone(),
        // 游戏一次只发一个请求：单槽位，整段上下文都给它，也省显存。
        "-np".into(),
        "1".into(),
        "--jinja".into(),
        "--metrics".into(),
        "--no-webui".into(),
    ]);
    t.args = a;
    t
}

/// 运行中的服务状态（来自 llama-server 的 `/metrics` 与 `/slots`）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ServerStats {
    pub healthy: bool,
    pub prompt_tps: Option<f64>,
    pub gen_tps: Option<f64>,
    pub ctx: Option<u64>,
    pub ctx_used: Option<u64>,
    pub busy: bool,
}

pub fn parse_metrics(text: &str, stats: &mut ServerStats) {
    for l in text.lines().filter(|l| !l.starts_with('#')) {
        let mut it = l.split_whitespace();
        let (Some(k), Some(v)) = (it.next(), it.next()) else { continue };
        let v: f64 = v.parse().unwrap_or(0.0);
        match k {
            "llamacpp:prompt_tokens_seconds" if v > 0.0 => stats.prompt_tps = Some(v),
            "llamacpp:predicted_tokens_seconds" if v > 0.0 => stats.gen_tps = Some(v),
            "llamacpp:requests_processing" => stats.busy = v > 0.0,
            _ => {}
        }
    }
}

pub fn parse_slots(v: &serde_json::Value, stats: &mut ServerStats) {
    let Some(slots) = v.as_array() else { return };
    stats.ctx = slots.first().and_then(|s| s["n_ctx"].as_u64());
    // 上下文用量：提示词 + 已生成（新版字段）；旧版用 n_past / tokens_cached。取所有槽位里最大的。
    stats.ctx_used = slots
        .iter()
        .filter_map(|s| {
            let decoded = s["next_token"].as_array().and_then(|a| a.first()).and_then(|x| x["n_decoded"].as_u64()).unwrap_or(0);
            s["n_prompt_tokens"].as_u64().map(|p| p + decoded).or(s["next_token"].as_array().and_then(|a| a.first()).and_then(|x| x["n_past"].as_u64())).or(s["n_past"].as_u64()).or(s["tokens_cached"].as_u64())
        })
        .max();
    stats.busy |= slots.iter().any(|s| s["is_processing"].as_bool().unwrap_or(false));
}

/// 读取本机 llama-server 的状态（短超时，读不到的项为 None）。
pub fn server_stats(port: u16) -> ServerStats {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_millis(800))).build().into();
    let base = format!("http://127.0.0.1:{port}");
    let mut s = ServerStats { healthy: agent.get(&format!("{base}/health")).call().is_ok(), ..Default::default() };
    if !s.healthy {
        return s;
    }
    if let Ok(mut r) = agent.get(&format!("{base}/metrics")).call()
        && let Ok(t) = r.body_mut().read_to_string()
    {
        parse_metrics(&t, &mut s);
    }
    if let Ok(mut r) = agent.get(&format!("{base}/slots")).call()
        && let Ok(t) = r.body_mut().read_to_string()
        && let Ok(v) = serde_json::from_str(&t)
    {
        parse_slots(&v, &mut s);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info12b() -> GgufInfo {
        // Mistral-Nemo 12B 一类：40 层，5120 维，32 头，8 个 KV 头，头维 128，上下文 1M。
        GgufInfo { block_count: Some(40), embedding_length: Some(5120), head_count: Some(32), head_count_kv: Some(8), key_length: Some(128), value_length: Some(128), context_length: Some(1_024_000), has_chat_template: true, ..Default::default() }
    }
    fn hw(vram_free: u64, ram: u64) -> Hardware {
        Hardware { os: "windows".into(), arch: "x64".into(), cpu_threads: 24, ram_mb: ram, nvidia: if vram_free > 0 { vec![crate::hw::Gpu { vram_mb: vram_free + 500, vram_free_mb: vram_free, ..Default::default() }] } else { vec![] }, cuda: Some((13, 0)), vulkan: true }
    }
    fn input() -> TuneInput {
        TuneInput { model: "m.gguf".into(), mmproj: None, port: 8080, ctx: 0, vram_reserve_mb: 1024, gpu_layers: None, threads: None, cache_type: None, flash_attn: None, batch: None }
    }
    const Q4_12B: u64 = 7_477_000_000;

    #[test]
    fn big_gpu_runs_everything_on_gpu_with_16k() {
        let t = tune(&input(), &info12b(), Q4_12B, 0, &hw(24_000, 32_000));
        assert_eq!(t.gpu_layers, 41);
        assert_eq!(t.ctx, 16384);
        assert_eq!(t.cache_type, "f16");
        assert!(t.flash_attn && t.ok);
        assert_eq!(t.threads, 12);
        assert!(t.args.windows(2).any(|w| w[0] == "--host" && w[1] == "127.0.0.1"), "只监听本机");
    }

    #[test]
    fn eight_gb_laptop_offloads_partially_and_ram_check_blocks_too_big() {
        // 8 GB 显卡（空闲 7 GB）跑 12B Q4（约 7.1 GB）：放不下全部，部分层放显卡。
        let t = tune(&input(), &info12b(), Q4_12B, 0, &hw(7_000, 32_000));
        assert!(t.gpu_layers > 20 && t.gpu_layers < 41, "{t:?}");
        assert!(t.ok);
        assert!(t.est_vram_mb <= 7_000 - 1024 + 50, "不超出预算：{}", t.est_vram_mb);
        // 没有显卡、内存只有 4 GB：不能启动，说明原因。
        let mut cpu_only = hw(0, 4_000);
        cpu_only.vulkan = false;
        let t = tune(&input(), &info12b(), Q4_12B, 0, &cpu_only);
        assert!(!t.ok && t.problems[0].contains("内存不够"));
        assert_eq!(t.gpu_layers, 0);
        // 手动覆盖优先。
        let mut i = input();
        i.gpu_layers = Some(10);
        i.ctx = 4096;
        i.threads = Some(6);
        let t = tune(&i, &info12b(), Q4_12B, 0, &hw(7_000, 32_000));
        assert_eq!((t.gpu_layers, t.ctx, t.threads), (10, 4096, 6));
    }

    #[test]
    fn kv_quantization_when_tight() {
        // 刚好差一点：KV 改 q8_0 就能全部放下。
        let need_f16 = Q4_12B as f64 / MIB + kv_mb(&info12b(), 16384, "f16") + 400.0 + 320.0;
        let need_q8 = Q4_12B as f64 / MIB + kv_mb(&info12b(), 16384, "q8_0") + 400.0 + 320.0;
        let free = ((need_f16 + need_q8) / 2.0) as u64 + 1024;
        let t = tune(&input(), &info12b(), Q4_12B, 0, &hw(free, 32_000));
        assert_eq!((t.gpu_layers, t.cache_type.as_str()), (41, "q8_0"), "{t:?}");
    }

    #[test]
    fn parses_metrics_and_slots() {
        let mut s = ServerStats::default();
        parse_metrics("# HELP x\nllamacpp:prompt_tokens_seconds 812.5\nllamacpp:predicted_tokens_seconds 41.2\nllamacpp:requests_processing 1\n", &mut s);
        parse_slots(&serde_json::json!([{ "n_ctx": 16384, "is_processing": true, "next_token": [{ "n_past": 2048 }] }]), &mut s);
        assert_eq!((s.prompt_tps, s.gen_tps, s.ctx, s.ctx_used, s.busy), (Some(812.5), Some(41.2), Some(16384), Some(2048), true));
        // 新版字段（实测 llama.cpp build 12491）：n_prompt_tokens + n_decoded。
        let mut s = ServerStats::default();
        parse_slots(&serde_json::json!([{ "n_ctx": 8192, "is_processing": false, "n_prompt_tokens": 59, "next_token": [{ "n_decoded": 40 }] }, { "n_ctx": 8192, "is_processing": false }]), &mut s);
        assert_eq!((s.ctx, s.ctx_used, s.busy), (Some(8192), Some(99), false));
    }
}
