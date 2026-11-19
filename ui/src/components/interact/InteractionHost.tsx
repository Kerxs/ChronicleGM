// 沉浸式互动：GM 提出（或玩家点场景对象）后，玩家在这里亲自操作。
// 结果全部由程序结算（interaction_act），这里只负责操作与演出：骰子、撬锁小游戏、机关状态、警戒表……
// 互动选项不是限制：输入框一直可用，玩家写别的办法时 GM 照常处理。
import { useEffect, useMemo, useRef, useState } from "react";
import { api } from "../../api";
import type { Ctx } from "../../App";
import type { Interaction } from "../../bindings/Interaction";
import type { ActionDef } from "../../bindings/ActionDef";
import type { ActResult } from "../../bindings/ActResult";
import type { CheckRoll } from "../../bindings/CheckRoll";
import type { InvItem } from "../../bindings/InvItem";
import type { Puzzle } from "../../bindings/Puzzle";
import { play } from "../../lib/sfx";
import { LockPick } from "./LockPick";

export const KIND_ICON: Record<string, string> = {
  check: "🎲",
  lock: "🔒",
  item_use: "🎒",
  investigate: "🔍",
  puzzle: "⚙️",
  social: "💬",
  combine: "🧪",
  stealth: "👣",
  medical: "🩹",
  survival: "🔥",
  ritual: "🕯️",
};

const ALERT = ["未察觉", "怀疑", "搜索", "发现"];

/** 结束时交给 GM 叙述的一句话（作为玩家这一回合的行动）。 */
export function resultLine(i: Interaction): string {
  const r = i.result;
  return `〔亲自操作〕${i.title}：${r?.degree ?? ""}${r?.summary ? `——${r.summary}` : ""}`;
}

export default function InteractionHost({
  ctx,
  campaignId,
  interaction,
  onDice,
  onFinished,
  onChanged,
}: {
  ctx: Ctx;
  campaignId: string;
  interaction: Interaction;
  onDice: (roll: CheckRoll, label: string) => void;
  onFinished: (i: Interaction) => void;
  onChanged: () => void;
}) {
  const [i, setI] = useState(interaction);
  const [busy, setBusy] = useState(false);
  const [log, setLog] = useState<string[]>(() => interaction.steps.map((s) => s.text).filter(Boolean));
  const [inv, setInv] = useState<InvItem[]>([]);
  const [names, setNames] = useState<Record<string, string>>({});
  const [picking, setPicking] = useState<ActionDef | null>(null);
  const [code, setCode] = useState("");
  const [amount, setAmount] = useState(10);
  const [chosen, setChosen] = useState<string[]>([]);
  const [now, setNow] = useState(Date.now());
  useEffect(() => setI(interaction), [interaction]);
  useEffect(() => {
    api
      .rpgOverview(ctx.worldId, campaignId)
      .then((o) => {
        setInv(o.inventory);
        setNames(o.item_names);
      })
      .catch(() => {});
  }, [ctx.worldId, campaignId, i.steps.length]);
  // 真实倒计时。
  const deadline = i.timeout_policy.kind === "real" ? i.timeout_policy.started + i.timeout_policy.amount * 1000 : 0;
  useEffect(() => {
    if (!deadline) return;
    const h = window.setInterval(() => setNow(Date.now()), 250);
    return () => clearInterval(h);
  }, [deadline]);
  const left = deadline ? Math.max(0, Math.ceil((deadline - now) / 1000)) : null;
  const fired = useRef(false);
  useEffect(() => {
    if (left === 0 && !fired.current && i.status === "pending") {
      fired.current = true;
      void act({ action: "timeout" });
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [left]);

  async function act(op: { action: string; input?: unknown; performance?: number }) {
    setBusy(true);
    try {
      const r: ActResult = await api.interactionAct(ctx.worldId, campaignId, i.interaction_id, { action: op.action, input: op.input ?? null, performance: op.performance ?? null });
      if (r.roll) {
        play("dice");
        onDice(r.roll, i.title);
      }
      setLog((l) => [...l, ...r.lines]);
      setI(r.interaction);
      setPicking(null);
      setChosen([]);
      setCode("");
      onChanged();
      if (r.finished) {
        const ok = r.interaction.result?.success;
        window.setTimeout(() => play(ok ? (i.type === "lock" ? "unlock" : "success") : r.interaction.status === "expired" ? "alarm" : "fail"), r.roll ? 1400 : 100);
        window.setTimeout(() => onFinished(r.interaction), r.roll ? 2200 : 700);
      } else if (r.lines.some((l) => l.includes("警报"))) play("alarm");
    } catch (e) {
      ctx.notify(e instanceof Error ? e.message : String(e), "error");
    } finally {
      setBusy(false);
    }
  }

  function start(a: ActionDef) {
    play("click");
    if (a.input || a.id === "pick" || a.id === "hack") setPicking(a);
    else void act({ action: a.id });
  }

  const puzzle = useMemo(() => {
    const d = i.data as { puzzle?: Puzzle; state?: number[]; stage?: number; steps?: number } | null;
    if (!d?.puzzle) return null;
    const p = d.stage && d.stage > 0 ? d.puzzle.stages[d.stage - 1] : d.puzzle;
    return { p, state: d.state ?? p.start, steps: d.steps ?? 0, stage: d.stage ?? 0, total: d.puzzle.stages.length + 1 };
  }, [i.data]);
  const level = (i.data as { level?: number } | null)?.level ?? 0;
  const ritual = i.data as { round?: number; passes?: number; fails?: number; hints?: number } | null;
  const ended = i.status !== "pending";

  return (
    <div className={`ix-card ix-${i.type}${ended ? " ended" : ""}`} role="group" aria-label={i.title}>
      <div className="ix-head">
        <span className="ix-icon">{KIND_ICON[i.type] ?? "✋"}</span>
        <div className="ix-title">
          <strong>{i.title}</strong>
          {i.description && <small>{i.description}</small>}
        </div>
        {left !== null && !ended && <span className={`ix-timer${left <= 10 ? " hot" : ""}`}>⏱ {left}s</span>}
        {i.timeout_policy.kind === "actions" && !ended && <span className="ix-timer">还剩 {Math.max(0, i.timeout_policy.amount - i.steps.length)} 次</span>}
      </div>
      {i.timeout_policy.reason && !ended && <div className="ix-urgent">{i.timeout_policy.reason}</div>}
      {/* 机关：显示当前状态与目标 */}
      {puzzle && (
        <div className="ix-puzzle">
          {puzzle.total > 1 && <div className="hint">第 {puzzle.stage + 1} / {puzzle.total} 阶段</div>}
          <div className="ix-slots">
            {puzzle.state.map((v, k) => (
              <div key={k} className={`ix-slot v${v}${puzzle.p.goal[k] === v ? " ok" : ""}`} style={{ ["--turn" as string]: `${(v / Math.max(2, puzzle.p.modulo)) * 360}deg` }}>
                <span className="ix-slot-face">{puzzle.p.symbols[v] ?? (puzzle.p.modulo === 2 ? (v ? "▲" : "▼") : v)}</span>
                <small>{puzzle.p.labels[k] ?? `#${k + 1}`}</small>
              </div>
            ))}
          </div>
          {puzzle.p.hint && <div className="hint">提示：{puzzle.p.hint}</div>}
          <div className="hint">已经 {puzzle.steps} 步{puzzle.p.max_steps > 0 ? ` / 最多 ${puzzle.p.max_steps} 步` : ""}</div>
        </div>
      )}
      {/* 潜行：警戒表 */}
      {i.type === "stealth" && (
        <div className="ix-alert" aria-label={`警戒：${ALERT[level]}`}>
          {ALERT.map((a, k) => (
            <span key={a} className={k <= level ? `on l${level}` : ""}>{a}</span>
          ))}
        </div>
      )}
      {i.type === "ritual" && ritual?.round !== undefined && ritual.round > 0 && (
        <div className="hint">
          第 {ritual.round} 轮 · 撑住 {ritual.passes ?? 0} · 动摇 {ritual.fails ?? 0}
        </div>
      )}
      {log.length > 0 && (
        <ul className="ix-log">
          {log.slice(-5).map((l, k) => (
            <li key={k} className="anim-fade-up">{l}</li>
          ))}
        </ul>
      )}
      {ended ? (
        <div className={`ix-result ${i.result?.success ? "ok" : "bad"}`}>
          <strong>{i.result?.degree}</strong> {i.result?.summary}
        </div>
      ) : picking ? (
        <div className="ix-input">
          {(picking.id === "pick" || picking.id === "hack") && <LockPick hack={picking.id === "hack"} disabled={busy} onDone={(perf) => void act({ action: picking.id, performance: perf })} />}
          {picking.input === "code" && (
            <form
              className="row"
              onSubmit={(e) => {
                e.preventDefault();
                if (code.trim()) void act({ action: picking.id, input: code.trim() });
              }}
            >
              <input className="ix-code" autoFocus value={code} onChange={(e) => setCode(e.target.value)} placeholder="输入密码" maxLength={16} />
              <button className="primary" disabled={busy || !code.trim()}>
                确定
              </button>
            </form>
          )}
          {picking.input === "amount" && (
            <div className="row">
              <input type="number" min={1} value={amount} onChange={(e) => setAmount(Math.max(1, Number(e.target.value) || 1))} style={{ width: 100 }} />
              <span className="hint">铜币</span>
              <button className="primary" disabled={busy} onClick={() => void act({ action: picking.id, input: amount })}>
                递过去
              </button>
            </div>
          )}
          {(picking.input === "item" || picking.input === "items") && (
            <div className="ix-items">
              {inv.length === 0 && <span className="hint">背包里没有东西</span>}
              {inv.map((it) => {
                const on = chosen.includes(it.item.id);
                return (
                  <button
                    key={it.item.id}
                    className={`ix-item${on ? " on" : ""}`}
                    disabled={busy}
                    draggable
                    onDragStart={(e) => e.dataTransfer.setData("text/item", it.item.id)}
                    onClick={() => {
                      if (picking.input === "item") void act({ action: picking.id, input: { item: it.item.id, on: i.target } });
                      else setChosen((c) => (on ? c.filter((x) => x !== it.item.id) : [...c, it.item.id]));
                    }}
                  >
                    {it.item.title}
                    {it.qty > 1 && <small>×{it.qty}</small>}
                  </button>
                );
              })}
              {picking.input === "items" && (
                <button className="primary" disabled={busy || chosen.length < 2} onClick={() => void act({ action: picking.id, input: chosen })}>
                  {i.type === "ritual" ? "举行仪式" : "组合"}（{chosen.map((c) => names[c] ?? c).join(" + ") || "选两件以上"}）
                </button>
              )}
            </div>
          )}
          {picking.input === "answer" && <SymbolAnswer i={i} busy={busy} onSubmit={(ans) => void act({ action: picking.id, input: ans })} />}
          <button className="link" onClick={() => setPicking(null)}>
            返回
          </button>
        </div>
      ) : (
        <div className="ix-actions">
          {i.available_actions
            .filter((a) => a.id !== "give_up")
            .map((a) => (
              <button key={a.id} className={a.skill ? "ix-act roll" : "ix-act"} disabled={busy || !a.available} title={a.note || (a.skill ? `${a.skill} 检定，难度 ${a.dc}` : "")} onClick={() => start(a)}>
                <span>{a.label}</span>
                {a.skill && <small>{a.skill}{a.dc > 0 ? ` · 难度 ${a.dc}` : ""}</small>}
                {a.note && !a.available && <small className="warn-text">{a.note}</small>}
              </button>
            ))}
          <button className="ix-act ghost" disabled={busy} onClick={() => void act({ action: "give_up" })}>
            放弃
          </button>
        </div>
      )}
      {!ended && (
        <div className="ix-foot hint">
          {i.rules} 也可以不用这些办法，直接在下面写你想怎么做。
        </div>
      )}
    </div>
  );
}

/** 符号破解：从可能的意思里依次选，组成译文。 */
function SymbolAnswer({ i, busy, onSubmit }: { i: Interaction; busy: boolean; onSubmit: (ans: string[]) => void }) {
  const [ans, setAns] = useState<string[]>([]);
  const target = i.target;
  const [def, setDef] = useState<{ symbols: string[]; meanings: string[] } | null>(null);
  useEffect(() => {
    // 符号和可选意思来自场景对象（玩家可见的部分）。
    const d = i.data as { symbols?: string[]; meanings?: string[] } | null;
    if (d?.symbols) setDef({ symbols: d.symbols, meanings: d.meanings ?? [] });
  }, [i.data, target]);
  const symbols = def?.symbols ?? [];
  const meanings = def?.meanings ?? [];
  return (
    <div className="ix-symbols">
      <div className="ix-slots">
        {symbols.map((s, k) => (
          <div key={k} className="ix-slot">
            <span className="ix-slot-face">{s}</span>
            <small>{ans[k] ?? "？"}</small>
          </div>
        ))}
      </div>
      <div className="ix-items">
        {meanings.map((m) => (
          <button key={m} className="ix-item" disabled={busy || ans.length >= symbols.length} onClick={() => setAns((a) => [...a, m])}>
            {m}
          </button>
        ))}
      </div>
      <div className="row">
        <button className="link" onClick={() => setAns([])}>
          清空
        </button>
        <button className="primary" disabled={busy || ans.length !== symbols.length} onClick={() => onSubmit(ans)}>
          提交译文
        </button>
      </div>
    </div>
  );
}
