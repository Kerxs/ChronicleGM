// GM 玻璃球：待命是一颗液态玻璃球；GM 构思时球里一条细弧旋转、下方显示回合进行到的阶段；
// 讲述时像灵动岛一样展开成面板，逐字输出叙述，讲完收回，叙述落进剧情流。
// 液态玻璃来自 @samasante/liquid-glass（MIT）。用"材质模式"：玻璃贴合元素，折射背后的页面，内容保持清晰。
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Glass } from "@samasante/liquid-glass";

export type OrbState = "idle" | "thinking" | "speaking";

// 浅色玻璃：边缘色散与唇边折射拉满，磨砂少，高光强（苹果浅色液态玻璃的观感）。
const OPTICS = {
  strength: 0.22,
  depth: 0.55,
  curvature: 0.45,
  dispersion: 1,
  bend: 0.85,
  bendWidth: 0.22,
  frost: 0.6,
  brightness: 0.02,
  saturate: 1.1,
  specular: 1.5,
  sheenAngle: 135,
  glow: 0.25,
};

export const STAGE_LABEL: Record<string, string> = {
  context: "整理上下文",
  plan: "理解你的行动",
  resolve: "程序判定 · 掷骰",
  narrate: "组织叙述",
  npc: "人物正在回应",
};

const CHARS_PER_SEC = 16;

export default function GmOrb({
  state,
  stage,
  text,
  level,
  onFinished,
}: {
  state: OrbState;
  /** 构思时的阶段（后端 gm-stage 事件）。 */
  stage?: string;
  /** 讲述时要输出的文字。 */
  text: string;
  /** 朗读音量 0–1（没有朗读时返回 null，按文字节奏起伏）。 */
  level: () => number | null;
  /** 讲述结束（文字输出完、停留片刻后，或点了跳过）。 */
  onFinished: () => void;
}) {
  const hostRef = useRef<HTMLDivElement>(null);
  const [panelW, setPanelW] = useState(720);
  const geom = useMemo(() => ({ idle: [64, 64, 32], thinking: [76, 76, 38], speaking: [panelW, 0, 30] }) as Record<OrbState, number[]>, [panelW]);
  const boxEl = useRef<HTMLDivElement | null>(null);
  const talkRef = useRef<HTMLDivElement>(null);
  const textRef = useRef<HTMLDivElement>(null);
  const [talkH, setTalkH] = useState(100);
  const [typed, setTyped] = useState(0);
  const [amp, setAmp] = useState(0);
  const [bars, setBars] = useState<number[]>(() => Array(14).fill(4));
  const finished = useRef(false);

  // 面板宽度跟着剧情区宽度走（最宽 720）。
  useEffect(() => {
    const el = hostRef.current?.parentElement;
    if (!el) return;
    const ro = new ResizeObserver(() => setPanelW(Math.max(320, Math.min(720, el.clientWidth - 24))));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // 尺寸用 CSS 过渡（按时间推进，窗口被遮挡、逐帧回调变慢时也能按时到位）；
  // 玻璃组件内部用 ResizeObserver 跟随元素尺寸。组件外层带尺寸隔离，在居中布局里会被压成 0 宽，所以外层也一起设。
  const [tw, th0, tr] = geom[state];
  const th = state === "speaking" ? talkH : th0;
  const applySize = useCallback(() => {
    const el = boxEl.current;
    if (!el) return;
    const wrap = el.closest("[data-liquid-glass]") as HTMLElement | null;
    for (const x of [el, wrap, wrap?.firstElementChild as HTMLElement | null]) {
      if (!x) continue;
      x.style.transition = "width .62s cubic-bezier(.2,.9,.22,1.12), height .62s cubic-bezier(.2,.9,.22,1.12), border-radius .62s";
      x.style.width = `${tw}px`;
      x.style.height = `${th}px`;
      x.style.borderRadius = `${tr}px`;
    }
  }, [tw, th, tr]);
  // Glass 第一次渲染不会立刻挂载子元素：用回调 ref，元素一出现就设尺寸。
  const boxRef = useCallback((el: HTMLDivElement | null) => {
    boxEl.current = el;
    applySize();
  }, [applySize]);
  useEffect(applySize, [applySize]);
  // 讲述时文字逐字增加：文字区域长高，面板跟着长高。
  useEffect(() => {
    if (state !== "speaking" || !talkRef.current) return;
    const el = talkRef.current;
    const ro = new ResizeObserver(() => setTalkH(Math.max(100, Math.ceil(el.getBoundingClientRect().height))));
    ro.observe(el);
    return () => ro.disconnect();
  }, [state]);

  // 讲述：逐字输出；起伏优先跟朗读音量，没有朗读时按文字节奏。输出完停留 1.6 秒再收回。
  useEffect(() => {
    if (state !== "speaking") {
      setAmp(0);
      return;
    }
    finished.current = false;
    setTyped(0);
    let raf = 0;
    let doneAt = 0;
    const t0 = performance.now();
    const tick = (now: number) => {
      const x = (now - t0) / 1000;
      const n = Math.min(text.length, Math.floor(Math.max(0, x - 0.5) * CHARS_PER_SEC));
      setTyped(n);
      const lv = level();
      const a = lv ?? (n < text.length ? Math.min(1, Math.max(0, 0.45 * Math.sin(x * 9.3) * Math.sin(x * 2.1) + 0.3 * Math.sin(x * 15.7 + 1) + 0.3)) : 0);
      setAmp(a);
      setBars((b) => b.map((_, i) => 4 + 12 * Math.abs(Math.sin(x * 7 + i * 0.7)) * (0.25 + a)));
      if (textRef.current) textRef.current.scrollTop = textRef.current.scrollHeight;
      // 文字输出完、且朗读也结束了，才收回。
      if (n >= text.length && lv === null) {
        if (!doneAt) doneAt = now;
        if (now - doneAt > 1600 && !finished.current) {
          finished.current = true;
          onFinished();
          return;
        }
      } else doneAt = 0;
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [state, text]);

  const skip = () => {
    if (finished.current) return;
    finished.current = true;
    setTyped(text.length);
    onFinished();
  };

  const label = state === "thinking" ? STAGE_LABEL[stage ?? ""] ?? "GM 正在构思" : state === "idle" ? "GM" : "";
  return (
    <div ref={hostRef} className={`gm ${state}`} style={{ "--amp": amp, "--talk-w": `${panelW}px` } as React.CSSProperties} aria-live="polite">
      <div className="halo" />
      <Glass optics={OPTICS} className="gm-glass">
        <div className="gm-box" ref={boxRef} style={{ width: 64, height: 64, borderRadius: 32 }}>
          <div className="core">
            <div className="blob b1" />
            <div className="blob b2" />
            <div className="blob b3" />
            <div className="gloss" />
          </div>
          <svg className="spin" viewBox="0 0 100 100" aria-hidden="true">
            <circle className="track" cx="50" cy="50" r="42" />
            <circle className="arc" cx="50" cy="50" r="42" pathLength={100} strokeDasharray="22 78" />
          </svg>
          <div className="talk" ref={talkRef}>
            <div className="who">
              GM · 讲述 <span className="wave">{bars.map((v, i) => <i key={i} style={{ height: v }} />)}</span>
            </div>
            <div className="text" ref={textRef}>
              {text.slice(0, typed)}
              {typed < text.length && <span className="cur" />}
            </div>
            <button type="button" className="skip" onClick={skip}>
              跳过 ⏭
            </button>
          </div>
        </div>
      </Glass>
      <div className="step-label" role="status">{label}</div>
    </div>
  );
}
