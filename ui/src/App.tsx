import GlobalSearch from "./components/GlobalSearch";
import { applyThumbSize } from "./lib/thumbs";
import { lazy, Suspense, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import Icon from "./components/Icon";
import TitleBar from "./components/TitleBar";
import Tutorial from "./components/Tutorial";
import { applyZoom } from "./lib/zoom";
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
const loadLobby = () => import("./pages/Lobby");
const Lobby = lazy(loadLobby);
const Multiplayer = lazy(() => import("./pages/Multiplayer"));
const LocalModels = lazy(() => import("./pages/LocalModels"));
const GeneralSettings = lazy(() => import("./pages/GeneralSettings"));

/** 三种模式：大厅（默认）、创作（原来带侧栏的编辑界面）、游戏空间（全屏玩一局）。 */
type Mode = { name: "lobby" } | { name: "studio" } | { name: "mp" } | { name: "play"; campaign: string; flyFrom: DOMRect | null; opening: boolean };

export type Page = { name: "game"; campaign?: string } | { name: "tree"; focus?: string } | { name: "tools" } | { name: "scripts" } | { name: "script"; id: string } | { name: "plot"; scriptId: string } | { name: "review"; id?: string } | { name: "jobs" } | { name: "general" } | { name: "local" } | { name: "settings" } | { name: "cloud" } | { name: "perf" } | { name: "archive" } | { name: "network"; focus?: string } | { name: "graph"; focus?: string };

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
  general: "设置 · 通用",
  local: "设置 · 本地模型",
  settings: "设置 · 模型服务",
  cloud: "设置 · 云端模型",
  perf: "设置 · 性能中心",
};
const GROUP: Record<Page["name"], string> = {
  game: "game", archive: "archive", tree: "tree", graph: "graph", network: "graph", scripts: "scripts", script: "script", plot: "plot",
  review: "review", jobs: "review", tools: "tools", general: "settings", local: "settings", settings: "settings", cloud: "settings", perf: "settings",
};

/** 导航顺序（决定页面切换时向左还是向右滑）。 */
const NAV_ORDER = ["game", "archive", "tree", "graph", "network", "scripts", "script", "plot", "review", "jobs", "tools", "general", "local", "settings", "cloud", "perf"];
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
  const [mode, setModeRaw] = useState<Mode>({ name: "lobby" });
  const setMode = useCallback((m: Mode) => viewTransition(() => setModeRaw(m)), []);
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
  // 左侧导航默认收起（用户展开过就记住）。
  const [collapsed, setCollapsed] = useState(true);
  // 界面大小（设置 → 通用）。
  useEffect(() => {
    api.uiPref("ui.zoom").then(applyZoom).catch(() => {});
  }, []);
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
    api.uiPref("ui.nav_collapsed").then((v) => setCollapsed(v !== "0")).catch(() => {});
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

  // 删除世界：移到回收站（世界工具里可以恢复）。删的是正在打开的世界时重新载入界面。
  async function deleteWorld(id: string) {
    const w = worlds.find((x) => x.id === id);
    if (!w || !window.confirm(`删除世界「${w.name}」？
世界文件会移到回收站，可以在「世界工具 → 删除与回收站」里恢复。`)) return;
    try {
      await api.trashWorld(id);
      notify(`「${w.name}」已移到回收站`);
      if (id === worldId) location.reload();
      else await refreshWorlds();
    } catch (e) {
      notify((e as Error).message, "error");
    }
  }

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

  // 在大厅或游戏空间里跳到某个编辑页面（例如从游戏里打开世界树）：切到创作模式。
  const goStudio = useCallback(
    (p: Page) => {
      setModeRaw({ name: "studio" });
      setPage(p);
    },
    [setPage],
  );
  // 进入游戏：只有游戏空间这一个游戏界面（老的创作模式游戏页已去掉）。
  // 给了战役就打开它；没给就打开最近玩的那一局；一局都没有就回大厅开新游戏。
  const enterGame = useCallback(
    (campaign?: string) => {
      if (campaign) return setModeRaw({ name: "play", campaign, flyFrom: null, opening: false });
      if (!worldId) return setModeRaw({ name: "lobby" });
      api
        .listCampaigns(worldId)
        .then((cs) => {
          const c = cs.filter((x) => !x.archived).sort((a, b) => b.updated_at - a.updated_at)[0];
          setModeRaw(c ? { name: "play", campaign: c.id, flyFrom: null, opening: false } : { name: "lobby" });
        })
        .catch(() => setModeRaw({ name: "lobby" }));
    },
    [worldId],
  );
  // 打开软件时：设置为「继续上次的游戏」就直接进入最近的一局（只在启动后做一次）。
  const startHandled = useRef(false);
  useEffect(() => {
    if (startHandled.current || !worldId) return;
    startHandled.current = true;
    api
      .uiPref("ui.start_page")
      .then((v) => {
        if (v === "continue" && tour === null) enterGame();
      })
      .catch(() => {});
  }, [worldId, enterGame, tour]);
  // 创作模式里有地方要打开游戏（搜索结果、档案馆……）：转到游戏空间。
  useEffect(() => {
    if (mode.name === "studio" && page.name === "game") {
      enterGame(page.campaign);
      setPageRaw({ name: "scripts" });
    }
  }, [mode.name, page, enterGame]);
  const goAny = useCallback((p: Page) => (p.name === "game" ? enterGame(p.campaign) : mode.name === "studio" ? setPage(p) : goStudio(p)), [enterGame, mode.name, setPage, goStudio]);
  const ctx: Ctx | null = worldId ? { worldId, go: goAny, notify, tick, bump, progress } : null;
  // 教程在大厅、游戏空间和创作模式之间带路。
  const tourGo = useCallback((p: Page | { name: "lobby" }) => (p.name === "lobby" ? setModeRaw({ name: "lobby" }) : goAny(p)), [goAny]);
  const tourEl =
    tour !== null && !fatal ? (
      <Tutorial
        worldId={worldId}
        startAt={tour}
        go={tourGo}
        onSelectWorld={(id) => {
          void refreshWorlds();
          setWorldId(id);
        }}
        onClose={closeTour}
      />
    ) : null;
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
  const toastEl = toast && (
    <div className={toast.kind === "error" ? "toast error" : "toast"} role="alert" onClick={() => setToast(null)}>
      {toast.msg}
    </div>
  );

  if (mode.name !== "studio") {
    return (
      <div className={`app ${mode.name}-mode`}>
        <TitleBar
          worlds={worlds}
          worldId={worldId}
          pageTitle={mode.name === "play" ? "游戏" : mode.name === "mp" ? "联机" : "大厅"}
          running={running}
          pending={pending}
          onSelectWorld={(id) => {
            setWorldId(id);
            setModeRaw({ name: "lobby" });
          }}
          onCreateWorld={create}
          onDeleteWorld={(id) => void deleteWorld(id)}
          onOpenJobs={() => goStudio({ name: "jobs" })}
          onOpenReview={() => goStudio({ name: "review" })}
          onOpenGuide={openTour}
          onOpenSearch={worldId ? () => setSearching(true) : undefined}
          updateVersion={updates.info?.version ?? null}
          onOpenUpdate={openUpdate}
          onOpenStudio={worldId && mode.name !== "mp" ? (p) => goStudio({ name: p } as Page) : undefined}
        />
        <UpdateDialog />
        {tourEl}
        {searching && worldId && <GlobalSearch worldId={worldId} go={goAny} onClose={() => setSearching(false)} />}
        <main className="mode-main">
          {toastEl}
          <Suspense fallback={<PageSkeleton />}>
            {mode.name === "lobby" ? (
              <Lobby
                ctx={ctx}
                worlds={worlds}
                notify={notify}
                onSelectWorld={(id) => {
                  void refreshWorlds();
                  setWorldId(id);
                }}
                onEnterGame={(campaign, flyFrom, opening) => setModeRaw({ name: "play", campaign, flyFrom, opening })}
                onOpenStudio={(p) => goStudio(p === "settings" ? { name: "local" } : p === "general" ? { name: "general" } : p === "tree" ? { name: "tree" } : { name: "scripts" })}
                onOpenMultiplayer={() => setMode({ name: "mp" })}
              />
            ) : mode.name === "mp" ? (
              <Multiplayer worldId={worldId} notify={notify} onExit={() => setMode({ name: "lobby" })} />
            ) : ctx && mode.name === "play" ? (
              <GamePage key={mode.campaign} ctx={ctx} campaign={mode.campaign} immersive flyFrom={mode.flyFrom} autoOpening={mode.opening} onExit={() => setMode({ name: "lobby" })} />
            ) : null}
          </Suspense>
        </main>
      </div>
    );
  }

  return (
    <div className={`app${collapsed ? " nav-collapsed" : ""}`}>
      <TitleBar
        worlds={worlds}
        worldId={worldId}
        pageTitle={worldId || page.name === "general" || page.name === "local" || page.name === "cloud" || page.name === "settings" || page.name === "perf" ? PAGE_TITLE[page.name] : "欢迎"}
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
      {tourEl}
      <aside className="sidebar" ref={sideRef}>
        {ind && <span className="nav-indicator" style={{ transform: `translateY(${ind.y}px)`, height: ind.h }} aria-hidden="true" />}
        <nav className="nav">
          <NavButton icon="home" label="返回大厅" prefetch={() => void loadLobby()} active={false} onClick={() => setMode({ name: "lobby" })} />
        </nav>
        {worldId ? (
          <nav className="nav">
            <div className="side-label">冒险</div>
            <NavButton icon="game" label="游戏" prefetch={() => preload("game")} active={is("game")} onClick={() => enterGame()} />
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
          <NavButton icon="models" label="设置" prefetch={() => preload("settings")} active={is("general", "local", "settings", "cloud", "perf")} onClick={() => setPage({ name: "general" })} />
          <button className="nav-toggle" onClick={toggleNav} title={collapsed ? "展开侧栏" : "收起侧栏"} aria-label={collapsed ? "展开侧栏" : "收起侧栏"}>
            <svg viewBox="0 0 24 24" aria-hidden="true"><path d={collapsed ? "M9 6l6 6-6 6" : "M15 6l-6 6 6 6"} /></svg>
            <span className="nav-label">收起侧栏</span>
          </button>
        </nav>
      </aside>
      <main>
        {toastEl}
        <Suspense fallback={<PageSkeleton />}>
        <div key={GROUP[page.name]} className="page-anim">
        {page.name === "general" || page.name === "local" || page.name === "settings" || page.name === "cloud" || page.name === "perf" ? (
          <TabbedPage
            title="设置"
            items={[{ key: "general", label: "通用" }, { key: "local", label: "本地模型" }, { key: "settings", label: "模型服务" }, { key: "cloud", label: "云端模型" }, { key: "perf", label: "性能中心" }]}
            value={page.name}
            onChange={(k) => setPage({ name: k } as Page)}
          >
            {page.name === "general" ? <GeneralSettings notify={notify} /> : page.name === "local" ? <LocalModels notify={notify} /> : page.name === "cloud" ? <SettingsPage notify={notify} info={info} view="cloud" /> : page.name === "settings" ? <SettingsPage notify={notify} info={info} onOpenPerf={() => setPage({ name: "perf" })} /> : <PerfPage notify={notify} worldId={worldId} onEditModels={() => setPage({ name: "settings" })} />}
          </TabbedPage>
        ) : !ctx ? (
          <StudioStart
            worlds={worlds}
            onOpen={(id) => {
              setWorldId(id);
              setPage({ name: "scripts" });
            }}
            onCreate={create}
            onDelete={(id) => void deleteWorld(id)}
            onDemo={() =>
              void api
                .openDemoWorld()
                .then(async (w) => {
                  await refreshWorlds();
                  setWorldId(w.id);
                  setPage({ name: "scripts" });
                })
                .catch((e: Error) => notify(e.message, "error"))
            }
            onTour={openTour}
          />
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
          <PageSkeleton />
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

/** 创作模式里还没选世界时的开始页：点世界卡片进入，或者新建世界、打开示例世界、看教程。 */
function StudioStart({ worlds, onOpen, onCreate, onDelete, onDemo, onTour }: { worlds: WorldInfo[]; onOpen: (id: string) => void; onCreate: (name: string) => Promise<boolean>; onDelete: (id: string) => void; onDemo: () => void; onTour: () => void }) {
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const sorted = [...worlds].sort((a, b) => (b.last_opened_at ?? b.updated_at) - (a.last_opened_at ?? a.updated_at));
  return (
    <div className="studio-start">
      <div className="ss-head">
        <img className="app-icon-hero" src="/app-icon.png" alt="Chronicle GM" draggable={false} />
        <div>
          <h2>创作模式</h2>
          <p className="hint">在这里搭建世界树、写剧本、审阅 AI 的修改。先选一个世界，或者新建一个。</p>
        </div>
      </div>
      {sorted.length > 0 && (
        <>
          <h3>我的世界</h3>
          <div className="ss-worlds">
            {sorted.map((w, i) => (
              <div key={w.id} className="ss-world-row" style={{ animationDelay: `${i * 0.04}s` }}>
                <button className="ss-world" onClick={() => onOpen(w.id)}>
                  <span className="ss-world-ic">🌍</span>
                  <span>
                    <strong>{w.name}</strong>
                    <small>上次打开：{w.last_opened_at ? new Date(w.last_opened_at).toLocaleString() : "从未"}</small>
                  </span>
                </button>
                <button className="row-del" title="删除这个世界（移到回收站）" aria-label={`删除世界「${w.name}」`} onClick={() => onDelete(w.id)}>
                  🗑
                </button>
              </div>
            ))}
          </div>
        </>
      )}
      <h3>开始</h3>
      <div className="ss-actions">
        <form
          className="ss-card ss-new"
          onSubmit={(e) => {
            e.preventDefault();
            if (!name.trim() || busy) return;
            setBusy(true);
            void onCreate(name.trim()).finally(() => setBusy(false));
          }}
        >
          <strong>✨ 新建世界</strong>
          <input value={name} onChange={(e) => setName(e.target.value)} placeholder="给世界起个名字，例如：雾湾" maxLength={60} />
          <button className="primary" type="submit" disabled={!name.trim() || busy}>
            {busy ? "正在创建…" : "创建"}
          </button>
        </form>
        <button className="ss-card" onClick={onDemo}>
          <strong>🏮 打开示例世界</strong>
          <small>「雾湾」：一座熄灭的灯塔、守塔老人和两个可选角色，打开就能看和玩。</small>
        </button>
        <button className="ss-card" onClick={onTour} data-tour="welcome-tour">
          <strong>🎓 游戏教程</strong>
          <small>GM 带你走一遍：配置模型、开一局游戏、做几个行动。</small>
        </button>
      </div>
    </div>
  );
}
