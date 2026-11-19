import SpoilerHint from "../components/SpoilerHint";
import { useEffect, useMemo, useRef, useState } from "react";
import { ReactFlow, Background, Controls, MiniMap, type Edge, type Node as FlowNode, type ReactFlowInstance } from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { api } from "../api";
import type { Ctx } from "../App";
import type { Campaign } from "../bindings/Campaign";
import type { Network } from "../bindings/Network";
import type { NetEdge } from "../bindings/NetEdge";
import type { NetNode } from "../bindings/NetNode";
import type { Bond } from "../bindings/Bond";
import type { BondChange } from "../bindings/BondChange";
import type { BondSummary } from "../bindings/BondSummary";
import type { RelationType } from "../bindings/RelationType";
import { NODE_KIND } from "../lib/labels";
import { forceLayout, type Pt } from "../lib/forceLayout";
import { timelineLabel, timelineTree } from "../lib/timelines";
import { useRenderTiming } from "../lib/renderTiming";
import { Cover, useCovers } from "../components/Covers";

const KIND_COLOR: Record<string, string> = { character: "#ff2d55", faction: "#007aff", race: "#af52de", party: "#ff9500" };

/** 好感决定颜色：敌意强时偏红，好感高时偏绿。 */
function bondColor(b: Bond | null | undefined): string {
  if (!b) return "#8e8e93";
  const score = b.affection - b.hostility;
  if (score >= 20) return "#34c759";
  if (score <= -20) return "#ff3b30";
  return "#ffcc00";
}

const bondText = (b: Bond | null | undefined) => (b ? `强度 ${b.strength} · 信任 ${b.trust} · 好感 ${b.affection} · 敌意 ${b.hostility}` : "（没有看法记录）");

export default function NetworkPage({
  ctx,
  focus,
  onOpenTree,
  onOpenGraph,
  lockedCampaign,
}: {
  ctx: Ctx;
  focus?: string;
  onOpenTree?: (id: string) => void;
  onOpenGraph?: (id: string) => void;
  /** 嵌在游戏空间里：固定显示这局游戏的关系，不显示战役选择和编辑器入口。 */
  lockedCampaign?: string;
}) {
  const [campaigns, setCampaigns] = useState<Campaign[]>([]);
  const [campaign, setCampaign] = useState<string>(lockedCampaign ?? "");
  const [search, setSearch] = useState("");
  const [applied, setApplied] = useState("");
  const [relTypes, setRelTypes] = useState<RelationType[]>([]);
  const [typeFilter, setTypeFilter] = useState("");
  const [minStrength, setMinStrength] = useState(0);
  const [includeWorld, setIncludeWorld] = useState(true);
  const [net, setNet] = useState<Network | null>(null);
  /** 一次最多显示的人物数，可以逐步加大（后端上限 3000）。 */
  const [limit, setLimit] = useState(400);
  const [pos, setPos] = useState<Map<string, Pt>>(new Map());
  /** 用户拖动过的位置（不触发重新适配视图；重新布局时清空）。 */
  const [dragged, setDragged] = useState<Map<string, Pt>>(new Map());
  useEffect(() => setDragged(new Map()), [pos]);
  const [sel, setSel] = useState<{ type: "node"; id: string } | { type: "edge"; id: string } | null>(focus ? { type: "node", id: focus } : null);
  const flow = useRef<ReactFlowInstance | null>(null);
  // 数据或布局变化后重新适配视图（例如搜索后只剩几个节点时放大显示）。
  useEffect(() => {
    if (pos.size) requestAnimationFrame(() => flow.current?.fitView({ padding: 0.2, maxZoom: 1.5, duration: 200 }));
  }, [pos]);
  const timing = useRenderTiming("人物关系网络", net, () => [net?.nodes.length ?? 0, net?.edges.length ?? 0]);

  useEffect(() => {
    api.listCampaigns(ctx.worldId).then((cs) => {
      setCampaigns(cs);
      // 默认选最近的进行中战役。
      const active = [...cs].reverse().find((c) => c.status === "active" && !c.archived);
      setCampaign((cur) => lockedCampaign || cur || active?.id || "");
    }).catch(() => {});
    api.relationTypes().then(setRelTypes).catch(() => {});
  }, [ctx.worldId]);

  // spoilers 由程序按全局剧透模式填写，这里传的值会被覆盖。
  useEffect(() => {
    let live = true; // 条件变了之后，旧请求晚到的结果不再覆盖新的
    api
      .relationNetwork(ctx.worldId, { campaign_id: campaign || null, search: applied, rel_types: typeFilter ? [typeFilter] : [], include_world: includeWorld, min_strength: minStrength, limit, spoilers: false })
      .then((n) => {
        if (!live) return;
        timing.mark();
        setNet(n);
        setPos(forceLayout(n.nodes.map((x) => x.id), n.edges.map((e) => [e.a, e.b])));
      })
      .catch((e: Error) => live && ctx.notify(e.message, "error"));
    return () => {
      live = false;
    };
  }, [ctx.worldId, campaign, applied, typeFilter, minStrength, includeWorld, ctx.tick, limit]);

  const typeLabel = (t: string) => relTypes.find((x) => x.id === t)?.label ?? t;

  const covers = useCovers(ctx.worldId, (net?.nodes ?? []).filter((n) => n.kind !== "party").map((n) => n.id), ctx.tick);

  const { nodes, edges } = useMemo(() => {
    if (!net) return { nodes: [] as FlowNode[], edges: [] as Edge[] };
    const nodes: FlowNode[] = net.nodes.map((n) => ({
      id: n.id,
      position: dragged.get(n.id) ?? pos.get(n.id) ?? { x: 0, y: 0 },
      // 小地图只画有尺寸的节点：受控模式下量到的尺寸不会写回这里，先给一个大致尺寸。
      initialWidth: 160,
      initialHeight: 64,
      data: {
        label: covers[n.id] ? (
          <span className="gnode-pic">
            <Cover src={covers[n.id]} size={56} round />
            <span>{`${n.title}${n.status ? `（${n.status}）` : ""}\n${NODE_KIND[n.kind] ?? n.kind}`}</span>
          </span>
        ) : (
          `${n.title}${n.status ? `（${n.status}）` : ""}\n${n.kind === "party" ? "玩家" : NODE_KIND[n.kind] ?? n.kind}`
        ),
      },
      className: `gnode${sel?.type === "node" && sel.id === n.id ? " center" : ""}${n.status === "dead" ? " dead" : ""}${covers[n.id] ? " has-pic" : ""}`,
      style: { borderColor: KIND_COLOR[n.kind] ?? "#8e8e93" },
    }));
    const edges: Edge[] = net.edges.map((e) => {
      const campaignEdge = e.source === "campaign";
      const color = campaignEdge ? bondColor(e.a_to_b ?? e.b_to_a) : e.inactive ? "#c7c7cc" : "#8e8e93";
      const strength = Math.max(e.a_to_b?.strength ?? 0, e.b_to_a?.strength ?? 0);
      return {
        id: e.id,
        source: e.a,
        target: e.b,
        type: "straight",
        label: `${typeLabel(e.rel_type) || (campaignEdge ? "关系" : "")}${e.inactive ? "（已不成立）" : ""}`,
        style: { stroke: color, strokeWidth: campaignEdge ? 1.5 + strength / 25 : 1, strokeDasharray: e.inactive ? "4 4" : campaignEdge ? undefined : "2 3" },
        labelStyle: { fill: color, fontSize: 11 },
        markerEnd: e.directed ? { type: "arrowclosed" as never, color } : undefined,
      };
    });
    return { nodes, edges };
  }, [net, pos, dragged, sel, relTypes, covers]);

  const selNode = sel?.type === "node" ? net?.nodes.find((n) => n.id === sel.id) : undefined;
  const selEdge = sel?.type === "edge" ? net?.edges.find((e) => e.id === sel.id) : undefined;
  const title = (id: string) => net?.nodes.find((n) => n.id === id)?.title ?? id;

  return (
    <section className="network-page">
      <h2>人物关系网络</h2>
      <SpoilerHint ctx={ctx} what="人物关系" />
      <div className="row wrap">
        <select value={campaign} onChange={(e) => setCampaign(e.target.value)} disabled={!!lockedCampaign} hidden={!!lockedCampaign}>
          <option value="">只看世界设定中的关系</option>
          {timelineTree(campaigns, false).map((r) => (
            <option key={r.campaign.id} value={r.campaign.id}>
              {timelineLabel(r)}
            </option>
          ))}
        </select>
        <form className="row" onSubmit={(e) => { e.preventDefault(); setApplied(search); }}>
          <input value={search} onChange={(e) => setSearch(e.target.value)} placeholder="搜索人物或势力" />
          <button type="submit">搜索</button>
          {applied && <button type="button" className="link" onClick={() => { setSearch(""); setApplied(""); }}>清除</button>}
        </form>
        <select value={typeFilter} onChange={(e) => setTypeFilter(e.target.value)}>
          <option value="">全部关系类型</option>
          {relTypes.filter((t) => t.group === "person" || t.group === "faction").map((t) => (
            <option key={t.id} value={t.id}>{t.label}</option>
          ))}
        </select>
        <label className="check">
          强度至少
          <input type="number" min={0} max={100} value={minStrength} onChange={(e) => setMinStrength(Number(e.target.value))} style={{ width: 64 }} />
        </label>
        <label className="check">
          <input type="checkbox" checked={includeWorld} onChange={(e) => setIncludeWorld(e.target.checked)} />
          包含世界设定中的关系
        </label>
        <button onClick={() => net && setPos(forceLayout(net.nodes.map((x) => x.id), net.edges.map((e) => [e.a, e.b])))}>自动布局</button>
        {net && <small className="hint">{net.nodes.length} 个节点 · {net.edges.length} 条关系{net.truncated && " · 已达上限"}</small>}
        {net?.truncated && limit < 3000 && (
          <button className="small" onClick={() => setLimit((l) => Math.min(3000, l + 400))}>
            加载更多（上限 {limit} → {Math.min(3000, limit + 400)}）
          </button>
        )}
      </div>
      <div className="network-body">
        <div className="graph big">
          <ReactFlow
            nodes={nodes}
            edges={edges}
            onInit={(i) => (flow.current = i)}
            fitView
            minZoom={0.05}
            nodesConnectable={false}
            onlyRenderVisibleElements={nodes.length > 300}
            onNodeClick={(_, n) => setSel({ type: "node", id: n.id })}
            onEdgeClick={(_, e) => setSel({ type: "edge", id: e.id })}
            onNodeDragStop={(_, n) => setDragged((p) => new Map(p).set(n.id, n.position))}
            proOptions={{ hideAttribution: true }}
          >
            <Background />
            <Controls />
            <MiniMap pannable zoomable nodeColor={(n) => (n.style?.borderColor as string) || "#8e8e93"} nodeStrokeWidth={3} maskColor="rgba(15, 23, 42, 0.08)" />
          </ReactFlow>
        </div>
        <div className="card side">
          {!sel && <p className="hint">点击人物查看档案，点击关系查看双方的看法与形成历史。实线是本战役中的关系（越粗越深，绿色友好、红色敌对），虚线是世界设定中的关系。可以拖动、缩放。</p>}
          {selNode && <PersonPanel ctx={ctx} node={selNode} campaign={campaign} onOpenTree={onOpenTree} onOpenGraph={onOpenGraph} />}
          {selEdge && <EdgePanel ctx={ctx} edge={selEdge} campaign={campaign} title={title} typeLabel={typeLabel} />}
        </div>
      </div>
    </section>
  );
}

function PersonPanel({ ctx, node, campaign, onOpenTree, onOpenGraph }: { ctx: Ctx; node: NetNode; campaign: string; onOpenTree?: (id: string) => void; onOpenGraph?: (id: string) => void }) {
  const [bonds, setBonds] = useState<BondSummary[]>([]);
  useEffect(() => {
    if (campaign) api.personBonds(ctx.worldId, campaign, node.id).then(setBonds).catch(() => setBonds([]));
    else setBonds([]);
  }, [ctx.worldId, campaign, node.id]);
  return (
    <div>
      <h3>{node.title}</h3>
      <p className="hint">{node.kind === "party" ? "玩家队伍" : NODE_KIND[node.kind] ?? node.kind}{node.status && ` · 状态：${node.status}`}</p>
      {node.summary && <p>{node.summary}</p>}
      {node.kind !== "party" && (
        <div className="row wrap">
          {onOpenTree && <button onClick={() => onOpenTree(node.id)}>在世界树中打开</button>}
          {onOpenGraph && <button onClick={() => onOpenGraph(node.id)}>在世界图谱中查看</button>}
        </div>
      )}
      {campaign && (
        <>
          <h4>本战役中的关系</h4>
          {bonds.length === 0 && <p className="hint">还没有记录。</p>}
          <ul className="list">
            {bonds.map((b) => (
              <li key={b.other}>
                <strong>{b.other_title}</strong>{b.kind && <span className="kind">{b.kind}</span>}
                <div className="hint">{node.title} 对 {b.other_title}：{bondText(b.outgoing)}</div>
                <div className="hint">{b.other_title} 对 {node.title}：{bondText(b.incoming)}</div>
              </li>
            ))}
          </ul>
        </>
      )}
    </div>
  );
}

function EdgePanel({ ctx, edge, campaign, title, typeLabel }: { ctx: Ctx; edge: NetEdge; campaign: string; title: (id: string) => string; typeLabel: (t: string) => string }) {
  const [hist, setHist] = useState<BondChange[]>([]);
  useEffect(() => {
    if (edge.source === "campaign" && campaign) api.bondHistory(ctx.worldId, campaign, edge.a, edge.b).then(setHist).catch(() => setHist([]));
    else setHist([]);
  }, [ctx.worldId, campaign, edge.id]);
  const [a, b] = [title(edge.a), title(edge.b)];
  if (edge.source === "world") {
    return (
      <div>
        <h3>{a} — {b}</h3>
        <p>世界设定中的关系：<strong>{typeLabel(edge.rel_type)}</strong>{edge.directed && "（单向）"}{edge.label && `，${edge.label}`}</p>
        {edge.inactive && <p className="warn-text">在本战役中这层关系已经不成立（世界设定本身没有改变）。</p>}
        <p className="hint">世界设定中的关系在世界树里编辑；它的形成原因写在关系说明里。</p>
      </div>
    );
  }
  return (
    <div>
      <h3>{a} ⇄ {b}</h3>
      <p><strong>{a} 对 {b}</strong>{edge.kind_a_to_b && `（${typeLabel(edge.kind_a_to_b)}）`}：{bondText(edge.a_to_b)}</p>
      <p><strong>{b} 对 {a}</strong>{edge.kind_b_to_a && `（${typeLabel(edge.kind_b_to_a)}）`}：{bondText(edge.b_to_a)}</p>
      <h4>形成与变化（{hist.length}）</h4>
      <ul className="list bond-history">
        {hist.map((h, i) => (
          <li key={i}>
            <span className="kind">第 {h.turn + 1} 回合</span>
            {title(h.from)} 对 {title(h.to)}：{h.reason}
            <div className="hint">
              信任 {h.old.trust}→{h.new.trust}，好感 {h.old.affection}→{h.new.affection}，敌意 {h.old.hostility}→{h.new.hostility}，强度 {h.old.strength}→{h.new.strength}
              {h.kind !== h.old_kind && `，关系类型变为「${typeLabel(h.kind)}」`}
            </div>
            {h.player_action && <div className="hint">当时玩家：{h.player_action}</div>}
          </li>
        ))}
      </ul>
    </div>
  );
}
