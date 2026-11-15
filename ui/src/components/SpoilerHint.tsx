// 剧透模式提示：关闭时说明哪些内容被隐藏，并可以在这里打开（打开前有警告）。
import { useEffect, useState } from "react";
import { api } from "../api";
import type { Ctx } from "../App";

export default function SpoilerHint({ ctx, what }: { ctx: Ctx; what: string }) {
  const [on, setOn] = useState<boolean | null>(null);
  useEffect(() => {
    api.spoilerMode().then(setOn).catch(() => setOn(null));
  }, [ctx.tick]);
  if (on === null) return null;
  const toggle = () => {
    if (!on && !window.confirm("打开剧透模式？\n会显示秘密、真相和主持人关系。自己当玩家时打开会剧透。这是全局设置。")) return;
    void api
      .setSpoilerMode(!on)
      .then(() => ctx.bump())
      .catch((e: Error) => ctx.notify(e.message, "error"));
  };
  return (
    <div className={`spoiler-hint${on ? " on" : ""}`}>
      {on ? `剧透模式已打开：${what}包含主持人信息。` : `剧透模式已关闭：${what}里主持人可见的关系和秘密已隐藏。`}
      <button className="link" onClick={toggle}>
        {on ? "关闭剧透模式" : "打开剧透模式"}
      </button>
    </div>
  );
}
