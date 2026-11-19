//! 提示词模板：内置一套，用户可以在覆盖目录中放同名文件替换。

use std::path::Path;

use std::collections::BTreeMap;

use minijinja::Environment;
use serde::{Deserialize, Serialize};

use crate::{GenError, Result};

const BUILTIN: &[(&str, &str)] = &[
    ("system", include_str!("../prompts/system.j2")),
    ("outline", include_str!("../prompts/outline.j2")),
    ("expand_chapter", include_str!("../prompts/expand_chapter.j2")),
    ("npcs", include_str!("../prompts/npcs.j2")),
    ("regenerate", include_str!("../prompts/regenerate.j2")),
    ("extend", include_str!("../prompts/extend.j2")),
    ("repair", include_str!("../prompts/repair.j2")),
    ("world_seed", include_str!("../prompts/world_seed.j2")),
    ("expand_node", include_str!("../prompts/expand_node.j2")),
    ("rewrite_node", include_str!("../prompts/rewrite_node.j2")),
    ("plot_from_script", include_str!("../prompts/plot_from_script.j2")),
    ("plot_branches", include_str!("../prompts/plot_branches.j2")),
    ("gm_plan", include_str!("../prompts/gm_plan.j2")),
    ("gm_narrate", include_str!("../prompts/gm_narrate.j2")),
    ("rpg_intent", include_str!("../prompts/rpg_intent.j2")),
    ("director", include_str!("../prompts/director.j2")),
    ("script_review", include_str!("../prompts/script_review.j2")),
    ("pc_generate", include_str!("../prompts/pc_generate.j2")),
    ("pc_kit", include_str!("../prompts/pc_kit.j2")),
    ("combat_narrate", include_str!("../prompts/combat_narrate.j2")),
    ("quest_generate", include_str!("../prompts/quest_generate.j2")),
    ("npc_day", include_str!("../prompts/npc_day.j2")),
    ("npc_reply", include_str!("../prompts/npc_reply.j2")),
    ("session_summary", include_str!("../prompts/session_summary.j2")),
];

/// 内置提示词的版本号：修改任何内置提示词（prompts/*.j2）时加 1，并更新 [`BUILTIN_DIGEST`]。
/// 以前写死成 v1，提示词改了很多轮界面上还是 v1。
/// v1：初版；v2：36 项需求、GM 教程与开场白（0.0.2–0.0.5）；v3：试玩反馈（初始装备、技能贴合、NPC 台词变短、节奏推进；真实试玩后：不重复描写、活着不能写死）；
/// v4：全面改革（新增战斗描述 combat_narrate；战斗里的闪避、技能、场景交互）。
pub const PROMPT_VERSION: u32 = 4;

/// 内置提示词内容的摘要。测试会检查它和实际内容一致：改了提示词却没升版本号时测试失败。
pub const BUILTIN_DIGEST: &str = "af67d086";

/// 一组模板源码的摘要（FNV-1a，取低 32 位）。
fn digest(all: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in all.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:08x}", h as u32)
}

/// 内置模板（不含用户覆盖）的摘要。
pub fn builtin_digest() -> String {
    digest(&BUILTIN.iter().map(|(_, s)| *s).collect::<String>())
}

/// 在线更新的提示词包（不用更新软件就能换提示词）。只包含模板文本，模板引擎是沙箱，不会执行程序。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PromptPack {
    /// 提示词版本号：比软件自带的 [`PROMPT_VERSION`] 大才会使用。
    pub version: u32,
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub notes: String,
    /// 模板名 → 模板源码（只需要包含改过的模板，没包含的用软件自带的）。
    pub templates: BTreeMap<String, String>,
    /// 校验值：所有模板按名字排序后 `名字\n源码\n` 拼起来的 SHA-256 前 16 位（防止下载损坏）。
    #[serde(default)]
    pub checksum: String,
}

impl PromptPack {
    pub fn compute_checksum(&self) -> String {
        use sha2::Digest;
        let mut h = sha2::Sha256::new();
        for (k, v) in &self.templates {
            h.update(k.as_bytes());
            h.update(b"\n");
            h.update(v.as_bytes());
            h.update(b"\n");
        }
        h.finalize().iter().take(8).map(|b| format!("{b:02x}")).collect()
    }

    /// 检查提示词包能不能用：版本号、校验值、模板名都认识、每个模板都能编译。
    pub fn validate(&self) -> Result<()> {
        if self.version == 0 || self.templates.is_empty() {
            return Err(GenError::Template("提示词包是空的".into()));
        }
        if self.checksum != self.compute_checksum() {
            return Err(GenError::Template("提示词包校验值不对（下载可能不完整）".into()));
        }
        for (name, src) in &self.templates {
            if !BUILTIN.iter().any(|(n, _)| n == name) {
                return Err(GenError::Template(format!("提示词包里有不认识的模板：{name}（可能需要先更新软件）")));
            }
            if src.len() > 200_000 {
                return Err(GenError::Template(format!("模板 {name} 太大")));
            }
        }
        Templates::load_with(Some(self), None).map(|_| ())
    }
}

pub struct Templates {
    env: Environment<'static>,
    /// 模板版本：内置版本号 + 实际使用的模板内容摘要（用户覆盖后会变化）。
    pub version: String,
    pub overridden: Vec<String>,
    /// 正在使用的在线提示词包版本（没有为 None）。
    pub pack_version: Option<u32>,
}

impl Templates {
    pub fn builtin() -> Self {
        Self::load(None).expect("内置模板必须有效")
    }

    /// 加载模板；`override_dir` 中存在 `<name>.j2` 时使用用户版本。
    pub fn load(override_dir: Option<&Path>) -> Result<Self> {
        Self::load_with(None, override_dir)
    }

    /// 三层：软件自带 → 在线提示词包（版本比自带的新才用）→ 用户自己的覆盖文件。
    pub fn load_with(pack: Option<&PromptPack>, override_dir: Option<&Path>) -> Result<Self> {
        let pack = pack.filter(|p| p.version > PROMPT_VERSION);
        let mut env = Environment::new();
        let mut overridden = vec![];
        let mut all = String::new();
        for (name, src) in BUILTIN {
            let mut source = pack.and_then(|p| p.templates.get(*name)).cloned().unwrap_or_else(|| src.to_string());
            if let Some(dir) = override_dir {
                let p = dir.join(format!("{name}.j2"));
                if p.is_file() {
                    source = std::fs::read_to_string(&p)
                        .map_err(|e| GenError::Template(format!("读取模板 {} 失败: {e}", p.display())))?;
                    overridden.push(name.to_string());
                }
            }
            all.push_str(&source);
            let leaked: &'static str = Box::leak(source.into_boxed_str());
            env.add_template(name, leaked).map_err(|e| GenError::Template(format!("模板 {name} 有误: {e}")))?;
        }
        // 界面显示「v3-1a2b3c4d」：前面是提示词版本号（在线更新后是提示词包的版本），后面是实际使用的模板内容摘要（自定义模板后会变）。
        let v = pack.map(|p| p.version).unwrap_or(PROMPT_VERSION);
        Ok(Self { env, version: format!("v{v}-{}", digest(&all)), overridden, pack_version: pack.map(|p| p.version) })
    }

    pub fn render<S: Serialize>(&self, name: &str, ctx: S) -> Result<String> {
        let t = self.env.get_template(name).map_err(|e| GenError::Template(e.to_string()))?;
        t.render(minijinja::value::Serde(ctx)).map_err(|e| GenError::Template(format!("渲染模板 {name} 失败: {e}")))
    }
}

/// 软件自带的全部模板（生成提示词包时用）。
pub fn builtin_sources() -> Vec<(&'static str, &'static str)> {
    BUILTIN.to_vec()
}

#[cfg(test)]
mod version_tests {
    use super::*;

    fn pack(version: u32, name: &str, src: &str) -> PromptPack {
        let mut p = PromptPack { version, date: "2026-11-19".into(), notes: String::new(), templates: BTreeMap::from([(name.to_string(), src.to_string())]), checksum: String::new() };
        p.checksum = p.compute_checksum();
        p
    }

    #[test]
    fn newer_prompt_pack_overrides_builtin_and_user_files_still_win() {
        let p = pack(PROMPT_VERSION + 1, "session_summary", "新的摘要模板 {{ 1 + 1 }}");
        p.validate().unwrap();
        let t = Templates::load_with(Some(&p), None).unwrap();
        assert_eq!(t.pack_version, Some(PROMPT_VERSION + 1));
        assert!(t.version.starts_with(&format!("v{}-", PROMPT_VERSION + 1)));
        assert_eq!(t.render("session_summary", serde_json::json!({})).unwrap(), "新的摘要模板 2");
        // 没包含的模板照常用自带的。
        let base = Templates::builtin();
        assert_eq!(t.env.get_template("npc_day").unwrap().source(), base.env.get_template("npc_day").unwrap().source());
        // 用户自己的覆盖文件优先于在线包。
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("session_summary.j2"), "我自己的").unwrap();
        let t = Templates::load_with(Some(&p), Some(dir.path())).unwrap();
        assert_eq!(t.render("session_summary", serde_json::json!({})).unwrap(), "我自己的");
        // 版本不比自带的新：不用（软件更新后自带的更新）。
        let old = pack(PROMPT_VERSION, "session_summary", "旧的");
        assert_eq!(Templates::load_with(Some(&old), None).unwrap().pack_version, None);
    }

    #[test]
    fn bad_prompt_packs_are_rejected() {
        let mut p = pack(PROMPT_VERSION + 1, "session_summary", "x");
        p.templates.insert("session_summary".into(), "被改过".into());
        assert!(p.validate().unwrap_err().to_string().contains("校验值"));
        assert!(pack(PROMPT_VERSION + 1, "not_a_template", "x").validate().unwrap_err().to_string().contains("不认识"));
        assert!(pack(PROMPT_VERSION + 1, "session_summary", "{% if %}").validate().is_err(), "编译不过的模板不能用");
    }

    /// 改了内置提示词就要升版本：把 PROMPT_VERSION 加 1，并把 BUILTIN_DIGEST 改成下面报出的新摘要。
    #[test]
    fn prompt_version_is_bumped_when_builtin_prompts_change() {
        assert_eq!(
            builtin_digest(),
            BUILTIN_DIGEST,
            "内置提示词变了：请把 templates.rs 里的 PROMPT_VERSION 加 1（现在是 {PROMPT_VERSION}），并把 BUILTIN_DIGEST 改成 \"{}\"",
            builtin_digest()
        );
        assert!(Templates::builtin().version.starts_with(&format!("v{PROMPT_VERSION}-")));
    }
}
