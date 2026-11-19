// 游戏教程：GM 玻璃球出现在屏幕中间说话，然后飘到要操作的位置；被讲解的元素高亮、周围变暗。
// 等你真的完成那一步（读真实数据判断）才继续。可以暂停（下次接着来）或跳过；进度存在资料库的界面偏好里。
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { api } from "../api";
import type { Page } from "../App";
import { TOUR, type TourStep } from "../lib/tutorialSteps";
import OrbCanvas from "./OrbCanvas";
import { useLowPower } from "../lib/lowPower";

type Rect = { x: number; y: number; w: number; h: number };

/** 这一步要高亮的区域：所有找得到的目标合在一起（例如「选剧本」和「新建战役」一起亮），
 *  合起来太大（超过窗口一半）时只用第一个。 */
function findTarget(step: TourStep): Rect | null {
  const rs: Rect[] = [];
  for (const t of step.target ?? []) {
    const el = document.querySelector<HTMLElement>(`[data-tour="${t}"]`);
    if (!el) continue;
    const r = el.getBoundingClientRect();
    if (r.width === 0 && r.height === 0) continue;
    rs.push({ x: r.left, y: r.top, w: r.width, h: r.height });
  }
  if (!rs.length) return null;
  const x1 = Math.min(...rs.map((r) => r.x));
  const y1 = Math.min(...rs.map((r) => r.y));
  const x2 = Math.max(...rs.map((r) => r.x + r.w));
  const y2 = Math.max(...rs.map((r) => r.y + r.h));
  const all = { x: x1, y: y1, w: x2 - x1, h: y2 - y1 };
  return all.w * all.h > window.innerWidth * window.innerHeight * 0.5 ? rs[0] : all;
}

const ORB = 56;
const GAP = 18;
const EDGE = 12;

function overlaps(a: Rect, b: Rect) {
  return a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
}

/** 玻璃球与气泡的位置：没有目标时在屏幕中间；有目标时依次试右、左、下、上，选第一个放得下且不挡住目标的位置；
 *  都不行就放到不挡目标的角落。`size` 是整个引导（球 + 气泡）的实际尺寸。 */
export function placeGuide(target: Rect | null, size: { w: number; h: number }, vw: number, vh: number, prefer?: number) {
  const { w, h } = size;
  const clampX = (x: number) => Math.min(Math.max(EDGE, x), Math.max(EDGE, vw - w - EDGE));
  const clampY = (y: number) => Math.min(Math.max(EDGE, y), Math.max(EDGE, vh - h - EDGE));
  if (!target) return { x: clampX(vw / 2 - w / 2), y: clampY(vh / 2 - h / 2 - 40), side: "below" as const, slot: -1 };
  const pad = 8;
  const t = { x: target.x - pad, y: target.y - pad, w: target.w + pad * 2, h: target.h + pad * 2 };
  const cands = [
    { x: t.x + t.w + GAP, y: clampY(t.y), side: "right" as const },
    { x: t.x - GAP - w, y: clampY(t.y), side: "left" as const },
    { x: clampX(t.x), y: t.y + t.h + GAP, side: "below" as const },
    { x: clampX(t.x), y: t.y - GAP - h, side: "below" as const },
    { x: vw - w - EDGE, y: vh - h - EDGE, side: "below" as const },
    { x: EDGE, y: vh - h - EDGE, side: "below" as const },
    { x: vw - w - EDGE, y: EDGE, side: "below" as const },
    { x: EDGE, y: EDGE, side: "below" as const },
  ];
  // 上次用的位置还放得下就继续用（避免内容稍有变化时左右来回跳）。
  const order = prefer != null && prefer >= 0 && prefer < cands.length ? [cands[prefer], ...cands] : cands;
  for (const c of order) {
    const inside = c.x >= EDGE - 0.5 && c.y >= EDGE - 0.5 && c.x + w <= vw - EDGE + 0.5 && c.y + h <= vh - EDGE + 0.5;
    if (inside && !overlaps({ x: c.x, y: c.y, w, h }, t)) return { ...c, slot: cands.indexOf(c) };
  }
  // 窗口太小，怎么放都会挡住：放在离目标最远的一边。
  const belowRoom = vh - (t.y + t.h);
  return { x: clampX(t.x), y: belowRoom > t.y ? vh - h - EDGE : EDGE, side: "below" as const, slot: -1 };
}

export default function Tutorial({
  worldId,
  startAt,
  go,
  onSelectWorld,
  onClose,
}: {
  worldId: string | null;
  startAt: number;
  go: (p: Page | { name: "lobby" }) => void;
  onSelectWorld: (id: string) => void;
  onClose: (finished: boolean) => void;
}) {
  const [idx, setIdx] = useState(Math.min(Math.max(0, startAt), TOUR.length - 1));
  const lowPower = useLowPower();
  const [rect, setRect] = useState<Rect | null>(null);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const step = TOUR[idx];
  // 引导（球 + 气泡）的实际尺寸：气泡里文字多少不一样，按实际大小找不挡住目标的位置。
  const guideRef = useRef<HTMLDivElement>(null);
  const [guideSize, setGuideSize] = useState({ w: ORB + 10 + 340, h: 240 });
  useLayoutEffect(() => {
    const el = guideRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => {
      const w = el.offsetWidth;
      const h = el.offsetHeight;
      setGuideSize((o) => (Math.abs(o.w - w) < 1 && Math.abs(o.h - h) < 1 ? o : { w, h }));
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  const slotRef = useRef<{ step: number; slot: number }>({ step: -1, slot: -1 });
  const envRef = useRef({ worldId });
  envRef.current = { worldId };

  const back = useCallback(() => {
    setErr(null);
    setIdx((i) => Math.max(0, i - 1));
  }, []);

  const next = useCallback(() => {
    setErr(null);
    setIdx((i) => {
      const n = i + 1;
      if (n >= TOUR.length) {
        onClose(true);
        return i;
      }
      void api.setUiPref("ui.tutorial", String(n)).catch(() => {});
      return n;
    });
  }, [onClose]);

  // 从中途接着来（上次暂停在示例世界之后）：先打开示例世界，后面的步骤才找得到战役。
  useEffect(() => {
    const demoAt = TOUR.findIndex((s) => s.action?.kind === "demo");
    if (idx > demoAt) {
      api
        .openDemoWorld()
        .then((w) => {
          if (w.id !== envRef.current.worldId) onSelectWorld(w.id);
        })
        .catch(() => {});
    }
    // 只在打开教程时检查一次。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 进入一步：切到对应页面。
  useEffect(() => {
    if (step.page) go({ name: step.page } as Page | { name: "lobby" });
    void api.setUiPref("ui.tutorial", String(idx)).catch(() => {});
    // 告诉页面这一步要讲哪里（游戏面板收着时要先展开）。页面切换后再发一次。
    const say = () => window.dispatchEvent(new CustomEvent("tour-step", { detail: step.target ?? [] }));
    say();
    const t = window.setTimeout(say, 500);
    return () => clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [idx]);

  // 跟踪目标位置（页面切换、内容加载后位置会变）。
  useLayoutEffect(() => {
    const measure = () => {
      const r = findTarget(step);
      setRect((old) => (old && r && Math.abs(old.x - r.x) < 1 && Math.abs(old.y - r.y) < 1 && Math.abs(old.w - r.w) < 1 && Math.abs(old.h - r.h) < 1 ? old : r));
    };
    measure();
    const id = window.setInterval(measure, 400);
    window.addEventListener("resize", measure);
    return () => {
      clearInterval(id);
      window.removeEventListener("resize", measure);
    };
  }, [step]);

  // 自动完成条件：轮询真实数据。
  useEffect(() => {
    if (!step.until) return;
    let alive = true;
    const check = async () => {
      try {
        if (await step.until!(envRef.current)) {
          if (alive) next();
          return;
        }
      } catch {
        /* 数据暂时读不到（例如世界还没打开）：下次再查 */
      }
      if (alive) timer = window.setTimeout(check, 1500);
    };
    let timer = window.setTimeout(check, 600);
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  }, [step, next]);

  // 键盘：Enter 下一步（不需要等待的步骤）、← 上一步、Esc 暂停。输入框里打字时不拦截。
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const tag = (e.target as HTMLElement | null)?.tagName;
      if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return;
      if (e.key === "Escape") onClose(false);
      else if (e.key === "Enter" && !step.until && !step.action) next();
      else if (e.key === "ArrowLeft" && idx > 0) back();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [step, idx, next, back, onClose]);

  async function runAction() {
    if (step.action?.kind !== "demo") return;
    setBusy(true);
    setErr(null);
    try {
      const w = await api.openDemoWorld();
      onSelectWorld(w.id);
      next();
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setBusy(false);
    }
  }

  // 没有要指的东西时：大厅里有大玻璃球，就由它来说话（气泡放在它旁边，不再另放一个小球、不压暗屏幕）。
  const speakerEl = !rect ? document.querySelector<HTMLElement>(".lobby-center .orb-canvas, .lobby-center .orb-static") : null;
  const sr = speakerEl?.getBoundingClientRect();
  const speaker = sr && sr.width > 0 ? { x: sr.left, y: sr.top, w: sr.width, h: sr.height } : null;
  const pos = placeGuide(rect ?? speaker, guideSize, window.innerWidth, window.innerHeight, slotRef.current.step === idx ? slotRef.current.slot : undefined);
  slotRef.current = { step: idx, slot: pos.slot };
  const pad = 6;
  const vw = window.innerWidth;
  const vh = window.innerHeight;
  // 遮罩：整屏 SVG，挖一个圆角洞露出目标（之前用 9999px 的阴影，WebView2 里会被裁成只暗一半，逐帧动画也会卡顿抖动）。
  const hole = rect ? { x: rect.x - pad, y: rect.y - pad, w: rect.w + pad * 2, h: rect.h + pad * 2 } : null;
  const r = 12;
  const holePath = hole
    ? `M${hole.x + r},${hole.y}h${hole.w - 2 * r}a${r},${r} 0 0 1 ${r},${r}v${hole.h - 2 * r}a${r},${r} 0 0 1 -${r},${r}h-${hole.w - 2 * r}a${r},${r} 0 0 1 -${r},-${r}v-${hole.h - 2 * r}a${r},${r} 0 0 1 ${r},-${r}z`
    : "";
  return (
    <div className="tour" role="dialog" aria-label="游戏教程">
      {!speaker && (
        <svg className={`tour-mask${hole ? "" : " soft"}`} width={vw} height={vh} viewBox={`0 0 ${vw} ${vh}`} aria-hidden="true">
          <path d={`M0,0H${vw}V${vh}H0Z${holePath}`} fillRule="evenodd" />
        </svg>
      )}
      {hole && <div className="tour-ring" style={{ transform: `translate(${hole.x}px, ${hole.y}px)`, width: hole.w, height: hole.h }} />}
      <div ref={guideRef} className={`tour-guide side-${pos.side}${speaker ? " no-orb" : ""}`} style={{ transform: `translate(${pos.x}px, ${pos.y}px)` }}>
        <div className="tour-orb" aria-hidden="true">
          {lowPower ? <span /> : <OrbCanvas size={72} active={!!step.until} />}
        </div>
        <div className="tour-bubble" key={step.id}>
          <div className="tour-progress">
            {TOUR.map((s, i) => (
              <span key={s.id} className={i < idx ? "done" : i === idx ? "now" : ""} />
            ))}
          </div>
          <div className="tour-count hint">
            第 {idx + 1} / {TOUR.length} 步
          </div>
          <p>{step.say}</p>
          {err && <p className="warn-text">{err}</p>}
          {step.until && step.waiting && (
            <p className="hint tour-wait">
              <span className="tour-dot" /> {step.waiting}
            </p>
          )}
          <div className="row">
            {step.action && (
              <button className="primary" disabled={busy} onClick={() => void runAction()}>
                {busy ? "正在打开…" : step.action.label}
              </button>
            )}
            {!step.until && !step.action && (
              <button className="primary" onClick={next}>
                {idx === TOUR.length - 1 ? "完成" : "下一步"}
              </button>
            )}
            {step.until && step.skipLabel && (
              <button className="small" onClick={next}>
                {step.skipLabel}
              </button>
            )}
            <span className="spacer" />
            {idx > 0 && (
              <button className="link" onClick={back} title="上一步（←）">
                上一步
              </button>
            )}
            <button className="link" onClick={() => onClose(false)} title="暂停后不会再自动出现；从标题栏世界菜单的「游戏教程」接着这一步看（Esc）">
              暂停
            </button>
            <button
              className="link"
              onClick={() => {
                void api.setUiPref("ui.tutorial", "done").catch(() => {});
                onClose(true);
              }}
            >
              跳过教程
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
