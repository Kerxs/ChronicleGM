import NodeImages from "../components/NodeImages";
import RefImages from "../components/RefImages";
import { Cover, useCovers } from "../components/Covers";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { CharacterStatsEditor, FactionPowerEditor, ItemStatsEditor, ScheduleEditor } from "../components/RuleEditors";
import { api, type NodeDetail, type TreeNode } from "../api";
import type { Ctx } from "../App";
import type { NodeKind } from "../bindings/NodeKind";
import type { CanonLevel } from "../bindings/CanonLevel";
import type { RelationType } from "../bindings/RelationType";
import type { SearchHit } from "../bindings/SearchHit";
import type { RewriteMode } from "../bindings/RewriteMode";
import { CANON, NODE_KIND, REWRITE_MODE } from "../lib/labels";
import { ROOT, visibleRows, windowRange } from "../lib/tree";
import RelationGraph from "./RelationGraph";

const ROW_H = 26;
const PAGE = 500;

export default function WorldTree({ ctx, focus }: { ctx: Ctx; focus?: string }) {
  const [cache, setCache] = useState<Map<string, TreeNode[]>>(new Map());
  const [totals, setTotals] = useState<Map<string, number>>(new Map());
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [selected, setSelected] = useState<string | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewport, setViewport] = useState(600);
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<SearchHit[] | null>(null);
  const [relTypes, setRelTypes] = useState<RelationType[]>([]);
  const [tab, setTab] = useState<"edit" | "img" | "rel" | "ai" | "graph">("edit");
  const scroller = useRef<HTMLDivElement>(null);

  const load = useCallback(
    async (parent: string, offset = 0) => {
      const kids = await api.treeChildren(ctx.worldId, parent === ROOT ? null : parent, offset, PAGE);
      setCache((c) => {
        const n = new Map(c);
        n.set(parent, offset === 0 ? kids : [...(c.get(parent) ?? []), ...kids]);
        return n;
      });
      return kids;
    },
    [ctx.worldId],
  );

  /** 重新加载所有已展开的层级（数据变化后调用）。 */
  const reload = useCallback(async () => {
    try {
      const parents = [ROOT, ...expanded];
      const fresh = new Map<string, TreeNode[]>();
      const tot = new Map<string, number>();
      for (const p of parents) {
        const kids = await api.treeChildren(ctx.worldId, p === ROOT ? null : p, 0, PAGE);
        fresh.set(p, kids);
      }
      for (const kids of fresh.values()) for (const k of kids) tot.set(k.node.id, k.child_count);
      tot.set(ROOT, fresh.get(ROOT)?.length ?? 0);
      setCache(fresh);
      setTotals(tot);
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }, [ctx.worldId, expanded]);

  useEffect(() => {
    void reload();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ctx.worldId, ctx.tick]);

  useEffect(() => {
    api.relationTypes().then(setRelTypes).catch(() => {});
    if (focus) void reveal(focus);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const el = scroller.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setViewport(el.clientHeight));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // 子节点总数取自父节点行上的计数。
  const totalsMerged = useMemo(() => {
    const m = new Map(totals);
    for (const kids of cache.values()) for (const k of kids) m.set(k.node.id, k.child_count);
    return m;
  }, [cache, totals]);

  const rows = useMemo(() => visibleRows(cache, totalsMerged, expanded), [cache, totalsMerged, expanded]);
  const [start, end] = windowRange(rows.length, scrollTop, viewport, ROW_H);
  const covers = useCovers(
    ctx.worldId,
    rows.slice(start, end).flatMap((r) => (r.type === "node" ? [r.item.node.id] : [])),
    ctx.tick,
  );

  async function toggle(item: TreeNode) {
    const id = item.node.id;
    const n = new Set(expanded);
    if (n.has(id)) n.delete(id);
    else {
      n.add(id);
      if (!cache.has(id)) await load(id).catch((e: Error) => ctx.notify(e.message, "error"));
    }
    setExpanded(n);
  }

  /** 展开祖先链并定位到节点。 */
  async function reveal(id: string) {
    try {
      const d = await api.nodeDetail(ctx.worldId, id);
      const n = new Set(expanded);
      for (const a of d.ancestors) {
        n.add(a.id);
        if (!cache.has(a.id)) await load(a.id);
      }
      setExpanded(n);
      setSelected(id);
      setTimeout(() => {
        const idx = visibleRows(cache, totalsMerged, n).findIndex((r) => r.type === "node" && r.item.node.id === id);
        if (idx >= 0 && scroller.current) scroller.current.scrollTop = Math.max(0, idx * ROW_H - viewport / 2);
      }, 50);
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }

  async function search() {
    if (!query.trim()) {
      setHits(null);
      return;
    }
    try {
      setHits(await api.searchNodes(ctx.worldId, query, { kinds: [], player_view: false, limit: 100, offset: 0 }));
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }

  async function dropOn(targetId: string | null, draggedId: string) {
    if (targetId === draggedId) return;
    try {
      const d = await api.nodeDetail(ctx.worldId, draggedId);
      await api.moveNode(ctx.worldId, draggedId, targetId, "last", d.node.rev);
      if (targetId) setExpanded((e) => new Set(e).add(targetId));
      ctx.bump();
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }

  const isEmpty = (cache.get(ROOT)?.length ?? 0) === 0;

  return (
    <section className="world">
      <div className="world-body">
        <div className="tree card">
          <form
            className="row"
            onSubmit={(e) => {
              e.preventDefault();
              void search();
            }}
          >
            <input value={query} onChange={(e) => setQuery(e.target.value)} placeholder="搜索名称、别名、描述…" style={{ flex: 1 }} />
            <button type="submit">搜索</button>
          </form>
          {hits && (
            <div className="hits">
              <div className="row">
                <small className="hint">找到 {hits.length} 个</small>
                <button className="link" onClick={() => setHits(null)}>
                  关闭
                </button>
              </div>
              {hits.map((h) => (
                <button key={h.node.id} className="hit" onClick={() => void reveal(h.node.id)}>
                  <span className="kind">{NODE_KIND[h.node.kind]}</span>
                  {h.node.title}
                  <small className="hint"> {h.node.summary.slice(0, 40)}</small>
                </button>
              ))}
            </div>
          )}
          <div className="row tree-tools">
            <NewNodeButton ctx={ctx} parent={selected} onCreated={(id) => void reveal(id)} />
          </div>
          <div
            className="vtree"
            ref={scroller}
            onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
            onDragOver={(e) => e.preventDefault()}
            onDrop={(e) => {
              if (e.target === e.currentTarget) void dropOn(null, e.dataTransfer.getData("text/node"));
            }}
          >
            <div style={{ height: rows.length * ROW_H, position: "relative" }}>
              {rows.slice(start, end).map((r, i) => {
                const top = (start + i) * ROW_H;
                const style = { top, paddingLeft: 6 + r.depth * 16, height: ROW_H };
                if (r.type === "loading")
                  return (
                    <div key={`l-${r.parent}`} className="vrow hint" style={style}>
                      加载中…
                    </div>
                  );
                if (r.type === "more")
                  return (
                    <div key={`m-${r.parent}`} className="vrow" style={style}>
                      <button className="link" onClick={() => void load(r.parent, r.loaded)}>
                        加载更多（已显示 {r.loaded} / {r.total}）
                      </button>
                    </div>
                  );
                const n = r.item.node;
                return (
                  <div
                    key={n.id}
                    className={`vrow ${n.id === selected ? "sel" : ""}`}
                    style={style}
                    draggable
                    onDragStart={(e) => e.dataTransfer.setData("text/node", n.id)}
                    onDragOver={(e) => e.preventDefault()}
                    onDrop={(e) => {
                      e.stopPropagation();
                      void dropOn(n.id, e.dataTransfer.getData("text/node"));
                    }}
                  >
                    <button className="twisty" onClick={() => void toggle(r.item)} disabled={r.item.child_count === 0}>
                      {r.item.child_count === 0 ? "·" : r.expanded ? "▾" : "▸"}
                    </button>
                    <button className="label" onClick={() => setSelected(n.id)} title={n.summary}>
                      <span className={`kind kk-${n.kind}`}>{NODE_KIND[n.kind]}</span>
                      <Cover src={covers[n.id]} size={22} round={n.kind === "character"} />
                      {n.title}
                      {n.canon_level === "core" && <span className="core" title="核心设定">★</span>}
                      {n.locked && <span title="已锁定"> 🔒</span>}
                      {r.item.child_count > 0 && <small className="hint"> {r.item.child_count}</small>}
                    </button>
                  </div>
                );
              })}
            </div>
          </div>
          <small className="hint">拖动节点到另一个节点上可移动；拖到空白处移到根级。共 {rows.length} 行可见。</small>
        </div>

        <div className="detail">
          {isEmpty && <SeedForm ctx={ctx} />}
          {selected ? (
            <>
              <div className="tabs">
                {(
                  [
                    ["edit", "编辑"],
                    ["img", "图片"],
                    ["rel", "关系"],
                    ["ai", "AI 辅助"],
                    ["graph", "关系图"],
                  ] as const
                ).map(([k, v]) => (
                  <button key={k} className={tab === k ? "active" : ""} onClick={() => setTab(k)}>
                    {v}
                  </button>
                ))}
              </div>
              <NodePanel key={selected + ctx.tick} ctx={ctx} id={selected} tab={tab} relTypes={relTypes} onSelect={(id) => void reveal(id)} onDeleted={() => setSelected(null)} />
            </>
          ) : (
            !isEmpty && <p className="hint">选择左侧的节点。</p>
          )}
        </div>
      </div>
    </section>
  );
}

function SeedForm({ ctx }: { ctx: Ctx }) {
  const [idea, setIdea] = useState("");
  const [genre, setGenre] = useState("");
  const [era, setEra] = useState("");
  const [style, setStyle] = useState("");
  const [rules, setRules] = useState("");
  const [refs, setRefs] = useState<string[]>([]);
  async function go() {
    try {
      await api.startGeneration(ctx.worldId, { task: "world_seed", idea, genre, era, style, rules }, refs);
      ctx.notify("已开始生成世界，完成后到审阅台确认");
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }
  return (
    <form
      className="card form"
      onSubmit={(e) => {
        e.preventDefault();
        void go();
      }}
    >
      <h3>一句话生成世界</h3>
      <label>
        创意 *
        <textarea rows={2} value={idea} onChange={(e) => setIdea(e.target.value)} placeholder="例如：一个被月潮支配的群岛，魔法随月相涨落" />
      </label>
      <div className="row">
        <label>
          题材
          <input value={genre} onChange={(e) => setGenre(e.target.value)} placeholder="奇幻 / 科幻 / 武侠…" />
        </label>
        <label>
          时代
          <input value={era} onChange={(e) => setEra(e.target.value)} placeholder="中世纪 / 蒸汽时代…" />
        </label>
        <label>
          风格
          <input value={style} onChange={(e) => setStyle(e.target.value)} placeholder="黑暗 / 史诗…" />
        </label>
      </div>
      <label>
        世界规则（可选）
        <input value={rules} onChange={(e) => setRules(e.target.value)} placeholder="例如：没有复活魔法；火药被禁止" />
      </label>
      <RefImages ctx={ctx} value={refs} onChange={setRefs} />
      <button className="primary" disabled={!idea.trim()}>
        生成世界树
      </button>
      <p className="hint">生成宇宙背景、历史、地理、势力、种族、人物、文化、科技魔法、事件传说等分类及其主要节点。确认写入后可以逐个节点继续展开。</p>
    </form>
  );
}

function NewNodeButton({ ctx, parent, onCreated }: { ctx: Ctx; parent: string | null; onCreated: (id: string) => void }) {
  const [kind, setKind] = useState<NodeKind>("category");
  const [title, setTitle] = useState("");
  async function add(under: string | null) {
    if (!title.trim()) return;
    try {
      const n = await api.createNode(ctx.worldId, {
        kind,
        parent_id: under,
        position: "last",
        title,
        aliases: [],
        summary: "",
        player_text: "",
        gm_text: "",
        payload: {},
        tags: [],
        canon_level: "draft",
        locked: false,
        status: "draft",
      });
      setTitle("");
      ctx.bump();
      onCreated(n.id);
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }
  return (
    <>
      <select value={kind} onChange={(e) => setKind(e.target.value as NodeKind)}>
        {Object.entries(NODE_KIND).map(([k, v]) => (
          <option key={k} value={k}>
            {v}
          </option>
        ))}
      </select>
      <input value={title} onChange={(e) => setTitle(e.target.value)} placeholder="新节点名称" style={{ flex: 1, minWidth: 0 }} />
      <button disabled={!title.trim() || !parent} onClick={() => void add(parent)} title="添加为选中节点的子节点">
        加为子节点
      </button>
      <button disabled={!title.trim()} onClick={() => void add(null)}>
        加到根级
      </button>
    </>
  );
}

function NodePanel({ ctx, id, tab, relTypes, onSelect, onDeleted }: { ctx: Ctx; id: string; tab: string; relTypes: RelationType[]; onSelect: (id: string) => void; onDeleted: () => void }) {
  const [d, setD] = useState<NodeDetail | null>(null);
  useEffect(() => {
    api.nodeDetail(ctx.worldId, id).then(setD).catch((e: Error) => ctx.notify(e.message, "error"));
  }, [ctx.worldId, id]);
  if (!d) return <p className="hint">加载中…</p>;
  return (
    <>
      <div className="crumbs hint">
        {d.ancestors.map((a) => (
          <button key={a.id} className="link" onClick={() => onSelect(a.id)}>
            {a.title} /
          </button>
        ))}
      </div>
      {tab === "edit" && <NodeEditor ctx={ctx} d={d} onDeleted={onDeleted} />}
      {tab === "img" && <NodeImages ctx={ctx} node={d.node} onChanged={() => {}} />}
      {tab === "rel" && <Relations ctx={ctx} d={d} relTypes={relTypes} onSelect={onSelect} />}
      {tab === "ai" && <NodeAi ctx={ctx} d={d} />}
      {tab === "graph" && <RelationGraph ctx={ctx} center={d.node.id} relTypes={relTypes} onSelect={onSelect} />}
    </>
  );
}

function NodeEditor({ ctx, d, onDeleted }: { ctx: Ctx; d: NodeDetail; onDeleted: () => void }) {
  const n = d.node;
  const [title, setTitle] = useState(n.title);
  const [aliases, setAliases] = useState(n.aliases.join("、"));
  const [summary, setSummary] = useState(n.summary);
  const [player, setPlayer] = useState(n.player_text);
  const [gm, setGm] = useState(n.gm_text);
  const [payload, setPayload] = useState(JSON.stringify(n.payload, null, 2));
  const [tags, setTags] = useState(n.tags.join("、"));
  const [canon, setCanon] = useState<CanonLevel>(n.canon_level);
  const split = (s: string) => s.split(/[、,，]/).map((x) => x.trim()).filter(Boolean);

  async function save() {
    let p: unknown;
    try {
      p = JSON.parse(payload || "{}");
    } catch {
      ctx.notify("结构化资料不是有效的 JSON", "error");
      return;
    }
    try {
      await api.updateNode(ctx.worldId, n.id, { title, aliases: split(aliases), summary, player_text: player, gm_text: gm, payload: p as never, tags: split(tags), canon_level: canon }, n.rev);
      ctx.notify("已保存");
      ctx.bump();
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }
  async function setLock(locked: boolean) {
    try {
      await api.updateNode(ctx.worldId, n.id, { locked }, n.rev);
      ctx.bump();
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }
  async function remove() {
    if (!confirm(`删除「${n.title}」及其全部 ${d.child_count} 个子节点？（进入回收站，可恢复或撤销）`)) return;
    try {
      await api.deleteNode(ctx.worldId, n.id, n.rev);
      onDeleted();
      ctx.bump();
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }
  async function reorder(dir: -1 | 1) {
    try {
      const sibs = await api.treeChildren(ctx.worldId, n.parent_id, 0, 1000);
      const i = sibs.findIndex((s) => s.node.id === n.id);
      const j = i + dir;
      if (j < 0 || j >= sibs.length) return;
      const ref = sibs[j].node.id;
      await api.moveNode(ctx.worldId, n.id, n.parent_id, dir < 0 ? { before: ref } : { after: ref }, n.rev);
      ctx.bump();
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }
  const ro = n.locked;
  return (
    <div className="card form">
      <h3>
        {NODE_KIND[n.kind]} · {CANON[n.canon_level]}
        {ro && " · 已锁定（解锁后才能编辑）"}
      </h3>
      <div className="row">
        <label>
          名称
          <input value={title} disabled={ro} onChange={(e) => setTitle(e.target.value)} />
        </label>
        <label>
          别名（用顿号分隔）
          <input value={aliases} disabled={ro} onChange={(e) => setAliases(e.target.value)} />
        </label>
        <label>
          正典等级
          <select value={canon} disabled={ro} onChange={(e) => setCanon(e.target.value as CanonLevel)}>
            {Object.entries(CANON).map(([k, v]) => (
              <option key={k} value={k}>
                {v}
              </option>
            ))}
          </select>
        </label>
      </div>
      <label>
        摘要
        <textarea rows={2} value={summary} disabled={ro} onChange={(e) => setSummary(e.target.value)} />
      </label>
      <label>
        玩家可见
        <textarea rows={4} value={player} disabled={ro} onChange={(e) => setPlayer(e.target.value)} />
      </label>
      <label className="gm-label">
        仅主持人
        <textarea rows={4} value={gm} disabled={ro} onChange={(e) => setGm(e.target.value)} />
      </label>
      {n.kind === "character" && <ScheduleEditor worldId={ctx.worldId} payload={payload} setPayload={setPayload} disabled={ro} />}
      {n.kind === "faction" && <FactionPowerEditor payload={payload} setPayload={setPayload} disabled={ro} />}
      {n.kind === "character" && <CharacterStatsEditor payload={payload} setPayload={setPayload} disabled={ro} />}
      {n.kind === "item" && <ItemStatsEditor payload={payload} setPayload={setPayload} disabled={ro} />}
      <details>
        <summary>结构化资料（JSON；secrets、hidden_agenda 和 gm_ 开头的字段只对主持人可见）</summary>
        <textarea rows={8} className="mono" value={payload} disabled={ro} onChange={(e) => setPayload(e.target.value)} />
      </details>
      <label>
        标签
        <input value={tags} disabled={ro} onChange={(e) => setTags(e.target.value)} />
      </label>
      <div className="row wrap">
        <button className="primary" disabled={ro} onClick={() => void save()}>
          保存
        </button>
        <button onClick={() => void setLock(!n.locked)}>{n.locked ? "解锁" : "锁定"}</button>
        <button onClick={() => void reorder(-1)}>上移</button>
        <button onClick={() => void reorder(1)}>下移</button>
        <button className="danger" disabled={ro} onClick={() => void remove()}>
          删除
        </button>
      </div>
      <small className="hint">
        来源：{n.source.startsWith("ai:") ? "AI 生成（已确认）" : n.source === "user" ? "手动" : n.source} · 版本 {n.rev} · 核心设定 AI 不能修改
      </small>
    </div>
  );
}

function NodePicker({ ctx, onPick, exclude }: { ctx: Ctx; onPick: (id: string, title: string) => void; exclude: string }) {
  const [q, setQ] = useState("");
  const [hits, setHits] = useState<SearchHit[]>([]);
  useEffect(() => {
    if (!q.trim()) {
      setHits([]);
      return;
    }
    const t = setTimeout(() => {
      api.searchNodes(ctx.worldId, q, { kinds: [], player_view: false, limit: 20, offset: 0 }).then(setHits).catch(() => {});
    }, 250);
    return () => clearTimeout(t);
  }, [q, ctx.worldId]);
  return (
    <div className="picker">
      <input value={q} onChange={(e) => setQ(e.target.value)} placeholder="搜索目标节点" />
      {hits.filter((h) => h.node.id !== exclude).map((h) => (
        <button
          key={h.node.id}
          className="hit"
          onClick={() => {
            onPick(h.node.id, h.node.title);
            setQ("");
          }}
        >
          <span className="kind">{NODE_KIND[h.node.kind]}</span>
          {h.node.title}
        </button>
      ))}
    </div>
  );
}

function Relations({ ctx, d, relTypes, onSelect }: { ctx: Ctx; d: NodeDetail; relTypes: RelationType[]; onSelect: (id: string) => void }) {
  const [type, setType] = useState("related_to");
  const [target, setTarget] = useState<{ id: string; title: string } | null>(null);
  const [label, setLabel] = useState("");
  const [from, setFrom] = useState("");
  const [to, setTo] = useState("");
  const typeLabel = (t: string) => relTypes.find((r) => r.id === t)?.label ?? t;

  async function add() {
    if (!target) return;
    const rt = relTypes.find((r) => r.id === type);
    try {
      await api.createRelation(ctx.worldId, {
        src_id: d.node.id,
        dst_id: target.id,
        rel_type: type,
        directed: rt?.directed ?? true,
        label,
        attrs: {},
        visibility: "player",
        valid_from: from ? Number(from) : null,
        valid_to: to ? Number(to) : null,
      });
      setTarget(null);
      setLabel("");
      ctx.bump();
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }

  return (
    <div className="card">
      <h3>关系（{d.relations.length}）</h3>
      <ul className="rels">
        {d.relations.map((r) => (
          <li key={r.relation.id}>
            <span className="hint">{r.outgoing ? "→" : "←"}</span> <strong>{typeLabel(r.relation.rel_type)}</strong>{" "}
            <button className="link" onClick={() => onSelect(r.other_id)}>
              {r.other_title}
            </button>
            {r.relation.label && <span className="hint">（{r.relation.label}）</span>}
            {(r.relation.valid_from !== null || r.relation.valid_to !== null) && (
              <span className="hint">
                {" "}
                [{r.relation.valid_from ?? "…"} – {r.relation.valid_to ?? "…"}]
              </span>
            )}
            <button
              className="link danger"
              onClick={() =>
                void api
                  .deleteRelation(ctx.worldId, r.relation.id, r.relation.rev)
                  .then(ctx.bump)
                  .catch((e: Error) => ctx.notify(e.message, "error"))
              }
            >
              删除
            </button>
          </li>
        ))}
      </ul>
      <div className="form">
        <h3>添加关系</h3>
        <div className="row">
          <select value={type} onChange={(e) => setType(e.target.value)}>
            {relTypes.map((t) => (
              <option key={t.id} value={t.id}>
                {t.label}
              </option>
            ))}
          </select>
          {target ? (
            <span>
              → <strong>{target.title}</strong>{" "}
              <button className="link" onClick={() => setTarget(null)}>
                换一个
              </button>
            </span>
          ) : (
            <NodePicker ctx={ctx} exclude={d.node.id} onPick={(id, title) => setTarget({ id, title })} />
          )}
        </div>
        <div className="row">
          <label>
            说明
            <input value={label} onChange={(e) => setLabel(e.target.value)} />
          </label>
          <label>
            起始（世界时间）
            <input type="number" value={from} onChange={(e) => setFrom(e.target.value)} />
          </label>
          <label>
            结束
            <input type="number" value={to} onChange={(e) => setTo(e.target.value)} />
          </label>
        </div>
        <button className="primary" disabled={!target} onClick={() => void add()}>
          添加
        </button>
      </div>
    </div>
  );
}

function NodeAi({ ctx, d }: { ctx: Ctx; d: NodeDetail }) {
  const [count, setCount] = useState(5);
  const [kind, setKind] = useState<NodeKind | "">("");
  const [text, setText] = useState("");
  const [mode, setMode] = useState<RewriteMode>("expand");
  const [refs, setRefs] = useState<string[]>([]);
  const n = d.node;
  const protectedNode = n.locked || n.canon_level === "core";
  const running = Object.keys(ctx.progress).length > 0;
  async function go(task: Parameters<typeof api.startGeneration>[1], msg: string) {
    try {
      await api.startGeneration(ctx.worldId, task, refs);
      ctx.notify(msg + (refs.length ? `（附带 ${refs.length} 张参考图片）` : ""));
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }
  return (
    <div className="card form">
      <h3>AI 辅助：{n.title}</h3>
      <label>
        补充说明 / 要求
        <textarea rows={2} value={text} onChange={(e) => setText(e.target.value)} />
      </label>
      <RefImages ctx={ctx} value={refs} onChange={setRefs} nodeId={n.id} />
      <div className="row">
        <label>
          子节点数量
          <input type="number" min={1} max={20} value={count} onChange={(e) => setCount(Number(e.target.value))} />
        </label>
        <label>
          子节点类型
          <select value={kind} onChange={(e) => setKind(e.target.value as NodeKind | "")}>
            <option value="">由 AI 决定</option>
            {Object.entries(NODE_KIND).map(([k, v]) => (
              <option key={k} value={k}>
                {v}
              </option>
            ))}
          </select>
        </label>
        <button
          disabled={running}
          onClick={() => void go({ task: "expand_node", node_id: n.id, count, kinds: kind ? [kind] : [], guidance: text }, `已开始为「${n.title}」生成子节点`)}
        >
          生成子节点
        </button>
      </div>
      <div className="row">
        <label>
          修改方式
          <select value={mode} onChange={(e) => setMode(e.target.value as RewriteMode)}>
            {Object.entries(REWRITE_MODE).map(([k, v]) => (
              <option key={k} value={k}>
                {v}
              </option>
            ))}
          </select>
        </label>
        <button disabled={running || protectedNode} onClick={() => void go({ task: "rewrite_node", node_id: n.id, mode, instruction: text }, `已开始${REWRITE_MODE[mode]}「${n.title}」`)}>
          {REWRITE_MODE[mode]}这个节点
        </button>
      </div>
      {protectedNode && <p className="hint">该节点已锁定或属于核心设定，不能交给 AI 修改（仍可以生成子节点）。</p>}
      <p className="hint">生成时会读取祖先节点、已有子节点、相关节点与世界核心设定；结果需要在审阅台确认后才写入。</p>
    </div>
  );
}
