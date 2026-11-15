import GlobalSearch from "./components/GlobalSearch";
import { applyThumbSize } from "./lib/thumbs";
import { lazy, Suspense, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import Icon from "./components/Icon";
import TitleBar from "./components/TitleBar";
import Tutorial from "./components/Tutorial";
import UpdateDialog from "./components/UpdateDialog";
import { checkUpdates, openUpdate, useUpdates } from "./lib/updates";
import { TabbedPage } from "./components/Segmented";
import { dirBetween, viewTransition } from "./lib/motion";
import { PageSkeleton } from "./components/Skeleton";
import { api, type AppInfo, type FinishedEvent, type ProgressEvent, type WorldOverview } from "./api";
import type { WorldInfo } from "./bindings/WorldInfo";
const loadScriptsPage = () => import("./pages/Scripts");
const ScriptsPage = lazy(loadScriptsPage);
const loadScriptEditor = () => import("./pages/ScriptEditor");
const ScriptEditor = lazy(loadScriptEditor);
const loadReviewPage = () => import("./pages/Review");
const ReviewPage = lazy(loadReviewPage);
const loadJobsPage = () => import("./pages/Jobs");
const JobsPage = lazy(loadJobsPage);
const loadSettingsPage = () => import("./pages/Settings");
const SettingsPage = lazy(loadSettingsPage);
const loadWorldTree = () => import("./pages/WorldTree");
const WorldTree = lazy(loadWorldTree);
const loadWorldTools = () => import("./pages/WorldTools");
const WorldTools = lazy(loadWorldTools);
const loadPlotEditor = () => import("./pages/PlotEditor");
const PlotEditor = lazy(loadPlotEditor);
const loadGamePage = () => import("./pages/Game");
const GamePage = lazy(loadGamePage);
const loadPerfPage = () => import("./pages/Perf");
const PerfPage = lazy(loadPerfPage);
const loadArchivePage = () => import("./pages/Archive");
const ArchivePage = lazy(loadArchivePage);
const loadNetworkPage = () => import("./pages/Network");
const NetworkPage = lazy(loadNetworkPage);
const loadKnowledgeGraph = () => import("./pages/KnowledgeGraph");
const KnowledgeGraph = lazy(loadKnowledgeGraph);

export type Page = { name: "game"; campaign?: string } | { name: "tree"; focus?: string } | { name: "tools" } | { name: "scripts" } | { name: "script"; id: string } | { name: "plot"; scriptId: string } | { name: "review"; id?: string } | { name: "jobs" } | { name: "settings" } | { name: "perf" } | { name: "archive" } | { name: "network"; focus?: string } | { name: "graph"; focus?: string };

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

/** 导航顺序（决定页面切换时向左还是向右滑）。 */
const NAV_ORDER = ["game", "archive", "tree", "graph", "network", "scripts", "script", "plot", "review", "jobs", "tools", "settings", "perf"];
/** 鼠标停在导航上时预加载的页面代码。 */
const PRELOAD: Partial<Record<Page["name"], () => Promise<unknown>>> = {
  game: loadGamePage, archive: loadArchivePage, tree: loadWorldTree, graph: loadKnowledgeGraph, network: loadNetworkPage,
  scripts: loadScriptsPage, script: loadScriptEditor, plot: loadPlotEditor, review: loadReviewPage, jobs: loadJobsPage,
  tools: loadWorldTools, settings: loadSettingsPage, perf: loadPerfPage,
};
const preload = (n: Page["name"]) => void PRELOAD[n]?.().catch(() => {});

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
function NavButton({ icon, label, active, onClick, count = 0, running = false, prefetch }: { icon: string; label: string; active: boolean; onClick: () => void; count?: number; running?: boolean; prefetch?: () => void }) {
  return (
    <button className={active ? "active" : ""} onClick={onClick} title={label} onMouseEnter={prefetch} onFocus={prefetch} data-tour={`nav-${icon}`}>
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
  const [page, setPageRaw] = useState<Page>({ name: "scripts" });
  // 页面切换都经过这里：用 View Transition 做连贯的过渡（按导航顺序左右滑），同组页面（分段切换）也一样。
  const pageRef = useRef(page.name);
  pageRef.current = page.name;
  const setPage = useCallback((p: Page) => {
    viewTransition(() => setPageRaw(p), dirBetween(NAV_ORDER, pageRef.current, p.name));
  }, []);
  // 使用引导：第一次打开时显示。是否看过、侧栏是否收起，存在资料库里（跟着数据目录走，
  // 不用浏览器存储：WebView2 的存储是所有数据目录共用的）。
  // 游戏教程：null = 不显示；数字 = 从第几步开始。
  const [tour, setTour] = useState<number | null>(null);
  // 全局搜索（Ctrl+K）。
  const [searching, setSearching] = useState(false);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setSearching((x) => !x);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
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
    // 只在第一次打开软件时自动开始教程，开始时就记下"已经出现过"。
    // 以前暂停（或中途关掉软件）后每次启动都会再弹出来；现在暂停后要从标题栏的世界菜单「游戏教程」接着来。
    Promise.all([api.uiPref("ui.onboarded"), api.uiPref("ui.tutorial")])
      .then(([seen, at]) => {
        if (seen !== "1" && at !== "done") {
          setTour(Number(at) || 0);
          void api.setUiPref("ui.onboarded", "1").catch(() => {});
        }
      })
      .catch(() => {});
    api.uiPref("ui.nav_collapsed").then((v) => setCollapsed(v === "1")).catch(() => {});
    api.uiPref("ui.thumb_size").then(applyThumbSize).catch(() => {});
    api
      .dataDirInfo()
      .then((d) => d.warning && setTimeout(() => notify(d.warning!, "error"), 1500))
      .catch(() => {});
  }, []);
  const closeTour = (finished: boolean) => {
    setTour(null);
    if (finished) {
      void api.setUiPref("ui.onboarded", "1").catch(() => {});
      void api.setUiPref("ui.tutorial", "done").catch(() => {});
    } else {
      notify("教程已暂停，不会再自动出现。想接着看，点标题栏的世界菜单 →「游戏教程」。");
    }
  };
  const openTour = () => {
    api
      .uiPref("ui.tutorial")
      .then((at) => setTour(at && at !== "done" ? Number(at) || 0 : 0))
      .catch(() => setTour(0));
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

  // 侧栏选中指示块：量出当前选中项的位置，用 transform 移过去（弹簧曲线）。
  const sideRef = useRef<HTMLElement>(null);
  const [ind, setInd] = useState<{ y: number; h: number } | null>(null);
  useLayoutEffect(() => {
    const measure = () => {
      const side = sideRef.current;
      const el = side?.querySelector<HTMLElement>(".nav button.active");
      if (!side || !el) return setInd(null);
      const a = el.getBoundingClientRect();
      const b = side.getBoundingClientRect();
      setInd({ y: a.top - b.top + side.scrollTop, h: a.height });
    };
    measure();
    const ro = new ResizeObserver(measure);
    if (sideRef.current) ro.observe(sideRef.current);
    return () => ro.disconnect();
  }, [page.name, collapsed, worldId]);

  function toggleNav() {
    const next = !collapsed;
    viewTransition(() => setCollapsed(next));
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
        onOpenGuide={openTour}
        onOpenSearch={worldId ? () => setSearching(true) : undefined}
        updateVersion={updates.info?.version ?? null}
        onOpenUpdate={openUpdate}
      />
      <UpdateDialog />
      {searching && worldId && <GlobalSearch worldId={worldId} go={setPage} onClose={() => setSearching(false)} />}
      {tour !== null && !fatal && (
        <Tutorial
          worldId={worldId}
          startAt={tour}
          go={setPage}
          onSelectWorld={(id) => {
            void refreshWorlds();
            setWorldId(id);
          }}
          onClose={closeTour}
        />
      )}
      <aside className="sidebar" ref={sideRef}>
        {ind && <span className="nav-indicator" style={{ transform: `translateY(${ind.y}px)`, height: ind.h }} aria-hidden="true" />}
        {worldId ? (
          <nav className="nav">
            <div className="side-label">冒险</div>
            <NavButton icon="game" label="游戏" prefetch={() => preload("game")} active={is("game")} onClick={() => setPage({ name: "game" })} />
            <NavButton icon="archive" label="战役档案馆" prefetch={() => preload("archive")} active={is("archive")} onClick={() => setPage({ name: "archive" })} />
            <div className="side-label">创作</div>
            <NavButton icon="tree" label="世界树" prefetch={() => preload("tree")} active={is("tree")} onClick={() => setPage({ name: "tree" })} />
            <NavButton icon="graph" label="图谱" prefetch={() => preload("graph")} active={is("graph", "network")} onClick={() => setPage({ name: "graph" })} />
            <NavButton icon="script" label="剧本" prefetch={() => preload("scripts")} active={is("scripts", "script", "plot")} onClick={() => setPage({ name: "scripts" })} />
            <NavButton icon="review" label="审阅与任务" prefetch={() => preload("review")} count={pending + running} running={running > 0 && pending === 0} active={is("review", "jobs")} onClick={() => setPage(pending === 0 && running > 0 ? { name: "jobs" } : { name: "review" })} />
            <NavButton icon="tools" label="世界工具" prefetch={() => preload("tools")} active={is("tools")} onClick={() => setPage({ name: "tools" })} />
          </nav>
        ) : (
          <p className="hint nav-empty">在顶部选择或新建一个世界。</p>
        )}
        <nav className="nav nav-bottom">
          <div className="side-label">系统</div>
          <NavButton icon="models" label="模型与性能" prefetch={() => preload("settings")} active={is("settings", "perf")} onClick={() => setPage({ name: "settings" })} />
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
        <Suspense fallback={<PageSkeleton />}>
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
            <img className="app-icon-hero" src="/app-icon.png" alt="Chronicle GM" draggable={false} />
            <h2>欢迎使用 Chronicle GM</h2>
            <p className="hint">在顶部标题栏的世界菜单里选择或新建一个世界。</p>
            <button className="primary" onClick={openTour} data-tour="welcome-tour">开始游戏教程</button>
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
          <GamePage ctx={ctx} campaign={page.campaign} />
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
