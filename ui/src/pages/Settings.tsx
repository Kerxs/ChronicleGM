import AppearanceData from "../components/AppearanceData";
import SearchSettings from "../components/SearchSettings";
import { useCallback, useEffect, useState } from "react";
import { api, type AppInfo, type ProviderView, type ScanResult } from "../api";
import type { DiscoveredService } from "../bindings/DiscoveredService";
import type { ProviderConfig } from "../bindings/ProviderConfig";
import type { ProviderStats } from "../bindings/ProviderStats";
import { ROUTE_TASKS } from "../lib/labels";
import LaunchSettingsCard from "../components/LaunchSettingsCard";
import CloudProviderForm, { blankProvider } from "../components/CloudProviderForm";
import { SERVICE_LABEL } from "../lib/cloudPresets";
import { checkUpdates, openUpdate, useUpdates } from "../lib/updates";

const KIND_LABEL: Record<string, string> = {
  llama_server: "llama.cpp (llama-server)",
  ollama: "Ollama",
  lm_studio: "LM Studio",
  kobold_cpp: "KoboldCpp",
  text_gen_web_ui: "text-generation-webui",
  vllm: "vLLM",
  open_ai_compatible: "OpenAI 兼容服务",
  breeze_tts: "Breeze TTS（语音合成）",
  open_ai_speech: "OpenAI 兼容语音合成（Kokoro、Edge-TTS 等）",
  gpt_sovits: "GPT-SoVITS（语音合成）",
};

function emptyConfig(): ProviderConfig {
  return blankProvider("chat");
}

export default function SettingsPage({ notify, info }: { notify: (m: string, k?: "info" | "error") => void; info?: AppInfo | null; onOpenPerf?: () => void }) {
  const [providers, setProviders] = useState<ProviderView[]>([]);
  const [routes, setRoutes] = useState<[string, string][]>([]);
  const [stats, setStats] = useState<ProviderStats[]>([]);
  const [found, setFound] = useState<DiscoveredService[] | null>(null);
  const [extra, setExtra] = useState("");
  const [scanning, setScanning] = useState(false);
  const [testing, setTesting] = useState<string | null>(null);
  const [editingId, setEditingId] = useState<string | null>(null);
  const updates = useUpdates();
  const [autoUpdate, setAutoUpdate] = useState(true);
  useEffect(() => {
    api.uiPref("ui.auto_update").then((v) => setAutoUpdate(v !== "0")).catch(() => {});
  }, []);
  const [folder, setFolder] = useState("");
  const [scan, setScan] = useState<ScanResult | null>(null);

  const load = useCallback(async () => {
    try {
      setProviders(await api.listProviders());
      setRoutes(await api.listRoutes());
      setStats(await api.providerStats());
    } catch (e) {
      notify((e as Error).message, "error");
    }
  }, [notify]);

  useEffect(() => {
    void load();
  }, [load]);

  async function discover() {
    setScanning(true);
    try {
      setFound(await api.discoverServices(extra.split(/[\s,，]+/).filter(Boolean)));
    } catch (e) {
      notify((e as Error).message, "error");
    } finally {
      setScanning(false);
    }
  }

  async function addFound(s: DiscoveredService, model: string, ctxLen: number | null) {
    try {
      const cfg = await api.saveProvider(
        { ...emptyConfig(), name: `${KIND_LABEL[s.kind] ?? s.kind} · ${model}`, base_url: s.base_url, model, server_kind: s.kind, service: s.kind === "breeze_tts" ? "speech" : "chat" },
        null,
      );
      notify("已添加，正在实测能力…");
      await load();
      await test(cfg.id, ctxLen);
    } catch (e) {
      notify((e as Error).message, "error");
    }
  }

  // 识别到的本机语音服务：添加为「语音合成」，合成"测试"两个字检查能用；还没有指定朗读用哪个服务时自动选它。
  // GPT-SoVITS 需要参考音频，添加后直接打开编辑表单让你填，填好再测试。
  async function addSpeech(s: DiscoveredService) {
    try {
      const base = { ...blankProvider("speech"), name: `${KIND_LABEL[s.kind] ?? s.kind}`, server_kind: s.kind, service: "speech" as const };
      const zh = s.voices.filter((v) => /^z[fm]_|^zh-/i.test(v));
      const draft =
        s.kind === "breeze_tts"
          ? { ...base, base_url: s.root_url, model: "breeze-tts" }
          : s.kind === "gpt_sovits"
            ? { ...base, base_url: s.root_url, model: "gpt-sovits", media: { ...base.media, tts_api: "gpt_sovits", audio_format: "wav", send_instructions: false } }
            : {
                ...base,
                base_url: s.base_url,
                model: s.models[0]?.id || (s.voices.length ? "kokoro" : "tts-1"),
                media: { ...base.media, voice: zh[0] ?? s.voices[0] ?? "", voices: (zh.length ? zh : s.voices).slice(0, 8), audio_format: "mp3", send_instructions: false },
              };
      const cfg = await api.saveProvider(draft, null);
      if (!routeOf("tts")) await api.setRoute("tts", cfg.id).catch(() => {});
      await load();
      if (s.kind === "gpt_sovits") {
        setEditingId(cfg.id);
        // 编辑表单在页面下方：滚过去，免得找不到。
        window.setTimeout(() => document.querySelector(`[data-provider="${cfg.id}"] .cloud-form`)?.scrollIntoView({ behavior: "smooth", block: "center" }), 300);
        notify("已添加 GPT-SoVITS。请在下面填参考音频（路径和音频里说的话），保存后点「重新测试」。");
        return;
      }
      notify("已添加，正在合成一句测试语音…");
      await test(cfg.id);
    } catch (e) {
      notify((e as Error).message, "error");
    }
  }

  async function test(id: string, hint: number | null = null) {
    setTesting(id);
    try {
      const caps = await api.testProvider(id, hint);
      notify(caps.chat ? "测试完成" : `测试未通过：${caps.notes.join("；")}`, caps.chat ? "info" : "error");
      await load();
    } catch (e) {
      notify((e as Error).message, "error");
    } finally {
      setTesting(null);
    }
  }


  async function toggleAllow(p: ProviderView) {
    if (!p.config.allow_campaign_content && !confirm(`允许把世界设定与剧本内容发送给「${p.config.name}」（${p.config.base_url}）？`)) return;
    try {
      await api.saveProvider({ ...p.config, allow_campaign_content: !p.config.allow_campaign_content }, null);
      await load();
    } catch (e) {
      notify((e as Error).message, "error");
    }
  }

  async function remove(p: ProviderView) {
    if (!confirm(`删除「${p.config.name}」的配置？（不会删除任何模型文件）`)) return;
    try {
      await api.deleteProvider(p.config.id);
      await load();
    } catch (e) {
      notify((e as Error).message, "error");
    }
  }

  async function doScan() {
    try {
      setScan(await api.scanModelFolder(folder));
    } catch (e) {
      notify((e as Error).message, "error");
    }
  }

  const routeOf = (task: string) => routes.find(([t]) => t === task)?.[1] ?? "";
  const isSpeech = (p: ProviderView) => p.config.service === "speech" || p.config.server_kind === "breeze_tts";
  const chatProviders = providers.filter((p) => p.config.service === "chat" && p.config.server_kind !== "breeze_tts");
  const routeChoices = (task: string) => (task === "tts" ? providers.filter(isSpeech) : task === "image" ? providers.filter((p) => p.config.service === "image") : chatProviders);

  return (
    <section className="settings">
      <p className="hint">模型由你自己准备；这里负责识别、测试与选择，不会下载或安装任何东西。默认也不启动程序，只有在下面的「模型启动参数」里配置了托管，才会按你填的参数启动和停止。</p>

      <SearchSettings notify={notify} />
      <div className="card">
        <h3>自动识别本机服务</h3>
        <div className="row">
          <input value={extra} onChange={(e) => setExtra(e.target.value)} placeholder="其他地址（可选），如 http://127.0.0.1:9090" style={{ flex: 1 }} />
          <button className="primary" disabled={scanning} onClick={() => void discover()} data-tour="models-scan">
            {scanning ? "识别中…" : "扫描"}
          </button>
        </div>
        <p className="hint">只扫描 127.0.0.1 的常见端口：8080（llama-server）、11434（Ollama）、1234（LM Studio）、5001、5000、8000、8137（Breeze TTS），以及语音合成常用的 8880（Kokoro）、5050（Edge-TTS）、9880（GPT-SoVITS）。</p>
        {found && found.length === 0 && <p className="hint">没有发现正在运行的服务。请先启动 llama-server 等程序后再扫描。</p>}
        {found?.map((s) => (
          <div key={s.root_url} className="found">
            <strong>{KIND_LABEL[s.kind] ?? s.kind}</strong> <span className="hint">{s.root_url}</span>
            {!s.usable_for_chat && (
              <div className="row">
                <span className="hint">
                  语音合成服务：用来朗读 GM 的讲述和 NPC 的台词（不能用于文本生成）。
                  {s.voices.length > 0 && `有 ${s.voices.length} 个声音。`}
                  {s.kind === "gpt_sovits" && "添加后需要填参考音频。"}
                </span>
                {providers.some((p) => p.config.service === "speech" && p.config.base_url.replace(/\/+$/, "").replace(/\/v1$/, "") === s.root_url.replace(/\/+$/, "")) ? (
                  <span className="hint">已添加</span>
                ) : (
                  <button onClick={() => void addSpeech(s)}>添加并测试</button>
                )}
              </div>
            )}
            {s.usable_for_chat &&
              s.models.map((m) => (
                <div key={m.id} className="row">
                  <span>
                    {m.id}
                    {m.context_length && <span className="hint">（上下文 {m.context_length}）</span>}
                  </span>
                  <button onClick={() => void addFound(s, m.id, m.context_length)}>添加并测试</button>
                </div>
              ))}
          </div>
        ))}
      </div>

      <div className="card">
        <h3>已配置的服务</h3>
        {providers.length === 0 && <p className="hint">还没有配置。</p>}
        {providers.map((p) => {
          const c = p.config.capabilities;
          const st = stats.find((s) => s.provider_id === p.config.id);
          return (
            <div key={p.config.id} className="provider" data-provider={p.config.id}>
              <div className="row">
                <span className={`svc svc-${isSpeech(p) ? "speech" : p.config.service}`}>{isSpeech(p) ? "语音合成" : SERVICE_LABEL[p.config.service]}</span>
                <strong>{p.config.name}</strong>
                {!p.loopback && <span className="svc cloud">云端</span>}
                <span className="hint">
                  {p.config.base_url} · {p.config.model}
                </span>
              </div>
              <div className="caps">
                {c && p.config.service !== "chat" ? (
                  <>
                    <span className={c.chat ? "ok" : "bad"}>{p.config.service === "speech" ? "合成测试" : "Key 与模型"}{c.chat ? "✓" : "✗"}</span>
                    {c.notes.length > 0 && <div className="hint">{c.notes.join("；")}</div>}
                  </>
                ) : c ? (
                  <>
                    <span className={c.chat ? "ok" : "bad"}>对话{c.chat ? "✓" : "✗"}</span>
                    <span className={c.streaming ? "ok" : "bad"}>流式{c.streaming ? "✓" : "✗"}</span>
                    <span className={c.json_schema ? "ok" : "bad"}>结构化输出{c.json_schema ? "✓" : "✗（改用提示词约束）"}</span>
                    <span className={c.vision ? "ok" : "bad"} title="能否使用参考图片、AI 看图描述（实测：发一张纯红色小图，看模型能否认出颜色）">看图{c.vision ? "✓" : "✗"}</span>
                    <span>上下文 {c.context_length ?? "未知"}</span>
                    {c.notes.length > 0 && <div className="hint">{c.notes.join("；")}</div>}
                  </>
                ) : (
                  <span className="hint">尚未测试</span>
                )}
              </div>
              {st && (
                <div className="hint">
                  请求 {st.requests} 次，失败 {st.failures} 次，平均 {Math.round(st.avg_latency_ms / 100) / 10} 秒，输入 {st.prompt_tokens} / 输出 {st.completion_tokens} tokens
                </div>
              )}
              <div className="row">
                <button disabled={testing === p.config.id} onClick={() => void test(p.config.id)}>
                  {testing === p.config.id ? "测试中…" : "重新测试"}
                </button>
                {!p.loopback && (
                  <label className="check">
                    <input type="checkbox" checked={p.config.allow_campaign_content} onChange={() => void toggleAllow(p)} />
                    {p.config.service === "speech" ? "允许发送叙述与台词" : p.config.service === "image" ? "允许发送图片描述" : "允许发送世界与剧本内容"}
                  </label>
                )}
                {(!p.loopback || p.config.service !== "chat") && <button onClick={() => setEditingId(editingId === p.config.id ? null : p.config.id)}>{editingId === p.config.id ? "收起" : "编辑"}</button>}
                {p.config.service === "chat" && <label className="check" title="关闭后生成更快，内容可能简短一些（需要服务支持 chat_template_kwargs，如 llama-server）">
                  思考模式
                  <select
                    value={p.config.thinking}
                    onChange={(e) =>
                      void api
                        .saveProvider({ ...p.config, thinking: e.target.value as "auto" | "off" }, null)
                        .then(load)
                        .catch((er: Error) => notify(er.message, "error"))
                    }
                  >
                    <option value="auto">模型默认</option>
                    <option value="off">关闭（更快）</option>
                  </select>
                </label>}
                {p.config.needs_key && <span className="hint">{p.has_key ? "API Key 已保存在系统凭据管理器" : "缺少 API Key"}</span>}
                <button className="danger" onClick={() => void remove(p)}>
                  删除配置
                </button>
              </div>
              {editingId === p.config.id && (
                <CloudProviderForm
                  initial={p.config}
                  hasKey={p.has_key}
                  notify={notify}
                  onCancel={() => setEditingId(null)}
                  onSaved={() => {
                    setEditingId(null);
                    void load();
                  }}
                />
              )}
            </div>
          );
        })}
      </div>

      <LaunchSettingsCard notify={notify} />

      <div className="card">
        <h3>任务使用哪个服务</h3>
        {ROUTE_TASKS.map(([task, label]) => (
          <div key={task} className="row">
            <span style={{ width: 220 }}>{label}</span>
            <select
              value={routeOf(task)}
              onChange={(e) => void api.setRoute(task, e.target.value || null).then(load).catch((er: Error) => notify(er.message, "error"))}
            >
              <option value="">{task === "default" ? "（未设置）" : task === "tts" ? "自动（第一个语音服务）" : task === "image" ? "自动（第一个生图服务）" : "使用默认"}</option>
              {routeChoices(task).map((p) => (
                <option key={p.config.id} value={p.config.id}>
                  {p.config.name}
                </option>
              ))}
            </select>
          </div>
        ))}
      </div>

      <VoiceCard notify={notify} />

      <div className="card cloud-card">
        <h3>添加服务（云端或本机）</h3>
        <p className="hint">语言模型、语音合成、生成图片都可以用云端服务（OpenAI 兼容接口）。选一个服务商会自动填好地址，填上 API Key 和模型即可；也可以填本机其他服务的地址。</p>
        <CloudProviderForm
          notify={notify}
          onSaved={(cfg) => {
            void load().then(() => test(cfg.id));
          }}
        />
      </div>

      <div className="card">
        <h3>识别本地模型文件（只读）</h3>
        <div className="row">
          <input value={folder} onChange={(e) => setFolder(e.target.value)} placeholder="例如 D:\llama" style={{ flex: 1 }} />
          <button onClick={() => void doScan()}>扫描文件夹</button>
        </div>
        <p className="hint">只读取 GGUF 文件头信息，不会移动、修改或启动任何文件。</p>
        {scan && (
          <>
            <table className="jobs">
              <thead>
                <tr>
                  <th>文件</th>
                  <th>用途</th>
                  <th>架构</th>
                  <th>规模</th>
                  <th>量化</th>
                  <th>训练上下文</th>
                  <th>大小</th>
                </tr>
              </thead>
              <tbody>
                {scan.models.map((m) => (
                  <tr key={m.path}>
                    <td title={m.path}>{m.path.split(/[\\/]/).pop()}</td>
                    <td>{{ chat: "对话模型", projector: "多模态投影", tts: "语音", other: "其他" }[m.role] ?? m.role}</td>
                    <td>{m.architecture ?? "—"}</td>
                    <td>{m.size_label ?? "—"}</td>
                    <td>{m.quantization ?? "—"}</td>
                    <td>{m.context_length ?? "—"}</td>
                    <td>{(m.file_size / 1e9).toFixed(2)} GB</td>
                  </tr>
                ))}
              </tbody>
            </table>
            {scan.executables.length > 0 && <p className="hint">找到服务程序：{scan.executables.join("、")}（不会自动启动）</p>}
            {scan.errors.map(([p, e]) => (
              <p key={p} className="err">
                {p}：{e}
              </p>
            ))}
          </>
        )}
      </div>
      <AppearanceData notify={notify} />
      {info && (
        <div className="card about">
          <h3>关于</h3>
          <dl>
            <dt>版本</dt>
            <dd>v{info.version}</dd>
            <dt>提示词模板</dt>
            <dd>
              {info.template_version}
              {info.templates_overridden.length > 0 && `（已自定义：${info.templates_overridden.join("、")}）`}
            </dd>
            <dt>数据目录</dt>
            <dd className="mono">{info.data_dir}</dd>
            <dt>更新</dt>
            <dd>
              <div className="row wrap">
                <button className="small" disabled={updates.checking} onClick={() => void checkUpdates(true)}>
                  {updates.checking ? "正在检查…" : "检查更新"}
                </button>
                {updates.info && (
                  <button className="small primary" onClick={openUpdate}>
                    更新到 {updates.info.version}
                  </button>
                )}
                <label className="check">
                  <input
                    type="checkbox"
                    checked={autoUpdate}
                    onChange={(e) => {
                      setAutoUpdate(e.target.checked);
                      void api.setUiPref("ui.auto_update", e.target.checked ? "1" : "0");
                    }}
                  />
                  启动时自动检查
                </label>
              </div>
              <small className="hint">
                {updates.error
                  ? `检查失败：${updates.error}`
                  : updates.checkedAt
                    ? updates.info
                      ? `有新版本 ${updates.info.version}`
                      : "已是最新版本"
                    : "从 GitHub 发布页检查（github.com/Kerxs/ChronicleGM）；安装包要通过签名校验才会安装。"}
              </small>
            </dd>
          </dl>
        </div>
      )}
    </section>
  );
}

/** 语音：服务状态、旁白的声音、缓存。 */
function VoiceCard({ notify }: { notify: (m: string, k?: "info" | "error") => void }) {
  const [st, setSt] = useState<Awaited<ReturnType<typeof api.voiceStatus>> | null>(null);
  const [instr, setInstr] = useState("");
  const [testing, setTesting] = useState(false);
  const load = () =>
    api
      .voiceStatus()
      .then((s) => {
        setSt(s);
        setInstr(s.narrator.instructions);
      })
      .catch((e: Error) => notify(e.message, "error"));
  useEffect(() => {
    void load();
  }, []);
  if (!st) return null;
  return (
    <div className="card">
      <h3>语音朗读</h3>
      <p className="hint">
        {st.configured
          ? `语音服务：${st.base_url}（${st.cloud ? "云端，按用量计费；点「试听」实测" : st.healthy ? "在线" : "未响应：托管的语音模型会在朗读时自动启动"}）`
          : st.problem ?? "还没有语音服务：先在上面添加本机 Breeze TTS 或云端语音合成。"}
        　缓存 {st.cache_files} 条，{st.cache_mb.toFixed(1)} MB（上限 500 MB，超出时删除最久没用的）；排队 {st.queued} 条（最多 16 条）。
      </p>
      <div className="row">
        <span style={{ width: 120 }}>旁白的声音</span>
        <input value={instr} onChange={(e) => setInstr(e.target.value)} style={{ flex: 1 }} placeholder="例如：沉稳的中文旁白，语速适中" />
        <button onClick={() => void api.voiceSetNarrator({ ...st.narrator, instructions: instr }).then(() => notify("已保存")).catch((e: Error) => notify(e.message, "error"))}>保存</button>
        <button
          disabled={!st.configured || testing}
          onClick={() => {
            setTesting(true);
            api
              .voiceSpeak("", null, "夜幕降临，酒馆里的炉火噼啪作响。")
              .then((r) => {
                void new Audio(r.data_url).play();
                notify(`合成${r.seconds > 0 ? ` ${r.seconds.toFixed(1)} 秒` : ""}语音，用时 ${(r.ms / 1000).toFixed(1)} 秒${r.cached ? "（缓存）" : ""}`);
                void load();
              })
              .catch((e: Error) => notify(e.message, "error"))
              .finally(() => setTesting(false));
          }}
        >
          {testing ? "合成中…" : "试听"}
        </button>
        <button onClick={() => void api.voiceClearCache().then((n) => { notify(`删除了 ${n} 条缓存`); void load(); }).catch((e: Error) => notify(e.message, "error"))}>清空缓存</button>
      </div>
      <p className="hint">每个人物的声音可以在世界树的人物编辑页设置；没有设置时按身份和性格生成，同一个人物每次的声音尽量一致。</p>
    </div>
  );
}
