// 自研标题栏（窗口没有系统边框）：拖动区、世界切换、当前页面、后台状态、最小化 / 最大化 / 关闭。
// 双击空白处最大化 / 还原（data-tauri-drag-region 自带）。
import { useEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { formatTime } from "../api";
import type { WorldInfo } from "../bindings/WorldInfo";

export type StudioPage = "tree" | "graph" | "scripts" | "review" | "tools" | "archive";
const STUDIO_PAGES: [StudioPage, string, string][] = [
  ["tree", "🌳", "世界树"],
  ["graph", "🕸️", "图谱"],
  ["scripts", "📜", "剧本"],
  ["review", "✅", "审阅与任务"],
  ["tools", "🔧", "世界工具"],
  ["archive", "🗄️", "战役档案馆"],
];

const win = (() => {
  try {
    return getCurrentWindow();
  } catch {
    return null; // 在普通浏览器里调试界面时没有窗口对象
  }
})();

export default function TitleBar({
  worlds,
  worldId,
  pageTitle,
  running,
  pending,
  onSelectWorld,
  onCreateWorld,
  onOpenJobs,
  onOpenReview,
  onOpenGuide,
  onDeleteWorld,
  onOpenSearch,
  updateVersion,
  onOpenUpdate,
  onOpenStudio,
}: {
  worlds: WorldInfo[];
  worldId: string | null;
  pageTitle: string;
  running: number;
  pending: number;
  onSelectWorld: (id: string) => void;
  onCreateWorld: (name: string) => Promise<boolean>;
  onOpenJobs: () => void;
  onOpenReview: () => void;
  onOpenGuide?: () => void;
  /** 删除世界（移到回收站）。 */
  onDeleteWorld?: (id: string) => void;
  onOpenSearch?: () => void;
  updateVersion?: string | null;
  onOpenUpdate?: () => void;
  /** 大厅 / 游戏空间里打开创作页面（世界树、剧本……）。 */
  onOpenStudio?: (page: StudioPage) => void;
}) {
  const [studioOpen, setStudioOpen] = useState(false);
  const studioRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!studioOpen) return;
    const close = (e: MouseEvent) => {
      if (studioRef.current && !studioRef.current.contains(e.target as Node)) setStudioOpen(false);
    };
    const esc = (e: KeyboardEvent) => e.key === "Escape" && setStudioOpen(false);
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", esc);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", esc);
    };
  }, [studioOpen]);
  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");
  const [maxed, setMaxed] = useState(false);
  const menu = useRef<HTMLDivElement>(null);
  const current = worlds.find((w) => w.id === worldId);

  useEffect(() => {
    if (!win) return;
    const sync = () => void win.isMaximized().then(setMaxed).catch(() => {});
    sync();
    const un = win.onResized(sync);
    return () => void un.then((f) => f());
  }, []);

  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => {
      if (menu.current && !menu.current.contains(e.target as Node)) setOpen(false);
    };
    const esc = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", esc);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", esc);
    };
  }, [open]);

  return (
    <header className="titlebar" data-tauri-drag-region>
      <div className="tb-brand" data-tauri-drag-region>
        <img className="tb-icon" src="/app-icon.png" alt="" draggable={false} />
        <span className="tb-name" data-tauri-drag-region>
          Chronicle GM
        </span>
      </div>
      <div className="tb-world" ref={menu}>
        <button className={`tb-world-btn${open ? " open" : ""}`} onClick={() => setOpen((x) => !x)} title="切换或新建世界">
          <span className="tb-world-label">{current ? current.name : "选择世界"}</span>
          <svg viewBox="0 0 12 12" aria-hidden="true">
            <path d="M3 4.5 6 7.5 9 4.5" />
          </svg>
        </button>
        {open && (
          <div className="tb-menu" role="menu">
            <div className="tb-menu-label">世界</div>
            <ul className="worlds">
              {worlds.length === 0 && <li className="empty">还没有世界</li>}
              {worlds.map((w) => (
                <li key={w.id} className="tb-world-row">
                  <button
                    className={worldId === w.id ? "active" : ""}
                    onClick={() => {
                      onSelectWorld(w.id);
                      setOpen(false);
                    }}
                  >
                    <span>{w.name}</span>
                    <small>{formatTime(w.last_opened_at ?? w.created_at)}</small>
                  </button>
                  {onDeleteWorld && (
                    <button
                      className="row-del"
                      title="删除这个世界（移到回收站）"
                      aria-label={`删除世界「${w.name}」`}
                      onClick={() => {
                        setOpen(false);
                        onDeleteWorld(w.id);
                      }}
                    >
                      🗑
                    </button>
                  )}
                </li>
              ))}
            </ul>
            <form
              className="create"
              onSubmit={(e) => {
                e.preventDefault();
                if (!name.trim()) return;
                void onCreateWorld(name.trim()).then((ok) => {
                  if (ok) {
                    setName("");
                    setOpen(false);
                  }
                });
              }}
            >
              <input value={name} onChange={(e) => setName(e.target.value)} placeholder="新世界名称" maxLength={100} />
              <button type="submit" className="primary" disabled={!name.trim()}>
                新建
              </button>
            </form>
            {onOpenGuide && (
              <button
                className="tb-menu-item"
                onClick={() => {
                  setOpen(false);
                  onOpenGuide();
                }}
              >
                游戏教程
              </button>
            )}
          </div>
        )}
      </div>
      <span className="tb-sep" data-tauri-drag-region>
        /
      </span>
      <span key={pageTitle} className="tb-page" data-tauri-drag-region>
        {pageTitle}
      </span>
      {onOpenStudio && (
        <div className="tb-world tb-studio" ref={studioRef}>
          <button className={`tb-world-btn${studioOpen ? " open" : ""}`} onClick={() => setStudioOpen((x) => !x)} title="打开世界树、剧本、档案馆等创作页面">
            <span className="tb-world-label">✏️ 创作</span>
            <svg viewBox="0 0 12 12" aria-hidden="true">
              <path d="M3 4.5 6 7.5 9 4.5" />
            </svg>
          </button>
          {studioOpen && (
            <div className="tb-menu tb-studio-menu" role="menu">
              <div className="tb-menu-label">创作</div>
              {STUDIO_PAGES.map(([k, ic, label]) => (
                <button
                  key={k}
                  className="tb-menu-item"
                  role="menuitem"
                  onClick={() => {
                    setStudioOpen(false);
                    onOpenStudio(k);
                  }}
                >
                  <span className="ic">{ic}</span> {label}
                </button>
              ))}
            </div>
          )}
        </div>
      )}
      <div className="tb-fill" data-tauri-drag-region />
      {onOpenSearch && (
        <button className="tb-status tb-search" onClick={onOpenSearch} title="全局搜索（Ctrl+K）" data-tour="global-search">
          🔍 搜索 <kbd>Ctrl K</kbd>
        </button>
      )}
      {updateVersion && (
        <button className="tb-status update" onClick={onOpenUpdate} title="有新版本可以更新">
          新版本 {updateVersion}
        </button>
      )}
      {running > 0 && (
        <button className="tb-status running" onClick={onOpenJobs} title="后台生成任务">
          <span className="tb-spinner" />
          生成中 {running}
        </button>
      )}
      {pending > 0 && (
        <button className="tb-status" onClick={onOpenReview} title="等待你确认的 AI 修改">
          待审阅 {pending}
        </button>
      )}
      <div className="tb-controls">
        <button className="tb-ctl" aria-label="最小化" title="最小化" onClick={() => void win?.minimize()}>
          <svg viewBox="0 0 10 10"><path d="M1 5h8" /></svg>
        </button>
        <button className="tb-ctl" aria-label={maxed ? "还原" : "最大化"} title={maxed ? "还原" : "最大化"} onClick={() => void win?.toggleMaximize()}>
          {maxed ? (
            <svg viewBox="0 0 10 10"><path d="M3 1h6v6M1 3h6v6H1z" /></svg>
          ) : (
            <svg viewBox="0 0 10 10"><path d="M1 1h8v8H1z" /></svg>
          )}
        </button>
        <button className="tb-ctl close" aria-label="关闭" title="关闭" onClick={() => void win?.close()}>
          <svg viewBox="0 0 10 10"><path d="M1 1l8 8M9 1 1 9" /></svg>
        </button>
      </div>
    </header>
  );
}
