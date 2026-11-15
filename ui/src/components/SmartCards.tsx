// 智能面板：根据回合结果里的结构化数据（不额外调用模型）弹出小卡片：任务、角色与背包、战斗、骰子、地图、发现。
// 卡片停靠在剧情区下方（不遮挡任何内容），可以固定或关闭；手动关闭的卡片在同一场景里不再弹出。
import { useEffect, useRef, useState } from "react";
import type { TurnResult } from "../bindings/TurnResult";

type Kind = "quest" | "character" | "combat" | "dice" | "map" | "discovery";
type Card = { kind: Kind; title: string; lines: string[]; tab?: string; pinned: boolean; at: number };

const ICON: Record<Kind, string> = { quest: "📜", character: "🎒", combat: "⚔️", dice: "🎲", map: "🗺️", discovery: "🔍" };
const DEGREE: Record<string, string> = { crit_success: "大成功", success: "成功", partial: "部分成功", failure: "失败", crit_failure: "大失败" };

/** 从回合结果里整理卡片（纯函数，便于测试）。 */
export function cardsFrom(r: TurnResult): Omit<Card, "pinned" | "at">[] {
  const out: Omit<Card, "pinned" | "at">[] = [];
  if (r.quest_updates.length) out.push({ kind: "quest", title: "任务", lines: r.quest_updates, tab: "quests" });
  const gear = r.rpg_updates.filter((l) => /获得|失去|买|卖|装备|经验|升级|铜币|金币|银币|背包|使用/.test(l));
  if (gear.length) out.push({ kind: "character", title: "角色与背包", lines: gear.slice(-4), tab: "character" });
  if (r.combat && r.combat.status === "active") out.push({ kind: "combat", title: `战斗 · 第 ${r.combat.round} 轮`, lines: r.combat.log.slice(-3), tab: "combat" });
  else if (r.combat && r.combat.status !== "active") out.push({ kind: "combat", title: "战斗结束", lines: [...r.combat.rewards, ...r.combat.log.slice(-1)], tab: "combat" });
  if (r.checks.length) out.push({ kind: "dice", title: "检定", lines: r.checks.map((c) => `${c.skill}：${c.total} / 难度 ${c.dc} → ${DEGREE[c.degree ?? ""] ?? (c.success ? "成功" : "失败")}`), tab: "state" });
  const places = [...(r.travel ? [r.travel] : []), ...r.places_heard.map((p) => `听说了：${p}`)];
  if (places.length) out.push({ kind: "map", title: "地图", lines: places, tab: "world" });
  if (r.discoveries.length) out.push({ kind: "discovery", title: "新发现", lines: r.discoveries });
  return out;
}

export default function SmartCards({ last, sceneKey, onOpenTab }: { last: TurnResult | null; sceneKey: string; onOpenTab: (tab: string) => void }) {
  const [cards, setCards] = useState<Card[]>([]);
  const closed = useRef<Set<string>>(new Set());
  const seen = useRef<TurnResult | null>(null);
  // 换场景：清掉没固定的卡片，之前关掉的种类可以重新出现。
  useEffect(() => {
    closed.current = new Set();
    setCards((cs) => cs.filter((c) => c.pinned));
  }, [sceneKey]);
  useEffect(() => {
    if (!last || seen.current === last) return;
    seen.current = last;
    const now = Date.now();
    const fresh = cardsFrom(last).filter((c) => !closed.current.has(c.kind));
    if (!fresh.length) return;
    setCards((cs) => {
      const keep = cs.filter((c) => !fresh.some((f) => f.kind === c.kind) || c.pinned);
      return [...keep.map((c) => (c.pinned ? { ...c, ...(fresh.find((f) => f.kind === c.kind) ?? {}) } : c)), ...fresh.filter((f) => !keep.some((c) => c.kind === f.kind)).map((f) => ({ ...f, pinned: false, at: now }))].slice(-6);
    });
  }, [last]);
  if (!cards.length) return null;
  return (
    <div className="smart-cards" aria-label="本回合的变化">
      {cards.map((c) => (
        <div key={c.kind} className={`smart-card k-${c.kind}${c.pinned ? " pinned" : ""}`}>
          <div className="sc-head">
            <span>
              {ICON[c.kind]} {c.title}
            </span>
            <span className="sc-actions">
              <button className="link" title={c.pinned ? "取消固定" : "固定（换场景也保留）"} onClick={() => setCards((cs) => cs.map((x) => (x.kind === c.kind ? { ...x, pinned: !x.pinned } : x)))}>
                {c.pinned ? "📌" : "📍"}
              </button>
              <button
                className="link"
                title="关闭（这个场景里不再弹出）"
                onClick={() => {
                  closed.current.add(c.kind);
                  setCards((cs) => cs.filter((x) => x.kind !== c.kind));
                }}
              >
                ✕
              </button>
            </span>
          </div>
          {c.lines.slice(0, 4).map((l, i) => (
            <div key={i} className="sc-line">
              {l}
            </div>
          ))}
          {c.tab && (
            <button className="link sc-open" onClick={() => onOpenTab(c.tab!)}>
              查看详情
            </button>
          )}
        </div>
      ))}
    </div>
  );
}
