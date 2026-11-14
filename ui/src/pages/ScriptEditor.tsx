import { useCallback, useEffect, useMemo, useState } from "react";
import { api, type ScriptView } from "../api";
import type { Ctx } from "../App";
import type { ScriptItem } from "../bindings/ScriptItem";
import type { ScriptItemKind } from "../bindings/ScriptItemKind";
import type { Task } from "../bindings/Task";
import { ITEM_KIND, QUEST_TYPE, toRows } from "../lib/labels";
import { ItemStatsEditor, QuestRulesEditor, TriggerRulesEditor } from "../components/RuleEditors";
import type { Choices } from "../components/DslEditors";

export default function ScriptEditor({ ctx, scriptId }: { ctx: Ctx; scriptId: string }) {
  const [view, setView] = useState<ScriptView | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [checked, setChecked] = useState<Set<string>>(new Set());
  const [castChecked, setCastChecked] = useState<Set<string>>(new Set());
  const [aiText, setAiText] = useState("");
  const [newCount, setNewCount] = useState(0);
  const [newKind, setNewKind] = useState<ScriptItemKind>("scene");
  const [showMeta, setShowMeta] = useState(false);

  const load = useCallback(async () => {
    try {
      setView(await api.getScript(ctx.worldId, scriptId));
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }, [ctx.worldId, scriptId, ctx.tick]);

  useEffect(() => {
    void load();
  }, [load]);

  const rows = useMemo(() => (view ? toRows(view.items) : []), [view]);
  const current = view?.items.find((i) => i.id === selected) ?? null;

  async function run(task: Task, msg: string) {
    try {
      await api.startGeneration(ctx.worldId, task);
      ctx.notify(msg);
      setAiText("");
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }

  function toggle(set: Set<string>, id: string, update: (s: Set<string>) => void) {
    const n = new Set(set);
    if (n.has(id)) n.delete(id);
    else n.add(id);
    update(n);
  }

  async function addItem() {
    const parent = current && current.kind !== "scene" ? current.id : (current?.parent_id ?? null);
    try {
      const it = await api.createScriptItem(ctx.worldId, {
        script_id: scriptId,
        kind: newKind,
        parent_id: parent,
        position: "last",
        title: `新${ITEM_KIND[newKind]}`,
        player_text: "",
        gm_text: "",
        payload: newKind === "quest" ? { quest_type: "side" } : {},
        locked: false,
      });
      await load();
      setSelected(it.id);
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }

  if (!view) return <p className="hint">加载中…</p>;
  const s = view.script;
  const running = Object.keys(ctx.progress).length > 0;

  return (
    <section className="editor">
      <header className="editor-head">
        <div>
          <button className="link" onClick={() => ctx.go({ name: "scripts" })}>
            ← 剧本列表
          </button>
          <h2>{s.title}</h2>
          <small className="hint">
            {s.genre || "未分类"} · {s.player_count} 人 · {s.duration_hours} 小时 · 版本 {s.rev}
          </small>
        </div>
        <div className="row">
          <button className="primary" onClick={() => ctx.go({ name: "plot", scriptId })}>
            剧情分支图
          </button>
          <button onClick={() => setShowMeta(!showMeta)}>{showMeta ? "收起剧本设定" : "编辑剧本设定"}</button>
        </div>
      </header>
      {showMeta && <MetaEditor ctx={ctx} view={view} onSaved={load} />}

      <div className="editor-body">
        <div className="tree card">
          <div className="tree-tools">
            <select value={newKind} onChange={(e) => setNewKind(e.target.value as ScriptItemKind)}>
              {Object.entries(ITEM_KIND).map(([k, v]) => (
                <option key={k} value={k}>
                  {v}
                </option>
              ))}
            </select>
            <button onClick={() => void addItem()} title="添加到选中条目下（场景则添加为同级）">
              新增
            </button>
          </div>
          {rows.length === 0 && <p className="hint">还没有条目。可以手动新增，或在下方让 AI 扩展。</p>}
          <ul>
            {rows.map(({ item, depth }) => (
              <li key={item.id} style={{ paddingLeft: depth * 16 }} className={item.id === selected ? "sel" : ""}>
                <input
                  type="checkbox"
                  checked={checked.has(item.id)}
                  onChange={() => toggle(checked, item.id, setChecked)}
                  title="选中后可让 AI 只重写这些条目"
                />
                <button onClick={() => setSelected(item.id)}>
                  <span className={`kind k-${item.kind}`}>{ITEM_KIND[item.kind]}</span>
                  {item.title}
                  {item.locked && <span title="已锁定"> 🔒</span>}
                  {item.source.startsWith("ai:") && <span className="ai" title="由 AI 生成，已经过你的确认">AI</span>}
                </button>
              </li>
            ))}
          </ul>
        </div>

        <div className="detail">
          {current ? <ItemEditor key={current.id + current.rev} ctx={ctx} item={current} view={view} onSaved={load} onDeleted={() => { setSelected(null); void load(); }} /> : <p className="hint">选择左侧的条目进行编辑。</p>}

          <div className="card">
            <h3>AI 辅助</h3>
            <textarea rows={2} value={aiText} onChange={(e) => setAiText(e.target.value)} placeholder="补充说明或修改要求（可选）" />
            <div className="row wrap">
              <button
                disabled={running || current?.kind !== "chapter"}
                title={current?.kind === "chapter" ? "" : "先在左侧选中一个章节"}
                onClick={() => current && void run({ task: "expand_chapter", script_id: scriptId, chapter_id: current.id, guidance: aiText }, `已开始展开「${current.title}」`)}
              >
                展开选中章节
              </button>
              <button
                disabled={running || checked.size === 0}
                onClick={() => void run({ task: "regenerate", script_id: scriptId, item_ids: [...checked], instruction: aiText }, `已开始重写 ${checked.size} 个条目`)}
              >
                只重写勾选的 {checked.size} 个条目
              </button>
              <button
                disabled={running || !aiText.trim()}
                title="只新增，不修改已有内容"
                onClick={() => void run({ task: "extend", script_id: scriptId, instruction: aiText }, "已开始扩展剧本")}
              >
                继续扩展（只新增）
              </button>
            </div>
            <p className="hint">锁定的条目不会交给 AI；所有结果都要在审阅台确认后才写入。</p>
          </div>

          <div className="card">
            <h3>剧本人物</h3>
            {view.cast.length === 0 && <p className="hint">还没有人物。生成大纲时会自动加入。</p>}
            <ul className="cast">
              {view.cast.map((c) => (
                <li key={c.cast.id}>
                  <label className="check">
                    <input type="checkbox" checked={castChecked.has(c.cast.node_id)} onChange={() => toggle(castChecked, c.cast.node_id, setCastChecked)} />
                    <strong>{c.name}</strong>
                    {c.cast.role && <span className="hint">（{c.cast.role}）</span>}
                  </label>
                  {c.cast.secret && <div className="gm">秘密：{c.cast.secret}</div>}
                </li>
              ))}
            </ul>
            <AddCast ctx={ctx} scriptId={scriptId} existing={view.cast.map((c) => c.cast.node_id)} onAdded={() => void load()} />
            <div className="row">
              <label>
                另外新增
                <input type="number" min={0} max={10} value={newCount} onChange={(e) => setNewCount(Number(e.target.value))} style={{ width: 60 }} />人
              </label>
              <button
                disabled={running || (castChecked.size === 0 && newCount === 0)}
                onClick={() => void run({ task: "npcs", script_id: scriptId, node_ids: [...castChecked], new_count: newCount, guidance: aiText }, "已开始完善人物档案")}
              >
                AI 完善勾选的人物
              </button>
            </div>
          </div>
        </div>
      </div>
    </section>
  );
}

function MetaEditor({ ctx, view, onSaved }: { ctx: Ctx; view: ScriptView; onSaved: () => void }) {
  const s = view.script;
  const [f, setF] = useState({ ...s });
  async function save() {
    try {
      await api.updateScript(
        ctx.worldId,
        s.id,
        {
          title: f.title,
          genre: f.genre,
          premise: f.premise,
          background: f.background,
          core_setting: f.core_setting,
          player_count: f.player_count,
          duration_hours: f.duration_hours,
          tone: f.tone,
        },
        s.rev,
      );
      ctx.notify("已保存");
      onSaved();
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }
  const field = (k: "title" | "genre" | "tone", label: string) => (
    <label>
      {label}
      <input value={f[k]} onChange={(e) => setF({ ...f, [k]: e.target.value })} />
    </label>
  );
  return (
    <div className="card form">
      <div className="row">
        {field("title", "名称")}
        {field("genre", "题材")}
        {field("tone", "基调")}
        <label>
          玩家人数
          <input type="number" value={f.player_count} onChange={(e) => setF({ ...f, player_count: Number(e.target.value) })} />
        </label>
        <label>
          时长
          <input type="number" step={0.5} value={f.duration_hours} onChange={(e) => setF({ ...f, duration_hours: Number(e.target.value) })} />
        </label>
      </div>
      <label>
        卖点
        <textarea rows={2} value={f.premise} onChange={(e) => setF({ ...f, premise: e.target.value })} />
      </label>
      <label>
        背景（玩家可见）
        <textarea rows={3} value={f.background} onChange={(e) => setF({ ...f, background: e.target.value })} />
      </label>
      <label className="gm-label">
        核心设定与真相（仅主持人）
        <textarea rows={3} value={f.core_setting} onChange={(e) => setF({ ...f, core_setting: e.target.value })} />
      </label>
      <button className="primary" onClick={() => void save()}>
        保存剧本设定
      </button>
    </div>
  );
}

function ItemEditor({ ctx, item, view, onSaved, onDeleted }: { ctx: Ctx; item: ScriptItem; view: ScriptView; onSaved: () => void; onDeleted: () => void }) {
  const choices: Choices = useMemo(() => {
    const by = (k: string) => view.items.filter((i) => i.kind === k).map((i) => ({ id: i.id, label: i.title }));
    return { quests: by("quest"), clues: by("clue"), items: by("item"), plotNodes: [], people: view.cast.map((c) => ({ id: c.cast.node_id, label: c.name })) };
  }, [view]);
  const [title, setTitle] = useState(item.title);
  const [player, setPlayer] = useState(item.player_text);
  const [gm, setGm] = useState(item.gm_text);
  const [payload, setPayload] = useState(JSON.stringify(item.payload, null, 2));
  const questType = (item.payload as Record<string, unknown> | null)?.["quest_type"] as string | undefined;

  async function save(extra?: { locked?: boolean }) {
    let p: unknown;
    try {
      p = JSON.parse(payload || "{}");
    } catch {
      ctx.notify("附加资料不是有效的 JSON", "error");
      return;
    }
    try {
      const patch = extra?.locked !== undefined && item.locked
        ? { locked: extra.locked }
        : { title, player_text: player, gm_text: gm, payload: p as never, ...extra };
      await api.updateScriptItem(ctx.worldId, item.id, patch, item.rev);
      ctx.notify("已保存");
      onSaved();
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }

  async function remove() {
    if (!confirm(`删除「${item.title}」及其所有子条目？（可在历史中撤销）`)) return;
    try {
      await api.deleteScriptItem(ctx.worldId, item.id, item.rev);
      onDeleted();
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }

  const ro = item.locked;
  return (
    <div className="card form">
      <h3>
        {ITEM_KIND[item.kind]}
        {questType && ` · ${QUEST_TYPE[questType] ?? questType}`}
        {ro && " · 已锁定（解锁后才能编辑）"}
      </h3>
      <label>
        标题
        <input value={title} disabled={ro} onChange={(e) => setTitle(e.target.value)} />
      </label>
      {item.kind === "quest" && (
        <label>
          任务类型
          <select
            disabled={ro}
            value={questType ?? "side"}
            onChange={(e) => {
              const obj = (() => {
                try {
                  return JSON.parse(payload || "{}");
                } catch {
                  return {};
                }
              })();
              setPayload(JSON.stringify({ ...obj, quest_type: e.target.value }, null, 2));
            }}
          >
            {Object.entries(QUEST_TYPE).map(([k, v]) => (
              <option key={k} value={k}>
                {v}
              </option>
            ))}
          </select>
        </label>
      )}
      <label>
        玩家可见
        <textarea rows={5} value={player} disabled={ro} onChange={(e) => setPlayer(e.target.value)} />
      </label>
      <label className="gm-label">
        仅主持人
        <textarea rows={5} value={gm} disabled={ro} onChange={(e) => setGm(e.target.value)} />
      </label>
      {item.kind === "quest" && <QuestRulesEditor worldId={ctx.worldId} payload={payload} setPayload={setPayload} choices={choices} disabled={ro} />}
      {item.kind === "item" && <ItemStatsEditor payload={payload} setPayload={setPayload} disabled={ro} />}
      {(item.kind === "trigger" || item.kind === "key_event") && <TriggerRulesEditor payload={payload} setPayload={setPayload} choices={choices} disabled={ro} />}
      <details>
        <summary>附加资料（JSON）</summary>
        <textarea rows={6} className="mono" value={payload} disabled={ro} onChange={(e) => setPayload(e.target.value)} />
      </details>
      <div className="row">
        <button className="primary" disabled={ro} onClick={() => void save()}>
          保存
        </button>
        <button onClick={() => void save({ locked: !item.locked })}>{item.locked ? "解锁" : "锁定（禁止 AI 修改）"}</button>
        <button className="danger" disabled={ro} onClick={() => void remove()}>
          删除
        </button>
      </div>
      <small className="hint">
        来源：{item.source.startsWith("ai:") ? "AI 生成（已确认）" : "手动"} · 版本 {item.rev}
      </small>
    </div>
  );
}

/** 从世界中已有的人物里选一个加入剧本人物表。 */
function AddCast({ ctx, scriptId, existing, onAdded }: { ctx: Ctx; scriptId: string; existing: string[]; onAdded: () => void }) {
  const [q, setQ] = useState("");
  const [hits, setHits] = useState<{ id: string; title: string }[]>([]);
  const [role, setRole] = useState("");
  return (
    <div className="row wrap" style={{ marginBottom: 8 }}>
      <input value={q} onChange={(e) => setQ(e.target.value)} placeholder="从世界中查找人物" style={{ width: 140 }} />
      <button
        onClick={() =>
          void api
            .searchNodes(ctx.worldId, q.trim(), { kinds: ["character"], player_view: false, limit: 10, offset: 0 })
            .then((h) => setHits(h.map((x) => ({ id: x.node.id, title: x.node.title })).filter((x) => !existing.includes(x.id))))
            .catch((e: Error) => ctx.notify(e.message, "error"))
        }
        disabled={!q.trim()}
      >
        查找
      </button>
      <input value={role} onChange={(e) => setRole(e.target.value)} placeholder="在剧本中的身份（可空）" style={{ width: 140 }} />
      {hits.map((h) => (
        <button
          key={h.id}
          className="link"
          onClick={() =>
            void api
              .addCastMember(ctx.worldId, scriptId, h.id, role)
              .then(() => {
                setHits([]);
                setQ("");
                ctx.notify(`已加入「${h.title}」`);
                onAdded();
              })
              .catch((e: Error) => ctx.notify(e.message, "error"))
          }
        >
          加入「{h.title}」
        </button>
      ))}
    </div>
  );
}
