// 运行监控：最近一次请求的首字时间、生成速度、提示词处理速度、上下文占用；历史统计（平均 / 最快 / 最慢）。
// 拿不到的数据显示「不可用」（例如非流式请求没有首字时间，云端服务通常不返回速度）。
import { useEffect, useState } from "react";
import { api } from "../api";
import type { MetricStat } from "../bindings/MetricStat";
import type { RecentRequest } from "../bindings/RecentRequest";
import type { RunStats } from "../bindings/RunStats";

const TASK: Record<string, string> = { narration: "GM 叙述", npc: "NPC", script_gen: "剧本生成", gen: "生成任务", director: "导演", tts: "语音", summary: "摘要", vision: "看图" };
const NA = <span className="na">不可用</span>;

const sec = (ms: number | null | undefined) => (ms == null ? NA : <>{(ms / 1000).toFixed(ms < 10000 ? 2 : 1)} 秒</>);
const tps = (v: number | null | undefined) => (v == null ? NA : <>{v.toFixed(v < 10 ? 1 : 0)} token/秒</>);
const pct = (v: number | null | undefined) => (v == null ? NA : <>{Math.round(v * 100)}%</>);

function Stat({ s, fmt }: { s: MetricStat; fmt: (v: number) => string }) {
  if (!s.n || s.avg == null) return NA;
  return (
    <span title={`${s.n} 次`}>
      {fmt(s.avg)}
      <small className="hint">
        {" "}
        （{fmt(s.min ?? s.avg)}～{fmt(s.max ?? s.avg)}）
      </small>
    </span>
  );
}

export default function RunMonitor({ compactDefault = true, openDefault = true, refreshKey = 0 }: { compactDefault?: boolean; openDefault?: boolean; refreshKey?: number }) {
  const [open, setOpen] = useState(openDefault);
  const [compact, setCompact] = useState(compactDefault);
  const [hours, setHours] = useState(24);
  const [data, setData] = useState<[RecentRequest[], RunStats[]] | null>(null);
  useEffect(() => {
    if (!open) return;
    api.runMonitor(hours, 20).then(setData).catch(() => setData([[], []]));
  }, [hours, refreshKey, open]);
  const recent = data?.[0] ?? [];
  const llm = recent.find((r) => r.task !== "tts" && r.ok);
  const tts = recent.find((r) => r.task === "tts" && r.ok);
  return (
    <div className="card run-monitor">
      <div className="row">
        <button className="link" onClick={() => setOpen(!open)} aria-expanded={open}>
          {open ? "▾" : "▸"} 运行监控
        </button>
        {open && (
          <>
            <span className="spacer" />
            <div className="seg-mini">
              <button className={compact ? "on" : ""} onClick={() => setCompact(true)}>
                简洁
              </button>
              <button className={!compact ? "on" : ""} onClick={() => setCompact(false)}>
                详细
              </button>
            </div>
          </>
        )}
      </div>
      {open && (
        <>
          {!llm && !tts && <p className="hint">还没有请求记录。</p>}
          {llm && (
            <div className="mon-grid">
              <span className="hint">最近一次（{TASK[llm.task] ?? llm.task}）</span>
              <span>
                首字 {sec(llm.first_token_ms)} · 总耗时 {sec(llm.latency_ms)}
              </span>
              <span>
                生成 {tps(llm.predicted_per_second)} · 提示词 {tps(llm.prompt_per_second)}
              </span>
              <span>
                输入 {llm.prompt_tokens ?? "—"} / 输出 {llm.completion_tokens ?? "—"} token · 上下文 {llm.prompt_tokens != null && llm.context_length ? pct(llm.prompt_tokens / llm.context_length) : NA}
              </span>
            </div>
          )}
          {tts && (
            <div className="mon-grid">
              <span className="hint">最近一次语音</span>
              <span>
                合成 {sec(tts.latency_ms)} · 音频 {sec(tts.audio_ms)} · 实时率 {tts.audio_ms ? (tts.latency_ms! / tts.audio_ms).toFixed(2) : NA}
              </span>
            </div>
          )}
          {!compact && (
            <>
              <div className="row">
                <span className="hint">历史统计（成功的请求；括号里是最快～最慢）</span>
                <span className="spacer" />
                <select value={hours} onChange={(e) => setHours(Number(e.target.value))}>
                  <option value={1}>最近 1 小时</option>
                  <option value={24}>最近 24 小时</option>
                  <option value={168}>最近 7 天</option>
                  <option value={0}>全部</option>
                </select>
              </div>
              <div className="mon-table">
                <table>
                  <thead>
                    <tr>
                      <th>任务</th>
                      <th>次数</th>
                      <th>耗时</th>
                      <th>首字</th>
                      <th>生成速度</th>
                      <th>提示词速度</th>
                      <th>上下文占用</th>
                    </tr>
                  </thead>
                  <tbody>
                    {(data?.[1] ?? []).map((s) => (
                      <tr key={`${s.provider_id}-${s.task}`}>
                        <td title={s.provider_id}>{TASK[s.task] ?? s.task}</td>
                        <td>
                          {s.requests}
                          {s.failures > 0 && <small className="warn-text">（失败 {s.failures}）</small>}
                        </td>
                        <td>
                          <Stat s={s.latency_ms} fmt={(v) => (v / 1000).toFixed(1) + "s"} />
                        </td>
                        <td>{s.task === "tts" ? <Stat s={s.tts_rtf} fmt={(v) => "RTF " + v.toFixed(2)} /> : <Stat s={s.first_token_ms} fmt={(v) => (v / 1000).toFixed(2) + "s"} />}</td>
                        <td>
                          <Stat s={s.predicted_per_second} fmt={(v) => v.toFixed(1)} />
                        </td>
                        <td>
                          <Stat s={s.prompt_per_second} fmt={(v) => v.toFixed(0)} />
                        </td>
                        <td>
                          <Stat s={s.context_usage} fmt={(v) => Math.round(v * 100) + "%"} />
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </>
          )}
        </>
      )}
    </div>
  );
}
