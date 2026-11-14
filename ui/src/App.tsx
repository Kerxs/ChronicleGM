import { lazy, Suspense, useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import Icon from "./components/Icon";
import TitleBar from "./components/TitleBar";
import Onboarding from "./components/Onboarding";
import UpdateDialog from "./components/UpdateDialog";
import { checkUpdates, openUpdate, useUpdates } from "./lib/updates";
import { TabbedPage } from "./components/Segmented";
import { api, type AppInfo, type FinishedEvent, type ProgressEvent, type WorldOverview } from "./api";
import type { WorldInfo } from "./bindings/WorldInfo";
const ScriptsPage = lazy(() => import("./pages/Scripts"));
const ScriptEditor = lazy(() => import("./pages/ScriptEditor"));
const ReviewPage = lazy(() => import("./pages/Review"));
const JobsPage = lazy(() => import("./pages/Jobs"));
const SettingsPage = lazy(() => import("./pages/Settings"));
const WorldTree = lazy(() => import("./pages/WorldTree"));
const WorldTools = lazy(() => import("./pages/WorldTools"));
const PlotEditor = lazy(() => import("./pages/PlotEditor"));
const GamePage = lazy(() => import("./pages/Game"));
const PerfPage = lazy(() => import("./pages/Perf"));
const ArchivePage = lazy(() => import("./pages/Archive"));
const NetworkPage = lazy(() => import("./pages/Network"));
const KnowledgeGraph = lazy(() => import("./pages/KnowledgeGraph"));

export type Page = { name: "game" } | { name: "tree"; focus?: string } | { name: "tools" } | { name: "scripts" } | { name: "script"; id: string } | { name: "plot"; scriptId: string } | { name: "review"; id?: string } | { name: "jobs" } | { name: "settings" } | { name: "perf" } | { name: "archive" } | { name: "network"; focus?: string } | { name: "graph"; focus?: string };

const PAGE_TITLE: Record<Page["name"], string> = {
  game: "游戏",
  archive: "战役档案馆",
  tree: "世界树",
  graph: "图谱 · 世界图谱",
  network: "图谱 · 人物关系",
  scripts: "剧本",
  script: "剧本",
  plot: "剧本 · 剧情分支",
  review: "审阅与任务 · 审阅台",
  jobs: "审阅与任务 · 生成任务",
  tools: "世界工具",
  settings: "模型与性能 · 模型服务",
  perf: "模型与性能 · 性能中心",
};
const GROUP: Record<Page["name"], string> = {
  game: "game", archive: "archive", tree: "tree", graph: "graph", network: "graph", scripts: "scripts", script: "script", plot: "plot",
  review: "review", jobs: "review", tools: "tools", settings: "settings", perf: "settings",
};

export interface Ctx {
  worldId: string;
  go: (p: Page) => void;
  notify: (msg: string, kind?: "info" | "error") => void;
  /** 每当后台任务结束或数据变化时递增，页面据此刷新。 */
  tick: number;
  bump: () => void;
  progress: Record<string, number>;
}

/** 侧栏导航按钮：图标 + 文字（文字保持原样，便于走查脚本按名称点击），可带数字角标。 */
function NavButton({ icon, label, active, onClick, count = 0, running = false }: { icon: string; label: string; active: boolean; onClick: () => void; count?: number; running?: boolean }) {
  return (
    <button className={active ? "active" : ""} onClick={onClick} title={label}>
      <Icon name={icon} />
      <span className="nav-label">{label}</span>
      {count > 0 && <span className={running ? "count run" : "count"} aria-label={running ? `${count} 个进行中` : `${count} 个待审`}>{count}</span>}
    </button>
  );
}

export default function App() {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [fatal, setFatal] = useState<string | null>(null);
  const [worlds, setWorlds] = useState<WorldInfo[]>([]);
  const [worldId, setWorldId] = useState<string | null>(null);
  const [overview, setOverview] = useState<WorldOverview | null>(null);
  const [page, setPage] = useState<Page>({ name: "scripts" });
  // 使用引导：第一次打开时显示。是否看过、侧栏是否收起，存在资料库里（跟着数据目录走，
  // 不用浏览器存储：WebView2 的存储是所有数据目录共用的）。
  const [guide, setGuide] = useState(false);
  const [collapsed, setCollapsed] = useState(false);
  // 启动后自动检查一次更新（可以在「关于」里关闭）；检查失败不打扰（关于卡片里能看到原因）。
  const updates = useUpdates();
  useEffect(() => {
    const t = setTimeout(() => {
      api
        .uiPref("ui.auto_update")
        .then((v) => {
          if (v !== "0") void checkUpdates(false);
        })
        .catch(() => {});
    }, 4000);
    return () => clearTimeout(t);
  }, []);
  useEffect(() => {
    api.uiPref("ui.onboarded").then((v) => setGuide(v !== "1")).catch(() => {});
    api.uiPref("ui.nav_collapsed").then((v) => setCollapsed(v === "1")).catch(() => {});
  }, []);
  const closeGuide = () => {
    setGuide(false);
    void api.setUiPref("ui.onboarded", "1").catch(() => {});
  };
  const [toast, setToast] = useState<{ msg: string; kind: "info" | "error" } | null>(null);
  const [tick, setTick] = useState(0);
  const [progress, setProgress] = useState<Record<string, number>>({});

  const notify = useCallback((msg: string, kind: "info" | "error" = "info") => setToast({ msg, kind }), []);
  const bump = useCallback(() => setTick((t) => t + 1), []);

  const refreshWorlds = useCallback(async () => {
    try {
      setWorlds(await api.listWorlds());
    } catch (e) {
      notify((e as Error).message, "error");
    }
  }, [notify]);

  useEffect(() => {
    api
      .startupError()
      .then((e) => {
        if (e) setFatal(e);
        else return api.appInfo().then(setInfo);
      })
      .catch((e: Error) => notify(e.message, "error"));
    void refreshWorlds();
  }, [refreshWorlds, notify]);

  useEffect(() => {
    if (!worldId) return;
    api.worldOverview(worldId).then(setOverview).catch((e: Error) => notify(e.message, "error"));
  }, [worldId, tick, notify]);

  // 后台生成任务的进度与完成事件。
  useEffect(() => {
    const un1 = listen<ProgressEvent>("gen-progress", (e) => setProgress((p) => ({ ...p, [e.payload.job_id]: e.payload.chars })));
    const un2 = listen<FinishedEvent>("gen-finished", (e) => {
      setProgress((p) => {
        const n = { ...p };
        delete n[e.payload.job_id];
        return n;
      });
      if (e.payload.ok) notify("生成完成，请到「审阅台」确认后再写入");
      else notify(`生成失败：${e.payload.error ?? "未知错误"}`, "error");
      bump();
    });
    return () => {
      void un1.then((f) => f());
      void un2.then((f) => f());
    };
  }, [notify, bump]);

  useEffect(() => {
    if (!toast) return;
    const t = setTimeout(() => setToast(null), toast.kind === "error" ? 9000 : 4000);
    return () => clearTimeout(t);
  }, [toast]);

  async function create(name: string): Promise<boolean> {
    try {
      const w = await api.createWorld(name);
      await refreshWorlds();
      setWorldId(w.id);
      setPage({ name: "scripts" });
      return true;
    } catch (e) {
      notify((e as Error).message, "error");
      return false;
    }
  }

  function toggleNav() {
    const next = !collapsed;
    setCollapsed(next);
    void api.setUiPref("ui.nav_collapsed", next ? "1" : "0").catch(() => {});
  }

  const ctx: Ctx | null = worldId ? { worldId, go: setPage, notify, tick, bump, progress } : null;
  const running = Object.keys(progress).length;

  if (fatal) {
    return (
      <div className="fatal-wrap">
      {/* 窗口没有系统边框：失败页也要能拖动、最小化和关闭。 */}
      <TitleBar worlds={[]} worldId={null} pageTitle="启动失败" running={0} pending={0} onSelectWorld={() => {}} onCreateWorld={async () => false} onOpenJobs={() => {}} onOpenReview={() => {}} />
      <div className="fatal">
        <h2>Chronicle GM 无法启动</h2>
        <pre>{fatal}</pre>
        <p className="hint">请把上面的信息和日志文件一起反馈。修复问题后重新打开程序即可。</p>
      </div>
      </div>
    );
  }

  const pending = overview?.pending_changesets ?? 0;
  const is = (...names: Page["name"][]) => names.includes(page.name);

  return (
    <div className={`app${collapsed ? " nav-collapsed" : ""}`}>
      <TitleBar
        worlds={worlds}
        worldId={worldId}
        pageTitle={worldId || page.name === "settings" || page.name === "perf" ? PAGE_TITLE[page.name] : "欢迎"}
        running={running}
        pending={pending}
        onSelectWorld={(id) => {
          setWorldId(id);
          setPage({ name: "tree" });
        }}
        onCreateWorld={create}
        onOpenJobs={() => setPage({ name: "jobs" })}
        onOpenReview={() => setPage({ name: "review" })}
        onOpenGuide={() => setGuide(true)}
        updateVersion={updates.info?.version ?? null}
        onOpenUpdate={openUpdate}
      />
      <UpdateDialog />
      {guide && !fatal && (
        <Onboarding
          worlds={worlds}
          onCreateWorld={create}
          onSelectWorld={(id) => {
            setWorldId(id);
            setPage({ name: "tree" });
          }}
          onOpenModels={() => {
            closeGuide();
            setPage({ name: "settings" });
          }}
          onClose={closeGuide}
        />
      )}
      <aside className="sidebar">
        {worldId ? (
          <nav className="nav">
            <div className="side-label">冒险</div>
            <NavButton icon="game" label="游戏" active={is("game")} onClick={() => setPage({ name: "game" })} />
            <NavButton icon="archive" label="战役档案馆" active={is("archive")} onClick={() => setPage({ name: "archive" })} />
            <div className="side-label">创作</div>
            <NavButton icon="tree" label="世界树" active={is("tree")} onClick={() => setPage({ name: "tree" })} />
            <NavButton icon="graph" label="图谱" active={is("graph", "network")} onClick={() => setPage({ name: "graph" })} />
            <NavButton icon="script" label="剧本" active={is("scripts", "script", "plot")} onClick={() => setPage({ name: "scripts" })} />
            <NavButton icon="review" label="审阅与任务" count={pending + running} running={running > 0 && pending === 0} active={is("review", "jobs")} onClick={() => setPage(pending === 0 && running > 0 ? { name: "jobs" } : { name: "review" })} />
            <NavButton icon="tools" label="世界工具" active={is("tools")} onClick={() => setPage({ name: "tools" })} />
          </nav>
        ) : (
          <p className="hint nav-empty">在顶部选择或新建一个世界。</p>
        )}
        <nav className="nav nav-bottom">
          <div className="side-label">系统</div>
          <NavButton icon="models" label="模型与性能" active={is("settings", "perf")} onClick={() => setPage({ name: "settings" })} />
          <button className="nav-toggle" onClick={toggleNav} title={collapsed ? "展开侧栏" : "收起侧栏"} aria-label={collapsed ? "展开侧栏" : "收起侧栏"}>
            <svg viewBox="0 0 24 24" aria-hidden="true"><path d={collapsed ? "M9 6l6 6-6 6" : "M15 6l-6 6 6 6"} /></svg>
            <span className="nav-label">收起侧栏</span>
          </button>
        </nav>
      </aside>
      <main>
        {toast && (
          <div className={toast.kind === "error" ? "toast error" : "toast"} role="alert" onClick={() => setToast(null)}>
            {toast.msg}
          </div>
        )}
        <Suspense fallback={<div className="page-loading"><span className="tb-spinner" /></div>}>
        <div key={GROUP[page.name]} className="page-anim">
        {page.name === "settings" || page.name === "perf" ? (
          <TabbedPage
            title="模型与性能"
            items={[{ key: "settings", label: "模型服务" }, { key: "perf", label: "性能中心" }]}
            value={page.name}
            onChange={(k) => setPage({ name: k } as Page)}
          >
            {page.name === "settings" ? <SettingsPage notify={notify} info={info} onOpenPerf={() => setPage({ name: "perf" })} /> : <PerfPage notify={notify} worldId={worldId} onEditModels={() => setPage({ name: "settings" })} />}
          </TabbedPage>
        ) : !ctx ? (
          <div className="welcome">
            <div className="onb-orb" />
            <h2>欢迎使用 Chronicle GM</h2>
            <p className="hint">在顶部标题栏的世界菜单里选择或新建一个世界。</p>
            <button className="primary" onClick={() => setGuide(true)}>打开使用引导</button>
          </div>
        ) : page.name === "network" || page.name === "graph" ? (
          <TabbedPage
            title="图谱"
            items={[{ key: "graph", label: "世界图谱" }, { key: "network", label: "人物关系" }]}
            value={page.name}
            onChange={(k) => setPage({ name: k } as Page)}
          >
            {page.name === "network" ? (
              <NetworkPage key={page.focus ?? "net"} ctx={ctx} focus={page.focus} onOpenTree={(id) => setPage({ name: "tree", focus: id })} onOpenGraph={(id) => setPage({ name: "graph", focus: id })} />
            ) : (
              <KnowledgeGraph key={page.focus ?? "kg"} ctx={ctx} focus={page.focus} onOpenTree={(id) => setPage({ name: "tree", focus: id })} onOpenNetwork={(id) => setPage({ name: "network", focus: id })} />
            )}
          </TabbedPage>
        ) : page.name === "review" || page.name === "jobs" ? (
          <TabbedPage
            title="审阅与任务"
            items={[{ key: "review", label: "审阅台", count: pending }, { key: "jobs", label: "生成任务", count: running }]}
            value={page.name}
            onChange={(k) => setPage({ name: k } as Page)}
          >
            {page.name === "review" ? <ReviewPage ctx={ctx} initialId={page.id} /> : <JobsPage ctx={ctx} />}
          </TabbedPage>
        ) : page.name === "tree" ? (
          <WorldTree key={page.focus ?? "tree"} ctx={ctx} focus={page.focus} />
        ) : page.name === "archive" ? (
          <ArchivePage ctx={ctx} onOpenTools={() => setPage({ name: "tools" })} />
        ) : page.name === "tools" ? (
          <WorldTools
            ctx={ctx}
            onOpenNode={(id) => setPage({ name: "tree", focus: id })}
            onWorldImported={(id) => {
              void refreshWorlds();
              setWorldId(id);
              setPage({ name: "tree" });
            }}
          />
        ) : page.name === "scripts" ? (
          <ScriptsPage ctx={ctx} overview={overview} />
        ) : page.name === "game" ? (
          <GamePage ctx={ctx} />
        ) : page.name === "plot" ? (
          <PlotEditor ctx={ctx} scriptId={page.scriptId} />
        ) : page.name === "script" ? (
          <ScriptEditor ctx={ctx} scriptId={page.id} />
        ) : null}
        </div>
        </Suspense>
      </main>
    </div>
  );
}
