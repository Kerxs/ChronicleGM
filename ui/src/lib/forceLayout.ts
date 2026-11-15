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
  const cutoff2 = (3 * k) * (3 * k);
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
        // 只在近处排斥：远处不相连的人物不会被一直推开（以前几个孤立人物会被推到很远，适配视图后整张图缩得很小）。
        if (d2 > cutoff2) continue;
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
      dx[i] -= xs[i] * 0.04;
      dy[i] -= ys[i] * 0.04;
      const d = Math.sqrt(dx[i] * dx[i] + dy[i] * dy[i]) || 1;
      const m = Math.min(d, temp);
      xs[i] += (dx[i] / d) * m;
      ys[i] += (dy[i] / d) * m;
    }
    temp *= 0.97;
  }
  // 收拢：离最近的人物超过 2 倍间距的，沿着朝向中心的方向拉回，直到离最近的人物约 1.6 倍间距。
  if (n > 1) {
    let cx = 0;
    let cy = 0;
    for (let i = 0; i < n; i++) {
      cx += xs[i];
      cy += ys[i];
    }
    cx /= n;
    cy /= n;
    for (let round = 0; round < 3; round++) {
      for (let i = 0; i < n; i++) {
        let best = Infinity;
        let bj = -1;
        for (let j = 0; j < n; j++) {
          if (j === i) continue;
          const d = Math.hypot(xs[i] - xs[j], ys[i] - ys[j]);
          if (d < best) {
            best = d;
            bj = j;
          }
        }
        if (bj < 0 || best <= 2 * k) continue;
        const toC = Math.hypot(xs[i] - cx, ys[i] - cy) || 1;
        const move = Math.min(best - 1.6 * k, Math.max(0, toC - 1.2 * k));
        xs[i] += ((cx - xs[i]) / toC) * move;
        ys[i] += ((cy - ys[i]) / toC) * move;
      }
    }
  }
  ids.forEach((id, i) => pos.set(id, { x: Math.round(xs[i]), y: Math.round(ys[i]) }));
  return pos;
}
