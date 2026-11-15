// 剧本版本记录：手动保存快照，和当前内容对比，恢复（恢复前自动保存当前状态）。
import { useEffect, useState } from "react";
import { api } from "../api";
import type { Ctx } from "../App";
import type { ScriptVersion } from "../bindings/ScriptVersion";
import type { VersionDiff } from "../bindings/VersionDiff";

export default function ScriptVersions({ ctx, scriptId, rev }: { ctx: Ctx; scriptId: string; rev: number }) {
  const [list, setList] = useState<ScriptVersion[]>([]);
  const [name, setName] = useState("");
  const [diff, setDiff] = useState<{ id: string; d: VersionDiff } | null>(null);
  const load = () => void api.scriptVersions(ctx.worldId, scriptId).then(setList).catch((e: Error) => ctx.notify(e.message, "error"));
  useEffect(load, [ctx.worldId, scriptId, rev, ctx.tick]);

  async function save() {
    try {
      await api.createScriptVersion(ctx.worldId, scriptId, name);
      setName("");
      ctx.notify("已保存版本");
      load();
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }

  return (
    <div className="card versions">
      <h3 style={{ marginTop: 0 }}>版本记录</h3>
      <form
        className="row"
        onSubmit={(e) => {
          e.preventDefault();
          void save();
        }}
      >
        <input value={name} onChange={(e) => setName(e.target.value)} placeholder="版本名称（可选，例如：改结局之前）" />
        <button className="primary">保存当前版本</button>
      </form>
      {list.length === 0 && <p className="hint">还没有版本。删除剧本、恢复版本之前会自动保存一个。</p>}
      {list.map((v, i) => (
        <div key={v.id}>
          <div className="ver" style={{ animationDelay: `${i * 0.03}s` }}>
            <span>
              <strong>{v.name}</strong>{" "}
              <small className="hint">
                {new Date(v.created_at).toLocaleString()} · {v.items} 个条目 · {v.nodes} 个剧情节点
              </small>
            </span>
            <button
              className="small"
              onClick={() =>
                diff?.id === v.id
                  ? setDiff(null)
                  : void api
                      .diffScriptVersion(ctx.worldId, v.id)
                      .then((d) => setDiff({ id: v.id, d }))
                      .catch((e: Error) => ctx.notify(e.message, "error"))
              }
            >
              {diff?.id === v.id ? "收起对比" : "和现在对比"}
            </button>
            <button
              className="small"
              onClick={() => {
                if (!window.confirm("把剧本恢复成「" + v.name + "」？\n会先自动保存当前状态为一个版本；之后新增的条目会移到回收站。")) return;
                void api
                  .restoreScriptVersion(ctx.worldId, v.id)
                  .then(() => {
                    ctx.notify("已恢复到「" + v.name + "」");
                    ctx.bump();
                  })
                  .catch((e: Error) => ctx.notify(e.message, "error"));
              }}
            >
              恢复
            </button>
            <button className="small danger" title="只删除这个快照，不影响剧本" onClick={() => void api.deleteScriptVersion(ctx.worldId, v.id).then(load).catch((e: Error) => ctx.notify(e.message, "error"))}>
              删除
            </button>
          </div>
          {diff?.id === v.id && (
            <div className="diff">
              {diff.d.added.length + diff.d.removed.length + diff.d.changed.length === 0 && <span className="hint">和现在完全一样。</span>}
              {diff.d.added.length > 0 && <div className="add">之后新增：{diff.d.added.join("、")}</div>}
              {diff.d.removed.length > 0 && <div className="del">之后删除：{diff.d.removed.join("、")}</div>}
              {diff.d.changed.length > 0 && <div className="chg">之后修改：{diff.d.changed.join("、")}</div>}
            </div>
          )}
        </div>
      ))}
    </div>
  );
}
