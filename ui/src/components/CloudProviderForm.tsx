// 添加 / 编辑云端服务（语言模型、语音合成、生成图片）。也可以填本机其他 OpenAI 兼容服务的地址。
// API Key 保存在 Windows 凭据管理器，不写入数据库；编辑时不填 Key 表示不修改。
import { useMemo, useState } from "react";
import { api } from "../api";
import type { ProviderConfig } from "../bindings/ProviderConfig";
import type { ServiceType } from "../bindings/ServiceType";
import { CLOUD_PRESETS, SERVICE_LABEL, type CloudPreset } from "../lib/cloudPresets";
import { Segmented } from "./Segmented";

export function blankProvider(service: ServiceType = "chat"): ProviderConfig {
  return {
    id: "",
    name: "",
    base_url: "",
    model: "",
    server_kind: null,
    needs_key: false,
    timeout_secs: service === "image" ? 300 : 600,
    capabilities: null,
    allow_campaign_content: false,
    thinking: "auto",
    service,
    media: { voice: "", voices: [], audio_format: "", send_instructions: true, image_size: "", tts_api: "", text_lang: "" },
  };
}

const isLocal = (url: string) => /^https?:\/\/(127\.0\.0\.1|localhost|\[::1\])(:|\/|$)/i.test(url.trim());

export default function CloudProviderForm({ initial, hasKey, onSaved, onCancel, notify }: { initial?: ProviderConfig; hasKey?: boolean; onSaved: (cfg: ProviderConfig) => void; onCancel?: () => void; notify: (m: string, k?: "info" | "error") => void }) {
  const [cfg, setCfg] = useState<ProviderConfig>(initial ?? blankProvider());
  const [preset, setPreset] = useState<string>(initial ? "" : "openai");
  const [key, setKey] = useState("");
  const [models, setModels] = useState<string[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const editing = !!initial?.id;
  const presets = useMemo(() => CLOUD_PRESETS.filter((p) => p.service === cfg.service), [cfg.service]);
  const current = CLOUD_PRESETS.find((p) => p.id === preset);
  const local = isLocal(cfg.base_url);
  const set = <K extends keyof ProviderConfig>(k: K, v: ProviderConfig[K]) => setCfg((c) => ({ ...c, [k]: v }));
  const setMedia = <K extends keyof ProviderConfig["media"]>(k: K, v: ProviderConfig["media"][K]) => setCfg((c) => ({ ...c, media: { ...c.media, [k]: v } }));

  function applyPreset(p: CloudPreset) {
    setPreset(p.id);
    setModels(null);
    setCfg((c) => ({
      ...c,
      name: p.base_url ? p.name : c.name,
      base_url: p.base_url || c.base_url,
      model: p.models[0] ?? "",
      media: {
        ...c.media,
        voice: p.voices?.[0] ?? "",
        voices: p.voices ?? [],
        audio_format: p.audio_format ?? "",
        send_instructions: p.send_instructions ?? true,
        image_size: p.sizes?.[0] ?? "",
        tts_api: p.tts_api ?? "",
        text_lang: "",
      },
    }));
  }

  // 语音：读取服务里实际有的声音（Kokoro-FastAPI 的 /v1/audio/voices、openai-edge-tts 的 /v1/voices/all）。
  const [voiceList, setVoiceList] = useState<string[] | null>(null);
  async function fetchVoices() {
    setLoading(true);
    try {
      const list = await api.providerVoices(editing ? cfg.id : null, cfg.base_url, key || null);
      setVoiceList(list);
      notify(`读到 ${list.length} 个声音`);
    } catch (e) {
      notify(`读取声音列表失败：${(e as Error).message}（可以照服务的文档手动填写）`, "error");
    } finally {
      setLoading(false);
    }
  }
  const sovits = cfg.service === "speech" && cfg.media.tts_api === "gpt_sovits";
  const sv = (() => {
    const [path = "", text = "", lang = "zh"] = cfg.media.voice.split("|").map((x) => x.trim());
    return { path, text, lang: lang || "zh" };
  })();
  const setSv = (patch: Partial<typeof sv>) => {
    const n = { ...sv, ...patch };
    setMedia("voice", [n.path, n.text, n.lang].join("|").replace(/\|zh$/, n.text ? "|zh" : "").replace(/\|+$/, ""));
  };

  function switchService(s: ServiceType) {
    const first = CLOUD_PRESETS.find((p) => p.service === s)!;
    setCfg((c) => ({ ...blankProvider(s), id: c.id }));
    applyPreset(first);
  }

  async function fetchModels() {
    setLoading(true);
    try {
      const list = await api.providerModels(editing ? cfg.id : null, cfg.base_url, key || null);
      setModels(list);
      notify(list.length ? `读到 ${list.length} 个模型` : "服务没有返回模型列表，请手动填写模型名");
    } catch (e) {
      notify(`读取模型列表失败：${(e as Error).message}`, "error");
    } finally {
      setLoading(false);
    }
  }

  async function save() {
    if (!local && !editing && !key.trim()) return notify("云端服务需要填写 API Key", "error");
    setSaving(true);
    try {
      const saved = await api.saveProvider({ ...cfg, needs_key: cfg.needs_key || !!key.trim() }, key.trim() || null);
      notify(editing ? "已保存" : "已添加，正在测试…");
      onSaved(saved);
      if (!editing) setCfg(blankProvider(cfg.service));
      setKey("");
    } catch (e) {
      notify((e as Error).message, "error");
    } finally {
      setSaving(false);
    }
  }

  const modelOptions = models ?? current?.models ?? [];
  return (
    <div className="cloud-form">
      {!editing && (
        <Segmented
          items={(["chat", "speech", "image", "transcription"] as ServiceType[]).map((s) => ({ key: s, label: SERVICE_LABEL[s] }))}
          value={cfg.service}
          onChange={switchService}
        />
      )}
      {!editing && (
        <div className="preset-grid">
          {presets.map((p) => (
            <button key={p.id} type="button" className={`preset${preset === p.id ? " active" : ""}`} onClick={() => applyPreset(p)}>
              {p.name}
            </button>
          ))}
        </div>
      )}
      {current?.note && <p className="hint">{current.note}</p>}
      <div className="grid2">
        <label>
          名称
          <input value={cfg.name} onChange={(e) => set("name", e.target.value)} placeholder="例如：DeepSeek" />
        </label>
        <label>
          地址（到 /v1 这一级）
          <input value={cfg.base_url} onChange={(e) => set("base_url", e.target.value.trim())} placeholder="https://api.example.com/v1" />
        </label>
        <label>
          API Key{editing && hasKey ? "（已保存；不填表示不修改）" : local ? "（本机服务一般不需要）" : ""}
          <input type="password" value={key} onChange={(e) => setKey(e.target.value)} autoComplete="off" placeholder={editing && hasKey ? "••••••••" : "sk-…"} />
        </label>
        <label>
          模型
          <span className="row">
            <input list="cloud-models" value={cfg.model} onChange={(e) => set("model", e.target.value)} placeholder="模型名" style={{ flex: 1 }} />
            <button type="button" className="small" disabled={!cfg.base_url || loading} onClick={() => void fetchModels()} title="读取服务的模型列表（不计费）">
              {loading ? "读取中…" : "获取模型列表"}
            </button>
          </span>
          <datalist id="cloud-models">
            {modelOptions.map((m) => (
              <option key={m} value={m} />
            ))}
          </datalist>
        </label>
      </div>
      {cfg.service === "speech" && (
        <div className="grid2">
          <label>
            接口类型
            <select
              value={cfg.media.tts_api === "gpt_sovits" ? "gpt_sovits" : ""}
              onChange={(e) => setCfg((c) => ({ ...c, model: e.target.value === "gpt_sovits" && !c.model ? "gpt-sovits" : c.model, media: { ...c.media, tts_api: e.target.value, voice: "", audio_format: e.target.value === "gpt_sovits" ? "wav" : c.media.audio_format } }))}
            >
              <option value="">OpenAI 兼容（/audio/speech）</option>
              <option value="gpt_sovits">GPT-SoVITS（api_v2 的 /tts）</option>
            </select>
          </label>
          {sovits && (
            <label>
              朗读文字的语言
              <select value={cfg.media.text_lang || "zh"} onChange={(e) => setMedia("text_lang", e.target.value)}>
                <option value="zh">中文</option>
                <option value="yue">粤语</option>
                <option value="en">英文</option>
                <option value="ja">日文</option>
                <option value="ko">韩文</option>
                <option value="auto">自动</option>
              </select>
            </label>
          )}
        </div>
      )}
      {sovits && (
        <div className="grid2">
          <label>
            旁白的参考音频（GPT-SoVITS 能读到的路径，3–10 秒）
            <input value={sv.path} onChange={(e) => setSv({ path: e.target.value.replace(/^"|"$/g, "") })} placeholder="例如 D:\GPT-SoVITS\参考\旁白.wav" />
          </label>
          <label>
            参考音频里说的话（可以不填，填了效果更好）
            <input value={sv.text} onChange={(e) => setSv({ text: e.target.value.replace(/\|/g, " ") })} />
          </label>
          <label>
            参考音频的语言
            <select value={sv.lang} onChange={(e) => setSv({ lang: e.target.value })}>
              <option value="zh">中文</option>
              <option value="yue">粤语</option>
              <option value="en">英文</option>
              <option value="ja">日文</option>
              <option value="ko">韩文</option>
            </select>
          </label>
          <label>
            分给 NPC 的声音（每行一个：参考音频路径|音频里说的话|语言）
            <textarea rows={3} value={cfg.media.voices.join("\n")} onChange={(e) => setMedia("voices", e.target.value.split(/\r?\n/).map((x) => x.trim()).filter(Boolean))} />
          </label>
        </div>
      )}
      {cfg.service === "speech" && !sovits && (
        <div className="grid2">
          <label>
            旁白的声音
            <span className="row">
              <input list="cloud-voices" value={cfg.media.voice} onChange={(e) => setMedia("voice", e.target.value)} placeholder="例如 alloy" style={{ flex: 1 }} />
              <button type="button" className="small" disabled={!cfg.base_url || loading} onClick={() => void fetchVoices()} title="读取服务里有哪些声音（不合成、不计费）">
                读取声音列表
              </button>
            </span>
            <datalist id="cloud-voices">
              {(voiceList ?? current?.voices ?? cfg.media.voices).map((v) => (
                <option key={v} value={v} />
              ))}
            </datalist>
          </label>
          <label>
            分给 NPC 的声音（逗号分隔；每个 NPC 固定用其中一个）
            <input value={cfg.media.voices.join(", ")} onChange={(e) => setMedia("voices", e.target.value.split(/[,，\s]+/).filter(Boolean))} />
          </label>
          <label>
            音频格式
            <select value={cfg.media.audio_format || "wav"} onChange={(e) => setMedia("audio_format", e.target.value)}>
              <option value="mp3">mp3（体积小，推荐云端）</option>
              <option value="wav">wav</option>
            </select>
          </label>
          <label className="check">
            <input type="checkbox" checked={cfg.media.send_instructions} onChange={(e) => setMedia("send_instructions", e.target.checked)} />
            发送声音描述（语气、性格）；服务不支持时关掉
          </label>
        </div>
      )}
      {cfg.service === "image" && (
        <div className="grid2">
          <label>
            默认尺寸
            <input list="cloud-sizes" value={cfg.media.image_size} onChange={(e) => setMedia("image_size", e.target.value)} placeholder="1024x1024" />
            <datalist id="cloud-sizes">
              {(current?.sizes ?? ["1024x1024"]).map((s) => (
                <option key={s} value={s} />
              ))}
            </datalist>
          </label>
          <label>
            超时（秒）
            <input type="number" min={30} max={600} value={cfg.timeout_secs} onChange={(e) => set("timeout_secs", Number(e.target.value))} />
          </label>
        </div>
      )}
      {!local && (
        <label className="check privacy">
          <input type="checkbox" checked={cfg.allow_campaign_content} onChange={(e) => set("allow_campaign_content", e.target.checked)} />
          允许把内容发送给这个云端服务（
          {cfg.service === "chat" ? "世界设定、剧本、战役叙述" : cfg.service === "speech" ? "要朗读的叙述与台词" : "图片描述（来自节点设定）"}
          ）。不勾选时，软件不会把这些内容发出去。
        </label>
      )}
      {!local && <p className="hint">云端服务按用量计费，费用由服务商收取；API Key 只保存在 Windows 凭据管理器里。</p>}
      <div className="row">
        <button className="primary" disabled={saving || !cfg.name.trim() || !cfg.base_url || !cfg.model.trim()} onClick={() => void save()}>
          {saving ? "保存中…" : editing ? "保存" : "添加并测试"}
        </button>
        {onCancel && <button onClick={onCancel}>取消</button>}
      </div>
    </div>
  );
}
