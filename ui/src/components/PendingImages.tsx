// 图片建议：进入新地点、人物出场时，程序只建议生成图片；玩家点「生成」才调用生图服务，看过预览点「保存」才写进节点。
import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";
import { api } from "../api";
import type { Ctx } from "../App";

type Req = { node_id: string; title: string; kind: string; prompt: string };

function size(dataUrl: string): Promise<[number, number]> {
  return new Promise((resolve) => {
    const img = new Image();
    img.onload = () => resolve([img.naturalWidth, img.naturalHeight]);
    img.onerror = () => resolve([0, 0]);
    img.src = dataUrl;
  });
}

export default function PendingImages({ ctx }: { ctx: Ctx }) {
  const [list, setList] = useState<Req[]>([]);
  const [busy, setBusy] = useState<string | null>(null);
  const [preview, setPreview] = useState<Record<string, string>>({});
  const [prompts, setPrompts] = useState<Record<string, string>>({});
  const load = () => void api.pendingImages(ctx.worldId).then(setList).catch(() => setList([]));
  useEffect(load, [ctx.worldId, ctx.tick]);
  useEffect(() => {
    let off: (() => void) | undefined;
    void listen<{ world_id: string }>("pending-images", (e) => e.payload.world_id === ctx.worldId && load()).then((f) => (off = f));
    return () => off?.();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ctx.worldId]);
  if (!list.length) return null;
  const done = (id: string) =>
    void api
      .dismissPendingImage(ctx.worldId, id)
      .then(() => {
        setPreview((p) => {
          const n = { ...p };
          delete n[id];
          return n;
        });
        load();
      })
      .catch((e: Error) => ctx.notify(e.message, "error"));
  return (
    <div className="pending-images card">
      <strong>图片建议（需要你确认；云端生图可能收费）</strong>
      {list.map((r) => (
        <div key={r.node_id} className="pending-image">
          <div className="row wrap">
            <span style={{ flex: 1 }}>
              为「{r.title}」生成{r.kind === "portrait" ? "立绘" : "场景插图"}？
            </span>
            {!preview[r.node_id] && (
              <button
                className="small primary"
                disabled={busy !== null}
                onClick={() => {
                  setBusy(r.node_id);
                  api
                    .generateImage(prompts[r.node_id] ?? r.prompt, null)
                    .then((g) => setPreview((p) => ({ ...p, [r.node_id]: g.data_url })))
                    .catch((e: Error) => ctx.notify(e.message, "error"))
                    .finally(() => setBusy(null));
                }}
              >
                {busy === r.node_id ? "生成中…" : "生成"}
              </button>
            )}
            <button className="small link" onClick={() => done(r.node_id)}>
              不要
            </button>
          </div>
          {!preview[r.node_id] && (
            <input className="prompt" value={prompts[r.node_id] ?? r.prompt} onChange={(e) => setPrompts((p) => ({ ...p, [r.node_id]: e.target.value }))} title="生图提示词（只含玩家可见的描述，可以改）" />
          )}
          {preview[r.node_id] && (
            <div className="row wrap">
              <img src={preview[r.node_id]} alt="" className="pending-preview" />
              <button
                className="small primary"
                onClick={async () => {
                  const [w, h] = await size(preview[r.node_id]);
                  try {
                    await api.nodeImageAdd(ctx.worldId, r.node_id, preview[r.node_id], w, h, r.kind === "portrait" ? "生成的立绘" : "生成的场景插图");
                    ctx.notify("已保存到「" + r.title + "」");
                    done(r.node_id);
                    ctx.bump();
                  } catch (e) {
                    ctx.notify((e as Error).message, "error");
                  }
                }}
              >
                保存
              </button>
              <button className="small" onClick={() => setPreview((p) => ({ ...p, [r.node_id]: "" }))}>
                重新生成
              </button>
            </div>
          )}
        </div>
      ))}
    </div>
  );
}
