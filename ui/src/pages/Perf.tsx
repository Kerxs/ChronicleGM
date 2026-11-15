import { PageSkeleton } from "../components/Skeleton";
import { useCallback, useEffect, useState } from "react";
import { api, formatTime, type DbProbe, type PerfSnapshot } from "../api";
import type { ModelProfile } from "../bindings/ModelProfile";
import type { ModelRole } from "../bindings/ModelRole";
import type { MemEstimate } from "../bindings/MemEstimate";
import type { Priority } from "../bindings/Priority";
import type { ProcState } from "../bindings/ProcState";
import type { ProcStatus } from "../bindings/ProcStatus";
import type { TaskInfo } from "../bindings/TaskInfo";
import { parseCommandLine } from "../lib/cmdline";
import LaunchArgs from "../components/LaunchArgs";
import RunMonitor from "../components/RunMonitor";

const PRIORITY: Record<Priority, string> = { P0: "P0 GM 回复", P1: "P1 NPC 对话", P2: "P2 语音", P3: "P3 分析", P4: "P4 记忆与模拟", P5: "P5 生成" };
const MODES: [string, string, string][] = [
  ["eco", "节能", "后台任务一次一个，GM 回复期间不启动后台任务；采样间隔 5 秒；托管模型空闲 3 分钟后释放"],
  ["balanced", "平衡", "后台任务一次一个，GM 回复期间不启动后台任务；采样间隔 2 秒；托管模型空闲 10 分钟后释放"],
  ["performance", "性能", "后台任务可以两个同时运行，并且与 GM 回复并行；采样间隔 1 秒；托管模型不自动释放"],
];
const ROLE: Record<ModelRole, string> = { llm: "大语言模型", tts: "语音合成", asr: "语音识别" };
const TASK_STATE: Record<string, string> = { queued: "排队", running: "运行中", done: "完成", failed: "失败", cancelled: "已取消", timed_out: "超时" };

function procState(s: ProcState): string {
  switch (s.state) {
    case "stopped":
      return "未运行";
    case "starting":
      return "启动中（加载模型）";
    case "running":
      return "运行中";
    case "external":
      return "运行中（不是本软件启动的，不会被停止）";
    case "stopping":
      return "停止中";
    case "crashed":
      return `意外退出（${s.detail}）`;
    case "failed":
      return `启动失败：${s.detail}`;
  }
}

const sec = (ms: number | null | undefined) => (ms == null ? "—" : ms < 1000 ? `${Math.round(ms)} ms` : `${(ms / 1000).toFixed(1)} 秒`);
const gb = (mb: number) => `${(mb / 1024).toFixed(1)} GB`;

export function emptyProfile(): ModelProfile {
  return {
    id: crypto.randomUUID(),
    name: "",
    role: "llm",
    exe: "",
    args: [],
    cwd: "",
    port: 8080,
    health_path: "/health",
    start_timeout_secs: 600,
    auto_restart: true,
    idle_unload_secs: null,
    exclusive_group: "",
    gpu_reserve_mb: 0,
  };
}

export default function PerfPage({ notify, worldId, onEditModels }: { notify: (m: string, k?: "info" | "error") => void; worldId: string | null; onEditModels?: () => void }) {
  const [snap, setSnap] = useState<PerfSnapshot | null>(null);
  const [probe, setProbe] = useState<DbProbe | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const refresh = useCallback(() => {
    api.perfSnapshot().then(setSnap).catch((e: Error) => notify(e.message, "error"));
  }, [notify]);
  useEffect(() => {
    refresh();
    const t = setInterval(refresh, 2000);
    return () => clearInterval(t);
  }, [refresh]);

  if (!snap) return <PageSkeleton />;
  const r = snap.resources;
  const s = snap.sched;

  async function act(label: string, f: () => Promise<unknown>) {
    setBusy(label);
    try {
      await f();
      refresh();
    } catch (e) {
      notify((e as Error).message, "error");
    } finally {
      setBusy(null);
    }
  }

  return (
    <section>
      <h2>性能中心</h2>
      <RunMonitor compactDefault={false} />
      <div className="card">
        <h3>性能模式</h3>
        <div className="row wrap">
          {MODES.map(([k, label]) => (
            <button key={k} className={s.mode === k ? "primary" : ""} onClick={() => void act("mode", () => api.setPerfMode(k))}>
              {label}
            </button>
          ))}
          <label className="check" style={{ marginLeft: 16 }}>
            <input type="checkbox" checked={s.paused_from === "P3"} onChange={(e) => void act("pause", () => api.pauseFrom(e.target.checked ? "P3" : null))} />
            暂停所有后台任务（P3–P5）
          </label>
        </div>
        <p className="hint">{MODES.find((m) => m[0] === s.mode)?.[2]}。GM 回复与 NPC 对话始终有独立的并发位置，不会被后台任务挡住。</p>
        {s.resource_pause && <p className="warn-text">⚠ 资源不足，已自动暂停 {PRIORITY[s.resource_pause[0]]} 及以下：{s.resource_pause[1]}</p>}
      </div>

      <div className="grid2">
        <div className="card">
          <h3>资源</h3>
          {!r && <p className="hint">还没有采样数据。</p>}
          {r && (
            <table className="kv">
              <tbody>
                <tr><th>CPU</th><td>{r.cpu_percent.toFixed(0)}%</td></tr>
                <tr><th>内存</th><td>已用 {gb(r.ram_total_mb - r.ram_available_mb)} / {gb(r.ram_total_mb)}（可用 {gb(r.ram_available_mb)}）</td></tr>
                {r.gpus.map((g, i) => (
                  <tr key={i}>
                    <th>显卡</th>
                    <td>
                      {g.name}：占用 {g.util_percent.toFixed(0)}%，显存 {gb(g.mem_used_mb)} / {gb(g.mem_total_mb)}
                      {g.temp_c != null && `，${g.temp_c.toFixed(0)}°C`}
                    </td>
                  </tr>
                ))}
                {r.gpu_note && <tr><th>显卡</th><td className="hint">{r.gpu_note}</td></tr>}
                {r.disk_free_mb != null && <tr><th>数据磁盘</th><td>剩余 {gb(r.disk_free_mb)} / {gb(r.disk_total_mb ?? 0)}</td></tr>}
                <tr><th>本程序</th><td>内存 {r.app.mem_mb} MB，CPU {r.app.cpu_percent.toFixed(1)}%，磁盘读 {r.app.read_kbps.toFixed(0)} KB/s、写 {r.app.write_kbps.toFixed(0)} KB/s（{r.app.pids.length} 个进程）</td></tr>
                {r.models.map((m) => (
                  <tr key={m.name}>
                    <th>托管模型</th>
                    <td>{snap.models.find((x) => x.profile.id === m.name)?.profile.name ?? m.name}：内存 {gb(m.mem_mb)}，CPU {m.cpu_percent.toFixed(0)}%</td>
                  </tr>
                ))}
                <tr><th>采样时间</th><td>{new Date(r.at).toLocaleTimeString()}</td></tr>
              </tbody>
            </table>
          )}
        </div>

        <div className="card">
          <h3>AI 任务队列（{s.active.length}）</h3>
          {s.active.length === 0 && <p className="hint">当前没有排队或运行中的模型请求。</p>}
          <TaskTable tasks={s.active} actions={(t) => (
            <>
              {t.state === "queued" && (
                <button className="link" onClick={() => void act("p", () => api.taskPause(t.id, !t.paused))}>{t.paused ? "恢复" : "暂停"}</button>
              )}
              <button className="link" onClick={() => void act("c", () => api.taskCancel(t.id))}>取消</button>
            </>
          )} />
          <h3>按类型统计（最近 500 个请求）</h3>
          <table className="data">
            <thead>
              <tr><th>类型</th><th>优先级</th><th>次数</th><th>失败</th><th>平均排队</th><th>最长排队</th><th>平均耗时</th></tr>
            </thead>
            <tbody>
              {s.stats.map((k) => (
                <tr key={k.priority + k.kind}>
                  <td>{k.kind}</td><td>{k.priority}</td><td>{k.count}</td><td>{k.failed}</td><td>{sec(k.avg_wait_ms)}</td><td>{sec(k.max_wait_ms)}</td><td>{sec(k.avg_run_ms)}</td>
                </tr>
              ))}
            </tbody>
          </table>
          {s.stats.length === 0 && <p className="hint">还没有记录。</p>}
          <details>
            <summary>最近完成的请求</summary>
            <TaskTable tasks={s.recent.slice(0, 20)} />
          </details>
        </div>
      </div>

      <div className="card">
        <div className="row">
          <h3 style={{ flex: 1 }}>托管的模型程序</h3>
          {onEditModels && <button onClick={onEditModels}>添加 / 编辑启动参数</button>}
        </div>
        <p className="hint">
          托管模式是可选的：按你填写的程序路径和启动参数启动、停止、重启模型，崩溃后自动重启（5 分钟内最多 3 次）。有请求发往对应端口时按需加载，空闲后按性能模式释放。软件不下载模型，也不替你选择或更换模型。没有托管的服务仍然只做识别。
        </p>
        {snap.models.length === 0 && <p className="hint">还没有托管配置。</p>}
        {snap.models.some((x) => x.profile.role === "tts") &&
          snap.models
            .filter((x) => x.profile.role === "llm" && x.profile.gpu_reserve_mb === 0 && x.profile.args.includes("--fit") && x.profile.args.includes("on"))
            .map((x) => (
              <p key={x.profile.id} className="warn-text">
                「{x.profile.name}」和语音模型在同一张显卡上：它用 --fit on 按加载时的空闲显存分配，语音模型之后多占的显存可能让它显存不足而崩溃（实测 8 GB 显卡上发生过）。
                <button
                  className="small"
                  onClick={() => void act("预留显存", () => api.saveManagedModel({ ...x.profile, gpu_reserve_mb: 1024 })).then(refresh)}
                >
                  预留 1024 MiB（推荐）
                </button>
                <span className="hint">　预留后启动它之前会先启动语音模型；下次启动时生效。</span>
              </p>
            ))}
        <ul className="list">
          {snap.models.map((m) => (
            <ModelRow key={m.profile.id} m={m} busy={busy} onEdit={onEditModels} act={act} />
          ))}
        </ul>
      </div>

      <div className="grid2">
        <div className="card">
          <h3>语音队列与缓存</h3>
          <p className="hint">{snap.voice_note}</p>
        </div>
        <div className="card">
          <h3>数据库与图谱渲染</h3>
          <button disabled={!worldId || busy === "probe"} onClick={() => worldId && void act("probe", async () => setProbe(await api.dbProbe(worldId)))}>
            {worldId ? "测试当前世界的数据库性能" : "先选择一个世界"}
          </button>
          {probe && (
            <table className="data">
              <tbody>
                <tr><td>世界文件</td><td>{probe.file_mb.toFixed(1)} MB（WAL {probe.wal_mb.toFixed(1)} MB），{probe.nodes} 个节点，{probe.events} 条事件</td></tr>
                {probe.timings.map(([k, v]) => (
                  <tr key={k}><td>{k}</td><td>{v.toFixed(2)} ms</td></tr>
                ))}
              </tbody>
            </table>
          )}
          <h4>最近的图谱渲染（界面实测）</h4>
          {snap.render.length === 0 && <p className="hint">打开剧情图或关系图后会在这里记录。</p>}
          <table className="data">
            <tbody>
              {snap.render.slice(0, 10).map((x, i) => (
                <tr key={i}><td>{x.view}</td><td>{x.nodes} 节点 / {x.edges} 边</td><td>{x.ms.toFixed(0)} ms</td><td className="hint">{new Date(x.at).toLocaleTimeString()}</td></tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>
    </section>
  );
}

function TaskTable({ tasks, actions }: { tasks: TaskInfo[]; actions?: (t: TaskInfo) => React.ReactNode }) {
  if (tasks.length === 0) return null;
  const now = Date.now();
  return (
    <table className="data">
      <thead>
        <tr><th>优先级</th><th>类型</th><th>状态</th><th>排队</th><th>耗时</th>{actions && <th />}</tr>
      </thead>
      <tbody>
        {tasks.map((t) => (
          <tr key={t.id} title={t.error ?? t.label}>
            <td>{PRIORITY[t.priority]}</td>
            <td>{t.kind}{t.label && <small className="hint"> {t.label}</small>}</td>
            <td>{TASK_STATE[t.state]}{t.paused && "（已暂停）"}</td>
            <td>{sec((t.started_at ?? now) - t.queued_at)}</td>
            <td>{t.started_at ? sec((t.finished_at ?? now) - t.started_at) : "—"}</td>
            {actions && <td>{actions(t)}</td>}
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function ModelRow({ m, busy, onEdit, act }: { m: ProcStatus; busy: string | null; onEdit?: () => void; act: (l: string, f: () => Promise<unknown>) => Promise<void> }) {
  const st = m.state.state;
  const running = st === "running" || st === "starting";
  return (
    <li className="row wrap" style={{ alignItems: "flex-start" }}>
      <span style={{ flex: 1, minWidth: 260 }}>
        <span className="kind">{ROLE[m.profile.role]}</span>
        <strong>{m.profile.name}</strong> <small className="hint">端口 {m.profile.port}{m.profile.exclusive_group && ` · 互斥组「${m.profile.exclusive_group}」`}</small>
        <div className={st === "failed" || st === "crashed" ? "warn-text" : "hint"}>
          {procState(m.state)}
          {m.pid != null && ` · PID ${m.pid}`}
          {m.last_load_ms != null && ` · 上次加载 ${sec(m.last_load_ms)}`}
          {m.last_used_at != null && ` · 最近使用 ${formatTime(m.last_used_at)}`}
          {m.restarts > 0 && ` · 自动重启 ${m.restarts} 次`}
        </div>
        <small className="hint">日志：{m.log_file}</small>
      </span>
      <button disabled={busy === m.profile.id || running || st === "external"} onClick={() => void act(m.profile.id, () => api.modelStart(m.profile.id, false))}>
        {busy === m.profile.id ? "处理中…" : "启动"}
      </button>
      <button disabled={busy === m.profile.id || st !== "running"} onClick={() => void act(m.profile.id, () => api.modelStart(m.profile.id, true))} title="按当前空闲显存重新加载（llama-server 的 --fit 在加载时适配显存）">
        重启
      </button>
      <button disabled={!running} onClick={() => void act("stop", () => api.modelStop(m.profile.id))}>停止</button>
      {onEdit && <button onClick={onEdit} title="启动参数在「模型服务」里编辑">编辑参数</button>}
    </li>
  );
}

export function ProfileForm({ profile, onCancel, onSaved, notify }: { profile: ModelProfile; onCancel: () => void; onSaved: () => void; notify: (m: string, k?: "info" | "error") => void }) {
  const [p, setP] = useState<ModelProfile>(profile);
  const [cmd, setCmd] = useState("");
  const [est, setEst] = useState<MemEstimate | null | undefined>(undefined);
  const [idleMode, setIdleMode] = useState<"mode" | "never" | "custom">(profile.idle_unload_secs == null ? "mode" : profile.idle_unload_secs === 0 ? "never" : "custom");
  const set = <K extends keyof ModelProfile>(k: K, v: ModelProfile[K]) => setP((x) => ({ ...x, [k]: v }));

  function importCmd() {
    const r = parseCommandLine(cmd);
    if (!r.exe) return notify("没有识别出程序路径", "error");
    setP((x) => ({ ...x, exe: r.exe, args: r.args, port: r.port ?? x.port, name: x.name || r.exe.split(/[\\/]/).pop()?.replace(/\.exe$/i, "") || "" }));
    notify(r.unresolved.length ? `已导入。注意：${r.unresolved.join("、")} 是 bat 里的变量，已忽略；需要的话请把实际参数加到参数列表里` : "已导入程序路径与参数");
  }

  async function save() {
    try {
      const idle = idleMode === "mode" ? null : idleMode === "never" ? 0 : Math.max(30, p.idle_unload_secs ?? 600);
      await api.saveManagedModel({ ...p, idle_unload_secs: idle });
      onSaved();
    } catch (e) {
      notify((e as Error).message, "error");
    }
  }

  return (
    <div className="card" style={{ marginTop: 12 }}>
      <h4>{profile.name ? `编辑「${profile.name}」` : "添加模型程序"}</h4>
      <label>
        从命令行或 .bat 粘贴（可选，只用来填写下面的字段，不会执行）
        <textarea rows={4} value={cmd} onChange={(e) => setCmd(e.target.value)} placeholder={'例如："D:\\llama\\llama-server.exe" -m "D:\\llama\\models\\xxx.gguf" -c 32768 --port 8080'} />
      </label>
      <div className="row wrap">
        <button onClick={importCmd} disabled={!cmd.trim()}>拆分并填入</button>
        <label className="button-like">
          读取 .bat 文件…
          <input
            type="file"
            accept=".bat,.cmd,.txt"
            hidden
            onChange={(e) => {
              const f = e.target.files?.[0];
              e.target.value = "";
              if (f) void f.text().then(setCmd);
            }}
          />
        </label>
        <small className="hint">只读取文本填进上面的框，不会执行。</small>
      </div>
      <div className="grid2" style={{ marginTop: 8 }}>
        <label>名称<input value={p.name} onChange={(e) => set("name", e.target.value)} /></label>
        <label>
          类型
          <select value={p.role} onChange={(e) => set("role", e.target.value as ModelRole)}>
            {Object.entries(ROLE).map(([k, v]) => <option key={k} value={k}>{v}</option>)}
          </select>
        </label>
        <label>程序路径<input value={p.exe} onChange={(e) => set("exe", e.target.value)} placeholder="D:\llama\llama-server.exe" /></label>
        <label>工作目录（可空，默认为程序所在目录）<input value={p.cwd} onChange={(e) => set("cwd", e.target.value)} /></label>
        <label>端口（与「模型服务」里的地址对应）<input type="number" value={p.port} onChange={(e) => set("port", Number(e.target.value))} /></label>
        <label>健康检查路径（空 = 只检查端口）<input value={p.health_path} onChange={(e) => set("health_path", e.target.value)} /></label>
        <label>启动等待上限（秒）<input type="number" value={p.start_timeout_secs} onChange={(e) => set("start_timeout_secs", Number(e.target.value))} /></label>
        <label>互斥组（同组模型不同时运行，例如显存只够放一个；可空）<input value={p.exclusive_group} onChange={(e) => set("exclusive_group", e.target.value)} placeholder="gpu" /></label>
        <label title="参数里有 --fit on、且没有自己写 --fit-target 时，启动时加上 --fit-target（预留 + 1024）。0 表示不改动启动参数。">
          给同一显卡上的其他模型预留显存（MiB，0 = 不改动参数）
          <input type="number" min={0} step={256} value={p.gpu_reserve_mb} onChange={(e) => set("gpu_reserve_mb", Math.max(0, Number(e.target.value)))} />
          {p.role === "llm" && p.args.some((a) => a === "on") && p.args.includes("--fit") && p.gpu_reserve_mb === 0 && (
            <small className="warn-text">
              如果同一张显卡上还托管了语音模型：llama 按加载时的空闲显存分配（--fit on），语音模型之后多占的显存会让它显存不足而崩溃。实测 Breeze TTS（Q8）说话时约占 4.3 GB，比空闲时多约 0.5 GB，建议预留 1024 以上。
            </small>
          )}
        </label>
        <label>
          空闲释放
          <select value={idleMode} onChange={(e) => setIdleMode(e.target.value as typeof idleMode)}>
            <option value="mode">跟随性能模式</option>
            <option value="never">从不自动释放</option>
            <option value="custom">自定义</option>
          </select>
          {idleMode === "custom" && <input type="number" value={p.idle_unload_secs ?? 600} onChange={(e) => set("idle_unload_secs", Number(e.target.value))} />}
        </label>
        <label className="check"><input type="checkbox" checked={p.auto_restart} onChange={(e) => set("auto_restart", e.target.checked)} />崩溃后自动重启</label>
      </div>
      <h4 style={{ marginBottom: 4 }}>启动参数</h4>
      <LaunchArgs profile={p} onArgs={(a) => set("args", a)} />
      <div className="row wrap">
        <button className="primary" onClick={() => void save()}>保存</button>
        <button onClick={onCancel}>取消</button>
        {p.role === "llm" && <button onClick={() => void api.estimateModel(p.args).then(setEst)}>估算显存与内存</button>}
      </div>
      {est === null && <p className="hint">无法估算：参数里没有可读取的 -m 模型文件。</p>}
      {est && (
        <p className="hint">
          估算（不是测量）：权重 {gb(est.weights_mb)} + KV 缓存 {gb(est.kv_cache_mb)}（上下文 {est.context}）+ 计算缓冲约 {gb(est.compute_mb)}
          {est.projector_mb > 0 && ` + 多模态投影 ${gb(est.projector_mb)}`} ≈ <strong>{gb(est.total_mb)}</strong>，由显存与内存共同承担。{est.notes.join("；")}
        </p>
      )}
    </div>
  );
}
