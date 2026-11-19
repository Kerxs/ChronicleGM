// 骰子托盘：把规则引擎给出的掷骰记录（DiceRecord）逐批演出。
// 3D 物理骰子按需加载（第一次要掷骰时才下载 three.js / cannon-es）；低性能模式、不支持的面数（d2 等）用平面骰子。
// 百分骰（d100）用两颗 d10 演出：十位骰印 00–90，个位骰印 0–9。
// 每颗骰子落地朝上的点数就是记录里的点数；旁边列出完整算式（骰面 + 各项加值 = 总值 ／ 目标 → 结果）。
// 演出不阻塞游戏逻辑：结果早已写进存档，托盘只是在旁边播放。
import { useEffect, useRef, useState } from "react";
import type { DiceRecord } from "../bindings/DiceRecord";
import type { DiceTable, DieSpec } from "../lib/dice3d";
import { splitD100 } from "../lib/d100";

const COLOR: Record<string, string> = { attack: "#d63b4a", damage: "#e8833a", check: "#2f6fe0", dodge: "#14a3a0", initiative: "#7b5be0", morale: "#8d6e63", effect: "#c2185b", other: "#5f6b7a" };
const LABEL: Record<string, string> = { attack: "攻击检定", damage: "伤害", check: "检定", dodge: "闪避检定", initiative: "先攻", morale: "士气", effect: "效果", other: "掷骰" };
const SUPPORTED = new Set([4, 6, 8, 10, 12, 20]);
const MAX_DICE = 8;

/** 一条记录的算式，例如「d20 14 + 属性 敏捷 2 + 技能 射击 3 = 19 ／ 难度 15 → 成功」。 */
export function formula(r: DiceRecord): string {
  const kept = r.kept.length ? r.kept : r.dice.map((_, i) => i);
  const face = r.dice.length > 1 ? `${r.expr}（${r.dice.map((d, i) => (kept.includes(i) ? `${d}` : `(${d})`)).join(" ")}）= ${r.natural}` : `${r.expr} ${r.natural}`;
  const parts = r.parts.map(([k, v]) => ` ${v >= 0 ? "+" : "−"} ${k} ${Math.abs(v)}`).join("");
  const total = r.parts.length ? ` = ${r.total}` : "";
  const target = r.target != null ? ` ／ ${r.category === "attack" ? "防御" : "目标"} ${r.target}` : "";
  return `${face}${parts}${total}${target}${r.outcome ? ` → ${r.outcome}` : ""}`;
}

/** 从队列头取一批（总骰子数不超过 MAX_DICE）。 */
export function takeBatch(q: DiceRecord[]): number {
  let n = 0, dice = 0;
  while (n < q.length && (n === 0 || dice + q[n].dice.length <= MAX_DICE)) {
    dice += q[n].dice.length;
    n++;
  }
  return n;
}

export default function DiceTray({ queue, lowPower, onDone }: { queue: DiceRecord[]; lowPower: boolean; onDone: (n: number) => void }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const table = useRef<DiceTable | null>(null);
  // 骰子桌绑定的是哪个 canvas：每批演出完托盘会收起、canvas 被移除，下一批是新的 canvas，
  // 必须重新建桌子（之前一直往第一次那个已经移除的 canvas 上画，所以只有第一次能看到判定动画）。
  const tableCanvas = useRef<HTMLCanvasElement | null>(null);
  const [batch, setBatch] = useState<DiceRecord[]>([]);
  const [settled, setSettled] = useState(false);
  const [failed3d, setFailed3d] = useState(false);
  const playing = useRef(false);

  useEffect(() => () => table.current?.dispose(), []);
  useEffect(() => {
    const el = canvas.current;
    if (!el) return;
    const ro = new ResizeObserver(() => table.current?.resize());
    ro.observe(el);
    return () => ro.disconnect();
  }, [batch.length > 0]);

  useEffect(() => {
    if (playing.current || queue.length === 0) return;
    playing.current = true;
    const n = takeBatch(queue);
    const b = queue.slice(0, n);
    setBatch(b);
    setSettled(false);
    let finished = false;
    const finish = () => {
      if (finished) return;
      finished = true;
      setSettled(true);
      window.setTimeout(() => {
        playing.current = false;
        setBatch([]);
        onDone(n);
      }, 1500 + 250 * b.length);
    };
    const specs: DieSpec[] = [];
    for (const r of b)
      r.dice.forEach((v, i) => {
        const color = COLOR[r.category] ?? COLOR.other;
        if (r.sides[i] === 100) {
          // 百分骰：两颗 d10（十位 00–90，个位 0–9；100 是 00 + 0）。
          const { tens, ones } = splitD100(v);
          specs.push({ sides: 10, value: tens, color, tens: true }, { sides: 10, value: ones === 0 ? 10 : ones, color });
        } else if (SUPPORTED.has(r.sides[i])) specs.push({ sides: r.sides[i], value: v, color });
      });
    if (lowPower || failed3d || specs.length === 0) {
      window.setTimeout(finish, 450);
      return;
    }
    // 兜底：无论如何 12 秒内结束这一批（演出绝不能卡住游戏）。
    window.setTimeout(finish, 12000);
    // 3D：第一次用到时才加载引擎。
    void import("../lib/dice3d")
      .then((m) => {
        if (!canvas.current) return finish();
        if (!table.current || tableCanvas.current !== canvas.current) {
          table.current?.dispose();
          table.current = new m.DiceTable(canvas.current);
          tableCanvas.current = canvas.current;
        }
        table.current.resize();
        table.current.throw(specs, finish);
      })
      .catch((e) => {
        console.warn("3D 骰子不可用，改用平面骰子：", e);
        setFailed3d(true);
        finish();
      });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [queue, lowPower, failed3d]);

  if (!batch.length) return null;
  const flat = lowPower || failed3d;
  return (
    <div className="dice-tray" aria-live="polite">
      {!flat && <canvas ref={canvas} className="dice-canvas" />}
      <div className={`dice-results${settled || flat ? " show" : ""}`}>
        {batch.map((r, i) => (
          <div key={`${r.stream}-${i}`} className={`dr cat-${r.category}`} style={{ animationDelay: `${i * 0.08}s` }}>
            <span className="dr-what">
              {LABEL[r.category] ?? "掷骰"}
              {r.manual && <em> · 亲手掷</em>}
              <small>{r.purpose}</small>
            </span>
            {(flat || r.sides.some((s) => !SUPPORTED.has(s) && s !== 100)) && (
              <span className="dr-flat">
                {r.dice.map((d, j) => (
                  <b key={j} style={{ background: COLOR[r.category] ?? COLOR.other }}>
                    {d}
                  </b>
                ))}
              </span>
            )}
            <span className="dr-formula">{formula(r)}</span>
          </div>
        ))}
      </div>
    </div>
  );
}
