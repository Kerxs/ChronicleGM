//! GGUF 模型文件识别：只读解析文件头元数据，不加载权重、不修改文件。

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{LlmError, Result};

const MAX_KV: u64 = 100_000;
const MAX_STRING: u64 = 64 * 1024 * 1024;
const MAX_TEMPLATE_CHARS: usize = 8000;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct GgufInfo {
    pub path: PathBuf,
    pub file_size: u64,
    pub version: u32,
    pub architecture: Option<String>,
    pub name: Option<String>,
    pub size_label: Option<String>,
    pub quantization: Option<String>,
    pub context_length: Option<u64>,
    pub block_count: Option<u64>,
    pub expert_count: Option<u64>,
    /// 显存估算用：隐藏层宽度、注意力头数、KV 头数、每头的 K/V 维度。
    #[serde(default)]
    pub embedding_length: Option<u64>,
    #[serde(default)]
    pub head_count: Option<u64>,
    #[serde(default)]
    pub head_count_kv: Option<u64>,
    #[serde(default)]
    pub key_length: Option<u64>,
    #[serde(default)]
    pub value_length: Option<u64>,
    /// 混合架构中每隔几层才有一层完整注意力（有 KV 缓存）；没有该字段时视为每层都有。
    #[serde(default)]
    pub full_attention_interval: Option<u64>,
    pub has_chat_template: bool,
    /// 截断后的 chat template（仅供查看）。
    pub chat_template: Option<String>,
    /// 多模态投影文件（mmproj），不能单独作为对话模型。
    pub is_projector: bool,
    /// 分卷文件的序号与总数（如 1/3）。
    pub split: Option<(u32, u32)>,
    /// 推测用途：chat（对话模型）/ projector（多模态投影）/ tts（语音）/ other。
    pub role: String,
}

#[derive(Debug)]
#[allow(dead_code)] // 浮点与布尔值目前只需跳过，保留以便后续读取
enum Val {
    U(u64),
    I(i64),
    F(f64),
    B(bool),
    S(String),
    Skipped,
}

struct R<T: Read + Seek> {
    r: T,
}

impl<T: Read + Seek> R<T> {
    fn u8(&mut self) -> Result<u8> {
        let mut b = [0u8; 1];
        self.r.read_exact(&mut b)?;
        Ok(b[0])
    }
    fn u32(&mut self) -> Result<u32> {
        let mut b = [0u8; 4];
        self.r.read_exact(&mut b)?;
        Ok(u32::from_le_bytes(b))
    }
    fn u64(&mut self) -> Result<u64> {
        let mut b = [0u8; 8];
        self.r.read_exact(&mut b)?;
        Ok(u64::from_le_bytes(b))
    }
    fn skip(&mut self, n: u64) -> Result<()> {
        self.r.seek(SeekFrom::Current(n as i64))?;
        Ok(())
    }
    fn string(&mut self, keep: bool) -> Result<Option<String>> {
        let len = self.u64()?;
        if len > MAX_STRING {
            return Err(LlmError::BadResponse("GGUF 字符串长度异常".into()));
        }
        if !keep {
            self.skip(len)?;
            return Ok(None);
        }
        let mut buf = vec![0u8; len as usize];
        self.r.read_exact(&mut buf)?;
        Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
    }

    fn scalar_size(t: u32) -> Option<u64> {
        Some(match t {
            0 | 1 | 7 => 1,
            2 | 3 => 2,
            4..=6 => 4,
            10..=12 => 8,
            _ => return None,
        })
    }

    fn value(&mut self, t: u32, keep: bool) -> Result<Val> {
        Ok(match t {
            0 => Val::U(self.u8()? as u64),
            1 => Val::I(self.u8()? as i8 as i64),
            2 => {
                let mut b = [0u8; 2];
                self.r.read_exact(&mut b)?;
                Val::U(u16::from_le_bytes(b) as u64)
            }
            3 => {
                let mut b = [0u8; 2];
                self.r.read_exact(&mut b)?;
                Val::I(i16::from_le_bytes(b) as i64)
            }
            4 => Val::U(self.u32()? as u64),
            5 => Val::I(self.u32()? as i32 as i64),
            6 => Val::F(f32::from_bits(self.u32()?) as f64),
            7 => Val::B(self.u8()? != 0),
            8 => match self.string(keep)? {
                Some(s) => Val::S(s),
                None => Val::Skipped,
            },
            9 => {
                let et = self.u32()?;
                let n = self.u64()?;
                if let Some(sz) = Self::scalar_size(et) {
                    self.skip(sz.checked_mul(n).ok_or_else(|| LlmError::BadResponse("GGUF 数组过大".into()))?)?;
                } else if et == 8 {
                    for _ in 0..n {
                        self.string(false)?;
                    }
                } else {
                    for _ in 0..n {
                        self.value(et, false)?;
                    }
                }
                Val::Skipped
            }
            10 => Val::U(self.u64()?),
            11 => Val::I(self.u64()? as i64),
            12 => Val::F(f64::from_bits(self.u64()?)),
            other => return Err(LlmError::BadResponse(format!("GGUF 未知值类型 {other}"))),
        })
    }
}

/// llama.cpp `LLAMA_FTYPE` 编号对应的量化名称。
pub fn quant_name(file_type: u64) -> String {
    let s = match file_type {
        0 => "F32",
        1 => "F16",
        2 => "Q4_0",
        3 => "Q4_1",
        7 => "Q8_0",
        8 => "Q5_0",
        9 => "Q5_1",
        10 => "Q2_K",
        11 => "Q3_K_S",
        12 => "Q3_K_M",
        13 => "Q3_K_L",
        14 => "Q4_K_S",
        15 => "Q4_K_M",
        16 => "Q5_K_S",
        17 => "Q5_K_M",
        18 => "Q6_K",
        19 => "IQ2_XXS",
        20 => "IQ2_XS",
        21 => "Q2_K_S",
        22 => "IQ3_XS",
        23 => "IQ3_XXS",
        24 => "IQ1_S",
        25 => "IQ4_NL",
        26 => "IQ3_S",
        27 => "IQ3_M",
        28 => "IQ2_S",
        29 => "IQ2_M",
        30 => "IQ4_XS",
        31 => "IQ1_M",
        32 => "BF16",
        36 => "TQ1_0",
        37 => "TQ2_0",
        38 => "MXFP4_MOE",
        other => return format!("类型 {other}"),
    };
    s.to_string()
}

fn split_of(path: &Path) -> Option<(u32, u32)> {
    // xxx-00001-of-00003.gguf
    let stem = path.file_stem()?.to_str()?;
    let pos = stem.rfind("-of-")?;
    let total = &stem[pos + 4..];
    let idx = stem[..pos].rsplit('-').next()?;
    Some((idx.parse().ok()?, total.parse().ok()?)).filter(|(a, b)| *a >= 1 && *b >= *a)
}

pub fn read_gguf(path: &Path) -> Result<GgufInfo> {
    let file = File::open(path)?;
    let file_size = file.metadata()?.len();
    let mut r = R { r: BufReader::with_capacity(1 << 16, file) };
    let mut magic = [0u8; 4];
    r.r.read_exact(&mut magic)?;
    if &magic != b"GGUF" {
        return Err(LlmError::BadResponse("不是 GGUF 文件".into()));
    }
    let version = r.u32()?;
    if !(2..=3).contains(&version) {
        return Err(LlmError::BadResponse(format!("不支持的 GGUF 版本 {version}")));
    }
    let _tensors = r.u64()?;
    let kv = r.u64()?;
    if kv > MAX_KV {
        return Err(LlmError::BadResponse("GGUF 元数据条目数异常".into()));
    }
    let mut info = GgufInfo { path: path.to_path_buf(), file_size, version, split: split_of(path), ..Default::default() };
    let mut ints: Vec<(String, u64)> = Vec::new();
    for _ in 0..kv {
        let key = r.string(true)?.unwrap_or_default();
        let t = r.u32()?;
        let keep = matches!(
            key.as_str(),
            "general.architecture" | "general.name" | "general.size_label" | "tokenizer.chat_template"
        );
        let v = r.value(t, keep)?;
        match (key.as_str(), v) {
            ("general.architecture", Val::S(s)) => info.architecture = Some(s),
            ("general.name", Val::S(s)) => info.name = Some(s),
            ("general.size_label", Val::S(s)) => info.size_label = Some(s),
            ("general.file_type", Val::U(n)) => info.quantization = Some(quant_name(n)),
            ("tokenizer.chat_template", Val::S(s)) => {
                info.has_chat_template = true;
                info.chat_template = Some(s.chars().take(MAX_TEMPLATE_CHARS).collect());
            }
            (k, Val::U(n)) => ints.push((k.to_string(), n)),
            (k, Val::I(n)) if n >= 0 => ints.push((k.to_string(), n as u64)),
            _ => {}
        }
    }
    if let Some(arch) = &info.architecture {
        let get = |suffix: &str| ints.iter().find(|(k, _)| *k == format!("{arch}.{suffix}")).map(|(_, v)| *v);
        info.context_length = get("context_length");
        info.block_count = get("block_count");
        info.expert_count = get("expert_count");
        info.embedding_length = get("embedding_length");
        info.head_count = get("attention.head_count");
        info.head_count_kv = get("attention.head_count_kv");
        info.key_length = get("attention.key_length");
        info.value_length = get("attention.value_length");
        info.full_attention_interval = get("full_attention_interval");
        info.is_projector = arch == "clip";
    }
    let arch = info.architecture.as_deref().unwrap_or("").to_lowercase();
    info.role = if info.is_projector {
        "projector"
    } else if arch.contains("tts") || arch.contains("speech") || arch.contains("audio") {
        "tts"
    } else if info.has_chat_template {
        "chat"
    } else {
        "other"
    }
    .into();
    Ok(info)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ScanResult {
    pub models: Vec<GgufInfo>,
    /// 解析失败的文件与原因。
    pub errors: Vec<(PathBuf, String)>,
    /// 发现的服务程序（llama-server 等），只列出位置，不会自动启动。
    pub executables: Vec<PathBuf>,
}

const EXECUTABLES: &[&str] =
    &["llama-server.exe", "llama-server", "ollama.exe", "koboldcpp.exe", "lm studio.exe", "breeze-server.exe"];

/// 扫描文件夹（最多 `max_depth` 层，不跟随符号链接）。只读。
pub fn scan_folder(dir: &Path, max_depth: usize) -> Result<ScanResult> {
    let mut res = ScanResult { models: vec![], errors: vec![], executables: vec![] };
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    let mut visited = 0usize;
    while let Some((d, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            visited += 1;
            if visited > 200_000 {
                return Ok(res);
            }
            let Ok(ft) = e.file_type() else { continue };
            let p = e.path();
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                if depth < max_depth {
                    stack.push((p, depth + 1));
                }
                continue;
            }
            let name = e.file_name().to_string_lossy().to_lowercase();
            if name.ends_with(".gguf") {
                // 分卷文件只解析第一卷。
                if let Some((i, _)) = split_of(&p)
                    && i > 1
                {
                    continue;
                }
                match read_gguf(&p) {
                    Ok(info) => res.models.push(info),
                    Err(err) => res.errors.push((p, err.to_string())),
                }
            } else if EXECUTABLES.contains(&name.as_str()) {
                res.executables.push(p);
            }
        }
    }
    res.models.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(res)
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// 构造一个最小 GGUF 文件（测试夹具）。
    pub fn write_fixture(path: &Path, arch: &str, ctx: u64, ftype: u32, template: Option<&str>) {
        let mut b: Vec<u8> = Vec::new();
        let s = |b: &mut Vec<u8>, x: &str| {
            b.extend((x.len() as u64).to_le_bytes());
            b.extend(x.as_bytes());
        };
        b.extend(b"GGUF");
        b.extend(3u32.to_le_bytes());
        b.extend(0u64.to_le_bytes());
        let n = 6 + template.is_some() as u64;
        b.extend(n.to_le_bytes());
        s(&mut b, "general.architecture");
        b.extend(8u32.to_le_bytes());
        s(&mut b, arch);
        s(&mut b, "general.name");
        b.extend(8u32.to_le_bytes());
        s(&mut b, "测试模型");
        s(&mut b, "general.file_type");
        b.extend(4u32.to_le_bytes());
        b.extend(ftype.to_le_bytes());
        s(&mut b, &format!("{arch}.context_length"));
        b.extend(4u32.to_le_bytes());
        b.extend((ctx as u32).to_le_bytes());
        // 一个字符串数组（模拟分词表），应被跳过。
        s(&mut b, "tokenizer.ggml.tokens");
        b.extend(9u32.to_le_bytes());
        b.extend(8u32.to_le_bytes());
        b.extend(3u64.to_le_bytes());
        for t in ["a", "bb", "ccc"] {
            s(&mut b, t);
        }
        // 一个 f32 数组。
        s(&mut b, "tokenizer.ggml.scores");
        b.extend(9u32.to_le_bytes());
        b.extend(6u32.to_le_bytes());
        b.extend(2u64.to_le_bytes());
        b.extend(1.0f32.to_le_bytes());
        b.extend(2.0f32.to_le_bytes());
        if let Some(t) = template {
            s(&mut b, "tokenizer.chat_template");
            b.extend(8u32.to_le_bytes());
            s(&mut b, t);
        }
        std::fs::write(path, b).unwrap();
    }

    #[test]
    fn parses_fixture() {
        let dir = std::env::temp_dir().join(format!("cgm-gguf-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        write_fixture(&dir.join("a.gguf"), "qwen3moe", 262144, 15, Some("{{ messages }}"));
        write_fixture(&dir.join("sub/mmproj.gguf"), "clip", 0, 1, None);
        write_fixture(&dir.join("sub/big-00002-of-00003.gguf"), "llama", 1, 1, None);
        std::fs::write(dir.join("bad.gguf"), b"GGUF\x01\x00\x00\x00").unwrap();
        std::fs::write(dir.join("sub/llama-server.exe"), b"").unwrap();

        let info = read_gguf(&dir.join("a.gguf")).unwrap();
        assert_eq!(info.architecture.as_deref(), Some("qwen3moe"));
        assert_eq!(info.quantization.as_deref(), Some("Q4_K_M"));
        assert_eq!(info.context_length, Some(262144));
        assert!(info.has_chat_template);
        assert_eq!(info.name.as_deref(), Some("测试模型"));
        assert_eq!(info.role, "chat");

        let scan = scan_folder(&dir, 3).unwrap();
        assert_eq!(scan.models.len(), 2, "分卷第 2 卷被跳过");
        assert!(scan.models.iter().any(|m| m.is_projector));
        assert_eq!(scan.errors.len(), 1);
        assert_eq!(scan.executables.len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn split_names() {
        assert_eq!(split_of(Path::new("Qwen-Q8_0-00001-of-00002.gguf")), Some((1, 2)));
        assert_eq!(split_of(Path::new("model.gguf")), None);
    }
}
