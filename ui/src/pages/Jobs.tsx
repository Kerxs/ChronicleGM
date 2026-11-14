import { Fragment, useEffect, useState } from "react";
import { api, formatTime } from "../api";
import type { Ctx } from "../App";
import type { GenJob } from "../bindings/GenJob";
import { JOB_KIND, JOB_STATUS } from "../lib/labels";

export default function JobsPage({ ctx }: { ctx: Ctx }) {
  const [jobs, setJobs] = useState<GenJob[]>([]);
  const [open, setOpen] = useState<string | null>(null);

  useEffect(() => {
    const load = () => api.listJobs(ctx.worldId).then(setJobs).catch((e: Error) => ctx.notify(e.message, "error"));
    void load();
    // 有任务运行时定期刷新状态。
    const running = Object.keys(ctx.progress).length > 0;
    if (!running) return;
    const t = setInterval(load, 3000);
    return () => clearInterval(t);
  }, [ctx.worldId, ctx.tick, Object.keys(ctx.progress).length]);

  async function retry(id: string) {
    try {
      await api.retryJob(ctx.worldId, id);
      ctx.notify("已重新开始");
      ctx.bump();
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }

  return (
    <section>
      <h2>生成任务</h2>
      <p className="hint">任务失败不会改动世界内容；原始输出会保留，可以查看或重试。</p>
      {jobs.length === 0 && <p className="hint">还没有任务。</p>}
      <table className="jobs">
        <thead>
          <tr>
            <th>任务</th>
            <th>状态</th>
            <th>尝试</th>
            <th>模型</th>
            <th>时间</th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          {jobs.map((j) => {
            const chars = ctx.progress[j.id];
            return (
              <Fragment key={j.id}>
                <tr className={`st-${j.status}`}>
                  <td>
                    <button className="link" onClick={() => setOpen(open === j.id ? null : j.id)}>
                      {JOB_KIND[j.kind] ?? j.kind}
                    </button>
                    {j.note && <div className="hint">{j.note}</div>}
                    {j.error && <div className="err">{j.error}</div>}
                  </td>
                  <td>
                    {JOB_STATUS[j.status]}
                    {chars !== undefined && `（已接收 ${chars} 字）`}
                  </td>
                  <td>{j.attempts}</td>
                  <td>{j.model || "—"}</td>
                  <td>{formatTime(j.updated_at)}</td>
                  <td className="row">
                    {j.status === "running" && <button onClick={() => void api.cancelJob(j.id)}>取消</button>}
                    {(j.status === "failed" || j.status === "cancelled") && <button onClick={() => void retry(j.id)}>重试</button>}
                    {j.changeset_id && <button onClick={() => ctx.go({ name: "review", id: j.changeset_id! })}>去审阅</button>}
                  </td>
                </tr>
                {open === j.id && (
                  <tr>
                    <td colSpan={6}>
                      <details open>
                        <summary>参数</summary>
                        <pre className="mono">{JSON.stringify(j.params, null, 2)}</pre>
                      </details>
                      <details>
                        <summary>模型原始输出</summary>
                        <pre className="mono">{j.raw_output ?? "（无）"}</pre>
                      </details>
                      <p className="hint">
                        模板 {j.template_version || "—"} · 上下文摘要 {j.context_digest || "—"}
                      </p>
                    </td>
                  </tr>
                )}
              </Fragment>
            );
          })}
        </tbody>
      </table>
    </section>
  );
}
