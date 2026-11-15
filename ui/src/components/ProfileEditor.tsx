// 剧本的玩法配置：玩法比重、属性方案、技能对应、检定规则、叙事风格。
// 由 AI 随剧本大纲生成；旧剧本按题材推断。改动保存到剧本，之后新开的战役按新配置设置规则（已有战役不受影响）。
import { useEffect, useState } from "react";
import { api } from "../api";
import type { Ctx } from "../App";
import type { AttrDef } from "../bindings/AttrDef";
import type { GameProfile } from "../bindings/GameProfile";
import type { Weights } from "../bindings/Weights";

const WEIGHTS: [keyof Weights, string, string][] = [
  ["combat", "战斗", "冲突、战斗与对抗"],
  ["exploration", "探索", "新地点、旅行、奇观"],
  ["social", "社交", "交流、关系、谈判"],
  ["investigation", "调查", "线索、推理、真相；只有悬疑类剧本才适合高比重"],
  ["survival", "生存", "资源、伤势、环境压力"],
  ["management", "经营", "建设、经营、资源管理"],
];
const ROLES: [string, string][] = [
  ["", "（无）"],
  ["might", "力量类"],
  ["agility", "敏捷类"],
  ["vitality", "体质类（生命成长）"],
  ["mind", "智力类"],
  ["insight", "感知类"],
  ["presence", "魅力类"],
];
const SOURCE: Record<string, string> = { ai: "AI 随大纲生成", inferred: "按题材推断（还没有保存过）", user: "手动修改" };
const LEVEL = ["无", "少", "中", "核心"];

export default function ProfileEditor({ ctx, scriptId, rev, onSaved }: { ctx: Ctx; scriptId: string; rev: number; onSaved: () => void }) {
  const [p, setP] = useState<GameProfile | null>(null);
  const [dirty, setDirty] = useState(false);
  useEffect(() => {
    api.scriptGameProfile(ctx.worldId, scriptId).then((x) => {
      setP(x);
      setDirty(false);
    }).catch((e: Error) => ctx.notify(e.message, "error"));
  }, [ctx.worldId, scriptId, rev, ctx]);
  if (!p) return null;
  const up = (f: (x: GameProfile) => GameProfile) => {
    setP((x) => (x ? f(x) : x));
    setDirty(true);
  };
  const attrs = p.attributes;
  const setAttr = (i: number, a: Partial<AttrDef>) => up((x) => ({ ...x, attributes: x.attributes.map((v, j) => (j === i ? { ...v, ...a } : v)) }));
  const skills = Object.entries(p.skills ?? {});
  const attrKeys = attrs.length ? attrs.map((a) => a.key) : ["str", "dex", "con", "int", "wis", "cha"];

  async function save() {
    if (!p) return;
    try {
      await api.updateScript(ctx.worldId, scriptId, { game_profile: { ...p, source: "user" } }, rev);
      ctx.notify("玩法配置已保存（之后新开的战役按新配置设置规则）");
      setDirty(false);
      onSaved();
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }

  return (
    <div className="card profile-editor">
      <div className="row">
        <h3 style={{ margin: 0 }}>玩法配置</h3>
        <span className="hint">{SOURCE[p.source] ?? p.source}</span>
        <span className="spacer" />
        <button className="primary" disabled={!dirty} onClick={() => void save()}>
          保存
        </button>
      </div>
      <p className="hint">决定这个剧本以什么玩法为主：生成章节时写多少线索、GM 怎样推进、游戏页显示哪些面板；属性与检定规则会在新开战役时复制进战役规则。</p>

      <h4>玩法比重</h4>
      <div className="weights">
        {WEIGHTS.map(([k, label, desc]) => (
          <div key={k} className="weight-row" title={desc}>
            <span className="weight-name">{label}</span>
            <div className="weight-seg" role="radiogroup" aria-label={label}>
              {[0, 1, 2, 3].map((v) => (
                <button key={v} className={p.weights[k] === v ? "active" : ""} onClick={() => up((x) => ({ ...x, weights: { ...x.weights, [k]: v } }))}>
                  {LEVEL[v]}
                </button>
              ))}
            </div>
            <small className="hint">{desc}</small>
          </div>
        ))}
      </div>

      <h4>属性方案</h4>
      {attrs.length === 0 && <p className="hint">使用默认的六项属性（力量、敏捷、体质、智力、感知、魅力）。</p>}
      {attrs.map((a, i) => (
        <div key={i} className="row attr-row">
          <input value={a.name} placeholder="名称" onChange={(e) => setAttr(i, { name: e.target.value })} style={{ width: 100 }} />
          <input value={a.key} placeholder="key（英文）" onChange={(e) => setAttr(i, { key: e.target.value.replace(/[^a-z0-9_]/gi, "").toLowerCase() })} style={{ width: 110 }} />
          <select value={a.role} onChange={(e) => setAttr(i, { role: e.target.value })}>
            {ROLES.map(([v, t]) => (
              <option key={v} value={v}>
                {t}
              </option>
            ))}
          </select>
          <input value={a.desc} placeholder="说明" onChange={(e) => setAttr(i, { desc: e.target.value })} style={{ flex: 1 }} />
          <button className="small danger" onClick={() => up((x) => ({ ...x, attributes: x.attributes.filter((_, j) => j !== i) }))}>
            删除
          </button>
        </div>
      ))}
      <div className="row">
        <button className="small" disabled={attrs.length >= 12} onClick={() => up((x) => ({ ...x, attributes: [...x.attributes, { key: `attr${x.attributes.length + 1}`, name: "", desc: "", role: "" }] }))}>
          + 添加属性
        </button>
        {attrs.length > 0 && (
          <button className="small link" onClick={() => up((x) => ({ ...x, attributes: [], skills: {} }))}>
            改回默认六项
          </button>
        )}
      </div>

      <h4>技能对应的属性</h4>
      {skills.map(([sk, at]) => (
        <div key={sk} className="row attr-row">
          <span style={{ width: 110 }}>{sk}</span>
          <select value={at} onChange={(e) => up((x) => ({ ...x, skills: { ...x.skills, [sk]: e.target.value } }))}>
            {attrKeys.map((k) => (
              <option key={k} value={k}>
                {attrs.find((a) => a.key === k)?.name || k}
              </option>
            ))}
          </select>
          <button className="small link" onClick={() => up((x) => { const n = { ...x.skills }; delete n[sk]; return { ...x, skills: n }; })}>
            移除
          </button>
        </div>
      ))}
      <SkillAdder keys={attrKeys} onAdd={(sk, at) => up((x) => ({ ...x, skills: { ...x.skills, [sk]: at } }))} />

      <h4>检定规则</h4>
      <div className="grid2 check-grid">
        <label>
          检定方式
          <select value={p.check.mode} onChange={(e) => up((x) => ({ ...x, check: { ...x.check, mode: e.target.value, die: e.target.value === "roll_under" ? "1d100" : x.check.die === "1d100" ? "1d20" : x.check.die } }))}>
            <option value="target">掷骰 + 修正 ≥ 难度（d20 类）</option>
            <option value="roll_under">百分骰 ≤ 目标值（d100 类）</option>
            <option value="pool">骰池：数成功的骰子</option>
          </select>
        </label>
        {p.check.mode !== "pool" ? (
          <label>
            主骰
            <input value={p.check.die} onChange={(e) => up((x) => ({ ...x, check: { ...x.check, die: e.target.value.trim() } }))} placeholder="1d20 / 2d6 / 1d100" />
          </label>
        ) : (
          <label>
            骰池：基础个数 / 面数 / 几点成功
            <span className="row">
              <input type="number" min={1} max={20} value={p.check.pool_base} onChange={(e) => up((x) => ({ ...x, check: { ...x.check, pool_base: Number(e.target.value) } }))} style={{ width: 70 }} />
              <input type="number" min={2} max={100} value={p.check.pool_sides} onChange={(e) => up((x) => ({ ...x, check: { ...x.check, pool_sides: Number(e.target.value) } }))} style={{ width: 70 }} />
              <input type="number" min={1} max={p.check.pool_sides} value={p.check.pool_success_on} onChange={(e) => up((x) => ({ ...x, check: { ...x.check, pool_success_on: Number(e.target.value) } }))} style={{ width: 70 }} />
            </span>
          </label>
        )}
        <label>
          部分成功范围（差几点以内，0 = 不用）
          <input type="number" min={0} max={20} value={p.check.partial_band} onChange={(e) => up((x) => ({ ...x, check: { ...x.check, partial_band: Number(e.target.value) } }))} />
        </label>
        <label className="check">
          <input type="checkbox" checked={p.check.crits} onChange={(e) => up((x) => ({ ...x, check: { ...x.check, crits: e.target.checked } }))} />
          大成功 / 大失败
        </label>
        <label className="check">
          <input type="checkbox" checked={p.check.advantage} onChange={(e) => up((x) => ({ ...x, check: { ...x.check, advantage: e.target.checked } }))} />
          优势 / 劣势（多掷一次取高 / 取低）
        </label>
        <label>
          叙事风格
          <input value={p.style.tone} onChange={(e) => up((x) => ({ ...x, style: { ...x.style, tone: e.target.value } }))} placeholder="例如：冷硬悬疑、轻松冒险" />
        </label>
        <label className="check">
          详细程度
          <select value={p.style.detail} onChange={(e) => up((x) => ({ ...x, style: { ...x.style, detail: Number(e.target.value) } }))}>
            <option value={0}>默认</option>
            <option value={1}>简洁</option>
            <option value={2}>适中</option>
            <option value={3}>细致</option>
          </select>
        </label>
        <div className="row wrap style-ratios">
          {(
            [
              ["dialogue", "对话"],
              ["action", "动作"],
              ["inner", "心理"],
              ["scenery", "环境"],
            ] as const
          ).map(([k, l]) => (
            <label key={k} className="check">
              {l}
              <select value={p.style[k]} onChange={(e) => up((x) => ({ ...x, style: { ...x.style, [k]: Number(e.target.value) } }))}>
                <option value={0}>默认</option>
                <option value={1}>少</option>
                <option value={2}>适中</option>
                <option value={3}>多</option>
              </select>
            </label>
          ))}
        </div>
      </div>
      <h4>货币与稀有度</h4>
      <div className="form">
        <label title="从大到小，用逗号分开；进位写每种等于多少个最小单位（最后一个是 1）">
          货币
          <input
            value={p.currency.map((c, i) => `${c}=${p.currency_rates[i] ?? 1}`).join("，")}
            placeholder="例如：金币=100，银币=10，铜币=1；留空用默认"
            onChange={(e) => {
              const parts = e.target.value.split(/[,，、]/).map((x) => x.trim()).filter(Boolean);
              const pairs = parts.map((x) => {
                const [n, r] = x.split(/[=＝]/);
                return [n.trim(), Math.max(1, Number(r) || 1)] as const;
              });
              up((x) => ({ ...x, currency: pairs.map((q) => q[0]), currency_rates: pairs.map((q) => q[1]) }));
            }}
          />
        </label>
        <label title="五档，从普通到传说，用逗号分开">
          稀有度叫法
          <input
            value={p.rarity_labels.join("，")}
            placeholder="例如：寻常，上品，珍品，宝物，神兵；留空用默认"
            onChange={(e) => up((x) => ({ ...x, rarity_labels: e.target.value.split(/[,，、]/).map((v) => v.trim()).filter(Boolean) }))}
          />
        </label>
        <p className="hint">新建战役时会带进战役规则；已经开始的战役可以在「角色」页的规则里改。</p>
      </div>
      <h4>世界树节点的资料字段（按题材）</h4>
      <div className="form">
        <label title="每行一类：节点类型: 字段1、字段2（例如 character: sect：所属门派、martial_arts：会的武功）。生成和改写世界树节点时，AI 会按这些字段写资料">
          <textarea
            rows={4}
            value={Object.entries(p.node_fields).map(([k, v]) => `${k}: ${v.join("、")}`).join("\n")}
            placeholder="character: sect：所属门派、martial_arts：会的武功"
            onChange={(e) => {
              const m: Record<string, string[]> = {};
              for (const line of e.target.value.split("\n")) {
                const i = line.indexOf(":");
                if (i <= 0) continue;
                const k = line.slice(0, i).trim();
                const v = line.slice(i + 1).split("、").map((x) => x.trim()).filter(Boolean);
                if (k && v.length) m[k] = v;
              }
              up((x) => ({ ...x, node_fields: m }));
            }}
          />
        </label>
      </div>
    </div>
  );
}

function SkillAdder({ keys, onAdd }: { keys: string[]; onAdd: (skill: string, attr: string) => void }) {
  const [sk, setSk] = useState("");
  const [at, setAt] = useState(keys[0] ?? "");
  useEffect(() => setAt(keys[0] ?? ""), [keys]);
  return (
    <div className="row attr-row">
      <input value={sk} onChange={(e) => setSk(e.target.value)} placeholder="技能名" style={{ width: 110 }} />
      <select value={at} onChange={(e) => setAt(e.target.value)}>
        {keys.map((k) => (
          <option key={k} value={k}>
            {k}
          </option>
        ))}
      </select>
      <button className="small" disabled={!sk.trim()} onClick={() => { onAdd(sk.trim(), at); setSk(""); }}>
        + 添加技能
      </button>
    </div>
  );
}
