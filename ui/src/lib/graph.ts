// 关系图布局：以中心节点为圆心，按关系距离分层放在同心圆上（纯函数）。

export function radialLayout(center: string, ids: string[], edges: [string, string][], ring = 220): Map<string, { x: number; y: number }> {
  const adj = new Map<string, string[]>();
  for (const id of ids) adj.set(id, []);
  for (const [a, b] of edges) {
    adj.get(a)?.push(b);
    adj.get(b)?.push(a);
  }
  const depth = new Map<string, number>([[center, 0]]);
  const queue = [center];
  while (queue.length) {
    const cur = queue.shift()!;
    for (const n of adj.get(cur) ?? []) {
      if (!depth.has(n)) {
        depth.set(n, depth.get(cur)! + 1);
        queue.push(n);
      }
    }
  }
  const layers = new Map<number, string[]>();
  for (const id of ids) {
    const d = depth.get(id) ?? Math.max(1, ...depth.values()) + 1;
    layers.set(d, [...(layers.get(d) ?? []), id]);
  }
  const pos = new Map<string, { x: number; y: number }>();
  for (const [d, list] of layers) {
    if (d === 0) {
      pos.set(list[0], { x: 0, y: 0 });
      continue;
    }
    // 外层节点多时加大半径，避免重叠。
    const r = Math.max(ring * d, (list.length * 170) / (2 * Math.PI));
    list.forEach((id, i) => {
      const a = (2 * Math.PI * i) / list.length + d * 0.3;
      pos.set(id, { x: Math.round(r * Math.cos(a)), y: Math.round(r * Math.sin(a)) });
    });
  }
  return pos;
}
