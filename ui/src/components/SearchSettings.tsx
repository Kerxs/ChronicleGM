// 联网搜索设置：可以配置多家搜索服务（Tavily / Brave Search / SearXNG），Key 自备、保存在 Windows 凭据管理器，默认关闭。
import { useEffect, useState } from "react";
import { api } from "../api";
import type { SearchConfig } from "../bindings/SearchConfig";
import type { SearchKind } from "../bindings/SearchKind";
import type { SearchProvider } from "../bindings/SearchProvider";

const KINDS: [SearchKind, string, string][] = [
  ["tavily", "Tavily", "需要 API Key（tavily.com）"],
  ["brave", "Brave Search", "需要 API Key（brave.com/search/api）"],
  ["searxng", "SearXNG", "自建或可信的实例地址，通常不需要 Key；实例要开启 JSON 格式"],
];

export default function SearchSettings({ notify }: { notify: (m: string, k?: "info" | "error") => void }) {
  const [cfg, setCfg] = useState<SearchConfig | null>(null);
  const [hasKey, setHasKey] = useState<string[]>([]);
  const [keys, setKeys] = useState<Record<string, string>>({});
  const [testQ, setTestQ] = useState("");
  const [testing, setTesting] = useState(false);
  const load = () =>
    void api
      .searchSettings()
      .then((v) => {
        setCfg(v.config);
        setHasKey(v.has_key);
        setKeys({});
      })
      .catch((e: Error) => notify(e.message, "error"));
  useEffect(load, []);
  if (!cfg) return null;
  const upd = (i: number, p: Partial<SearchProvider>) => setCfg({ ...cfg, providers: cfg.providers.map((x, j) => (j === i ? { ...x, ...p } : x)) });
  const add = (kind: SearchKind) => {
    const id = `${kind}-${Date.now().toString(36)}`;
    setCfg({ ...cfg, providers: [...cfg.providers, { id, kind, name: KINDS.find((k) => k[0] === kind)![1], base_url: "" }], provider_id: cfg.provider_id || id });
  };
  async function save() {
    if (!cfg) return;
    try {
      await api.saveSearchSettings(cfg, keys);
      notify("已保存联网搜索设置");
      load();
    } catch (e) {
      notify((e as Error).message, "error");
    }
  }
  return (
    <div className="card search-settings">
      <h3>联网搜索（可选）</h3>
      <p className="hint">默认关闭。打开后可以在游戏里手动搜索资料；也可以允许 GM 在需要现实世界的资料时自己搜索。网页内容只当作资料、标明来源，不会当成指令执行，也不会自动写进剧本。搜索词会发给你选的搜索服务。</p>
      <label className="check">
        <input type="checkbox" checked={cfg.enabled} onChange={(e) => setCfg({ ...cfg, enabled: e.target.checked })} />
        打开联网搜索
      </label>
      <label className="check">
        <input type="checkbox" checked={cfg.gm_can_search} disabled={!cfg.enabled} onChange={(e) => setCfg({ ...cfg, gm_can_search: e.target.checked })} />
        允许 GM 在回合中搜索（每小时最多
        <input type="number" min={1} max={100} value={cfg.per_hour} onChange={(e) => setCfg({ ...cfg, per_hour: Number(e.target.value) })} style={{ width: 56 }} />
        次）
      </label>
      <label className="check">
        每次最多
        <input type="number" min={1} max={10} value={cfg.max_results} onChange={(e) => setCfg({ ...cfg, max_results: Number(e.target.value) })} style={{ width: 56 }} />
        条结果
      </label>
      {cfg.providers.map((p, i) => (
        <div key={p.id} className="search-provider">
          <label className="check">
            <input type="radio" name="search-active" checked={cfg.provider_id === p.id} onChange={() => setCfg({ ...cfg, provider_id: p.id })} />
            使用
          </label>
          <span className="badge">{KINDS.find((k) => k[0] === p.kind)?.[1]}</span>
          <input value={p.name} onChange={(e) => upd(i, { name: e.target.value })} placeholder="名称" style={{ width: 120 }} />
          <input value={p.base_url} onChange={(e) => upd(i, { base_url: e.target.value })} placeholder={p.kind === "searxng" ? "地址，例如 http://127.0.0.1:8888" : "地址（留空用官方地址）"} />
          <input
            type="password"
            value={keys[p.id] ?? ""}
            onChange={(e) => setKeys({ ...keys, [p.id]: e.target.value })}
            placeholder={hasKey.includes(p.id) ? "已保存 Key（留空不改）" : p.kind === "searxng" ? "Key（可选）" : "API Key"}
            autoComplete="off"
          />
          <button className="small danger" onClick={() => setCfg({ ...cfg, providers: cfg.providers.filter((_, j) => j !== i), provider_id: cfg.provider_id === p.id ? "" : cfg.provider_id })}>
            移除
          </button>
        </div>
      ))}
      <div className="row wrap">
        {KINDS.map(([k, label, tip]) => (
          <button key={k} className="small" title={tip} onClick={() => add(k)}>
            添加 {label}
          </button>
        ))}
      </div>
      <div className="row">
        <button className="primary" onClick={() => void save()}>
          保存
        </button>
        <input value={testQ} onChange={(e) => setTestQ(e.target.value)} placeholder="保存后试搜一下（例如：明朝的驿站制度）" />
        <button
          disabled={testing || !testQ.trim()}
          onClick={() => {
            setTesting(true);
            api
              .webSearch(testQ)
              .then((r) => notify(r.length ? `搜索成功：${r.length} 条，第一条来自 ${r[0].source}` : "搜索成功，但没有结果"))
              .catch((e: Error) => notify(e.message, "error"))
              .finally(() => setTesting(false));
          }}
        >
          {testing ? "搜索中…" : "试搜"}
        </button>
      </div>
    </div>
  );
}
