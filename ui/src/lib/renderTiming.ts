// 图谱渲染耗时上报：从数据到达开始计时，到浏览器画出下一帧为止（两次 requestAnimationFrame）。
import { useEffect, useRef } from "react";
import { api } from "../api";

/** 数据到达时调用 `mark()`；数据渲染完后自动上报给性能中心。 */
export function useRenderTiming(view: string, data: unknown, count: () => [number, number]) {
  const start = useRef(0);
  useEffect(() => {
    if (!data || !start.current) return;
    const t0 = start.current;
    start.current = 0;
    requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        const [n, e] = count();
        void api.reportRender(view, n, e, performance.now() - t0);
      }),
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [data]);
  return { mark: () => (start.current = performance.now()) };
}
