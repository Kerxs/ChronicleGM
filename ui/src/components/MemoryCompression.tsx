// 上下文压缩：设置（自动 / 阈值）、立即压缩；自动摘要显示压缩前后的 token 数，可以确认、重新生成、删除。
import { useEffect, useState } from "react";
import { api } from "../api";
import type { Ctx } from "../App";
import type { MemorySettings } from "../bindings/MemorySettings";
import type { SessionSummary } from "../bindings/SessionSummary";

export function AutoSummary({ ctx, campaignId, s, latest }: { ctx: Ctx; campaignId: string; s: SessionSummary; latest: boolean }) {
  const [busy, setBusy] = useState(false);
  const run = (p: Promise<unknown>, ok: string) => {
    setBusy(true);
    p.then(() => {
      ctx.notify(ok);
      ctx.bump();
    })
      .catch((e: Error) => ctx.notify(e.message, "error"))
      .finally(() => setBusy(false));
  };
  return (
    <div className="auto-summary">
      <div className="row">
        <strong>自动压缩的摘要</strong>
        <span className="badge">GM 会参考</span>
        {s.tokens_before != null && s.tokens_after != null && (
          <small className="hint" title={s.token_method === "measured" ? "服务端分词实测" : "估算（服务不支持分词）"}>
            {s.tokens_before} → {s.tokens_after} token{s.token_method === "measured" ? "" : "（估算）"}
          </small>
        )}
      </div>
      <p>{s.summary}</p>
      <div className="row">
        <button className="small" disabled={busy} onClick={() => run(api.confirmSummary(ctx.worldId, s.id, null), "已确认")}>
          确认
        </button>
        {latest && (
          <>
            <button className="small" disabled={busy} onClick={() => run(api.compressMemory(ctx.worldId, campaignId, s.id), "已重新生成")}>
              {busy ? "处理中…" : "重新生成"}
            </button>
            <button className="small danger" disabled={busy} title="删除摘要；原始游戏记录一直保留" onClick={() => run(api.deleteSummary(ctx.worldId, campaignId, s.id), "已删除，之后会重新压缩")}>
              删除
            </button>
          </>
        )}
      </div>
    </div>
  );
}

export default function MemoryCompression({ ctx, campaignId }: { ctx: Ctx; campaignId: string }) {
  const [cfg, setCfg] = useState<MemorySettings | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    api.memorySettings(ctx.worldId, campaignId).then(setCfg).catch(() => setCfg(null));
  }, [ctx.worldId, campaignId, ctx.tick]);
  if (!cfg) return null;
  const save = (c: MemorySettings) => {
    setCfg(c);
    void api.setMemorySettings(ctx.worldId, campaignId, c).catch((e: Error) => ctx.notify(e.message, "error"));
  };
  return (
    <div className="memory-cfg">
      <label className="check" title="最近 8 条对话原样保留；更早的、还没摘要过的记录超过阈值时，在后台整理成摘要。物品、骰子、决定等事实另有记录，不会被压缩">
        <input type="checkbox" checked={cfg.auto} onChange={(e) => save({ ...cfg, auto: e.target.checked })} />
        自动压缩上下文
      </label>
      <label className="check">
        超过
        <input type="number" min={300} max={20000} step={100} value={cfg.threshold_tokens} onChange={(e) => setCfg({ ...cfg, threshold_tokens: Number(e.target.value) })} onBlur={() => save(cfg)} style={{ width: 80 }} />
        token 时压缩
      </label>
      <button
        className="small"
        disabled={busy}
        onClick={() => {
          setBusy(true);
          api
            .compressMemory(ctx.worldId, campaignId, null)
            .then((s) => {
              ctx.notify(`已压缩：${s.tokens_before} → ${s.tokens_after} token`);
              ctx.bump();
            })
            .catch((e: Error) => ctx.notify(e.message, "error"))
            .finally(() => setBusy(false));
        }}
      >
        {busy ? "压缩中…" : "立即压缩"}
      </button>
    </div>
  );
}
