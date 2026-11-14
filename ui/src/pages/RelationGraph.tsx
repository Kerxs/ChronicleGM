import { useEffect, useMemo, useState } from "react";
import { useRenderTiming } from "../lib/renderTiming";
import { ReactFlow, Background, Controls, MiniMap, type Edge, type Node as FlowNode } from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { api } from "../api";
import type { Ctx } from "../App";
import type { Graph } from "../bindings/Graph";
import type { RelationType } from "../bindings/RelationType";
import { NODE_KIND } from "../lib/labels";
import { radialLayout } from "../lib/graph";

const GROUP_COLOR: Record<string, string> = {
  person: "#ff2d55",
  faction: "#007aff",
  geography: "#34c759",
  event: "#ff9500",
  other: "#8e8e93",
};

export default function RelationGraph({ ctx, center, relTypes, onSelect }: { ctx: Ctx; center: string; relTypes: RelationType[]; onSelect: (id: string) => void }) {
  const [depth, setDepth] = useState(1);
  const [limit, setLimit] = useState(80);
  const [g, setG] = useState<Graph | null>(null);
  const timing = useRenderTiming("关系图", g, () => [g?.nodes.length ?? 0, g?.relations.length ?? 0]);

  useEffect(() => {
    api
      .nodeGraph(ctx.worldId, center, depth, limit)
      .then((x) => {
        timing.mark();
        setG(x);
      })
      .catch((e: Error) => ctx.notify(e.message, "error"));
  }, [ctx.worldId, center, depth, limit, ctx.tick]);

  const { nodes, edges } = useMemo(() => {
    if (!g) return { nodes: [] as FlowNode[], edges: [] as Edge[] };
    const pos = radialLayout(center, g.nodes.map((n) => n.id), g.relations.map((r) => [r.src_id, r.dst_id]));
    const nodes: FlowNode[] = g.nodes.map((n) => ({
      id: n.id,
      position: pos.get(n.id) ?? { x: 0, y: 0 },
      data: { label: `${n.title}\n${NODE_KIND[n.kind] ?? n.kind}` },
      className: n.id === center ? "gnode center" : "gnode",
    }));
    const edges: Edge[] = g.relations.map((r) => {
      const t = relTypes.find((x) => x.id === r.rel_type);
      const color = GROUP_COLOR[t?.group ?? "other"];
      return {
        id: r.id,
        source: r.src_id,
        target: r.dst_id,
        label: (t?.label ?? r.rel_type) + (r.label ? `·${r.label}` : ""),
        style: { stroke: color },
        labelStyle: { fill: color, fontSize: 11 },
        markerEnd: r.directed ? { type: "arrowclosed" as never, color } : undefined,
      };
    });
    return { nodes, edges };
  }, [g, center, relTypes]);

  return (
    <div className="card">
      <div className="row">
        <h3 style={{ margin: 0 }}>关系图</h3>
        <label className="check">
          层数
          <select value={depth} onChange={(e) => setDepth(Number(e.target.value))}>
            <option value={1}>1</option>
            <option value={2}>2</option>
            <option value={3}>3</option>
          </select>
        </label>
        <label className="check">
          最多节点
          <select value={limit} onChange={(e) => setLimit(Number(e.target.value))}>
            {[40, 80, 150, 300].map((v) => (
              <option key={v} value={v}>
                {v}
              </option>
            ))}
          </select>
        </label>
        {g && (
          <small className="hint">
            {g.nodes.length} 个节点 · {g.relations.length} 条关系{g.truncated && " · 已达上限，部分节点未显示"}
          </small>
        )}
      </div>
      <div className="graph">
        <ReactFlow
          nodes={nodes}
          edges={edges}
          fitView
          nodesConnectable={false}
          onNodeDoubleClick={(_, n) => onSelect(n.id)}
          proOptions={{ hideAttribution: true }}
        >
          <Background />
          <Controls />
          <MiniMap pannable zoomable />
        </ReactFlow>
      </div>
      <small className="hint">双击节点跳转；颜色：人物关系 / 势力 / 地理 / 事件 / 其他。只显示邻域，避免一次渲染整个世界。</small>
    </div>
  );
}
