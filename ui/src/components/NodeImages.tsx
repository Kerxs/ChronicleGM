// 世界树节点的图片：图库、上传（选文件 / 拖放 / 粘贴）、大图、设封面、说明、删除、AI 看图描述。
import { SkeletonBlock } from "./Skeleton";
import { useEffect, useRef, useState } from "react";
import { api } from "../api";
import type { Ctx } from "../App";
import type { NodeImage } from "../bindings/NodeImage";
import type { Node } from "../bindings/Node";
import { imageFiles, resizeImage } from "../lib/images";
import { ensureThumb, makeThumb } from "../lib/thumbs";
import ImageGenPanel from "./ImageGenPanel";

const MAX_SIDE = 2048;

export default function NodeImages({ ctx, node, onChanged }: { ctx: Ctx; node: Node; onChanged: () => void }) {
  const [list, setList] = useState<NodeImage[]>([]);
  /** 网格里显示缩略图；大图、看图描述用原图（点开时才取）。 */
  const [thumbs, setThumbs] = useState<Record<string, string>>({});
  const [full, setFull] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const [drag, setDrag] = useState(false);
  const [open, setOpen] = useState<NodeImage | null>(null);
  const [desc, setDesc] = useState<{ id: string; text: string } | null>(null);
  const [describing, setDescribing] = useState<string | null>(null);
  const [genOpen, setGenOpen] = useState(false);
  const fileRef = useRef<HTMLInputElement>(null);
  const fail = (e: unknown) => ctx.notify((e as Error).message ?? String(e), "error");

  const reload = async () => {
    try {
      const [l, ts] = await Promise.all([api.nodeImages(ctx.worldId, node.id), api.nodeImageThumbs(ctx.worldId, node.id)]);
      setList(l);
      // 旧图片没有缩略图时用原图生成一次并保存。
      const got = await Promise.all(ts.filter((t) => !thumbs[t.image_id]).map(async (t) => [t.image_id, await ensureThumb(ctx.worldId, t)] as const));
      const ok = got.filter((x): x is readonly [string, string] => !!x[1]);
      if (ok.length) setThumbs((m) => ({ ...m, ...Object.fromEntries(ok) }));
    } catch (e) {
      fail(e);
    }
  };
  useEffect(() => {
    void reload();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [node.id]);

  async function add(files: File[]) {
    if (!files.length) return;
    setBusy(true);
    let ok = 0;
    for (const f of files) {
      try {
        const r = await resizeImage(f, MAX_SIDE);
        const img = await api.nodeImageAdd(ctx.worldId, node.id, r.dataUrl, r.width, r.height, f.name.replace(/\.[^.]+$/, ""));
        // 同时保存缩略图（树、图谱、游戏页用）；失败不影响图片本身，之后显示时会再补。
        try {
          const t = await makeThumb(r.dataUrl);
          await api.nodeImageSetThumb(ctx.worldId, img.id, t);
          setThumbs((m) => ({ ...m, [img.id]: t }));
        } catch {
          /* 之后补 */
        }
        ok++;
      } catch (e) {
        fail(e);
      }
    }
    setBusy(false);
    if (ok) ctx.notify(`已添加 ${ok} 张图片`);
    await reload();
    onChanged();
  }

  async function openFull(img: NodeImage) {
    setOpen(img);
    if (!full[img.id]) {
      try {
        const u = await api.nodeImageData(ctx.worldId, img.id);
        setFull((m) => ({ ...m, [img.id]: u }));
      } catch (e) {
        fail(e);
      }
    }
  }

  async function describe(img: NodeImage) {
    setDescribing(img.id);
    setDesc(null);
    try {
      setDesc({ id: img.id, text: await api.describeNodeImage(ctx.worldId, img.id) });
    } catch (e) {
      fail(e);
    } finally {
      setDescribing(null);
    }
  }

  async function appendToSummary(text: string) {
    try {
      const d = await api.nodeDetail(ctx.worldId, node.id);
      const summary = d.node.summary.trim() ? `${d.node.summary.trim()}\n\n${text}` : text;
      await api.updateNode(ctx.worldId, node.id, { summary }, d.node.rev);
      ctx.notify("已追加到摘要（可以在版本记录里撤销）");
      ctx.bump();
    } catch (e) {
      fail(e);
    }
  }

  return (
    <div
      className={`card node-images${drag ? " drag" : ""}`}
      onDragOver={(e) => {
        e.preventDefault();
        setDrag(true);
      }}
      onDragLeave={() => setDrag(false)}
      onDrop={(e) => {
        e.preventDefault();
        setDrag(false);
        void add(imageFiles(e.dataTransfer.files));
      }}
      onPaste={(e) => void add(imageFiles(e.clipboardData.items))}
      tabIndex={0}
    >
      <div className="row">
        <h3 style={{ margin: 0 }}>图片 · {node.title}</h3>
        <span className="spacer" />
        <button onClick={() => setGenOpen((x) => !x)}>AI 生成图片</button>
        <button className="primary" disabled={busy} onClick={() => fileRef.current?.click()}>
          {busy ? "正在添加…" : "添加图片"}
        </button>
        <input ref={fileRef} type="file" accept="image/png,image/jpeg,image/webp,image/gif" multiple hidden onChange={(e) => { void add(Array.from(e.target.files ?? [])); e.target.value = ""; }} />
      </div>
      <p className="hint">人物立绘、地点插图、地图、物品……可以选文件、拖进来，或直接粘贴（Ctrl+V）。图片保存在世界文件里，会随备份与导出一起保存；长边超过 {MAX_SIDE} 像素会自动缩小。第一张是封面。</p>
      {genOpen && (
        <ImageGenPanel
          ctx={ctx}
          node={node}
          onClose={() => setGenOpen(false)}
          onSaved={() => {
            void reload();
            onChanged();
          }}
        />
      )}
      {list.length === 0 && <div className="img-empty">还没有图片</div>}
      <div className="img-grid">
        {list.map((img, i) => (
          <figure key={img.id} className={i === 0 ? "cover" : ""}>
            <button className="thumb" onClick={() => void openFull(img)} title="查看大图">
              {thumbs[img.id] ? <img src={thumbs[img.id]} alt={img.caption || node.title} /> : <SkeletonBlock w="100%" h="100%" radius={10} />}
              {i === 0 && <span className="cover-tag">封面</span>}
            </button>
            <figcaption>
              <input
                defaultValue={img.caption}
                placeholder="说明"
                onBlur={(e) => {
                  if (e.target.value !== img.caption) void api.nodeImageCaption(ctx.worldId, img.id, e.target.value).then(reload).catch(fail);
                }}
              />
              <div className="row img-actions">
                <button className="small" disabled={describing !== null} onClick={() => void describe(img)}>
                  {describing === img.id ? "AI 看图中…" : "AI 看图描述"}
                </button>
                {i > 0 && <button className="small" onClick={() => void api.nodeImageCover(ctx.worldId, img.id).then(reload).catch(fail)}>设为封面</button>}
                <button
                  className="small danger"
                  onClick={() => {
                    if (window.confirm("删除这张图片？删除后不能撤销（备份里仍有）。")) void api.nodeImageDelete(ctx.worldId, img.id).then(reload).then(onChanged).catch(fail);
                  }}
                >
                  删除
                </button>
              </div>
              <small className="hint">
                {img.width}×{img.height} · {(img.size_bytes / 1024).toFixed(0)} KB
              </small>
            </figcaption>
            {desc?.id === img.id && (
              <div className="img-desc">
                <div className="hint">AI 看到的（只是建议，不会自动写入）：</div>
                <p>{desc.text}</p>
                <div className="row">
                  <button className="small primary" onClick={() => void appendToSummary(desc.text)}>追加到摘要</button>
                  <button className="small" onClick={() => void navigator.clipboard.writeText(desc.text).then(() => ctx.notify("已复制"))}>复制</button>
                  <button className="small link" onClick={() => setDesc(null)}>关闭</button>
                </div>
              </div>
            )}
          </figure>
        ))}
      </div>
      {open && (
        <div className="lightbox" onClick={() => setOpen(null)} role="dialog" aria-label="大图">
          {full[open.id] ? <img src={full[open.id]} alt={open.caption} /> : <SkeletonBlock w="100%" h="100%" radius={10} />}
          {open.caption && <div className="lightbox-cap">{open.caption}</div>}
        </div>
      )}
    </div>
  );
}
