import { useCallback, useEffect, useMemo, useState } from "react";
import { ReactFlow, Background, Controls, MiniMap, type Edge as FlowEdge, type Node as FlowNode, type Connection } from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { api, type CampaignStatus, type PlotGraph, type ScriptView } from "../api";
import type { Ctx } from "../App";
import { useRenderTiming } from "../lib/renderTiming";
import type { PlotNode } from "../bindings/PlotNode";
import type { PlotEdge } from "../bindings/PlotEdge";
import type { PlotNodeKind } from "../bindings/PlotNodeKind";
import type { PlotReport } from "../bindings/PlotReport";
import type { SimState } from "../bindings/SimState";
import type { EdgeOption } from "../bindings/EdgeOption";
import type { CheckMode } from "../bindings/CheckMode";
import type { Campaign } from "../bindings/Campaign";
import type { Condition } from "../bindings/Condition";
import type { Effect } from "../bindings/Effect";
import { ConditionEditor, EffectsEditor, type Choices } from "../components/DslEditors";
import { defaultCondition, describeCondition, describeEffect, layeredLayout, type Names } from "../lib/dsl";

const KIND: Record<PlotNodeKind, string> = { start: "开场", scene: "场景", choice: "选择", event: "事件", ending: "结局", dynamic: "自由行动" };
const TRI: Record<string, string> = { true: "可走", false: "不可走", unknown: "需检定" };

type Sel = { type: "node"; id: string } | { type: "edge"; id: string } | null;
type Tab = "edit" | "analysis" | "sim" | "play" | "ai";

export default function PlotEditor({ ctx, scriptId }: { ctx: Ctx; scriptId: string }) {
  const [graph, setGraph] = useState<PlotGraph | null>(null);
  const [script, setScript] = useState<ScriptView | null>(null);
  const [sel, setSel] = useState<Sel>(null);
  const [tab, setTab] = useState<Tab>("edit");
  const [report, setReport] = useState<PlotReport | null>(null);
  const timing = useRenderTiming("剧情图", graph, () => [graph?.nodes.length ?? 0, graph?.edges.length ?? 0]);
  const [highlight, setHighlight] = useState<string | null>(null);
  const [newKind, setNewKind] = useState<PlotNodeKind>("scene");
  const [newTitle, setNewTitle] = useState("");

  const load = useCallback(async () => {
    try {
      const [g, s] = await Promise.all([api.plotGraph(ctx.worldId, scriptId), api.getScript(ctx.worldId, scriptId)]);
      timing.mark();
      setGraph(g);
      setScript(s);
      setReport(await api.analyzePlot(ctx.worldId, scriptId));
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }, [ctx.worldId, scriptId, ctx.tick]);

  useEffect(() => {
    void load();
  }, [load]);

  const choices: Choices = useMemo(() => {
    const items = script?.items ?? [];
    const by = (k: string) => items.filter((i) => i.kind === k).map((i) => ({ id: i.id, label: i.title }));
    return {
      quests: by("quest"),
      clues: by("clue"),
      items: by("item"),
      plotNodes: (graph?.nodes ?? []).map((n) => ({ id: n.id, label: `${KIND[n.kind]}·${n.title}` })),
      people: (script?.cast ?? []).map((c) => ({ id: c.cast.node_id, label: c.name })),
    };
  }, [script, graph]);

  const names: Names = useCallback(
    (id: string) => {
      const all = [...choices.quests, ...choices.clues, ...choices.items, ...choices.plotNodes, ...choices.people];
      return all.find((o) => o.id === id)?.label ?? id;
    },
    [choices],
  );

  const { nodes, edges } = useMemo(() => {
    if (!graph) return { nodes: [] as FlowNode[], edges: [] as FlowEdge[] };
    const start = graph.nodes.find((n) => n.kind === "start")?.id ?? null;
    const auto = layeredLayout(
      graph.nodes.map((n) => n.id),
      start,
      graph.edges.map((e) => [e.from_id, e.to_id]),
    );
    const flagged = new Set(report?.issues.flatMap((i) => i.node_ids) ?? []);
    const nodes: FlowNode[] = graph.nodes.map((n) => {
      const p = n.payload as { x?: number; y?: number } | null;
      return {
        id: n.id,
        position: typeof p?.x === "number" && typeof p?.y === "number" ? { x: p.x, y: p.y } : (auto.get(n.id) ?? { x: 0, y: 0 }),
        data: { label: `${KIND[n.kind]}\n${n.title}` },
        className: `pnode pk-${n.kind}${sel?.type === "node" && sel.id === n.id ? " sel" : ""}${highlight === n.id ? " here" : ""}${flagged.has(n.id) ? " flagged" : ""}`,
      };
    });
    const edges: FlowEdge[] = graph.edges.map((e) => ({
      id: e.id,
      source: e.from_id,
      target: e.to_id,
      label: `${e.condition ? "🔒 " : ""}${e.label}${e.effects.length ? ` ⚡${e.effects.length}` : ""}`,
      markerEnd: { type: "arrowclosed" as never },
      className: sel?.type === "edge" && sel.id === e.id ? "pedge sel" : "pedge",
      animated: e.once,
    }));
    return { nodes, edges };
  }, [graph, report, sel, highlight]);

  async function onConnect(c: Connection) {
    if (!c.source || !c.target) return;
    try {
      const e = await api.createPlotEdge(ctx.worldId, {
        script_id: scriptId,
        from_id: c.source,
        to_id: c.target,
        label: "新分支",
        condition: null,
        effects: [],
        priority: 0,
        once: false,
      });
      ctx.bump();
      setSel({ type: "edge", id: e.id });
      setTab("edit");
    } catch (er) {
      ctx.notify((er as Error).message, "error");
    }
  }

  async function onDragStop(n: FlowNode) {
    const node = graph?.nodes.find((x) => x.id === n.id);
    if (!node) return;
    const payload = { ...(node.payload as object), x: Math.round(n.position.x), y: Math.round(n.position.y) };
    try {
      await api.updatePlotNode(ctx.worldId, node.id, { payload: payload as never }, node.rev);
      ctx.bump();
    } catch (er) {
      ctx.notify((er as Error).message, "error");
    }
  }

  async function addNode() {
    if (!newTitle.trim()) return;
    try {
      const n = await api.createPlotNode(ctx.worldId, {
        script_id: scriptId,
        kind: newKind,
        title: newTitle,
        player_text: "",
        gm_text: "",
        scene_ref: null,
        script_item_ref: null,
        payload: {},
      });
      setNewTitle("");
      ctx.bump();
      setSel({ type: "node", id: n.id });
      setTab("edit");
    } catch (er) {
      ctx.notify((er as Error).message, "error");
    }
  }

  async function generateFromScript() {
    try {
      await api.startGeneration(ctx.worldId, { task: "plot_from_script", script_id: scriptId, guidance: "" });
      ctx.notify("已开始根据剧本生成剧情分支图，完成后到审阅台确认");
    } catch (er) {
      ctx.notify((er as Error).message, "error");
    }
  }

  if (!graph || !script) return <p className="hint">加载中…</p>;
  const selNode = sel?.type === "node" ? graph.nodes.find((n) => n.id === sel.id) : undefined;
  const selEdge = sel?.type === "edge" ? graph.edges.find((e) => e.id === sel.id) : undefined;

  return (
    <section className="plot">
      <header className="editor-head">
        <div>
          <button className="link" onClick={() => ctx.go({ name: "script", id: scriptId })}>
            ← 返回剧本
          </button>
          <h2>剧情分支图 · {script.script.title}</h2>
          {report && (
            <small className="hint">
              {report.node_count} 个节点 · {report.edge_count} 条分支 · {report.ending_count} 个结局（可到达 {report.reachable_endings}）
              {report.issues.length > 0 && ` · ${report.issues.length} 个提示`}
            </small>
          )}
        </div>
        <div className="row">
          <select value={newKind} onChange={(e) => setNewKind(e.target.value as PlotNodeKind)}>
            {Object.entries(KIND).map(([k, v]) => (
              <option key={k} value={k}>
                {v}
              </option>
            ))}
          </select>
          <input value={newTitle} onChange={(e) => setNewTitle(e.target.value)} placeholder="新节点标题" />
          <button onClick={() => void addNode()} disabled={!newTitle.trim()}>
            新增节点
          </button>
          <button className="primary" onClick={() => void generateFromScript()}>
            AI 根据剧本生成
          </button>
        </div>
      </header>
      <div className="plot-body">
        <div className="graph plot-graph">
          {graph.nodes.length === 0 ? (
            <p className="hint" style={{ padding: 16 }}>
              还没有剧情节点。可以手动新增（先建一个"开场"），或让 AI 根据剧本生成。从一个节点的边缘拖到另一个节点即可连出分支。
            </p>
          ) : (
            <ReactFlow
              nodes={nodes}
              edges={edges}
              fitView
              onConnect={(c) => void onConnect(c)}
              onNodeClick={(_, n) => {
                setSel({ type: "node", id: n.id });
                if (tab !== "sim" && tab !== "play") setTab("edit");
              }}
              onEdgeClick={(_, e) => {
                setSel({ type: "edge", id: e.id });
                setTab("edit");
              }}
              onNodeDragStop={(_, n) => void onDragStop(n)}
              proOptions={{ hideAttribution: true }}
            >
              <Background />
              <Controls />
              <MiniMap pannable zoomable />
            </ReactFlow>
          )}
        </div>
        <div className="detail">
          <div className="tabs">
            {(
              [
                ["edit", "编辑"],
                ["analysis", `检查${report && report.issues.length ? `（${report.issues.length}）` : ""}`],
                ["sim", "模拟"],
                ["play", "试玩"],
                ["ai", "AI 分支"],
              ] as [Tab, string][]
            ).map(([k, v]) => (
              <button key={k} className={tab === k ? "active" : ""} onClick={() => setTab(k)}>
                {v}
              </button>
            ))}
          </div>
          {tab === "edit" && selNode && <NodeForm key={selNode.id + selNode.rev} ctx={ctx} node={selNode} names={names} />}
          {tab === "edit" && selEdge && <EdgeForm key={selEdge.id + selEdge.rev} ctx={ctx} edge={selEdge} choices={choices} names={names} />}
          {tab === "edit" && !selNode && !selEdge && <p className="hint">点击图中的节点或分支进行编辑。</p>}
          {tab === "analysis" && report && <Analysis ctx={ctx} scriptId={scriptId} report={report} onSelect={(id) => setSel({ type: "node", id })} />}
          {tab === "sim" && <SimPanel ctx={ctx} scriptId={scriptId} names={names} onHere={setHighlight} />}
          {tab === "play" && <PlayPanel ctx={ctx} scriptId={scriptId} names={names} onHere={setHighlight} />}
          {tab === "ai" && <AiPanel ctx={ctx} scriptId={scriptId} node={selNode} />}
        </div>
      </div>
    </section>
  );
}

function NodeForm({ ctx, node, names }: { ctx: Ctx; node: PlotNode; names: Names }) {
  const [title, setTitle] = useState(node.title);
  const [kind, setKind] = useState<PlotNodeKind>(node.kind);
  const [player, setPlayer] = useState(node.player_text);
  const [gm, setGm] = useState(node.gm_text);
  const ro = node.locked;
  async function save() {
    try {
      await api.updatePlotNode(ctx.worldId, node.id, { title, kind, player_text: player, gm_text: gm }, node.rev);
      ctx.notify("已保存");
      ctx.bump();
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }
  async function remove() {
    if (!confirm(`删除剧情节点「${node.title}」及与它相连的分支？（可在历史中撤销）`)) return;
    try {
      await api.deletePlotNode(ctx.worldId, node.id, node.rev);
      ctx.bump();
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }
  return (
    <div className="card form">
      <h3>剧情节点{ro && " · 已锁定"}</h3>
      <div className="row">
        <label>
          标题
          <input value={title} disabled={ro} onChange={(e) => setTitle(e.target.value)} />
        </label>
        <label>
          类型
          <select value={kind} disabled={ro} onChange={(e) => setKind(e.target.value as PlotNodeKind)}>
            {Object.entries(KIND).map(([k, v]) => (
              <option key={k} value={k}>
                {v}
              </option>
            ))}
          </select>
        </label>
      </div>
      <label>
        玩家可见
        <textarea rows={4} value={player} disabled={ro} onChange={(e) => setPlayer(e.target.value)} />
      </label>
      <label className="gm-label">
        仅主持人
        <textarea rows={4} value={gm} disabled={ro} onChange={(e) => setGm(e.target.value)} />
      </label>
      {node.script_item_ref && <p className="hint">对应剧本条目：{names(node.script_item_ref)}</p>}
      <div className="row">
        <button className="primary" disabled={ro} onClick={() => void save()}>
          保存
        </button>
        <button className="danger" disabled={ro} onClick={() => void remove()}>
          删除
        </button>
      </div>
      <small className="hint">来源：{node.source.startsWith("ai:") ? "AI 生成（已确认）" : "手动"} · 版本 {node.rev}</small>
    </div>
  );
}

function EdgeForm({ ctx, edge, choices, names }: { ctx: Ctx; edge: PlotEdge; choices: Choices; names: Names }) {
  const [label, setLabel] = useState(edge.label);
  const [cond, setCond] = useState<Condition | null>(edge.condition);
  const [effects, setEffects] = useState<Effect[]>(edge.effects);
  const [priority, setPriority] = useState(edge.priority);
  const [once, setOnce] = useState(edge.once);
  async function save() {
    try {
      await api.updatePlotEdge(ctx.worldId, edge.id, { label, condition: cond, effects, priority, once }, edge.rev);
      ctx.notify("已保存");
      ctx.bump();
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }
  async function remove() {
    if (!confirm("删除这条分支？")) return;
    try {
      await api.deletePlotEdge(ctx.worldId, edge.id, edge.rev);
      ctx.bump();
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }
  return (
    <div className="card form">
      <h3>
        分支：{names(edge.from_id)} → {names(edge.to_id)}
      </h3>
      <label>
        玩家看到的选择 / 触发描述
        <input value={label} onChange={(e) => setLabel(e.target.value)} />
      </label>
      <div className="row">
        <label>
          优先级（越大越靠前）
          <input type="number" value={priority} onChange={(e) => setPriority(Number(e.target.value))} />
        </label>
        <label className="check">
          <input type="checkbox" checked={once} onChange={(e) => setOnce(e.target.checked)} />
          只能走一次
        </label>
      </div>
      <h3>条件</h3>
      <label className="check">
        <input type="checkbox" checked={cond !== null} onChange={(e) => setCond(e.target.checked ? defaultCondition("all") : null)} />
        需要满足条件才能走
      </label>
      {cond && <ConditionEditor value={cond} onChange={setCond} choices={choices} />}
      <p className="hint">读作：{describeCondition(cond, names)}</p>
      <h3>效果（按顺序执行）</h3>
      <EffectsEditor value={effects} onChange={setEffects} choices={choices} />
      {effects.length > 0 && <p className="hint">读作：{effects.map((e) => describeEffect(e, names)).join("；")}</p>}
      <div className="row">
        <button className="primary" onClick={() => void save()}>
          保存
        </button>
        <button className="danger" onClick={() => void remove()}>
          删除
        </button>
      </div>
      <small className="hint">条件由程序求值；技能检定由程序掷骰。版本 {edge.rev}</small>
    </div>
  );
}

function Analysis({ ctx, scriptId, report, onSelect }: { ctx: Ctx; scriptId: string; report: PlotReport; onSelect: (id: string) => void }) {
  const deadEnds = report.issues.filter((i) => i.code === "dead_end").length;
  const [busy, setBusy] = useState(false);
  async function fill() {
    setBusy(true);
    try {
      const ids = await api.fillDeadEnds(ctx.worldId, scriptId);
      ctx.notify(`已为 ${ids.length} 个死路节点排队生成后续分支，依次执行。完成后在审阅台确认写入`);
      ctx.bump();
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    } finally {
      setBusy(false);
    }
  }
  return (
    <div className="card">
      <h3>剧情图检查</h3>
      {deadEnds > 0 && (
        <p className="row">
          <button className="primary" disabled={busy} onClick={() => void fill()}>
            AI 为全部死路生成后续（{Math.min(deadEnds, 10)}）
          </button>
          <span className="hint">每个死路节点生成 2 条后续分支，只新增、不改动已有内容；一次最多 10 个。</span>
        </p>
      )}
      {report.issues.length === 0 && <p className="ok-text">没有发现问题。</p>}
      <ul className="issues">
        {report.issues.map((i, k) => (
          <li key={k} className={i.severity}>
            {i.message}{" "}
            {i.node_ids.slice(0, 1).map((id) => (
              <button key={id} className="link" onClick={() => onSelect(id)}>
                定位
              </button>
            ))}
          </li>
        ))}
      </ul>
      <p className="hint">检查：开场、可达性、死路、结局、条件与效果中的引用、无条件循环。</p>
    </div>
  );
}

function OptionList({ options, names, onTake }: { options: EdgeOption[]; names: Names; onTake: (o: EdgeOption, mode?: CheckMode) => void }) {
  if (options.length === 0) return <p className="hint">当前节点没有出边。</p>;
  return (
    <ul className="options">
      {options.map((o) => (
        <li key={o.edge.id} className={`opt tri-${o.availability}`}>
          <div>
            <strong>{o.edge.label || names(o.edge.to_id)}</strong> <span className="badge">{o.used_up ? "已走过" : TRI[o.availability]}</span>
            <div className="hint">
              → {names(o.edge.to_id)} · {describeCondition(o.edge.condition, names)}
            </div>
          </div>
          <div className="row">
            {o.availability === "unknown" ? (
              <>
                <button onClick={() => onTake(o, "roll")}>掷骰</button>
                <button onClick={() => onTake(o, "pass")}>当作成功</button>
                <button onClick={() => onTake(o, "fail")}>当作失败</button>
              </>
            ) : (
              <button disabled={o.availability === "false"} onClick={() => onTake(o, "roll")}>
                走这条
              </button>
            )}
          </div>
        </li>
      ))}
    </ul>
  );
}

function StateSummary({ s, names }: { s: { vars: Record<string, unknown>; quests: Record<string, string>; items: Record<string, number>; clues: string[] | Set<string> }; names: Names }) {
  const clues = Array.isArray(s.clues) ? s.clues : [...s.clues];
  return (
    <div className="state">
      <div>
        <strong>任务</strong>：{Object.entries(s.quests).map(([q, st]) => `${names(q)}=${st}`).join("，") || "—"}
      </div>
      <div>
        <strong>线索</strong>：{clues.map(names).join("，") || "—"}
      </div>
      <div>
        <strong>物品</strong>：{Object.entries(s.items).filter(([, n]) => n > 0).map(([k, n]) => `${names(k.split("|")[1] ?? k)}×${n}`).join("，") || "—"}
      </div>
      <div>
        <strong>变量</strong>：{Object.entries(s.vars).map(([k, v]) => `${k}=${JSON.stringify(v)}`).join("，") || "—"}
      </div>
    </div>
  );
}

function SimPanel({ ctx, scriptId, names, onHere }: { ctx: Ctx; scriptId: string; names: Names; onHere: (id: string | null) => void }) {
  const [sim, setSim] = useState<SimState | null>(null);
  const [opts, setOpts] = useState<EdgeOption[]>([]);
  const [raw, setRaw] = useState("");
  const [seed, setSeed] = useState(1);

  async function refresh(s: SimState) {
    setSim(s);
    setRaw(JSON.stringify({ vars: s.vars, quests: s.quests, items: s.items, clues: s.clues, relations: s.relations }, null, 2));
    onHere(s.current);
    setOpts(await api.simOptions(ctx.worldId, s));
  }
  async function reset() {
    try {
      await refresh(await api.simStart(ctx.worldId, scriptId));
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }
  useEffect(() => {
    void reset();
    return () => onHere(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ctx.tick]);

  async function take(o: EdgeOption, mode: CheckMode = "roll") {
    if (!sim) return;
    try {
      const r = await api.simStep(ctx.worldId, sim, o.edge.id, mode, seed);
      if (r.checks.length) ctx.notify(r.checks.map((c) => `${c.skill} 检定：${c.roll.total}+${c.modifier}=${c.total} / DC ${c.dc} ${c.success ? "成功" : "失败"}`).join("；"));
      await refresh(r.state);
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }
  async function applyRaw() {
    if (!sim) return;
    try {
      const patch = JSON.parse(raw);
      await refresh({ ...sim, ...patch });
    } catch {
      ctx.notify("状态不是有效的 JSON", "error");
    }
  }

  return (
    <div className="card">
      <h3>模拟（不写入任何数据）</h3>
      <div className="row">
        <button onClick={() => void reset()}>从开场重新开始</button>
        <label className="check">
          随机种子
          <input type="number" value={seed} onChange={(e) => setSeed(Number(e.target.value))} style={{ width: 80 }} />
        </label>
      </div>
      {sim && (
        <>
          <p>
            当前：<strong>{sim.current ? names(sim.current) : "（未确定开场）"}</strong>
          </p>
          <OptionList options={opts} names={names} onTake={(o, m) => void take(o, m)} />
          <StateSummary s={sim} names={names} />
          <details>
            <summary>手动修改模拟状态（JSON）</summary>
            <textarea className="mono" rows={8} value={raw} onChange={(e) => setRaw(e.target.value)} />
            <button onClick={() => void applyRaw()}>应用</button>
            <p className="hint">例如给队伍加钥匙：items 里加 "party|物品条目id": 1；设置标志：vars 里加 "flag.名称": true。</p>
          </details>
          {sim.history.length > 0 && (
            <ol className="history">
              {sim.history.map((h, i) => (
                <li key={i}>{h}</li>
              ))}
            </ol>
          )}
        </>
      )}
    </div>
  );
}

function PlayPanel({ ctx, scriptId, names, onHere }: { ctx: Ctx; scriptId: string; names: Names; onHere: (id: string | null) => void }) {
  const [list, setList] = useState<Campaign[]>([]);
  const [cid, setCid] = useState<string | null>(null);
  const [st, setSt] = useState<CampaignStatus | null>(null);
  const [name, setName] = useState("");

  useEffect(() => {
    api
      .listCampaigns(ctx.worldId)
      .then((l) => setList(l.filter((c) => c.script_id === scriptId)))
      .catch(() => {});
  }, [ctx.worldId, scriptId, ctx.tick]);

  useEffect(() => {
    if (!cid) return;
    api
      .campaignStatus(ctx.worldId, cid)
      .then((s) => {
        setSt(s);
        onHere(s.current?.id ?? null);
      })
      .catch((e: Error) => ctx.notify(e.message, "error"));
    return () => onHere(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cid, ctx.tick]);

  async function create() {
    try {
      const c = await api.createCampaign(ctx.worldId, scriptId, name || `试玩 ${list.length + 1}`);
      setName("");
      setCid(c.id);
      ctx.bump();
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }
  async function take(o: EdgeOption) {
    if (!cid) return;
    try {
      const r = await api.campaignTakeEdge(ctx.worldId, cid, o.edge.id);
      const checks = r.checks.map((c) => `${c.skill} ${c.roll.total}+${c.modifier}=${c.total}/DC${c.dc} ${c.success ? "成功" : "失败"}`).join("；");
      ctx.notify(r.taken ? `已推进${checks ? `（${checks}）` : ""}` : `没有通过：${checks || "条件不满足"}`, r.taken ? "info" : "error");
      ctx.bump();
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }

  return (
    <div className="card">
      <h3>试玩（真实战役：状态会保存，但不改动世界设定）</h3>
      <div className="row">
        <select value={cid ?? ""} onChange={(e) => setCid(e.target.value || null)}>
          <option value="">选择战役</option>
          {list.map((c) => (
            <option key={c.id} value={c.id}>
              {c.name}（回合 {c.turn}）
            </option>
          ))}
        </select>
        <input value={name} onChange={(e) => setName(e.target.value)} placeholder="新战役名称" />
        <button onClick={() => void create()}>新建战役</button>
      </div>
      {st && (
        <>
          <p>
            回合 {st.campaign.turn} · 当前：<strong>{st.current?.title ?? "（剧本没有唯一开场）"}</strong>
          </p>
          {st.current?.player_text && <p className="player">{st.current.player_text}</p>}
          {st.current?.gm_text && <p className="gm">主持人：{st.current.gm_text}</p>}
          <OptionList options={st.options} names={names} onTake={(o) => void take(o)} />
          <StateSummary s={st.state} names={names} />
          <details>
            <summary>事件日志（{st.events.length}）</summary>
            <ol className="history">
              {st.events.map((e) => (
                <li key={e.seq}>
                  回合 {e.turn} · {e.kind}
                </li>
              ))}
            </ol>
          </details>
        </>
      )}
    </div>
  );
}

function AiPanel({ ctx, scriptId, node }: { ctx: Ctx; scriptId: string; node?: PlotNode }) {
  const [action, setAction] = useState("");
  const [count, setCount] = useState(2);
  async function go() {
    if (!node) return;
    try {
      await api.startGeneration(ctx.worldId, { task: "plot_branches", script_id: scriptId, from_node_id: node.id, player_action: action, count });
      ctx.notify("已开始生成新分支，完成后到审阅台确认");
      setAction("");
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }
  return (
    <div className="card form">
      <h3>AI 生成分支</h3>
      {!node ? (
        <p className="hint">先在图中选中一个节点。</p>
      ) : (
        <>
          <p>
            从「<strong>{node.title}</strong>」出发
          </p>
          <label>
            玩家的行动（可选：描述剧本没有预设的行为，AI 会据此生成后续）
            <textarea rows={3} value={action} onChange={(e) => setAction(e.target.value)} placeholder="例如：玩家想贿赂守卫，偷偷溜进仓库" />
          </label>
          {!action.trim() && (
            <label>
              补充几个选择
              <input type="number" min={1} max={6} value={count} onChange={(e) => setCount(Number(e.target.value))} />
            </label>
          )}
          <button className="primary" disabled={node.kind === "ending"} onClick={() => void go()}>
            生成
          </button>
          <p className="hint">只会新增节点与从当前节点出发的分支，不会修改已有内容；条件与效果格式不对的会被移除并在审阅台提示。</p>
        </>
      )}
    </div>
  );
}
