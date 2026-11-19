// 本地模型（内置 llama.cpp）：运行状态、模型库、模型市场、下载管理、运行环境。
// 所有下载、安装、推理都在 Rust 后台线程；这里只发命令、显示状态。
import { useCallback, useEffect, useMemo, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import {
  api,
  type LlmBackend,
  type LlmHardware,
  type LlmModelDetail,
  type LlmModelEntry,
  type LlmModelSummary,
  type LlmOverrides,
  type LlmRuntimeView,
  type LlmServeView,
  type LlmTask,
  type LlmTuned,
} from "../api";
import { Segmented } from "../components/Segmented";
import "../models.css";

type Tab = "serve" | "library" | "market" | "downloads" | "runtime";
type Notify = (m: string, k?: "info" | "error") => void;

export const DEFAULT_REPO = "mradermacher/Rocinante-X-12B-v1-Heretic-Uncensored-GGUF";
export const DEFAULT_FILE = "Rocinante-X-12B-v1-Heretic-Uncensored.Q4_K_M.gguf";

export function fmtSize(b: number | null | undefined): string {
  if (!b) return "—";
  const u = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  let v = b;
  while (v >= 1024 && i < u.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(i >= 3 ? 2 : i >= 2 ? 1 : 0)} ${u[i]}`;
}
export function fmtEta(s: number | null | undefined): string {
  if (s == null || !isFinite(s)) return "—";
  if (s < 60) return `${Math.round(s)} 秒`;
  if (s < 3600) return `${Math.floor(s / 60)} 分 ${Math.round(s % 60)} 秒`;
  return `${Math.floor(s / 3600)} 小时 ${Math.round((s % 3600) / 60)} 分`;
}
export function backendLabel(b: LlmBackend | undefined | null): string {
  if (!b) return "—";
  return b.kind === "cuda" ? `CUDA ${b.major}.${b.minor}` : b.kind === "vulkan" ? "Vulkan" : b.kind === "metal" ? "Metal" : "CPU";
}

export default function LocalModels({ notify }: { notify: Notify }) {
  const [tab, setTab] = useState<Tab>("serve");
  const [downloads, setDownloads] = useState<LlmTask[]>([]);
  const active = downloads.filter((t) => ["queued", "running", "retrying"].includes(t.state.state)).length;
  useEffect(() => {
    let alive = true;
    const tick = () => api.llmDownloads().then((d) => alive && setDownloads(d)).catch(() => {});
    tick();
    const h = setInterval(tick, 1000);
    return () => {
      alive = false;
      clearInterval(h);
    };
  }, []);
  return (
    <div className="lm-page">
      <Segmented<Tab>
        items={[
          { key: "serve", label: "运行状态" },
          { key: "library", label: "模型库" },
          { key: "market", label: "模型市场" },
          { key: "downloads", label: "下载管理", count: active },
          { key: "runtime", label: "运行环境" },
        ]}
        value={tab}
        onChange={setTab}
      />
      {tab === "serve" && <ServePanel notify={notify} onGo={setTab} />}
      {tab === "library" && <LibraryPanel notify={notify} onGo={setTab} />}
      {tab === "market" && <MarketPanel notify={notify} onQueued={() => setTab("downloads")} />}
      {tab === "downloads" && <DownloadsPanel tasks={downloads} notify={notify} />}
      {tab === "runtime" && <RuntimePanel notify={notify} />}
    </div>
  );
}

// ====================== 运行状态 ======================

function ServePanel({ notify, onGo }: { notify: Notify; onGo: (t: Tab) => void }) {
  const [v, setV] = useState<LlmServeView | null>(null);
  const [models, setModels] = useState<LlmModelEntry[]>([]);
  const [pick, setPick] = useState<string>("");
  const [ov, setOv] = useState<LlmOverrides>({});
  const [preview, setPreview] = useState<LlmTuned | null>(null);
  const [busy, setBusy] = useState(false);
  const refresh = useCallback(() => {
    api.llmServeStatus().then(setV).catch(() => {});
  }, []);
  useEffect(() => {
    refresh();
    const h = setInterval(refresh, 2000);
    api.llmLibrary().then((l) => {
      setModels(l.data.models);
      setPick((p) => p || l.data.default || l.data.models[0]?.id || "");
    });
    return () => clearInterval(h);
  }, [refresh]);
  useEffect(() => {
    if (!pick) return setPreview(null);
    api.llmTune(pick, ov).then(setPreview).catch((e: Error) => notify(e.message, "error"));
  }, [pick, ov, notify]);

  const running = !!v?.process?.pid;
  const st = v?.process?.state;
  const stLabel = running ? (v?.stats.healthy ? (v.stats.busy ? "推理中" : "就绪") : "加载中") : typeof st === "object" && st ? `已停止（${Object.values(st as Record<string, string>)[0]}）` : "已停止";
  const gpu = v?.resources?.gpus?.[0];
  const me = v?.resources?.models?.[0];
  const start = async () => {
    setBusy(true);
    try {
      const t = await api.llmServeStart(pick, ov);
      notify(`已启动：GPU 层 ${t.gpu_layers}/${t.total_layers}，上下文 ${t.ctx}`);
      refresh();
    } catch (e) {
      notify(e instanceof Error ? e.message : String(e), "error");
    } finally {
      setBusy(false);
    }
  };

  if (models.length === 0)
    return (
      <div className="lm-card lm-empty" data-tour="local-serve">
        <h3>还没有本地模型</h3>
        <p className="hint">推荐先下载默认模型 Rocinante-X-12B（Q4_K_M，约 7 GB）。也可以到模型库添加电脑里已有的 GGUF 文件。</p>
        <div className="lm-row">
          <button className="primary" onClick={() => api.llmDownloadDefault().then(() => onGo("downloads")).catch((e: Error) => notify(e.message, "error"))}>
            下载默认模型
          </button>
          <button onClick={() => onGo("library")}>添加已有文件</button>
          <button onClick={() => onGo("runtime")}>安装运行环境</button>
        </div>
      </div>
    );

  return (
    <div className="lm-grid">
      <div className="lm-card" data-tour="local-serve">
        <h3>推理服务</h3>
        <dl className="lm-kv">
          <dt>状态</dt>
          <dd className={running ? "ok" : ""}>{stLabel}</dd>
          <dt>模型</dt>
          <dd>{v?.model?.name ?? "—"}</dd>
          <dt>llama.cpp</dt>
          <dd>{v?.runtime ? `${v.runtime.tag}` : "未安装"}</dd>
          <dt>推理后端</dt>
          <dd>{backendLabel(v?.runtime?.backend)}</dd>
          <dt>地址</dt>
          <dd>{v?.config.port ? `127.0.0.1:${v.config.port}（只有本机能访问）` : "—"}</dd>
          <dt>生成速度</dt>
          <dd>{v?.stats.gen_tps ? `${v.stats.gen_tps.toFixed(1)} tokens/s` : "—"}</dd>
          <dt>读提示速度</dt>
          <dd>{v?.stats.prompt_tps ? `${v.stats.prompt_tps.toFixed(0)} tokens/s` : "—"}</dd>
          <dt>上下文</dt>
          <dd>{v?.stats.ctx ? `${v.stats.ctx_used ?? 0} / ${v.stats.ctx}` : "—"}</dd>
          <dt>显存</dt>
          <dd>{gpu ? `${fmtSize(gpu.mem_used_mb * 1048576)} / ${fmtSize(gpu.mem_total_mb * 1048576)}（显卡占用 ${gpu.util_percent.toFixed(0)}%）` : "—"}</dd>
          <dt>模型进程内存</dt>
          <dd>{running && me ? fmtSize((me.private_mb || me.mem_mb) * 1048576) : "—"}</dd>
          <dt>CPU</dt>
          <dd>{running && me ? `${me.cpu_percent.toFixed(0)}%` : v?.resources ? `整机 ${v.resources.cpu_percent.toFixed(0)}%` : "—"}</dd>
        </dl>
        <div className="lm-row">
          <button disabled={!running || busy} onClick={() => api.llmServeStop().then(refresh).catch((e: Error) => notify(e.message, "error"))}>
            停止
          </button>
          <button disabled={busy || !v?.model} onClick={() => v?.model && api.llmServeStart(v.model.id, ov).then(refresh).catch((e: Error) => notify(e.message, "error"))}>
            重新加载
          </button>
        </div>
        <p className="hint">游戏里的 AI GM 默认用这个服务；用到时会自动启动。</p>
      </div>
      <div className="lm-card">
        <h3>启动 / 切换模型</h3>
        <label>
          模型
          <select value={pick} onChange={(e) => setPick(e.target.value)}>
            {models.map((m) => (
              <option key={m.id} value={m.id}>
                {m.name} {m.quant ? `· ${m.quant}` : ""} · {fmtSize(m.size)}
              </option>
            ))}
          </select>
        </label>
        <details className="lm-adv">
          <summary>参数（默认全部自动）</summary>
          <label>
            上下文长度
            <input type="number" placeholder="自动" value={ov.ctx || ""} onChange={(e) => setOv({ ...ov, ctx: Number(e.target.value) || 0 })} />
          </label>
          <label>
            GPU 层数
            <input type="number" placeholder="自动" value={ov.gpu_layers ?? ""} onChange={(e) => setOv({ ...ov, gpu_layers: e.target.value === "" ? null : Number(e.target.value) })} />
          </label>
          <label>
            CPU 线程
            <input type="number" placeholder="自动" value={ov.threads ?? ""} onChange={(e) => setOv({ ...ov, threads: e.target.value === "" ? null : Number(e.target.value) })} />
          </label>
          <label>
            KV 缓存
            <select value={ov.cache_type ?? "auto"} onChange={(e) => setOv({ ...ov, cache_type: e.target.value === "auto" ? null : e.target.value })}>
              <option value="auto">自动</option>
              <option value="f16">f16</option>
              <option value="q8_0">q8_0</option>
              <option value="q4_0">q4_0</option>
            </select>
          </label>
          <label>
            Flash Attention
            <select value={ov.flash_attn == null ? "auto" : ov.flash_attn ? "on" : "off"} onChange={(e) => setOv({ ...ov, flash_attn: e.target.value === "auto" ? null : e.target.value === "on" })}>
              <option value="auto">自动</option>
              <option value="on">开</option>
              <option value="off">关</option>
            </select>
          </label>
          <label>
            给其他程序留的显存（MB）
            <input type="number" value={ov.vram_reserve_mb ?? 1024} onChange={(e) => setOv({ ...ov, vram_reserve_mb: Number(e.target.value) })} />
          </label>
        </details>
        {preview && (
          <div className={`lm-tune ${preview.ok ? "" : "bad"}`}>
            <div>
              GPU 层 {preview.gpu_layers}/{preview.total_layers} · 上下文 {preview.ctx} · KV {preview.cache_type} · FA {preview.flash_attn ? "开" : "关"} · 线程 {preview.threads} · 批 {preview.batch}
            </div>
            <div className="hint">
              估计显存 {fmtSize(preview.est_vram_mb * 1048576)}，内存 {fmtSize(preview.est_ram_mb * 1048576)}
            </div>
            {[...preview.problems, ...preview.notes].map((n) => (
              <div key={n} className={preview.problems.includes(n) ? "err" : "hint"}>
                {n}
              </div>
            ))}
          </div>
        )}
        <button className="primary" disabled={busy || !pick || (preview ? !preview.ok : false)} onClick={() => void start()}>
          {busy ? "启动中…（大模型加载可能要一两分钟）" : running ? "切换到这个模型" : "启动"}
        </button>
      </div>
    </div>
  );
}

// ====================== 模型库 ======================

function LibraryPanel({ notify, onGo }: { notify: Notify; onGo: (t: Tab) => void }) {
  const [data, setData] = useState<{ models_dir: string; default: string | null; models: LlmModelEntry[] } | null>(null);
  const [inUse, setInUse] = useState<string | null>(null);
  const [detail, setDetail] = useState<{ id: string; info: Record<string, unknown>; check: { state: string; [k: string]: unknown } } | null>(null);
  const refresh = useCallback(() => {
    api.llmLibrary().then((l) => {
      setData(l.data);
      setInUse(l.in_use);
    });
  }, []);
  useEffect(refresh, [refresh]);
  const run = async (f: () => Promise<unknown>, ok?: string) => {
    try {
      await f();
      if (ok) notify(ok);
      refresh();
    } catch (e) {
      notify(e instanceof Error ? e.message : String(e), "error");
    }
  };
  const addFile = async () => {
    const p = await open({ multiple: false, filters: [{ name: "GGUF 模型", extensions: ["gguf"] }] });
    if (typeof p === "string") await run(() => api.llmLibraryAdd(p, null), "已加入模型库");
  };
  const remove = async (m: LlmModelEntry, files: boolean) => {
    if (files) {
      // 删除文件：二次确认。
      if (!window.confirm(`删除模型文件「${m.name}」？\n${m.path}\n\n文件会从磁盘上永久删除，不能恢复。`)) return;
      if (!window.confirm(`再确认一次：永久删除 ${fmtSize(m.size)} 的模型文件？`)) return;
    } else if (!window.confirm(`从模型库移除「${m.name}」的记录？（文件保留在磁盘上）`)) return;
    await run(() => api.llmLibraryRemove(m.id, files), files ? "已删除文件" : "已移除记录");
  };
  const move = async () => {
    const d = await open({ directory: true, multiple: false });
    if (typeof d === "string" && window.confirm(`把模型目录里的文件移动到\n${d}\n？文件较大时需要一些时间。`)) await run(() => api.llmLibraryMove(d), "已移动");
  };
  if (!data) return null;
  return (
    <div className="lm-card">
      <div className="lm-row lm-between">
        <div>
          <strong>模型保存位置</strong>
          <div className="hint">{data.models_dir}</div>
        </div>
        <div className="lm-row">
          <button onClick={() => void move()}>修改位置</button>
          <button onClick={() => void addFile()}>添加已有 GGUF</button>
          <button className="primary" onClick={() => onGo("market")}>
            去模型市场
          </button>
        </div>
      </div>
      {data.models.length === 0 && <p className="hint">模型库是空的。</p>}
      <table className="lm-table">
        <thead>
          <tr>
            <th>模型</th>
            <th>量化</th>
            <th>大小</th>
            <th>架构 / 上下文</th>
            <th>来源</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {data.models.map((m) => (
            <tr key={m.id}>
              <td>
                <strong>{m.name}</strong>
                {data.default === m.id && <span className="lm-tag">默认</span>}
                {inUse === m.path && <span className="lm-tag ok">使用中</span>}
                {m.mmproj && <span className="lm-tag">多模态</span>}
                <div className="hint lm-path">{m.path}</div>
              </td>
              <td>{m.quant ?? "—"}</td>
              <td>{fmtSize(m.size)}</td>
              <td>
                {m.arch ?? "—"} / {m.context_length ? `${Math.round(m.context_length / 1024)}K` : "—"}
              </td>
              <td>{m.source === "local" ? "本地文件" : `${m.source === "hf" ? "HF" : "魔搭"} · ${m.repo}`}</td>
              <td className="lm-actions">
                {data.default !== m.id && <button onClick={() => void run(() => api.llmLibraryDefault(m.id))}>设为默认</button>}
                <button onClick={() => api.llmLibraryInfo(m.id).then(([info, check]) => setDetail({ id: m.id, info, check }))}>详情</button>
                <button
                  onClick={async () => {
                    const p = await open({ multiple: false, filters: [{ name: "mmproj", extensions: ["gguf"] }] });
                    if (typeof p === "string") await run(() => api.llmLibraryMmproj(m.id, p), "已关联 mmproj");
                  }}
                >
                  关联 mmproj
                </button>
                <button onClick={() => void remove(m, false)}>移除记录</button>
                <button className="danger" disabled={inUse === m.path} onClick={() => void remove(m, true)}>
                  删除文件
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      {detail && (
        <div className="lm-detail">
          <div className="lm-row lm-between">
            <strong>模型详情</strong>
            <button className="link" onClick={() => setDetail(null)}>
              关闭
            </button>
          </div>
          <div className={detail.check.state === "ok" ? "ok" : "err"}>
            完整性：{{ ok: "正常", missing: "文件不见了", size_changed: "大小和记录不一致（文件可能不完整）", corrupt: "GGUF 结构损坏" }[detail.check.state] ?? detail.check.state}
          </div>
          <dl className="lm-kv">
            {["architecture", "name", "size_label", "quantization", "context_length", "block_count", "embedding_length", "head_count", "head_count_kv", "file_size", "has_chat_template"].map((k) => (
              <span key={k} style={{ display: "contents" }}>
                <dt>{k}</dt>
                <dd>{k === "file_size" ? fmtSize(detail.info[k] as number) : String(detail.info[k] ?? "—")}</dd>
              </span>
            ))}
          </dl>
        </div>
      )}
    </div>
  );
}

// ====================== 模型市场 ======================

function MarketPanel({ notify, onQueued }: { notify: Notify; onQueued: () => void }) {
  const [source, setSource] = useState<"hf" | "modelscope">("hf");
  const [q, setQ] = useState("");
  const [list, setList] = useState<LlmModelSummary[]>([]);
  const [busy, setBusy] = useState(false);
  const [detail, setDetail] = useState<LlmModelDetail | null>(null);
  const [quant, setQuant] = useState("");
  const [sel, setSel] = useState<Set<string>>(new Set());
  const [tokens, setTokens] = useState<Record<string, boolean>>({});
  const [tokenInput, setTokenInput] = useState("");
  useEffect(() => {
    api.llmRepoTokenStatus().then(setTokens).catch(() => {});
  }, []);
  const search = async () => {
    if (!q.trim()) return;
    setBusy(true);
    setDetail(null);
    try {
      setList(await api.llmRepoSearch(source, q.trim()));
    } catch (e) {
      notify(e instanceof Error ? e.message : String(e), "error");
    } finally {
      setBusy(false);
    }
  };
  const openRepo = async (id: string, src = source) => {
    setBusy(true);
    try {
      const d = await api.llmRepoDetails(src, id);
      setDetail(d);
      setQuant("");
      setSel(new Set());
    } catch (e) {
      notify(e instanceof Error ? e.message : String(e), "error");
    } finally {
      setBusy(false);
    }
  };
  const files = useMemo(() => (detail ? detail.files.filter((f) => !quant || f.quant === quant || f.mmproj) : []), [detail, quant]);
  const quants = useMemo(() => [...new Set((detail?.files ?? []).filter((f) => !f.mmproj && f.quant).map((f) => f.quant as string))].sort(), [detail]);
  const toggle = (path: string) => {
    if (!detail) return;
    const f = detail.files.find((x) => x.path === path);
    const n = new Set(sel);
    // 分片模型：一起选上 / 一起取消。
    const group = f?.split ? detail.files.filter((x) => x.split && x.path.replace(/-\d+-of-\d+\.gguf$/, "") === path.replace(/-\d+-of-\d+\.gguf$/, "")).map((x) => x.path) : [path];
    const on = !n.has(path);
    group.forEach((p) => (on ? n.add(p) : n.delete(p)));
    // 选了主模型时自动带上推荐的 mmproj（可以取消）。
    if (on && f && !f.mmproj && !sel.size) {
      const mp = detail.files.find((x) => x.mmproj);
      if (mp) n.add(mp.path);
    }
    setSel(n);
  };
  const total = detail ? detail.files.filter((f) => sel.has(f.path)).reduce((a, f) => a + f.size, 0) : 0;
  const queue = async () => {
    if (!detail || sel.size === 0) return;
    const chosen = detail.files.filter((f) => sel.has(f.path));
    try {
      await api.llmDownloadAdd(detail.summary.source, detail.summary.id, chosen.map((f) => ({ path: f.path, url: f.url, size: f.size, sha256: f.sha256, mmproj: f.mmproj })));
      notify(`已加入下载：${chosen.length} 个文件，${fmtSize(total)}`);
      onQueued();
    } catch (e) {
      notify(e instanceof Error ? e.message : String(e), "error");
    }
  };
  return (
    <div className="lm-card">
      <div className="lm-recommend">
        <div>
          <strong>推荐默认模型</strong>
          <div className="hint">
            {DEFAULT_REPO} · {DEFAULT_FILE}
          </div>
        </div>
        <div className="lm-row">
          <button onClick={() => void openRepo(DEFAULT_REPO, "hf")}>查看</button>
          <button className="primary" onClick={() => api.llmDownloadDefault().then(onQueued).catch((e: Error) => notify(e.message, "error"))}>
            一键下载
          </button>
        </div>
      </div>
      <div className="lm-row">
        <select value={source} onChange={(e) => setSource(e.target.value as "hf" | "modelscope")}>
          <option value="hf">Hugging Face</option>
          <option value="modelscope">魔搭社区</option>
        </select>
        <input
          className="lm-grow"
          value={q}
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && void search()}
          placeholder={source === "hf" ? "关键词或仓库 ID（作者/名字）" : "仓库 ID（作者/名字）或作者名"}
        />
        <button className="primary" disabled={busy} onClick={() => void search()}>
          搜索
        </button>
      </div>
      <details className="lm-adv">
        <summary>
          访问令牌（私有仓库用）{tokens[source] ? " · 已设置" : ""}
        </summary>
        <div className="lm-row">
          <input type="password" className="lm-grow" value={tokenInput} onChange={(e) => setTokenInput(e.target.value)} placeholder={source === "hf" ? "hf_…" : "魔搭 SDK 令牌"} />
          <button onClick={() => api.llmRepoToken(source, tokenInput).then((v) => { setTokens({ ...tokens, [source]: v }); setTokenInput(""); notify(v ? "已保存（存在系统钥匙串）" : "已清除"); })}>保存</button>
          {tokens[source] && <button onClick={() => api.llmRepoToken(source, null).then(() => setTokens({ ...tokens, [source]: false }))}>清除</button>}
        </div>
      </details>
      {!detail && (
        <div className="lm-results">
          {list.map((m) => (
            <button key={m.id} className="lm-result" onClick={() => void openRepo(m.id)}>
              <strong>{m.id}</strong>
              <small>
                {m.author} · 下载 {m.downloads.toLocaleString()} · 喜欢 {m.likes} · {m.license ?? "许可证未注明"} · 更新 {m.updated.slice(0, 10)}
                {m.gated ? " · 需要授权" : ""}
              </small>
            </button>
          ))}
          {!busy && list.length === 0 && <p className="hint">输入关键词搜索 GGUF 模型。</p>}
        </div>
      )}
      {detail && (
        <div className="lm-repo">
          <div className="lm-row lm-between">
            <div>
              <button className="link" onClick={() => setDetail(null)}>
                ← 返回结果
              </button>
              <h3>{detail.summary.id}</h3>
              <small className="hint">
                {detail.summary.author} · {detail.summary.license ?? "许可证未注明"} · 更新 {detail.summary.updated.slice(0, 10)} · 下载 {detail.summary.downloads.toLocaleString()}
              </small>
            </div>
            <label>
              量化
              <select value={quant} onChange={(e) => setQuant(e.target.value)}>
                <option value="">全部</option>
                {quants.map((x) => (
                  <option key={x}>{x}</option>
                ))}
              </select>
            </label>
          </div>
          <table className="lm-table">
            <tbody>
              {files.map((f) => (
                <tr key={f.path} className={sel.has(f.path) ? "on" : ""} onClick={() => toggle(f.path)}>
                  <td>
                    <input type="checkbox" readOnly checked={sel.has(f.path)} />
                  </td>
                  <td>
                    {f.path}
                    {f.mmproj && <span className="lm-tag">mmproj</span>}
                    {f.split && <span className="lm-tag">分片 {f.split[0]}/{f.split[1]}</span>}
                  </td>
                  <td>{f.quant ?? "—"}</td>
                  <td>{fmtSize(f.size)}</td>
                  <td className="hint">{f.sha256 ? "有校验值" : ""}</td>
                </tr>
              ))}
            </tbody>
          </table>
          {detail.files.length === 0 && <p className="hint">这个仓库里没有 GGUF 文件。</p>}
          <div className="lm-row lm-between">
            <span className="hint">已选 {sel.size} 个文件，共 {fmtSize(total)}</span>
            <button className="primary" disabled={sel.size === 0} onClick={() => void queue()}>
              下载所选
            </button>
          </div>
          {detail.card && (
            <details>
              <summary>模型说明（仅供阅读）</summary>
              <pre className="lm-card-text">{detail.card}</pre>
            </details>
          )}
        </div>
      )}
    </div>
  );
}

// ====================== 下载管理 ======================

const STATE_LABEL: Record<string, string> = { queued: "排队中", running: "下载中", paused: "已暂停", retrying: "重试中", failed: "失败", done: "完成", cancelled: "已取消" };

function DownloadsPanel({ tasks, notify }: { tasks: LlmTask[]; notify: Notify }) {
  const [conc, setConc] = useState(2);
  const op = (id: string, o: "pause" | "resume" | "cancel" | "remove") => api.llmDownloadOp(id, o).catch((e: Error) => notify(e.message, "error"));
  const sorted = [...tasks].sort((a, b) => b.created_at - a.created_at);
  return (
    <div className="lm-card">
      <div className="lm-row lm-between">
        <strong>下载任务</strong>
        <label>
          同时下载
          <select
            value={conc}
            onChange={(e) => {
              const n = Number(e.target.value);
              setConc(n);
              void api.llmDownloadSettings(n);
            }}
          >
            {[1, 2, 3, 4].map((n) => (
              <option key={n} value={n}>
                {n} 个
              </option>
            ))}
          </select>
        </label>
      </div>
      {sorted.length === 0 && <p className="hint">没有下载任务。关掉软件时没下完的任务，下次打开会自动继续。</p>}
      {sorted.map((t) => {
        const total = t.files.reduce((a, f) => a + f.size, 0);
        const pct = total ? Math.min(100, (t.downloaded / total) * 100) : 0;
        const eta = t.speed > 1 && total ? (total - t.downloaded) / t.speed : null;
        const s = t.state.state;
        return (
          <div key={t.id} className={`lm-task st-${s}`}>
            <div className="lm-row lm-between">
              <div>
                <strong>{t.repo}</strong>
                <small className="hint"> · {t.files.map((f) => f.path).join("、")}</small>
              </div>
              <span className="lm-tag">{STATE_LABEL[s] ?? s}</span>
            </div>
            <div className="lm-bar">
              <i style={{ width: `${pct}%` }} />
            </div>
            <div className="lm-row lm-between hint">
              <span>
                {fmtSize(t.downloaded)} / {fmtSize(total)}（{pct.toFixed(1)}%）
                {s === "running" && ` · ${fmtSize(t.speed)}/s · 剩余 ${fmtEta(eta)}`}
                {t.state.state === "retrying" && ` · 第 ${t.state.attempt} 次重试：${t.state.error}`}
                {t.state.state === "failed" && ` · ${t.state.error}`}
              </span>
              <span className="lm-row">
                {(s === "running" || s === "queued" || s === "retrying") && <button onClick={() => void op(t.id, "pause")}>暂停</button>}
                {(s === "paused" || s === "failed") && <button onClick={() => void op(t.id, "resume")}>{s === "failed" ? "重试" : "继续"}</button>}
                {s !== "done" && s !== "cancelled" && (
                  <button
                    onClick={() => {
                      if (window.confirm("取消这个下载？已下载的部分会删除。")) void op(t.id, "cancel");
                    }}
                  >
                    取消
                  </button>
                )}
                {(s === "done" || s === "cancelled" || s === "failed" || s === "paused") && <button onClick={() => void op(t.id, "remove")}>移除记录</button>}
              </span>
            </div>
          </div>
        );
      })}
    </div>
  );
}

// ====================== 运行环境 ======================

function RuntimePanel({ notify }: { notify: Notify }) {
  const [v, setV] = useState<LlmRuntimeView | null>(null);
  const [hw, setHw] = useState<LlmHardware | null>(null);
  const [upd, setUpd] = useState<{ tag: string; newer: boolean; plan: { reason: string; backend: LlmBackend; assets: { name: string; size: number }[] } | null } | null>(null);
  const [prog, setProg] = useState<Record<string, unknown> | null>(null);
  const [prefer, setPrefer] = useState("auto");
  const refresh = useCallback((deep = false) => {
    api.llmRuntimeStatus(deep).then((x) => {
      setV(x);
      setPrefer(x.state.prefer ?? "auto");
      if (x.install && (x.install as { stage?: string }).stage && !["done", "error"].includes((x.install as { stage: string }).stage)) setProg(x.install);
    });
  }, []);
  useEffect(() => {
    refresh(true);
    api.llmHw().then(setHw);
    const un = listen<Record<string, unknown>>("llm-runtime", (e) => {
      setProg(e.payload);
      if (e.payload.stage === "done") {
        notify(`llama.cpp ${String(e.payload.tag)} 安装完成`);
        setProg(null);
        refresh(true);
      }
      if (e.payload.stage === "error") {
        notify(String(e.payload.error), "error");
        setProg(null);
        refresh();
      }
    });
    return () => {
      void un.then((f) => f());
    };
  }, [refresh, notify]);
  const check = () => api.llmRuntimeCheck().then(setUpd).catch((e: Error) => notify(e.message, "error"));
  const installing = !!prog;
  const cur = v?.state.current;
  const health = v?.health;
  return (
    <div className="lm-grid">
      <div className="lm-card">
        <h3>llama.cpp 运行环境</h3>
        <dl className="lm-kv">
          <dt>状态</dt>
          <dd className={health?.state === "ok" ? "ok" : health?.state === "broken" ? "err" : ""}>{health?.state === "ok" ? "正常" : health?.state === "broken" ? `损坏：${health.reason}` : "未安装"}</dd>
          <dt>当前版本</dt>
          <dd>{cur ? `${cur.tag}（${backendLabel(cur.backend)}）` : "—"}</dd>
          <dt>自检</dt>
          <dd className="lm-path">{cur?.version_line || "—"}</dd>
          <dt>上一个版本</dt>
          <dd>{v?.state.previous ? `${v.state.previous.tag}（${backendLabel(v.state.previous.backend)}）` : "—"}</dd>
          <dt>上次检查</dt>
          <dd>{v?.state.last_check ? new Date(v.state.last_check).toLocaleString() : "从未"}</dd>
          <dt>位置</dt>
          <dd className="lm-path">{v?.root}</dd>
        </dl>
        <label className="check">
          <input type="checkbox" checked={!!v?.state.auto_check} onChange={(e) => api.llmRuntimeSettings(e.target.checked, prefer === "auto" ? null : prefer).then(() => refresh())} /> 每天自动检查更新（只提示，不自动安装）
        </label>
        <label>
          推理后端
          <select
            value={prefer}
            onChange={(e) => {
              setPrefer(e.target.value);
              void api.llmRuntimeSettings(!!v?.state.auto_check, e.target.value === "auto" ? null : e.target.value);
            }}
          >
            <option value="auto">自动（推荐）</option>
            <option value="cuda">CUDA（NVIDIA）</option>
            <option value="vulkan">Vulkan</option>
            <option value="cpu">CPU</option>
          </select>
        </label>
        <div className="lm-row">
          <button disabled={installing} onClick={() => void check()}>
            检查更新
          </button>
          <button className="primary" disabled={installing} onClick={() => api.llmRuntimeInstall(prefer).then(() => setProg({ stage: "check" })).catch((e: Error) => notify(e.message, "error"))}>
            {!cur ? "下载并安装" : health?.state === "broken" ? "重新安装" : "安装最新版本"}
          </button>
          <button disabled={installing || !v?.state.previous} onClick={() => api.llmRuntimeRollback().then(() => { notify("已回滚到上一个版本"); refresh(true); }).catch((e: Error) => notify(e.message, "error"))}>
            回滚
          </button>
          <button onClick={() => refresh(true)}>运行自检</button>
        </div>
        {upd && (
          <div className="lm-tune">
            最新版本 {upd.tag}
            {upd.newer ? "（有更新）" : "（已是最新）"}
            {upd.plan && (
              <div className="hint">
                会安装：{backendLabel(upd.plan.backend)} · {upd.plan.reason} · {fmtSize(upd.plan.assets.reduce((a, x) => a + x.size, 0))}
              </div>
            )}
          </div>
        )}
        {installing && (
          <div className="lm-task">
            <div>
              {prog.stage === "check" ? "正在读取发布列表…" : `正在下载 ${String(prog.file ?? prog.tag ?? "")}`}
              {prog.reason ? <div className="hint">{String(prog.reason)}</div> : null}
            </div>
            {typeof prog.downloaded === "number" && typeof prog.total === "number" && (
              <>
                <div className="lm-bar">
                  <i style={{ width: `${Math.min(100, (Number(prog.downloaded) / Number(prog.total)) * 100)}%` }} />
                </div>
                <div className="hint">
                  {fmtSize(Number(prog.downloaded))} / {fmtSize(Number(prog.total))} · {fmtSize(Number(prog.speed ?? 0))}/s · 剩余 {fmtEta(prog.eta as number)}
                </div>
              </>
            )}
            <button onClick={() => void api.llmRuntimeCancel()}>取消</button>
          </div>
        )}
        <p className="hint">从 ggml-org/llama.cpp 官方 GitHub Releases 下载预编译版本。更新前保留旧版本，新版本自检通过才切换；不会动模型、配置和存档。</p>
      </div>
      <div className="lm-card">
        <h3>本机硬件</h3>
        {hw && (
          <dl className="lm-kv">
            <dt>系统</dt>
            <dd>
              {hw.os} {hw.arch}
            </dd>
            <dt>CPU 线程</dt>
            <dd>{hw.cpu_threads}</dd>
            <dt>内存</dt>
            <dd>{fmtSize(hw.ram_mb * 1048576)}</dd>
            <dt>显卡</dt>
            <dd>{hw.nvidia.length ? hw.nvidia.map((g) => `${g.name}（${fmtSize(g.vram_mb * 1048576)}，驱动 ${g.driver}）`).join("；") : "没有检测到 NVIDIA 显卡"}</dd>
            <dt>CUDA</dt>
            <dd>{hw.cuda ? `驱动支持到 ${hw.cuda[0]}.${hw.cuda[1]}` : "—"}</dd>
            <dt>Vulkan</dt>
            <dd>{hw.vulkan ? "可用" : "不可用"}</dd>
          </dl>
        )}
        <button onClick={() => api.llmHw(true).then(setHw)}>重新检测</button>
      </div>
    </div>
  );
}
