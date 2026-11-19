// 撬锁 / 破解小游戏：指针在锁芯里来回摆动，在亮起的区域按下就能拨好一颗锁销（共三颗）。
// 表现（0–1）只换算成检定的 −3…+3 修正，成败仍由程序掷骰决定。
import { useEffect, useRef, useState } from "react";
import { play } from "../../lib/sfx";

/** 一颗锁销的得分：指针离目标中心越近越高（0–1）。 */
export function pinScore(pos: number, center: number, width: number): number {
  const d = Math.abs(pos - center);
  return d <= width / 2 ? 1 - (d / (width / 2)) * 0.5 : Math.max(0, 0.5 - (d - width / 2) * 2);
}

export function LockPick({ hack, disabled, onDone }: { hack?: boolean; disabled?: boolean; onDone: (performance: number) => void }) {
  const PINS = 3;
  const [pin, setPin] = useState(0);
  const [scores, setScores] = useState<number[]>([]);
  const [pos, setPos] = useState(0);
  const [targets] = useState(() => Array.from({ length: PINS }, () => 0.2 + Math.random() * 0.6));
  const raf = useRef(0);
  const t0 = useRef(performance.now());
  const speed = 0.0009 + pin * 0.00035;
  useEffect(() => {
    if (pin >= PINS) return;
    const tick = (t: number) => {
      const x = ((t - t0.current) * speed) % 2;
      setPos(x < 1 ? x : 2 - x);
      raf.current = requestAnimationFrame(tick);
    };
    raf.current = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf.current);
  }, [pin, speed]);
  const width = 0.16 - pin * 0.03;
  function press() {
    if (pin >= PINS || disabled) return;
    const s = pinScore(pos, targets[pin], width);
    play(s > 0.6 ? "click" : "fail");
    const next = [...scores, s];
    setScores(next);
    setPin(pin + 1);
    t0.current = performance.now();
    if (next.length >= PINS) {
      const avg = next.reduce((a, b) => a + b, 0) / next.length;
      window.setTimeout(() => onDone(avg), 350);
    }
  }
  useEffect(() => {
    const k = (e: KeyboardEvent) => {
      if (e.code === "Space" && (e.target as HTMLElement)?.tagName !== "INPUT" && (e.target as HTMLElement)?.tagName !== "TEXTAREA") {
        e.preventDefault();
        press();
      }
    };
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  });
  return (
    <div className={`lockpick${hack ? " hack" : ""}`}>
      <div className="lp-pins">
        {targets.map((_, k) => (
          <span key={k} className={k < scores.length ? (scores[k] > 0.6 ? "set" : "loose") : k === pin ? "now" : ""} />
        ))}
      </div>
      {pin < PINS && (
        <div className="lp-track" onClick={press} role="button" aria-label="在亮起的区域按下">
          <div className="lp-zone" style={{ left: `${(targets[pin] - width / 2) * 100}%`, width: `${width * 100}%` }} />
          <div className="lp-needle" style={{ left: `${pos * 100}%` }} />
        </div>
      )}
      <div className="hint">
        {pin < PINS ? `${hack ? "信号" : "锁销"} ${pin + 1}/${PINS}：在亮起的区域点一下（或按空格）` : "正在转动锁芯……"}
      </div>
    </div>
  );
}
