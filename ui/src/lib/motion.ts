// 动效工具：
// - viewTransition：把一次界面状态变化包进 View Transition（WebView2 的 Chromium 支持）。
//   浏览器先截下旧画面，状态立刻变成新的，再用"截图"做过渡：只动合成层（transform / opacity），
//   不会在动画的每一帧重新排版（侧栏收起时的大面积布局偏移就是这样消掉的）。
// - 方向：data-nav-dir="forward|back|none" 写在 <html> 上，CSS 据此左右滑动。
// - 系统设置了"减少动态效果"、或不支持 View Transition 时，直接改状态。
import { flushSync } from "react-dom";

export type NavDir = "forward" | "back" | "none";

const reduced = () => {
  try {
    return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  } catch {
    return false;
  }
};

let running: { skipTransition?: () => void } | null = null;

export function viewTransition(update: () => void, dir: NavDir = "none") {
  const doc = document as Document & { startViewTransition?: (cb: () => void) => { finished: Promise<void>; ready: Promise<void>; updateCallbackDone: Promise<void>; skipTransition?: () => void } };
  if (!doc.startViewTransition || reduced()) {
    update();
    return;
  }
  // 连续快速点击：跳过上一段过渡，避免排队。
  running?.skipTransition?.();
  document.documentElement.dataset.navDir = dir;
  const t = doc.startViewTransition(() => flushSync(update));
  running = t;
  // 被下一次点击跳过时，这几个 Promise 会以 AbortError 结束：属于正常情况，不当作错误。
  t.ready.catch(() => {});
  t.updateCallbackDone.catch(() => {});
  t.finished
    .catch(() => {})
    .finally(() => {
      if (running === t) running = null;
    });
}

/** 按顺序表判断方向：后面的 → forward，前面的 → back。 */
export function dirBetween(order: string[], from: string, to: string): NavDir {
  const a = order.indexOf(from);
  const b = order.indexOf(to);
  if (a < 0 || b < 0 || a === b) return "none";
  return b > a ? "forward" : "back";
}
