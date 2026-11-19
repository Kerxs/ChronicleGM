// 仪式演出层：程序判定的重要时刻（进入新地点、人物首次登场、关键线索、战斗开始、濒死、转折、序章、结局）
// 依次演出。三级是转场卡片 + 专属音效；四级是全屏演出 + 音乐（演出强度为"简化"时四级也只用卡片，"关闭"时不演出）。
// 点击或按 Esc 跳过；低性能模式只保留文字卡片。
import { useEffect, useState } from "react";
import type { RitualCue } from "../../bindings/RitualCue";
import { fxSettings, play, startMusic } from "../../lib/sfx";
import { api } from "../../api";

const SCENE_LABEL: Record<string, string> = {
  prologue: "序章",
  location: "抵达",
  npc: "登场",
  item: "获得",
  clue: "发现线索",
  combat: "遭遇",
  near_death: "危急",
  turning: "转折",
  ending: "终章",
};

/** 从队列里挑要演出的：一级不演出（骰子和小卡片已经负责）；同一回合最多三个，四级优先。 */
export function pickCues(cues: RitualCue[], level: "full" | "simple" | "off"): RitualCue[] {
  if (level === "off") return [];
  return cues
    .filter((c) => c.level >= 3)
    .sort((a, b) => b.level - a.level)
    .slice(0, 3)
    .map((c) => (level === "simple" ? { ...c, level: Math.min(c.level, 3) } : c));
}

export default function RitualLayer({ worldId, queue, lowPower, onDone }: { worldId: string; queue: RitualCue[]; lowPower: boolean; onDone: (n: number) => void }) {
  const cue = queue[0];
  const [img, setImg] = useState<string | null>(null);
  const [leaving, setLeaving] = useState(false);
  useEffect(() => {
    if (!cue) return;
    setLeaving(false);
    setImg(null);
    play(cue.sound);
    if (cue.level >= 4 && !lowPower) void startMusic(cue.scene === "ending" ? "ending" : cue.scene === "prologue" ? "opening" : "explore");
    if (cue.scene === "combat") void startMusic("combat");
    if (cue.node_id) {
      api
        .nodeImages(worldId, cue.node_id)
        .then((list) => {
          const first = [...list].sort((a, b) => a.sort - b.sort)[0];
          if (first) return api.nodeImageData(worldId, first.id).then(setImg);
        })
        .catch(() => {});
    }
    const ms = cue.level >= 4 ? 5200 : 2600;
    const t1 = window.setTimeout(() => setLeaving(true), ms - 500);
    const t2 = window.setTimeout(() => onDone(1), ms);
    return () => {
      clearTimeout(t1);
      clearTimeout(t2);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cue]);
  useEffect(() => {
    const k = (e: KeyboardEvent) => e.key === "Escape" && cue && onDone(queue.length);
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, [cue, queue.length, onDone]);
  if (!cue || fxSettings.level === "off") return null;
  const epic = cue.level >= 4 && fxSettings.level === "full";
  return (
    <div className={`ritual ${epic ? "epic" : "card"} scene-${cue.scene}${leaving ? " leaving" : ""}${lowPower ? " low" : ""}`} onClick={() => onDone(1)} role="status" aria-live="polite">
      {epic && <div className="ritual-veil" />}
      <div className="ritual-body">
        {img && <img className="ritual-portrait" src={img} alt="" />}
        <div className="ritual-kicker">{SCENE_LABEL[cue.scene] ?? ""}</div>
        <div className="ritual-title">{cue.title}</div>
        {cue.subtitle && <div className="ritual-sub">{cue.subtitle}</div>}
        {epic && <div className="ritual-skip">点击或按 Esc 跳过</div>}
      </div>
    </div>
  );
}
