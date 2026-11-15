// 回收站：战役（恢复 / 永久删除）、世界（删除当前世界 / 恢复）。
import { useEffect, useState } from "react";
import { api } from "../api";
import type { Ctx } from "../App";
import type { DeletedCampaign } from "../bindings/DeletedCampaign";
import type { WorldInfo } from "../bindings/WorldInfo";

export function CampaignTrash({ ctx }: { ctx: Ctx }) {
  const [list, setList] = useState<DeletedCampaign[]>([]);
  const [open, setOpen] = useState(false);
  useEffect(() => {
    api.campaignTrash(ctx.worldId).then(setList).catch(() => setList([]));
  }, [ctx.worldId, ctx.tick]);
  if (list.length === 0) return null;
  const act = (p: Promise<unknown>, ok: string) =>
    void p
      .then(() => {
        ctx.notify(ok);
        ctx.bump();
      })
      .catch((e: Error) => ctx.notify(e.message, "error"));
  return (
    <span className="trash-inline">
      <button className="link" onClick={() => setOpen(!open)}>
        回收站（{list.length}）
      </button>
      {open && (
        <div className="trash-pop card">
          {list.map((c) => (
            <div key={c.id} className="row">
              <span style={{ flex: 1 }}>
                {c.name} <small className="hint">第 {c.turn} 回合 · {new Date(c.deleted_at).toLocaleString()}</small>
              </span>
              <button className="small" onClick={() => act(api.restoreCampaign(ctx.worldId, c.id), "已恢复「" + c.name + "」")}>
                恢复
              </button>
              <button
                className="small danger"
                onClick={() => {
                  if (window.confirm("永久删除战役「" + c.name + "」？\n事件日志、记忆、存档点都会删除，不能恢复（删除前会自动备份世界）。")) act(api.purgeCampaign(ctx.worldId, c.id), "已永久删除");
                }}
              >
                永久删除
              </button>
            </div>
          ))}
        </div>
      )}
    </span>
  );
}

export function WorldTrash({ ctx, worldName, onDeleted }: { ctx: Ctx; worldName: string; onDeleted: () => void }) {
  const [list, setList] = useState<WorldInfo[]>([]);
  useEffect(() => {
    api.worldTrash().then(setList).catch(() => setList([]));
  }, [ctx.tick]);
  return (
    <div className="card">
      <h3>删除与回收站</h3>
      <p className="hint">删除世界会把世界文件移到数据目录的 trash 文件夹，可以在这里恢复；备份文件不受影响。</p>
      <button
        className="danger"
        onClick={() => {
          if (!window.confirm("删除世界「" + worldName + "」？\n世界文件会移到回收站，可以恢复。")) return;
          void api
            .trashWorld(ctx.worldId)
            .then(() => {
              ctx.notify("已移到回收站");
              onDeleted();
            })
            .catch((e: Error) => ctx.notify(e.message, "error"));
        }}
      >
        删除这个世界
      </button>
      {list.length > 0 && (
        <ul className="list">
          {list.map((w) => (
            <li key={w.id} className="row">
              <span style={{ flex: 1 }}>{w.name}</span>
              <button
                className="small"
                onClick={() =>
                  void api
                    .restoreWorld(w.id)
                    .then(() => {
                      ctx.notify("已恢复「" + w.name + "」，在世界菜单里可以打开");
                      onDeleted();
                    })
                    .catch((e: Error) => ctx.notify(e.message, "error"))
                }
              >
                恢复
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
