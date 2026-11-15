// 全局搜索（Ctrl+K）：世界树节点、剧本与条目、战役、任务、游戏记录。剧透模式关闭时不搜主持人信息。
import { useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api";
import type { Page } from "../App";
import type { GlobalHit } from "../bindings/GlobalHit";

const GROUP: [string, string][] = [
  ["node", "世界树"],
  ["script", "剧本"],
  ["script_item", "剧本条目"],
  ["campaign", "战役"],
  ["quest", "任务"],
  ["event", "游戏记录"],
];

export default function GlobalSearch({ worldId, go, onClose }: { worldId: string; go: (p: Page) => void; onClose: () => void }) {
  const [q, setQ] = useState("");
  const [hits, setHits] = useState<GlobalHit[] | null>(null);
  const [sel, setSel] = useState(0);
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => input.current?.focus(), []);
  useEffect(() => {
    if (!q.trim()) {
      setHits(null);
      return;
    }
    const t = setTimeout(() => {
      api
        .globalSearch(worldId, q)
        .then((h) => {
          setHits(h);
          setSel(0);
        })
        .catch(() => setHits([]));
    }, 220);
    return () => clearTimeout(t);
  }, [q, worldId]);
  const flat = useMemo(() => GROUP.flatMap(([k]) => (hits ?? []).filter((h) => h.kind === k)), [hits]);
  const open = (h: GlobalHit) => {
    if (h.kind === "node") go({ name: "tree", focus: h.id });
    else if (h.kind === "script") go({ name: "script", id: h.id });
    else if (h.kind === "script_item") go({ name: "script", id: h.parent });
    else go({ name: "game", campaign: h.parent });
    onClose();
  };
  return (
    <div className="gsearch-backdrop" onMouseDown={onClose}>
      <div className="gsearch card" onMouseDown={(e) => e.stopPropagation()} role="dialog" aria-label="全局搜索">
        <input
          ref={input}
          value={q}
          onChange={(e) => setQ(e.target.value)}
          placeholder="搜索世界树、剧本、战役、任务、游戏记录…（Esc 关闭）"
          onKeyDown={(e) => {
            if (e.key === "Escape") onClose();
            if (e.key === "ArrowDown") {
              e.preventDefault();
              setSel((s) => Math.min(s + 1, flat.length - 1));
            }
            if (e.key === "ArrowUp") {
              e.preventDefault();
              setSel((s) => Math.max(s - 1, 0));
            }
            if (e.key === "Enter" && flat[sel]) open(flat[sel]);
          }}
        />
        {hits && hits.length === 0 && <p className="hint">没有找到。</p>}
        <div className="gsearch-results">
          {GROUP.map(([k, label]) => {
            const list = (hits ?? []).filter((h) => h.kind === k);
            if (!list.length) return null;
            return (
              <div key={k} className="gsearch-group">
                <div className="side-label">{label}（{list.length}）</div>
                {list.map((h) => {
                  const i = flat.indexOf(h);
                  return (
                    <button key={h.kind + h.id} className={`gsearch-hit${i === sel ? " on" : ""}`} onMouseEnter={() => setSel(i)} onClick={() => open(h)}>
                      <strong>{h.title}</strong>
                      <small className="hint">{h.detail}</small>
                    </button>
                  );
                })}
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
