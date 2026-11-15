// 数字滚动：值变化时从旧值平滑过渡到新值（缓出曲线），并轻轻弹一下。系统要求减少动态效果时直接显示新值。
import { useEffect, useRef, useState } from "react";

export default function AnimatedNumber({ value, duration = 600 }: { value: number; duration?: number }) {
  const [shown, setShown] = useState(value);
  const [bump, setBump] = useState(false);
  const from = useRef(value);
  useEffect(() => {
    const start = from.current;
    if (start === value) return;
    let reduced = false;
    try {
      reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    } catch {
      /* 默认播放 */
    }
    from.current = value;
    if (reduced) {
      setShown(value);
      return;
    }
    setBump(true);
    const t0 = performance.now();
    let raf = 0;
    // 用定时器兜底：窗口被遮挡时 requestAnimationFrame 会被节流，保证最终值一定显示出来。
    const done = window.setTimeout(() => setShown(value), duration + 50);
    const step = (t: number) => {
      const p = Math.min(1, (t - t0) / duration);
      const e = 1 - Math.pow(1 - p, 3);
      setShown(Math.round(start + (value - start) * e));
      if (p < 1) raf = requestAnimationFrame(step);
    };
    raf = requestAnimationFrame(step);
    const unBump = window.setTimeout(() => setBump(false), 520);
    return () => {
      cancelAnimationFrame(raf);
      clearTimeout(done);
      clearTimeout(unBump);
    };
  }, [value, duration]);
  return <span className={`num-roll${bump ? " bump" : ""}`}>{shown}</span>;
}
