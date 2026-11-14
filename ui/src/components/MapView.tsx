// 图形地图：可见的地点与道路，用确定性的力导向布局（同样的数据每次画出来一样）。
// 已探索实心、听说过虚线、未知（只在主持人视角）半透明；当前位置加圈；点地点查看路线。
import { useMemo } from "react";
import type { MapPlace } from "../bindings/MapPlace";
import type { MapRoad } from "../bindings/MapRoad";

const W = 640;
const H = 420;

function layout(places: MapPlace[], roads: MapRoad[]): Map<string, { x: number; y: number }> {
  const ids = places.map((p) => p.id);
  const idx = new Map(ids.map((id, i) => [id, i]));
  const n = ids.length;
  const pos = ids.map((_, i) => ({ x: W / 2 + Math.cos((2 * Math.PI * i) / Math.max(n, 1)) * W * 0.35, y: H / 2 + Math.sin((2 * Math.PI * i) / Math.max(n, 1)) * H * 0.35 }));
  const edges = roads.map((r) => [idx.get(r.a), idx.get(r.b), r.minutes] as const).filter((e): e is [number, number, number] => e[0] !== undefined && e[1] !== undefined);
  const k = Math.sqrt((W * H) / Math.max(n, 1)) * 0.6;
  for (let it = 0; it < 300; it++) {
    const disp = pos.map(() => ({ x: 0, y: 0 }));
    for (let i = 0; i < n; i++) {
      for (let j = i + 1; j < n; j++) {
        const dx = pos[i].x - pos[j].x;
        const dy = pos[i].y - pos[j].y;
        const d = Math.max(Math.hypot(dx, dy), 1);
        const f = (k * k) / d;
        disp[i].x += (dx / d) * f;
        disp[i].y += (dy / d) * f;
        disp[j].x -= (dx / d) * f;
        disp[j].y -= (dy / d) * f;
      }
    }
    for (const [a, b, m] of edges) {
      const dx = pos[a].x - pos[b].x;
      const dy = pos[a].y - pos[b].y;
      const d = Math.max(Math.hypot(dx, dy), 1);
      // 路程越长，理想距离越远（对数缩放）。
      const ideal = k * (0.5 + Math.log10(1 + m / 15) * 0.6);
      const f = (d - ideal) * 0.08;
      disp[a].x -= (dx / d) * f * 10;
      disp[a].y -= (dy / d) * f * 10;
      disp[b].x += (dx / d) * f * 10;
      disp[b].y += (dy / d) * f * 10;
    }
    const temp = 20 * (1 - it / 300) + 1;
    for (let i = 0; i < n; i++) {
      const d = Math.max(Math.hypot(disp[i].x, disp[i].y), 1);
      pos[i].x = Math.min(W - 40, Math.max(40, pos[i].x + (disp[i].x / d) * Math.min(d, temp)));
      pos[i].y = Math.min(H - 30, Math.max(30, pos[i].y + (disp[i].y / d) * Math.min(d, temp)));
    }
  }
  return new Map(ids.map((id, i) => [id, pos[i]]));
}

function dur(m: number) {
  return m < 60 ? `${m}分` : m < 1440 ? `${Math.round(m / 6) / 10}时` : `${Math.round(m / 144) / 10}天`;
}

export function MapView({ places, roads, onPick }: { places: MapPlace[]; roads: MapRoad[]; onPick: (id: string) => void }) {
  const pos = useMemo(() => layout(places, roads), [places, roads]);
  return (
    <svg className="world-map" viewBox={`0 0 ${W} ${H}`} role="img" aria-label="世界地图">
      {roads.map((r) => {
        const a = pos.get(r.a);
        const b = pos.get(r.b);
        if (!a || !b) return null;
        return (
          <g key={`${r.a}-${r.b}`}>
            <line x1={a.x} y1={a.y} x2={b.x} y2={b.y} className={`road road-${r.kind}`} />
            {r.kind !== "附近" && (
              <text x={(a.x + b.x) / 2} y={(a.y + b.y) / 2 - 3} className="road-label">
                {dur(r.minutes)}
              </text>
            )}
          </g>
        );
      })}
      {places.map((p) => {
        const q = pos.get(p.id)!;
        const r = p.kind === "site" ? 6 : p.kind === "city" ? 9 : 11;
        return (
          <g key={p.id} className={`place state-${p.state}${p.here ? " here" : ""}`} onClick={() => !p.here && onPick(p.id)} style={{ cursor: p.here ? "default" : "pointer" }}>
            <title>{`${p.title}${p.ruler ? `（${p.ruler}）` : ""}${p.markers.length ? "\n" + p.markers.map((m) => "📍" + m.label).join("\n") : ""}`}</title>
            {p.here && <circle cx={q.x} cy={q.y} r={r + 6} className="here-ring" />}
            <circle cx={q.x} cy={q.y} r={r} />
            <text x={q.x} y={q.y + r + 13} className="place-label">
              {p.markers.length > 0 ? "📍" : ""}
              {p.title}
            </text>
            {p.ruler && (
              <text x={q.x} y={q.y + r + 25} className="ruler-label">
                {p.ruler}
              </text>
            )}
          </g>
        );
      })}
    </svg>
  );
}
