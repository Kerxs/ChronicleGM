// 游戏教程：GM 玻璃球出现在屏幕中间说话，然后飘到要操作的位置；被讲解的元素高亮、周围变暗。
// 等你真的完成那一步（读真实数据判断）才继续。可以暂停（下次接着来）或跳过；进度存在资料库的界面偏好里。
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { api } from "../api";
import type { Page } from "../App";
import { TOUR, type TourStep } from "../lib/tutorialSteps";

type Rect = { x: number; y: number; w: number; h: number };

function findTarget(step: TourStep): Rect | null {
  for (const t of step.target ?? []) {
    const el = document.querySelector<HTMLElement>(`[data-tour="${t}"]`);
    if (!el) continue;
    const r = el.getBoundingClientRect();
    if (r.width === 0 && r.height === 0) continue;
    return { x: r.left, y: r.top, w: r.width, h: r.height };
  }
  return null;
}

const ORB = 56;
const BUBBLE_W = 340;

/** 玻璃球与气泡的位置：没有目标时在屏幕中间；有目标时放在目标旁边（右边放不下就放左边，再不行放下面）。 */
function place(target: Rect | null) {
  const vw = window.innerWidth;
  const vh = window.innerHeight;
  if (!target) return { x: vw / 2 - ORB / 2, y: vh / 2 - 120, side: "below" as const };
  const gap = 18;
  if (target.x + target.w + gap + ORB + BUBBLE_W < vw) return { x: target.x + target.w + gap, y: Math.min(Math.max(12, target.y), vh - 260), side: "right" as const };
  if (target.x - gap - ORB - BUBBLE_W > 0) return { x: target.x - gap - ORB, y: Math.min(Math.max(12, target.y), vh - 260), side: "left" as const };
  const below = target.y + target.h + gap;
  const y = below + 240 < vh ? below : Math.max(12, target.y - 260);
  const x = Math.min(Math.max(12, target.x), Math.max(12, vw - ORB - BUBBLE_W - 24));
  return { x, y: Math.min(y, Math.max(12, vh - 260)), side: "below" as const };
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
  go: (p: Page) => void;
  onSelectWorld: (id: string) => void;
  onClose: (finished: boolean) => void;
}) {
  const [idx, setIdx] = useState(Math.min(Math.max(0, startAt), TOUR.length - 1));
  const [rect, setRect] = useState<Rect | null>(null);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const step = TOUR[idx];
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
    if (step.page) go({ name: step.page } as Page);
    void api.setUiPref("ui.tutorial", String(idx)).catch(() => {});
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

  const pos = place(rect);
  const pad = 6;
  return (
    <div className="tour" role="dialog" aria-label="游戏教程">
      {rect ? (
        <div className="tour-spot" style={{ transform: `translate(${rect.x - pad}px, ${rect.y - pad}px)`, width: rect.w + pad * 2, height: rect.h + pad * 2 }} />
      ) : (
        <div className="tour-dim" />
      )}
      <div className={`tour-guide side-${pos.side}`} style={{ transform: `translate(${pos.x}px, ${pos.y}px)` }}>
        <div className="tour-orb" aria-hidden="true">
          <span />
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
            <button className="link" onClick={() => onClose(false)} title="下次从这一步接着来（世界菜单里可以重新打开；Esc）">
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
