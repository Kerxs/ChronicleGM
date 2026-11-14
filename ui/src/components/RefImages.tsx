// 生成用的参考图片：上传（选文件 / 拖放 / 粘贴），或勾选某个节点上已有的图片。最多 6 张。
// 上传的图片缩到长边 1280 像素的 JPEG（足够模型看清，请求也不会太大）。
import { useEffect, useRef, useState } from "react";
import { api } from "../api";
import type { Ctx } from "../App";
import type { NodeImage } from "../bindings/NodeImage";
import { imageFiles, resizeImage } from "../lib/images";
import { ensureThumb } from "../lib/thumbs";

export const MAX_REFS = 6;

export default function RefImages({ ctx, value, onChange, nodeId }: { ctx: Ctx; value: string[]; onChange: (v: string[]) => void; nodeId?: string }) {
  const [nodeImgs, setNodeImgs] = useState<NodeImage[]>([]);
  const [thumbs, setThumbs] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const fileRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!nodeId) return;
    api
      .nodeImages(ctx.worldId, nodeId)
      .then(async (l) => {
        setNodeImgs(l);
        const ts = await api.nodeImageThumbs(ctx.worldId, nodeId);
        const got = await Promise.all(ts.map(async (t) => [`node-image:${t.image_id}`, (await ensureThumb(ctx.worldId, t)) ?? ""] as const));
        setThumbs((m) => ({ ...m, ...Object.fromEntries(got) }));
      })
      .catch(() => {});
  }, [ctx.worldId, nodeId, ctx.tick]);

  async function add(files: File[]) {
    const room = MAX_REFS - value.length;
    if (room <= 0) return ctx.notify(`参考图片最多 ${MAX_REFS} 张`, "error");
    setBusy(true);
    const out: string[] = [];
    for (const f of files.slice(0, room)) {
      try {
        out.push((await resizeImage(f, 1280, 0.85, true)).dataUrl);
      } catch (e) {
        ctx.notify((e as Error).message, "error");
      }
    }
    setBusy(false);
    onChange([...value, ...out]);
  }

  const toggleNode = (id: string) => {
    const ref = `node-image:${id}`;
    if (value.includes(ref)) onChange(value.filter((v) => v !== ref));
    else if (value.length < MAX_REFS) onChange([...value, ref]);
    else ctx.notify(`参考图片最多 ${MAX_REFS} 张`, "error");
  };

  return (
    <div
      className="ref-images"
      onDragOver={(e) => e.preventDefault()}
      onDrop={(e) => {
        e.preventDefault();
        void add(imageFiles(e.dataTransfer.files));
      }}
      onPaste={(e) => {
        const f = imageFiles(e.clipboardData.items);
        if (f.length) void add(f);
      }}
    >
      <div className="row wrap">
        <span className="ref-label">参考图片</span>
        {value.map((v, i) => (
          <span key={i} className="ref-chip">
            <img src={v.startsWith("node-image:") ? thumbs[v] : v} alt="" />
            <button type="button" className="link" title="移除" onClick={() => onChange(value.filter((_, j) => j !== i))}>
              ×
            </button>
          </span>
        ))}
        {value.length < MAX_REFS && (
          <button type="button" className="small" disabled={busy} onClick={() => fileRef.current?.click()}>
            {busy ? "处理中…" : "+ 添加图片"}
          </button>
        )}
        <input ref={fileRef} type="file" accept="image/png,image/jpeg,image/webp,image/gif" multiple hidden onChange={(e) => { void add(Array.from(e.target.files ?? [])); e.target.value = ""; }} />
      </div>
      {nodeImgs.length > 0 && (
        <div className="row wrap ref-node">
          <span className="hint">使用本节点的图片：</span>
          {nodeImgs.map((i) => (
            <label key={i.id} className="check ref-pick">
              <input type="checkbox" checked={value.includes(`node-image:${i.id}`)} onChange={() => toggleNode(i.id)} />
              {thumbs[`node-image:${i.id}`] && <img src={thumbs[`node-image:${i.id}`]} alt="" />}
              {i.caption || "图片"}
            </label>
          ))}
        </div>
      )}
      <p className="hint">可选。设定图、地图、人物画像等，AI 会把图里看得清的内容作为设定依据。需要模型服务通过「看图」测试（模型服务页）。也可以拖进来或粘贴。</p>
    </div>
  );
}
