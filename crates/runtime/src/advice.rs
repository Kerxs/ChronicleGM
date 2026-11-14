//! 内存建议：托管的大语言模型 + 其他托管模型 能否同时放进物理内存，放不下时建议的启动参数。
//!
//! 依据（2026-11-14 本机实测，docs/launch-params-report.md）：llama-server 默认把用过的提示词状态缓存在内存里，
//! 上限 `--cache-ram 8192` MiB；Ornith-35B 每换一段约 4700 token 的新提示词，私有内存涨约 238 MB，一直涨到上限。
//! 设成 `--cache-ram 2048` 后涨到约 2.3 GB 就不再涨。所以峰值 ≈ 加载后的内存 + 缓存上限。
//!
//! 内存都按私有内存算（工作集会偏小：本机 llama 私有 28.4 GB 时工作集只有 22.4 GB）。
//!
//! 这里只给建议，不改任何配置；改不改由用户在界面上决定。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::estimate::estimate_llama;
use crate::monitor::ModelMem;
use crate::procs::{ModelProfile, ModelRole};

/// 给系统和本程序留的内存（MiB）。
pub const SYSTEM_RESERVE_MB: u64 = 3072;
/// llama-server 的默认提示词缓存上限（MiB），`--help` 里写的。
pub const DEFAULT_CACHE_RAM_MB: u64 = 8192;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct MemoryAdvice {
    pub profile_id: String,
    /// 加载后的内存：正在运行时用加载完成时实测的私有内存，否则按模型文件估算。
    pub base_mb: u64,
    /// base_mb 是实测（true）还是按模型文件估算（false）。
    pub base_measured: bool,
    /// 提示词缓存上限；None = 不限制（-1）。
    pub cache_mb: Option<u64>,
    /// 同时托管的其他模型（正在运行的按实测私有内存的峰值）。
    pub others: Vec<(String, u64)>,
    /// 没有运行、无法计入的其他托管模型。
    pub others_unknown: Vec<String>,
    pub total_ram_mb: u64,
    pub reserve_mb: u64,
    /// 峰值估计；缓存不限制时为 None。
    pub peak_mb: Option<u64>,
    /// 峰值会超过物理内存。
    pub over: bool,
    /// 建议的 --cache-ram（MiB，512 的倍数）；None 表示当前设置已经够用。
    pub suggested_cache_mb: Option<u64>,
    pub notes: Vec<String>,
}

fn arg_value<'a>(args: &'a [String], names: &[&str]) -> Option<&'a str> {
    args.iter().position(|a| names.contains(&a.as_str())).and_then(|i| args.get(i + 1)).map(String::as_str)
}

/// 参数里的提示词缓存上限：Some(n) = n MiB（0 = 关闭），None = 不限制。
pub fn cache_ram(args: &[String]) -> Option<u64> {
    match arg_value(args, &["--cache-ram", "-cram"]).and_then(|v| v.parse::<i64>().ok()) {
        Some(n) if n < 0 => None,
        Some(n) => Some(n as u64),
        None => Some(DEFAULT_CACHE_RAM_MB),
    }
}

/// 计算一个大语言模型托管配置的内存建议。
/// `measured`：正在运行的托管模型的内存记录（配置 ID → 基线 / 峰值，私有内存）。
/// 调用方在参数改过（与运行中的不同）时应去掉这个配置自己的记录。
pub fn memory_advice(profile: &ModelProfile, all: &[ModelProfile], measured: &HashMap<String, ModelMem>, total_ram_mb: u64) -> Option<MemoryAdvice> {
    if profile.role != ModelRole::Llm {
        return None;
    }
    let mut notes = vec![];
    let cache = cache_ram(&profile.args);
    let (base_mb, base_measured) = match measured.get(&profile.id) {
        Some(m) => {
            notes.push(format!("加载后内存是实测的（加载完成时的私有内存）；现在 {:.1} GB，运行以来最高 {:.1} GB", m.current_mb as f64 / 1024.0, m.peak_mb as f64 / 1024.0));
            (m.baseline_mb, true)
        }
        None => {
            let est = estimate_llama(&profile.args)?;
            notes.push("加载后内存是按模型文件估算的，实际通常更多（本机 Ornith-35B 实测比估算多约 2 GB）；启动模型后会改用实测值".into());
            (est.total_mb, false)
        }
    };
    let mut others = vec![];
    let mut others_unknown = vec![];
    for p in all.iter().filter(|p| p.id != profile.id) {
        match measured.get(&p.id) {
            Some(m) => others.push((p.name.clone(), m.peak_mb)),
            None => others_unknown.push(p.name.clone()),
        }
    }
    let others_mb: u64 = others.iter().map(|(_, m)| m).sum();
    let budget = total_ram_mb.saturating_sub(SYSTEM_RESERVE_MB);
    let peak = cache.map(|c| base_mb + c + others_mb);
    let over = peak.is_none_or(|p| p > budget);
    let room = budget.saturating_sub(base_mb + others_mb);
    let fit = (room.min(DEFAULT_CACHE_RAM_MB) / 512) * 512;
    let suggested = if over { Some(fit) } else { None };
    if cache.is_none() {
        notes.push("提示词缓存设为不限制（-1）：长时间游玩内存会一直上涨".into());
    }
    if over && fit < 1024 {
        notes.push("即使把缓存降到很小也放不下：可以减小上下文长度、去掉视觉模块（不能看图）或减少同时托管的模型".into());
    }
    if !others_unknown.is_empty() {
        notes.push(format!("没有运行、无法计入：{}（启动后再看一次）", others_unknown.join("、")));
    }
    Some(MemoryAdvice {
        profile_id: profile.id.clone(),
        base_mb,
        base_measured,
        cache_mb: cache,
        others,
        others_unknown,
        total_ram_mb,
        reserve_mb: SYSTEM_RESERVE_MB,
        peak_mb: peak,
        over,
        suggested_cache_mb: suggested,
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem(mb: u64) -> ModelMem {
        ModelMem { baseline_mb: mb, peak_mb: mb, current_mb: mb }
    }

    fn prof(id: &str, role: ModelRole, args: &[&str]) -> ModelProfile {
        ModelProfile {
            id: id.into(),
            name: id.into(),
            role,
            exe: String::new(),
            args: args.iter().map(|s| s.to_string()).collect(),
            cwd: String::new(),
            port: 8080,
            health_path: String::new(),
            start_timeout_secs: 60,
            auto_restart: true,
            idle_unload_secs: None,
            exclusive_group: String::new(),
            gpu_reserve_mb: 0,
        }
    }

    #[test]
    fn reads_cache_ram() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(cache_ram(&a(&["-c", "4096"])), Some(8192));
        assert_eq!(cache_ram(&a(&["--cache-ram", "2048"])), Some(2048));
        assert_eq!(cache_ram(&a(&["-cram", "0"])), Some(0));
        assert_eq!(cache_ram(&a(&["--cache-ram", "-1"])), None);
    }

    /// 本机实测数字：32 GB 内存，llama 加载后约 25.4 GB，Breeze 约 5 GB → 默认 8 GB 缓存放不下。
    #[test]
    fn default_cache_overflows_32gb_with_tts() {
        let llm = prof("llama", ModelRole::Llm, &["-c", "32768"]);
        let tts = prof("breeze", ModelRole::Tts, &[]);
        let measured = HashMap::from([("breeze".to_string(), mem(5000)), ("llama".to_string(), mem(25432))]);
        let a = memory_advice(&llm, &[llm.clone(), tts.clone()], &measured, 32547).unwrap();
        assert!(a.base_measured);
        assert_eq!(a.peak_mb, Some(25432 + 8192 + 5000));
        assert!(a.over);
        // 32547 - 3072 - 25432 - 5000 = -957 → 放不下任何缓存，建议 0 并提示其他办法。
        assert_eq!(a.suggested_cache_mb, Some(0));
        assert!(a.notes.iter().any(|n| n.contains("上下文")));

        // 只有 llama 时：32547 - 3072 - 25432 = 4043 → 建议 3584。
        let only = HashMap::from([("llama".to_string(), mem(25432))]);
        let b = memory_advice(&llm, std::slice::from_ref(&llm), &only, 32547).unwrap();
        assert_eq!(b.suggested_cache_mb, Some(3584));

        // 已经设成 2048：不超，不建议。
        let ok = prof("llama", ModelRole::Llm, &["--cache-ram", "2048"]);
        let c = memory_advice(&ok, std::slice::from_ref(&ok), &only, 32547).unwrap();
        assert!(!c.over);
        assert_eq!(c.suggested_cache_mb, None);
    }

    #[test]
    fn unlimited_cache_is_always_over_and_unknown_others_are_listed() {
        let llm = prof("llama", ModelRole::Llm, &["--cache-ram", "-1"]);
        let tts = prof("breeze", ModelRole::Tts, &[]);
        let a = memory_advice(&llm, &[llm.clone(), tts], &HashMap::from([("llama".to_string(), mem(20000))]), 65536).unwrap();
        assert!(a.over);
        assert_eq!(a.peak_mb, None);
        assert_eq!(a.others_unknown, vec!["breeze".to_string()]);
        assert!(memory_advice(&prof("t", ModelRole::Tts, &[]), &[], &HashMap::new(), 1).is_none());
    }
}
