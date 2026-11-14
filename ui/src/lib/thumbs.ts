// 节点图片缩略图：后端存长边 256 像素的 JPEG；旧图片没有缩略图时，用原图在界面里生成一次并保存。
// 内存里按 图片 ID 缓存（同一张图片的内容不会变，换封面时图片 ID 会变）。
import { api } from "../api";
import type { ImageThumb } from "../bindings/ImageThumb";

export const THUMB_SIDE = 256;
const cache = new Map<string, string>();
const pending = new Map<string, Promise<string | null>>();

function loadImg(src: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => resolve(img);
    img.onerror = () => reject(new Error("无法读取图片"));
    img.src = src;
  });
}

/** 把一张图片（data URL）缩成缩略图 JPEG data URL。 */
export async function makeThumb(src: string, side = THUMB_SIDE): Promise<string> {
  const img = await loadImg(src);
  const scale = Math.min(1, side / Math.max(img.naturalWidth, img.naturalHeight));
  const w = Math.max(1, Math.round(img.naturalWidth * scale));
  const h = Math.max(1, Math.round(img.naturalHeight * scale));
  const c = document.createElement("canvas");
  c.width = w;
  c.height = h;
  const g = c.getContext("2d");
  if (!g) throw new Error("无法处理图片");
  g.fillStyle = "#ffffff";
  g.fillRect(0, 0, w, h);
  g.imageSmoothingQuality = "high";
  g.drawImage(img, 0, 0, w, h);
  return c.toDataURL("image/jpeg", 0.82);
}

/** 生成并保存缩略图（失败时返回 null，界面显示占位）。 */
export function ensureThumb(worldId: string, t: Pick<ImageThumb, "image_id" | "thumb">): Promise<string | null> {
  if (t.thumb) {
    cache.set(t.image_id, t.thumb);
    return Promise.resolve(t.thumb);
  }
  const hit = cache.get(t.image_id);
  if (hit) return Promise.resolve(hit);
  let p = pending.get(t.image_id);
  if (!p) {
    p = api
      .nodeImageData(worldId, t.image_id)
      .then((full) => makeThumb(full))
      .then(async (thumb) => {
        cache.set(t.image_id, thumb);
        await api.nodeImageSetThumb(worldId, t.image_id, thumb).catch(() => {});
        return thumb;
      })
      .catch(() => null)
      .finally(() => pending.delete(t.image_id));
    pending.set(t.image_id, p);
  }
  return p;
}

/** 一批节点的封面缩略图：节点 ID → 缩略图 data URL（没有图片的节点不在结果里）。 */
export async function coverThumbs(worldId: string, nodeIds: string[]): Promise<Record<string, string>> {
  if (!nodeIds.length) return {};
  const list = await api.nodeCoverThumbs(worldId, nodeIds);
  const got = await Promise.all(list.map(async (t) => [t.node_id, await ensureThumb(worldId, t)] as const));
  return Object.fromEntries(got.filter((x): x is readonly [string, string] => !!x[1]));
}
