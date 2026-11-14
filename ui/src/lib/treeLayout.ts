// 世界图谱的树形布局（纯函数）：已展开的节点按层级从左到右排列，子节点在父节点右侧依次排开，
// 父节点竖直居中于自己的子节点。只布局可见（已展开）的部分。

export interface TreeItem {
  id: string;
  parent: string | null;
}

export function treeLayout(items: TreeItem[], opts: { colWidth?: number; rowHeight?: number; perColumn?: number } = {}): Map<string, { x: number; y: number }> {
  const colWidth = opts.colWidth ?? 260;
  const rowHeight = opts.rowHeight ?? 56;
  const kids = new Map<string | null, string[]>();
  const ids = new Set(items.map((i) => i.id));
  for (const it of items) {
    const p = it.parent && ids.has(it.parent) ? it.parent : null;
    kids.set(p, [...(kids.get(p) ?? []), it.id]);
  }
  const pos = new Map<string, { x: number; y: number }>();
  const perColumn = opts.perColumn ?? 30;
  let row = 0;
  const place = (id: string, depth: number, xOffset = 0): number => {
    const ch = kids.get(id) ?? [];
    let y: number;
    if (ch.length === 0) {
      y = row * rowHeight;
      row++;
    } else if (ch.length > perColumn && ch.every((c) => (kids.get(c) ?? []).length === 0)) {
      // 很多没有展开的子节点：分成多列排列，避免拉成一条很长的竖线。
      const top = row;
      const cols = Math.ceil(ch.length / perColumn);
      ch.forEach((c, i) => {
        const col = Math.floor(i / perColumn);
        pos.set(c, { x: (depth + 1) * colWidth + xOffset + col * colWidth * 0.85, y: (top + (i % perColumn)) * rowHeight });
      });
      row = top + Math.min(perColumn, ch.length);
      y = ((top + (row - 1)) / 2) * rowHeight;
      void cols;
    } else {
      const ys = ch.map((c) => place(c, depth + 1, xOffset));
      y = (ys[0] + ys[ys.length - 1]) / 2;
    }
    pos.set(id, { x: depth * colWidth + xOffset, y });
    return y;
  };
  for (const root of kids.get(null) ?? []) place(root, 0);
  return pos;
}
