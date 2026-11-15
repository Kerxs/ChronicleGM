// 剧本检查：程序检查（立即）+ AI 逻辑检查（按需，会调用模型）。只给问题与建议，不修改剧本。
import { useEffect, useState } from "react";
import { api } from "../api";
import type { Ctx } from "../App";
import type { ScriptFinding } from "../bindings/ScriptFinding";

const SEV: Record<string, string> = { error: "必须处理", warning: "建议补上", info: "提示" };

function List({ items }: { items: ScriptFinding[] }) {
  if (items.length === 0) return <p className="hint">没有发现问题。</p>;
  return (
    <ul className="findings">
      {items.map((f, i) => (
        <li key={i} className={`finding sev-${f.severity}`} style={{ animationDelay: `${i * 0.03}s` }}>
          <span className="sev">{SEV[f.severity] ?? f.severity}</span>
          <div>
            <div>{f.message}</div>
            {f.suggestion && <small className="hint">建议：{f.suggestion}</small>}
          </div>
        </li>
      ))}
    </ul>
  );
}

export default function ScriptCheck({ ctx, scriptId, rev }: { ctx: Ctx; scriptId: string; rev: number }) {
  const [prog, setProg] = useState<ScriptFinding[] | null>(null);
  const [ai, setAi] = useState<ScriptFinding[] | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    api.checkScript(ctx.worldId, scriptId).then(setProg).catch((e: Error) => ctx.notify(e.message, "error"));
  }, [ctx, scriptId, rev]);
  const errors = prog?.filter((f) => f.severity === "error").length ?? 0;
  return (
    <div className="card script-check">
      <div className="row">
        <h3 style={{ margin: 0 }}>检查剧本</h3>
        {prog && <span className={`badge ${errors ? "bad" : "ok"}`}>{errors ? `${errors} 个必须处理的问题` : "可以开始生成世界树和游戏"}</span>}
      </div>
      <h4>程序检查</h4>
      {prog ? <List items={prog} /> : <p className="hint">检查中…</p>}
      <h4>AI 逻辑检查</h4>
      <p className="hint">让模型检查人物设定是否前后一致、剧情是否连贯、任务与结局有没有达成路径、可选角色是否和主线有关联。只给建议，不会修改剧本。</p>
      <button
        disabled={busy}
        onClick={() => {
          setBusy(true);
          api
            .reviewScript(ctx.worldId, scriptId)
            .then(setAi)
            .catch((e: Error) => ctx.notify(e.message, "error"))
            .finally(() => setBusy(false));
        }}
      >
        {busy ? "AI 检查中…（本地模型可能需要一两分钟）" : ai ? "重新检查" : "开始 AI 检查"}
      </button>
      {ai && <List items={ai} />}
    </div>
  );
}
