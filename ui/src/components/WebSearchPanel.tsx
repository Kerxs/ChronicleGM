// 游戏里的手动联网搜索：结果只显示（标题、摘要、来源、时间、链接），不会写进剧本或战役。
import { useEffect, useState } from "react";
import { api } from "../api";
import type { SearchResult } from "../bindings/SearchResult";

export default function WebSearchPanel({ notify }: { notify: (m: string, k?: "info" | "error") => void }) {
  const [enabled, setEnabled] = useState(false);
  const [open, setOpen] = useState(false);
  const [q, setQ] = useState("");
  const [busy, setBusy] = useState(false);
  const [res, setRes] = useState<SearchResult[] | null>(null);
  useEffect(() => {
    api.searchSettings().then((v) => setEnabled(v.config.enabled && v.config.providers.length > 0)).catch(() => setEnabled(false));
  }, []);
  if (!enabled) return null;
  return (
    <div className="card web-search">
      <button className="link" onClick={() => setOpen(!open)} aria-expanded={open}>
        {open ? "▾" : "▸"} 联网查资料
      </button>
      {open && (
        <>
          <form
            className="row"
            onSubmit={(e) => {
              e.preventDefault();
              if (!q.trim()) return;
              setBusy(true);
              api
                .webSearch(q)
                .then(setRes)
                .catch((x: Error) => notify(x.message, "error"))
                .finally(() => setBusy(false));
            }}
          >
            <input value={q} onChange={(e) => setQ(e.target.value)} placeholder="搜索现实世界的资料（只显示，不会写进剧本）" />
            <button disabled={busy || !q.trim()}>{busy ? "搜索中…" : "搜索"}</button>
          </form>
          {res && res.length === 0 && <p className="hint">没有结果。</p>}
          {res && (
            <ul className="search-results">
              {res.map((r, i) => (
                <li key={i} style={{ animationDelay: `${i * 0.04}s` }}>
                  <strong>{r.title || r.url}</strong>
                  <small className="hint">
                    {" "}
                    {r.source}
                    {r.published && ` · ${r.published}`}
                  </small>
                  <div>{r.snippet}</div>
                  {r.url && (
                    <a href={r.url} target="_blank" rel="noreferrer noopener">
                      {r.url}
                    </a>
                  )}
                </li>
              ))}
            </ul>
          )}
        </>
      )}
    </div>
  );
}
