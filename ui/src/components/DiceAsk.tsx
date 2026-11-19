// 玻璃球里的"轮到你掷骰"：回合停在检定处，等玩家亲手掷。
// click：点「掷骰」由界面随机出点数；physical：掷实体骰子后输入点数。也可以「交给程序掷」。
// 点数交回后端校验（颗数、范围不对时由程序掷），结果再由 DiceShow 演出。
import { useEffect, useRef, useState } from "react";
import type { DiceAsk as Ask } from "../bindings/DiceAsk";

export type DiceMode = "auto" | "click" | "physical";

/** 解析玩家输入的一组点数（空格、逗号、顿号分隔）；颗数或范围不对时返回 null。 */
export function parseFaces(text: string, count: number, sides: number): number[] | null {
  const xs = text
    .split(/[\s,，、]+/)
    .filter(Boolean)
    .map(Number);
  if (xs.length !== count || xs.some((x) => !Number.isInteger(x) || x < 1 || x > sides)) return null;
  return xs;
}

function randomFace(sides: number): number {
  try {
    const a = new Uint32Array(1);
    crypto.getRandomValues(a);
    return 1 + (a[0] % sides);
  } catch {
    return 1 + Math.floor(Math.random() * sides);
  }
}

export default function DiceAskPanel({ ask, mode, onAnswer }: { ask: Ask; mode: DiceMode; onAnswer: (faces: number[][] | null) => void }) {
  const skill = ask.reason.split(" 检定")[0] || "检定";
  const why = ask.reason.includes("：") ? ask.reason.slice(ask.reason.indexOf("：") + 1) : "";
  const [texts, setTexts] = useState<string[]>(() => Array(ask.rolls).fill(""));
  const [sent, setSent] = useState(false);
  const first = useRef<HTMLInputElement>(null);
  const rollBtn = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    (first.current ?? rollBtn.current)?.focus();
  }, []);
  const parsed = texts.map((t) => parseFaces(t, ask.count, ask.sides));
  const ok = parsed.every((p) => p !== null);
  const answer = (faces: number[][] | null) => {
    if (sent) return;
    setSent(true);
    onAnswer(faces);
  };
  const advTxt = ask.advantage > 0 ? "优势：掷两次取高" : ask.advantage < 0 ? "劣势：掷两次取低" : "";
  const target =
    ask.mode === "pool" ? `需要 ${ask.dc} 个成功` : ask.mode === "roll_under" ? `目标 ≤ ${ask.dc + ask.modifier}` : `难度 ${ask.dc}，修正 ${ask.modifier >= 0 ? "+" : "−"}${Math.abs(ask.modifier)}`;
  return (
    <div className="dice-ask" onClick={(e) => e.stopPropagation()}>
      <div className="dice-head">
        <span className="dice-skill">{skill} 检定</span>
        {advTxt && <span className={`dice-adv ${ask.advantage > 0 ? "up" : "down"}`}>{advTxt}</span>}
        <span className="dice-expr">{ask.expr}</span>
      </div>
      {why && <div className="dice-why">{why}</div>}
      <div className="dice-target">{target}</div>
      {mode === "physical" ? (
        <form
          className="dice-inputs"
          onSubmit={(e) => {
            e.preventDefault();
            if (ok) answer(parsed as number[][]);
          }}
        >
          {texts.map((t, i) => (
            <input
              key={i}
              ref={i === 0 ? first : undefined}
              value={t}
              inputMode="numeric"
              aria-label={ask.rolls > 1 ? `第 ${i + 1} 次掷出的点数` : "掷出的点数"}
              placeholder={ask.count > 1 ? `${ask.count} 颗 d${ask.sides} 的点数，空格分隔` : `d${ask.sides} 掷出的点数（1–${ask.sides}）`}
              className={t && !parsed[i] ? "bad" : ""}
              onChange={(e) => setTexts((xs) => xs.map((x, j) => (j === i ? e.target.value : x)))}
            />
          ))}
          <button type="submit" className="primary" disabled={!ok || sent}>
            确定
          </button>
        </form>
      ) : (
        <button
          type="button"
          ref={rollBtn}
          className="primary dice-roll-btn"
          disabled={sent}
          onClick={() => answer(Array.from({ length: ask.rolls }, () => Array.from({ length: ask.count }, () => randomFace(ask.sides))))}
        >
          🎲 掷骰
        </button>
      )}
      <button type="button" className="link dice-skip" disabled={sent} onClick={() => answer(null)}>
        交给程序掷
      </button>
    </div>
  );
}
