import SpoilerHint from "../components/SpoilerHint";
import { useEffect, useMemo, useRef, useState } from "react";
import { ReactFlow, Background, Controls, MiniMap, type Edge, type Node as FlowNode, type ReactFlowInstance } from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { api, type TreeNode } from "../api";
import type { Ctx } from "../App";
import type { Relation } from "../bindings/Relation";
import type { RelationType } from "../bindings/RelationType";
import type { Finding } from "../bindings/Finding";
import type { SearchHit } from "../bindings/SearchHit";
import { NODE_KIND } from "../lib/labels";
import { treeLayout } from "../lib/treeLayout";
import { useRenderTiming } from "../lib/renderTiming";
import { Cover, useCovers } from "../components/Covers";

const PEOPLE = new Set(["character", "faction", "race"]);

/** 世界知识图谱：无限画布，按需展开 / 收起世界树，显示可见节点之间的关系。 */
export default function KnowledgeGraph({ ctx, focus, onOpenTree, onOpenNetwork }: { ctx: Ctx; focus?: string; onOpenTree: (id: string) => void; onOpenNetwork: (id: string) => void }) {
  const [loaded, setLoaded] = useState<Map<string, TreeNode>>(new Map());
  const [kids, setKids] = useState<Map<string | null, string[]>>(new Map());
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [relations, setRelations] = useState<Relation[]>([]);
  const [showRelations, setShowRelations] = useState(true);
  const [relTypes, setRelTypes] = useState<RelationType[]>([]);
  const [findings, setFindings] = useState<Finding[] | null>(null);
  const [sel, setSel] = useState<string | null>(focus ?? null);
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<SearchHit[] | null>(null);
  /** 子节点超过一次加载上限（300）的层级。 */
  const [truncated, setTruncated] = useState<Set<string>>(new Set());
  const flow = useRef<ReactFlowInstance | null>(null);
  /** 等目标节点出现在画布上之后再移动视图（加载与布局是异步的）。 */
  const pendingCenter = useRef<string | null>(null);

  /** 每次重新加载（换世界、数据刷新）加一；晚到的旧请求结果直接丢掉，不覆盖新的（走查时偶发：图谱显示的是上一个世界的节点）。 */
  const loadGen = useRef(0);

  async function loadChildren(parent: string | null): Promise<string[]> {
    const gen = loadGen.current;
    const list = await api.treeChildren(ctx.worldId, parent, 0, 300);
    if (gen !== loadGen.current) return [];
    setLoaded((m) => {
      const n = new Map(m);
      list.forEach((t) => n.set(t.node.id, t));
      return n;
    });
    const ids = list.map((t) => t.node.id);
    setKids((m) => new Map(m).set(parent, ids));
    if (list.length >= 300) setTruncated((t) => new Set(t).add(parent ?? "(根)"));
    return ids;
  }

  /** 某一层再加载 300 个子节点（接在已加载的后面）。 */
  async function loadMore(level: string) {
    const parent = level === "(根)" ? null : level;
    const have = kids.get(parent)?.length ?? 0;
    const list = await api.treeChildren(ctx.worldId, parent, have, 300);
    setLoaded((m) => {
      const n = new Map(m);
      list.forEach((t) => n.set(t.node.id, t));
      return n;
    });
    setKids((m) => new Map(m).set(parent, [...(m.get(parent) ?? []), ...list.map((t) => t.node.id).filter((id) => !(m.get(parent) ?? []).includes(id))]));
    if (list.length < 300)
      setTruncated((t) => {
        const n = new Set(t);
        n.delete(level);
        return n;
      });
  }

  useEffect(() => {
    loadGen.current += 1;
    setLoaded(new Map());
    setKids(new Map());
    setExpanded(new Set());
    void loadChildren(null).then(() => focus && void reveal(focus));
    api.relationTypes().then(setRelTypes).catch(() => {});
  }, [ctx.worldId, ctx.tick]);

  // 可见节点：根节点，以及已展开节点的子节点。
  const visible = useMemo(() => {
    const out: { id: string; parent: string | null }[] = [];
    const walk = (p: string | null) => {
      for (const id of kids.get(p) ?? []) {
        out.push({ id, parent: p });
        if (expanded.has(id)) walk(id);
      }
    };
    walk(null);
    return out;
  }, [kids, expanded]);
  const timing = useRenderTiming("世界图谱", visible.length ? visible : null, () => [visible.length, relations.length]);

  useEffect(() => {
    if (!showRelations || visible.length === 0) return setRelations([]);
    const t = setTimeout(() => {
      api.relationsAmong(ctx.worldId, visible.map((v) => v.id)).then(setRelations).catch(() => setRelations([]));
    }, 150);
    return () => clearTimeout(t);
  }, [visible, showRelations, ctx.worldId]);

  async function toggle(id: string) {
    timing.mark();
    if (expanded.has(id)) {
      setExpanded((s) => {
        const n = new Set(s);
        n.delete(id);
        return n;
      });
      return;
    }
    if (!kids.has(id)) await loadChildren(id);
    setExpanded((s) => new Set(s).add(id));
  }

  /** 展开到某个节点（沿路径逐层加载），选中并移到画面中央。 */
  async function reveal(id: string) {
    const d = await api.nodeDetail(ctx.worldId, id);
    timing.mark();
    const chain = d.ancestors.map((a) => a.id);
    if (!kids.has(null)) await loadChildren(null);
    for (const a of chain) {
      if (!kids.has(a)) await loadChildren(a);
    }
    // 某一层的子节点超过一次加载的上限时，路径上的节点可能不在已加载的列表里：补进去，保证能展开到目标。
    const path = [...d.ancestors, d.node];
    setLoaded((m) => {
      const n = new Map(m);
      path.forEach((x, i) => {
        if (!n.has(x.id)) n.set(x.id, { node: x, child_count: i < path.length - 1 ? 1 : d.child_count });
      });
      return n;
    });
    setKids((m) => {
      const n = new Map(m);
      path.forEach((x, i) => {
        const parent = i === 0 ? null : path[i - 1].id;
        const list = n.get(parent) ?? [];
        if (!list.includes(x.id)) n.set(parent, [...list, x.id]);
      });
      return n;
    });
    setExpanded((s) => {
      const n = new Set(s);
      chain.forEach((a) => n.add(a));
      return n;
    });
    setSel(id);
    pendingCenter.current = id;
  }

  const flagged = useMemo(() => {
    const m = new Map<string, Finding[]>();
    for (const f of findings ?? []) for (const id of f.node_ids) m.set(id, [...(m.get(id) ?? []), f]);
    return m;
  }, [findings]);

  const covers = useCovers(ctx.worldId, visible.map((v) => v.id), ctx.tick);

  const { nodes, edges } = useMemo(() => {
    const pos = treeLayout(visible);
    const nodes: FlowNode[] = visible.map(({ id }) => {
      const t = loaded.get(id)!;
      const open = expanded.has(id);
      const warn = flagged.has(id);
      return {
        id,
        position: pos.get(id) ?? { x: 0, y: 0 },
        data: {
          label: covers[id] ? (
            <span className="gnode-pic">
              <Cover src={covers[id]} size={34} round={PEOPLE.has(t.node.kind)} />
              <span>{`${t.child_count > 0 ? (open ? "▾ " : "▸ ") : ""}${warn ? "⚠ " : ""}${t.node.title}${t.child_count > 0 && !open ? `（${t.child_count}）` : ""}\n${NODE_KIND[t.node.kind] ?? t.node.kind}`}</span>
            </span>
          ) : (
            `${t.child_count > 0 ? (open ? "▾ " : "▸ ") : ""}${warn ? "⚠ " : ""}${t.node.title}${t.child_count > 0 && !open ? `（${t.child_count}）` : ""}\n${NODE_KIND[t.node.kind] ?? t.node.kind}`
          ),
        },
        className: `gnode kg${sel === id ? " center" : ""}${warn ? " flagged" : ""}${covers[id] ? " has-pic" : ""}`,
      };
    });
    const tree: Edge[] = visible.filter((v) => v.parent).map((v) => ({ id: `t:${v.id}`, source: v.parent!, target: v.id, style: { stroke: "#d1d1d6" }, type: "smoothstep" }));
    const rel: Edge[] = relations.map((r) => {
      const label = relTypes.find((x) => x.id === r.rel_type)?.label ?? r.rel_type;
      return {
        id: `r:${r.id}`,
        source: r.src_id,
        target: r.dst_id,
        label,
        animated: false,
        style: { stroke: "#af52de", strokeDasharray: "5 4" },
        labelStyle: { fill: "#af52de", fontSize: 11 },
        markerEnd: r.directed ? { type: "arrowclosed" as never, color: "#af52de" } : undefined,
      };
    });
    return { nodes, edges: [...tree, ...rel] };
  }, [visible, loaded, expanded, relations, relTypes, sel, flagged, covers]);

  useEffect(() => {
    const id = pendingCenter.current;
    const n = id ? nodes.find((x) => x.id === id) : undefined;
    if (!n) return;
    pendingCenter.current = null;
    // 用布局算出的坐标居中（只渲染可见区域时，画面外的节点不会被测量，不能依赖测量结果）。
    const { x, y } = n.position;
    setTimeout(() => flow.current?.setCenter(x + 90, y + 24, { zoom: 1, duration: 300 }), 60);
  }, [nodes]);

  const selNode = sel ? loaded.get(sel) : undefined;

  return (
    <section className="network-page">
      <h2>世界知识图谱</h2>
      <SpoilerHint ctx={ctx} what="图谱" />
      <div className="row wrap">
        <form
          className="row"
          onSubmit={(e) => {
            e.preventDefault();
            if (query.trim()) api.searchNodes(ctx.worldId, query.trim(), { kinds: [], player_view: false, limit: 20, offset: 0 }).then(setHits).catch((x: Error) => ctx.notify(x.message, "error"));
          }}
        >
          <input value={query} onChange={(e) => setQuery(e.target.value)} placeholder="搜索设定（名称、别名、描述）" />
          <button type="submit">搜索</button>
        </form>
        <label className="check">
          <input type="checkbox" checked={showRelations} onChange={(e) => setShowRelations(e.target.checked)} />
          显示可见节点之间的关系
        </label>
        <button onClick={() => setExpanded(new Set())}>全部收起</button>
        <button onClick={() => api.checkConsistency(ctx.worldId).then((f) => { setFindings(f); ctx.notify(f.length ? `发现 ${f.length} 处冲突或重复，已在图上标出 ⚠` : "没有发现冲突或重复"); }).catch((e: Error) => ctx.notify(e.message, "error"))}>
          检查冲突与重复
        </button>
        <small className="hint">
          {visible.length} 个可见节点 · {relations.length} 条关系 · 双击节点展开 / 收起
          {truncated.size > 0 && " · 以下层级还有更多子节点："}
        </small>
        {[...truncated].map((lv) => (
          <button key={lv} className="small" onClick={() => void loadMore(lv).catch((e: Error) => ctx.notify(e.message, "error"))}>
            「{lv === "(根)" ? "根" : loaded.get(lv)?.node.title ?? lv}」再加载 300 个（已有 {kids.get(lv === "(根)" ? null : lv)?.length ?? 0}）
          </button>
        ))}
      </div>
      {hits && (
        <div className="card hits">
          {hits.length === 0 && <span className="hint">没有找到。</span>}
          {hits.map((h) => (
            <button key={h.node.id} className="hit" onClick={() => { setHits(null); void reveal(h.node.id); }}>
              {h.node.title} <small className="hint">{NODE_KIND[h.node.kind]}</small>
            </button>
          ))}
          <button className="link" onClick={() => setHits(null)}>关闭</button>
        </div>
      )}
      <div className="network-body">
        <div className="graph big">
          <ReactFlow
            nodes={nodes}
            edges={edges}
            onInit={(i) => {
              flow.current = i;
              setTimeout(() => i.fitView({ padding: 0.2, maxZoom: 1 }), 100);
            }}
            minZoom={0.03}
            nodesConnectable={false}
            onlyRenderVisibleElements={nodes.length > 300}
            onNodeClick={(_, n) => setSel(n.id)}
            onNodeDoubleClick={(_, n) => void toggle(n.id)}
            proOptions={{ hideAttribution: true }}
          >
            <Background />
            <Controls />
            <MiniMap pannable zoomable />
          </ReactFlow>
        </div>
        <div className="card side">
          {!selNode && <p className="hint">点击节点查看，双击展开或收起。AI 生成的内容会先进入审阅台，你确认后才写入正式世界。</p>}
          {selNode && (
            <div>
              {covers[selNode.node.id] && <img className="side-cover" src={covers[selNode.node.id]} alt="" />}
              <h3>{selNode.node.title}</h3>
              <p className="hint">{NODE_KIND[selNode.node.kind]} · {selNode.child_count} 个子节点 · {selNode.node.canon_level === "core" ? "核心设定" : selNode.node.status === "draft" ? "草稿" : "已确认"}</p>
              {selNode.node.summary && <p>{selNode.node.summary}</p>}
              {(flagged.get(selNode.node.id) ?? []).map((f, i) => (
                <p key={i} className="warn-text">⚠ {f.message}</p>
              ))}
              <div className="row wrap">
                {selNode.child_count > 0 && <button onClick={() => void toggle(selNode.node.id)}>{expanded.has(selNode.node.id) ? "收起" : "展开"}</button>}
                <button onClick={() => onOpenTree(selNode.node.id)}>在世界树中编辑</button>
                {PEOPLE.has(selNode.node.kind) && <button onClick={() => onOpenNetwork(selNode.node.id)}>在人物关系网络中查看</button>}
                <button
                  className="primary"
                  onClick={() =>
                    void api
                      .startGeneration(ctx.worldId, { task: "expand_node", node_id: selNode.node.id, count: 4, kinds: [], guidance: "" })
                      .then(() => ctx.notify("已排队生成子节点。完成后在审阅台确认才会写入世界"))
                      .catch((e: Error) => ctx.notify(e.message, "error"))
                  }
                >
                  AI 扩展子节点
                </button>
              </div>
            </div>
          )}
        </div>
      </div>
    </section>
  );
}
