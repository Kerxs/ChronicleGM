//! 提示词模板：内置一套，用户可以在覆盖目录中放同名文件替换。

use std::path::Path;

use minijinja::Environment;
use serde::Serialize;

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
    ("quest_generate", include_str!("../prompts/quest_generate.j2")),
    ("npc_day", include_str!("../prompts/npc_day.j2")),
    ("npc_reply", include_str!("../prompts/npc_reply.j2")),
    ("session_summary", include_str!("../prompts/session_summary.j2")),
];

pub struct Templates {
    env: Environment<'static>,
    /// 模板版本：内置版本号 + 实际使用的模板内容摘要（用户覆盖后会变化）。
    pub version: String,
    pub overridden: Vec<String>,
}

impl Templates {
    pub fn builtin() -> Self {
        Self::load(None).expect("内置模板必须有效")
    }

    /// 加载模板；`override_dir` 中存在 `<name>.j2` 时使用用户版本。
    pub fn load(override_dir: Option<&Path>) -> Result<Self> {
        let mut env = Environment::new();
        let mut overridden = vec![];
        let mut all = String::new();
        for (name, src) in BUILTIN {
            let mut source = src.to_string();
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
        let mut h: u64 = 0xcbf29ce484222325;
        for b in all.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        Ok(Self { env, version: format!("script-v1-{:08x}", h as u32), overridden })
    }

    pub fn render<S: Serialize>(&self, name: &str, ctx: S) -> Result<String> {
        let t = self.env.get_template(name).map_err(|e| GenError::Template(e.to_string()))?;
        t.render(minijinja::value::Serde(ctx)).map_err(|e| GenError::Template(format!("渲染模板 {name} 失败: {e}")))
    }
}
