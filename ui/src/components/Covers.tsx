// 封面缩略图：按需取一批节点的封面（只取还没问过的），世界树、图谱、游戏页共用。
import { useEffect, useRef, useState } from "react";
import { coverThumbs } from "../lib/thumbs";

export function useCovers(worldId: string, ids: string[], tick = 0): Record<string, string> {
  const [map, setMap] = useState<Record<string, string>>({});
  const asked = useRef(new Set<string>());
  const world = useRef(worldId);
  useEffect(() => {
    // 世界或数据变了：重新问一遍（不清空现有的，避免闪烁；结果回来再覆盖）。
    asked.current = new Set();
    if (world.current !== worldId) setMap({});
    world.current = worldId;
  }, [worldId, tick]);
  const key = ids.join(",");
  useEffect(() => {
    const need = ids.filter((i) => !asked.current.has(i));
    if (!need.length) return;
    need.forEach((i) => asked.current.add(i));
    const w = worldId;
    coverThumbs(w, need)
      .then((got) => {
        if (world.current !== w) return;
        setMap((m) => {
          const next = { ...m };
          for (const id of need) {
            if (got[id]) next[id] = got[id];
            else delete next[id];
          }
          return next;
        });
      })
      .catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [worldId, key, tick]);
  return map;
}

export function Cover({ src, size = 22, round = false, title }: { src?: string; size?: number; round?: boolean; title?: string }) {
  if (!src) return null;
  // 显示大小随「缩略图大小」设置缩放（--thumb-k）。
  const px = `calc(${size}px * var(--thumb-k, 1))`;
  return <img className={`cover-thumb${round ? " round" : ""}`} src={src} alt="" title={title} style={{ width: px, height: px }} draggable={false} />;
}
