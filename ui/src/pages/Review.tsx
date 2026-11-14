import { useEffect, useMemo, useState } from "react";
import { api, formatTime } from "../api";
import type { Ctx } from "../App";
import type { Changeset } from "../bindings/Changeset";
import type { Issue } from "../bindings/Issue";
import type { Op } from "../bindings/Op";
import { describeOp } from "../lib/labels";

const MODE: Record<string, string> = { normal: "普通", append: "只新增", targeted: "只改选中条目" };

export default function ReviewPage({ ctx, initialId }: { ctx: Ctx; initialId?: string }) {
  const [list, setList] = useState<Changeset[]>([]);
  const [selId, setSelId] = useState<string | null>(initialId ?? null);
  const [picked, setPicked] = useState<Set<number>>(new Set());
  const [editing, setEditing] = useState<number | null>(null);
  const [draft, setDraft] = useState("");
  const [lastCommit, setLastCommit] = useState<{ id: string; title: string } | null>(null);
  const [showDone, setShowDone] = useState(false);

  useEffect(() => {
    api
      .listChangesets(ctx.worldId, showDone ? null : "pending")
      .then((l) => {
        setList(l);
        if (!selId && l.length > 0) setSelId(l[0].id);
      })
      .catch((e: Error) => ctx.notify(e.message, "error"));
  }, [ctx.worldId, ctx.tick, showDone]);

  const cs = list.find((c) => c.id === selId) ?? null;

  // 默认勾选没有错误的操作。
  useEffect(() => {
    if (!cs) return;
    const bad = new Set(cs.validation.issues.filter((i) => i.severity === "error").map((i) => i.op));
    setPicked(new Set(cs.ops.map((_, i) => i).filter((i) => !bad.has(i))));
    setEditing(null);
  }, [cs?.id, cs?.updated_at]);

  const issuesByOp = useMemo(() => {
    const m = new Map<number, Issue[]>();
    for (const i of cs?.validation.issues ?? []) m.set(i.op, [...(m.get(i.op) ?? []), i]);
    return m;
  }, [cs]);

  async function apply() {
    if (!cs) return;
    try {
      const all = picked.size === cs.ops.length;
      const done = await api.applyChangeset(ctx.worldId, cs.id, all ? null : [...picked].sort((a, b) => a - b));
      if (done.commit_id) setLastCommit({ id: done.commit_id, title: cs.title });
      ctx.notify(`已写入 ${picked.size} 项`);
      setSelId(null);
      ctx.bump();
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }

  async function reject() {
    if (!cs || !confirm("放弃这个变更集？世界内容不会改变。")) return;
    try {
      await api.rejectChangeset(ctx.worldId, cs.id);
      setSelId(null);
      ctx.bump();
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }

  async function saveEdit(index: number) {
    if (!cs) return;
    let op: Op;
    try {
      op = JSON.parse(draft);
    } catch {
      ctx.notify("不是有效的 JSON", "error");
      return;
    }
    try {
      await api.reviseOp(ctx.worldId, cs.id, index, op);
      setEditing(null);
      ctx.bump();
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }

  async function undo() {
    if (!lastCommit) return;
    try {
      await api.undoCommit(ctx.worldId, lastCommit.id);
      ctx.notify(`已撤销「${lastCommit.title}」`);
      setLastCommit(null);
      ctx.bump();
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }

  const generalNotes = cs?.validation.issues.filter((i) => i.code.startsWith("gen.") && (i.code === "gen.scale" || i.code.startsWith("gen.skipped") || i.code.startsWith("gen.missing") || i.code.startsWith("gen.ignored") || i.code.startsWith("gen.unexpected"))) ?? [];

  return (
    <section className="review">
      <h2>审阅台</h2>
      {lastCommit && (
        <div className="card notice">
          已写入「{lastCommit.title}」。<button onClick={() => void undo()}>撤销这次写入</button>
        </div>
      )}
      <div className="review-body">
        <div className="card list-col">
          <label className="check">
            <input type="checkbox" checked={showDone} onChange={(e) => setShowDone(e.target.checked)} />
            显示已处理
          </label>
          {list.length === 0 && <p className="hint">没有待审的变更。</p>}
          <ul className="list">
            {list.map((c) => (
              <li key={c.id}>
                <button className={c.id === selId ? "active" : ""} onClick={() => setSelId(c.id)}>
                  <strong>{c.title || "未命名变更"}</strong>
                  <small>
                    {c.ops.length} 项 · {MODE[c.mode.type]} · {c.validation.ok ? "可写入" : "有错误"} · {formatTime(c.created_at)}
                    {c.status !== "pending" && ` · ${c.status}`}
                  </small>
                </button>
              </li>
            ))}
          </ul>
        </div>

        {cs && (
          <div className="ops-col">
            <div className="card">
              <h3>{cs.title}</h3>
              <p className="hint">
                来源：{cs.author.type === "ai" ? `AI 任务 ${cs.author.job_id.slice(-6)}` : cs.author.type} · 模式：{MODE[cs.mode.type]} · 校验时间 {formatTime(cs.validation.checked_at)}
              </p>
              {generalNotes.length > 0 && (
                <ul className="issues">
                  {generalNotes.map((i, k) => (
                    <li key={k} className={i.severity}>
                      {i.message}
                    </li>
                  ))}
                </ul>
              )}
              {cs.status === "pending" && (
                <div className="row">
                  <button className="primary" disabled={picked.size === 0} onClick={() => void apply()}>
                    写入勾选的 {picked.size} / {cs.ops.length} 项
                  </button>
                  <button onClick={() => setPicked(new Set(cs.ops.map((_, i) => i)))}>全选</button>
                  <button onClick={() => setPicked(new Set())}>全不选</button>
                  <button className="danger" onClick={() => void reject()}>
                    放弃
                  </button>
                </div>
              )}
              <p className="hint">依赖关系：如果一项依赖另一项新建的内容（例如场景挂在新章节下），必须一起勾选，否则写入会被拒绝且不会写入任何内容。</p>
            </div>
            <ol className="ops">
              {cs.ops.map((op, i) => {
                const d = describeOp(op);
                const iss = (issuesByOp.get(i) ?? []).filter((x) => !generalNotes.includes(x));
                const hasError = iss.some((x) => x.severity === "error");
                return (
                  <li key={i} className={hasError ? "op has-error" : "op"}>
                    <div className="op-head">
                      {cs.status === "pending" && (
                        <input
                          type="checkbox"
                          checked={picked.has(i)}
                          onChange={() => {
                            const n = new Set(picked);
                            if (n.has(i)) n.delete(i);
                            else n.add(i);
                            setPicked(n);
                          }}
                        />
                      )}
                      <span className={`act act-${d.action}`}>{d.action}</span>
                      <span className="hint">{d.what}</span>
                      <strong>{d.title}</strong>
                      {cs.status === "pending" && (
                        <button
                          className="link"
                          onClick={() => {
                            setEditing(editing === i ? null : i);
                            setDraft(JSON.stringify(op, null, 2));
                          }}
                        >
                          {editing === i ? "取消编辑" : "编辑"}
                        </button>
                      )}
                    </div>
                    {d.player && <div className="player">{d.player}</div>}
                    {d.gm && <div className="gm">主持人：{d.gm}</div>}
                    {iss.length > 0 && (
                      <ul className="issues">
                        {iss.map((x, k) => (
                          <li key={k} className={x.severity}>
                            {x.severity === "error" ? "错误" : "提示"}：{x.message}
                          </li>
                        ))}
                      </ul>
                    )}
                    {editing === i && (
                      <div className="form">
                        <textarea className="mono" rows={12} value={draft} onChange={(e) => setDraft(e.target.value)} />
                        <button className="primary" onClick={() => void saveEdit(i)}>
                          保存修改并重新校验
                        </button>
                      </div>
                    )}
                  </li>
                );
              })}
            </ol>
          </div>
        )}
      </div>
    </section>
  );
}
