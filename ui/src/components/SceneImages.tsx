// 游戏页的节点图片：
// - 「此刻的画面」：玩家所在地点的封面 + 在场人物的头像（来自世界树节点图片）。
// - 叙述里提到的、配了图的设定（按名称与别名匹配，只匹配玩家已经在叙述里看到的名字）。
// - 点开看这个节点的全部图片。图片说明可能写了秘密，只在主持人视角显示。
import { SkeletonBlock } from "./Skeleton";
import { useEffect, useMemo, useState } from "react";
import { api } from "../api";
import type { Ctx } from "../App";
import type { IllustratedNode } from "../bindings/IllustratedNode";
import type { ImageThumb } from "../bindings/ImageThumb";
import type { SceneView } from "../bindings/SceneView";
import { ensureThumb } from "../lib/thumbs";
import { useCovers } from "./Covers";
import { mentioned } from "../lib/mentions";

export interface GalleryTarget {
  nodeId: string;
  title: string;
}

export function useIllustrated(worldId: string, tick: number): IllustratedNode[] {
  const [list, setList] = useState<IllustratedNode[]>([]);
  useEffect(() => {
    api.illustratedNodes(worldId).then(setList).catch(() => setList([]));
  }, [worldId, tick]);
  return list;
}

export function ScenePictures({ ctx, campaignId, refreshKey, onOpen }: { ctx: Ctx; campaignId: string; refreshKey: number; onOpen: (t: GalleryTarget) => void }) {
  const [scene, setScene] = useState<SceneView | null>(null);
  const [thumbs, setThumbs] = useState<Record<string, string>>({});
  useEffect(() => {
    let live = true;
    api
      .sceneView(ctx.worldId, campaignId)
      .then(async (s) => {
        if (!live) return;
        setScene(s);
        const got = await Promise.all(s.covers.map(async (c) => [c.node_id, await ensureThumb(ctx.worldId, c)] as const));
        if (live) setThumbs(Object.fromEntries(got.filter((x): x is readonly [string, string] => !!x[1])));
      })
      .catch(() => live && setScene(null));
    return () => {
      live = false;
    };
  }, [ctx.worldId, campaignId, refreshKey]);

  // 地点是大图：用原图（缩略图放大会糊），读到之前先显示缩略图。
  const [locFull, setLocFull] = useState<string | null>(null);
  useEffect(() => {
    setLocFull(null);
    const cover = scene?.location ? scene.covers.find((c) => c.node_id === scene.location!.node_id) : undefined;
    if (!cover) return;
    let live = true;
    api
      .nodeImageData(ctx.worldId, cover.image_id)
      .then((u) => live && setLocFull(u))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [ctx.worldId, scene]);

  if (!scene) return null;
  const loc = scene.location && thumbs[scene.location.node_id] ? scene.location : null;
  const people = scene.npcs.filter((n) => thumbs[n.node_id]);
  if (!loc && !people.length) return null;
  return (
    <div className="scene-pics" aria-label="此刻的画面">
      {loc && (
        <button className="scene-loc" onClick={() => onOpen({ nodeId: loc.node_id, title: loc.title })} title={`查看「${loc.title}」的图片`}>
          <img src={locFull ?? thumbs[loc.node_id]} alt="" />
          <span>{loc.title}</span>
        </button>
      )}
      {people.length > 0 && (
        <div className="scene-people">
          {people.map((p) => (
            <button key={p.node_id} className="scene-person" onClick={() => onOpen({ nodeId: p.node_id, title: p.title })} title={`查看「${p.title}」的图片`}>
              <img src={thumbs[p.node_id]} alt="" />
              <span>{p.title}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

/** 一段叙述下方：提到的、配了图的设定的小缩略图。 */
export function MentionThumbs({ ctx, text, nodes, onOpen }: { ctx: Ctx; text: string; nodes: IllustratedNode[]; onOpen: (t: GalleryTarget) => void }) {
  const hits = useMemo(() => mentioned(text, nodes), [text, nodes]);
  const covers = useCovers(ctx.worldId, hits.map((h) => h.node_id), ctx.tick);
  const shown = hits.filter((h) => covers[h.node_id]);
  if (!shown.length) return null;
  return (
    <span className="mention-pics">
      {shown.map((h) => (
        <button key={h.node_id} className="mention-pic" onClick={() => onOpen({ nodeId: h.node_id, title: h.title })} title={`${h.title}（${h.count} 张图片）`}>
          <img src={covers[h.node_id]} alt="" />
          <span>{h.title}</span>
        </button>
      ))}
    </span>
  );
}

/** 一个节点的全部图片：大图 + 下方缩略图切换。 */
export function NodeGallery({ ctx, target, gmView, onClose }: { ctx: Ctx; target: GalleryTarget; gmView: boolean; onClose: () => void }) {
  const [list, setList] = useState<ImageThumb[]>([]);
  const [thumbs, setThumbs] = useState<Record<string, string>>({});
  const [cur, setCur] = useState(0);
  const [full, setFull] = useState<Record<string, string>>({});
  useEffect(() => {
    api
      .nodeImageThumbs(ctx.worldId, target.nodeId)
      .then(async (l) => {
        setList(l);
        setCur(0);
        const got = await Promise.all(l.map(async (t) => [t.image_id, await ensureThumb(ctx.worldId, t)] as const));
        setThumbs(Object.fromEntries(got.filter((x): x is readonly [string, string] => !!x[1])));
      })
      .catch((e: Error) => ctx.notify(e.message, "error"));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ctx.worldId, target.nodeId]);
  const img = list[cur];
  useEffect(() => {
    if (img && !full[img.image_id]) {
      api
        .nodeImageData(ctx.worldId, img.image_id)
        .then((u) => setFull((m) => ({ ...m, [img.image_id]: u })))
        .catch(() => {});
    }
  }, [ctx.worldId, img, full]);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
      if (e.key === "ArrowRight") setCur((c) => Math.min(list.length - 1, c + 1));
      if (e.key === "ArrowLeft") setCur((c) => Math.max(0, c - 1));
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [list.length, onClose]);

  return (
    <div className="lightbox gallery" onClick={onClose} role="dialog" aria-label={`${target.title} 的图片`}>
      <div className="gallery-title">
        {target.title}
        {list.length > 1 && <small> · {cur + 1} / {list.length}</small>}
      </div>
      {img && (full[img.image_id] || thumbs[img.image_id]) ? <img src={full[img.image_id] ?? thumbs[img.image_id]} alt="" onClick={(e) => e.stopPropagation()} /> : <SkeletonBlock w={360} h={240} radius={12} />}
      {gmView && img?.caption && <div className="lightbox-cap">{img.caption}</div>}
      {list.length > 1 && (
        <div className="gallery-strip" onClick={(e) => e.stopPropagation()}>
          {list.map((t, i) => (
            <button key={t.image_id} className={i === cur ? "active" : ""} onClick={() => setCur(i)}>
              {thumbs[t.image_id] && <img src={thumbs[t.image_id]} alt="" />}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
