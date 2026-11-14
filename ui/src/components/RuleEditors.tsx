// 剧本任务的规则编辑（前置 / 完成 / 失败条件、期限、关联）、触发器规则、人物日程与势力实力编辑。
// 都直接修改附加资料（payload）的 JSON 文本，与"附加资料（JSON）"保持同步；校验在后端完成。
import { useEffect, useState } from "react";
import { api } from "../api";
import type { Condition } from "../bindings/Condition";
import type { ScheduleEntry } from "../bindings/ScheduleEntry";
import type { Node } from "../bindings/Node";
import { ConditionEditor, EffectsEditor, type Choices } from "./DslEditors";
import type { Effect } from "../bindings/Effect";
import { defaultCondition } from "../lib/dsl";

type Obj = Record<string, unknown>;
const parse = (t: string): Obj => {
  try {
    const v = JSON.parse(t || "{}");
    return v && typeof v === "object" ? (v as Obj) : {};
  } catch {
    return {};
  }
};

/** 地点选择：按名称搜索世界中的区域、国家、城市、地点。 */
export function PlacePicker({ worldId, value, onChange, disabled }: { worldId: string; value: string; onChange: (id: string) => void; disabled?: boolean }) {
  const [q, setQ] = useState("");
  const [places, setPlaces] = useState<Node[]>([]);
  const [title, setTitle] = useState("");
  useEffect(() => {
    api.findPlaces(worldId, q).then(setPlaces).catch(() => setPlaces([]));
  }, [worldId, q]);
  useEffect(() => {
    if (!value) return setTitle("");
    api.getNode(worldId, value).then((n) => setTitle(n?.title ?? value)).catch(() => setTitle(value));
  }, [worldId, value]);
  return (
    <span className="row">
      <input value={q} disabled={disabled} onChange={(e) => setQ(e.target.value)} placeholder={title || "搜索地点"} style={{ width: 110 }} />
      <select value={value} disabled={disabled} onChange={(e) => onChange(e.target.value)} style={{ maxWidth: 160 }}>
        <option value="">（不指定）</option>
        {value && !places.some((p) => p.id === value) && <option value={value}>{title}</option>}
        {places.map((p) => (
          <option key={p.id} value={p.id}>{p.title}</option>
        ))}
      </select>
    </span>
  );
}

function OptionalCondition({ label, value, onChange, choices, disabled }: { label: string; value: unknown; onChange: (c: Condition | null) => void; choices: Choices; disabled?: boolean }) {
  const on = !!value;
  return (
    <div className="rule">
      <label className="check">
        <input type="checkbox" checked={on} disabled={disabled} onChange={(e) => onChange(e.target.checked ? defaultCondition("clue_known") : null)} />
        {label}
      </label>
      {on && <ConditionEditor value={value as Condition} onChange={onChange} choices={choices} />}
    </div>
  );
}

/** 任务规则：前置 / 完成 / 失败条件（程序在每回合结束时求值）、期限、相关人物 / 地点 / 线索。 */
export function QuestRulesEditor({ worldId, payload, setPayload, choices, disabled }: { worldId: string; payload: string; setPayload: (s: string) => void; choices: Choices; disabled?: boolean }) {
  const p = parse(payload);
  const set = (k: string, v: unknown) => {
    const next = { ...parse(payload) };
    if (v === null || v === undefined || (Array.isArray(v) && v.length === 0)) delete next[k];
    else next[k] = v;
    setPayload(JSON.stringify(next, null, 2));
  };
  const ids = (k: string) => (Array.isArray(p[k]) ? (p[k] as string[]) : []);
  const toggle = (k: string, id: string, on: boolean) => set(k, on ? [...ids(k), id] : ids(k).filter((x) => x !== id));
  return (
    <fieldset className="rules" disabled={disabled}>
      <legend>任务规则（由程序在每回合结束时判定）</legend>
      <OptionalCondition label="满足条件时自动开始" value={p.precondition} onChange={(c) => set("precondition", c)} choices={choices} />
      <OptionalCondition label="满足条件时完成" value={p.complete_when} onChange={(c) => set("complete_when", c)} choices={choices} />
      <OptionalCondition label="满足条件时失败" value={p.fail_when} onChange={(c) => set("fail_when", c)} choices={choices} />
      <label className="check">
        期限：开始后
        <input type="number" min={0} value={(p.deadline_turns as number) ?? 0} onChange={(e) => set("deadline_turns", Number(e.target.value) > 0 ? Number(e.target.value) : null)} style={{ width: 60 }} />
        回合内完成（0 = 没有期限）
      </label>
      <label className="check">
        或者：开始后故事时间
        <input type="number" min={0} step={0.5} value={(p.deadline_hours as number) ?? 0} onChange={(e) => set("deadline_hours", Number(e.target.value) > 0 ? Number(e.target.value) : null)} style={{ width: 70 }} />
        小时内完成（旅行、等待、制作都会消耗时间）
      </label>
      <div className="row wrap">
        <span className="hint">相关人物：</span>
        {choices.people.map((c) => (
          <label key={c.id} className="check">
            <input type="checkbox" checked={ids("npc_ids").includes(c.id)} onChange={(e) => toggle("npc_ids", c.id, e.target.checked)} />
            {c.label}
          </label>
        ))}
      </div>
      <div className="row wrap">
        <span className="hint">相关线索：</span>
        {choices.clues.map((c) => (
          <label key={c.id} className="check">
            <input type="checkbox" checked={ids("clue_ids").includes(c.id)} onChange={(e) => toggle("clue_ids", c.id, e.target.checked)} />
            {c.label}
          </label>
        ))}
      </div>
      <div className="row">
        <span className="hint">地点：</span>
        <PlacePicker worldId={worldId} value={(p.location as string) ?? ""} onChange={(v) => set("location", v || null)} disabled={disabled} />
      </div>
    </fieldset>
  );
}

/** 人物日程：每段时间在哪里、做什么（程序按世界时间计算 NPC 的位置与活动，决定谁在场）。 */
export function ScheduleEditor({ worldId, payload, setPayload, disabled }: { worldId: string; payload: string; setPayload: (s: string) => void; disabled?: boolean }) {
  const p = parse(payload);
  const rows: ScheduleEntry[] = Array.isArray(p.schedule) ? (p.schedule as ScheduleEntry[]) : [];
  const save = (next: ScheduleEntry[]) => {
    const o = { ...parse(payload) };
    if (next.length) o.schedule = next;
    else delete o.schedule;
    setPayload(JSON.stringify(o, null, 2));
  };
  const upd = (i: number, patch: Partial<ScheduleEntry>) => save(rows.map((r, j) => (j === i ? { ...r, ...patch } : r)));
  return (
    <fieldset className="rules" disabled={disabled}>
      <legend>日程（结束早于开始表示跨过午夜）</legend>
      {rows.map((r, i) => (
        <div key={i} className="row wrap">
          <input value={r.start} onChange={(e) => upd(i, { start: e.target.value })} placeholder="08:00" style={{ width: 60 }} />—
          <input value={r.end} onChange={(e) => upd(i, { end: e.target.value })} placeholder="18:00" style={{ width: 60 }} />
          <input value={r.activity} onChange={(e) => upd(i, { activity: e.target.value })} placeholder="在铁匠铺打铁" style={{ width: 140 }} />
          <PlacePicker worldId={worldId} value={r.location} onChange={(v) => upd(i, { location: v })} disabled={disabled} />
          <label className="check" title="暴雨、暴雪时改为待在家里">
            <input type="checkbox" checked={!!r.outdoor} onChange={(e) => upd(i, { outdoor: e.target.checked })} />
            户外
          </label>
          <button type="button" className="link danger" onClick={() => save(rows.filter((_, j) => j !== i))}>×</button>
        </div>
      ))}
      <button type="button" onClick={() => save([...rows, { start: "08:00", end: "18:00", activity: "", location: "", outdoor: false }])}>添加时段</button>
      <div className="row">
        <span className="hint">家（恶劣天气时户外活动改为待在这里）：</span>
        <PlacePicker
          worldId={worldId}
          value={(p.home as string) ?? ""}
          onChange={(v) => {
            const o = { ...parse(payload) };
            if (v) o.home = v;
            else delete o.home;
            setPayload(JSON.stringify(o, null, 2));
          }}
          disabled={disabled}
        />
      </div>
    </fieldset>
  );
}

const POWER_KEYS: [string, string][] = [
  ["wealth", "财富"],
  ["military", "军力"],
  ["influence", "影响"],
  ["stability", "稳定"],
];

/** 势力的初始实力（世界模拟按天推演的起点；战役中的变化不改动世界树）。 */
export function FactionPowerEditor({ payload, setPayload, disabled }: { payload: string; setPayload: (s: string) => void; disabled?: boolean }) {
  const p = parse(payload);
  const power = (p.power && typeof p.power === "object" ? p.power : {}) as Record<string, number>;
  const set = (k: string, v: number) => {
    const o = { ...parse(payload) };
    o.power = { ...power, [k]: Math.max(0, Math.min(100, Math.round(v))) };
    setPayload(JSON.stringify(o, null, 2));
  };
  return (
    <fieldset className="rules" disabled={disabled}>
      <legend>势力实力（0–100，世界模拟的初始值；没填的按 50）</legend>
      <div className="row wrap">
        {POWER_KEYS.map(([k, l]) => (
          <label key={k} className="check">
            {l}
            <input type="number" min={0} max={100} value={power[k] ?? 50} onChange={(e) => set(k, Number(e.target.value))} style={{ width: 60 }} />
          </label>
        ))}
      </div>
      <p className="hint">目标里写"扩张""征服"的势力军力强时可能宣战；写"军""防御"的会征兵。势力之间的战争、同盟、贸易用世界树的关系设定。</p>
    </fieldset>
  );
}

/** 触发器 / 关键事件：时间推进时条件满足就自动触发（默认只触发一次），执行效果并记入世界动态。 */
export function TriggerRulesEditor({ payload, setPayload, choices, disabled }: { payload: string; setPayload: (s: string) => void; choices: Choices; disabled?: boolean }) {
  const p = parse(payload);
  const set = (k: string, v: unknown) => {
    const next = { ...parse(payload) };
    if (v === null || v === undefined || (Array.isArray(v) && v.length === 0) || v === "") delete next[k];
    else next[k] = v;
    setPayload(JSON.stringify(next, null, 2));
  };
  return (
    <fieldset className="rules" disabled={disabled}>
      <legend>自动触发（时间推进时由程序检查；条件可以引用 env.daypart、env.weather、env.season、env.day）</legend>
      <OptionalCondition label="满足条件时触发" value={p.when} onChange={(c) => set("when", c)} choices={choices} />
      {!!p.when && (
        <>
          <div className="rule">
            <span className="hint">触发时执行：</span>
            <EffectsEditor value={(Array.isArray(p.effects) ? p.effects : []) as Effect[]} onChange={(e) => set("effects", e)} choices={choices} />
          </div>
          <label className="check">
            世界动态里显示的文字：
            <input value={(p.text as string) ?? ""} onChange={(e) => set("text", e.target.value)} placeholder="（默认用标题）" style={{ width: 240 }} />
          </label>
          <label className="check">
            <input type="checkbox" checked={p.once !== false} onChange={(e) => set("once", e.target.checked ? null : false)} />
            只触发一次
          </label>
        </>
      )}
    </fieldset>
  );
}

const RARITY_OPTS: [string, string][] = [
  ["common", "普通"],
  ["uncommon", "优秀"],
  ["rare", "稀有"],
  ["epic", "史诗"],
  ["legendary", "传说"],
];
const SLOT_OPTS: [string, string][] = [
  ["", "不能装备"],
  ["weapon", "武器"],
  ["armor", "护甲"],
  ["shield", "盾牌"],
  ["accessory", "饰品"],
];

function usePayload(payload: string, setPayload: (s: string) => void) {
  const p = parse(payload);
  const set = (k: string, v: unknown) => {
    const next = { ...parse(payload) };
    if (v === null || v === undefined || v === "" || (typeof v === "number" && Number.isNaN(v))) delete next[k];
    else next[k] = v;
    setPayload(JSON.stringify(next, null, 2));
  };
  return { p, set };
}

/** 物品的游戏数值：价值、稀有度、装备栏位、伤害、射程、护甲、治疗、制作配方（由程序结算）。 */
export function ItemStatsEditor({ payload, setPayload, disabled }: { payload: string; setPayload: (s: string) => void; disabled?: boolean }) {
  const { p, set } = usePayload(payload, setPayload);
  const num = (k: string) => (typeof p[k] === "number" ? (p[k] as number) : "");
  const [recipe, setRecipe] = useState(p.recipe ? JSON.stringify(p.recipe) : "");
  return (
    <fieldset className="rules" disabled={disabled}>
      <legend>物品数值（价值单位：铜币）</legend>
      <div className="row wrap">
        <label className="check">
          价值 <input type="number" min={0} value={num("value")} onChange={(e) => set("value", e.target.value === "" ? null : Number(e.target.value))} style={{ width: 80 }} />
        </label>
        <label className="check">
          稀有度
          <select value={(p.rarity as string) ?? "common"} onChange={(e) => set("rarity", e.target.value)}>
            {RARITY_OPTS.map(([k, l]) => (
              <option key={k} value={k}>{l}</option>
            ))}
          </select>
        </label>
        <label className="check">
          装备栏位
          <select value={(p.slot as string) ?? ""} onChange={(e) => set("slot", e.target.value)}>
            {SLOT_OPTS.map(([k, l]) => (
              <option key={k} value={k}>{l}</option>
            ))}
          </select>
        </label>
      </div>
      <div className="row wrap">
        <label className="check">
          伤害 <input value={(p.damage as string) ?? ""} placeholder="1d8" onChange={(e) => set("damage", e.target.value)} style={{ width: 70 }} />
        </label>
        <label className="check">
          射程（格） <input type="number" min={0} value={num("range")} onChange={(e) => set("range", e.target.value === "" ? null : Number(e.target.value))} style={{ width: 50 }} />
        </label>
        <label className="check">
          护甲 <input type="number" min={0} value={num("armor")} onChange={(e) => set("armor", e.target.value === "" ? null : Number(e.target.value))} style={{ width: 50 }} />
        </label>
        <label className="check">
          使用后恢复 <input value={(p.heal as string) ?? ""} placeholder="2d4+2" onChange={(e) => set("heal", e.target.value)} style={{ width: 70 }} />
        </label>
      </div>
      <label className="check" style={{ width: "100%" }}>
        制作配方（JSON，可选）
        <input
          value={recipe}
          placeholder='{"inputs":[{"item":"材料ID","qty":2}],"skill":"制作","dc":12,"minutes":60}'
          onChange={(e) => setRecipe(e.target.value)}
          onBlur={() => {
            if (!recipe.trim()) return set("recipe", null);
            try {
              set("recipe", JSON.parse(recipe));
            } catch {
              /* 保存时后端会报告格式错误 */
            }
          }}
          style={{ flex: 1 }}
        />
      </label>
      <p className="hint">加成、命中附带状态（on_hit）、解除状态（cures）可以写在下面的"附加资料（JSON）"里。</p>
    </fieldset>
  );
}

/** 人物的战斗数值（初始值；战役中的变化记在战役里）与商店设置。 */
export function CharacterStatsEditor({ payload, setPayload, disabled }: { payload: string; setPayload: (s: string) => void; disabled?: boolean }) {
  const { p, set } = usePayload(payload, setPayload);
  const stats = (p.stats && typeof p.stats === "object" ? p.stats : {}) as Record<string, unknown>;
  const setStat = (k: string, v: unknown) => {
    const next = { ...stats };
    if (v === "" || v === null || (typeof v === "number" && Number.isNaN(v))) delete next[k];
    else next[k] = v;
    set("stats", Object.keys(next).length ? next : null);
  };
  const attrs = (stats.attrs && typeof stats.attrs === "object" ? stats.attrs : {}) as Record<string, number>;
  const shop = (p.shop && typeof p.shop === "object" ? p.shop : null) as Record<string, unknown> | null;
  const [stock, setStock] = useState(shop?.stock ? JSON.stringify(shop.stock) : "[]");
  const setShop = (k: string, v: unknown) => set("shop", { ...(shop ?? {}), [k]: v });
  return (
    <fieldset className="rules" disabled={disabled}>
      <legend>战斗数值（没填的按等级生成）</legend>
      <div className="row wrap">
        {(
          [
            ["level", "等级"],
            ["hp", "生命"],
            ["ac", "防御"],
            ["attack", "攻击加值"],
            ["xp", "击败经验"],
          ] as [string, string][]
        ).map(([k, l]) => (
          <label key={k} className="check">
            {l} <input type="number" min={0} value={typeof stats[k] === "number" ? (stats[k] as number) : ""} onChange={(e) => setStat(k, e.target.value === "" ? "" : Number(e.target.value))} style={{ width: 56 }} />
          </label>
        ))}
        <label className="check">
          伤害 <input value={(stats.damage as string) ?? ""} placeholder="1d6+1" onChange={(e) => setStat("damage", e.target.value)} style={{ width: 70 }} />
        </label>
      </div>
      <div className="row wrap">
        {(
          [
            ["str", "力量"],
            ["dex", "敏捷"],
            ["con", "体质"],
            ["int", "智力"],
            ["wis", "感知"],
            ["cha", "魅力"],
          ] as [string, string][]
        ).map(([k, l]) => (
          <label key={k} className="check">
            {l}
            <input
              type="number"
              min={1}
              max={40}
              value={attrs[k] ?? ""}
              onChange={(e) => {
                const next = { ...attrs };
                if (e.target.value === "") delete next[k];
                else next[k] = Number(e.target.value);
                setStat("attrs", Object.keys(next).length ? next : "");
              }}
              style={{ width: 50 }}
            />
          </label>
        ))}
      </div>
      <label className="check" style={{ width: "100%" }}>
        声音（朗读台词时用）
        <input
          value={((p.voice as Record<string, unknown> | undefined)?.instructions as string) ?? ""}
          placeholder="例如：沙哑的老年男声，说话慢吞吞（不填则按身份和性格生成）"
          onChange={(e) => set("voice", e.target.value.trim() ? { ...((p.voice as object) ?? {}), instructions: e.target.value } : null)}
          style={{ flex: 1 }}
        />
      </label>
      <label className="check">
        <input type="checkbox" checked={!!shop} onChange={(e) => set("shop", e.target.checked ? { stock: [], markup: 1, buy_rate: 0.5, money: 1000 } : null)} />
        这个人物是商人
      </label>
      {shop && (
        <div className="rule">
          <div className="row wrap">
            <label className="check">
              加价倍率 <input type="number" step={0.1} min={0} value={(shop.markup as number) ?? 1} onChange={(e) => setShop("markup", Number(e.target.value))} style={{ width: 60 }} />
            </label>
            <label className="check">
              收购比例 <input type="number" step={0.1} min={0} max={10} value={(shop.buy_rate as number) ?? 0.5} onChange={(e) => setShop("buy_rate", Number(e.target.value))} style={{ width: 60 }} />
            </label>
            <label className="check">
              本钱（铜币） <input type="number" min={0} value={(shop.money as number) ?? 1000} onChange={(e) => setShop("money", Number(e.target.value))} style={{ width: 80 }} />
            </label>
          </div>
          <label className="check" style={{ width: "100%" }}>
            库存（JSON：物品 ID 与数量，没写数量表示不限量）
            <input
              value={stock}
              placeholder='[{"item":"物品ID","qty":3}]'
              onChange={(e) => setStock(e.target.value)}
              onBlur={() => {
                try {
                  setShop("stock", JSON.parse(stock || "[]"));
                } catch {
                  /* 保存时后端会报告格式错误 */
                }
              }}
              style={{ flex: 1 }}
            />
          </label>
        </div>
      )}
    </fieldset>
  );
}
