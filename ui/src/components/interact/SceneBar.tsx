// 场景互动条（输入框上方）：这里能调查、能动手的东西，以及几个常用的亲自操作。
// 点一下就开始互动；不想用也完全可以直接在输入框里写。
import { useEffect, useState } from "react";
import { api } from "../../api";
import type { Ctx } from "../../App";
import type { ObjectView } from "../../bindings/ObjectView";
import type { Interaction } from "../../bindings/Interaction";
import { KIND_ICON } from "./InteractionHost";
import { play } from "../../lib/sfx";

const KIND_LABEL: Record<string, string> = { investigate: "调查", lock: "开锁", puzzle: "摆弄机关", ritual: "进行仪式", item_use: "用物品" };
const OBJ_ICON: Record<string, string> = { door: "🚪", container: "📦", mechanism: "⚙️", body: "💀", document: "📜", furniture: "🪑", window: "🪟", altar: "🕯️", device: "📟", wall: "🧱", floor: "🟫" };

export default function SceneBar({ ctx, campaignId, refreshKey, disabled, onOpened }: { ctx: Ctx; campaignId: string; refreshKey: number; disabled?: boolean; onOpened: (i: Interaction) => void }) {
  const [objects, setObjects] = useState<ObjectView[]>([]);
  const [survival, setSurvival] = useState<[number, number] | null>(null);
  const [alert, setAlert] = useState(0);
  const [menu, setMenu] = useState<string | null>(null);
  useEffect(() => {
    api
      .interactionView(ctx.worldId, campaignId)
      .then((v) => {
        setObjects(v.objects);
        setSurvival(v.survival);
        setAlert(Number(v.alert.level));
      })
      .catch(() => {});
  }, [ctx.worldId, campaignId, refreshKey]);
  const open = (type: string, target = "") => {
    setMenu(null);
    play("click");
    api
      .interactionOpen(ctx.worldId, campaignId, { type, target })
      .then(onOpened)
      .catch((e: Error) => ctx.notify(e.message, "error"));
  };
  return (
    <div className="scene-bar" aria-label="场景互动" data-tour="scene-bar">
      {objects.map((o) => (
        <div key={o.key} className="sb-obj">
          <button className={`sb-chip${o.state.opened || o.state.solved ? " done" : ""}`} disabled={disabled} onClick={() => setMenu(menu === o.key ? null : o.key)} title={o.object.description}>
            <span>{OBJ_ICON[o.object.kind] ?? "✦"}</span> {o.object.name}
            {o.object.lock && !o.state.opened && !o.state.broken && <small>🔒</small>}
            {o.state.searched > 0 && <small title="搜过">✓</small>}
          </button>
          {menu === o.key && (
            <div className="sb-menu anim-pop">
              {o.can.map((k) => (
                <button key={k} onClick={() => open(k, o.key)}>
                  {KIND_ICON[k]} {KIND_LABEL[k] ?? k}
                </button>
              ))}
            </div>
          )}
        </div>
      ))}
      <span className="sb-sep" />
      <button className="sb-chip ghost" disabled={disabled} onClick={() => open("investigate")} title="调查四周（可能发现隐藏的东西）">
        🔍 四周
      </button>
      <button className="sb-chip ghost" disabled={disabled} onClick={() => open("item_use")} title="把背包里的东西用在某处">
        🎒 物品
      </button>
      <button className="sb-chip ghost" disabled={disabled} onClick={() => open("combine")} title="把几样东西组合起来">
        🧪 组合
      </button>
      <button className={`sb-chip ghost${alert > 0 ? " warn" : ""}`} disabled={disabled} onClick={() => open("stealth")} title="潜行（警戒会随你的行动变化）">
        👣 潜行{alert > 0 ? `（${["", "怀疑", "搜索", "发现"][alert]}）` : ""}
      </button>
      <button className="sb-chip ghost" disabled={disabled} onClick={() => open("medical", "self")} title="处理自己的伤">
        🩹 医疗
      </button>
      {survival && (
        <button className={`sb-chip ghost${survival[0] < 25 || survival[1] < 25 ? " warn" : ""}`} disabled={disabled} onClick={() => open("survival")} title={`食物 ${survival[0]}/100，水 ${survival[1]}/100`}>
          🔥 生存 <small>🍖{survival[0]} 💧{survival[1]}</small>
        </button>
      )}
    </div>
  );
}
