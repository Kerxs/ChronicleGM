// 独立战斗场景（改革第 6 阶段）。游戏状态机进入「战斗」时盖在剧情区上（剧情暂停），GM 玻璃球留在顶部。
// 1. 转场：黑边推入 → 遭遇提示 → 镜头推近战场 → 敌我依次登场 → 行动顺序条（播完通知后端 combat_ready）。
// 2. 战斗：网格战场（单位、障碍、物件）、行动顺序、行动栏（攻击 / 技能 / 防御 / 闪避 / 道具 / 场景交互 / 移动 / 撤退 / 结束回合）。
//    所有行动都交给规则引擎（combat_act）结算；返回的掷骰记录交给骰子演出，结算行交给 AI 异步写描述（不等它）。
// 3. 战后：胜负、奖励，确认后回到探索。
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api";
import type { Ctx } from "../App";
import type { Combat } from "../bindings/Combat";
import type { CombatAction } from "../bindings/CombatAction";
import type { Combatant } from "../bindings/Combatant";
import type { CustomAction } from "../bindings/CustomAction";
import type { DiceRecord } from "../bindings/DiceRecord";
import type { GameState } from "../bindings/GameState";
import type { InvItem } from "../bindings/InvItem";

const PROP_ICON: Record<string, string> = { cover: "📦", fire: "🪔", drop: "🪨", mechanism: "⚙️" };
const STATUS_TEXT: Record<string, string> = { victory: "胜利", defeat: "战败", fled: "撤退成功", ended: "战斗结束" };
type Menu = null | "skill" | "item" | "prop" | "aim";

export function dist(a: { x: number; y: number }, b: { x: number; y: number }): number {
  return Math.max(Math.abs(a.x - b.x), Math.abs(a.y - b.y));
}

/** 行动顺序：从当前行动者开始轮一圈（倒下的排在后面并变灰）。 */
export function turnOrder(c: Combat): Combatant[] {
  const byId = new Map(c.units.map((u) => [u.id, u]));
  const ring = [...c.order.slice(c.turn), ...c.order.slice(0, c.turn)].map((id) => byId.get(id)).filter((u): u is Combatant => !!u);
  return [...ring.filter((u) => !u.out), ...ring.filter((u) => !!u.out)];
}

export default function BattleScene({
  ctx,
  campaignId,
  state,
  lowPower,
  onDice,
  onNarration,
  onChanged,
}: {
  ctx: Ctx;
  campaignId: string;
  state: GameState;
  lowPower: boolean;
  /** 一步结算里的全部掷骰（交给骰子演出）。 */
  onDice: (d: DiceRecord[]) => void;
  /** AI 写好的战斗描述（显示在玻璃球里）。 */
  onNarration: (text: string) => void;
  onChanged: () => void;
}) {
  const [combat, setCombat] = useState<Combat | null>(null);
  const [skills, setSkills] = useState<CustomAction[]>([]);
  const [items, setItems] = useState<InvItem[]>([]);
  const [costs, setCosts] = useState({ attack: 2, move: 1, speed: 4 });
  const [target, setTarget] = useState<string | null>(null);
  const [menu, setMenu] = useState<Menu>(null);
  const [moving, setMoving] = useState(false);
  const [busy, setBusy] = useState(false);
  const [fresh, setFresh] = useState<string[]>([]);
  const intro = state.combat === "intro";
  const after = state.phase === "aftermath";
  const [introStep, setIntroStep] = useState(intro ? 0 : 4);
  const readySent = useRef(false);

  const load = useCallback(async () => {
    try {
      const [ov, last] = await Promise.all([api.rpgOverview(ctx.worldId, campaignId), api.combatLast(ctx.worldId, campaignId)]);
      setCombat(ov.combat ?? last);
      setCosts({ attack: ov.rules.attack_cost, move: ov.rules.move_cost, speed: ov.rules.speed });
      setItems(ov.inventory.filter((i) => i.qty > 0 && (i.item.heal || i.item.cures.length)));
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }, [ctx, campaignId]);
  useEffect(() => {
    void load();
    api.combatSkills(ctx.worldId, campaignId).then(setSkills).catch(() => {});
  }, [load, ctx.worldId, campaignId, ctx.tick]);

  // 转场：按时间推进几个镜头；低性能模式只停一下。播完通知后端进入行动（只发一次）。
  useEffect(() => {
    if (!intro || !combat) return;
    const steps = lowPower ? [0, 150, 250, 300] : [0, 700, 1500, 2300];
    const ts = steps.map((ms, i) => window.setTimeout(() => setIntroStep(i + 1), ms));
    const done = window.setTimeout(
      () => {
        if (readySent.current) return;
        readySent.current = true;
        void api
          .gameCombatReady(ctx.worldId, campaignId)
          .then(onChanged)
          .catch((e: Error) => ctx.notify(e.message, "error"));
      },
      lowPower ? 400 : 3000,
    );
    return () => {
      ts.forEach(clearTimeout);
      clearTimeout(done);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [intro, !!combat, lowPower]);

  const me = combat?.units.find((u) => u.side === "party");
  const myTurn = !!combat && combat.status === "active" && combat.units.find((u) => u.id === combat.order[combat.turn])?.side === "party" && !intro;
  const foes = combat?.units.filter((u) => u.side === "enemy" && !u.out) ?? [];
  const tgt = combat?.units.find((u) => u.id === target && !u.out) ?? null;
  // 行动点不够再攻击一次（实测：攻击后剩 1 点，按钮还能点，点了只报错，回合也不结束）。
  const lowAp = !!me && me.ap < costs.attack;

  async function act(a: Partial<CombatAction>) {
    if (!combat || busy) return;
    setBusy(true);
    setMenu(null);
    setMoving(false);
    try {
      const r = await api.combatAct(ctx.worldId, campaignId, { kind: "end_turn", target: "", item: "", x: null, y: null, custom: "", part: "", ...a });
      setCombat(r.combat);
      setFresh(r.lines);
      if (r.dice.length) onDice(r.dice);
      if (target && r.combat.units.find((u) => u.id === target)?.out) setTarget(null);
      onChanged();
      // AI 按结算写描述：异步，不阻塞下一步操作；失败只提示一次。
      if (r.lines.length)
        void api
          .gmCombatNarrate(ctx.worldId, campaignId, r.lines)
          .then((t) => t && onNarration(t))
          .catch(() => {});
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    } finally {
      setBusy(false);
    }
  }

  const attack = (part = "") => {
    const t = tgt ?? (me ? [...foes].sort((a, b) => dist(me, a) - dist(me, b))[0] : undefined);
    if (!t) return;
    if (me && dist(me, t) > me.range) void act({ kind: "move", target: t.id });
    else void act({ kind: "attack", target: t.id, part });
  };
  const order = useMemo(() => (combat ? turnOrder(combat) : []), [combat]);

  if (!combat) return <div className="battle loading">正在载入战场…</div>;

  if (after || combat.status !== "active") {
    return (
      <div className={`battle aftermath st-${combat.status}`}>
        <div className="after-card">
          <div className="after-title">{STATUS_TEXT[combat.status] ?? "战斗结束"}</div>
          <p className="hint">{combat.reason}</p>
          {combat.rewards.length > 0 && (
            <ul className="after-rewards">
              {combat.rewards.map((r, i) => (
                <li key={i} style={{ animationDelay: `${0.25 + i * 0.12}s` }}>
                  {r}
                </li>
              ))}
            </ul>
          )}
          <div className="after-log">{combat.log.slice(-4).map((l, i) => <div key={i}>{l}</div>)}</div>
          <button
            className="primary"
            onClick={() =>
              void api
                .gameResumeExploring(ctx.worldId, campaignId)
                .then(onChanged)
                .catch((e: Error) => ctx.notify(e.message, "error"))
            }
          >
            继续冒险
          </button>
        </div>
      </div>
    );
  }

  const cell = (x: number, y: number) => {
    const u = combat.units.find((v) => v.x === x && v.y === y && (!v.out || v.out === "down"));
    const p = combat.props.find((q) => q.x === x && q.y === y);
    const wall = combat.obstacles.some(([ox, oy]) => ox === x && oy === y);
    const reach = moving && me && !u && !wall && !p && dist(me, { x, y }) <= costs.speed;
    return (
      <div
        key={`${x}-${y}`}
        className={`bf-cell${wall ? " wall" : ""}${reach ? " reach" : ""}`}
        onClick={() => {
          if (reach) void act({ kind: "move", x, y });
          else if (u && u.side === "enemy" && !u.out) setTarget(u.id);
        }}
      >
        {p && (
          <span className={`bf-prop${p.used ? " used" : ""}`} title={`${p.name}：${p.desc}${p.used ? "（已用过）" : ""}`}>
            {PROP_ICON[p.kind] ?? "◆"}
          </span>
        )}
        {u && (
          <span
            className={`bf-unit side-${u.side}${u.out ? " out" : ""}${u.id === target ? " targeted" : ""}${u.id === combat.order[combat.turn] ? " acting" : ""}${u.dodging ? " dodging" : ""}`}
            title={`${u.name}　生命 ${u.hp}/${u.max_hp}　防御 ${u.ac}　${u.weapon}（${u.damage}，射程 ${u.range}）　速度 ${u.speed}${u.statuses.length ? "　" + u.statuses.map((s) => s.name).join("、") : ""}`}
            style={{ animationDelay: intro ? `${(u.side === "enemy" ? 0.25 : 0) + combat.units.indexOf(u) * 0.08}s` : undefined }}
          >
            <span className="tok">{u.name.slice(0, 1)}</span>
            <span className="hp">
              <i style={{ width: `${Math.max(0, (u.hp / Math.max(1, u.max_hp)) * 100)}%` }} />
            </span>
          </span>
        )}
      </div>
    );
  };

  return (
    <div className={`battle${intro ? ` intro i${introStep}` : ""}`}>
      {intro && (
        <>
          <div className="letterbox top" />
          <div className="letterbox bottom" />
          <div className="encounter">
            <small>遭遇</small>
            <strong>{foes.map((f) => f.name).join("、") || combat.reason}</strong>
            <span>{combat.reason}</span>
          </div>
        </>
      )}
      <div className="battle-top">
        <span className="round">第 {combat.round} 轮</span>
        <div className="order" aria-label="行动顺序">
          {order.map((u, i) => (
            <span key={u.id} className={`ord side-${u.side}${i === 0 && !u.out ? " now" : ""}${u.out ? " out" : ""}`} title={`先攻 ${u.init}（速度 ${u.speed}）`} style={{ animationDelay: intro ? `${1.6 + i * 0.07}s` : undefined }}>
              {u.name}
              <small>{u.init}</small>
            </span>
          ))}
        </div>
      </div>
      <div className="battlefield" style={{ gridTemplateColumns: `repeat(${combat.width}, minmax(0, 1fr))`, gridTemplateRows: `repeat(${combat.height}, minmax(0, 1fr))`, aspectRatio: `${combat.width} / ${combat.height}` }}>
        {Array.from({ length: combat.height }, (_, y) => Array.from({ length: combat.width }, (_, x) => cell(x, y)))}
      </div>
      <div className="battle-bottom">
        <div className="me-card">
          {me && (
            <>
              <strong>{me.name}</strong>
              <span>
                生命 {me.hp}/{me.max_hp} · 防御 {me.ac} · 行动点 {me.ap}
              </span>
              <span className="hint">
                {me.weapon}（{me.damage}）{me.statuses.length > 0 && ` · ${me.statuses.map((s) => s.name).join("、")}`}
              </span>
            </>
          )}
          {tgt && <span className="tgt">目标：{tgt.name}（生命 {tgt.hp}/{tgt.max_hp}，距离 {me ? dist(me, tgt) : "?"}）</span>}
        </div>
        <div className={`actions${myTurn && !busy ? "" : " waiting"}`}>
          <button className={lowAp ? "" : "primary"} disabled={!myTurn || busy || !foes.length || lowAp} onClick={() => attack()} title={lowAp ? `行动点不够攻击（需要 ${costs.attack}）` : tgt ? `攻击${tgt.name}（太远时先靠近）` : "攻击最近的敌人（点战场上的敌人可以换目标）"}>
            攻击
          </button>
          <div className="act-menu">
            <button disabled={!myTurn || busy || !foes.length || lowAp} onClick={() => setMenu(menu === "aim" ? null : "aim")} title="瞄准部位：更难打中，打中有额外效果">
              瞄准 ▾
            </button>
            {menu === "aim" && (
              <div className="pop">
                <button onClick={() => attack("head")}>
                  头部<small>命中 −3 · 伤害 +50%</small>
                </button>
                <button onClick={() => attack("arms")}>
                  手臂<small>命中 −2 · 对方检定 −2 三轮</small>
                </button>
                <button onClick={() => attack("legs")}>
                  腿<small>命中 −1 · 对方防御 −1、检定 −1 三轮</small>
                </button>
              </div>
            )}
          </div>
          <div className="act-menu">
            <button disabled={!myTurn || busy} onClick={() => setMenu(menu === "skill" ? null : "skill")}>
              技能 ▾
            </button>
            {menu === "skill" && (
              <div className="pop">
                {skills.map((s) => (
                  <button key={s.name} disabled={!me || me.ap < s.ap} onClick={() => void act({ kind: "skill", custom: s.name, target: s.range === 0 ? "" : s.kind === "heal" ? me?.id ?? "" : tgt?.id ?? foes[0]?.id ?? "" })}>
                    {s.name}
                    <small>{s.ap} 行动点{s.range ? ` · 射程 ${s.range}` : ""}</small>
                  </button>
                ))}
              </div>
            )}
          </div>
          <button disabled={!myTurn || busy} onClick={() => void act({ kind: "defend" })} title="防御 +2，直到下次行动">
            防御
          </button>
          <button disabled={!myTurn || busy} onClick={() => void act({ kind: "dodge" })} title="到下次行动前，被攻击时掷闪避（1d20 + 速度）对抗攻击">
            闪避
          </button>
          <div className="act-menu">
            <button disabled={!myTurn || busy || !items.length} onClick={() => setMenu(menu === "item" ? null : "item")} title={items.length ? "" : "背包里没有能在战斗中用的东西"}>
              道具 ▾
            </button>
            {menu === "item" && (
              <div className="pop">
                {items.map((i) => (
                  <button key={i.item.id} onClick={() => void act({ kind: "use_item", item: i.item.id })}>
                    {i.item.title} ×{i.qty}
                    <small>{i.item.heal ? `恢复 ${i.item.heal}` : `解除 ${i.item.cures.join("、")}`}</small>
                  </button>
                ))}
              </div>
            )}
          </div>
          <div className="act-menu">
            <button disabled={!myTurn || busy || !combat.props.some((p) => !p.used)} onClick={() => setMenu(menu === "prop" ? null : "prop")}>
              场景交互 ▾
            </button>
            {menu === "prop" && (
              <div className="pop">
                {combat.props
                  .filter((p) => !p.used)
                  .map((p) => {
                    const d = me ? dist(me, p) : 99;
                    return (
                      <span key={p.id} className="prop-row">
                        <button onClick={() => void act(d <= 1 ? { kind: "interact", target: p.id } : { kind: "move", x: p.x + (me && me.x < p.x ? -1 : 1), y: p.y })} title={p.desc}>
                          {PROP_ICON[p.kind] ?? "⚙️"} {p.name}
                          <small>{d <= 1 ? p.desc : `距离 ${d} 格：先走过去`}</small>
                        </button>
                        {p.kind === "cover" && (
                          <button disabled={!me || me.ap < costs.attack} onClick={() => void act({ kind: "destroy", target: p.id })} title="砸毁它：躲在后面的人失去掩护">
                            砸毁<small>{costs.attack} 行动点</small>
                          </button>
                        )}
                      </span>
                    );
                  })}
              </div>
            )}
          </div>
          <button className={moving ? "on" : ""} disabled={!myTurn || busy} onClick={() => setMoving(!moving)} title="点战场上高亮的格子移动">
            移动
          </button>
          <button disabled={!myTurn || busy} onClick={() => void act({ kind: "flee" })} title={combat.no_escape ? `无法撤退：${combat.no_escape}` : "撤退：敏捷检定，旁边的敌人越多越难"}>
            撤退
          </button>
          <button disabled={!myTurn || busy} onClick={() => window.confirm("放下武器投降？接下来怎么样由对方决定。") && void act({ kind: "surrender" })} title="投降：战斗结束，接下来由对方决定">
            投降
          </button>
          <button className={lowAp ? "primary" : "link"} disabled={!myTurn || busy} onClick={() => void act({ kind: "end_turn" })}>
            结束回合
          </button>
          {lowAp && myTurn && <span className="hint">剩 {me?.ap} 点行动点：不够攻击，可以防御、闪避、移动、场景交互，或结束回合</span>}
        </div>
        <div className="battle-log">
          {(fresh.length ? fresh : combat.log.slice(-3)).slice(-5).map((l, i) => (
            <div key={`${l}-${i}`} className="bl-line">
              {l}
            </div>
          ))}
          {!myTurn && !intro && combat.status === "active" && <div className="hint">等待其他人行动…</div>}
        </div>
      </div>
    </div>
  );
}
