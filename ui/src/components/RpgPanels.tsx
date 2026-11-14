// 游戏页的"角色"与"战斗"面板：角色卡、背包与装备、买卖、制作、成长记录、规则设置；回合制战斗。
// 所有数值由后端程序计算，这里只显示和发出操作。
import { useEffect, useState } from "react";
import type React from "react";
import { api, type RpgOverview } from "../api";
import type { Ctx } from "../App";
import type { Combat } from "../bindings/Combat";
import type { Combatant } from "../bindings/Combatant";
import type { CombatAction } from "../bindings/CombatAction";
import type { EnemySpec } from "../bindings/EnemySpec";
import type { RuleSet } from "../bindings/RuleSet";
import type { Sheet } from "../bindings/Sheet";

const ATTRS: [string, string][] = [
  ["str", "力量"],
  ["dex", "敏捷"],
  ["con", "体质"],
  ["int", "智力"],
  ["wis", "感知"],
  ["cha", "魅力"],
];
const RARITY: Record<string, string> = { common: "普通", uncommon: "优秀", rare: "稀有", epic: "史诗", legendary: "传说" };
const SLOT: Record<string, string> = { weapon: "武器", armor: "护甲", shield: "盾牌", accessory: "饰品" };
const GROWTH: Record<string, string> = { xp: "经验", level: "升级", skill: "技能", attr: "属性", wound: "伤势", heal: "恢复", craft: "制作", edit: "修改", status: "状态" };
const mod = (v: number) => {
  const m = Math.floor((v - 10) / 2);
  return m >= 0 ? `+${m}` : `${m}`;
};

function HpBar({ hp, max }: { hp: number; max: number }) {
  const pct = Math.max(0, Math.min(100, (hp / Math.max(1, max)) * 100));
  return (
    <span className="hpbar" title={`${hp}/${max}`}>
      <span style={{ width: `${pct}%` }} className={pct <= 25 ? "low" : ""} />
    </span>
  );
}

/** 角色：角色卡、加点、背包与装备、商人、制作、成长记录；主持人可以调整数值与规则。 */
export function CharacterPanel({ ctx, campaignId, gmView, ended, refreshKey, onChanged }: { ctx: Ctx; campaignId: string; gmView: boolean; ended: boolean; refreshKey: number; onChanged: () => void }) {
  const [ov, setOv] = useState<RpgOverview | null>(null);
  const [merchant, setMerchant] = useState("");
  const [skill, setSkill] = useState("");
  const [edit, setEdit] = useState<Sheet | null>(null);
  const [gm, setGm] = useState({ money: 0, xp: 0, reason: "" });
  const [rules, setRules] = useState<RuleSet | null>(null);
  const [customText, setCustomText] = useState("[]");
  const load = () =>
    api
      .rpgOverview(ctx.worldId, campaignId)
      .then((o) => {
        setOv(o);
        if (!merchant && o.merchants.length) setMerchant(o.merchants[0].id);
      })
      .catch((e: Error) => ctx.notify(e.message, "error"));
  useEffect(() => {
    void load();
  }, [campaignId, refreshKey]);
  if (!ov) return <p className="hint">加载中…</p>;
  const s = ov.sheet;
  const fail = (e: Error) => ctx.notify(e.message, "error");
  const act = (kind: string, target = "", item = "", qty = 1, reason = "") =>
    void api
      .rpgAction(ctx.worldId, campaignId, kind, target, item, qty, reason)
      .then((msg) => {
        ctx.notify(msg);
        onChanged();
      })
      .catch(fail);
  const m = ov.merchants.find((x) => x.id === merchant);
  return (
    <div className="rpg-panel">
      <div className="card">
        <div className="row wrap">
          <strong className="big">{s.name}</strong>
          <span>{s.level} 级</span>
          <span className="hint">经验 {s.xp}/{ov.next_level_xp}</span>
          <span>
            生命 {s.hp}/{s.max_hp} <HpBar hp={s.hp} max={s.max_hp} />
          </span>
          <span>防御 {ov.ac}</span>
          <span>武器：{ov.weapon}</span>
          <span>💰 {ov.money_text}</span>
          {!ended && <button className="small" onClick={() => setEdit(edit ? null : { ...s, attrs: { ...s.attrs } })}>{edit ? "取消编辑" : "编辑"}</button>}
        </div>
        <div className="row wrap">
          {ATTRS.map(([k, l]) => (
            <span key={k} className="attr">
              {l} <strong>{s.attrs[k] ?? 10}</strong>（{mod(s.attrs[k] ?? 10)}）
              {s.unspent_attr > 0 && !ended && <button className="small" onClick={() => act("spend_attr", k)}>+1</button>}
            </span>
          ))}
        </div>
        <div className="hint">
          技能：{Object.entries(s.skills).length ? Object.entries(s.skills).map(([k, v]) => `${k}+${v}`).join("、") : "无"}
        </div>
        {s.unspent_skill > 0 && !ended && (
          <div className="row">
            <span>可用技能点 {s.unspent_skill}：</span>
            <input value={skill} placeholder="技能名，例如 潜行" onChange={(e) => setSkill(e.target.value)} />
            <button disabled={!skill.trim()} onClick={() => act("spend_skill", skill.trim())}>提升</button>
          </div>
        )}
        {(s.statuses.length > 0 || s.wounds.length > 0) && (
          <div className="warn-text">
            {s.statuses.map((x) => x.name).concat(s.wounds.map((w) => `${w.name}（检定 -${w.penalty}）`)).join("、")}
          </div>
        )}
        {s.hp <= 0 && <p className="warn-text">已经倒下：休息时每小时恢复生命。</p>}
        {edit && (
          <div className="rules">
            <div className="row wrap">
              <label className="check">
                名字 <input value={edit.name} onChange={(e) => setEdit({ ...edit, name: e.target.value })} />
              </label>
              {gmView && (
                <>
                  <label className="check">
                    生命 <input type="number" value={edit.hp} onChange={(e) => setEdit({ ...edit, hp: Number(e.target.value) })} style={{ width: 60 }} />
                  </label>
                  <label className="check">
                    上限 <input type="number" value={edit.max_hp} onChange={(e) => setEdit({ ...edit, max_hp: Number(e.target.value) })} style={{ width: 60 }} />
                  </label>
                  <label className="check">
                    基础防御 <input type="number" value={edit.base_ac} onChange={(e) => setEdit({ ...edit, base_ac: Number(e.target.value) })} style={{ width: 60 }} />
                  </label>
                </>
              )}
            </div>
            {gmView && (
              <div className="row wrap">
                {ATTRS.map(([k, l]) => (
                  <label key={k} className="check">
                    {l} <input type="number" min={1} max={40} value={edit.attrs[k] ?? 10} onChange={(e) => setEdit({ ...edit, attrs: { ...edit.attrs, [k]: Number(e.target.value) } })} style={{ width: 50 }} />
                  </label>
                ))}
              </div>
            )}
            <button
              className="primary"
              onClick={() =>
                void api
                  .rpgSaveSheet(ctx.worldId, campaignId, edit)
                  .then(() => {
                    setEdit(null);
                    onChanged();
                  })
                  .catch(fail)
              }
            >
              保存
            </button>
            {!gmView && <span className="hint">　打开主持人视角可以修改数值。</span>}
          </div>
        )}
        {gmView && !ended && (
          <div className="row wrap">
            <span className="hint">主持人：</span>
            <input type="number" value={gm.money} onChange={(e) => setGm({ ...gm, money: Number(e.target.value) })} style={{ width: 80 }} title="铜币，可以为负" />
            <button onClick={() => act("money", "", "", gm.money, gm.reason)}>调整金钱（铜币）</button>
            <input type="number" value={gm.xp} onChange={(e) => setGm({ ...gm, xp: Number(e.target.value) })} style={{ width: 70 }} />
            <button onClick={() => act("xp", "", "", gm.xp, gm.reason)}>给经验</button>
            <input placeholder="原因" value={gm.reason} onChange={(e) => setGm({ ...gm, reason: e.target.value })} />
          </div>
        )}
      </div>

      <div className="card">
        <strong>背包</strong>
        {ov.merchants.length > 0 && (
          <span className="hint">
            　商人：
            <select value={merchant} onChange={(e) => setMerchant(e.target.value)}>
              {ov.merchants.map((x) => (
                <option key={x.id} value={x.id}>{x.name}</option>
              ))}
            </select>
          </span>
        )}
        {ov.inventory.length === 0 && <p className="hint">背包是空的。</p>}
        <table className="grid-table">
          <tbody>
            {ov.inventory.map((i) => (
              <tr key={i.item.id}>
                <td>
                  <span className={`rarity r-${i.item.rarity}`}>{i.item.title}</span> ×{i.qty}
                  {i.equipped && <span className="ok-text">（已装备）</span>}
                  <div className="hint">
                    {[RARITY[i.item.rarity], SLOT[i.item.slot], i.item.damage && `伤害 ${i.item.damage}`, i.item.armor ? `护甲 +${i.item.armor}` : "", i.item.heal && `恢复 ${i.item.heal}`, i.source && `来源：${i.source}`].filter(Boolean).join(" · ")}
                  </div>
                </td>
                <td>
                  {!ended && i.item.slot && !i.equipped && <button className="small" onClick={() => act("equip", "", i.item.id)}>装备</button>}
                  {!ended && i.equipped && <button className="small" onClick={() => act("unequip", i.item.slot)}>卸下</button>}
                  {!ended && (i.item.heal || i.item.cures.length > 0) && <button className="small" onClick={() => act("use", "", i.item.id)}>使用</button>}
                  {!ended && m && (
                    <button className="small" onClick={() => act("sell", m.id, i.item.id, 1)}>
                      卖出（{ov.sell_prices[m.id]?.[i.item.id] ?? 0} 铜）
                    </button>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {m && (
        <div className="card">
          <strong>{m.name}的商品</strong>
          <span className="hint">
            　收购按价值的 {Math.round(m.shop.buy_rate * 100)}%{m.shop.discount >= 0.01 ? `，关系好打 ${((1 - m.shop.discount) * 10).toFixed(1)} 折` : ""}；商人有 {m.shop.merchant_money} 铜
          </span>
          <ul className="list">
            {m.shop.items.map((it) => (
              <li key={it.item.id}>
                <span className={`rarity r-${it.item.rarity}`}>{it.item.title}</span>　{it.buy_price} 铜{it.stock !== null ? `（剩 ${it.stock}）` : ""}
                {!ended && <button className="small" disabled={it.stock === 0} onClick={() => act("buy", m.id, it.item.id, 1)}>买</button>}
              </li>
            ))}
          </ul>
          {m.shop.price_notes.length > 0 && <p className="warn-text">{m.shop.price_notes.join("；")}</p>}
          {!ended && <Barter m={m} ov={ov} act={(target, item, qty, item2, qty2) => void api.rpgActionFull(ctx.worldId, campaignId, { kind: "barter", target, item, qty, reason: "", item2, qty2 }).then((msg) => { ctx.notify(msg); onChanged(); }).catch(fail)} />}
          <p className="hint">商人只有在场时才能在游戏中交易；这里方便测试与主持人操作。</p>
        </div>
      )}

      {ov.recipes.length > 0 && (
        <div className="card">
          <strong>制作</strong>
          <ul className="list">
            {ov.recipes.map(([it, ok]) => (
              <li key={it.id}>
                {it.title}
                <span className="hint">
                  　材料：{it.recipe?.inputs.map((x) => `${ov.item_names[x.item] ?? "?"}×${x.qty}`).join("、")}
                  {it.recipe?.skill ? `；${it.recipe.skill} 难度 ${it.recipe.dc}` : ""}
                  {it.recipe?.minutes ? `；${it.recipe.minutes} 分钟` : ""}
                </span>
                {!ended && <button className="small" disabled={!ok} onClick={() => act("craft", "", it.id)}>{ok ? "制作" : "材料不够"}</button>}
              </li>
            ))}
          </ul>
        </div>
      )}

      <div className="card">
        <strong>成长记录</strong>
        {ov.growth.length === 0 && <p className="hint">还没有记录。</p>}
        <ul className="list">
          {ov.growth.slice(0, 40).map((g) => (
            <li key={g.id}>
              <span className="hint">{g.time}</span>　<span className="badge">{GROWTH[g.kind] ?? g.kind}</span> {g.text}
            </li>
          ))}
        </ul>
      </div>

      {gmView && (
        <div className="card">
          <strong>规则设置（本战役）</strong>
          {!rules ? (
            <button
              className="small"
              onClick={() => {
                setRules(ov.rules);
                setCustomText(JSON.stringify(ov.rules.custom_actions, null, 2));
              }}
            >
              编辑
            </button>
          ) : (
            <div className="rules">
              <div className="row wrap">
                {(
                  [
                    ["ap_per_turn", "每回合行动点"],
                    ["attack_cost", "攻击消耗"],
                    ["move_cost", "移动消耗"],
                    ["speed", "一次移动格数"],
                    ["crit_on", "暴击点数"],
                    ["xp_curve", "经验曲线"],
                    ["hp_per_level", "每级生命"],
                    ["skill_points_per_level", "每级技能点"],
                    ["heal_per_hour", "每小时恢复"],
                    ["max_xp_award", "单次经验上限"],
                  ] as [keyof RuleSet, string][]
                ).map(([k, l]) => (
                  <label key={k} className="check">
                    {l}
                    <input type="number" value={rules[k] as number} onChange={(e) => setRules({ ...rules, [k]: Number(e.target.value) })} style={{ width: 70 }} />
                  </label>
                ))}
                <label className="check">
                  玩家生命降到 0
                  <select value={rules.party_death} onChange={(e) => setRules({ ...rules, party_death: e.target.value })}>
                    <option value="downed">倒地</option>
                    <option value="dead">死亡</option>
                  </select>
                </label>
              </div>
              <div className="hint">自定义行动（JSON 列表：name、ap、kind=attack/heal/status、dice、attr、range、status）：</div>
              <textarea rows={5} value={customText} onChange={(e) => setCustomText(e.target.value)} style={{ width: "100%" }} />
              <div className="row">
                <button
                  className="primary"
                  onClick={() => {
                    let custom;
                    try {
                      custom = JSON.parse(customText || "[]");
                    } catch {
                      return ctx.notify("自定义行动不是有效的 JSON", "error");
                    }
                    void api
                      .rpgSetRules(ctx.worldId, campaignId, { ...rules, custom_actions: custom })
                      .then(() => {
                        setRules(null);
                        ctx.notify("规则已保存");
                        onChanged();
                      })
                      .catch(fail);
                  }}
                >
                  保存规则
                </button>
                <button onClick={() => setRules(null)}>取消</button>
              </div>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

const SIDE: Record<string, string> = { party: "玩家", ally: "同伴", enemy: "敌人" };
const OUT: Record<string, string> = { down: "倒下", fled: "逃走", dead: "死亡" };
const RESULT: Record<string, string> = { victory: "胜利", defeat: "失败", fled: "逃脱", ended: "中止", active: "进行中" };

/** 战斗：网格战场、先攻顺序、行动点；玩家点格子移动、点敌人攻击。敌人和同伴由程序控制。 */
export function CombatPanel({ ctx, campaignId, gmView, ended, refreshKey, onChanged }: { ctx: Ctx; campaignId: string; gmView: boolean; ended: boolean; refreshKey: number; onChanged: () => void }) {
  const [ov, setOv] = useState<RpgOverview | null>(null);
  const [allies, setAllies] = useState<{ id: string; name: string }[]>([]);
  const [pick, setPick] = useState<string[]>([]);
  const [enemies, setEnemies] = useState<EnemySpec[]>([{ name: "强盗", npc: null, level: 1, count: 2, loot: [] }]);
  const [busy, setBusy] = useState(false);
  const [open, setOpen] = useState<string | null>(null);
  const load = () => api.rpgOverview(ctx.worldId, campaignId).then(setOv).catch((e: Error) => ctx.notify(e.message, "error"));
  useEffect(() => {
    void load();
    api
      .npcStates(ctx.worldId, campaignId)
      .then((s) => setAllies(s.npcs.filter((n) => n.status.life_status !== "dead").map((n) => ({ id: n.status.npc_id, name: n.status.name }))))
      .catch(() => setAllies([]));
  }, [campaignId, refreshKey]);
  if (!ov) return <p className="hint">加载中…</p>;
  const fail = (e: Error) => ctx.notify(e.message, "error");
  const done = (lines: string[]) => {
    if (lines.length) ctx.notify(lines.slice(-4).join("；"));
    onChanged();
    void load();
  };
  const run = (a: Partial<CombatAction>) => {
    setBusy(true);
    api
      .combatAct(ctx.worldId, campaignId, { kind: "end_turn", target: "", item: "", x: null, y: null, custom: "", ...a })
      .then((r) => done(r.lines))
      .catch(fail)
      .finally(() => setBusy(false));
  };
  const c: Combat | null = ov.combat;
  if (!c) {
    return (
      <div className="combat-panel">
        {!ended && <div className="card">
          <strong>开始战斗</strong>
          <p className="hint">游戏中冲突爆发时 GM 会自动开始战斗；也可以在这里手动开始。数值由程序生成：等级越高越强。</p>
          {enemies.map((e, i) => (
            <div key={i} className="row wrap">
              <input value={e.name} onChange={(x) => setEnemies(enemies.map((y, j) => (j === i ? { ...y, name: x.target.value } : y)))} placeholder="敌人名字" />
              <label className="check">
                等级 <input type="number" min={1} max={20} value={e.level} onChange={(x) => setEnemies(enemies.map((y, j) => (j === i ? { ...y, level: Number(x.target.value) } : y)))} style={{ width: 50 }} />
              </label>
              <label className="check">
                数量 <input type="number" min={1} max={8} value={e.count} onChange={(x) => setEnemies(enemies.map((y, j) => (j === i ? { ...y, count: Number(x.target.value) } : y)))} style={{ width: 50 }} />
              </label>
              <button className="link danger" onClick={() => setEnemies(enemies.filter((_, j) => j !== i))}>×</button>
            </div>
          ))}
          <button className="small" onClick={() => setEnemies([...enemies, { name: "", npc: null, level: 1, count: 1, loot: [] }])}>添加敌人</button>
          {allies.length > 0 && (
            <div className="row wrap">
              <span className="hint">同伴：</span>
              {allies.map((a) => (
                <label key={a.id} className="check">
                  <input type="checkbox" checked={pick.includes(a.id)} onChange={(e) => setPick(e.target.checked ? [...pick, a.id] : pick.filter((x) => x !== a.id))} />
                  {a.name}
                </label>
              ))}
            </div>
          )}
          {!ended && (
            <button
              className="primary"
              disabled={busy || !enemies.some((e) => e.name.trim())}
              onClick={() => {
                setBusy(true);
                api
                  .combatStart(ctx.worldId, campaignId, enemies.filter((e) => e.name.trim()), pick, "手动开始的战斗")
                  .then((r) => done(r.lines))
                  .catch(fail)
                  .finally(() => setBusy(false));
              }}
            >
              开始战斗
            </button>
          )}
        </div>}
        <div className="card">
          <strong>战斗记录</strong>
          {ov.combats.length === 0 && <p className="hint">还没有战斗。</p>}
          <ul className="list">
            {ov.combats.map((b) => (
              <li key={b.id}>
                <button className="link" onClick={() => setOpen(open === b.id ? null : b.id)}>
                  {b.reason || b.id}
                </button>
                <span className="hint">
                  　{RESULT[b.status] ?? b.status}，{b.round} 轮{b.rewards.length ? `；${b.rewards.join("；")}` : ""}
                </span>
                {open === b.id && <pre className="combat-log">{b.log.join("\n")}</pre>}
              </li>
            ))}
          </ul>
        </div>
      </div>
    );
  }
  const me = c.units.find((u) => u.actor === "party");
  const myTurn = c.status === "active" && c.order[c.turn] === me?.id;
  const at = (x: number, y: number) => c.units.find((u) => u.x === x && u.y === y && !u.out);
  const dist = (a: Combatant, b: { x: number; y: number }) => Math.max(Math.abs(a.x - b.x), Math.abs(a.y - b.y));
  const cells: React.ReactElement[] = [];
  for (let y = 0; y < c.height; y++) {
    for (let x = 0; x < c.width; x++) {
      const u = at(x, y);
      const rock = c.obstacles.some(([ox, oy]) => ox === x && oy === y);
      const cur = u && c.order[c.turn] === u.id;
      const canMove = myTurn && me && !u && !rock && dist(me, { x, y }) <= ov.rules.speed && me.ap >= ov.rules.move_cost;
      const canHit = myTurn && me && u && u.side === "enemy" && dist(me, u) <= me.range && me.ap >= ov.rules.attack_cost;
      cells.push(
        <div
          key={`${x},${y}`}
          className={`cell${u ? ` side-${u.side}` : ""}${rock ? " rock" : ""}${cur ? " current" : ""}${canMove ? " reach" : ""}${canHit ? " target" : ""}`}
          title={u ? `${u.name} 生命 ${u.hp}/${u.max_hp}` : rock ? "障碍物（挡路；远程攻击时可以当掩体，+2 防御）" : `${x},${y}`}
          onClick={() => {
            if (busy) return;
            if (canHit && u) run({ kind: "attack", target: u.id });
            else if (canMove) run({ kind: "move", x, y });
          }}
        >
          {u ? u.name.slice(0, 2) : rock ? "▲" : ""}
        </div>,
      );
    }
  }
  const usable = ov.inventory.filter((i) => i.item.heal || i.item.cures.length > 0);
  return (
    <div className="combat-panel">
      <div className="card">
        <div className="row wrap">
          <strong>第 {c.round} 轮</strong>
          <span>{myTurn ? `轮到你：剩余行动点 ${me?.ap}` : c.status === "active" ? "等待中" : RESULT[c.status]}</span>
          <span className="hint">攻击 {ov.rules.attack_cost} 点，移动 {ov.rules.move_cost} 点（最多 {ov.rules.speed} 格）</span>
        </div>
        <div className="battlefield" style={{ gridTemplateColumns: `repeat(${c.width}, 1fr)` }}>
          {cells}
        </div>
        <p className="hint">点亮的格子可以移动过去，红框的敌人可以攻击。</p>
        {myTurn && (
          <div className="row wrap">
            <button disabled={busy} onClick={() => run({ kind: "defend" })}>防御（+2 防御）</button>
            {usable.map((i) => (
              <button key={i.item.id} disabled={busy} onClick={() => run({ kind: "use_item", item: i.item.id })}>
                使用{i.item.title}（{i.qty}）
              </button>
            ))}
            {ov.rules.custom_actions.map((a) => (
              <button
                key={a.name}
                disabled={busy}
                onClick={() => {
                  const t = a.range === 0 ? "" : (c.units.filter((u) => !u.out && (a.kind === "heal" ? u.side !== "enemy" : u.side === "enemy")).sort((p, q) => dist(me!, p) - dist(me!, q))[0]?.id ?? "");
                  run({ kind: "custom", custom: a.name, target: t });
                }}
              >
                {a.name}（{a.ap} 点）
              </button>
            ))}
            <button disabled={busy} onClick={() => run({ kind: "flee" })}>逃跑</button>
            <button className="primary" disabled={busy} onClick={() => run({ kind: "end_turn" })}>结束回合</button>
          </div>
        )}
        {gmView && (
          <button className="small" onClick={() => void api.combatAbort(ctx.worldId, campaignId, "主持人中止").then((r) => done(r.lines)).catch(fail)}>
            中止战斗
          </button>
        )}
      </div>
      <div className="card">
        <strong>先攻顺序</strong>
        <ul className="list">
          {c.order.map((id, i) => {
            const u = c.units.find((x) => x.id === id)!;
            return (
              <li key={id} className={i === c.turn ? "current-row" : ""}>
                <span className={`side-text side-${u.side}`}>{SIDE[u.side]}</span> <strong>{u.name}</strong>
                <span className="hint">（先攻 {u.init}）</span> 生命 {Math.max(0, u.hp)}/{u.max_hp} <HpBar hp={u.hp} max={u.max_hp} /> 防御 {u.ac} · {u.weapon}
                {u.statuses.length > 0 && <span className="warn-text">　{u.statuses.map((s) => s.name).join("、")}</span>}
                {u.out && <span className="hint">　{OUT[u.out]}</span>}
              </li>
            );
          })}
        </ul>
      </div>
      <div className="card">
        <strong>战斗经过</strong>
        <pre className="combat-log">{c.log.slice(-20).join("\n")}</pre>
      </div>
    </div>
  );
}

/** 以物易物：拿背包里的一样东西换商人的一样东西，差价按收购价与售价用钱补。 */
function Barter({ m, ov, act }: { m: RpgOverview["merchants"][number]; ov: RpgOverview; act: (target: string, item: string, qty: number, item2: string, qty2: number) => void }) {
  const [give, setGive] = useState("");
  const [get, setGet] = useState("");
  if (ov.inventory.length === 0 || m.shop.items.length === 0) return null;
  const gv = give ? ov.sell_prices[m.id]?.[give] ?? 0 : 0;
  const tv = get ? m.shop.items.find((i) => i.item.id === get)?.buy_price ?? 0 : 0;
  return (
    <div className="row wrap">
      <span className="hint">以物易物：</span>
      <select value={give} onChange={(e) => setGive(e.target.value)}>
        <option value="">拿出…</option>
        {ov.inventory.map((i) => (
          <option key={i.item.id} value={i.item.id}>{i.item.title}（作价 {ov.sell_prices[m.id]?.[i.item.id] ?? 0} 铜）</option>
        ))}
      </select>
      <span>换</span>
      <select value={get} onChange={(e) => setGet(e.target.value)}>
        <option value="">想要…</option>
        {m.shop.items.map((i) => (
          <option key={i.item.id} value={i.item.id}>{i.item.title}（{i.buy_price} 铜）</option>
        ))}
      </select>
      {give && get && <span className="hint">{tv - gv > 0 ? `要补 ${tv - gv} 铜` : tv - gv < 0 ? `对方找你 ${gv - tv} 铜` : "正好等价"}</span>}
      <button className="small" disabled={!give || !get} onClick={() => act(m.id, give, 1, get, 1)}>交换</button>
    </div>
  );
}
