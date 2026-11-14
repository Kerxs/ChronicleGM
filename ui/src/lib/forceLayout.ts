// 力导向布局（纯函数、确定性）：节点互相排斥、相连的节点互相吸引。用于人物关系网络的"自动布局"。
// 初始位置按 ID 顺序排在圆上，所以同样的输入总得到同样的结果。

export interface Pt {
  x: number;
  y: number;
}

export function forceLayout(ids: string[], links: [string, string][], opts: { iterations?: number; spacing?: number } = {}): Map<string, Pt> {
  const n = ids.length;
  const pos = new Map<string, Pt>();
  if (n === 0) return pos;
  const spacing = opts.spacing ?? 140;
  const iterations = opts.iterations ?? (n > 400 ? 80 : 160);
  const index = new Map(ids.map((id, i) => [id, i]));
  const xs = new Float64Array(n);
  const ys = new Float64Array(n);
  const r0 = spacing * Math.sqrt(n) * 0.6;
  ids.forEach((_, i) => {
    const a = (2 * Math.PI * i) / n;
    xs[i] = r0 * Math.cos(a);
    ys[i] = r0 * Math.sin(a);
  });
  const edges = links.map(([a, b]) => [index.get(a), index.get(b)]).filter((e): e is [number, number] => e[0] !== undefined && e[1] !== undefined && e[0] !== e[1]);
  const k = spacing;
  let temp = r0 / 4;
  const dx = new Float64Array(n);
  const dy = new Float64Array(n);
  for (let it = 0; it < iterations; it++) {
    dx.fill(0);
    dy.fill(0);
    for (let i = 0; i < n; i++) {
      for (let j = i + 1; j < n; j++) {
        let ddx = xs[i] - xs[j];
        let ddy = ys[i] - ys[j];
        let d2 = ddx * ddx + ddy * ddy;
        if (d2 < 0.01) {
          ddx = 0.1 * (i - j);
          ddy = 0.1;
          d2 = ddx * ddx + ddy * ddy;
        }
        const f = (k * k) / d2;
        dx[i] += ddx * f;
        dy[i] += ddy * f;
        dx[j] -= ddx * f;
        dy[j] -= ddy * f;
      }
    }
    for (const [a, b] of edges) {
      const ddx = xs[a] - xs[b];
      const ddy = ys[a] - ys[b];
      const d = Math.sqrt(ddx * ddx + ddy * ddy) || 0.01;
      const f = d / k;
      dx[a] -= ddx * f;
      dy[a] -= ddy * f;
      dx[b] += ddx * f;
      dy[b] += ddy * f;
    }
    for (let i = 0; i < n; i++) {
      // 轻微的向心力，防止不相连的部分飞得太远。
      dx[i] -= xs[i] * 0.01;
      dy[i] -= ys[i] * 0.01;
      const d = Math.sqrt(dx[i] * dx[i] + dy[i] * dy[i]) || 1;
      const m = Math.min(d, temp);
      xs[i] += (dx[i] / d) * m;
      ys[i] += (dy[i] / d) * m;
    }
    temp *= 0.97;
  }
  ids.forEach((id, i) => pos.set(id, { x: Math.round(xs[i]), y: Math.round(ys[i]) }));
  return pos;
}
