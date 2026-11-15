// 玻璃球里的掷骰演出：骰子翻滚 → 依次落定为真实点数 → 计算过程 → 判定徽章。
// 结果在后端已经确定（gm-check 事件），这里只负责演出，不改变任何数值。
import { useEffect, useState } from "react";
import type { CheckRoll } from "../bindings/CheckRoll";
import type { Degree } from "../bindings/Degree";

export interface ShownCheck {
  skill: string;
  reason: string;
  detail: CheckRoll;
}

export const DEGREE_LABEL: Record<Degree, string> = {
  crit_success: "大成功",
  success: "成功",
  partial: "部分成功",
  failure: "失败",
  crit_failure: "大失败",
};

const ROLL_MS = 900;
const LAND_STEP = 160;

function sidesOf(expr: string, mode: string): number {
  const m = /d(\d+)/i.exec(expr);
  const n = m ? Number(m[1]) : 20;
  return mode === "pool" ? n : n;
}

export default function DiceShow({ check, onDone }: { check: ShownCheck; onDone: () => void }) {
  const d = check.detail;
  const sides = sidesOf(d.expr, d.mode);
  const [faces, setFaces] = useState<number[]>(() => d.dice.map(() => 1 + Math.floor(Math.random() * sides)));
  const [landed, setLanded] = useState(0);
  const [phase, setPhase] = useState<"roll" | "land" | "result">("roll");

  useEffect(() => {
    let reduced = false;
    try {
      reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    } catch {
      /* 默认播放 */
    }
    const timers: number[] = [];
    if (reduced) {
      setFaces(d.dice);
      setLanded(d.dice.length);
      setPhase("result");
      timers.push(window.setTimeout(onDone, 1600));
      return () => timers.forEach(clearTimeout);
    }
    // 翻滚：每 70ms 换一次点数（定时器驱动：窗口被遮挡时也能按时结束）。
    const spin = window.setInterval(() => setFaces((f) => f.map((_, i) => (i < landedRef.current ? d.dice[i] : 1 + Math.floor(Math.random() * sides)))), 70);
    const landedRef = { current: 0 };
    timers.push(
      window.setTimeout(() => {
        setPhase("land");
        d.dice.forEach((_, i) =>
          timers.push(
            window.setTimeout(() => {
              landedRef.current = i + 1;
              setLanded(i + 1);
              setFaces((f) => f.map((x, j) => (j <= i ? d.dice[j] : x)));
              if (i === d.dice.length - 1) {
                clearInterval(spin);
                timers.push(window.setTimeout(() => setPhase("result"), 220));
                timers.push(window.setTimeout(onDone, 220 + 1900));
              }
            }, i * LAND_STEP),
          ),
        );
      }, ROLL_MS),
    );
    return () => {
      clearInterval(spin);
      timers.forEach(clearTimeout);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [check]);

  // 骰池：成功的骰子就是点数最高的那 natural 个。
  const hitIdx = new Set(d.mode === "pool" ? d.dice.map((v, i) => [v, i] as const).sort((a, b) => b[0] - a[0]).slice(0, d.natural).map(([, i]) => i) : []);
  const calc =
    d.mode === "pool"
      ? `成功 ${d.natural} 个 / 需要 ${d.dc} 个`
      : d.mode === "roll_under"
        ? `掷出 ${d.natural} ／ 目标 ≤ ${d.target}`
        : `${d.natural} ${d.modifier >= 0 ? "+" : "−"} ${Math.abs(d.modifier)} = ${d.total} ／ 难度 ${d.dc}`;
  return (
    <div className="dice-show" aria-live="polite">
      <div className="dice-head">
        <span className="dice-skill">{check.skill} 检定</span>
        {d.advantage !== 0 && <span className={`dice-adv ${d.advantage > 0 ? "up" : "down"}`}>{d.advantage > 0 ? "优势" : "劣势"}</span>}
        <span className="dice-expr">{d.expr}</span>
      </div>
      <div className="dice-row">
        {faces.map((v, i) => (
          <span key={i} className={`die${i < landed ? " landed" : " rolling"}${phase === "result" && hitIdx.has(i) ? " hit" : ""}`} style={{ animationDelay: `${i * 40}ms` }}>
            {v}
          </span>
        ))}
        {d.other.length > 0 && phase === "result" && (
          <span className="dice-other" title={d.advantage > 0 ? "优势：另一次较低的掷骰" : "劣势：另一次较高的掷骰"}>
            {d.other.join(" ")}
          </span>
        )}
      </div>
      <div className={`dice-calc${phase === "result" ? " show" : ""}`}>{calc}</div>
      {phase === "result" && <div className={`dice-degree deg-${d.degree}`}>{DEGREE_LABEL[d.degree]}</div>}
    </div>
  );
}
