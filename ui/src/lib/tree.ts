// 世界树的展开状态与可见行计算（纯函数，便于测试）。
import type { TreeNode } from "../api";

export const ROOT = "";

export type Row =
  | { type: "node"; depth: number; item: TreeNode; expanded: boolean }
  | { type: "more"; depth: number; parent: string; loaded: number; total: number }
  | { type: "loading"; depth: number; parent: string };

/**
 * 按展开状态把已加载的子节点缓存展开成可见行。
 * cache：父节点 ID（根为 ""）→ 已加载的子节点；totals：父节点 → 子节点总数。
 */
export function visibleRows(cache: Map<string, TreeNode[]>, totals: Map<string, number>, expanded: Set<string>): Row[] {
  const out: Row[] = [];
  const walk = (parent: string, depth: number) => {
    const kids = cache.get(parent);
    if (!kids) {
      if (parent !== ROOT) out.push({ type: "loading", depth, parent });
      return;
    }
    for (const k of kids) {
      const isOpen = expanded.has(k.node.id) && k.child_count > 0;
      out.push({ type: "node", depth, item: k, expanded: isOpen });
      if (isOpen) walk(k.node.id, depth + 1);
    }
    const total = totals.get(parent) ?? kids.length;
    if (kids.length < total) out.push({ type: "more", depth, parent, loaded: kids.length, total });
  };
  walk(ROOT, 0);
  return out;
}

/** 虚拟滚动：给定滚动位置，返回需要渲染的行区间。 */
export function windowRange(count: number, scrollTop: number, viewport: number, rowHeight: number, overscan = 10): [number, number] {
  const start = Math.max(0, Math.floor(scrollTop / rowHeight) - overscan);
  const end = Math.min(count, Math.ceil((scrollTop + viewport) / rowHeight) + overscan);
  return [start, end];
}
